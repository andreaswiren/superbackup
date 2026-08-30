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

use std::path::PathBuf;

use superbackup_core::config::Store;
use superbackup_core::crypto::kdf::KdfParams;
use superbackup_core::crypto::{Envelope, Vault};
use superbackup_core::error::Error;
use superbackup_core::model::{Config, RemoteAuth, RemoteConfigSource, SecretRef};
use superbackup_core::paths::Paths;
use superbackup_core::remote::{
    apply_pull, apply_pull_with, verify_pull, verify_pull_with, FetchedVault, Freshness,
    PullOptions,
};
use superbackup_core::secret::Secret;

/// A throwaway `SUPERBACKUP_HOME`, removed on drop.
struct Home(PathBuf);

impl Home {
    fn new(tag: &str) -> Home {
        let dir = std::env::temp_dir().join(format!(
            "sb-review-{tag}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        Home(dir)
    }
    fn paths(&self) -> Paths {
        Paths::rooted_at(&self.0, false)
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

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

/// Regression guard for H4.
///
/// `verify_pull` used to compare nothing about *time*. A strictly older version
/// of the very same vault — same `vault_id`, same passphrase, older
/// `updated_at`, valid signature if it was signed — passed every check and
/// produced a plan that `apply_pull` would happily install. Anyone who could
/// write the shared repository, or who simply replayed a previously published
/// blob, could silently roll the installation back: reinstating an S3 key the
/// user rotated away, restoring a destination they deleted, or undoing a
/// `trusted_signers` change.
///
/// Note that no cryptography is broken here and none could have caught it: the
/// replayed blob is authentic in every sense. Freshness is a separate property
/// and needs a separate check.
#[test]
fn an_older_version_of_the_same_vault_is_refused_unless_a_rollback_is_confirmed() {
    let pass = Secret::from_str("shared-passphrase");

    // v1: the old state, with a credential the user is about to rotate away.
    let mut vault = Vault::create_unchecked(&pass, kdf()).expect("vault");
    vault
        .put(SecretRef("s3.access:1".into()), Secret::from_str("OLD-COMPROMISED-KEY"))
        .expect("put");
    let v1 = vault.seal().expect("seal v1");

    // v2: the user rotates the credential and publishes.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    vault.put(SecretRef("s3.access:1".into()), Secret::from_str("NEW-ROTATED-KEY")).expect("put");
    let v2 = vault.seal().expect("seal v2");
    assert_ne!(v1, v2);

    let e1 = Envelope::parse(&v1).expect("parse v1");
    let e2 = Envelope::parse(&v2).expect("parse v2");
    assert_eq!(e1.header.vault_id, e2.header.vault_id, "same vault");
    assert!(e1.header.updated_at < e2.header.updated_at, "v1 is genuinely older");

    // The local machine already holds v2. The attacker serves v1.
    let local = Vault::unlock(&v2, &pass).expect("local");
    let fetched = FetchedVault { bytes: v1, source_url: "https://example/x".into(), sha: None };

    let err = verify_pull(&fetched, &Config::default(), &local, &source(Vec::new()), &pass)
        .expect_err(
            "verify_pull accepted a strictly older version of the same vault; the plan \
             would reinstate the rotated-away credential",
        );
    assert!(matches!(err, Error::Remote(_)), "{err:?}");
    let message = format!("{err}");
    assert!(message.contains("older version"), "{message}");

    // A rollback is still possible, but only when the user says so in as many
    // words — restoring from an old published copy is a legitimate recovery
    // move, it just must not happen silently.
    let confirmed = PullOptions { allow_rollback: true, ..PullOptions::default() };
    let plan = verify_pull_with(
        &fetched,
        &Config::default(),
        &local,
        &source(Vec::new()),
        &pass,
        &confirmed,
    )
    .expect("an explicitly confirmed rollback is allowed");

    assert_eq!(plan.freshness, Freshness::Older);
    assert!(plan.rollback_confirmed);
    assert!(plan.age_delta < chrono::Duration::zero(), "the delta is surfaced for the GUI");

    let rolled_back = Vault::unlock(plan.bytes(), &pass).expect("unlock the plan");
    assert_eq!(
        rolled_back.get(&SecretRef("s3.access:1".into())).expect("get").expect("present").expose(),
        b"OLD-COMPROMISED-KEY",
        "and it really is the old state that comes back"
    );

    // The newer blob is of course still accepted with no confirmation at all.
    let forward =
        FetchedVault { bytes: v2.clone(), source_url: "https://example/x".into(), sha: None };
    let plan = verify_pull(&forward, &Config::default(), &local, &source(Vec::new()), &pass)
        .expect("the same version is not a rollback");
    assert_eq!(plan.freshness, Freshness::Same);
    assert!(!plan.rollback_confirmed);
}

/// Regression guard for H5.
///
/// `verify_pull` reads `trusted_signers` from the **local** config, but
/// `apply_pull` used to overwrite the whole local config — `remote` included —
/// with the pulled one. The pinned signer list was therefore data that the
/// pulled artifact itself supplied, and one accepted pull was enough to clear
/// the pin, and to repoint `url`, for every pull afterwards. Pinning that a
/// single successful publish can switch off is not pinning.
///
/// The fix is that the whole `remote` block is machine-local and never
/// travels: `verify_pull` strips it out of the incoming configuration, and
/// `apply_pull` re-asserts the local one even on a hand-built plan.
#[test]
fn a_pulled_config_cannot_clear_the_pinned_signer_list() {
    let pass = Secret::from_str("shared-passphrase");
    let mut vault = Vault::create_unchecked(&pass, kdf()).expect("vault");

    // The publisher embeds a config whose remote source pins nobody.
    let published = Config { remote: Some(source(Vec::new())), ..Config::default() };
    vault.set_embedded_config(Some(published)).expect("embed");
    let signed = vault.seal_signed().expect("seal signed");

    let fingerprint = vault.signer_fingerprint().expect("fingerprint");

    // The local machine pins that publisher, so this pull verifies.
    let local_source = source(vec![fingerprint]);
    let local_config = Config { remote: Some(local_source.clone()), ..Config::default() };

    let local = Vault::unlock(&signed, &pass).expect("local");
    let fetched = FetchedVault { bytes: signed, source_url: "https://example/x".into(), sha: None };

    let plan = verify_pull(&fetched, &local_config, &local, &local_source, &pass)
        .expect("a correctly signed vault verifies");

    let incoming = plan.incoming_config.as_ref().expect("the vault published a config");
    let incoming_remote = incoming.remote.as_ref().expect("with a remote source");

    assert!(
        !incoming_remote.trusted_signers.is_empty(),
        "the plan carries the publisher's empty trusted_signers list; applying it \
         would permanently disable signature pinning for this remote"
    );
    assert_eq!(
        incoming_remote.trusted_signers, local_source.trusted_signers,
        "the local pin must survive into the plan verbatim"
    );
    assert_eq!(incoming_remote.url, local_source.url, "and so must the local URL");
    assert!(plan.incoming_remote_ignored, "the GUI must be able to say this was discarded");
}

/// The same guarantee driven all the way through a real `Store`, because the
/// defect was in `apply_pull`, not in the plan.
#[test]
fn applying_a_pull_preserves_the_local_remote_block() {
    let home = Home::new("sticky-pin");
    let pass = Secret::from_str("shared-passphrase");
    let mut store =
        Store::initialise_with(home.paths(), Vault::create_unchecked(&pass, kdf()).expect("vault"))
            .expect("initialise");

    // This machine pins the publisher, points at its own URL, and holds a token.
    store.unlock(&pass).expect("unlock");
    let fingerprint = store.vault().signer_fingerprint().expect("fingerprint");
    let mut pinned = source(vec![fingerprint]);
    pinned.url = "https://github.com/me/my-own-config".into();
    pinned.auth = RemoteAuth::Token { token_ref: SecretRef("github.token:local".into()) };
    let mut local_config = store.config().clone();
    local_config.remote = Some(pinned.clone());
    store.set_config(local_config).expect("set config");

    // The publisher — the same vault, correctly signed, so every authenticity
    // check passes — embeds a remote block that unpins everybody and repoints
    // the URL. This is the pull that used to switch pinning off for good.
    let mut hostile = store.config().clone();
    hostile.remote = Some({
        let mut r = source(Vec::new());
        r.url = "https://github.com/attacker/config".into();
        r
    });
    store.vault_file_mut().vault_mut().set_embedded_config(Some(hostile)).expect("embed");
    let bytes = store.vault_file_mut().vault_mut().seal_signed().expect("seal signed");

    let fetched = FetchedVault { bytes, source_url: "https://example/x".into(), sha: None };
    let plan =
        verify_pull(&fetched, store.config(), store.vault(), &pinned, &pass).expect("verify");
    apply_pull(&mut store, &plan).expect("apply");

    let after = store.config().remote.as_ref().expect("the remote block must survive");
    assert_eq!(
        after.trusted_signers, pinned.trusted_signers,
        "an accepted pull must not be able to clear the pinned signer list"
    );
    assert_eq!(after.url, pinned.url, "nor repoint this machine at another repository");
    assert!(
        matches!(&after.auth, RemoteAuth::Token { token_ref } if token_ref.as_str() == "github.token:local"),
        "nor swap out the local credential handle"
    );
}

/// Regression guard for M2.
///
/// `envelope.rs` documents `vault_id` as what lets remote sync "refuse to
/// overwrite" a completely different vault. `verify_pull` computed
/// `different_vault` and `apply_pull` ignored it, so the refusal existed only
/// in a UI outside this crate — which is to say, it did not exist.
#[test]
fn applying_a_pull_from_a_different_vault_is_refused_by_default() {
    let home = Home::new("other-vault");
    let pass = Secret::from_str("shared-passphrase");
    let mut store =
        Store::initialise_with(home.paths(), Vault::create_unchecked(&pass, kdf()).expect("vault"))
            .expect("initialise");
    let before = std::fs::read(home.paths().vault_file()).expect("read");

    // A completely separate installation that happens to use the same
    // passphrase.
    let mut stranger = Vault::create_unchecked(&pass, kdf()).expect("stranger");
    stranger.put(SecretRef("s3.access:1".into()), Secret::from_str("STRANGER-KEY")).expect("put");
    let bytes = stranger.seal().expect("seal");

    let fetched = FetchedVault { bytes, source_url: "https://example/x".into(), sha: None };
    let plan = verify_pull(&fetched, store.config(), store.vault(), &source(Vec::new()), &pass)
        .expect("verification itself succeeds; it is application that must refuse");
    assert!(plan.different_vault);
    assert_eq!(plan.freshness, Freshness::Unrelated);

    let err = apply_pull(&mut store, &plan).expect_err("must refuse a stranger's vault");
    assert!(format!("{err}").contains("different vault"), "{err}");
    assert_eq!(
        std::fs::read(home.paths().vault_file()).expect("read"),
        before,
        "and must not have touched the local vault"
    );
    assert!(
        store.vault_file().list_backups().expect("backups").is_empty(),
        "a refused pull must not even take a backup"
    );

    // Deliberately joining another installation is the one case where this is
    // right, and it has to be said explicitly.
    let joining = PullOptions { allow_different_vault: true, ..PullOptions::default() };
    apply_pull_with(&mut store, &plan, &joining).expect("an explicit join is allowed");
    store.unlock(&pass).expect("unlock");
    assert_eq!(
        store.secret(&SecretRef("s3.access:1".into())).expect("get").expect("present").expose(),
        b"STRANGER-KEY"
    );
}
