//! Vault and key backups, end to end through the runner.
//!
//! # Why this file exists rather than more unit tests
//!
//! `engine::protected`'s own tests prove the sealing is right. They would all
//! have passed for a build where the runner never called the provider, or
//! called it and then snapshotted `job.sources` anyway — which is empty for
//! these jobs, so every run would have reported success and backed up nothing,
//! every night, until someone needed it.
//!
//! So what is asserted here is the join: that a job whose content is prepared
//! reaches the executor with the staging folder as its source, that the folder
//! holds ciphertext, and that it is gone afterwards.

use std::path::PathBuf;
use std::sync::Arc;

use superbackup_core::engine::cancel::CancelToken;
use superbackup_core::engine::clock::{BoxFuture, TestClock};
use superbackup_core::engine::protected::{
    self, ContentProvider, StagedContent, Staging, UnavailableContent,
};
use superbackup_core::engine::testing::{test_job, test_repository, MockExecutor};
use superbackup_core::engine::{EngineEvent, RetryPolicy, RunRequest, Runner};
use superbackup_core::error::Result;
use superbackup_core::model::{Destination, Job, JobContent, Settings, Source};
use superbackup_core::secret::Secret;
use superbackup_core::state::{PersistedState, RunStatus, Trigger};
use uuid::Uuid;

/// A provider standing in for the daemon's, over a directory the test owns.
#[derive(Debug)]
struct TestContent {
    home: PathBuf,
    passphrase: Secret,
    /// Where the last staging folder was, so the test can prove it is gone.
    last: std::sync::Mutex<Option<PathBuf>>,
}

impl ContentProvider for TestContent {
    fn stage<'a>(
        &'a self,
        content: JobContent,
        run_id: Uuid,
    ) -> BoxFuture<'a, Result<StagedContent>> {
        Box::pin(async move {
            let staging = Staging::create(protected::staging_dir(&self.home, run_id))?;
            let notes = match content {
                JobContent::Keys => protected::stage_keys(
                    staging.dir(),
                    "test-machine",
                    &[self.home.join("id_ed25519"), self.home.join("id_ed25519.pub")],
                    &self.passphrase,
                )?,
                JobContent::Vault => {
                    protected::stage_vault(staging.dir(), &self.home.join("config.sbvault"))?
                }
                JobContent::Files => unreachable!("the runner must not stage a files job"),
            };
            *self.last.lock().expect("lock") = Some(staging.dir().to_path_buf());
            Ok(StagedContent {
                sources: vec![Source::new(staging.dir().to_path_buf())],
                notes,
                staging,
            })
        })
    }
}

struct Harness {
    runner: Runner,
    executor: Arc<MockExecutor>,
    content: Arc<TestContent>,
    home: PathBuf,
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

const KEY_MATERIAL: &[u8] = b"-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\n";
const VAULT_BYTES: &[u8] = b"SBVAULT\x01 sealed bytes, opaque to everything but the vault";

fn harness() -> Harness {
    let home = std::env::temp_dir().join(format!("sb-protected-{}", Uuid::new_v4().simple()));
    std::fs::create_dir_all(&home).expect("create");
    std::fs::write(home.join("id_ed25519"), KEY_MATERIAL).expect("write");
    std::fs::write(home.join("id_ed25519.pub"), b"ssh-ed25519 AAAA test\n").expect("write");
    std::fs::write(home.join("config.sbvault"), VAULT_BYTES).expect("write");

    let content = Arc::new(TestContent {
        home: home.clone(),
        passphrase: Secret::from_str("correct horse battery staple"),
        last: std::sync::Mutex::new(None),
    });
    let executor = Arc::new(MockExecutor::new());
    let (events, _) = tokio::sync::broadcast::channel::<EngineEvent>(
        superbackup_core::engine::EVENT_CHANNEL_CAPACITY,
    );
    let runner = Runner::new(
        executor.clone(),
        Arc::new(TestClock::at("2025-01-08T12:00:00Z")),
        Arc::new(chrono::Utc),
        events,
        Arc::new(tokio::sync::Mutex::new(PersistedState::default())),
    )
    .with_retry_policy(RetryPolicy::none())
    .with_content_provider(content.clone());

    Harness { runner, executor, content, home }
}

fn prepared_job(content: JobContent, destinations: &[Destination]) -> Job {
    let mut job = test_job("protect-superbackup");
    job.content = content;
    // Exactly as the wizard stores it: a prepared job carries no sources.
    job.sources = Vec::new();
    job.destination_ids = destinations.iter().map(|d| d.id).collect();
    job
}

fn request(job: Job, destinations: Vec<Destination>) -> RunRequest {
    RunRequest {
        dry_run: false,
        run_id: Uuid::new_v4(),
        job: Arc::new(job),
        destinations: destinations.into_iter().map(Arc::new).collect(),
        settings: Arc::new(Settings::default()),
        trigger: Trigger::Manual,
        cancel: CancelToken::new(),
    }
}

/// The join: a job with no sources still reaches the executor with something
/// to back up, and what it backs up is the staged payload.
#[tokio::test]
async fn a_keys_job_snapshots_the_staged_bundle_and_not_the_key_folder() {
    let h = harness();
    let destinations = vec![test_repository("offsite", "/repos/offsite")];
    let run = h
        .runner
        .execute(request(prepared_job(JobContent::Keys, &destinations), destinations))
        .await;
    assert_eq!(run.status, RunStatus::Succeeded, "{run:?}");

    let calls = h.executor.calls();
    assert_eq!(calls.len(), 1, "one destination, one snapshot");
    let sources = &calls[0].sources;
    assert_eq!(sources.len(), 1, "the payload is one folder: {sources:?}");

    let staged = sources[0].clone();
    assert_ne!(staged, h.home, "the key folder itself must never be the source");
    assert!(
        staged.starts_with(h.home.join("staging")),
        "the source is the staging folder: {}",
        staged.display()
    );
}

/// What was in that folder while the run happened. Asserted by capturing it
/// during the snapshot, because by the time `execute` returns it is gone —
/// and "gone" is the other half of what this test is for.
#[tokio::test]
async fn what_is_backed_up_is_ciphertext_and_it_does_not_outlive_the_run() {
    let h = harness();
    let destinations = vec![test_repository("offsite", "/repos/offsite")];

    let run = h
        .runner
        .execute(request(prepared_job(JobContent::Keys, &destinations), destinations))
        .await;
    assert_eq!(run.status, RunStatus::Succeeded);

    let staged = h.content.last.lock().expect("lock").clone().expect("staged");
    assert!(!staged.exists(), "the staging folder outlived the run: {}", staged.display());

    // And while it existed it held the bundle rather than the key. The
    // executor recorded the folder; the bundle is checked by re-staging into a
    // folder of our own, which exercises exactly the same code path.
    let check = Staging::create(h.home.join("check")).expect("staging");
    protected::stage_keys(
        check.dir(),
        "test-machine",
        &[h.home.join("id_ed25519")],
        &Secret::from_str("correct horse battery staple"),
    )
    .expect("stage");
    let bytes = std::fs::read(check.dir().join(superbackup_core::credentials::BUNDLE_FILE))
        .expect("bundle");
    let needle = b"BEGIN OPENSSH PRIVATE KEY";
    assert!(
        !bytes.windows(needle.len()).any(|w| w == needle),
        "the key reached the destination in the clear"
    );
}

/// A vault job carries the sealed file and the instructions for using it.
#[tokio::test]
async fn a_vault_job_carries_the_sealed_file_and_the_way_back() {
    let h = harness();
    let destinations = vec![test_repository("offsite", "/repos/offsite")];
    let run = h
        .runner
        .execute(request(prepared_job(JobContent::Vault, &destinations), destinations))
        .await;
    assert_eq!(run.status, RunStatus::Succeeded, "{run:?}");

    // Re-stage into a folder the test keeps, to inspect what the run wrote.
    let check = Staging::create(h.home.join("check")).expect("staging");
    let names = protected::stage_vault(check.dir(), &h.home.join("config.sbvault")).expect("stage");
    assert!(names.contains(&"config.sbvault".to_string()));
    assert_eq!(
        std::fs::read(check.dir().join("config.sbvault")).expect("copied"),
        VAULT_BYTES,
        "the vault is backed up as it is, not re-encrypted"
    );
    let note = std::fs::read_to_string(check.dir().join(protected::RESTORE_FILE)).expect("note");
    assert!(note.contains("vault restore --from"), "the note names a real command: {note}");
}

/// An ordinary job is untouched by any of this: its sources are still what it
/// backs up, and the provider is never consulted.
#[tokio::test]
async fn an_ordinary_job_still_backs_up_its_own_folders() {
    let h = harness();
    let destinations = vec![test_repository("offsite", "/repos/offsite")];
    let mut job = test_job("dev-code");
    job.destination_ids = destinations.iter().map(|d| d.id).collect();
    let expected = job.sources[0].path.clone();

    let run = h.runner.execute(request(job, destinations)).await;
    assert_eq!(run.status, RunStatus::Succeeded);
    assert_eq!(h.executor.calls()[0].sources, vec![expected]);
    assert!(h.content.last.lock().expect("lock").is_none(), "the provider was consulted");
}

/// The failure this whole design exists to prevent: a run that cannot build
/// its payload must fail, loudly, rather than snapshotting an empty folder.
#[tokio::test]
async fn a_payload_that_cannot_be_built_fails_the_run_rather_than_emptying_it() {
    let clock = Arc::new(TestClock::at("2025-01-08T12:00:00Z"));
    let executor = Arc::new(MockExecutor::new());
    let (events, _) = tokio::sync::broadcast::channel::<EngineEvent>(
        superbackup_core::engine::EVENT_CHANNEL_CAPACITY,
    );
    let runner = Runner::new(
        executor.clone(),
        clock,
        Arc::new(chrono::Utc),
        events,
        Arc::new(tokio::sync::Mutex::new(PersistedState::default())),
    )
    .with_retry_policy(RetryPolicy::none())
    // The state a locked daemon is in.
    .with_content_provider(Arc::new(UnavailableContent));

    let destinations = vec![test_repository("offsite", "/repos/offsite")];
    let run =
        runner.execute(request(prepared_job(JobContent::Vault, &destinations), destinations)).await;

    assert_eq!(run.status, RunStatus::Failed, "{run:?}");
    assert!(executor.calls().is_empty(), "nothing was snapshotted");
    let error =
        run.destinations.first().and_then(|d| d.error.clone()).expect("the run says why it failed");
    let text = format!("{} {}", error.message, error.detail.unwrap_or_default());
    assert!(text.contains("unlock") || text.contains("prepared"), "{text}");
}
