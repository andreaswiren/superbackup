//! Controls this review attacked and could **not** break. These tests pass;
//! they exist so the finding "we checked and it holds" is regression-guarded
//! rather than a sentence in a report.

use superbackup_core::crypto::kdf::KdfParams;
use superbackup_core::crypto::{Envelope, Vault};
use superbackup_core::error::Error;
use superbackup_core::model::SecretRef;
use superbackup_core::secret::Secret;

fn sealed() -> (Vec<u8>, Secret) {
    let pass = Secret::from_str("the-master-passphrase");
    let mut vault = Vault::create_unchecked(
        &pass,
        KdfParams {
            memory_kib: 64,
            iterations: 2,
            ..KdfParams::insecure_for_tests().expect("kdf")
        },
    )
    .expect("vault");
    vault.put(SecretRef("s3.access:1".into()), Secret::from_str("AKIA")).expect("put");
    let bytes = vault.seal().expect("seal");
    (bytes, pass)
}

fn reserialise(e: &Envelope) -> Vec<u8> {
    serde_json::to_vec(e).expect("serialise")
}

/// Every security-relevant header field is inside the AEAD's associated data.
/// Attempts to lower the KDF cost, swap the salt, restamp the identity or
/// roll back `updated_at` all make the file refuse to open. I could not find
/// a field that is left out.
#[test]
fn no_header_field_can_be_tampered_with_and_still_decrypt() {
    let (bytes, pass) = sealed();
    let base = Envelope::parse(&bytes).expect("parse");
    Vault::unlock(&bytes, &pass).expect("the untampered file opens");

    let mut cheap = base.clone();
    cheap.header.kdf.memory_kib = 8;
    cheap.header.kdf.iterations = 1;

    let mut resalted = base.clone();
    resalted.header.kdf.salt[0] ^= 0x01;

    let mut restamped = base.clone();
    restamped.header.vault_id = uuid::Uuid::from_u128(0xdead_beef);

    let mut rolled = base.clone();
    rolled.header.updated_at -= chrono::Duration::days(365);

    let mut recreated = base.clone();
    recreated.header.created_at -= chrono::Duration::days(365);

    let mut renonced = base.clone();
    renonced.nonce[0] ^= 0x01;

    let mut retagged = base;
    retagged.ciphertext[0] ^= 0x01;

    for (what, envelope) in [
        ("KDF downgrade", cheap),
        ("salt swap", resalted),
        ("vault_id swap", restamped),
        ("updated_at rollback", rolled),
        ("created_at rewrite", recreated),
        ("nonce change", renonced),
        ("ciphertext bit flip", retagged),
    ] {
        let tampered = reserialise(&envelope);
        match Vault::unlock(&tampered, &pass) {
            Err(Error::BadPassphrase) => {}
            other => panic!("{what} must not decrypt, got {other:?}"),
        }
    }
}

/// Reformatting the file — different whitespace, different key order — does
/// not break it, because the AAD is derived from the parsed struct. Both
/// halves of the claim are true at once.
#[test]
fn reformatting_the_file_does_not_break_it() {
    let (bytes, pass) = sealed();
    let parsed = Envelope::parse(&bytes).expect("parse");
    let compact = serde_json::to_vec(&parsed).expect("compact");
    let pretty = serde_json::to_vec_pretty(&parsed).expect("pretty");
    Vault::unlock(&compact, &pass).expect("compact form opens");
    Vault::unlock(&pretty, &pass).expect("pretty form opens");
}

/// A wrong passphrase and a tampered ciphertext produce the identical error
/// value with no distinguishing text. Structural damage is reported
/// differently, but structural damage is not passphrase-dependent, so it is
/// not an oracle.
#[test]
fn a_wrong_passphrase_and_a_tampered_file_are_the_same_error() {
    let (bytes, _pass) = sealed();
    let wrong = Vault::unlock(&bytes, &Secret::from_str("not-the-passphrase"))
        .expect_err("wrong passphrase");

    let mut envelope = Envelope::parse(&bytes).expect("parse");
    envelope.ciphertext[4] ^= 0x01;
    let corrupt =
        Vault::unlock(&reserialise(&envelope), &Secret::from_str("the-master-passphrase"))
            .expect_err("tampered ciphertext");

    assert!(matches!(wrong, Error::BadPassphrase));
    assert!(matches!(corrupt, Error::BadPassphrase));
    assert_eq!(
        wrong.to_string(),
        corrupt.to_string(),
        "the two must be textually indistinguishable"
    );
}

/// A locked vault physically cannot answer for secret material, and its
/// `Debug` renders only metadata. Both hold.
#[test]
fn a_locked_vault_answers_nothing_and_debug_reveals_nothing() {
    let (bytes, pass) = sealed();
    let mut vault = Vault::unlock(&bytes, &pass).expect("unlock");
    assert!(vault.get(&SecretRef("s3.access:1".into())).expect("get").is_some());

    let rendered = format!("{vault:?}");
    assert!(!rendered.contains("AKIA"), "{rendered}");

    vault.lock();
    assert!(matches!(vault.get(&SecretRef("s3.access:1".into())), Err(Error::Locked)));
    assert!(matches!(vault.list_refs(), Err(Error::Locked)));
    assert!(matches!(vault.opened(), Err(Error::Locked)));
}

/// The IPC line cap is enforced *before* the bytes are accumulated: a peer
/// that writes a huge line without a newline is refused rather than buffered.
#[tokio::test]
async fn the_ipc_line_cap_is_enforced_before_buffering() {
    let limit = 1024usize;
    let huge = vec![b'x'; 8 * 1024 * 1024];
    let mut reader = tokio::io::BufReader::new(&huge[..]);
    let mut buf = Vec::new();
    let err = superbackup_core::ipc::codec::read_line(&mut reader, &mut buf, limit)
        .await
        .expect_err("must refuse");
    assert!(matches!(err, superbackup_core::ipc::codec::LineError::TooLong { .. }));
    assert!(buf.len() <= limit, "it buffered {} bytes despite a {limit}-byte cap", buf.len());
}
