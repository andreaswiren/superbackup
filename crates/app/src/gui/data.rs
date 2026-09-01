//! The window's model of the world, and everything derived from it.
//!
//! Deliberately free of egui: this is the layer that decides what a screen
//! says, so it can be tested without a rendering context. Screens read it and
//! draw; they never compute a status, a gate or a reason of their own.

// The interface is a library-shaped tree inside a binary crate. Its components,
// view models and fixtures are also compiled by `crates/app/tests/gui_app.rs`
// as a separate crate, so items that are used and tested there look unused from
// the binary's side. The allow is scoped to this module rather than the crate.
#![allow(dead_code)]
use std::collections::BTreeMap;

use chrono::{DateTime, Duration, Utc};
use uuid::Uuid;

use superbackup_core::ipc::protocol::{ErrorPayload, Reply, ServiceReply, VersionReply};
use superbackup_core::model::{Destination, DestinationKind, Job, Settings, StorageProvider};
use superbackup_core::state::{Event, Health, JobRun, JobSummary, RunStatus, StatusSnapshot};

use super::copy;
use super::daemon::{Incoming, Intent};
use super::format;

/// Everything the window knows, refreshed from the daemon.
#[derive(Debug, Default)]
pub struct Data {
    pub snapshot: Option<StatusSnapshot>,
    pub version: Option<VersionReply>,
    pub service: Option<ServiceReply>,
    pub settings: Settings,
    pub jobs: Vec<Job>,
    pub destinations: Vec<Destination>,
    pub providers: Vec<StorageProvider>,
    pub history: Vec<JobRun>,
    pub events: Vec<Event>,
    /// False once a request has come back as `DaemonUnreachable`.
    pub link_up: bool,
    /// True until the first status reply lands, so the interface can tell
    /// "empty" from "not asked yet" and not flash an empty state at startup.
    pub loading: bool,
    /// The stream told us it dropped items. The next status reply clears it.
    pub lagged: bool,
    pub last_error: Option<(Intent, ErrorPayload)>,
}

impl Data {
    pub fn new() -> Data {
        Data { loading: true, ..Default::default() }
    }

    // -- state from the daemon ---------------------------------------------

    pub fn unlocked(&self) -> bool {
        self.snapshot.as_ref().map(|s| s.unlocked).unwrap_or(false)
    }

    pub fn paused(&self) -> bool {
        self.snapshot.as_ref().map(|s| s.paused).unwrap_or(false)
    }

    pub fn paused_until(&self) -> Option<DateTime<Utc>> {
        self.snapshot.as_ref().and_then(|s| s.paused_until)
    }

    pub fn health(&self) -> Health {
        self.snapshot.as_ref().map(|s| s.health).unwrap_or(Health::Attention)
    }

    pub fn kopia_missing(&self) -> bool {
        match &self.snapshot {
            Some(s) => s.kopia_version.is_none(),
            None => false,
        }
    }

    pub fn machine_label(&self) -> &str {
        self.snapshot.as_ref().map(|s| s.machine_label.as_str()).unwrap_or("This machine")
    }

    pub fn machine_slug(&self) -> &str {
        self.snapshot.as_ref().map(|s| s.machine_slug.as_str()).unwrap_or("")
    }

    pub fn active_runs(&self) -> &[JobRun] {
        self.snapshot.as_ref().map(|s| s.active_runs.as_slice()).unwrap_or(&[])
    }

    pub fn job_summaries(&self) -> BTreeMap<Uuid, JobSummary> {
        self.snapshot.as_ref().map(|s| s.jobs.clone()).unwrap_or_default()
    }

    pub fn summary_for(&self, job: &Uuid) -> Option<JobSummary> {
        self.snapshot.as_ref().and_then(|s| s.jobs.get(job).cloned())
    }

    pub fn active_run_for(&self, job: &Uuid) -> Option<&JobRun> {
        self.active_runs().iter().find(|r| &r.job_id == job)
    }

    pub fn job(&self, id: &Uuid) -> Option<&Job> {
        self.jobs.iter().find(|j| &j.id == id)
    }

    pub fn destination(&self, id: &Uuid) -> Option<&Destination> {
        self.destinations.iter().find(|d| &d.id == id)
    }

    pub fn provider(&self, id: &Uuid) -> Option<&StorageProvider> {
        self.providers.iter().find(|p| &p.id == id)
    }

    pub fn destination_name(&self, id: &Uuid) -> String {
        self.destination(id)
            .map(|d| d.name.clone())
            .unwrap_or_else(|| format!("Destination {}", format::short_uuid(id)))
    }

    pub fn machine_hostname(&self) -> &str {
        self.snapshot.as_ref().map(|s| s.machine_hostname.as_str()).unwrap_or_default()
    }

    pub fn provider_name(&self, id: &Uuid) -> String {
        self.providers
            .iter()
            .find(|p| &p.id == id)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| format!("Provider {}", format::short_uuid(id)))
    }

    pub fn job_name(&self, id: &Uuid) -> String {
        self.job(id)
            .map(|j| j.name.clone())
            .unwrap_or_else(|| format!("Job {}", format::short_uuid(id)))
    }

    /// The jobs that write to a destination — the list the removal modal owes
    /// the user (`Config::jobs_using`, computed from what the daemon sent).
    pub fn jobs_using(&self, destination: &Uuid) -> Vec<&Job> {
        self.jobs.iter().filter(|j| j.destination_ids.contains(destination)).collect()
    }

    /// The jobs that would be left with nowhere to write.
    pub fn jobs_orphaned_by(&self, destination: &Uuid) -> Vec<&Job> {
        self.jobs
            .iter()
            .filter(|j| j.destination_ids.len() == 1 && j.destination_ids.contains(destination))
            .collect()
    }

    /// Destinations on a provider that inherit its credentials, and those that
    /// pinned their own. Rotation is dangerous without this distinction.
    pub fn destinations_using(&self, provider: &Uuid) -> (Vec<&Destination>, Vec<&Destination>) {
        let mut inheriting = Vec::new();
        let mut overriding = Vec::new();
        for d in &self.destinations {
            if let DestinationKind::S3 { provider_id, credential_override, .. } = &d.kind {
                if provider_id == provider {
                    if credential_override.is_some() {
                        overriding.push(d);
                    } else {
                        inheriting.push(d);
                    }
                }
            }
        }
        (inheriting, overriding)
    }

    /// Jobs that reach a provider through any of its destinations.
    pub fn jobs_via_provider(&self, provider: &Uuid) -> Vec<&Job> {
        let (inheriting, overriding) = self.destinations_using(provider);
        let ids: Vec<Uuid> = inheriting.iter().chain(overriding.iter()).map(|d| d.id).collect();
        self.jobs.iter().filter(|j| j.destination_ids.iter().any(|d| ids.contains(d))).collect()
    }

    pub fn stale_jobs(&self, now: DateTime<Utc>) -> Vec<&Job> {
        let days = self.settings.notifications.stale_after_days;
        if days == 0 {
            return Vec::new();
        }
        self.jobs
            .iter()
            .filter(|j| self.summary_for(&j.id).map(|s| s.is_stale(days, now)).unwrap_or(false))
            .collect()
    }

    pub fn failing_jobs(&self) -> Vec<&Job> {
        self.jobs
            .iter()
            .filter(|j| {
                self.summary_for(&j.id)
                    .map(|s| s.last_status == Some(RunStatus::Failed))
                    .unwrap_or(false)
            })
            .collect()
    }

    pub fn unverified_destinations(&self) -> Vec<&Destination> {
        self.destinations.iter().filter(|d| d.last_verified_at.is_none()).collect()
    }

    /// The one sentence the health tile shows, in the priority order the spec
    /// declares. Never two reasons at once.
    pub fn health_reason(&self, now: DateTime<Utc>) -> String {
        match self.health() {
            Health::Running => copy::dash_health_running(self.active_runs().len()),
            Health::Paused => match self.paused_until() {
                Some(until) => {
                    let time = format::clock(until);
                    match self.pause_reason() {
                        Some(reason) => copy::dash_health_paused_reason(&time, &reason),
                        None => copy::dash_health_paused_until(&time),
                    }
                }
                None => copy::dash::HEALTH_PAUSED_FOREVER.to_string(),
            },
            Health::Failed => {
                let failing = self.failing_jobs();
                match failing.first() {
                    Some(job) => {
                        let when = self
                            .summary_for(&job.id)
                            .and_then(|s| s.last_run)
                            .map(|t| format::relative_past(t, now))
                            .unwrap_or_else(|| copy::state::UNKNOWN.to_lowercase());
                        if failing.len() > 1 {
                            copy::dash_health_failed_more(&job.name, &when, failing.len() - 1)
                        } else {
                            copy::dash_health_failed(&job.name, &when)
                        }
                    }
                    None => copy::state::UNKNOWN.to_string(),
                }
            }
            Health::Attention => {
                if !self.unlocked() {
                    copy::dash::HEALTH_ATT_LOCKED.to_string()
                } else if self.kopia_missing() {
                    copy::dash::HEALTH_ATT_KOPIA.to_string()
                } else {
                    let stale = self.stale_jobs(now);
                    if !stale.is_empty() {
                        copy::dash_health_att_stale(
                            stale.len(),
                            self.settings.notifications.stale_after_days,
                        )
                    } else {
                        let unverified = self.unverified_destinations();
                        copy::dash_health_att_unverified(unverified.len())
                    }
                }
            }
            Health::Idle => match self.last_success(now) {
                Some(rel) => copy::dash_health_idle_last(&rel),
                None => copy::dash::HEALTH_IDLE_NEVER.to_string(),
            },
        }
    }

    pub fn pause_reason(&self) -> Option<String> {
        self.settings.pause.reason.clone()
    }

    fn last_success(&self, now: DateTime<Utc>) -> Option<String> {
        self.job_summaries()
            .values()
            .filter_map(|s| s.last_success)
            .max()
            .map(|t| format::relative_past(t, now))
    }

    pub fn next_scheduled(&self) -> Option<(Uuid, DateTime<Utc>)> {
        self.snapshot.as_ref().and_then(|s| s.next_scheduled)
    }

    // -- the seven-day strip ------------------------------------------------

    /// Seven days of run outcomes, oldest first, for the dashboard tile.
    pub fn last_seven_days(&self, now: DateTime<Utc>) -> Vec<DayOutcome> {
        let mut days: Vec<DayOutcome> = (0..7)
            .rev()
            .map(|back| {
                let day = now - Duration::days(back);
                DayOutcome { date: day, succeeded: 0, warned: 0, failed: 0, uploaded: 0 }
            })
            .collect();
        for run in &self.history {
            let age = (now.date_naive() - run.started_at.date_naive()).num_days();
            if !(0..7).contains(&age) {
                continue;
            }
            let index = (6 - age) as usize;
            if let Some(slot) = days.get_mut(index) {
                match run.status {
                    RunStatus::Succeeded => slot.succeeded += 1,
                    RunStatus::SucceededWithWarnings => slot.warned += 1,
                    RunStatus::Failed => slot.failed += 1,
                    _ => {}
                }
                slot.uploaded +=
                    run.destinations.iter().map(|d| d.progress.bytes_uploaded).sum::<u64>();
            }
        }
        days
    }

    // -- applying daemon messages ------------------------------------------

    pub fn apply(&mut self, message: Incoming) {
        match message {
            Incoming::Link { up, .. } => self.link_up = up,
            Incoming::Failed(intent, payload) => {
                self.loading = false;
                self.last_error = Some((intent, payload));
            }
            Incoming::Stream(item) => self.apply_stream(*item),
            Incoming::Reply(intent, reply) => self.apply_reply(intent, *reply),
        }
    }

    fn apply_reply(&mut self, intent: Intent, reply: Reply) {
        self.link_up = true;
        match reply {
            Reply::Status(r) => {
                self.snapshot = Some(*r.snapshot);
                self.loading = false;
                self.lagged = false;
                if let Some(s) = &self.snapshot {
                    self.events = s.recent_events.clone();
                }
            }
            Reply::Version(r) => self.version = Some(r),
            Reply::Service(r) => self.service = Some(r),
            Reply::Settings(r) => self.settings = *r.settings,
            Reply::Jobs(r) => self.jobs = r.jobs,
            Reply::Destinations(r) => self.destinations = r.destinations,
            Reply::Providers(r) => self.providers = r.providers,
            Reply::Runs(r) => self.history = r.runs,
            Reply::Job(r) => {
                let job = *r.job;
                match self.jobs.iter_mut().find(|j| j.id == job.id) {
                    Some(existing) => *existing = job,
                    None => self.jobs.push(job),
                }
            }
            Reply::Destination(r) => {
                let dest = *r.destination;
                match self.destinations.iter_mut().find(|d| d.id == dest.id) {
                    Some(existing) => *existing = dest,
                    None => self.destinations.push(dest),
                }
            }
            Reply::Provider(r) => {
                let provider = *r.provider;
                match self.providers.iter_mut().find(|p| p.id == provider.id) {
                    Some(existing) => *existing = provider,
                    None => self.providers.push(provider),
                }
            }
            Reply::Unlocked(r) => {
                if let Some(s) = &mut self.snapshot {
                    s.unlocked = r.unlocked;
                }
            }
            Reply::Pause(r) => {
                self.settings.pause = r.pause.clone();
                if let Some(s) = &mut self.snapshot {
                    s.paused = r.pause.paused;
                    s.paused_until = r.pause.until;
                }
            }
            Reply::Ack(_) => {
                if let Intent::DeleteJob(name) = &intent {
                    self.jobs.retain(|j| &j.name != name);
                }
                if let Intent::DeleteDestination(name) = &intent {
                    self.destinations.retain(|d| &d.name != name);
                }
                if let Intent::DeleteProvider(name) = &intent {
                    self.providers.retain(|p| &p.name != name);
                }
            }
            // Everything else is consumed by the screen that asked for it and
            // is routed through the app's pending-request table, not here.
            _ => {}
        }
    }

    fn apply_stream(&mut self, item: superbackup_core::ipc::protocol::StreamItem) {
        use superbackup_core::ipc::protocol::StreamItem as S;
        match item {
            S::Status { snapshot } => {
                self.snapshot = Some(*snapshot);
                self.loading = false;
                self.lagged = false;
            }
            S::Event { event } => {
                self.events.insert(0, *event);
                self.events.truncate(500);
            }
            S::Progress { run_id, destination_id, status, progress, .. } => {
                if let Some(s) = &mut self.snapshot {
                    if let Some(run) = s.active_runs.iter_mut().find(|r| r.run_id == run_id) {
                        if let Some(dest) =
                            run.destinations.iter_mut().find(|d| d.destination_id == destination_id)
                        {
                            dest.status = status;
                            dest.progress = *progress;
                        }
                        // The job's own status is always the roll-up, never a
                        // value the interface invents.
                        run.status = run.derive_status();
                    }
                }
            }
            // Items were dropped rather than buffered. The correct reaction is
            // to resynchronise, which the app does by reissuing `status`.
            S::Lagged { .. } => self.lagged = true,
        }
    }
}

/// One column of the dashboard's seven-day strip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DayOutcome {
    pub date: DateTime<Utc>,
    pub succeeded: usize,
    pub warned: usize,
    pub failed: usize,
    pub uploaded: u64,
}

impl DayOutcome {
    pub fn total(&self) -> usize {
        self.succeeded + self.warned + self.failed
    }
}

// ---------------------------------------------------------------------------
// The gate: what a locked vault, a missing daemon and a pause actually block
// ---------------------------------------------------------------------------

/// Every action the interface offers that can be refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    RunJob,
    StopRun,
    VerifyDestination,
    ConnectRepository,
    CreateRepository,
    BrowseSnapshots,
    Restore,
    TestProvider,
    RotateKeys,
    PullRemote,
    /// Editing configuration: names, sources, schedules, exclusions, retention.
    EditConfig,
    /// Reading history, activity and logs.
    ReadHistory,
    /// Pausing, resuming, changing the theme, quitting.
    Control,
}

/// Why an action is unavailable, or that it is not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    Allowed,
    /// The vault must be unlocked. Unlocking from here performs the action
    /// afterwards rather than discarding it (`UX_SPEC.md` §3.3).
    NeedsUnlock,
    /// The daemon is not answering.
    NeedsDaemon,
}

impl Gate {
    pub fn allowed(&self) -> bool {
        matches!(self, Gate::Allowed)
    }
    /// The tooltip and the AccessKit suffix for a refused control.
    pub fn reason(&self) -> Option<&'static str> {
        match self {
            Gate::Allowed => None,
            Gate::NeedsUnlock => Some(copy::locked::ACTION_BLOCKED),
            Gate::NeedsDaemon => Some(copy::err::DAEMON_UNREACHABLE),
        }
    }
}

impl Data {
    /// The one place the locked-vault matrix lives.
    ///
    /// Locking blocks anything that needs a `SecretRef` resolved, and nothing
    /// else: configuration editing, history and activity stay available,
    /// because locking must never hide information already on disk in plain
    /// text.
    pub fn gate(&self, action: Action) -> Gate {
        let needs_secret = matches!(
            action,
            Action::RunJob
                | Action::VerifyDestination
                | Action::ConnectRepository
                | Action::CreateRepository
                | Action::BrowseSnapshots
                | Action::Restore
                | Action::TestProvider
                | Action::RotateKeys
                | Action::PullRemote
        );
        let needs_daemon = needs_secret || matches!(action, Action::StopRun | Action::Control);

        if needs_daemon && !self.link_up {
            return Gate::NeedsDaemon;
        }
        if needs_secret && !self.unlocked() {
            return Gate::NeedsUnlock;
        }
        Gate::Allowed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use superbackup_core::state::{DestinationRun, Progress, Trigger};

    fn snapshot(unlocked: bool, paused: bool, health: Health) -> StatusSnapshot {
        StatusSnapshot {
            health,
            version: "0.1.0".into(),
            machine_label: "ANDREAS-PC".into(),
            machine_hostname: "ANDREAS-PC".into(),
            machine_slug: "andreas-pc".into(),
            unlocked,
            paused,
            paused_until: None,
            service_installed: true,
            service_running: true,
            kopia_version: Some("0.17.0".into()),
            active_runs: vec![],
            jobs: BTreeMap::new(),
            next_scheduled: None,
            recent_events: vec![],
            uptime_seconds: 42,
            generated_at: Utc::now(),
        }
    }

    fn data(unlocked: bool) -> Data {
        let mut d = Data::new();
        d.link_up = true;
        d.loading = false;
        d.snapshot = Some(snapshot(unlocked, false, Health::Idle));
        d
    }

    #[test]
    fn a_locked_vault_blocks_exactly_the_actions_that_need_a_secret() {
        let d = data(false);
        for action in [
            Action::RunJob,
            Action::VerifyDestination,
            Action::ConnectRepository,
            Action::CreateRepository,
            Action::BrowseSnapshots,
            Action::Restore,
            Action::TestProvider,
            Action::RotateKeys,
            Action::PullRemote,
        ] {
            assert_eq!(d.gate(action), Gate::NeedsUnlock, "{action:?} should be blocked");
        }
        for action in [Action::EditConfig, Action::ReadHistory, Action::Control, Action::StopRun] {
            assert_eq!(d.gate(action), Gate::Allowed, "{action:?} should stay available");
        }
    }

    #[test]
    fn an_unlocked_vault_blocks_nothing() {
        let d = data(true);
        for action in [
            Action::RunJob,
            Action::Restore,
            Action::EditConfig,
            Action::ReadHistory,
            Action::Control,
        ] {
            assert!(d.gate(action).allowed(), "{action:?}");
        }
    }

    #[test]
    fn a_missing_daemon_still_lets_configuration_be_edited() {
        let mut d = data(true);
        d.link_up = false;
        assert_eq!(d.gate(Action::RunJob), Gate::NeedsDaemon);
        assert_eq!(d.gate(Action::EditConfig), Gate::Allowed);
        assert_eq!(d.gate(Action::ReadHistory), Gate::Allowed);
    }

    #[test]
    fn a_locked_vault_outranks_a_missing_daemon_only_when_the_daemon_is_there() {
        let mut d = data(false);
        d.link_up = false;
        // Nothing can be run either way, but the reason the user is shown is
        // the one they can act on first.
        assert_eq!(d.gate(Action::RunJob), Gate::NeedsDaemon);
    }

    fn run_with(destinations: Vec<RunStatus>) -> JobRun {
        JobRun {
            run_id: Uuid::new_v4(),
            job_id: Uuid::new_v4(),
            job_name: "Dev code".into(),
            trigger: Trigger::Schedule,
            status: RunStatus::Running,
            started_at: Utc::now(),
            finished_at: None,
            destinations: destinations
                .into_iter()
                .enumerate()
                .map(|(i, status)| DestinationRun {
                    destination_id: Uuid::new_v4(),
                    destination_name: format!("dest {i}"),
                    status,
                    started_at: None,
                    finished_at: None,
                    progress: Progress::default(),
                    snapshot_id: None,
                    error: None,
                    warnings: vec![],
                    replicated_from: None,
                    skipped_reason: None,
                })
                .collect(),
        }
    }

    #[test]
    fn progress_updates_reroll_the_job_status_from_its_destinations() {
        let mut d = data(true);
        let mut run = run_with(vec![RunStatus::Running, RunStatus::Failed]);
        let dest_id = run.destinations[0].destination_id;
        let run_id = run.run_id;
        let job_id = run.job_id;
        run.status = RunStatus::Running;
        if let Some(s) = &mut d.snapshot {
            s.active_runs = vec![run];
        }

        d.apply(Incoming::Stream(Box::new(
            superbackup_core::ipc::protocol::StreamItem::Progress {
                run_id,
                job_id,
                destination_id: dest_id,
                status: RunStatus::Succeeded,
                progress: Box::new(Progress { bytes_processed: 10, ..Default::default() }),
            },
        )));

        let run = &d.snapshot.as_ref().expect("a snapshot").active_runs[0];
        // One destination failed: the run is a failure, never a success.
        assert_eq!(run.status, RunStatus::Failed);
        assert_eq!(run.destinations[0].status, RunStatus::Succeeded);
    }

    #[test]
    fn a_lagged_stream_sets_the_resynchronise_flag() {
        let mut d = data(true);
        d.apply(Incoming::Stream(Box::new(superbackup_core::ipc::protocol::StreamItem::Lagged {
            missed: 12,
        })));
        assert!(d.lagged);
    }

    #[test]
    fn health_reasons_follow_the_documented_priority() {
        let now = Utc::now();
        let mut d = data(false);
        if let Some(s) = &mut d.snapshot {
            s.health = Health::Attention;
        }
        assert_eq!(d.health_reason(now), copy::dash::HEALTH_ATT_LOCKED);

        // Unlocked but kopia missing: the reason moves down the list.
        let mut d = data(true);
        if let Some(s) = &mut d.snapshot {
            s.health = Health::Attention;
            s.kopia_version = None;
        }
        assert_eq!(d.health_reason(now), copy::dash::HEALTH_ATT_KOPIA);
    }

    #[test]
    fn destinations_using_a_provider_separate_the_ones_with_their_own_keys() {
        use superbackup_core::model::S3Credentials;
        let provider_id = Uuid::new_v4();
        let mut d = data(true);
        let make = |override_creds: bool, name: &str| Destination {
            id: Uuid::new_v4(),
            name: name.into(),
            kind: DestinationKind::S3 {
                provider_id,
                bucket: "b".into(),
                prefix: String::new(),
                credential_override: override_creds
                    .then(|| S3Credentials::for_destination(&Uuid::new_v4())),
            },
            encryption: None,
            passphrase_ref: None,
            retention: Default::default(),
            enabled: true,
            auto_discovered: false,
            bandwidth: None,
            replicate_from: None,
            created_at: Utc::now(),
            last_verified_at: None,
        };
        d.destinations = vec![make(false, "inherits"), make(true, "own key")];
        let (inheriting, overriding) = d.destinations_using(&provider_id);
        assert_eq!(inheriting.len(), 1);
        assert_eq!(inheriting[0].name, "inherits");
        assert_eq!(overriding.len(), 1);
        assert_eq!(overriding[0].name, "own key");
    }

    #[test]
    fn removing_a_destination_names_the_jobs_it_would_orphan() {
        let mut d = data(true);
        let dest = Uuid::new_v4();
        let other = Uuid::new_v4();
        let job = |name: &str, dests: Vec<Uuid>| Job {
            id: Uuid::new_v4(),
            name: name.into(),
            project_id: None,
            description: String::new(),
            sources: vec![],
            destination_ids: dests,
            schedule: Default::default(),
            exclusions: Default::default(),
            bandwidth: None,
            retention: None,
            enabled: true,
            timeout_minutes: None,
            hooks: Default::default(),
            continue_on_destination_error: true,
            created_at: Utc::now(),
            tags: vec![],
        };
        d.jobs = vec![job("only here", vec![dest]), job("also elsewhere", vec![dest, other])];
        assert_eq!(d.jobs_using(&dest).len(), 2);
        let orphans = d.jobs_orphaned_by(&dest);
        assert_eq!(orphans.len(), 1);
        assert_eq!(orphans[0].name, "only here");
    }

    #[test]
    fn the_seven_day_strip_has_seven_columns_oldest_first() {
        let d = data(true);
        let days = d.last_seven_days(Utc::now());
        assert_eq!(days.len(), 7);
        assert!(days[0].date < days[6].date);
    }
}
