//! Adversarial tests for the `config.sbvault` file format.
//!
//! Every test here corresponds to something an attacker who has the file, or a
//! filesystem that damaged it, can actually do. The bar is: no panic, no
//! silent acceptance, and no leak.

use superbackup_core::crypto::{KdfParams, Vault};
use superbackup_core::error::Error;
use superbackup_core::model::SecretRef;
use superbackup_core::secret::Secret;

/// Cheap KDF parameters. A real vault costs half a second to open by design;
/// these tests open hundreds.
///
/// The cost is set slightly above the absolute floor so that the "an attacker
/// downgrades the KDF parameters" test has somewhere to downgrade *to*. Even
/// at 512 KiB and two passes a derivation is well under a millisecond.
fn kdf() -> KdfParams {
    KdfParams {
        memory_kib: 512,
        iterations: 2,
        ..KdfParams::insecure_for_tests().expect("test kdf parameters")
    }
}

fn pass(s: &str) -> Secret {
    Secret::from_str(s)
}

/// A sealed vault with a couple of recognisable secrets in it.
fn sealed(passphrase: &str) -> Vec<u8> {
    let mut vault = Vault::create_unchecked(&pass(passphrase), kdf()).expect("create");
    vault
        .put(SecretRef("s3.access:1".into()), Secret::from_str("AKIAEXAMPLECANARY"))
        .expect("put access key");
    vault
        .put(SecretRef("repo.passphrase:2".into()), Secret::from_str("REPOCANARYVALUE"))
        .expect("put repo passphrase");
    vault.seal().expect("seal")
}

#[test]
fn round_trip_preserves_every_secret() {
    let bytes = sealed("correct horse battery staple");
    let vault = Vault::unlock(&bytes, &pass("correct horse battery staple")).expect("unlock");

    assert_eq!(
        vault.list_refs().expect("refs"),
        vec![SecretRef("repo.passphrase:2".into()), SecretRef("s3.access:1".into())],
        "handles must round-trip, sorted"
    );
    let access = vault.get(&SecretRef("s3.access:1".into())).expect("get").expect("present");
    assert_eq!(access.expose(), b"AKIAEXAMPLECANARY");
    let repo =
        vault.get(&SecretRef("repo.passphrase:2".into())).expect("get").expect("present");
    assert_eq!(repo.expose(), b"REPOCANARYVALUE");
    assert!(vault.get(&SecretRef("nothing:0".into())).expect("get").is_none());
}

#[test]
fn the_sealed_file_contains_no_plaintext_secret_and_no_plaintext_config() {
    let mut vault = Vault::create_unchecked(&pass("pass"), kdf()).expect("create");
    vault
        .put(SecretRef("s3.secret:9".into()), Secret::from_str("PLAINTEXTCANARY"))
        .expect("put");

    let mut config = superbackup_core::model::Config::default();
    config.machine.label = "MACHINELABELCANARY".into();
    vault.set_embedded_config(Some(config)).expect("embed");

    let bytes = vault.seal().expect("seal");
    let text = String::from_utf8(bytes.clone()).expect("the file must be valid UTF-8");

    assert!(!text.contains("PLAINTEXTCANARY"), "secret leaked into the file:\n{text}");
    assert!(!text.contains("MACHINELABELCANARY"), "config leaked into the file:\n{text}");
    assert!(!text.contains("s3.secret:9"), "even the handle names are inside the ciphertext");

    // The header, by contrast, is deliberately readable.
    assert!(text.contains("SBVAULT"));
    assert!(text.contains("argon2id"));
    assert!(text.contains("xchacha20-poly1305"));

    // And nothing about the vault's own Debug rendering leaks either.
    let rendered = format!("{vault:?}");
    assert!(!rendered.contains("PLAINTEXTCANARY"), "{rendered}");
}

#[test]
fn the_file_is_pure_ascii_so_git_on_windows_cannot_corrupt_it() {
    let bytes = sealed("pass");
    assert!(bytes.is_ascii(), "a non-ASCII byte would make this a binary blob to Git");
    // No bare newlines inside the payload fields; the only newlines are the
    // pretty-printer's structural ones, which `core.autocrlf` rewriting would
    // not change any parsed value.
    let text = String::from_utf8(bytes).expect("utf8");
    let reparsed: serde_json::Value = serde_json::from_str(&text).expect("json");
    assert_eq!(reparsed["magic"], "SBVAULT");
}

#[test]
fn a_wrong_passphrase_is_rejected() {
    let bytes = sealed("the right one");
    for wrong in ["the wrong one", "", "the right one ", "The right one"] {
        match Vault::unlock(&bytes, &pass(wrong)) {
            Err(Error::BadPassphrase) => {}
            other => panic!("{wrong:?} should not open the vault: {other:?}"),
        }
    }
}

#[test]
fn a_bit_flipped_ciphertext_is_rejected() {
    let bytes = sealed("pass");
    let mut document: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    let ciphertext = document["ciphertext"].as_str().expect("ciphertext").to_string();

    // Flip a base64 symbol in the middle of the ciphertext to a different one.
    let mut chars: Vec<char> = ciphertext.chars().collect();
    let middle = chars.len() / 2;
    chars[middle] = if chars[middle] == 'A' { 'B' } else { 'A' };
    document["ciphertext"] = serde_json::Value::String(chars.into_iter().collect());
    let tampered = serde_json::to_vec(&document).expect("serialise");

    match Vault::unlock(&tampered, &pass("pass")) {
        // Indistinguishable from a wrong passphrase, by design: Poly1305 does
        // not say why it failed, and guessing would be an oracle.
        Err(Error::BadPassphrase) => {}
        other => panic!("a modified ciphertext must not open: {other:?}"),
    }
}

#[test]
fn a_bit_flipped_authentication_tag_is_rejected() {
    let bytes = sealed("pass");
    let mut document: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    let ciphertext = document["ciphertext"].as_str().expect("ciphertext").to_string();
    let mut chars: Vec<char> = ciphertext.chars().collect();
    // The Poly1305 tag is the last 16 bytes, so the last few base64 symbols.
    let last = chars.len() - 2;
    chars[last] = if chars[last] == 'A' { 'B' } else { 'A' };
    document["ciphertext"] = serde_json::Value::String(chars.into_iter().collect());
    let tampered = serde_json::to_vec(&document).expect("serialise");
    assert!(Vault::unlock(&tampered, &pass("pass")).is_err(), "a forged tag must not open");
}

/// The whole point of using the header as associated data.
#[test]
fn a_tampered_header_breaks_decryption_rather_than_being_accepted() {
    let bytes = sealed("pass");

    // 1. The KDF downgrade attack: make the offline grind a million times
    //    cheaper by rewriting the cost parameters.
    let mut document: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    document["header"]["kdf"]["memory_kib"] = serde_json::json!(8);
    document["header"]["kdf"]["iterations"] = serde_json::json!(1);
    let downgraded = serde_json::to_vec(&document).expect("serialise");
    assert!(
        Vault::unlock(&downgraded, &pass("pass")).is_err(),
        "lowering the KDF cost must break the file, not weaken it silently"
    );

    // 2. Swapping the salt.
    let mut document: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    let salt = document["header"]["kdf"]["salt"].as_str().expect("salt").to_string();
    let mut chars: Vec<char> = salt.chars().collect();
    chars[0] = if chars[0] == 'A' { 'B' } else { 'A' };
    document["header"]["kdf"]["salt"] = serde_json::json!(chars.into_iter().collect::<String>());
    let resalted = serde_json::to_vec(&document).expect("serialise");
    assert!(Vault::unlock(&resalted, &pass("pass")).is_err(), "salt substitution");

    // 3. Rolling `updated_at` back, which a remote-sync downgrade would want.
    let mut document: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    document["header"]["updated_at"] = serde_json::json!("2001-01-01T00:00:00Z");
    let backdated = serde_json::to_vec(&document).expect("serialise");
    assert!(Vault::unlock(&backdated, &pass("pass")).is_err(), "timestamp rollback");

    // 4. Re-labelling the vault as somebody else's.
    let mut document: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    document["header"]["vault_id"] = serde_json::json!("00000000-0000-4000-8000-000000000000");
    let reidentified = serde_json::to_vec(&document).expect("serialise");
    assert!(Vault::unlock(&reidentified, &pass("pass")).is_err(), "identity substitution");
}

/// Reformatting must NOT break the file — a `prettier` run or an editor's
/// "format on save" inside a config repository is not an attack.
#[test]
fn reformatting_the_json_keeps_the_vault_openable() {
    let bytes = sealed("pass");
    let document: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    let compact = serde_json::to_vec(&document).expect("compact");
    assert_ne!(compact, bytes, "the fixture should have been pretty-printed");
    let vault = Vault::unlock(&compact, &pass("pass")).expect("reformatted vault must still open");
    assert_eq!(vault.list_refs().expect("refs").len(), 2);
}

#[test]
fn truncated_files_are_errors_not_panics() {
    let bytes = sealed("pass");
    // `len() - 1` is deliberately absent: the file ends with a structural
    // newline, so dropping it alone yields a byte-for-byte valid vault.
    for cut in [0usize, 1, 10, 50, bytes.len() / 3, bytes.len() / 2, bytes.len() - 2] {
        let truncated = &bytes[..cut];
        let result = Vault::unlock(truncated, &pass("pass"));
        assert!(result.is_err(), "a {cut}-byte prefix must not open");
    }
}

#[test]
fn structurally_broken_files_are_errors_not_panics() {
    let cases: Vec<Vec<u8>> = vec![
        Vec::new(),
        b"{".to_vec(),
        b"null".to_vec(),
        b"[]".to_vec(),
        b"{}".to_vec(),
        br#"{"magic":"SBVAULT"}"#.to_vec(),
        br#"{"magic":"SBVAULT","format_version":1}"#.to_vec(),
        vec![0x00, 0xff, 0xfe, 0x80],
        vec![b'{'; 4096],
        // A Git LFS pointer, which is what a repository with LFS enabled will
        // actually serve instead of the file.
        b"version https://git-lfs.github.com/spec/v1\noid sha256:abc\nsize 1234\n".to_vec(),
        // A GitHub 404 page.
        b"<!DOCTYPE html><html><body>404</body></html>".to_vec(),
    ];
    for case in cases {
        let result = Vault::unlock(&case, &pass("pass"));
        assert!(result.is_err(), "input {:?} must be rejected", String::from_utf8_lossy(&case));
        let locked = Vault::open_locked(&case);
        assert!(locked.is_err(), "input must not even parse");
    }
}

#[test]
fn a_hostile_header_cannot_make_us_allocate_gigabytes() {
    let bytes = sealed("pass");
    let mut document: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    document["header"]["kdf"]["memory_kib"] = serde_json::json!(u32::MAX);
    let hostile = serde_json::to_vec(&document).expect("serialise");

    // This must fail at parse time, before Argon2 is asked to allocate 4 TiB.
    match Vault::open_locked(&hostile) {
        Err(Error::VaultCorrupt(message)) => {
            assert!(message.contains("memory cost"), "{message}");
        }
        other => panic!("expected a bounds error, got {other:?}"),
    }
}

#[test]
fn a_newer_format_version_is_refused_with_a_specific_error() {
    let bytes = sealed("pass");
    let mut document: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    document["format_version"] = serde_json::json!(99);
    let future = serde_json::to_vec(&document).expect("serialise");

    match Vault::open_locked(&future) {
        Err(Error::VaultVersion { found, supported }) => {
            assert_eq!(found, 99);
            assert!(supported < 99);
        }
        other => panic!("expected a version error, got {other:?}"),
    }
    // And the same through the unlock path, so a user with a newer vault gets
    // "upgrade superbackup", not "wrong passphrase".
    assert!(matches!(
        Vault::unlock(&future, &pass("pass")),
        Err(Error::VaultVersion { .. })
    ));
}

#[test]
fn a_vault_from_another_installation_does_not_open_with_our_passphrase() {
    let mine = sealed("shared passphrase");
    let theirs = sealed("shared passphrase");
    assert_ne!(mine, theirs, "two vaults must never seal to the same bytes");

    let a = Vault::unlock(&mine, &pass("shared passphrase")).expect("mine");
    let b = Vault::unlock(&theirs, &pass("shared passphrase")).expect("theirs");
    assert_ne!(a.id(), b.id(), "independently created vaults have distinct identities");
    assert_ne!(
        a.header().kdf.salt,
        b.header().kdf.salt,
        "salt reuse between vaults would let one precomputation attack both"
    );
    assert_ne!(
        a.derive_repo_passphrase(&uuid::Uuid::from_u128(1)).expect("a").expose(),
        b.derive_repo_passphrase(&uuid::Uuid::from_u128(1)).expect("b").expose(),
        "the same destination id in two vaults must not derive the same repository key"
    );
}

#[test]
fn every_seal_uses_a_fresh_nonce() {
    let mut vault = Vault::create_unchecked(&pass("pass"), kdf()).expect("create");
    let mut nonces = std::collections::BTreeSet::new();
    for i in 0..25u32 {
        vault
            .put(SecretRef(format!("k:{i}")), Secret::from_str("value"))
            .expect("put");
        let bytes = vault.seal().expect("seal");
        let document: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        let nonce = document["nonce"].as_str().expect("nonce").to_string();
        assert!(nonces.insert(nonce), "a repeated nonce would destroy confidentiality");
    }
}
