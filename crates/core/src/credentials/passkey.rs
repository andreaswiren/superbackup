//! Unlocking with a passkey: the sealed half that lives on this machine.
//!
//! [`crate::platform::webauthn`] turns a touch and a PIN into 32 bytes. This
//! turns those 32 bytes into an unlocked vault, and it does so the same way
//! the platform-keychain cache does, because it is the same problem:
//!
//! ```text
//!   the authenticator                data/passkeys/<id>.sbvault
//!   ─────────────────                ──────────────────────────
//!   hmac-secret(salt) ──unwraps──▶   a vault holding one entry:
//!   32 bytes, never stored           the master passphrase
//! ```
//!
//! # What is on disk, and what it is worth without the key
//!
//! `passkeys.json` holds the credential id, the salt, a label and a date.
//! None of it is secret: the credential id names a credential the
//! authenticator will only use after verifying the person holding it, and the
//! salt is an input, not an output. Someone who copies the whole configuration
//! root gets those, and a sealed file they cannot open — the 32 bytes that
//! open it exist only inside a piece of hardware.
//!
//! It lives in `data_dir` rather than `config_dir` for the same reason the
//! keychain sidecar does: `config_dir` is the directory whose contents are
//! designed to be published to a shared Git remote, and a passkey enrolment is
//! machine-local by definition. Copying it to another computer would describe
//! an authenticator that computer cannot reach.
//!
//! # Why every enrolment gets its own salt and its own file
//!
//! So that removing one passkey removes exactly one door. A shared salt would
//! mean two authenticators deriving the same key from the same file, and
//! "forget my old security key" would then either delete both or neither.
//!
//! # When they are destroyed
//!
//! On a passphrase rotation, all of them. The sealed file holds the old
//! passphrase, and a door that opens onto a passphrase the vault no longer
//! accepts is worse than no door: it fails at the moment somebody is relying
//! on it. They are re-enrolled by touching the authenticator again, which is
//! the only way to reach the key.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::crypto::keys::encode_passphrase;
use crate::crypto::{KdfParams, Vault};
use crate::error::{Error, Result};
use crate::model::SecretRef;
use crate::paths::{self, Paths};
use crate::platform::webauthn::{self, Credential, Window, SALT_LEN};
use crate::secret::Secret;

/// One enrolled passkey, as it is recorded on this machine.
///
/// Everything here is safe to show, log and back up. See the module notes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Enrolled {
    pub id: Uuid,
    /// What the user called it — "YubiKey on my keyring", "this laptop".
    pub label: String,
    pub created_at: DateTime<Utc>,
    /// The authenticator's handle for the credential.
    #[serde(with = "base64_bytes")]
    credential_id: Vec<u8>,
    /// The input this credential's key is derived from. Fixed at enrolment: a
    /// different salt is a different key and would not open the sealed file.
    #[serde(with = "base64_bytes")]
    salt: Vec<u8>,
}

impl Enrolled {
    fn credential(&self) -> Credential {
        Credential { id: self.credential_id.clone() }
    }

    fn salt(&self) -> Result<[u8; SALT_LEN]> {
        self.salt.as_slice().try_into().map_err(|_| {
            Error::Crypto(format!(
                "the salt recorded for the passkey \"{}\" is not {SALT_LEN} bytes, so it cannot \
                 be the one it was enrolled with",
                self.label
            ))
        })
    }
}

/// The record of every passkey enrolled on this machine.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Record {
    #[serde(default)]
    passkeys: Vec<Enrolled>,
}

fn record_path(paths: &Paths) -> PathBuf {
    paths.data_dir.join("passkeys.json")
}

fn sidecar_path(paths: &Paths, id: Uuid) -> PathBuf {
    paths.data_dir.join("passkeys").join(format!("{}.sbvault", id.simple()))
}

/// The one entry a sidecar holds.
fn master_handle() -> SecretRef {
    SecretRef::new("master-passphrase", &Uuid::nil())
}

/// KDF parameters for the sidecar.
///
/// At core's documented floor rather than the recommended cost, and for the
/// same reason the keychain sidecar is: the input is 32 bytes of HMAC output
/// from a hardware key, not a phrase somebody chose, so an offline attacker's
/// cheapest path is the authenticator itself. Stretching would add a second of
/// latency to every unlock and nothing else. The floor, rather than below it,
/// so the file is never weaker than core will validate.
fn sidecar_kdf() -> Result<KdfParams> {
    let mut kdf = KdfParams::recommended()?;
    kdf.memory_kib = crate::crypto::kdf::MIN_NEW_MEMORY_KIB;
    Ok(kdf)
}

/// The passphrase the sidecar is sealed under, derived from the 32 bytes the
/// authenticator returned.
fn wrap_passphrase(derived: &Secret) -> Result<Secret> {
    let key: [u8; SALT_LEN] = derived.expose().try_into().map_err(|_| {
        Error::Crypto("the authenticator returned a key of the wrong length".into())
    })?;
    Ok(encode_passphrase(&key))
}

/// Every passkey enrolled here, oldest first.
///
/// A missing or unreadable record is "none enrolled" rather than an error: the
/// interface asks this to decide whether to offer a button, and a machine that
/// has never enrolled one is the ordinary case.
pub fn list(paths: &Paths) -> Vec<Enrolled> {
    let Ok(bytes) = std::fs::read(record_path(paths)) else {
        return Vec::new();
    };
    serde_json::from_slice::<Record>(&bytes).map(|r| r.passkeys).unwrap_or_default()
}

fn save(paths: &Paths, record: &Record) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(record)
        .map_err(|e| Error::Config(format!("the passkey record could not be written: {e}")))?;
    let path = record_path(paths);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| Error::io(format!("creating {}", parent.display()), e))?;
    }
    paths::write_atomic(&path, &bytes)?;
    paths::harden_file(&path)
}

/// Enrol a passkey, so this machine can be unlocked by touching it.
///
/// Needs the master passphrase, because that is what is being sealed. A
/// passkey does not replace it and cannot: this stores the passphrase, wrapped
/// in a key only the authenticator can recompute.
///
/// Two prompts, and both are the operating system's: one to create the
/// credential, one to use it. The second is not ceremony — creating a
/// credential does not yield the key, so it is the only way to find out
/// whether this authenticator can actually derive one. Better to discover that
/// while the user is standing here than at the next unlock.
pub fn enrol(paths: &Paths, window: Window, label: &str, passphrase: &Secret) -> Result<Enrolled> {
    let label = label.trim();
    if label.is_empty() {
        return Err(Error::Validation("a passkey needs a name, so it can be told apart".into()));
    }

    // Prove the passphrase before sealing it.
    //
    // Sealing an unverified one produces a passkey that works perfectly and
    // hands back something the vault refuses — a door that opens onto a wall,
    // discovered at the moment somebody is relying on it. Checking costs one
    // Argon2 derivation, once, while they are standing here.
    let mut store = crate::config::Store::open(paths.clone())?;
    store.unlock(passphrase).map_err(|_| {
        Error::Validation(
            "that is not this vault's master passphrase, so a passkey sealed with it would not              unlock anything"
                .into(),
        )
    })?;
    drop(store);

    let credential = webauthn::enrol(window, label)?;
    let salt = crate::crypto::random_bytes(SALT_LEN)?;
    let salt_array: [u8; SALT_LEN] = salt
        .as_slice()
        .try_into()
        .map_err(|_| Error::Crypto("the salt is not the right length".into()))?;

    let derived = webauthn::derive(window, &credential, &salt_array)?;

    let entry = Enrolled {
        id: Uuid::new_v4(),
        label: label.to_string(),
        created_at: Utc::now(),
        credential_id: credential.id,
        salt,
    };

    seal(paths, &entry, &derived, passphrase)?;

    let mut record = Record { passkeys: list(paths) };
    record.passkeys.push(entry.clone());
    if let Err(e) = save(paths, &record) {
        // A sidecar nothing points at is unopenable dead weight, and worse, it
        // would be left behind by a "forget" that never knew about it.
        let _ = std::fs::remove_file(sidecar_path(paths, entry.id));
        return Err(e);
    }
    Ok(entry)
}

fn seal(paths: &Paths, entry: &Enrolled, derived: &Secret, passphrase: &Secret) -> Result<()> {
    let mut vault = Vault::create_with(&wrap_passphrase(derived)?, sidecar_kdf()?)?;
    vault.put(master_handle(), Secret::new(passphrase.expose().to_vec()))?;
    let bytes = vault.seal()?;
    let path = sidecar_path(paths, entry.id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| Error::io(format!("creating {}", parent.display()), e))?;
    }
    paths::write_atomic(&path, &bytes)?;
    paths::harden_file(&path)
}

/// Recover the master passphrase using one enrolled passkey.
///
/// Prompts. The authenticator asks for its PIN or a fingerprint, and the user
/// may refuse or walk away, so this can fail for entirely ordinary reasons and
/// the caller must fall back to asking for the passphrase.
pub fn unlock_with(paths: &Paths, window: Window, entry: &Enrolled) -> Result<Secret> {
    let derived = webauthn::derive(window, &entry.credential(), &entry.salt()?)?;
    let path = sidecar_path(paths, entry.id);
    let bytes =
        std::fs::read(&path).map_err(|e| Error::io(format!("reading {}", path.display()), e))?;
    let vault = Vault::unlock(&bytes, &wrap_passphrase(&derived)?)?;
    vault.get(&master_handle())?.ok_or_else(|| {
        Error::Crypto("the passkey opened its file, but the master passphrase was not in it".into())
    })
}

/// Forget one passkey. Succeeds when it was not there.
///
/// The sealed file goes first: if this is interrupted, what survives is a
/// record pointing at nothing, which fails loudly at the next attempt, rather
/// than a sealed passphrase nobody is tracking.
pub fn forget(paths: &Paths, id: Uuid) -> Result<()> {
    remove_file(&sidecar_path(paths, id))?;
    let remaining: Vec<Enrolled> = list(paths).into_iter().filter(|e| e.id != id).collect();
    save(paths, &Record { passkeys: remaining })
}

/// Forget all of them. Called when the master passphrase changes.
pub fn forget_all(paths: &Paths) -> Result<()> {
    for entry in list(paths) {
        remove_file(&sidecar_path(paths, entry.id))?;
    }
    match std::fs::remove_file(record_path(paths)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::io(format!("removing {}", record_path(paths).display()), e)),
    }
}

fn remove_file(path: &std::path::Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::io(format!("removing {}", path.display()), e)),
    }
}

/// Base64 for the two byte fields, so the record is a text file a person can
/// read and a `diff` can show.
mod base64_bytes {
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(d)?;
        base64::engine::general_purpose::STANDARD
            .decode(text.as_bytes())
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Home(PathBuf);

    impl Home {
        fn new(tag: &str) -> Home {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default();
            let dir = std::env::temp_dir()
                .join(format!("sb-passkey-{tag}-{}-{nanos}", std::process::id()));
            std::fs::create_dir_all(&dir).expect("temp dir");
            Home(dir)
        }
        fn paths(&self) -> Paths {
            let paths = Paths::rooted_at(&self.0, false);
            paths.ensure().expect("directories");
            paths
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The 32 bytes an authenticator would have returned. Standing in for
    /// hardware nobody can touch from a test.
    fn derived(byte: u8) -> Secret {
        Secret::new(vec![byte; SALT_LEN])
    }

    fn entry(label: &str) -> Enrolled {
        Enrolled {
            id: Uuid::new_v4(),
            label: label.to_string(),
            created_at: Utc::now(),
            credential_id: vec![7; 64],
            salt: vec![9; SALT_LEN],
        }
    }

    /// The whole point: the right key opens it, and nothing else does.
    ///
    /// Exercises the halves either side of the hardware — sealing under a
    /// derived key and opening with it — which is everything this file is
    /// responsible for.
    #[test]
    fn the_passphrase_comes_back_only_for_the_key_it_was_sealed_under() {
        let home = Home::new("roundtrip");
        let paths = home.paths();
        let entry = entry("test key");
        let passphrase = Secret::from_str("correct horse battery staple");

        seal(&paths, &entry, &derived(1), &passphrase).expect("seal");

        let path = sidecar_path(&paths, entry.id);
        let bytes = std::fs::read(&path).expect("read");
        let opened = Vault::unlock(&bytes, &wrap_passphrase(&derived(1)).expect("wrap"))
            .expect("the right key must open it");
        assert_eq!(
            opened.get(&master_handle()).expect("get").expect("entry").expose(),
            passphrase.expose()
        );

        // A different authenticator derives different bytes, and must get
        // nowhere. This is the assertion that says the file is encrypted
        // rather than merely hidden.
        assert!(
            Vault::unlock(&bytes, &wrap_passphrase(&derived(2)).expect("wrap")).is_err(),
            "another key opened the sealed passphrase"
        );
    }

    /// What is on disk must not be the passphrase.
    #[test]
    fn the_sealed_file_does_not_contain_the_passphrase() {
        let home = Home::new("ciphertext");
        let paths = home.paths();
        let entry = entry("test key");
        let needle = b"correct horse battery staple";

        seal(&paths, &entry, &derived(3), &Secret::new(needle.to_vec())).expect("seal");
        let bytes = std::fs::read(sidecar_path(&paths, entry.id)).expect("read");
        assert!(
            !bytes.windows(needle.len()).any(|w| w == needle),
            "the passphrase is on disk in the clear"
        );
    }

    /// The record is readable text and holds nothing secret.
    #[test]
    fn the_record_round_trips_and_carries_no_secret() {
        let home = Home::new("record");
        let paths = home.paths();
        let one = entry("YubiKey on my keyring");

        save(&paths, &Record { passkeys: vec![one.clone()] }).expect("save");
        let read = list(&paths);
        assert_eq!(read, vec![one.clone()]);

        // The credential id and salt are inputs, not outputs: an attacker with
        // this file and without the authenticator has nothing.
        let text = std::fs::read_to_string(record_path(&paths)).expect("read");
        assert!(text.contains("YubiKey on my keyring"), "the label is what a person recognises");
        assert!(!text.contains("passphrase"), "{text}");
    }

    /// Forgetting one leaves the others alone, and takes its sealed file with
    /// it. A record entry without its file, or a file without its entry, is
    /// how a "removed" passkey stays usable.
    #[test]
    fn forgetting_one_passkey_removes_exactly_one_door() {
        let home = Home::new("forget");
        let paths = home.paths();
        let keep = entry("laptop");
        let drop = entry("old key");

        seal(&paths, &keep, &derived(4), &Secret::from_str("pass")).expect("seal");
        seal(&paths, &drop, &derived(5), &Secret::from_str("pass")).expect("seal");
        save(&paths, &Record { passkeys: vec![keep.clone(), drop.clone()] }).expect("save");

        forget(&paths, drop.id).expect("forget");

        assert_eq!(list(&paths), vec![keep.clone()]);
        assert!(!sidecar_path(&paths, drop.id).exists(), "the sealed passphrase outlived it");
        assert!(sidecar_path(&paths, keep.id).exists(), "the wrong door was removed");
    }

    /// A rotation must leave nothing that opens onto the old passphrase.
    #[test]
    fn rotating_the_passphrase_removes_every_passkey() {
        let home = Home::new("rotate");
        let paths = home.paths();
        let a = entry("one");
        let b = entry("two");

        seal(&paths, &a, &derived(6), &Secret::from_str("pass")).expect("seal");
        seal(&paths, &b, &derived(7), &Secret::from_str("pass")).expect("seal");
        save(&paths, &Record { passkeys: vec![a.clone(), b.clone()] }).expect("save");

        forget_all(&paths).expect("forget all");

        assert!(list(&paths).is_empty());
        assert!(!sidecar_path(&paths, a.id).exists());
        assert!(!sidecar_path(&paths, b.id).exists());
    }

    /// A machine that has never enrolled one says so, rather than failing.
    #[test]
    fn no_record_means_no_passkeys() {
        let home = Home::new("empty");
        assert!(list(&home.paths()).is_empty());
    }

    /// A salt of the wrong length is a corrupted record, and must be refused
    /// rather than padded into something that derives a different key.
    #[test]
    fn a_salt_of_the_wrong_length_is_refused() {
        let mut broken = entry("bent");
        broken.salt = vec![1; 8];
        let err = broken.salt().expect_err("must refuse");
        assert!(err.to_string().contains("bent"), "{err}");
    }
}
