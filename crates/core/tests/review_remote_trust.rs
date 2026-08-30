//! Hostile review of the shared-config trust path.
//!
//! Claims under test (THREAT_MODEL.md §A4, envelope.rs, remote.rs):
//!
//! * "`vault_id` ... lets the remote sync notice 'this is a completely
//!   different vault, not a newer version of mine' and refuse to overwrite,
//!   instead of cheerfully replacing the user's keys with a stranger's."
//! * "When `trusted_signers` is populated, the vault's detached Ed25519
//!   signature must verify against a pinned key or the pull is rejected."
//! * `updated_at` — "which is what lets a remote sync say 'theirs is newer'".

use superbackup_core::crypto::kdf::KdfParams;
use superbackup_core::crypto::{Envelope, Vault};
use superbackup_core::model::{Config, RemoteConfigSource, SecretRef};
use superbackup_core::remote::{verify_pull, FetchedVault};
use superbackup_core::secret::Secret;

fn kdf() -> KdfParams {
    KdfParams::insecure_for_tests().expect("kdf")
}

fn source(trusted: Vec<String>) -> RemoteConfigSource {
    RemoteConfigSource {
        url: "https://github.com/team/cfg".into(),
        branch: "main".into(),
        path: "config.sbvault".into(),
        auth: Default::default(),
        auto_pull: false,
        pull_interval_minutes: 60,
        allow_push: false,
        last_pull_at: None,
        last_known_commit: None,
        trusted_signers: trusted,
    }
}

/// `verify_pull` compares nothing about *time*. A strictly older version of
/// the very same vault — same `vault_id`, same passphrase, older `updated_at`,
/// valid signature if it was signed — passes every check and produces a plan
/// that `apply_pull` will happily install.
///
/// So anyone who can write the shared repository (or replay a previously
/// published blob) can silently roll the installation back: reinstating an S3
/// key the user rotated away, restoring a destination they deleted, or undoing
/// a `trusted_signers` change. `updated_at` and `last_known_commit` both exist
/// in the model and neither is consulted here.
#[test]
fn an_older_version_of_the_same_vault_is_accepted_without_complaint() {
    let pass = Secret::from_str("shared-passphrase");

    // v1: the old state, with a credential the user is about to rotate away.
    let mut vault = Vault::create_unchecked(&pass, kdf()).expect("vault");
    vault
        .put(SecretRef("s3.access:1".into()), Secret::from_str("OLD-COMPROMISED-KEY"))
        .expect("put");
    let v1 = vault.seal().expect("seal v1");

    // v2: the user rotates the credential and publishes.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    vault
        .put(SecretRef("s3.access:1".into()), Secret::from_str("NEW-ROTATED-KEY"))
        .expect("put");
    let v2 = vault.seal().expect("seal v2");
    assert_ne!(v1, v2);

    let e1 = Envelope::parse(&v1).expect("parse v1");
    let e2 = Envelope::parse(&v2).expect("parse v2");
    assert_eq!(e1.header.vault_id, e2.header.vault_id, "same vault");
    assert!(e1.header.updated_at < e2.header.updated_at, "v1 is genuinely older");

    // The local machine already holds v2. The attacker serves v1.
    let local = Vault::unlock(&v2, &pass).expect("local");
    let fetched =
        FetchedVault { bytes: v1, source_url: "https://example/x".into(), sha: None };

    let plan = verify_pull(&fetched, &Config::default(), &local, &source(Vec::new()), &pass)
        .expect("verify_pull accepts it");

    let rolled_back = Vault::unlock(plan.bytes(), &pass).expect("unlock the plan");
    let key = rolled_back
        .get(&SecretRef("s3.access:1".into()))
        .expect("get")
        .expect("present");

    assert_ne!(
        key.expose(),
        b"OLD-COMPROMISED-KEY",
        "verify_pull accepted a strictly older version of the same vault and the \
         plan would reinstate the rotated-away credential; nothing compares \
         header.updated_at or last_known_commit against local state"
    );
}

/// `verify_pull` reads `trusted_signers` from the **local** config, then
/// `apply_pull` overwrites the whole local config — `remote` included — with
/// the pulled one. So the pinned signer list is data that the pulled artifact
/// itself supplies.
///
/// One accepted pull is enough to clear the pin (and to repoint `url`) for
/// every pull afterwards. Pinning that a single successful publish can switch
/// off is not pinning.
#[test]
fn a_pulled_config_can_clear_the_pinned_signer_list() {
    let pass = Secret::from_str("shared-passphrase");
    let mut vault = Vault::create_unchecked(&pass, kdf()).expect("vault");

    // The publisher embeds a config whose remote source pins nobody.
    let mut published = Config::default();
    published.remote = Some(source(Vec::new()));
    vault.set_embedded_config(Some(published)).expect("embed");
    let signed = vault.seal_signed().expect("seal signed");

    let fingerprint = vault.signer_fingerprint().expect("fingerprint");

    // The local machine pins that publisher, so this pull verifies.
    let local_source = source(vec![fingerprint]);
    let mut local_config = Config::default();
    local_config.remote = Some(local_source.clone());

    let local = Vault::unlock(&signed, &pass).expect("local");
    let fetched =
        FetchedVault { bytes: signed, source_url: "https://example/x".into(), sha: None };

    let plan = verify_pull(&fetched, &local_config, &local, &local_source, &pass)
        .expect("a correctly signed vault verifies");

    let incoming = plan.incoming_config.as_ref().expect("the vault published a config");
    let incoming_remote = incoming.remote.as_ref().expect("with a remote source");

    assert!(
        !incoming_remote.trusted_signers.is_empty(),
        "the pulled configuration carries an empty trusted_signers list, and \
         apply_pull writes it over the local one via Store::set_config — so a \
         single accepted pull permanently disables signature pinning for this \
         remote"
    );
}
