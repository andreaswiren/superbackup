//! Making a new SSH key pair.
//!
//! # Why `ssh-keygen` and not a library
//!
//! There are perfectly good Rust libraries that can write an OpenSSH private
//! key. Using one would mean this program deciding the file format, the KDF
//! rounds and the on-disk encoding for a credential the user will rely on for
//! years — and being subtly wrong in a way that only shows up as "some server
//! won't take my key". `ssh-keygen` is the tool every one of those servers
//! agrees with, it is present wherever ssh is, and it is not our job to
//! reimplement.
//!
//! # Why the keys it makes have no passphrase
//!
//! `ssh-keygen` takes a passphrase in an *argument* (`-N`) or from a terminal.
//! There is no file and no stdin. An argument is readable by every process on
//! the machine — `/proc/<pid>/cmdline` on Linux, `Win32_Process` over WMI on
//! Windows without elevation — which is precisely the leak the kopia driver's
//! argv rule exists to prevent, and it would be strange to hold repository
//! passphrases to that standard and not SSH keys.
//!
//! So superbackup makes keys with an empty passphrase, says so plainly, and
//! hands the user the one command that adds one properly:
//!
//! ```text
//! ssh-keygen -p -f ~/.ssh/id_ed25519
//! ```
//!
//! typed in their own terminal, where a passphrase belongs. A key with no
//! passphrase is also exactly what "open it at every boot without asking me"
//! requires, which is what these are usually being made for.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// What kind of key to make.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum KeyType {
    /// The default, and the right answer for almost everyone: short, fast,
    /// and accepted everywhere that has been updated this decade.
    #[default]
    Ed25519,
    /// For a host that predates Ed25519 support. 4096 bits.
    Rsa4096,
}

impl KeyType {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ed25519 => "Ed25519",
            Self::Rsa4096 => "RSA 4096",
        }
    }

    pub fn detail(self) -> &'static str {
        match self {
            Self::Ed25519 => {
                "Short, fast and accepted by every current server. Choose this unless something \
                 has refused it."
            }
            Self::Rsa4096 => {
                "For a host too old to accept Ed25519. Larger and slower, and still perfectly \
                 secure at this size."
            }
        }
    }

    fn args(self) -> [&'static str; 4] {
        match self {
            Self::Ed25519 => ["-t", "ed25519", "-a", "100"],
            Self::Rsa4096 => ["-t", "rsa", "-b", "4096"],
        }
    }

    /// The name OpenSSH would have chosen, so a key made here looks like a key
    /// made any other way.
    pub fn default_file_name(self) -> &'static str {
        match self {
            Self::Ed25519 => "id_ed25519",
            Self::Rsa4096 => "id_rsa",
        }
    }
}

/// What to make.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewKey {
    /// The file name inside the key folder. Not a path.
    pub name: String,
    pub key_type: KeyType,
    /// The trailing comment, which is how people recognise their own keys.
    pub comment: String,
}

impl NewKey {
    /// A name that can safely become a file in the key folder.
    ///
    /// The same reasoning as [`super::safe_key_name`], and stricter: this one
    /// is typed by a person into a box, so it also refuses the shapes that
    /// merely produce a confusing result — a name ending in `.pub` would make
    /// a private key that looks like a public one.
    pub fn validate(&self) -> Result<()> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err(Error::Validation("the key needs a file name".into()));
        }
        if name.len() > 64 {
            return Err(Error::Validation("a key file name is at most 64 characters".into()));
        }
        if !name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')) {
            return Err(Error::Validation(
                "a key file name can hold letters, digits, dashes, underscores and dots only"
                    .into(),
            ));
        }
        if name.starts_with('.') || name.contains("..") {
            return Err(Error::Validation("a key file name cannot start with a dot".into()));
        }
        if name.ends_with(".pub") {
            return Err(Error::Validation(
                "that name ends in .pub, which is what the *public* half is called. The pair is \
                 named after the private key."
                    .into(),
            ));
        }
        // A comment goes into the public key file and into `ssh-add -l`
        // output; a newline in it would corrupt both.
        if self.comment.contains('\n') || self.comment.contains('\r') {
            return Err(Error::Validation("a comment is one line".into()));
        }
        Ok(())
    }
}

/// A key pair that now exists.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedKey {
    pub private_path: PathBuf,
    pub public_path: PathBuf,
    pub fingerprint: Option<String>,
    /// The public key line, ready to be pasted into a forge's settings — which
    /// is the very next thing anybody does after making a key.
    pub public_key: String,
}

/// Make a key pair in `dir`.
pub async fn generate(dir: &Path, spec: &NewKey) -> Result<GeneratedKey> {
    spec.validate()?;
    let tool = super::agent::ssh_keygen().ok_or_else(|| {
        Error::Config(
            "ssh-keygen is not installed. On Windows it comes with the OpenSSH client, which can \
             be added from Settings › Apps › Optional features."
                .into(),
        )
    })?;

    let private = dir.join(spec.name.trim());
    let public = dir.join(format!("{}.pub", spec.name.trim()));
    // Checked before running, because `ssh-keygen` asks "Overwrite (y/n)?" on
    // a terminal that is not there and would simply hang.
    if private.exists() || public.exists() {
        return Err(Error::Validation(format!(
            "{} already exists. Choose another name rather than replacing a key something may \
             still be using.",
            private.display()
        )));
    }
    std::fs::create_dir_all(dir).map_err(|e| Error::io(format!("creating {}", dir.display()), e))?;
    crate::paths::harden_dir(dir)?;

    let mut command = tokio::process::Command::new(&tool);
    command.args(spec.key_type.args());
    command.arg("-f").arg(&private);
    // An *empty* passphrase, which is not a secret and so is not a leak. See
    // this module's header for why superbackup does not offer to set one.
    command.arg("-N").arg("");
    command.arg("-C").arg(spec.comment.trim());
    command.arg("-q");
    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    crate::kopia::harden_child(&mut command);

    let output = tokio::time::timeout(std::time::Duration::from_secs(60), command.output())
        .await
        .map_err(|_| Error::Config("ssh-keygen did not finish within a minute".into()))?
        .map_err(|e| Error::io("running ssh-keygen", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let reason = stderr
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("ssh-keygen failed without saying why");
        return Err(Error::Config(reason.to_string()));
    }
    if !private.is_file() || !public.is_file() {
        return Err(Error::Config(
            "ssh-keygen reported success but the key files are not there".into(),
        ));
    }

    let public_key = std::fs::read_to_string(&public)
        .map_err(|e| Error::io(format!("reading {}", public.display()), e))?
        .trim()
        .to_string();
    let fingerprint = super::ssh::discover_in(dir)
        .into_iter()
        .find(|k| k.private_path == private)
        .and_then(|k| k.fingerprint);

    Ok(GeneratedKey { private_path: private, public_path: public, fingerprint, public_key })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A name typed into a box becomes a file in the key folder, so the shapes
    /// that would escape it or produce a confusing pair are refused.
    #[test]
    fn a_key_name_cannot_escape_or_mislead() {
        let ok = NewKey {
            name: "id_work_ed25519".into(),
            key_type: KeyType::Ed25519,
            comment: "andreas@AWPC34".into(),
        };
        assert!(ok.validate().is_ok());

        for bad in ["../escape", "with/slash", "with\\slash", ".hidden", "..", "", "   "] {
            let spec = NewKey { name: bad.into(), ..ok.clone() };
            assert!(spec.validate().is_err(), "{bad:?} must be refused");
        }
        // `.pub` is the public half's name; a private key called that would be
        // confusing for as long as it existed.
        let confusing = NewKey { name: "id_ed25519.pub".into(), ..ok.clone() };
        let err = confusing.validate().expect_err("refused");
        assert!(err.to_string().contains("public"), "{err}");

        // A comment ends up in the public key file and in `ssh-add -l`.
        let multiline = NewKey { comment: "two\nlines".into(), ..ok.clone() };
        assert!(multiline.validate().is_err());
    }

    /// Ed25519 is the default because it is the right answer for almost
    /// everyone, and the file names match what OpenSSH would have chosen.
    #[test]
    fn the_default_is_the_one_most_people_should_have() {
        assert_eq!(KeyType::default(), KeyType::Ed25519);
        assert_eq!(KeyType::Ed25519.default_file_name(), "id_ed25519");
        assert_eq!(KeyType::Rsa4096.default_file_name(), "id_rsa");
        assert!(KeyType::Ed25519.args().contains(&"ed25519"));
        assert!(KeyType::Rsa4096.args().contains(&"4096"));
    }

    /// The one thing that must never appear: a passphrase in the argument
    /// list. `-N ""` is an *empty* passphrase, which is not a secret.
    #[test]
    fn no_passphrase_is_ever_put_in_an_argument() {
        // The spec has nowhere to put one, which is the structural guarantee.
        let spec = NewKey {
            name: "id_ed25519".into(),
            key_type: KeyType::Ed25519,
            comment: "me@here".into(),
        };
        let encoded = serde_json::to_string(&spec).expect("json");
        assert!(!encoded.contains("passphrase"), "{encoded}");
        assert!(!encoded.contains("password"), "{encoded}");
    }

    /// Making a real key with the real tool, if it is installed.
    #[tokio::test]
    async fn a_generated_pair_is_a_key_this_program_then_recognises() {
        if super::super::agent::ssh_keygen().is_none() {
            eprintln!("ssh-keygen is not installed; skipping");
            return;
        }
        let dir = std::env::temp_dir()
            .join(format!("sb-keygen-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).expect("create");

        let spec = NewKey {
            name: "id_test_ed25519".into(),
            key_type: KeyType::Ed25519,
            comment: "superbackup@test".into(),
        };
        let made = generate(&dir, &spec).await.expect("generated");
        assert!(made.private_path.is_file());
        assert!(made.public_path.is_file());
        assert!(made.public_key.starts_with("ssh-ed25519 "), "{}", made.public_key);
        assert!(made.public_key.ends_with("superbackup@test"), "{}", made.public_key);
        assert!(made.fingerprint.as_deref().unwrap_or_default().starts_with("SHA256:"));

        // The discovery half agrees it is a key, and that it has no passphrase
        // — which is what makes it loadable at boot without being asked.
        let found = super::super::ssh::discover_in(&dir);
        let key = found.iter().find(|k| k.private_path == made.private_path).expect("found");
        assert_eq!(key.algorithm.as_deref(), Some("ssh-ed25519"));
        assert_eq!(key.comment.as_deref(), Some("superbackup@test"));
        assert_eq!(key.encrypted, Some(false), "generated without a passphrase, deliberately");
        assert_eq!(key.fingerprint, made.fingerprint);

        // Making it again is refused rather than silently replacing a key
        // something may still be using.
        let again = generate(&dir, &spec).await.expect_err("refused");
        assert!(again.to_string().contains("already exists"), "{again}");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
