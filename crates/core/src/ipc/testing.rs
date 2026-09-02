//! A [`Handler`] that answers everything, for testing the transport.
//!
//! Not behind `#[cfg(test)]` on purpose: integration tests live in a separate
//! crate and cannot see this crate's unit-test items, and the daemon's own
//! tests want the same fixture. It is documented as test scaffolding and does
//! nothing a production caller would want.
//!
//! What it gives a test:
//!
//! * every command answers with a plausible, well-formed reply, so a test can
//!   exercise the transport without a working engine behind it;
//! * a counter of every command received, so a test can assert dispatch
//!   actually reached the handler it expected, and the *peak* number that ran
//!   at once, which is how the key-derivation gate is tested;
//! * a count of stalled handlers that were dropped before finishing, which is
//!   how "a timed-out handler is aborted rather than detached" is tested;
//! * a [`MockHandler::publish`] hook for driving the event stream;
//! * switches for the failure modes the transport has to survive —
//!   [`MockHandler::fail_with`], [`MockHandler::stall`] and
//!   [`MockHandler::panic_on`].

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Utc;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::error::{Error, ErrorCode, Result};
use crate::model::{
    BandwidthSettings, Destination, EncryptionSettings, Job, PauseState, SecretRef, Settings,
    StorageProvider,
};
use crate::state::{Health, StatusSnapshot};

use super::protocol::*;

/// A handler that says yes to everything, plausibly.
#[derive(Debug)]
pub struct MockHandler {
    events: broadcast::Sender<StreamItem>,
    state: Mutex<MockState>,
    /// When set, every command fails with this code instead of succeeding.
    failure: Mutex<Option<ErrorCode>>,
    /// When set, every command sleeps this long first. Used to test the
    /// handler timeout and to make a client's timeout observable.
    stall: Mutex<Option<Duration>>,
    /// When set, the named command panics. Used to prove that a panicking
    /// handler costs one request and not the daemon.
    panic_on: Mutex<Option<String>>,
    /// Refuse to open new subscriptions.
    refuse_subscriptions: AtomicBool,
    /// Stalled handlers that were dropped before their sleep finished — i.e.
    /// tasks the transport actually aborted rather than merely stopped waiting
    /// for.
    aborted: Arc<AtomicU64>,
}

#[derive(Debug, Default)]
struct MockState {
    /// How many times each command was dispatched, keyed by wire name.
    calls: BTreeMap<String, u32>,
    /// How many calls to each command are running right now.
    inflight: BTreeMap<String, u32>,
    /// The largest value `inflight` ever reached for each command.
    peak: BTreeMap<String, u32>,
    jobs: Vec<Job>,
    destinations: Vec<Destination>,
    providers: Vec<StorageProvider>,
    unlocked: bool,
    paused: PauseState,
    settings: Settings,
    /// Secrets that were set. Values are *not* stored, only their handles —
    /// mirroring the real daemon, which has no way to give one back.
    secret_refs: Vec<SecretRef>,
    /// The machine's display label, so a rename can be observed.
    machine_label: Option<String>,
}

impl Default for MockHandler {
    fn default() -> Self {
        MockHandler::new()
    }
}

impl MockHandler {
    /// A handler with an empty configuration and a locked vault.
    pub fn new() -> MockHandler {
        MockHandler::with_capacity(64)
    }

    /// A handler whose event channel holds `capacity` items.
    ///
    /// A small capacity is how a test makes a slow subscriber lag on purpose.
    pub fn with_capacity(capacity: usize) -> MockHandler {
        let (events, _) = broadcast::channel(capacity.max(1));
        MockHandler {
            events,
            state: Mutex::new(MockState::default()),
            failure: Mutex::new(None),
            stall: Mutex::new(None),
            panic_on: Mutex::new(None),
            refuse_subscriptions: AtomicBool::new(false),
            aborted: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Publish an item to every subscriber.
    ///
    /// Returns the number of subscribers it reached; zero is normal and not
    /// an error, exactly as in the daemon.
    pub fn publish(&self, item: StreamItem) -> usize {
        self.events.send(item).unwrap_or(0)
    }

    /// How many times a command was dispatched.
    pub fn calls(&self, command: &str) -> u32 {
        self.state.lock().map(|s| s.calls.get(command).copied().unwrap_or(0)).unwrap_or(0)
    }

    /// Every command that has been dispatched, with counts.
    pub fn all_calls(&self) -> BTreeMap<String, u32> {
        self.state.lock().map(|s| s.calls.clone()).unwrap_or_default()
    }

    /// The largest number of calls to `command` that were running at the same
    /// time. This is what proves a concurrency gate actually gates.
    pub fn peak_concurrency(&self, command: &str) -> u32 {
        self.state.lock().map(|s| s.peak.get(command).copied().unwrap_or(0)).unwrap_or(0)
    }

    /// How many stalled handlers were dropped before their sleep finished.
    ///
    /// A handler that the transport merely stopped *waiting* for keeps running
    /// and never increments this; one that was aborted does.
    pub fn aborted_handlers(&self) -> u64 {
        self.aborted.load(Ordering::Relaxed)
    }

    /// Make every subsequent command fail with `code`, or clear the failure.
    pub fn fail_with(&self, code: Option<ErrorCode>) {
        if let Ok(mut f) = self.failure.lock() {
            *f = code;
        }
    }

    /// Make every subsequent command sleep before answering.
    pub fn stall(&self, delay: Option<Duration>) {
        if let Ok(mut s) = self.stall.lock() {
            *s = delay;
        }
    }

    /// Make one command panic, to prove the transport survives it.
    pub fn panic_on(&self, command: Option<&str>) {
        if let Ok(mut p) = self.panic_on.lock() {
            *p = command.map(|c| c.to_string());
        }
    }

    /// Make `subscribe` fail.
    pub fn refuse_subscriptions(&self, refuse: bool) {
        self.refuse_subscriptions.store(refuse, Ordering::Relaxed);
    }

    /// Seed the configuration a test wants the handler to report.
    pub fn seed_jobs(&self, jobs: Vec<Job>) {
        if let Ok(mut s) = self.state.lock() {
            s.jobs = jobs;
        }
    }

    /// Number of live subscribers, for asserting that cancellation worked.
    pub fn subscriber_count(&self) -> usize {
        self.events.receiver_count()
    }

    /// Record a call and apply whichever failure mode is armed.
    ///
    /// Returns a guard that must be held for the body of the handler: it is
    /// what makes [`MockHandler::peak_concurrency`] measure the handler's
    /// lifetime rather than a single instant. Every handler below therefore
    /// binds it rather than discarding it.
    async fn enter<'a>(&'a self, command: &str) -> Result<CallGuard<'a>> {
        if let Ok(mut s) = self.state.lock() {
            *s.calls.entry(command.to_string()).or_insert(0) += 1;
            let live = s.inflight.entry(command.to_string()).or_insert(0);
            *live += 1;
            let live = *live;
            let peak = s.peak.entry(command.to_string()).or_insert(0);
            *peak = (*peak).max(live);
        }
        // Created before anything that can panic or return, so the in-flight
        // count is decremented on every path out.
        let guard = CallGuard { state: &self.state, command: command.to_string() };

        let panic_on = self.panic_on.lock().ok().and_then(|p| p.clone());
        if panic_on.as_deref() == Some(command) {
            panic!("MockHandler was asked to panic on `{command}`");
        }
        let stall = self.stall.lock().ok().and_then(|s| *s);
        if let Some(delay) = stall {
            // If this future is dropped mid-sleep, the transport aborted us.
            let mut witness = AbortWitness { finished: false, counter: Arc::clone(&self.aborted) };
            tokio::time::sleep(delay).await;
            witness.finished = true;
        }
        let failure = self.failure.lock().ok().and_then(|f| *f);
        match failure {
            None => Ok(guard),
            Some(ErrorCode::Locked) => Err(Error::Locked),
            Some(ErrorCode::BadPassphrase) => Err(Error::BadPassphrase),
            Some(ErrorCode::JobNotFound) => Err(Error::JobNotFound(command.into())),
            Some(_) => Err(Error::Internal(format!("MockHandler was told to fail `{command}`"))),
        }
    }

    fn snapshot(&self) -> StatusSnapshot {
        let (unlocked, paused, label) = self
            .state
            .lock()
            .map(|s| (s.unlocked, s.paused.paused, s.machine_label.clone()))
            .unwrap_or((false, false, None));
        StatusSnapshot {
            health: if paused { Health::Paused } else { Health::Idle },
            version: crate::VERSION.to_string(),
            // A rename shows up here, so a test can assert the label moved.
            machine_label: label.unwrap_or_else(|| "mock".into()),
            machine_hostname: "mock".into(),
            // Deliberately unchanged by a rename: the slug is the on-disk
            // folder name and is fixed for the life of the install.
            machine_slug: "mock".into(),
            unlocked,
            paused,
            paused_until: None,
            service_installed: false,
            service_running: false,
            kopia_version: Some("0.0.0-mock".into()),
            active_runs: vec![],
            jobs: Default::default(),
            next_scheduled: None,
            recent_events: vec![],
            uptime_seconds: 0,
            generated_at: Utc::now(),
        }
    }

    fn unlocked_reply(&self) -> UnlockedReply {
        let unlocked = self.state.lock().map(|s| s.unlocked).unwrap_or(false);
        UnlockedReply { unlocked, auto_lock_at: None }
    }

    fn pause_reply(&self) -> PauseReply {
        let pause = self.state.lock().map(|s| s.paused.clone()).unwrap_or_default();
        PauseReply { pause }
    }

    fn find_job(&self, needle: &str) -> Result<Job> {
        self.state
            .lock()
            .ok()
            .and_then(|s| {
                s.jobs
                    .iter()
                    .find(|j| j.id.to_string() == needle || j.name.starts_with(needle))
                    .cloned()
            })
            .ok_or_else(|| Error::JobNotFound(needle.to_string()))
    }

    fn probe() -> ProbeReply {
        ProbeReply {
            reachable: true,
            writable: true,
            latency_ms: Some(1),
            repository_present: Some(true),
            detail: None,
        }
    }

    fn service() -> ServiceReply {
        ServiceReply {
            installed: false,
            running: false,
            autostart: false,
            scope: "user".into(),
            detail: None,
            in_applications_menu: false,
            applications_menu_path: None,
        }
    }

    fn remote_status() -> RemoteStatusReply {
        RemoteStatusReply {
            url: None,
            branch: None,
            last_pull_at: None,
            last_known_commit: None,
            local_changes: false,
            remote_changes: false,
            detail: None,
        }
    }

    fn started() -> StartedReply {
        StartedReply { run_id: Uuid::new_v4(), started: true, note: None }
    }
}

/// Every method records the call, honours the armed failure mode, and returns
/// a well-formed reply. Nothing here models real behaviour; the point is to
/// exercise the transport.
impl Handler for MockHandler {
    async fn ping(&self, _ctx: &RequestContext) -> Result<AckReply> {
        let _guard = self.enter("ping").await?;
        Ok(AckReply {})
    }

    async fn status(&self, _ctx: &RequestContext) -> Result<StatusReply> {
        let _guard = self.enter("status").await?;
        Ok(StatusReply { snapshot: Box::new(self.snapshot()) })
    }

    async fn version(&self, _ctx: &RequestContext) -> Result<VersionReply> {
        let _guard = self.enter("version").await?;
        Ok(VersionReply {
            version: crate::VERSION.to_string(),
            protocol: super::PROTOCOL_VERSION,
            min_protocol: super::MIN_PROTOCOL_VERSION,
            target_os: std::env::consts::OS.to_string(),
            target_arch: std::env::consts::ARCH.to_string(),
            kopia_version: Some("0.0.0-mock".into()),
            service_scope: false,
            build: "0.0.0-mock+test".to_string(),
        })
    }

    async fn health(&self, _ctx: &RequestContext) -> Result<HealthReply> {
        let _guard = self.enter("health").await?;
        Ok(HealthReply { health: Health::Idle, summary: "Up to date".into(), reasons: vec![] })
    }

    async fn doctor(&self, _ctx: &RequestContext, fix: bool) -> Result<DoctorReply> {
        let _guard = self.enter("doctor").await?;
        Ok(DoctorReply {
            ok: true,
            checks: vec![DoctorCheck {
                id: "kopia.present".into(),
                title: "kopia is installed".into(),
                status: CheckStatus::Pass,
                detail: None,
                hint: None,
                fixable: false,
            }],
            fixed: if fix { vec!["kopia.present".to_string()] } else { vec![] },
        })
    }

    async fn kopia_probe(
        &self,
        _ctx: &RequestContext,
        destination: Option<String>,
        _check_for_update: bool,
    ) -> Result<KopiaProbeReply> {
        let _guard = self.enter("kopia.probe").await?;
        let mut invocations = vec![KopiaInvocation {
            label: "--version".into(),
            command_line: "/usr/bin/kopia --version".into(),
            secret_env: Vec::new(),
            exit_code: Some(0),
            stdout: "0.21.1 build: mock from: kopia/kopia".into(),
            stderr: String::new(),
            duration_ms: 12,
            ok: true,
        }];
        if destination.is_some() {
            invocations.push(KopiaInvocation {
                label: "repository status".into(),
                command_line: "/usr/bin/kopia --config-file=... repository status --json".into(),
                secret_env: vec!["KOPIA_PASSWORD".into()],
                exit_code: Some(0),
                stdout: "{\"uniqueId\":\"mock-repo\"}".into(),
                stderr: String::new(),
                duration_ms: 84,
                ok: true,
            });
        }
        Ok(KopiaProbeReply {
            path: Some("/usr/bin/kopia".into()),
            provenance: KopiaProvenance::SystemPath,
            version: Some("0.21.1".into()),
            banner: Some("0.21.1 build: mock from: kopia/kopia".into()),
            routes: Vec::new(),
            managed_path: "/var/lib/superbackup/kopia".into(),
            managed_version: None,
            update_policy: "notify".into(),
            update_available: None,
            update_summary: Some("kopia 0.21.1 is up to date.".into()),
            minimum_version: "0.17.0".into(),
            invocations,
            detail: None,
        })
    }

    async fn list_jobs(&self, _ctx: &RequestContext, _include_disabled: bool) -> Result<JobsReply> {
        let _guard = self.enter("job.list").await?;
        let jobs = self.state.lock().map(|s| s.jobs.clone()).unwrap_or_default();
        Ok(JobsReply { jobs })
    }

    async fn get_job(&self, _ctx: &RequestContext, job: String) -> Result<JobReply> {
        let _guard = self.enter("job.get").await?;
        Ok(JobReply { job: Box::new(self.find_job(&job)?) })
    }

    async fn create_job(&self, _ctx: &RequestContext, job: Box<Job>) -> Result<JobReply> {
        let _guard = self.enter("job.create").await?;
        let mut job = *job;
        job.id = Uuid::new_v4();
        if let Ok(mut s) = self.state.lock() {
            s.jobs.push(job.clone());
        }
        Ok(JobReply { job: Box::new(job) })
    }

    async fn update_job(&self, _ctx: &RequestContext, job: Box<Job>) -> Result<JobReply> {
        let _guard = self.enter("job.update").await?;
        Ok(JobReply { job })
    }

    async fn delete_job(&self, _ctx: &RequestContext, job: String) -> Result<AckReply> {
        let _guard = self.enter("job.delete").await?;
        if let Ok(mut s) = self.state.lock() {
            s.jobs.retain(|j| j.id.to_string() != job && j.name != job);
        }
        Ok(AckReply {})
    }

    async fn set_job_enabled(
        &self,
        _ctx: &RequestContext,
        job: String,
        enabled: bool,
    ) -> Result<JobReply> {
        let _guard = self.enter("job.set_enabled").await?;
        let mut found = self.find_job(&job)?;
        found.enabled = enabled;
        Ok(JobReply { job: Box::new(found) })
    }

    async fn run_job(
        &self,
        _ctx: &RequestContext,
        _job: String,
        dry_run: bool,
    ) -> Result<StartedReply> {
        let _guard = self.enter("job.run").await?;
        Ok(StartedReply {
            run_id: Uuid::new_v4(),
            started: true,
            note: dry_run.then(|| "dry run: nothing will be written".to_string()),
        })
    }

    async fn stop_run(&self, _ctx: &RequestContext, run_id: Uuid) -> Result<StoppedReply> {
        let _guard = self.enter("job.stop").await?;
        Ok(StoppedReply { stopped: vec![run_id] })
    }

    async fn stop_all_runs(&self, _ctx: &RequestContext) -> Result<StoppedReply> {
        let _guard = self.enter("job.stop_all").await?;
        Ok(StoppedReply { stopped: vec![] })
    }

    async fn job_history(
        &self,
        _ctx: &RequestContext,
        _job: Option<String>,
        _limit: u32,
    ) -> Result<RunsReply> {
        let _guard = self.enter("job.history").await?;
        Ok(RunsReply { runs: vec![] })
    }

    async fn list_destinations(&self, _ctx: &RequestContext) -> Result<DestinationsReply> {
        let _guard = self.enter("dest.list").await?;
        let destinations = self.state.lock().map(|s| s.destinations.clone()).unwrap_or_default();
        Ok(DestinationsReply { destinations })
    }

    async fn get_destination(
        &self,
        _ctx: &RequestContext,
        destination: String,
    ) -> Result<DestinationReply> {
        let _guard = self.enter("dest.get").await?;
        self.state
            .lock()
            .ok()
            .and_then(|s| {
                s.destinations
                    .iter()
                    .find(|d| d.id.to_string() == destination || d.name.starts_with(&destination))
                    .cloned()
            })
            .map(|d| DestinationReply { destination: Box::new(d) })
            .ok_or_else(|| Error::Validation(format!("no destination matching `{destination}`")))
    }

    async fn create_destination(
        &self,
        _ctx: &RequestContext,
        destination: Box<Destination>,
    ) -> Result<DestinationReply> {
        let _guard = self.enter("dest.create").await?;
        let mut destination = *destination;
        destination.id = Uuid::new_v4();
        if let Ok(mut s) = self.state.lock() {
            s.destinations.push(destination.clone());
        }
        Ok(DestinationReply { destination: Box::new(destination) })
    }

    async fn update_destination(
        &self,
        _ctx: &RequestContext,
        destination: Box<Destination>,
    ) -> Result<DestinationReply> {
        let _guard = self.enter("dest.update").await?;
        Ok(DestinationReply { destination })
    }

    async fn delete_destination(
        &self,
        _ctx: &RequestContext,
        _destination: String,
        _force: bool,
    ) -> Result<AckReply> {
        let _guard = self.enter("dest.delete").await?;
        Ok(AckReply {})
    }

    async fn test_destination(
        &self,
        _ctx: &RequestContext,
        _destination: String,
    ) -> Result<ProbeReply> {
        let _guard = self.enter("dest.test").await?;
        Ok(Self::probe())
    }

    async fn check_encryption_key(
        &self,
        _ctx: &RequestContext,
        _destination: String,
        key: Option<SecretString>,
    ) -> Result<KeyCheckReply> {
        let _guard = self.enter("dest.check_key").await?;
        // A key the fixture recognises opens the mock repository; anything
        // else does not, so a caller can exercise both branches.
        let valid = match &key {
            None => true,
            Some(k) => k.expose().expose_str() == Some("correct-key"),
        };
        Ok(KeyCheckReply {
            destination_id: Uuid::new_v4(),
            valid,
            no_repository: false,
            repository_id: valid.then(|| "mock-repo".to_string()),
            detail: (!valid).then(|| "invalid repository password".to_string()),
        })
    }

    async fn create_repository(
        &self,
        _ctx: &RequestContext,
        _destination: String,
        _encryption: Option<EncryptionSettings>,
    ) -> Result<RepositoryReply> {
        let _guard = self.enter("dest.repo_create").await?;
        Ok(RepositoryReply {
            destination_id: Uuid::new_v4(),
            connected: true,
            repository_id: Some("mock-repo".into()),
            created: true,
        })
    }

    async fn connect_repository(
        &self,
        _ctx: &RequestContext,
        _destination: String,
    ) -> Result<RepositoryReply> {
        let _guard = self.enter("dest.repo_connect").await?;
        Ok(RepositoryReply {
            destination_id: Uuid::new_v4(),
            connected: true,
            repository_id: Some("mock-repo".into()),
            created: false,
        })
    }

    async fn disconnect_repository(
        &self,
        _ctx: &RequestContext,
        _destination: String,
    ) -> Result<RepositoryReply> {
        let _guard = self.enter("dest.repo_disconnect").await?;
        Ok(RepositoryReply {
            destination_id: Uuid::new_v4(),
            connected: false,
            repository_id: None,
            created: false,
        })
    }

    async fn destination_stats(
        &self,
        _ctx: &RequestContext,
        _destination: String,
        _refresh: bool,
    ) -> Result<StorageStatsReply> {
        let _guard = self.enter("dest.stats").await?;
        Ok(StorageStatsReply {
            destination_id: Uuid::new_v4(),
            snapshot_count: 0,
            logical_bytes: None,
            stored_bytes: None,
            last_snapshot_at: None,
            computed_at: Utc::now(),
        })
    }

    async fn list_providers(&self, _ctx: &RequestContext) -> Result<ProvidersReply> {
        let _guard = self.enter("provider.list").await?;
        let providers = self.state.lock().map(|s| s.providers.clone()).unwrap_or_default();
        Ok(ProvidersReply { providers })
    }

    async fn get_provider(&self, _ctx: &RequestContext, provider: String) -> Result<ProviderReply> {
        let _guard = self.enter("provider.get").await?;
        self.state
            .lock()
            .ok()
            .and_then(|s| {
                s.providers
                    .iter()
                    .find(|p| p.id.to_string() == provider || p.name.starts_with(&provider))
                    .cloned()
            })
            .map(|p| ProviderReply { provider: Box::new(p) })
            .ok_or_else(|| Error::Validation(format!("no provider matching `{provider}`")))
    }

    async fn create_provider(
        &self,
        _ctx: &RequestContext,
        provider: Box<StorageProvider>,
    ) -> Result<ProviderReply> {
        let _guard = self.enter("provider.create").await?;
        let mut provider = *provider;
        provider.id = Uuid::new_v4();
        if let Ok(mut s) = self.state.lock() {
            s.providers.push(provider.clone());
        }
        Ok(ProviderReply { provider: Box::new(provider) })
    }

    async fn update_provider(
        &self,
        _ctx: &RequestContext,
        provider: Box<StorageProvider>,
    ) -> Result<ProviderReply> {
        let _guard = self.enter("provider.update").await?;
        Ok(ProviderReply { provider })
    }

    async fn delete_provider(
        &self,
        _ctx: &RequestContext,
        _provider: String,
        _force: bool,
    ) -> Result<AckReply> {
        let _guard = self.enter("provider.delete").await?;
        Ok(AckReply {})
    }

    async fn test_provider(&self, _ctx: &RequestContext, _provider: String) -> Result<ProbeReply> {
        let _guard = self.enter("provider.test").await?;
        Ok(Self::probe())
    }

    async fn list_buckets(&self, _ctx: &RequestContext, _provider: String) -> Result<BucketsReply> {
        let _guard = self.enter("provider.list_buckets").await?;
        Ok(BucketsReply {
            provider_id: Uuid::nil(),
            buckets: ["dev-backups", "photos", "archive-2024"]
                .into_iter()
                .map(|name| BucketInfo { name: name.into(), created_at: Some(Utc::now()) })
                .collect(),
            listed: true,
            credentials_ok: true,
            detail: None,
            latency_ms: Some(84),
        })
    }

    async fn list_objects(
        &self,
        _ctx: &RequestContext,
        _provider: String,
        bucket: String,
        prefix: String,
        _max_keys: u32,
    ) -> Result<ObjectsReply> {
        let _guard = self.enter("provider.list_objects").await?;
        Ok(ObjectsReply {
            keys: vec![ObjectInfo {
                key: format!("{prefix}kopia.repository"),
                size: 661,
                last_modified: Some(Utc::now()),
            }],
            bucket,
            prefix,
            truncated: false,
            holds_repository: true,
            listed: true,
            detail: None,
        })
    }

    async fn provider_used_by(
        &self,
        _ctx: &RequestContext,
        _provider: String,
    ) -> Result<UsedByReply> {
        let _guard = self.enter("provider.used_by").await?;
        Ok(UsedByReply { destinations: vec![], jobs: vec![] })
    }

    async fn rotate_provider_credentials(
        &self,
        _ctx: &RequestContext,
        provider: String,
        access_key_id: SecretString,
        secret_access_key: SecretString,
        session_token: Option<SecretString>,
    ) -> Result<ProviderReply> {
        let _guard = self.enter("provider.rotate_credentials").await?;
        // Prove the secrets arrived without ever copying them anywhere: only
        // their lengths are observed, and the values are dropped (and zeroed)
        // at the end of this scope.
        debug_assert!(!access_key_id.expose().is_empty());
        debug_assert!(!secret_access_key.expose().is_empty());
        let _ = session_token;
        self.get_provider(_ctx, provider).await
    }

    async fn list_snapshots(
        &self,
        _ctx: &RequestContext,
        _destination: String,
        _job: Option<String>,
        _limit: u32,
    ) -> Result<SnapshotsReply> {
        let _guard = self.enter("snapshot.list").await?;
        Ok(SnapshotsReply { snapshots: vec![] })
    }

    async fn browse_snapshot(
        &self,
        _ctx: &RequestContext,
        _destination: String,
        _snapshot: String,
        path: String,
    ) -> Result<ListingReply> {
        let _guard = self.enter("snapshot.browse").await?;
        Ok(ListingReply { path, entries: vec![], truncated: false })
    }

    async fn restore_snapshot(
        &self,
        _ctx: &RequestContext,
        _destination: String,
        _snapshot: String,
        _path: String,
        _target: std::path::PathBuf,
        _conflict: ConflictPolicy,
        _dry_run: bool,
    ) -> Result<StartedReply> {
        let _guard = self.enter("snapshot.restore").await?;
        Ok(Self::started())
    }

    async fn delete_snapshot(
        &self,
        _ctx: &RequestContext,
        _destination: String,
        _snapshot: String,
    ) -> Result<AckReply> {
        let _guard = self.enter("snapshot.delete").await?;
        Ok(AckReply {})
    }

    async fn unlock_vault(
        &self,
        _ctx: &RequestContext,
        passphrase: SecretString,
    ) -> Result<UnlockedReply> {
        let _guard = self.enter("vault.unlock").await?;
        // The mock vault opens for any non-empty passphrase. Note what it does
        // not do: store it, log it, or return it.
        let ok = !passphrase.expose().is_empty();
        if !ok {
            return Err(Error::BadPassphrase);
        }
        if let Ok(mut s) = self.state.lock() {
            s.unlocked = true;
        }
        Ok(self.unlocked_reply())
    }

    async fn lock_vault(&self, _ctx: &RequestContext) -> Result<UnlockedReply> {
        let _guard = self.enter("vault.lock").await?;
        if let Ok(mut s) = self.state.lock() {
            s.unlocked = false;
        }
        Ok(self.unlocked_reply())
    }

    async fn vault_is_unlocked(&self, _ctx: &RequestContext) -> Result<UnlockedReply> {
        let _guard = self.enter("vault.is_unlocked").await?;
        Ok(self.unlocked_reply())
    }

    async fn change_passphrase(
        &self,
        _ctx: &RequestContext,
        current: SecretString,
        replacement: SecretString,
    ) -> Result<AckReply> {
        let _guard = self.enter("vault.change_passphrase").await?;
        if current.expose().is_empty() || replacement.expose().is_empty() {
            return Err(Error::BadPassphrase);
        }
        Ok(AckReply {})
    }

    async fn set_secret(
        &self,
        _ctx: &RequestContext,
        secret_ref: SecretRef,
        value: SecretString,
    ) -> Result<AckReply> {
        let _guard = self.enter("vault.set_secret").await?;
        if value.expose().is_empty() {
            return Err(Error::Validation("refusing to store an empty secret".into()));
        }
        if let Ok(mut s) = self.state.lock() {
            if !s.secret_refs.contains(&secret_ref) {
                s.secret_refs.push(secret_ref);
            }
        }
        Ok(AckReply {})
    }

    async fn export_encryption_keys(
        &self,
        _ctx: &RequestContext,
        _passphrase: SecretString,
    ) -> Result<KeyExportReply> {
        let _guard = self.enter("vault.export_keys").await?;
        Ok(KeyExportReply {
            document: "SUPERBACKUP - REPOSITORY ENCRYPTION KEYS\r\n(mock)\r\n".to_string(),
            json: "{}".to_string(),
            destinations: 0,
            omitted: Vec::new(),
            suggested_file_name: "superbackup-encryption-keys-mock.txt".to_string(),
            generated_at: Utc::now(),
        })
    }

    async fn list_secret_refs(&self, _ctx: &RequestContext) -> Result<SecretRefsReply> {
        let _guard = self.enter("vault.list_refs").await?;
        let refs = self.state.lock().map(|s| s.secret_refs.clone()).unwrap_or_default();
        Ok(SecretRefsReply { refs })
    }

    async fn pause(
        &self,
        _ctx: &RequestContext,
        seconds: Option<u64>,
        reason: Option<String>,
    ) -> Result<PauseReply> {
        let _guard = self.enter("control.pause").await?;
        if let Ok(mut s) = self.state.lock() {
            s.paused = PauseState {
                paused: true,
                until: seconds.map(|secs| Utc::now() + chrono::Duration::seconds(secs as i64)),
                reason,
            };
        }
        Ok(self.pause_reply())
    }

    async fn resume(&self, _ctx: &RequestContext) -> Result<PauseReply> {
        let _guard = self.enter("control.resume").await?;
        if let Ok(mut s) = self.state.lock() {
            s.paused = PauseState::default();
        }
        Ok(self.pause_reply())
    }

    async fn pause_state(&self, _ctx: &RequestContext) -> Result<PauseReply> {
        let _guard = self.enter("control.pause_state").await?;
        Ok(self.pause_reply())
    }

    async fn set_bandwidth(
        &self,
        _ctx: &RequestContext,
        bandwidth: BandwidthSettings,
    ) -> Result<BandwidthReply> {
        let _guard = self.enter("control.set_bandwidth").await?;
        Ok(BandwidthReply { bandwidth })
    }

    async fn reload_config(&self, _ctx: &RequestContext) -> Result<AckReply> {
        let _guard = self.enter("control.reload_config").await?;
        Ok(AckReply {})
    }

    async fn shutdown(&self, _ctx: &RequestContext, _stop_runs: bool) -> Result<AckReply> {
        let _guard = self.enter("control.shutdown").await?;
        Ok(AckReply {})
    }

    async fn get_settings(&self, _ctx: &RequestContext) -> Result<SettingsReply> {
        let _guard = self.enter("settings.get").await?;
        let settings = self.state.lock().map(|s| s.settings.clone()).unwrap_or_default();
        Ok(SettingsReply { settings: Box::new(settings) })
    }

    async fn export_config(&self, _ctx: &RequestContext) -> Result<ConfigDocumentReply> {
        let _guard = self.enter("config.export").await?;
        Ok(ConfigDocumentReply {
            document: "bW9jay1zZWFsZWQtdmF1bHQ=".into(),
            suggested_filename: "superbackup-config-mock.sbvault".into(),
            size_bytes: 17,
        })
    }

    async fn import_config(
        &self,
        _ctx: &RequestContext,
        _document: String,
        _allow_rollback: bool,
    ) -> Result<RemoteDiffReply> {
        let _guard = self.enter("config.import").await?;
        Ok(RemoteDiffReply { changes: vec![], remote_commit: None })
    }

    async fn rename_machine(&self, _ctx: &RequestContext, label: String) -> Result<AckReply> {
        let _guard = self.enter("machine.rename").await?;
        if let Ok(mut s) = self.state.lock() {
            let label = label.trim();
            if !label.is_empty() {
                // The label changes; the slug deliberately does not, which is
                // the behaviour the real handler has to preserve.
                s.machine_label = Some(label.to_string());
            }
        }
        Ok(AckReply {})
    }

    async fn update_settings(
        &self,
        _ctx: &RequestContext,
        settings: Box<Settings>,
    ) -> Result<SettingsReply> {
        let _guard = self.enter("settings.update").await?;
        if let Ok(mut s) = self.state.lock() {
            s.settings = (*settings).clone();
        }
        Ok(SettingsReply { settings })
    }

    async fn remote_pull(&self, _ctx: &RequestContext) -> Result<RemoteStatusReply> {
        let _guard = self.enter("remote.pull").await?;
        Ok(Self::remote_status())
    }

    async fn remote_diff(&self, _ctx: &RequestContext) -> Result<RemoteDiffReply> {
        let _guard = self.enter("remote.diff").await?;
        Ok(RemoteDiffReply { changes: vec![], remote_commit: None })
    }

    async fn remote_apply(&self, _ctx: &RequestContext) -> Result<RemoteStatusReply> {
        let _guard = self.enter("remote.apply").await?;
        Ok(Self::remote_status())
    }

    async fn remote_push(
        &self,
        _ctx: &RequestContext,
        _message: Option<String>,
    ) -> Result<RemoteStatusReply> {
        let _guard = self.enter("remote.push").await?;
        Ok(Self::remote_status())
    }

    async fn service_status(&self, _ctx: &RequestContext) -> Result<ServiceReply> {
        let _guard = self.enter("service.status").await?;
        Ok(Self::service())
    }

    async fn install_service(&self, _ctx: &RequestContext) -> Result<ServiceReply> {
        let _guard = self.enter("service.install").await?;
        Ok(Self::service())
    }

    async fn uninstall_service(&self, _ctx: &RequestContext) -> Result<ServiceReply> {
        let _guard = self.enter("service.uninstall").await?;
        Ok(Self::service())
    }

    async fn set_autostart(&self, _ctx: &RequestContext, enabled: bool) -> Result<ServiceReply> {
        let _guard = self.enter("service.set_autostart").await?;
        Ok(ServiceReply { autostart: enabled, ..Self::service() })
    }

    async fn preview_file(
        &self,
        _ctx: &RequestContext,
        _destination: String,
        _snapshot: String,
        path: String,
    ) -> Result<PreviewReply> {
        let _guard = self.enter("snapshot.preview").await?;
        Ok(PreviewReply {
            path: format!("/tmp/superbackup-preview/{path}"),
            size_bytes: 11,
            executable: false,
        })
    }

    async fn set_shortcut(&self, _ctx: &RequestContext, enabled: bool) -> Result<ServiceReply> {
        let _guard = self.enter("app.set_shortcut").await?;
        Ok(ServiceReply { in_applications_menu: enabled, ..Self::service() })
    }

    fn event_stream(
        &self,
        _ctx: &RequestContext,
        _topics: &[Topic],
    ) -> Result<broadcast::Receiver<StreamItem>> {
        if self.refuse_subscriptions.load(Ordering::Relaxed) {
            return Err(Error::Ipc("subscriptions are refused by this handler".into()));
        }
        if let Ok(mut s) = self.state.lock() {
            *s.calls.entry("subscribe".to_string()).or_insert(0) += 1;
        }
        Ok(self.events.subscribe())
    }
}

/// Decrements the in-flight count for one command however the handler leaves.
#[derive(Debug)]
struct CallGuard<'a> {
    state: &'a Mutex<MockState>,
    command: String,
}

impl Drop for CallGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut s) = self.state.lock() {
            if let Some(live) = s.inflight.get_mut(&self.command) {
                *live = live.saturating_sub(1);
            }
        }
    }
}

/// Counts a stalled handler that was dropped before it finished sleeping.
#[derive(Debug)]
struct AbortWitness {
    finished: bool,
    counter: Arc<AtomicU64>,
}

impl Drop for AbortWitness {
    fn drop(&mut self) {
        if !self.finished {
            self.counter.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// A handler wrapped for [`Server::bind`](super::Server::bind).
pub fn mock() -> Arc<MockHandler> {
    Arc::new(MockHandler::new())
}
