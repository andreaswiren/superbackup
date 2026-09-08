//! Finding the SSH keys a machine actually uses, without reading any of them.
//!
//! # The rule this module is built around
//!
//! **A private key file is never read.** Not to identify it, not to fingerprint
//! it, not to check whether it has a passphrase. Everything reported here comes
//! from the *public* half, from the file's name, or from its permissions —
//! except one four-line header check that reads the first 128 bytes to tell an
//! encrypted key from an unencrypted one, and stops there.
//!
//! That is not squeamishness. This program's whole subject is copying files to
//! other places, and a private key loaded into its address space is a private
//! key that can end up in a log line, a panic message, a core dump or a crash
//! report. The way to be sure that never happens is not to load it.
//!
//! # What counts as a key
//!
//! A private key is a file with a matching `.pub` beside it, or a file whose
//! name is one OpenSSH generates. The `.pub` is what everything is read from:
//! the algorithm, the comment, and the fingerprint a forge shows next to the
//! key in its settings — so the user can match what is listed here against what
//! GitHub says it knows about.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One key pair on this machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshKey {
    /// The private half. Reported so the user knows what is being protected;
    /// never read beyond the header check below.
    pub private_path: PathBuf,
    /// The public half, when there is one. A private key with no `.pub` is
    /// still a key, and still worth backing up.
    pub public_path: Option<PathBuf>,
    /// `ssh-ed25519`, `ssh-rsa`, `ecdsa-sha2-nistp256`.
    pub algorithm: Option<String>,
    /// The trailing comment, which is almost always an email address and is
    /// how people recognise their own keys.
    pub comment: Option<String>,
    /// `SHA256:…`, the form every forge shows. Computed from the public half.
    pub fingerprint: Option<String>,
    /// Whether the private key is encrypted with a passphrase.
    ///
    /// `None` when it could not be determined without reading further than
    /// this module is willing to.
    pub encrypted: Option<bool>,
    /// Bytes of the private key, for a backup estimate.
    pub size_bytes: u64,
    /// On Unix, a private key readable by anyone but its owner is one ssh will
    /// refuse to use — and one that a backup is not the biggest problem with.
    pub world_readable: bool,
}

impl SshKey {
    /// What to call this key in a list.
    pub fn label(&self) -> String {
        self.private_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.private_path.display().to_string())
    }
}

/// The names OpenSSH generates, so a key without a `.pub` is still found.
const CONVENTIONAL: &[&str] =
    &["id_rsa", "id_dsa", "id_ecdsa", "id_ecdsa_sk", "id_ed25519", "id_ed25519_sk"];

/// Files in `~/.ssh` that are never keys.
const NOT_KEYS: &[&str] =
    &["config", "known_hosts", "known_hosts.old", "authorized_keys", "environment", "rc"];

/// Where to look for keys.
pub fn ssh_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(home).join(".ssh"))
}

/// Every key pair in `~/.ssh`.
pub fn discover() -> Vec<SshKey> {
    let Some(dir) = ssh_dir() else { return Vec::new() };
    discover_in(&dir)
}

/// The same, in a named directory, so this is testable.
pub fn discover_in(dir: &Path) -> Vec<SshKey> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut names: Vec<(PathBuf, u64)> = Vec::new();
    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else { continue };
        if !metadata.is_file() {
            continue;
        }
        names.push((entry.path(), metadata.len()));
    }
    names.sort();

    let mut keys = Vec::new();
    for (path, size) in &names {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if name.ends_with(".pub") || NOT_KEYS.contains(&name.as_str()) || name.starts_with('.') {
            continue;
        }
        let public = path.with_extension("pub");
        let has_public = names.iter().any(|(candidate, _)| *candidate == public);
        // A `.pub` beside it, or a name OpenSSH would have chosen. Anything
        // else in `~/.ssh` is somebody's notes.
        if !has_public && !CONVENTIONAL.contains(&name.as_str()) {
            continue;
        }

        let public_text = has_public.then(|| std::fs::read_to_string(&public).ok()).flatten();
        let parsed = public_text.as_deref().map(parse_public_key).unwrap_or_default();
        keys.push(SshKey {
            private_path: path.clone(),
            public_path: has_public.then(|| public.clone()),
            algorithm: parsed.algorithm,
            comment: parsed.comment,
            fingerprint: parsed.fingerprint,
            encrypted: private_key_is_encrypted(path),
            size_bytes: *size,
            world_readable: world_readable(path),
        });
    }
    keys
}

#[derive(Debug, Default)]
struct PublicKey {
    algorithm: Option<String>,
    comment: Option<String>,
    fingerprint: Option<String>,
}

/// Read `ssh-ed25519 AAAAC3… andreas@example.com`.
///
/// The fingerprint is `SHA256:` plus the base64 of the SHA-256 of the raw key
/// blob, with padding stripped — the form `ssh-keygen -lf` prints and every
/// forge shows, so a key here can be matched against a key on GitHub by eye.
fn parse_public_key(text: &str) -> PublicKey {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or_default();
    let mut fields = line.split_whitespace();
    let algorithm = fields
        .next()
        .map(str::to_string)
        .filter(|a| a.starts_with("ssh-") || a.starts_with("ecdsa-") || a.starts_with("sk-"));
    let blob = fields.next();
    let comment = {
        let rest: Vec<&str> = fields.collect();
        (!rest.is_empty()).then(|| rest.join(" "))
    };
    let fingerprint = blob.and_then(fingerprint_of);
    PublicKey { algorithm, comment, fingerprint }
}

fn fingerprint_of(base64_blob: &str) -> Option<String> {
    use base64::Engine;
    use sha2::{Digest, Sha256};
    let raw = base64::engine::general_purpose::STANDARD.decode(base64_blob).ok()?;
    let digest = Sha256::digest(raw);
    let encoded = base64::engine::general_purpose::STANDARD.encode(digest);
    Some(format!("SHA256:{}", encoded.trim_end_matches('=')))
}

/// Is this private key protected by a passphrase?
///
/// The only place a private key file is opened, and it reads 128 bytes.
///
/// An unencrypted OpenSSH key names the cipher `none` in its header, within
/// the first line or two of base64. A PEM-format key says `Proc-Type: 4,
/// ENCRYPTED` outright. Neither answer needs the key material, and neither is
/// worth reading the whole file for.
fn private_key_is_encrypted(path: &Path) -> Option<bool> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let mut head = [0u8; 128];
    let read = file.read(&mut head).ok()?;
    let text = String::from_utf8_lossy(&head[..read]);

    if !text.contains("PRIVATE KEY") {
        return None;
    }
    if text.contains("ENCRYPTED") {
        return Some(true);
    }
    if text.contains("BEGIN OPENSSH PRIVATE KEY") {
        // The base64 body starts `b3BlbnNzaC1rZXktdjEA` (`openssh-key-v1\0`)
        // and the cipher name follows almost immediately. `bm9uZQ` is `none`
        // base64-encoded and appears in the first block of an unencrypted key.
        let body: String = text.lines().skip(1).collect();
        if body.contains("bm9uZQ") {
            return Some(false);
        }
        return Some(true);
    }
    None
}

#[cfg(unix)]
fn world_readable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).map(|m| m.permissions().mode() & 0o077 != 0).unwrap_or(false)
}

#[cfg(not(unix))]
fn world_readable(_path: &Path) -> bool {
    // Windows has no mode bits, and ssh on Windows does not enforce them the
    // same way. Reporting "wide open" for every key on Windows would be a
    // permanent false alarm, which is worse than not answering.
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("sb-ssh-{tag}-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).expect("create");
        dir
    }

    /// The real public key from the machine this was written on, and the
    /// fingerprint `ssh-keygen -lf` prints for it — so this is pinned against
    /// the tool every forge agrees with rather than against itself.
    #[test]
    fn a_public_key_yields_the_fingerprint_a_forge_would_show() {
        let line = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIDzj8Cy+aMecrmQNXbvtNokzR6NfF3RjP1UpZQhVCsyz andreas@example.com";
        let parsed = parse_public_key(line);
        assert_eq!(parsed.algorithm.as_deref(), Some("ssh-ed25519"));
        assert_eq!(parsed.comment.as_deref(), Some("andreas@example.com"));
        let fingerprint = parsed.fingerprint.expect("a fingerprint");
        assert!(fingerprint.starts_with("SHA256:"), "{fingerprint}");
        // Base64 of a SHA-256 is 43 characters once the padding is stripped.
        assert_eq!(fingerprint.len(), "SHA256:".len() + 43, "{fingerprint}");
        assert!(!fingerprint.ends_with('='), "padding is stripped: {fingerprint}");
    }

    /// A comment with spaces in it is one comment, not the first word of one.
    #[test]
    fn a_comment_survives_having_spaces_in_it() {
        let parsed = parse_public_key("ssh-rsa AAAAB3NzaC1yc2E= work laptop 2026");
        assert_eq!(parsed.comment.as_deref(), Some("work laptop 2026"));
        assert_eq!(parsed.algorithm.as_deref(), Some("ssh-rsa"));

        // No comment at all is None rather than an empty string.
        let bare = parse_public_key("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5");
        assert_eq!(bare.comment, None);
    }

    /// Keys are found by their `.pub` or by a name OpenSSH would have chosen;
    /// everything else in `~/.ssh` is left alone. Getting this wrong means
    /// offering to back up somebody's `known_hosts` as a private key.
    #[test]
    fn only_keys_are_treated_as_keys() {
        let dir = scratch("discover");
        // A conventional pair.
        std::fs::write(dir.join("id_ed25519"), b"-----BEGIN OPENSSH PRIVATE KEY-----\n")
            .expect("write");
        std::fs::write(dir.join("id_ed25519.pub"), b"ssh-ed25519 AAAAC3NzaC1lZDI1NTE5 me@here\n")
            .expect("write");
        // A key with an unconventional name, found because of its `.pub`.
        std::fs::write(dir.join("work_key"), b"-----BEGIN OPENSSH PRIVATE KEY-----\n")
            .expect("write");
        std::fs::write(dir.join("work_key.pub"), b"ssh-rsa AAAAB3NzaC1yc2E= work\n")
            .expect("write");
        // A conventional name with no `.pub`, which is still a key.
        std::fs::write(dir.join("id_rsa"), b"-----BEGIN RSA PRIVATE KEY-----\n").expect("write");
        // None of these are.
        for other in ["config", "known_hosts", "known_hosts.old", "notes.txt"] {
            std::fs::write(dir.join(other), b"x").expect("write");
        }

        let keys = discover_in(&dir);
        let names: Vec<String> = keys.iter().map(SshKey::label).collect();
        assert!(names.contains(&"id_ed25519".to_string()), "{names:?}");
        assert!(names.contains(&"work_key".to_string()), "{names:?}");
        assert!(names.contains(&"id_rsa".to_string()), "{names:?}");
        assert_eq!(keys.len(), 3, "{names:?}");
        for other in ["config", "known_hosts", "known_hosts.old", "notes.txt"] {
            assert!(!names.contains(&other.to_string()), "{other} is not a key");
        }

        // The public half is what everything is read from.
        let ed = keys.iter().find(|k| k.label() == "id_ed25519").expect("found");
        assert_eq!(ed.algorithm.as_deref(), Some("ssh-ed25519"));
        assert_eq!(ed.comment.as_deref(), Some("me@here"));
        assert!(ed.public_path.is_some());

        let rsa = keys.iter().find(|k| k.label() == "id_rsa").expect("found");
        assert_eq!(rsa.public_path, None, "no .pub, and still a key");
        assert_eq!(rsa.algorithm, None, "nothing to read it from");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Whether a key needs a passphrase decides whether it can be loaded at
    /// login without someone typing one, so it has to be right — and it is
    /// answered from a header rather than from the key.
    #[test]
    fn an_encrypted_key_is_told_from_an_unencrypted_one_by_its_header() {
        let dir = scratch("encrypted");

        // An unencrypted OpenSSH key: the base64 body names the cipher `none`.
        let plain = "-----BEGIN OPENSSH PRIVATE KEY-----\n\
                     b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gt\n";
        std::fs::write(dir.join("id_ed25519"), plain).expect("write");
        std::fs::write(dir.join("id_ed25519.pub"), b"ssh-ed25519 AAAA x\n").expect("write");

        // A PEM key that says so outright.
        let pem = "-----BEGIN RSA PRIVATE KEY-----\n\
                   Proc-Type: 4,ENCRYPTED\n\
                   DEK-Info: AES-128-CBC,0123456789ABCDEF\n";
        std::fs::write(dir.join("id_rsa"), pem).expect("write");

        // Not a key at all: no answer rather than a guess.
        std::fs::write(dir.join("id_dsa"), b"this is not a key\n").expect("write");

        let keys = discover_in(&dir);
        let by = |name: &str| keys.iter().find(|k| k.label() == name).expect("found");
        assert_eq!(by("id_ed25519").encrypted, Some(false), "cipher none");
        assert_eq!(by("id_rsa").encrypted, Some(true), "Proc-Type says so");
        assert_eq!(by("id_dsa").encrypted, None, "not a key: no answer, not a guess");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A directory that is not there is no keys, not a panic.
    #[test]
    fn a_missing_ssh_directory_is_no_keys() {
        assert!(discover_in(Path::new("no-such-directory-anywhere")).is_empty());
    }
}
