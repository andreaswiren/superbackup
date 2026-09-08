//! The keys and tokens this machine signs in with.
//!
//! # Why a backup tool cares
//!
//! A developer machine holds two kinds of thing that cannot be recreated from
//! a backup of the files: the *work*, which [`crate::git`] is about, and the
//! *credentials*, which are what get you back to the work. Losing a laptop
//! with an SSH key on it and no copy of that key means every forge, every
//! server and every deploy target has to be re-keyed by hand.
//!
//! So this finds them, says what state they are in, and — the part that needs
//! stating plainly — offers to copy them somewhere **encrypted**.
//!
//! # The rule about copying keys
//!
//! A private key is never written to a destination in the clear. Not to a
//! folder mirror, not to OneDrive, not to a bucket. [`KeyBundle`] seals the
//! key material under the vault before it leaves this process, so what lands
//! in a shared folder is a file that is useless without the master
//! passphrase — which is the only arrangement under which "put my SSH keys in
//! OneDrive so my other machine can have them" is not a way to lose them.
//!
//! That is not a policy that can be turned off in settings. A plaintext option
//! would exist to be chosen by the person least able to judge the consequence.

pub mod agent;
pub mod keygen;
pub mod ssh;

use serde::{Deserialize, Serialize};

pub use ssh::SshKey;

use crate::error::{Error, Result};
use crate::secret::Secret;

/// What a credential is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CredentialKind {
    /// An SSH key pair on disk.
    SshKey(Box<SshKey>),
    /// A token for a git forge, held in the vault.
    ForgeToken {
        forge: crate::git::Forge,
        /// `https://github.com` or a self-hosted address.
        base_url: String,
        /// The account the token belongs to, when it is known.
        account: Option<String>,
        /// The vault handle. Never the token.
        token_ref: crate::model::SecretRef,
    },
    /// The GitHub CLI is signed in, and superbackup can borrow that rather
    /// than being given a token of its own.
    ///
    /// The best case, and worth naming as its own kind: nothing is stored,
    /// nothing can be leaked by us, and the user revokes it where they granted
    /// it. See [`crate::git::forge`].
    GitHubCli { account: Option<String> },
}

impl CredentialKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::SshKey(_) => "SSH key",
            Self::ForgeToken { .. } => "Access token",
            Self::GitHubCli { .. } => "GitHub CLI",
        }
    }

    /// Is there a file on disk that a backup could protect?
    ///
    /// A token lives in the vault and is already covered by the vault's own
    /// backup; only keys are files.
    pub fn backable_paths(&self) -> Vec<std::path::PathBuf> {
        match self {
            Self::SshKey(key) => {
                let mut paths = vec![key.private_path.clone()];
                if let Some(public) = &key.public_path {
                    paths.push(public.clone());
                }
                paths
            }
            _ => Vec::new(),
        }
    }
}

/// One credential, as the interface lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credential {
    /// Stable for a key: its path. For a token: the vault handle.
    pub id: String,
    pub name: String,
    pub kind: CredentialKind,
    /// Included in the key backup job.
    #[serde(default)]
    pub backed_up: bool,
    /// Sealed into the shared bundle, so another machine can have it.
    #[serde(default)]
    pub synced: bool,
}

impl Credential {
    /// Everything found on this machine, without reading a single private key.
    pub fn discover() -> Vec<Credential> {
        let mut found: Vec<Credential> = ssh::discover()
            .into_iter()
            .map(|key| Credential {
                id: key.private_path.display().to_string(),
                name: key.label(),
                kind: CredentialKind::SshKey(Box::new(key)),
                backed_up: false,
                synced: false,
            })
            .collect();

        if let Some(account) = github_cli_account() {
            found.push(Credential {
                id: "gh-cli".to_string(),
                name: match &account {
                    Some(account) => format!("GitHub CLI ({account})"),
                    None => "GitHub CLI".to_string(),
                },
                kind: CredentialKind::GitHubCli { account },
                backed_up: false,
                synced: false,
            });
        }
        found
    }
}

/// Is `gh` installed and signed in, and as whom?
///
/// Read from `gh auth status`, which prints the account and never the token.
/// Superbackup does not ask `gh` for the token and has no use for it: the
/// point of borrowing `gh` is that the credential stays where the user put it.
fn github_cli_account() -> Option<Option<String>> {
    let output = std::process::Command::new("gh").args(["auth", "status"]).output().ok()?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if !text.contains("Logged in") {
        return None;
    }
    // `✓ Logged in to github.com account andreaswiren (keyring)`
    let account = text
        .lines()
        .find(|line| line.contains("Logged in"))
        .and_then(|line| {
            let after = line.split("account ").nth(1)?;
            Some(after.split_whitespace().next()?.to_string())
        })
        .filter(|a| !a.is_empty());
    Some(account)
}

/// A set of key files, sealed for a shared location.
///
/// # What this is for
///
/// "Put my SSH keys somewhere my other machine can get them" is a reasonable
/// thing to want and a catastrophic thing to do naively: OneDrive syncs to
/// every device on the account, a bucket is one policy change from public, and
/// a private key is useful to whoever holds it.
///
/// So the bundle is encrypted before it leaves this process, under the same
/// vault that holds every other secret superbackup keeps. What lands in the
/// shared folder is bytes; turning them back into keys needs the master
/// passphrase, on the machine that wants them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyBundle {
    /// Format version, so a newer bundle is refused by an older build rather
    /// than half-read.
    pub version: u32,
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Which machine sealed it, so a folder holding two is not a mystery.
    pub machine: String,
    pub files: Vec<BundledFile>,
}

/// One file inside a bundle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundledFile {
    /// The name only — `id_ed25519`, never a path.
    ///
    /// A bundle carries no directories, and unsealing writes into the
    /// receiving machine's own `~/.ssh`. A path in the bundle would be a path
    /// chosen by whoever wrote the bundle, and unsealing one would be writing
    /// a file wherever a shared folder told us to.
    pub name: String,
    /// The file's bytes, base64-encoded so the bundle is JSON.
    pub content: String,
    /// True for a file that must land with owner-only permissions.
    pub private: bool,
}

/// The bundle format this build writes and can read.
pub const BUNDLE_VERSION: u32 = 1;

/// The name a sealed bundle is written under.
pub const BUNDLE_FILE: &str = "superbackup-keys.sbkeys";

impl KeyBundle {
    /// Gather the named files into a bundle, reading each one once.
    ///
    /// This is the only place key material is read, and it goes straight into
    /// a structure that is sealed before it is returned to anything that could
    /// write it somewhere.
    pub fn gather(machine: &str, paths: &[std::path::PathBuf]) -> Result<KeyBundle> {
        use base64::Engine;
        let mut files = Vec::new();
        for path in paths {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .ok_or_else(|| Error::Validation(format!("{} has no file name", path.display())))?;
            let bytes = std::fs::read(path)
                .map_err(|e| Error::io(format!("reading {}", path.display()), e))?;
            files.push(BundledFile {
                private: !name.ends_with(".pub"),
                name,
                content: base64::engine::general_purpose::STANDARD.encode(&bytes),
            });
        }
        if files.is_empty() {
            return Err(Error::Validation("no key files were selected".into()));
        }
        Ok(KeyBundle {
            version: BUNDLE_VERSION,
            created_at: chrono::Utc::now(),
            machine: machine.to_string(),
            files,
        })
    }

    /// Refuse a bundle this build does not understand.
    pub fn check_version(&self) -> Result<()> {
        if self.version > BUNDLE_VERSION {
            return Err(Error::Validation(format!(
                "this key bundle was written by a newer superbackup (format {}, this build reads \
                 {BUNDLE_VERSION}). Update before unsealing it, rather than restoring half of it.",
                self.version
            )));
        }
        Ok(())
    }

    /// The bytes of one file, decoded.
    pub fn decode(file: &BundledFile) -> Result<Secret> {
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&file.content)
            .map_err(|_| Error::Validation(format!("{} is not readable", file.name)))?;
        Ok(Secret::new(bytes))
    }
}

/// The handle the bundle's contents are stored under inside its envelope.
const BUNDLE_SECRET: &str = "keys:bundle";

impl KeyBundle {
    /// Seal this bundle under a passphrase, ready to be written anywhere.
    ///
    /// # Why it reuses the vault rather than encrypting by hand
    ///
    /// Argon2id with the vault's parameters, HKDF subkeys, XChaCha20-Poly1305,
    /// a versioned header, associated data bound to that header, and a format
    /// an older build refuses rather than misreads — all of that already
    /// exists in [`crate::crypto::Vault`], is audited, and is tested. A second
    /// bespoke encryption path for the most sensitive bytes this program ever
    /// touches would be the worst possible place to have one.
    ///
    /// So a bundle is a vault holding exactly one secret. It is a slightly odd
    /// use of the type and it is the right trade: nothing here is new
    /// cryptography, and a bundle can be opened by anything that can open a
    /// vault.
    pub fn seal(&self, passphrase: &Secret) -> Result<Vec<u8>> {
        let json = serde_json::to_vec(self)
            .map_err(|e| Error::Internal(format!("the key bundle could not be encoded: {e}")))?;
        let mut vault = crate::crypto::Vault::create(passphrase)?;
        vault.put(crate::model::SecretRef(BUNDLE_SECRET.to_string()), Secret::new(json))?;
        vault.seal()
    }

    /// Open a sealed bundle.
    ///
    /// The version check happens *after* decryption and before anything is
    /// written, so a bundle from a newer build is refused whole rather than
    /// unpacked as far as this build understands it.
    pub fn unseal(bytes: &[u8], passphrase: &Secret) -> Result<KeyBundle> {
        let vault = crate::crypto::Vault::unlock(bytes, passphrase)?;
        let secret =
            vault.get(&crate::model::SecretRef(BUNDLE_SECRET.to_string()))?.ok_or_else(|| {
                Error::Validation(
                    "this file opened, but it is not a key bundle — it holds no key material"
                        .into(),
                )
            })?;
        let bundle: KeyBundle = serde_json::from_slice(secret.expose()).map_err(|_| {
            Error::Validation("this key bundle could not be read; it may be damaged".into())
        })?;
        bundle.check_version()?;
        Ok(bundle)
    }

    /// Write the bundle's files into a folder, with the right permissions.
    ///
    /// Every name is checked with [`safe_key_name`] first: a bundle comes out
    /// of a folder shared between machines, so its contents are input from
    /// somewhere the user does not wholly control.
    ///
    /// Existing files are never overwritten unless asked, because the most
    /// likely mistake here is unsealing an old bundle over the key a machine
    /// is currently using.
    pub fn write_into(&self, dir: &std::path::Path, overwrite: bool) -> Result<Vec<String>> {
        std::fs::create_dir_all(dir)
            .map_err(|e| Error::io(format!("creating {}", dir.display()), e))?;
        crate::paths::harden_dir(dir)?;

        // Every name is validated before anything is written, so a bundle with
        // one bad name in it does not leave half of itself on disk.
        for file in &self.files {
            safe_key_name(&file.name)?;
        }

        let mut written = Vec::new();
        for file in &self.files {
            let target = dir.join(&file.name);
            if target.exists() && !overwrite {
                continue;
            }
            let bytes = KeyBundle::decode(file)?;
            crate::paths::write_atomic(&target, bytes.expose())?;
            harden_private_file(&target);
            written.push(file.name.clone());
        }
        Ok(written)
    }
}

/// Take a freshly written private key down to owner-only.
///
/// ssh refuses to use a key anyone else can read, so a restored key with the
/// wrong permissions is a key that does not work — and a failure the user
/// would meet as an unexplained authentication error rather than as anything
/// to do with a restore.
///
/// Best effort: a filesystem that cannot express it (a FAT volume, a network
/// share) is not a reason to fail the whole unsealing.
fn harden_private_file(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    {
        // Windows inherits the folder ACL, and `harden_dir` has already
        // restricted the folder to this user.
        let _ = path;
    }
}

/// A file name that is safe to write into `~/.ssh`.
///
/// Bundles come out of a folder that is shared between machines, which means
/// a bundle is input from somewhere the user does not fully control — a
/// second machine, a synced folder, a bucket somebody else can write to. A
/// name of `../../.bashrc` in one of them would otherwise be a way to write an
/// arbitrary file on every machine that unseals it.
pub fn safe_key_name(name: &str) -> Result<&str> {
    if name.is_empty() {
        return Err(Error::Validation("a bundled file has no name".into()));
    }
    if name.contains('/') || name.contains('\\') || name.contains("..") || name.starts_with('.') {
        return Err(Error::Validation(format!(
            "\"{name}\" is not a file name that can be written into the key folder"
        )));
    }
    if std::path::Path::new(name).components().count() != 1 {
        return Err(Error::Validation(format!("\"{name}\" is not a plain file name")));
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bundle is input from a folder shared between machines. A name that
    /// escapes the key folder would be a way to write an arbitrary file on
    /// every machine that unseals it.
    #[test]
    fn a_bundled_name_cannot_escape_the_key_folder() {
        assert_eq!(safe_key_name("id_ed25519").expect("fine"), "id_ed25519");
        assert_eq!(safe_key_name("id_ed25519.pub").expect("fine"), "id_ed25519.pub");

        for hostile in [
            "../.bashrc",
            "..\\.bashrc",
            "../../.ssh/authorized_keys",
            "/etc/passwd",
            "C:\\Windows\\System32\\x",
            ".profile",
            "",
        ] {
            assert!(safe_key_name(hostile).is_err(), "{hostile} must be refused");
        }
    }

    /// A newer bundle is refused rather than half-read: restoring some of a
    /// key set and not the rest is worse than restoring none of it.
    #[test]
    fn a_bundle_from_a_newer_build_is_refused() {
        let mut bundle = KeyBundle {
            version: BUNDLE_VERSION,
            created_at: chrono::Utc::now(),
            machine: "awpc34".into(),
            files: Vec::new(),
        };
        assert!(bundle.check_version().is_ok());

        bundle.version = BUNDLE_VERSION + 1;
        let err = bundle.check_version().expect_err("refused");
        assert!(err.to_string().contains("newer superbackup"), "{err}");
    }

    /// The public half is not secret and the private half is; unsealing has to
    /// know which is which to set the permissions.
    #[test]
    fn a_bundle_marks_which_files_are_private() {
        let dir = std::env::temp_dir().join(format!("sb-bundle-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).expect("create");
        std::fs::write(dir.join("id_ed25519"), b"private").expect("write");
        std::fs::write(dir.join("id_ed25519.pub"), b"public").expect("write");

        let bundle =
            KeyBundle::gather("awpc34", &[dir.join("id_ed25519"), dir.join("id_ed25519.pub")])
                .expect("gathered");
        assert_eq!(bundle.files.len(), 2);
        assert!(bundle.files[0].private, "the key is private");
        assert!(!bundle.files[1].private, "the .pub is not");
        // Names only: a bundle carries no directories.
        assert_eq!(bundle.files[0].name, "id_ed25519");
        assert!(!bundle.files[0].name.contains(std::path::MAIN_SEPARATOR));

        let decoded = KeyBundle::decode(&bundle.files[0]).expect("decoded");
        assert_eq!(decoded.expose(), b"private");

        // Nothing to gather is refused rather than producing an empty bundle
        // that would look like a successful backup of nothing.
        assert!(KeyBundle::gather("awpc34", &[]).is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
