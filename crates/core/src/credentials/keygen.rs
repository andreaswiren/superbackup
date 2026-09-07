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
//! # How a passphrase reaches ssh-keygen without touching a command line
//!
//! `ssh-keygen` takes a passphrase from `-N`, from a terminal, or — since
//! OpenSSH 8.4 — from an **askpass helper** named by `SSH_ASKPASS` when
//! `SSH_ASKPASS_REQUIRE=force` is set: it runs the helper and reads one line
//! from its output, once for the passphrase and once for the confirmation.
//!
//! `-N` was the obvious route and it is the wrong one. An argument is readable
//! by every process on the machine — `/proc/<pid>/cmdline` is mode 0444, and
//! on Windows a command line comes out of `Win32_Process` over WMI without
//! elevation. That is precisely the leak the kopia driver's argv rule exists
//! to prevent, and holding repository passphrases to that standard but not SSH
//! keys would be indefensible.
//!
//! So superbackup is its own askpass helper: `superbackup askpass` prints
//! whatever is in [`ASKPASS_ENV`]. The passphrase travels in the child's
//! *environment*, which is readable by the user who already owns `~/.ssh` and
//! by nobody else.
//!
//! A key with no passphrase is still offered, and is what "open it at every
//! boot without asking me" requires — but it is now a choice rather than a
//! limitation.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::secret::Secret;

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
    /// Whether it ended up protected by a passphrase. Checked against the key
    /// on disk rather than assumed from what was asked for.
    pub protected: bool,
}

/// The environment variable the askpass helper reads the passphrase from.
pub const ASKPASS_ENV: &str = "SUPERBACKUP_ASKPASS";

/// Make a key pair in `dir`.
///
/// With `passphrase`, the key is protected by it; without, it has none. See
/// this module header for what each choice means, and for how a passphrase
/// reaches `ssh-keygen` without touching a command line.
pub async fn generate(
    dir: &Path,
    spec: &NewKey,
    passphrase: Option<&Secret>,
) -> Result<GeneratedKey> {
    // Superbackup is its own askpass helper; see the module header.
    let helper = std::env::current_exe()
        .map_err(|e| Error::io("finding superbackup's own path, to use as the askpass helper", e))?;
    generate_with_helper(dir, spec, passphrase, &helper).await
}

/// The same, with the askpass helper named explicitly.
///
/// Exists so the mechanism can be tested. It has to be: a helper that returns
/// nothing makes `ssh-keygen` use an *empty* passphrase and report success,
/// which produces an unprotected key that everything downstream believes is
/// protected. The check at the end of this function is what catches that, and
/// it caught exactly that during development — the test binary is not
/// superbackup and has no `askpass` subcommand.
pub async fn generate_with_helper(
    dir: &Path,
    spec: &NewKey,
    passphrase: Option<&Secret>,
    askpass: &Path,
) -> Result<GeneratedKey> {
    spec.validate()?;
    if let Some(passphrase) = passphrase {
        let phrase = passphrase
            .expose_str()
            .ok_or_else(|| Error::Internal("the generated passphrase is not text".into()))?;
        // OpenSSH refuses anything shorter, and learning that from its own
        // message would be a confusing way to find out.
        if phrase.chars().count() < 5 {
            return Err(Error::Validation(
                "a key passphrase is at least five characters".into(),
            ));
        }
    }

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
    command.arg("-C").arg(spec.comment.trim());
    command.arg("-q");
    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());

    match passphrase {
        None => {
            // An *empty* passphrase, which is not a secret and so is not a
            // leak to put in an argument.
            command.arg("-N").arg("");
        }
        Some(passphrase) => {
            // `ssh-keygen` runs the program named by SSH_ASKPASS and reads one
            // line from its standard output, twice — once for the passphrase
            // and once for the confirmation. Superbackup is that program:
            // `superbackup askpass` prints whatever is in ASKPASS_ENV.
            //
            // The passphrase therefore travels in the child's *environment*
            // rather than its argument list, and that difference is the whole
            // point. `/proc/<pid>/cmdline` is mode 0444 — readable by every
            // user on the machine. `/proc/<pid>/environ` is 0400 — readable by
            // the process's own user, who already owns `~/.ssh`. On Windows
            // the same asymmetry holds: a command line comes out of
            // `Win32_Process` over WMI without elevation; another process's
            // environment does not.
            command.env("SSH_ASKPASS", askpass);
            command.env("SSH_ASKPASS_REQUIRE", "force");
            // Some builds still consult DISPLAY before running an askpass at
            // all; a value that is merely non-empty is enough.
            command.env("DISPLAY", ":0");
            command.env(
                ASKPASS_ENV,
                passphrase
                    .expose_str()
                    .ok_or_else(|| Error::Internal("the passphrase is not text".into()))?,
            );
        }
    }
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
    let found = super::ssh::discover_in(dir).into_iter().find(|k| k.private_path == private);

    // The key must actually be in the state that was asked for. A passphrase
    // that silently did not take would leave an unprotected key whose vault
    // entry claims otherwise — and nothing downstream would ever notice.
    if let Some(key) = &found {
        let wanted = passphrase.is_some();
        if key.encrypted == Some(!wanted) {
            return Err(Error::Config(format!(
                "ssh-keygen produced a key that is {} a passphrase, which is not what was asked \
                 for. The key at {} has been left in place for you to inspect.",
                if wanted { "without" } else { "with" },
                private.display()
            )));
        }
    }

    Ok(GeneratedKey {
        private_path: private,
        public_path: public,
        fingerprint: found.and_then(|k| k.fingerprint),
        public_key,
        protected: passphrase.is_some(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A standalone askpass helper for the tests: superbackup itself plays
    /// this part in production, and the test binary cannot.
    fn write_askpass(dir: &Path, passphrase: &crate::secret::Secret) -> PathBuf {
        let phrase = passphrase.expose_str().expect("text");
        #[cfg(windows)]
        {
            let path = dir.join("askpass.bat");
            std::fs::write(&path, format!("@echo off\r\necho {phrase}\r\n")).expect("write");
            path
        }
        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = dir.join("askpass.sh");
            std::fs::write(&path, format!("#!/bin/sh\necho '{phrase}'\n")).expect("write");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
                .expect("chmod");
            path
        }
    }

    /// A helper that returns nothing makes ssh-keygen use an *empty*
    /// passphrase and report success — an unprotected key that everything
    /// downstream believes is protected. This is the case the check at the end
    /// of `generate_with_helper` exists for, and it is worth a test of its own
    /// because it fails silently in every other way.
    #[tokio::test]
    async fn a_helper_that_gives_nothing_is_caught_rather_than_trusted() {
        if super::super::agent::ssh_keygen().is_none() {
            return;
        }
        let dir = std::env::temp_dir()
            .join(format!("sb-keygen-n-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).expect("create");

        // A helper that succeeds and prints nothing, which is the shape of the
        // failure: not a crash, an empty answer.
        #[cfg(windows)]
        let helper = {
            let path = dir.join("silent.bat");
            std::fs::write(&path, "@echo off\r\n").expect("write");
            path
        };
        #[cfg(not(windows))]
        let helper = {
            use std::os::unix::fs::PermissionsExt;
            let path = dir.join("silent.sh");
            std::fs::write(&path, "#!/bin/sh\nexit 0\n").expect("write");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
                .expect("chmod");
            path
        };

        let spec = NewKey {
            name: "id_silent".into(),
            key_type: KeyType::Ed25519,
            comment: "superbackup@test".into(),
        };
        let passphrase = crate::crypto::generate_passphrase().expect("passphrase");
        let result = generate_with_helper(&dir, &spec, Some(&passphrase), &helper).await;

        // Either ssh-keygen refused outright, or it made an unprotected key
        // and the check caught it. What must not happen is a success that
        // leaves the caller believing the key is protected.
        match result {
            Err(e) => {
                let text = e.to_string();
                assert!(
                    text.contains("without a passphrase") || text.contains("passphrase"),
                    "{text}"
                );
            }
            Ok(made) => panic!("an unprotected key was reported as made: {made:?}"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

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
        let made = generate(&dir, &spec, None).await.expect("generated");
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
        let again = generate(&dir, &spec, None).await.expect_err("refused");
        assert!(again.to_string().contains("already exists"), "{again}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The passphrase route, end to end with the real tool: the key must come
    /// out actually encrypted, and the passphrase must never have been in an
    /// argument. The first half is checked against the file; the second is
    /// structural — `generate` has no code path that puts one there.
    #[tokio::test]
    async fn a_protected_key_is_really_protected() {
        if super::super::agent::ssh_keygen().is_none() {
            eprintln!("ssh-keygen is not installed; skipping");
            return;
        }
        let dir = std::env::temp_dir()
            .join(format!("sb-keygen-p-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).expect("create");

        let spec = NewKey {
            name: "id_protected".into(),
            key_type: KeyType::Ed25519,
            comment: "superbackup@test".into(),
        };
        let passphrase = crate::crypto::generate_passphrase().expect("passphrase");
        let helper = write_askpass(&dir, &passphrase);
        let made = generate_with_helper(&dir, &spec, Some(&passphrase), &helper)
            .await
            .expect("generated");
        assert!(made.protected, "it was asked for and it was checked");

        // Read back from the file, not from what was asked for: a passphrase
        // that silently did not take would leave an unprotected key whose
        // vault entry claimed otherwise.
        let found = super::super::ssh::discover_in(&dir);
        let key = found.iter().find(|k| k.private_path == made.private_path).expect("found");
        assert_eq!(key.encrypted, Some(true), "the key on disk is encrypted");

        // A passphrase below OpenSSH's minimum is refused here rather than by
        // ssh-keygen, whose message about it is not obvious.
        let short = NewKey { name: "id_short".into(), ..spec.clone() };
        let err =
            generate_with_helper(&dir, &short, Some(&crate::secret::Secret::from_str("abc")), &helper)
                .await
                .expect_err("too short");
        assert!(err.to_string().contains("five characters"), "{err}");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
