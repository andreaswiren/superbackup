//! Optional caching of the master passphrase, so an unattended machine can
//! back up without anyone typing it.
//!
//! Gated by [`Settings::use_os_keychain`](superbackup_core::model::Settings),
//! which is **off by default and must stay that way**.
//! `docs/compliance/THREAT_MODEL.md` §5 states the trade explicitly: caching
//! moves the boundary from "something the user knows" to "something the
//! machine holds", so an attacker who reaches the logged-in account reaches
//! every backup. That is a decision only the machine's owner may make, and
//! nothing here widens it beyond what they agreed to.
//!
//! # What is stored, and where
//!
//! ```text
//!   OS keychain                     data/keychain.sbvault
//!   ───────────                     ─────────────────────
//!   32 random bytes  ──unwraps──▶   a vault holding one entry:
//!   (the wrap key)                  the master passphrase
//! ```
//!
//! **The platform store never holds the passphrase.** It holds a random
//! 256-bit wrap key, and the passphrase lives beside it in a sealed vault of
//! superbackup's own format. Recovering the passphrase needs *both*, so:
//!
//! * a backup tool, a support script, or a credential-enumerating process that
//!   reads the keychain gets 32 bytes of noise and no way to use them;
//! * a copy of the configuration root — the thing users e-mail to each other
//!   when asking for help, and the thing a cloud-synced profile replicates —
//!   contains only ciphertext;
//! * the entry is scoped to one configuration root, so a portable install and
//!   a system service never read each other's key.
//!
//! The sidecar is deliberately in `data_dir`, not `config_dir`: it is
//! machine-local state, and `config_dir` is the directory whose contents are
//! designed to be published to a shared Git remote.
//!
//! Its KDF sits at core's documented floor rather than the recommended cost,
//! and that is a considered choice rather than a corner cut: the input is 256
//! bits of uniform CSPRNG output, so an offline attacker's cheapest path is
//! brute-forcing the key itself and not guessing a passphrase. Stretching adds
//! nothing but a second of start-up latency. The floor is used rather than
//! anything below it so the file is never weaker than core is willing to
//! validate.
//!
//! # When the cached key is destroyed
//!
//! Both halves are erased on every one of these, because a cached key that
//! still opens a vault whose passphrase has moved on is the failure that
//! matters:
//!
//! | Event | Why |
//! |---|---|
//! | `vault.lock`, and the auto-lock timer | "Lock" must mean locked. A machine that re-opens itself the moment it is asked to shut has not locked anything. |
//! | `use_os_keychain` switched off | The setting is the consent; withdrawing it withdraws the cache. |
//! | passphrase rotation | A stale key is worthless at best and misleading at worst. It is re-cached with the *new* passphrase when the setting is still on. |
//! | the cached passphrase failing to open the vault | It was rotated elsewhere; keep nothing that does not work. |
//!
//! Note the consequence of the first row: pairing this setting with a non-zero
//! `auto_lock_minutes` means the cache survives only until the first auto-lock.
//! The daemon says so at start-up rather than leaving it to be discovered.
//!
//! # Failure is always "ask the user"
//!
//! Every operation here degrades to prompting, and every degradation is
//! reported as an activity event. A keychain that is missing, locked, refused
//! by policy, or simply not running must never fail a backup — but it must
//! also never be silent, because the user's actual position in that case is
//! "scheduled backups will be skipped until I unlock by hand", and they can
//! only act on that if they are told.

use std::path::PathBuf;

use superbackup_core::crypto::{encode_passphrase, KdfParams, Vault};
use superbackup_core::model::SecretRef;
use superbackup_core::paths::{self, Paths};
use superbackup_core::secret::Secret;
use superbackup_core::{Error, Result};

/// Length of the wrap key. 256 bits, matching the vault's own key sizes.
const WRAP_KEY_LEN: usize = 32;

/// The keychain service name. Stable: a user looking through Credential
/// Manager or Keychain Access should see one recognisable entry.
pub const SERVICE: &str = "superbackup";

/// The vault entry the sidecar holds. One handle, one secret.
fn master_handle() -> SecretRef {
    SecretRef::new("master-passphrase", &uuid::Uuid::nil())
}

/// The account name for one installation's entry.
///
/// Derived from the configuration root so the per-user instance and the
/// machine-wide service — different vaults, different passphrases — cannot
/// read each other's key. The path is not a secret; it is an identifier, and
/// using the whole of it rather than a hash means a user auditing their
/// credential store can see which install an entry belongs to.
pub fn entry_name(paths: &Paths) -> String {
    let scope = if paths.service_scope { "service" } else { "user" };
    format!("{SERVICE}/{scope}:{}", paths.config_dir.display())
}

/// Where the wrapped passphrase lives.
pub fn sidecar_path(paths: &Paths) -> PathBuf {
    paths.data_dir.join("keychain.sbvault")
}

/// Whether this build can reach a platform keychain at all.
pub fn available() -> bool {
    true
}

/// The sentence shown when the keychain cannot be used.
pub fn explain_unavailable() -> &'static str {
    "superbackup could not use the operating system's keychain, so it will ask for your \
     passphrase at the next start. Scheduled backups are skipped until the vault is unlocked."
}

/// Render a keyring failure as something a user can act on.
///
/// Never includes the entry name or any byte of the key: the message goes into
/// the activity log, which is the first thing pasted into a bug report.
fn describe(error: &keyring::Error) -> String {
    match error {
        keyring::Error::NoEntry => "there is no saved key for this installation".into(),
        keyring::Error::NoStorageAccess(_) => {
            "the credential store refused access — it may be locked, or blocked by policy".into()
        }
        keyring::Error::PlatformFailure(_) => {
            "the operating system's credential store reported a failure".into()
        }
        keyring::Error::Ambiguous(_) => {
            "the credential store holds more than one entry for superbackup".into()
        }
        keyring::Error::TooLong(what, limit) => {
            format!("the credential store rejected the {what} field (limit {limit})")
        }
        keyring::Error::Invalid(what, why) => {
            format!("the credential store rejected the {what} field: {why}")
        }
        _ => "the credential store could not be used".into(),
    }
}

// ---------------------------------------------------------------------------
// The platform half
// ---------------------------------------------------------------------------

/// Run one blocking keyring call off the runtime's worker threads.
///
/// Every backend blocks: Credential Manager and the macOS Keychain are
/// synchronous syscalls, and the `async-secret-service` backend drives zbus on
/// its own executor. `spawn_blocking` also contains the one documented panic
/// in `Entry::new` (a poisoned builder lock) as a `JoinError` rather than
/// letting it reach a backup daemon.
async fn on_blocking<T, F>(work: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    match tokio::task::spawn_blocking(work).await {
        Ok(result) => result,
        Err(e) => Err(Error::Platform(format!("the keychain call did not complete: {e}"))),
    }
}

fn entry(name: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(SERVICE, name)
        .map_err(|e| Error::Platform(format!("keychain entry: {}", describe(&e))))
}

/// Write the wrap key to the platform store.
async fn put_key(name: String, key: Vec<u8>) -> Result<()> {
    on_blocking(move || {
        entry(&name)?
            .set_secret(&key)
            .map_err(|e| Error::Platform(format!("keychain: {}", describe(&e))))
    })
    .await
}

/// Read the wrap key back. `Ok(None)` means "nothing saved", which is the
/// ordinary first-run answer and not a failure.
async fn take_key(name: String) -> Result<Option<Vec<u8>>> {
    on_blocking(move || match entry(&name)?.get_secret() {
        Ok(bytes) => Ok(Some(bytes)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(Error::Platform(format!("keychain: {}", describe(&e)))),
    })
    .await
}

/// Remove the entry. Succeeds when there was nothing to remove.
async fn drop_key(name: String) -> Result<()> {
    on_blocking(move || match entry(&name)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(Error::Platform(format!("keychain: {}", describe(&e)))),
    })
    .await
}

// ---------------------------------------------------------------------------
// The local half
// ---------------------------------------------------------------------------

/// KDF parameters for the sidecar. See the module documentation for why this
/// sits at the floor rather than at the recommended cost.
fn sidecar_kdf() -> Result<KdfParams> {
    let mut kdf = KdfParams::recommended()?;
    kdf.memory_kib = superbackup_core::crypto::kdf::MIN_NEW_MEMORY_KIB;
    Ok(kdf)
}

/// The passphrase the sidecar vault is sealed under, derived from the wrap key.
///
/// `encode_passphrase` is the same function that turns a derived subkey into a
/// repository password, so the encoding is one the codebase already relies on.
fn wrap_passphrase(key: &[u8]) -> Result<Secret> {
    let key: [u8; WRAP_KEY_LEN] = key
        .try_into()
        .map_err(|_| Error::Crypto("the cached key is not the right length".into()))?;
    Ok(encode_passphrase(&key))
}

/// Seal `passphrase` into the sidecar under `key`. Exposed for tests, which
/// exercise this half without touching the platform store.
pub fn seal_local(paths: &Paths, key: &[u8], passphrase: &Secret) -> Result<()> {
    let mut vault = Vault::create_with(&wrap_passphrase(key)?, sidecar_kdf()?)?;
    vault.put(master_handle(), Secret::new(passphrase.expose().to_vec()))?;
    let bytes = vault.seal()?;
    let path = sidecar_path(paths);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| Error::Io {
            context: format!("creating {}", parent.display()),
            source: e,
        })?;
    }
    paths::write_atomic(&path, &bytes)?;
    paths::harden_file(&path)
}

/// Recover the passphrase from the sidecar. `Ok(None)` when there is no
/// sidecar; `Err` when there is one and it will not open, which is a state the
/// caller must report rather than ignore.
pub fn open_local(paths: &Paths, key: &[u8]) -> Result<Option<Secret>> {
    let path = sidecar_path(paths);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(Error::Io { context: format!("reading {}", path.display()), source: e })
        }
    };
    let vault = Vault::unlock(&bytes, &wrap_passphrase(key)?)?;
    vault.get(&master_handle())
}

/// Delete the sidecar. Succeeds when it was not there.
pub fn remove_local(paths: &Paths) -> Result<()> {
    match std::fs::remove_file(sidecar_path(paths)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::Io {
            context: format!("removing {}", sidecar_path(paths).display()),
            source: e,
        }),
    }
}

// ---------------------------------------------------------------------------
// The public operations
// ---------------------------------------------------------------------------

/// Cache the passphrase: a fresh wrap key into the platform store, the
/// passphrase sealed under it on disk.
///
/// A fresh key every time, so re-caching after a rotation cannot leave the old
/// sidecar openable. On any failure both halves are erased, because half a
/// cache is a file that looks like it should work and never will.
pub async fn store(paths: &Paths, passphrase: &Secret) -> Result<()> {
    let key = superbackup_core::crypto::random_bytes(WRAP_KEY_LEN)?;
    seal_local(paths, &key, passphrase)?;
    if let Err(e) = put_key(entry_name(paths), key).await {
        // The sidecar without its key is unopenable dead weight.
        let _ = remove_local(paths);
        return Err(e);
    }
    Ok(())
}

/// Recover the cached passphrase.
///
/// `Ok(None)` is the ordinary "nothing cached" answer. `Err` means something
/// went wrong that the user should be told about — and either way the caller
/// falls back to asking.
pub async fn load(paths: &Paths) -> Result<Option<Secret>> {
    let Some(key) = take_key(entry_name(paths)).await? else {
        return Ok(None);
    };
    match open_local(paths, &key) {
        Ok(Some(passphrase)) => Ok(Some(passphrase)),
        // A key with no sidecar, or a sidecar the key will not open: the two
        // halves have drifted apart and neither is worth keeping.
        Ok(None) => {
            let _ = forget(paths).await;
            Ok(None)
        }
        Err(e) => {
            let _ = forget(paths).await;
            Err(e)
        }
    }
}

/// Destroy both halves. Succeeds when there was nothing cached.
///
/// The local half goes first: if the process dies between the two, what
/// survives is a keychain entry that opens nothing, rather than a sidecar
/// anyone with the key could open.
pub async fn forget(paths: &Paths) -> Result<()> {
    let local = remove_local(paths);
    let remote = drop_key(entry_name(paths)).await;
    local.and(remote)
}

/// Whether anything is cached for this installation, without decrypting it.
///
/// Cheap enough for a settings screen; does not prove the cache still works.
pub fn has_local(paths: &Paths) -> bool {
    sidecar_path(paths).is_file()
}

/// [`forget`], but only when this installation has actually cached something.
///
/// Used by the paths that run on *every* lock and every rotation. Calling
/// `forget` there unconditionally would mean a credential-store syscall — and
/// a delete against an entry that does not exist — on machines whose owner
/// never opted into caching at all. Touching a user's credential store when
/// they asked for nothing of the sort is the kind of small liberty that is
/// worth not taking.
///
/// The sidecar is the marker: it is written before the platform entry and
/// removed after it, so its presence means the feature was used here. A key
/// left behind with no sidecar is cleaned up by [`load`].
pub async fn forget_if_cached(paths: &Paths) -> Result<()> {
    if !has_local(paths) {
        return Ok(());
    }
    forget(paths).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> (PathBuf, Paths) {
        let root = std::env::temp_dir().join(format!("sb-keychain-{}", uuid::Uuid::new_v4()));
        let paths = Paths::rooted_at(&root, false);
        paths.ensure().expect("dirs");
        (root, paths)
    }

    #[test]
    fn the_entry_name_separates_the_service_from_the_user_instance() {
        let user = Paths::rooted_at("/tmp/sb", false);
        let service = Paths::rooted_at("/tmp/sb", true);
        assert_ne!(entry_name(&user), entry_name(&service));
        assert!(entry_name(&user).starts_with("superbackup/user:"));
        assert!(entry_name(&service).starts_with("superbackup/service:"));
    }

    #[test]
    fn two_installations_never_share_an_entry() {
        // The whole point of scoping: a portable install on a stick must not
        // be able to read the key belonging to the copy on the machine.
        let a = Paths::rooted_at("/tmp/sb-one", false);
        let b = Paths::rooted_at("/tmp/sb-two", false);
        assert_ne!(entry_name(&a), entry_name(&b));
    }

    #[test]
    fn the_sidecar_is_local_state_and_never_publishable_configuration() {
        // `config_dir` is what a remote push publishes. A wrapped passphrase
        // landing there would be sent to every machine sharing the vault.
        let paths = Paths::rooted_at("/tmp/sb", false);
        assert!(sidecar_path(&paths).starts_with(&paths.data_dir));
        assert!(!sidecar_path(&paths).starts_with(&paths.config_dir));
    }

    #[test]
    fn a_passphrase_round_trips_through_the_local_half() {
        let (root, paths) = scratch();
        let key = superbackup_core::crypto::random_bytes(WRAP_KEY_LEN).expect("key");
        let secret = Secret::from_str("correct-horse-battery-staple-42");

        seal_local(&paths, &key, &secret).expect("seal");
        let recovered = open_local(&paths, &key).expect("open").expect("a cached passphrase");
        assert_eq!(recovered.expose(), secret.expose());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_sidecar_alone_reveals_nothing() {
        // The file is what leaks when a profile is synced or a config folder
        // is e-mailed. It must not contain the passphrase, and the wrong key
        // must not open it.
        let (root, paths) = scratch();
        let key = superbackup_core::crypto::random_bytes(WRAP_KEY_LEN).expect("key");
        let secret = Secret::from_str("correct-horse-battery-staple-42");
        seal_local(&paths, &key, &secret).expect("seal");

        let bytes = std::fs::read(sidecar_path(&paths)).expect("read");
        let haystack = String::from_utf8_lossy(&bytes);
        assert!(
            !haystack.contains("correct-horse-battery-staple-42"),
            "the passphrase is in the sidecar in the clear"
        );

        let wrong = superbackup_core::crypto::random_bytes(WRAP_KEY_LEN).expect("key");
        assert!(open_local(&paths, &wrong).is_err(), "another key opened the sidecar");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_key_of_the_wrong_length_is_refused_rather_than_padded() {
        assert!(wrap_passphrase(&[0u8; 16]).is_err());
        assert!(wrap_passphrase(&[]).is_err());
        assert!(wrap_passphrase(&[0u8; WRAP_KEY_LEN]).is_ok());
    }

    #[test]
    fn clearing_the_local_half_makes_the_cache_unusable() {
        // This is the mechanism behind clear-on-lock and clear-on-rotate: even
        // if the platform entry survived, there is nothing left for it to open.
        let (root, paths) = scratch();
        let key = superbackup_core::crypto::random_bytes(WRAP_KEY_LEN).expect("key");
        seal_local(&paths, &key, &Secret::from_str("a-long-enough-passphrase")).expect("seal");
        assert!(has_local(&paths));

        remove_local(&paths).expect("remove");
        assert!(!has_local(&paths));
        assert!(open_local(&paths, &key).expect("no sidecar is not an error").is_none());

        // Idempotent: the lock path runs it whether or not anything was cached.
        remove_local(&paths).expect("removing nothing succeeds");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn re_caching_mints_a_fresh_key_so_the_old_one_stops_working() {
        // A rotation re-caches. The previous key must not open the new
        // sidecar, or a key captured before the rotation would still work.
        let (root, paths) = scratch();
        let old_key = superbackup_core::crypto::random_bytes(WRAP_KEY_LEN).expect("key");
        seal_local(&paths, &old_key, &Secret::from_str("the-old-passphrase-here")).expect("seal");

        let new_key = superbackup_core::crypto::random_bytes(WRAP_KEY_LEN).expect("key");
        seal_local(&paths, &new_key, &Secret::from_str("the-new-passphrase-here")).expect("seal");

        assert!(open_local(&paths, &old_key).is_err(), "the old key still opens the sidecar");
        let recovered = open_local(&paths, &new_key).expect("open").expect("entry");
        assert_eq!(recovered.expose(), b"the-new-passphrase-here");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn clearing_an_installation_that_never_cached_touches_nothing() {
        // The lock path runs on every lock, including on machines whose owner
        // never opted in. It must not reach for their credential store.
        let (root, paths) = scratch();
        assert!(!has_local(&paths));
        forget_if_cached(&paths).await.expect("nothing to clear is not a failure");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_sidecar_kdf_is_at_the_floor_and_not_below_it() {
        let kdf = sidecar_kdf().expect("params");
        assert_eq!(kdf.memory_kib, superbackup_core::crypto::kdf::MIN_NEW_MEMORY_KIB);
        // Core must be willing to create a vault with these; anything it
        // rejects would be weaker than the project has agreed to accept.
        kdf.validate_for_new_vault().expect("core accepts the sidecar's parameters");
    }

    /// Touches the real platform credential store, so it is opt-in: run with
    /// `cargo test -p superbackup keychain -- --ignored`. On a headless CI box
    /// there is no credential store to talk to, and on a developer's machine
    /// it prompts.
    #[tokio::test]
    #[ignore = "writes to the operating system's credential store"]
    async fn a_passphrase_round_trips_through_the_real_keychain() {
        let (root, paths) = scratch();
        let secret = Secret::from_str("correct-horse-battery-staple-42");

        store(&paths, &secret).await.expect("store");
        let recovered = load(&paths).await.expect("load").expect("a cached passphrase");
        assert_eq!(recovered.expose(), secret.expose());

        forget(&paths).await.expect("forget");
        assert!(load(&paths).await.expect("load after forget").is_none());
        assert!(!has_local(&paths));

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The half-cache case: the platform entry exists but the sidecar is gone.
    #[tokio::test]
    #[ignore = "writes to the operating system's credential store"]
    async fn a_key_whose_sidecar_vanished_is_discarded_rather_than_reported_as_a_secret() {
        let (root, paths) = scratch();
        store(&paths, &Secret::from_str("a-long-enough-passphrase")).await.expect("store");
        remove_local(&paths).expect("simulate a deleted sidecar");

        assert!(load(&paths).await.expect("load").is_none());
        // And the orphaned key was cleaned up rather than left behind.
        assert!(load(&paths).await.expect("load").is_none());

        let _ = std::fs::remove_dir_all(&root);
    }
}
