//! Sealing SSH keys for a folder that other machines can read.
//!
//! The whole feature rests on one property: **what lands in the shared folder
//! is useless to whoever finds it**. OneDrive syncs to every device on the
//! account, a bucket is one policy change from public, and a private key is
//! useful to anyone holding it — so a test that only checked the round trip
//! would be missing the point of the exercise.

use superbackup_core::credentials::{KeyBundle, BUNDLE_VERSION};
use superbackup_core::secret::Secret;

const PASSPHRASE: &str = "correct horse battery staple";

/// A real OpenSSH private key's opening line, so the "is it in there?" checks
/// are looking for something a scanner would actually recognise.
const KEY_BODY: &[u8] = b"-----BEGIN OPENSSH PRIVATE KEY-----\n\
                          b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAAB\n\
                          -----END OPENSSH PRIVATE KEY-----\n";

fn scratch(tag: &str) -> std::path::PathBuf {
    let dir =
        std::env::temp_dir().join(format!("sb-bundle-{tag}-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).expect("create");
    dir
}

fn a_key_pair(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    std::fs::write(dir.join("id_ed25519"), KEY_BODY).expect("write");
    std::fs::write(dir.join("id_ed25519.pub"), b"ssh-ed25519 AAAAC3 me@here\n").expect("write");
    vec![dir.join("id_ed25519"), dir.join("id_ed25519.pub")]
}

/// The property the whole design exists for.
///
/// Somebody who finds this file in a shared folder — a second device on the
/// OneDrive account, anyone who can list the bucket — must not be able to get
/// a key out of it.
#[test]
fn a_sealed_bundle_carries_no_recognisable_key_material() {
    let dir = scratch("opaque");
    let paths = a_key_pair(&dir);
    let bundle = KeyBundle::gather("awpc34", &paths).expect("gathered");
    let sealed = bundle.seal(&Secret::from_str(PASSPHRASE)).expect("sealed");

    // Nothing that looks like a key, a file name, or the machine it came from.
    let haystack = String::from_utf8_lossy(&sealed);
    for needle in [
        "BEGIN OPENSSH PRIVATE KEY",
        "b3BlbnNzaC1rZXktdjEA",
        "ssh-ed25519",
        "id_ed25519",
        "me@here",
        "awpc34",
    ] {
        assert!(!haystack.contains(needle), "the sealed bundle leaks {needle:?}");
    }
    // And not as raw bytes either, in case any of it survived un-encoded.
    assert!(
        sealed.windows(KEY_BODY.len()).all(|w| w != KEY_BODY),
        "the key body is in the sealed bytes verbatim"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The keys come back, byte for byte, on a machine with the passphrase.
#[test]
fn a_bundle_round_trips_through_a_shared_folder() {
    let source = scratch("source");
    let paths = a_key_pair(&source);
    let original = std::fs::read(&paths[0]).expect("read");

    let bundle = KeyBundle::gather("awpc34", &paths).expect("gathered");
    let sealed = bundle.seal(&Secret::from_str(PASSPHRASE)).expect("sealed");

    // The other machine.
    let opened = KeyBundle::unseal(&sealed, &Secret::from_str(PASSPHRASE)).expect("unsealed");
    assert_eq!(opened.version, BUNDLE_VERSION);
    assert_eq!(opened.machine, "awpc34", "it says which machine sealed it");
    assert_eq!(opened.files.len(), 2);

    let target = scratch("target");
    let written = opened.write_into(&target, false).expect("written");
    assert_eq!(written.len(), 2, "{written:?}");
    assert_eq!(std::fs::read(target.join("id_ed25519")).expect("read"), original);
    assert!(target.join("id_ed25519.pub").exists());

    // Owner-only where the platform can express it.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(target.join("id_ed25519"))
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "ssh refuses a key anyone else can read");
    }

    let _ = std::fs::remove_dir_all(&source);
    let _ = std::fs::remove_dir_all(&target);
}

/// The wrong passphrase gets nothing, and says so as a passphrase problem
/// rather than as a corrupt file — those lead a user to different actions.
#[test]
fn the_wrong_passphrase_opens_nothing() {
    let dir = scratch("wrong");
    let paths = a_key_pair(&dir);
    let sealed = KeyBundle::gather("awpc34", &paths)
        .expect("gathered")
        .seal(&Secret::from_str(PASSPHRASE))
        .expect("sealed");

    let err = KeyBundle::unseal(&sealed, &Secret::from_str("not the passphrase"))
        .expect_err("must not open");
    assert_eq!(err.code(), superbackup_core::ErrorCode::BadPassphrase, "{err}");

    // Damaged bytes are a different answer again.
    let mut damaged = sealed.clone();
    let last = damaged.len() - 1;
    damaged[last] ^= 0xff;
    assert!(KeyBundle::unseal(&damaged, &Secret::from_str(PASSPHRASE)).is_err());

    let _ = std::fs::remove_dir_all(&dir);
}

/// The most likely mistake with this feature is unsealing an old bundle over
/// the key a machine is currently using. Existing files are left alone unless
/// overwriting is asked for.
#[test]
fn unsealing_does_not_quietly_replace_the_key_in_use() {
    let source = scratch("keep-source");
    let paths = a_key_pair(&source);
    let sealed = KeyBundle::gather("awpc34", &paths)
        .expect("gathered")
        .seal(&Secret::from_str(PASSPHRASE))
        .expect("sealed");
    let opened = KeyBundle::unseal(&sealed, &Secret::from_str(PASSPHRASE)).expect("unsealed");

    let target = scratch("keep-target");
    std::fs::write(target.join("id_ed25519"), b"the key this machine uses").expect("write");

    let written = opened.write_into(&target, false).expect("written");
    assert_eq!(written, vec!["id_ed25519.pub".to_string()], "only the file that was missing");
    assert_eq!(
        std::fs::read(target.join("id_ed25519")).expect("read"),
        b"the key this machine uses",
        "the key in use was not touched"
    );

    // And it is replaced when that is what was asked for.
    let written = opened.write_into(&target, true).expect("written");
    assert_eq!(written.len(), 2);
    assert_eq!(std::fs::read(target.join("id_ed25519")).expect("read"), KEY_BODY);

    let _ = std::fs::remove_dir_all(&source);
    let _ = std::fs::remove_dir_all(&target);
}

/// A bundle comes out of a folder shared between machines, so its contents are
/// input from somewhere the user does not wholly control. A name that escapes
/// the key folder must not write anything at all — not even the files either
/// side of it.
#[test]
fn a_bundle_with_an_escaping_name_writes_nothing() {
    let dir = scratch("hostile");
    let paths = a_key_pair(&dir);
    let mut bundle = KeyBundle::gather("awpc34", &paths).expect("gathered");
    // As if a second machine, or somebody with write access to the shared
    // folder, had sealed this.
    bundle.files[1].name = "../../.bashrc".to_string();

    let target = scratch("hostile-target");
    let err = bundle.write_into(&target, true).expect_err("must be refused");
    assert!(err.to_string().contains("file name"), "{err}");

    // Nothing at all: the names are all checked before the first write, so a
    // bundle does not leave half of itself behind.
    assert!(!target.join("id_ed25519").exists(), "no file was written");
    assert!(!dir.parent().expect("parent").join(".bashrc").exists());

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&target);
}
