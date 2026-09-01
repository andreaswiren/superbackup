//! The configuration domain model.
//!
//! This is the shared contract between the GUI, the CLI, the scheduler and the
//! Kopia driver. It is serialised to `config.json` (non-secret) and, for the
//! fields marked as secret, into the encrypted vault (`config.sbvault`).
//!
//! Invariant: **no secret material ever appears in this module.** Anything that
//! would be a password, key, or token is stored here as a [`SecretRef`] — a
//! stable handle that is resolved against the unlocked vault at use time.
//!
//! Shape of the world:
//!
//! ```text
//! StorageProvider  (credentials + endpoint, e.g. "StorJ eu-1")
//!        |  1..n
//! Destination      (provider + bucket + prefix, or a local path)
//!        |  n..n
//! Job              (sources -> many destinations, schedule, exclusions)
//!        |  n..1
//! Project          (grouping only)
//! ```
//!
//! A provider is defined once and reused by every destination that lives on
//! it. A destination pins a bucket and a key prefix, and may override the
//! provider credentials when that particular bucket uses its own key pair.
//!
//! # Chained destinations
//!
//! A destination may declare itself a **replica** of another
//! ([`Destination::replicate_from`]): instead of the job's sources being read,
//! chunked and encrypted a second time, the source destination's repository is
//! copied blob-for-blob with `kopia repository sync-to`. See
//! [`Destination::replicate_from`] for the constraints that fall out of how
//! kopia implements that, the most important of which is that **a replica is
//! the same repository, with the same encryption key and the same
//! passphrase** — it is not, and cannot be, independently keyed.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use uuid::Uuid;

/// Bump when a breaking change lands in the on-disk shapes below. The loader
/// migrates forward and refuses to load anything newer than it understands.
///
/// * 1 — first versioned schema.
/// * 2 — [`Destination::replicate_from`]. The bump is deliberate even though
///   the field has a serde default: an older build would silently drop it and
///   then *create a separate, independently keyed repository* at a destination
///   the user believes is a replica. Refusing to open the document is the only
///   safe way for an older build to behave, and refusing is exactly what the
///   version check makes it do.
///
/// Not every added field earns a bump, and the difference is what an older
/// build would *do* with the field missing, not whether the field is new.
/// [`ProviderKind::S3::admin_url`] is documentation: an older build that drops
/// it loses a bookmark and behaves identically in every other respect, so it
/// is a plain `#[serde(default)]` addition and the version stays at 2. The
/// test is always "would an older build act wrongly?", and for a link nobody
/// connects to the answer is no.
pub const CONFIG_SCHEMA_VERSION: u32 = 2;

/// How many `replicate_from` hops a chain may have.
///
/// Kopia places no limit — a replica is byte-identical to its source, so a
/// replica of a replica is well defined and works. The cap exists so a
/// malformed or hostile document cannot make chain resolution walk a very long
/// list, and so the run order of one job stays comprehensible to the person
/// reading it. Eight is far beyond any real topology.
pub const MAX_REPLICATION_DEPTH: usize = 8;

// ---------------------------------------------------------------------------
// Secret handles
// ---------------------------------------------------------------------------

/// A stable reference to a secret held in the encrypted vault.
///
/// The plaintext never touches `config.json`, log output, IPC responses, or
/// crash reports. Resolving a `SecretRef` requires an unlocked vault.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SecretRef(pub String);

impl SecretRef {
    pub fn new(kind: &str, owner: &Uuid) -> Self {
        SecretRef(format!("{kind}:{owner}"))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SecretRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

// ---------------------------------------------------------------------------
// Top-level configuration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    /// Identity of this machine; drives per-PC destination folder naming.
    pub machine: MachineIdentity,
    #[serde(default)]
    pub settings: Settings,
    /// Reusable credential + endpoint definitions. Referenced by destinations.
    #[serde(default)]
    pub providers: Vec<StorageProvider>,
    #[serde(default)]
    pub destinations: Vec<Destination>,
    #[serde(default)]
    pub projects: Vec<Project>,
    #[serde(default)]
    pub jobs: Vec<Job>,
    #[serde(default)]
    pub remote: Option<RemoteConfigSource>,
    #[serde(default)]
    pub updated_at: Option<DateTime<Utc>>,
}

fn default_schema_version() -> u32 {
    CONFIG_SCHEMA_VERSION
}

impl Default for Config {
    fn default() -> Self {
        Config {
            schema_version: CONFIG_SCHEMA_VERSION,
            machine: MachineIdentity::default(),
            settings: Settings::default(),
            providers: Vec::new(),
            destinations: Vec::new(),
            projects: Vec::new(),
            jobs: Vec::new(),
            remote: None,
            updated_at: None,
        }
    }
}

impl Config {
    pub fn job(&self, id: &Uuid) -> Option<&Job> {
        self.jobs.iter().find(|j| &j.id == id)
    }
    pub fn job_mut(&mut self, id: &Uuid) -> Option<&mut Job> {
        self.jobs.iter_mut().find(|j| &j.id == id)
    }
    pub fn destination(&self, id: &Uuid) -> Option<&Destination> {
        self.destinations.iter().find(|d| &d.id == id)
    }
    pub fn provider(&self, id: &Uuid) -> Option<&StorageProvider> {
        self.providers.iter().find(|p| &p.id == id)
    }
    pub fn project(&self, id: &Uuid) -> Option<&Project> {
        self.projects.iter().find(|p| &p.id == id)
    }

    /// Every destination that sits on a given provider.
    pub fn destinations_using(&self, provider_id: &Uuid) -> Vec<&Destination> {
        self.destinations.iter().filter(|d| d.kind.provider_id() == Some(provider_id)).collect()
    }

    /// Every job that writes to a given destination.
    pub fn jobs_using(&self, destination_id: &Uuid) -> Vec<&Job> {
        self.jobs.iter().filter(|j| j.destination_ids.contains(destination_id)).collect()
    }

    /// The destination a replica is synchronised from, if it is one and the
    /// source still exists.
    pub fn replication_source(&self, destination: &Destination) -> Option<&Destination> {
        self.destination(destination.replicate_from.as_ref()?)
    }

    /// Every destination that replicates *from* this one, in configuration
    /// order. Direct dependants only, not the whole subtree.
    pub fn replicas_of(&self, destination_id: &Uuid) -> Vec<&Destination> {
        self.destinations
            .iter()
            .filter(|d| d.replicate_from.as_ref() == Some(destination_id))
            .collect()
    }

    /// Walk `destination` up its chain to the destination that is actually
    /// backed up from sources.
    ///
    /// This is the destination whose repository — and therefore whose
    /// encryption key and passphrase — the whole chain shares, which is why
    /// every secret lookup for a replica has to start here rather than at the
    /// replica itself.
    ///
    /// Returns `None` when the chain is broken (a dangling `replicate_from`)
    /// or cyclic. Both are rejected by [`crate::config::validate`], so a caller
    /// holding a validated config may treat `None` as "not a replica"; a
    /// caller holding a leniently loaded one must handle it, which is why this
    /// returns an `Option` rather than looping forever or panicking.
    pub fn replication_root<'a>(&'a self, destination: &'a Destination) -> Option<&'a Destination> {
        let mut current = destination;
        let mut seen: Vec<Uuid> = vec![current.id];
        for _ in 0..MAX_REPLICATION_DEPTH {
            let Some(parent_id) = current.replicate_from else { return Some(current) };
            if seen.contains(&parent_id) {
                return None;
            }
            seen.push(parent_id);
            current = self.destination(&parent_id)?;
        }
        None
    }

    /// `destination`'s chain from its root down to itself, root first.
    ///
    /// Empty when the chain is broken or cyclic, for the reasons given on
    /// [`Config::replication_root`].
    pub fn replication_chain<'a>(&'a self, destination: &'a Destination) -> Vec<&'a Destination> {
        let mut chain: Vec<&Destination> = Vec::new();
        let mut current = destination;
        for _ in 0..=MAX_REPLICATION_DEPTH {
            chain.push(current);
            let Some(parent_id) = current.replicate_from else {
                chain.reverse();
                return chain;
            };
            if chain.iter().any(|d| d.id == parent_id) {
                return Vec::new();
            }
            match self.destination(&parent_id) {
                Some(parent) => current = parent,
                None => return Vec::new(),
            }
        }
        Vec::new()
    }

    /// Resolve a job by UUID, by exact name, or by unambiguous name prefix.
    /// This is what makes `superbackup run dev-code` work from the CLI.
    pub fn resolve_job(&self, needle: &str) -> Option<&Job> {
        resolve_by_name(needle, self.jobs.iter(), |j| (&j.id, &j.name))
    }
    pub fn resolve_destination(&self, needle: &str) -> Option<&Destination> {
        resolve_by_name(needle, self.destinations.iter(), |d| (&d.id, &d.name))
    }
    pub fn resolve_provider(&self, needle: &str) -> Option<&StorageProvider> {
        resolve_by_name(needle, self.providers.iter(), |p| (&p.id, &p.name))
    }
}

/// Shared UUID / exact-name / unambiguous-prefix lookup used by the CLI.
/// An ambiguous prefix returns `None` so the caller can report the ambiguity
/// rather than silently acting on the wrong object.
fn resolve_by_name<'a, T, I, F>(needle: &str, items: I, key: F) -> Option<&'a T>
where
    I: Iterator<Item = &'a T> + Clone,
    F: Fn(&'a T) -> (&'a Uuid, &'a String),
{
    if let Ok(id) = Uuid::parse_str(needle) {
        return items.clone().find(|t| key(t).0 == &id);
    }
    if let Some(hit) = items.clone().find(|t| key(t).1.eq_ignore_ascii_case(needle)) {
        return Some(hit);
    }
    let lower = needle.to_ascii_lowercase();
    let mut matches = items.filter(|t| key(t).1.to_ascii_lowercase().starts_with(&lower));
    let first = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    Some(first)
}

// ---------------------------------------------------------------------------
// Machine identity
// ---------------------------------------------------------------------------

/// Identifies this PC inside a shared destination that may hold backups from
/// many machines. Written to `<destination>/_superbackup/machines/<id>.json`
/// so that a human (or another install) opening the destination can tell at a
/// glance which folder belongs to which computer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MachineIdentity {
    /// Stable random v4 UUID generated on first run. Deliberately *not*
    /// derived from hardware serials, so it cannot be used to fingerprint the
    /// machine beyond this application.
    pub id: Uuid,
    /// Human-friendly label, defaults to the hostname, editable in Settings.
    pub label: String,
    pub hostname: String,
    pub os: String,
    pub os_version: String,
    pub arch: String,
    pub username: String,
    /// `<sanitised-label>-<first 8 of id>` — the on-disk folder name used
    /// under every destination root. Stable for the life of the install.
    pub slug: String,
    pub created_at: DateTime<Utc>,
}

impl MachineIdentity {
    /// Is this the identity `Default` mints, rather than one detected from the
    /// machine it is running on?
    ///
    /// Used to decide whether adopting the real hostname as the label is a
    /// correction or would be overwriting a name the user chose. `"this-pc"`
    /// and `"unknown"` are the two literals below and nothing else produces
    /// them together: `detect` sets both from the host, and a user who renames
    /// the machine changes `label` alone.
    pub fn is_placeholder(&self) -> bool {
        self.label == "this-pc" && self.hostname == "unknown"
    }
}

impl Default for MachineIdentity {
    fn default() -> Self {
        let id = Uuid::new_v4();
        MachineIdentity {
            id,
            label: "this-pc".into(),
            hostname: "unknown".into(),
            os: std::env::consts::OS.into(),
            os_version: String::new(),
            arch: std::env::consts::ARCH.into(),
            username: String::new(),
            slug: format!("this-pc-{}", &id.simple().to_string()[..8]),
            created_at: Utc::now(),
        }
    }
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Explicit path to the kopia executable. `None` means "discover it".
    ///
    /// When set, superbackup uses exactly this binary and never manages it —
    /// a user who has pinned their own build has said something deliberate,
    /// and silently replacing it would be wrong.
    pub kopia_path: Option<PathBuf>,
    #[serde(default)]
    pub kopia: KopiaManagement,
    pub start_at_login: bool,
    pub start_minimised: bool,
    pub run_as_service: bool,
    pub theme: Theme,
    pub notifications: NotificationSettings,
    pub bandwidth: BandwidthSettings,
    /// Global scheduling suppression, set by "Pause for X hours" in the tray.
    pub pause: PauseState,
    /// Do not start scheduled jobs while on a metered connection.
    pub skip_on_metered: bool,
    /// Do not start scheduled jobs while running on battery.
    pub skip_on_battery: bool,
    /// Run schedules that elapsed while the PC was asleep or powered off.
    pub run_missed_on_start: bool,
    /// Keep the vault key in memory for this long after the last GUI action.
    /// 0 = lock immediately when the window closes.
    pub auto_lock_minutes: u32,
    /// Cache the unlocked master key in the OS keychain so unattended service
    /// runs work without a prompt. Opt-in, off by default.
    pub use_os_keychain: bool,
    pub log_level: LogLevel,
    pub log_retention_days: u32,
    /// Concurrent job executions. Kopia parallelises within a single snapshot.
    pub max_parallel_jobs: u32,
    /// Write a small identifying manifest next to the backups at every
    /// destination that has a local path.
    ///
    /// On by default. It costs a few hundred bytes and it is the difference,
    /// during a disaster recovery, between a drive full of opaque folders and
    /// a drive that says which computer each folder came from and when it was
    /// last written. See
    /// [`crate::platform::identity::write_manifest`].
    ///
    /// It contains no secret and no file names — a label, a hostname, an OS
    /// version, and timestamps — but it is *identifying*, which is why it can
    /// be switched off.
    #[serde(default = "default_true")]
    pub write_machine_manifest: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            kopia_path: None,
            kopia: KopiaManagement::default(),
            start_at_login: true,
            start_minimised: true,
            run_as_service: false,
            theme: Theme::System,
            notifications: NotificationSettings::default(),
            bandwidth: BandwidthSettings::default(),
            pause: PauseState::default(),
            skip_on_metered: true,
            skip_on_battery: false,
            run_missed_on_start: true,
            auto_lock_minutes: 30,
            use_os_keychain: false,
            log_level: LogLevel::Info,
            log_retention_days: 30,
            max_parallel_jobs: 1,
            write_machine_manifest: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    System,
    Light,
    Dark,
}

/// How superbackup looks after the `kopia` binary it depends on.
///
/// Kopia is a hard prerequisite: without it, repository destinations cannot
/// work at all. Rather than presenting a new user with an installation errand,
/// superbackup fetches a verified build from Kopia's own GitHub releases on
/// first run and keeps it current.
///
/// That convenience is also a supply-chain decision, so it is bounded: the
/// download must come from the pinned upstream repository, its SHA-256 must
/// match the checksum file published with the same release, and the binary is
/// only moved into place after verification. See
/// `docs/compliance/THREAT_MODEL.md` §A8.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct KopiaManagement {
    /// Download and install kopia automatically when it is missing.
    ///
    /// On by default: a backup tool that cannot back up until the user has
    /// completed a separate installation is a backup tool that does not run.
    pub auto_install: bool,
    /// Check GitHub for a newer kopia release and offer or apply the upgrade.
    pub auto_update: UpdatePolicy,
    /// Minimum hours between update checks. Prevents a machine that restarts
    /// often from hammering the GitHub API, which is rate-limited per IP.
    pub check_interval_hours: u32,
    /// Upstream repository, as `owner/name`. Configurable so an organisation
    /// can point at an internal mirror, but any change moves the trust anchor
    /// and the interface says so.
    pub source_repo: String,
    /// Accept pre-release builds. Off by default.
    pub allow_prerelease: bool,
    /// Refuse to run against a kopia older than this. Guards against a
    /// downgrade attack and against genuinely incompatible old releases.
    pub minimum_version: String,
    /// Pin an exact version and stop tracking latest, for reproducible
    /// deployments.
    pub pinned_version: Option<String>,
    /// Prefer a kopia already on `PATH` over the managed one.
    ///
    /// True by default: if the user has installed kopia themselves, that is
    /// the one they expect to be used.
    pub prefer_system_binary: bool,
    /// When the last update check happened, so `check_interval_hours` can be
    /// honoured across restarts.
    pub last_check_at: Option<DateTime<Utc>>,
    /// The version currently installed under superbackup's own directory.
    pub managed_version: Option<String>,
}

impl Default for KopiaManagement {
    fn default() -> Self {
        KopiaManagement {
            auto_install: true,
            auto_update: UpdatePolicy::Notify,
            check_interval_hours: 24,
            source_repo: "kopia/kopia".into(),
            allow_prerelease: false,
            // Kopia's snapshot JSON output and repository flags settled by
            // 0.17; below that the driver's parsers cannot be relied upon.
            minimum_version: "0.17.0".into(),
            pinned_version: None,
            prefer_system_binary: true,
            last_check_at: None,
            managed_version: None,
        }
    }
}

/// What to do when a newer kopia release exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdatePolicy {
    /// Never check.
    Off,
    /// Check, and tell the user. Nothing is replaced without a decision.
    ///
    /// The default. Kopia is the component that reads and writes the
    /// repository, so swapping it underneath a working setup without asking
    /// is not a decision superbackup should make on the user's behalf.
    #[default]
    Notify,
    /// Check and install, but never while a job is running.
    Automatic,
}

impl UpdatePolicy {
    pub fn checks_for_updates(&self) -> bool {
        !matches!(self, UpdatePolicy::Off)
    }
    pub fn title(&self) -> &'static str {
        match self {
            UpdatePolicy::Off => "Never check",
            UpdatePolicy::Notify => "Tell me when an update is available",
            UpdatePolicy::Automatic => "Install updates automatically",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl LogLevel {
    pub fn as_filter(&self) -> &'static str {
        match self {
            LogLevel::Error => "error",
            LogLevel::Warn => "warn",
            LogLevel::Info => "info",
            LogLevel::Debug => "debug",
            LogLevel::Trace => "trace",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationSettings {
    pub enabled: bool,
    pub on_failure: bool,
    pub on_success: bool,
    /// Warn when a job has not completed successfully for this many days.
    pub stale_after_days: u32,
    pub on_service_error: bool,
    /// Suppress repeats of the same failure within this window.
    pub dedupe_minutes: u32,
}

impl Default for NotificationSettings {
    fn default() -> Self {
        NotificationSettings {
            enabled: true,
            on_failure: true,
            on_success: false,
            stale_after_days: 3,
            on_service_error: true,
            dedupe_minutes: 60,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BandwidthSettings {
    /// Upload ceiling in kilobytes per second. `None` = unlimited.
    pub upload_kbps: Option<u32>,
    pub download_kbps: Option<u32>,
    /// Optional lower ceiling applied inside a daily window (e.g. work hours).
    pub schedule: Option<BandwidthWindow>,
}

impl BandwidthSettings {
    pub fn is_unlimited(&self) -> bool {
        self.upload_kbps.is_none() && self.download_kbps.is_none() && self.schedule.is_none()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BandwidthWindow {
    /// Minutes past local midnight.
    pub start_minute: u32,
    pub end_minute: u32,
    pub upload_kbps: Option<u32>,
    pub download_kbps: Option<u32>,
    /// 0 = Monday .. 6 = Sunday. Empty = every day.
    #[serde(default)]
    pub weekdays: Vec<u8>,
}

/// Global pause. Set by the tray "Pause for 1/2/4/8 hours" menu, by
/// `superbackup pause 4h`, or indefinitely.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PauseState {
    pub paused: bool,
    /// `None` while `paused` means "paused until explicitly resumed".
    pub until: Option<DateTime<Utc>>,
    pub reason: Option<String>,
}

impl PauseState {
    pub fn is_active(&self, now: DateTime<Utc>) -> bool {
        match (self.paused, self.until) {
            (false, _) => false,
            (true, None) => true,
            (true, Some(until)) => now < until,
        }
    }
}

// ---------------------------------------------------------------------------
// Storage providers
// ---------------------------------------------------------------------------

/// A reusable remote-storage account: endpoint, region, and credentials.
///
/// Defined once, referenced by any number of [`Destination`]s. Rotating a key
/// here rotates it for every bucket and every job that uses this provider —
/// unless a destination has pinned its own credential override.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageProvider {
    pub id: Uuid,
    /// Display name, e.g. "StorJ eu-1 (personal)".
    pub name: String,
    pub kind: ProviderKind,
    #[serde(default)]
    pub notes: String,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub last_verified_at: Option<DateTime<Utc>>,
}

impl StorageProvider {
    /// Every secret handle this provider owns, for vault garbage collection
    /// and for the "what will I delete?" confirmation in the GUI.
    pub fn secret_refs(&self) -> Vec<&SecretRef> {
        match &self.kind {
            ProviderKind::S3 { credentials, .. } => credentials.refs(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProviderKind {
    /// Any S3-compatible endpoint. StorJ is the tested default; AWS,
    /// Backblaze B2's S3 API, Wasabi and MinIO are all reachable here.
    S3 {
        /// Host, with or without scheme, e.g. `https://gateway.storjshare.io`.
        endpoint: String,
        region: String,
        credentials: S3Credentials,
        #[serde(default = "default_true")]
        tls: bool,
        /// Request path-style addressing (`host/bucket/key`) rather than
        /// virtual-hosted style.
        ///
        /// **Honoured by superbackup's own S3 client; still ignored by
        /// kopia.** Kopia's S3 backend is minio-go, which selects path-style
        /// automatically for endpoints that need it, and `cli/storage_s3.go`
        /// exposes no flag to override that choice — so backups are unaffected
        /// by this field and `KopiaDriver::unsupported_options()` still
        /// reports it as inert for them.
        ///
        /// [`crate::s3::S3Endpoint::uses_path_style`] does honour it, for the
        /// bucket and object listings this application makes itself. It
        /// applies the same rule minio-go does — path style for everything
        /// that is not `*.amazonaws.com`, and for any bucket name that cannot
        /// be a DNS label — so the two agree about where an object lives even
        /// though only one of them reads the flag. The interface must keep
        /// saying so rather than implying the flag changes how a backup is
        /// written.
        #[serde(default)]
        path_style: bool,
        #[serde(default)]
        flavour: S3Flavour,
        /// Where a human goes to administer this account: StorJ's console,
        /// the AWS console, a MinIO web UI.
        ///
        /// **Documentation only.** Nothing connects to it, nothing validates
        /// that it resolves, and an empty value must never block saving a
        /// provider or creating a destination. It exists because "which
        /// console do I log into to rotate this key?" is a real question at
        /// 2am and the answer is otherwise in the user's head.
        ///
        /// It belongs to the *provider* rather than to a destination because
        /// it is an account-level fact: one StorJ account with three buckets
        /// has one console. A destination reaches it through its provider,
        /// which is the whole point of the provider/destination split.
        ///
        /// Not a secret, so it lives in `config.json` beside the endpoint —
        /// but it can carry a tenant or account identifier, so it is kept out
        /// of the plain-text key-export document, which is meant to be
        /// printed. The published (remote) copy of the configuration is
        /// sealed inside the vault, so it travels there with the same
        /// protection as the endpoint itself.
        #[serde(default)]
        admin_url: Option<String>,
    },
}

fn default_true() -> bool {
    true
}

/// Access-key pair, held as vault handles only.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct S3Credentials {
    pub access_key_ref: SecretRef,
    pub secret_key_ref: SecretRef,
    /// Session token, for temporary STS-style credentials.
    #[serde(default)]
    pub session_token_ref: Option<SecretRef>,
}

impl S3Credentials {
    pub fn for_provider(provider_id: &Uuid) -> Self {
        S3Credentials {
            access_key_ref: SecretRef(format!("s3.access:{provider_id}")),
            secret_key_ref: SecretRef(format!("s3.secret:{provider_id}")),
            session_token_ref: None,
        }
    }
    pub fn for_destination(destination_id: &Uuid) -> Self {
        S3Credentials {
            access_key_ref: SecretRef(format!("s3.access.dest:{destination_id}")),
            secret_key_ref: SecretRef(format!("s3.secret.dest:{destination_id}")),
            session_token_ref: None,
        }
    }
    pub fn refs(&self) -> Vec<&SecretRef> {
        let mut v = vec![&self.access_key_ref, &self.secret_key_ref];
        if let Some(t) = &self.session_token_ref {
            v.push(t);
        }
        v
    }
}

/// Known S3 dialects. Only used to pick sensible defaults and to write
/// accurate help text — the transport is identical.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum S3Flavour {
    #[default]
    Storj,
    AwsS3,
    BackblazeB2,
    Wasabi,
    MinIo,
    Cloudflare,
    Other,
}

impl S3Flavour {
    pub fn all() -> &'static [S3Flavour] {
        use S3Flavour::*;
        &[Storj, AwsS3, BackblazeB2, Wasabi, MinIo, Cloudflare, Other]
    }
    pub fn title(&self) -> &'static str {
        use S3Flavour::*;
        match self {
            Storj => "StorJ",
            AwsS3 => "Amazon S3",
            BackblazeB2 => "Backblaze B2",
            Wasabi => "Wasabi",
            MinIo => "MinIO",
            Cloudflare => "Cloudflare R2",
            Other => "Other S3-compatible",
        }
    }
    /// Endpoint and region prefilled when the user picks this flavour.
    pub fn default_endpoint(&self) -> Option<&'static str> {
        use S3Flavour::*;
        match self {
            Storj => Some("https://gateway.storjshare.io"),
            AwsS3 => Some("https://s3.amazonaws.com"),
            Wasabi => Some("https://s3.wasabisys.com"),
            _ => None,
        }
    }
    pub fn default_region(&self) -> Option<&'static str> {
        match self {
            S3Flavour::Storj => Some("eu-1"),
            S3Flavour::AwsS3 => Some("us-east-1"),
            S3Flavour::Cloudflare => Some("auto"),
            _ => None,
        }
    }
    pub fn wants_path_style(&self) -> bool {
        matches!(self, S3Flavour::MinIo)
    }
    /// The console a user of this flavour most likely administers from.
    ///
    /// A *prefill*, never a value the user is stuck with: a self-hosted MinIO
    /// or an S3-compatible provider nobody has heard of has no obvious
    /// answer, and even StorJ users on a custom deployment must be able to
    /// clear it. Only the two flavours with one unambiguous console get one.
    pub fn default_admin_url(&self) -> Option<&'static str> {
        match self {
            S3Flavour::Storj => Some("https://storj.io/login"),
            S3Flavour::AwsS3 => Some("https://console.aws.amazon.com/s3/"),
            _ => None,
        }
    }
}

/// Check an administration-panel URL without connecting to it.
///
/// Deliberately minimal. This is a bookmark, so the only things worth
/// refusing are the ones that could *do* something: a `javascript:` or `file:`
/// URL that a click would execute or open locally. Everything else — a typo, a
/// host that no longer exists, a redirect — is the user's own note to
/// themselves and is none of our business. An empty value is always valid,
/// because the field is optional and an optional field that can block a save
/// is not optional.
pub fn validate_admin_url(input: &str) -> Result<(), &'static str> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    let Some((scheme, rest)) = trimmed.split_once("://") else {
        return Err("Enter a full web address, starting with https://.");
    };
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return Err("Only http:// and https:// addresses are allowed here.");
    }
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    if host.trim().is_empty() {
        return Err("Enter a full web address, starting with https://.");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Destinations
// ---------------------------------------------------------------------------

/// Where backups land. A destination owns at most one Kopia repository; plain
/// mirror destinations own none.
///
/// Many jobs may share one destination, and one job may fan out to many.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Destination {
    pub id: Uuid,
    pub name: String,
    pub kind: DestinationKind,
    /// Set for repository destinations. `None` for [`DestinationKind::LocalMirror`].
    #[serde(default)]
    pub encryption: Option<EncryptionSettings>,
    /// Handle to the repository passphrase inside the vault.
    #[serde(default)]
    pub passphrase_ref: Option<SecretRef>,
    #[serde(default)]
    pub retention: RetentionPolicy,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// True when superbackup created this destination automatically (OneDrive
    /// discovery). Kept so the GUI can mark it and offer to re-detect.
    #[serde(default)]
    pub auto_discovered: bool,
    /// Per-destination bandwidth ceiling, overriding the global setting.
    #[serde(default)]
    pub bandwidth: Option<BandwidthSettings>,
    /// When set, this destination is a **replica** of the destination with
    /// this id: its contents are produced by `kopia repository sync-to` from
    /// that repository, not by reading the job's sources a second time.
    ///
    /// # What kopia actually does, and what follows from it
    ///
    /// `repository sync-to` is *blob-level replication*. It compares the two
    /// storage locations' blob lists and copies what is missing — including
    /// the `kopia.repository` format blob, which it writes to an empty
    /// destination before anything else. If the destination already holds a
    /// repository with a different unique id, kopia refuses outright with
    /// `destination repository contains incompatible data`
    /// (`ensureRepositoriesHaveSameFormatBlob` in `cli/command_repository_sync.go`).
    ///
    /// So a replica is not "another backup of the same data": it *is* the same
    /// repository, at a second address. Three consequences, all load-bearing:
    ///
    /// 1. **The encryption key is not independent.** The replica has the
    ///    source's format blob, so it has the source's master key and opens
    ///    with the source's passphrase. A replica must therefore carry no
    ///    `encryption` and no `passphrase_ref` of its own — the validator
    ///    rejects a document where it does, because a user who believes the
    ///    offsite copy has its own key has a false belief about the only thing
    ///    that matters if the source machine is compromised.
    /// 2. **A replica is never created.** Running `repository create` against
    ///    it would mint a *different* unique id and every later sync would
    ///    fail. The engine connects rather than creates.
    /// 3. **The source must be a repository.** A
    ///    [`DestinationKind::LocalMirror`] is a plain file copy with no blobs
    ///    and no format blob, so there is nothing for kopia to replicate.
    ///
    /// Chains deeper than one hop are allowed (see [`MAX_REPLICATION_DEPTH`]):
    /// a replica is byte-identical to its source, so replicating *it* onward is
    /// the same operation again. Cycles are not, and are rejected at
    /// validation.
    #[serde(default)]
    pub replicate_from: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub last_verified_at: Option<DateTime<Utc>>,
}

impl Destination {
    /// True when this destination is filled by replication rather than by
    /// backing up the job's sources.
    pub fn is_replica(&self) -> bool {
        self.replicate_from.is_some()
    }

    pub fn secret_refs(&self) -> Vec<&SecretRef> {
        let mut v = Vec::new();
        if let Some(p) = &self.passphrase_ref {
            v.push(p);
        }
        if let DestinationKind::S3 { credential_override: Some(c), .. } = &self.kind {
            v.extend(c.refs());
        }
        v
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DestinationKind {
    /// Kopia repository on a local disk, external drive, or UNC share.
    LocalRepository { path: PathBuf },
    /// Kopia repository inside a detected OneDrive folder. Behaves like
    /// `LocalRepository` but is tracked separately so the GUI can explain the
    /// OneDrive interaction and set the right sync exclusions.
    OneDrive { path: PathBuf, account: Option<String> },
    /// Kopia repository in a bucket on a reusable [`StorageProvider`].
    S3 {
        /// The provider supplying endpoint, region and default credentials.
        provider_id: Uuid,
        bucket: String,
        /// Object-key prefix inside the bucket. Defaults to
        /// `superbackup/<machine-slug>/`, which is what keeps several PCs and
        /// several jobs apart inside one bucket.
        #[serde(default)]
        prefix: String,
        /// Credentials scoped to this bucket, when it does not use the
        /// provider-level key pair. `None` = inherit from the provider.
        #[serde(default)]
        credential_override: Option<S3Credentials>,
    },
    /// Straight file mirror — no repository, no encryption, no dedup. This is
    /// the "I want a plain readable copy" mode.
    LocalMirror { path: PathBuf },
}

impl DestinationKind {
    pub fn label(&self) -> &'static str {
        match self {
            DestinationKind::LocalRepository { .. } => "Local repository",
            DestinationKind::OneDrive { .. } => "OneDrive repository",
            DestinationKind::S3 { .. } => "S3 bucket",
            DestinationKind::LocalMirror { .. } => "Folder mirror",
        }
    }
    /// Mirror destinations are plain copies; everything else is a Kopia repo.
    pub fn is_repository(&self) -> bool {
        !matches!(self, DestinationKind::LocalMirror { .. })
    }
    /// Local filesystem root, when there is one.
    pub fn local_path(&self) -> Option<&PathBuf> {
        match self {
            DestinationKind::LocalRepository { path }
            | DestinationKind::OneDrive { path, .. }
            | DestinationKind::LocalMirror { path } => Some(path),
            DestinationKind::S3 { .. } => None,
        }
    }
    pub fn provider_id(&self) -> Option<&Uuid> {
        match self {
            DestinationKind::S3 { provider_id, .. } => Some(provider_id),
            _ => None,
        }
    }
    /// The credentials to use for this destination, given its provider.
    pub fn effective_credentials<'a>(
        &'a self,
        provider: Option<&'a StorageProvider>,
    ) -> Option<&'a S3Credentials> {
        match self {
            DestinationKind::S3 { credential_override: Some(c), .. } => Some(c),
            DestinationKind::S3 { .. } => match provider.map(|p| &p.kind) {
                Some(ProviderKind::S3 { credentials, .. }) => Some(credentials),
                None => None,
            },
            _ => None,
        }
    }
}

/// Normalise a user-typed prefix into a trailing-slashed, leading-slash-free
/// key segment. Empty input yields the empty prefix (bucket root).
pub fn normalise_prefix(input: &str) -> String {
    let trimmed = input.trim().trim_matches('/');
    if trimmed.is_empty() {
        return String::new();
    }
    let cleaned: Vec<&str> =
        trimmed.split('/').filter(|seg| !seg.is_empty() && *seg != "." && *seg != "..").collect();
    if cleaned.is_empty() {
        return String::new();
    }
    format!("{}/", cleaned.join("/"))
}

/// The default prefix for a new S3 destination on this machine.
pub fn default_s3_prefix(machine_slug: &str) -> String {
    format!("superbackup/{machine_slug}/")
}

// ---------------------------------------------------------------------------
// Encryption
// ---------------------------------------------------------------------------

/// Mirrors the knobs Kopia exposes at `repository create` time. Defaults match
/// Kopia's own recommended defaults, which we do not silently override.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EncryptionSettings {
    pub algorithm: EncryptionAlgorithm,
    pub hash: HashAlgorithm,
    pub splitter: Splitter,
    pub ecc: Option<EccAlgorithm>,
    /// Percentage of error-correction overhead when `ecc` is set.
    pub ecc_overhead_percent: u8,
    /// How the repository passphrase was produced.
    pub passphrase_source: PassphraseSource,
}

impl Default for EncryptionSettings {
    fn default() -> Self {
        EncryptionSettings {
            algorithm: EncryptionAlgorithm::Aes256GcmHmacSha256,
            hash: HashAlgorithm::Blake2b256,
            splitter: Splitter::Dynamic4mBuzhash,
            ecc: None,
            ecc_overhead_percent: 0,
            passphrase_source: PassphraseSource::Generated,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EncryptionAlgorithm {
    #[serde(rename = "AES256-GCM-HMAC-SHA256")]
    Aes256GcmHmacSha256,
    #[serde(rename = "CHACHA20-POLY1305-HMAC-SHA256")]
    Chacha20Poly1305HmacSha256,
}

impl EncryptionAlgorithm {
    /// The exact token kopia expects for `--encryption`.
    pub fn kopia_id(&self) -> &'static str {
        match self {
            EncryptionAlgorithm::Aes256GcmHmacSha256 => "AES256-GCM-HMAC-SHA256",
            EncryptionAlgorithm::Chacha20Poly1305HmacSha256 => "CHACHA20-POLY1305-HMAC-SHA256",
        }
    }
    pub fn all() -> &'static [EncryptionAlgorithm] {
        &[EncryptionAlgorithm::Aes256GcmHmacSha256, EncryptionAlgorithm::Chacha20Poly1305HmacSha256]
    }
    pub fn describe(&self) -> &'static str {
        match self {
            EncryptionAlgorithm::Aes256GcmHmacSha256 => {
                "Hardware-accelerated on virtually every modern x86 and ARM CPU. Recommended."
            }
            EncryptionAlgorithm::Chacha20Poly1305HmacSha256 => {
                "Faster on CPUs without AES instructions. Equally strong."
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HashAlgorithm {
    #[serde(rename = "BLAKE2B-256")]
    Blake2b256,
    #[serde(rename = "BLAKE2B-256-128")]
    Blake2b256128,
    #[serde(rename = "BLAKE2S-256")]
    Blake2s256,
    #[serde(rename = "BLAKE3-256")]
    Blake3256,
    #[serde(rename = "HMAC-SHA256")]
    HmacSha256,
    #[serde(rename = "HMAC-SHA256-128")]
    HmacSha256128,
}

impl HashAlgorithm {
    pub fn kopia_id(&self) -> &'static str {
        match self {
            HashAlgorithm::Blake2b256 => "BLAKE2B-256",
            HashAlgorithm::Blake2b256128 => "BLAKE2B-256-128",
            HashAlgorithm::Blake2s256 => "BLAKE2S-256",
            HashAlgorithm::Blake3256 => "BLAKE3-256",
            HashAlgorithm::HmacSha256 => "HMAC-SHA256",
            HashAlgorithm::HmacSha256128 => "HMAC-SHA256-128",
        }
    }
    pub fn all() -> &'static [HashAlgorithm] {
        &[
            HashAlgorithm::Blake2b256,
            HashAlgorithm::Blake2b256128,
            HashAlgorithm::Blake3256,
            HashAlgorithm::Blake2s256,
            HashAlgorithm::HmacSha256,
            HashAlgorithm::HmacSha256128,
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Splitter {
    #[serde(rename = "DYNAMIC-4M-BUZHASH")]
    Dynamic4mBuzhash,
    #[serde(rename = "DYNAMIC-8M-BUZHASH")]
    Dynamic8mBuzhash,
    #[serde(rename = "DYNAMIC-2M-BUZHASH")]
    Dynamic2mBuzhash,
    #[serde(rename = "FIXED-4M")]
    Fixed4m,
}

impl Splitter {
    pub fn kopia_id(&self) -> &'static str {
        match self {
            Splitter::Dynamic4mBuzhash => "DYNAMIC-4M-BUZHASH",
            Splitter::Dynamic8mBuzhash => "DYNAMIC-8M-BUZHASH",
            Splitter::Dynamic2mBuzhash => "DYNAMIC-2M-BUZHASH",
            Splitter::Fixed4m => "FIXED-4M",
        }
    }
    pub fn all() -> &'static [Splitter] {
        &[
            Splitter::Dynamic4mBuzhash,
            Splitter::Dynamic8mBuzhash,
            Splitter::Dynamic2mBuzhash,
            Splitter::Fixed4m,
        ]
    }
    /// Millions of tiny files (node_modules, .next/cache) dedup better with a
    /// smaller average block. The GUI surfaces this as a one-click preset.
    pub fn recommended_for_many_small_files() -> Splitter {
        Splitter::Dynamic2mBuzhash
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EccAlgorithm {
    #[serde(rename = "REED-SOLOMON-CRC32")]
    ReedSolomonCrc32,
}

impl EccAlgorithm {
    pub fn kopia_id(&self) -> &'static str {
        "REED-SOLOMON-CRC32"
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PassphraseSource {
    /// 256 bits from the OS CSPRNG, stored only in the vault. The user is
    /// forced through a "write this down" screen before the repo is created.
    Generated,
    /// Typed by the user. Strength-checked before acceptance.
    UserSupplied,
    /// Deterministically derived from the master key + destination id, so the
    /// vault alone can reconstruct it. Convenient, still never leaves the vault.
    DerivedFromMaster,
}

// ---------------------------------------------------------------------------
// Retention
// ---------------------------------------------------------------------------

/// Maps 1:1 onto `kopia policy set --keep-*`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RetentionPolicy {
    pub keep_latest: u32,
    pub keep_hourly: u32,
    pub keep_daily: u32,
    pub keep_weekly: u32,
    pub keep_monthly: u32,
    pub keep_annual: u32,
    /// Run `kopia maintenance run` after this many successful snapshots.
    pub maintenance_every_n_runs: u32,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        RetentionPolicy {
            keep_latest: 10,
            keep_hourly: 24,
            keep_daily: 14,
            keep_weekly: 8,
            keep_monthly: 12,
            keep_annual: 3,
            maintenance_every_n_runs: 20,
        }
    }
}

// ---------------------------------------------------------------------------
// Projects and jobs
// ---------------------------------------------------------------------------

/// A grouping of jobs, purely organisational — "Development", "Documents".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub colour: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub project_id: Option<Uuid>,
    #[serde(default)]
    pub description: String,
    pub sources: Vec<Source>,
    /// One job fans out to every destination listed here — typically a fast
    /// local repo, a OneDrive repo, and an offsite S3 bucket.
    pub destination_ids: Vec<Uuid>,
    #[serde(default)]
    pub schedule: Schedule,
    #[serde(default)]
    pub exclusions: ExclusionSet,
    /// Per-job override of the global bandwidth ceiling.
    #[serde(default)]
    pub bandwidth: Option<BandwidthSettings>,
    /// Per-job override of the destination retention policy.
    #[serde(default)]
    pub retention: Option<RetentionPolicy>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Abort a run that exceeds this many minutes. `None` = no limit.
    #[serde(default)]
    pub timeout_minutes: Option<u32>,
    #[serde(default)]
    pub hooks: JobHooks,
    /// Keep going to the remaining destinations when one fails, instead of
    /// failing the whole job at the first broken destination.
    #[serde(default = "default_true")]
    pub continue_on_destination_error: bool,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    pub path: PathBuf,
    /// Follow symlinks out of the source tree. Off by default — following them
    /// is how a "back up my project" job accidentally swallows the whole disk.
    #[serde(default)]
    pub follow_symlinks: bool,
    /// Stay on one filesystem while walking.
    #[serde(default)]
    pub one_filesystem: bool,
}

impl Source {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Source { path: path.into(), follow_symlinks: false, one_filesystem: false }
    }
}

/// Ignore rules. `presets` expand into concrete globs at run time so that the
/// stored config stays readable and the preset list can grow between releases.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ExclusionSet {
    pub presets: Vec<ExclusionPreset>,
    /// Raw `.gitignore`-style patterns handed to `kopia policy set --add-ignore`.
    pub patterns: Vec<String>,
    /// Honour `.gitignore` files found inside the source tree.
    pub use_gitignore: bool,
    /// Skip files larger than this many megabytes.
    pub max_file_size_mb: Option<u64>,
    /// Skip directories tagged with `CACHEDIR.TAG`.
    pub respect_cachedir_tag: bool,
}

impl ExclusionSet {
    /// The preset that makes a dev-folder job survivable, applied by the
    /// "Development folder" template in the new-job wizard.
    pub fn developer_defaults() -> Self {
        use ExclusionPreset::*;
        ExclusionSet {
            presets: vec![
                NodeModules,
                NextCache,
                RustTarget,
                PythonCaches,
                DotnetBuild,
                JavaBuild,
                GoBuild,
                IdeMetadata,
                OsJunk,
                LogsAndTemp,
            ],
            patterns: Vec::new(),
            use_gitignore: false,
            max_file_size_mb: None,
            respect_cachedir_tag: true,
        }
    }

    /// Fully expanded pattern list, presets first then user patterns.
    pub fn effective_patterns(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for p in &self.presets {
            out.extend(p.patterns().iter().map(|s| s.to_string()));
        }
        out.extend(self.patterns.iter().cloned());
        out.sort();
        out.dedup();
        out
    }
}

/// Curated ignore bundles. These exist because the whole reason this tool
/// exists is that OneDrive drowns in `node_modules` and `.next/cache`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExclusionPreset {
    NodeModules,
    NextCache,
    RustTarget,
    PythonCaches,
    DotnetBuild,
    JavaBuild,
    GoBuild,
    IdeMetadata,
    OsJunk,
    VirtualMachineImages,
    LogsAndTemp,
    GitObjects,
}

impl ExclusionPreset {
    pub fn all() -> &'static [ExclusionPreset] {
        use ExclusionPreset::*;
        &[
            NodeModules,
            NextCache,
            RustTarget,
            PythonCaches,
            DotnetBuild,
            JavaBuild,
            GoBuild,
            IdeMetadata,
            OsJunk,
            VirtualMachineImages,
            LogsAndTemp,
            GitObjects,
        ]
    }

    pub fn title(&self) -> &'static str {
        use ExclusionPreset::*;
        match self {
            NodeModules => "node_modules",
            NextCache => "Next.js / bundler caches",
            RustTarget => "Rust target directories",
            PythonCaches => "Python caches and venvs",
            DotnetBuild => ".NET build output",
            JavaBuild => "Java / Gradle / Maven output",
            GoBuild => "Go build cache",
            IdeMetadata => "Editor scratch files",
            OsJunk => "Recycle Bin and thumbnail files",
            VirtualMachineImages => "Virtual machine images",
            LogsAndTemp => "Logs and temporary files",
            GitObjects => "Git packfiles",
        }
    }

    /// Expanded `.gitignore`-syntax patterns.
    pub fn patterns(&self) -> &'static [&'static str] {
        use ExclusionPreset::*;
        match self {
            NodeModules => &["/**/node_modules/", "/**/.pnpm-store/", "/**/.yarn/cache/"],
            NextCache => &[
                "/**/.next/cache/",
                "/**/.turbo/",
                "/**/.parcel-cache/",
                "/**/.vite/",
                "/**/.nuxt/",
                "/**/.svelte-kit/",
                "/**/.astro/",
                "/**/.angular/cache/",
                "/**/.webpack/",
            ],
            RustTarget => &["/**/target/debug/", "/**/target/release/", "/**/target/tmp/"],
            PythonCaches => &[
                "/**/__pycache__/",
                "/**/*.pyc",
                "/**/.pytest_cache/",
                "/**/.mypy_cache/",
                "/**/.ruff_cache/",
                "/**/.tox/",
                "/**/.venv/",
                "/**/venv/",
            ],
            DotnetBuild => &["/**/bin/Debug/", "/**/bin/Release/", "/**/obj/"],
            JavaBuild => &["/**/.gradle/", "/**/build/tmp/", "/**/.m2/repository/"],
            GoBuild => &["/**/.cache/go-build/", "/**/pkg/mod/cache/"],
            IdeMetadata => &["/**/.idea/shelf/", "/**/.vs/", "/**/*.iml"],
            OsJunk => &[
                "/**/.DS_Store",
                "/**/Thumbs.db",
                "/**/desktop.ini",
                "/**/$RECYCLE.BIN/",
                "/**/System Volume Information/",
            ],
            VirtualMachineImages => {
                &["/**/*.vmdk", "/**/*.vdi", "/**/*.vhdx", "/**/*.qcow2", "/**/*.iso"]
            }
            LogsAndTemp => &["/**/*.log", "/**/tmp/", "/**/temp/", "/**/*.tmp"],
            GitObjects => &["/**/.git/objects/pack/", "/**/.git/lfs/"],
        }
    }

    /// What this actually matches, in plain words.
    ///
    /// The patterns are shown too, but a glob list is not an answer to "what
    /// will this delete from my backup" for most people. A preset called
    /// "OS junk files" told the user nothing at all, and the honest response to
    /// not knowing what a checkbox does is to leave it alone — which is how a
    /// backup ends up carrying millions of cache files.
    pub fn matches_description(&self) -> &'static str {
        use ExclusionPreset::*;
        match self {
            NodeModules => "Installed npm packages, and the pnpm and Yarn download caches.",
            NextCache => {
                "Build caches written by Next.js, Turbo, Vite, Nuxt, SvelteKit, Astro, Angular, Parcel and webpack. Not your source, and not your build output."
            }
            RustTarget => "Compiled Rust artefacts under target/debug, target/release and target/tmp.",
            PythonCaches => {
                "Compiled .pyc files, __pycache__, pytest/mypy/ruff caches, and virtualenv directories named .venv or venv."
            }
            DotnetBuild => "Compiled output under bin/Debug, bin/Release and obj.",
            JavaBuild => "The Gradle cache, Gradle build temporaries, and your local Maven repository.",
            GoBuild => "The Go build cache and the downloaded module cache.",
            IdeMetadata => {
                "JetBrains shelved changes, Visual Studio's .vs folder, and .iml project files. Your editor settings and .vscode are NOT excluded."
            }
            OsJunk => {
                "macOS .DS_Store, Windows Thumbs.db and desktop.ini, the Recycle Bin, and System Volume Information."
            }
            VirtualMachineImages => "Disk images: .vmdk, .vdi, .vhdx, .qcow2 and .iso files, wherever they are.",
            LogsAndTemp => "Files ending .log or .tmp, and folders named tmp or temp.",
            GitObjects => {
                "Packed git objects under .git/objects/pack and Git LFS storage. Your working files and git history metadata are still backed up."
            }
        }
    }

    /// One line explaining the trade-off, shown in the GUI next to the toggle.
    pub fn rationale(&self) -> &'static str {
        use ExclusionPreset::*;
        match self {
            NodeModules => "Reinstallable from your lockfile. Usually the single largest win.",
            NextCache => "Regenerated on the next build. Pure churn otherwise.",
            RustTarget => "Rebuildable. Keeps repository maintenance fast.",
            PythonCaches => "Byte-code and virtualenvs are reproducible from requirements.",
            DotnetBuild => "Rebuildable from source.",
            JavaBuild => "Rebuildable; the local Maven cache mirrors Central.",
            GoBuild => "Module cache is re-downloadable.",
            IdeMetadata => "Machine-local editor state. Excludes shelf and scratch only.",
            OsJunk => "Filesystem noise that changes constantly and restores nothing.",
            VirtualMachineImages => "Huge block-level churn. Back these up deliberately instead.",
            LogsAndTemp => "Append-only churn with no restore value.",
            GitObjects => "Packfiles are recoverable from your remote — if you have one.",
        }
    }

    /// `true` when excluding this could genuinely lose work, so the GUI shows
    /// a caution marker rather than recommending it by default.
    pub fn is_risky(&self) -> bool {
        matches!(self, ExclusionPreset::GitObjects | ExclusionPreset::VirtualMachineImages)
    }
}

/// Commands run around a job.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct JobHooks {
    pub before: Option<String>,
    pub after_success: Option<String>,
    pub after_failure: Option<String>,
    pub abort_on_before_failure: bool,
}

// ---------------------------------------------------------------------------
// Scheduling
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Schedule {
    /// Only ever runs when a human or the CLI asks.
    Manual,
    /// Every N minutes, measured from the daemon's start time.
    Interval { minutes: u32 },
    /// At fixed times of day, local time.
    Daily { times: Vec<TimeOfDay> },
    /// At fixed times on selected weekdays. 0 = Monday.
    Weekly { weekdays: Vec<u8>, times: Vec<TimeOfDay> },
    /// Five-field cron, evaluated in local time.
    Cron { expression: String },
    /// Debounced filesystem watching — run once the tree goes quiet.
    OnChange { debounce_seconds: u32, min_interval_minutes: u32 },
}

impl Default for Schedule {
    fn default() -> Self {
        Schedule::Daily { times: vec![TimeOfDay { hour: 2, minute: 0 }] }
    }
}

impl Schedule {
    pub fn is_automatic(&self) -> bool {
        !matches!(self, Schedule::Manual)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TimeOfDay {
    pub hour: u8,
    pub minute: u8,
}

impl TimeOfDay {
    pub fn minutes_from_midnight(&self) -> u32 {
        self.hour as u32 * 60 + self.minute as u32
    }
}

impl std::fmt::Display for TimeOfDay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:02}:{:02}", self.hour, self.minute)
    }
}

// ---------------------------------------------------------------------------
// Remote (GitHub) configuration source
// ---------------------------------------------------------------------------

/// Pulls `config.sbvault` from a Git repository so several machines can share
/// one definition. The file in the repo is always the sealed vault: the plain
/// `config.json` is never pushed, and the vault is opened only in memory after
/// the user supplies the master passphrase.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteConfigSource {
    pub url: String,
    #[serde(default = "default_branch")]
    pub branch: String,
    /// Path to the sealed vault inside the repository.
    #[serde(default = "default_vault_path")]
    pub path: String,
    #[serde(default)]
    pub auth: RemoteAuth,
    #[serde(default)]
    pub auto_pull: bool,
    #[serde(default = "default_pull_interval")]
    pub pull_interval_minutes: u32,
    /// Never push automatically. Publishing config is always an explicit act.
    #[serde(default)]
    pub allow_push: bool,
    #[serde(default)]
    pub last_pull_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_known_commit: Option<String>,
    /// Pinned signing-key fingerprints; when non-empty, a pulled vault whose
    /// embedded signature does not verify against one of these is rejected.
    #[serde(default)]
    pub trusted_signers: Vec<String>,
}

fn default_branch() -> String {
    "main".into()
}
fn default_vault_path() -> String {
    "config.sbvault".into()
}
fn default_pull_interval() -> u32 {
    60
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RemoteAuth {
    /// Public repository, or credentials already available to the system git.
    #[default]
    None,
    /// Personal access token held in the vault.
    Token { token_ref: SecretRef },
    /// SSH key path; the key itself stays where the user put it.
    Ssh { key_path: PathBuf },
}

// ---------------------------------------------------------------------------
// On-destination layout
// ---------------------------------------------------------------------------

/// Reserved directory / key-prefix segment used inside every destination root.
pub const MANIFEST_DIR: &str = "_superbackup";

/// The layout written under every destination root. It is deliberately
/// self-describing, so that someone browsing a shared drive or an S3 bucket
/// can tell whose backup is whose without this application:
///
/// ```text
/// <destination-root>/
///   _superbackup/
///     machines/<machine-uuid>.json    <- label, host, OS, first/last seen
///     README.txt                      <- human-readable explanation
///   <machine-slug>/
///     repository/                     <- the kopia repository
///     mirror/<job-slug>/              <- plain folder mirrors
/// ```
pub fn machine_folder(slug: &str) -> String {
    slug.to_string()
}

/// Sanitise an arbitrary label into something safe on NTFS, ext4, APFS and as
/// an S3 key segment.
pub fn slugify(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut last_dash = false;
    for ch in input.chars() {
        // Everything that is not a safe filename character collapses to a
        // single dash. Dropping separators instead of collapsing them would
        // turn `../../etc/passwd` into `etcpasswd`, silently gluing unrelated
        // path segments together.
        let mapped = match ch {
            'a'..='z' | '0'..='9' => Some(ch),
            'A'..='Z' => Some(ch.to_ascii_lowercase()),
            _ => Some('-'),
        };
        match mapped {
            Some('-') => {
                if !last_dash && !out.is_empty() {
                    out.push('-');
                    last_dash = true;
                }
            }
            Some(c) => {
                out.push(c);
                last_dash = false;
            }
            None => {}
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        out.push_str("machine");
    }
    out.truncate(48);
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// Extra metadata written alongside a machine record; kept open-ended so that
/// future releases can add fields without breaking older readers.
pub type Extra = BTreeMap<String, serde_json::Value>;

#[cfg(test)]
mod tests {
    use super::*;

    fn job(name: &str) -> Job {
        Job {
            id: Uuid::new_v4(),
            name: name.into(),
            project_id: None,
            description: String::new(),
            sources: vec![],
            destination_ids: vec![],
            schedule: Schedule::Manual,
            exclusions: ExclusionSet::default(),
            bandwidth: None,
            retention: None,
            enabled: true,
            timeout_minutes: None,
            hooks: JobHooks::default(),
            continue_on_destination_error: true,
            created_at: Utc::now(),
            tags: vec![],
        }
    }

    #[test]
    fn slugify_handles_hostile_input() {
        assert_eq!(slugify("Andreas' Work PC!"), "andreas-work-pc");
        assert_eq!(slugify("   "), "machine");
        assert_eq!(slugify("../../etc/passwd"), "etc-passwd");
        assert!(slugify(&"x".repeat(200)).len() <= 48);
    }

    #[test]
    fn prefix_normalisation_strips_traversal() {
        assert_eq!(normalise_prefix("/foo/bar/"), "foo/bar/");
        assert_eq!(normalise_prefix(""), "");
        assert_eq!(normalise_prefix("   /  "), "");
        assert_eq!(normalise_prefix("a/../../b"), "a/b/");
        assert_eq!(normalise_prefix("a//b"), "a/b/");
    }

    #[test]
    fn pause_state_expires() {
        let now = Utc::now();
        let p = PauseState {
            paused: true,
            until: Some(now - chrono::Duration::hours(1)),
            reason: None,
        };
        assert!(!p.is_active(now));
        let p = PauseState { paused: true, until: None, reason: None };
        assert!(p.is_active(now));
    }

    #[test]
    fn resolve_job_rejects_ambiguous_prefix() {
        let mut cfg = Config::default();
        cfg.jobs.push(job("dev-code"));
        cfg.jobs.push(job("dev-docs"));
        assert!(cfg.resolve_job("dev").is_none(), "ambiguous prefix must not resolve");
        assert!(cfg.resolve_job("dev-code").is_some());
        let id = cfg.jobs[0].id;
        assert!(cfg.resolve_job(&id.to_string()).is_some());
    }

    #[test]
    fn destination_inherits_provider_credentials() {
        let pid = Uuid::new_v4();
        let provider = StorageProvider {
            id: pid,
            name: "StorJ".into(),
            kind: ProviderKind::S3 {
                endpoint: "https://gateway.storjshare.io".into(),
                region: "eu-1".into(),
                credentials: S3Credentials::for_provider(&pid),
                tls: true,
                path_style: false,
                flavour: S3Flavour::Storj,
                admin_url: None,
            },
            notes: String::new(),
            created_at: Utc::now(),
            last_verified_at: None,
        };
        let inherited = DestinationKind::S3 {
            provider_id: pid,
            bucket: "backups".into(),
            prefix: "superbackup/pc-1/".into(),
            credential_override: None,
        };
        assert_eq!(
            inherited.effective_credentials(Some(&provider)).unwrap().access_key_ref,
            S3Credentials::for_provider(&pid).access_key_ref
        );

        let did = Uuid::new_v4();
        let overridden = DestinationKind::S3 {
            provider_id: pid,
            bucket: "other".into(),
            prefix: String::new(),
            credential_override: Some(S3Credentials::for_destination(&did)),
        };
        assert_eq!(
            overridden.effective_credentials(Some(&provider)).unwrap().access_key_ref,
            S3Credentials::for_destination(&did).access_key_ref
        );
    }

    #[test]
    fn exclusion_presets_expand_and_dedupe() {
        let set = ExclusionSet::developer_defaults();
        let pats = set.effective_patterns();
        assert!(pats.iter().any(|p| p.contains("node_modules")));
        assert!(pats.iter().any(|p| p.contains(".next/cache")));
        let mut sorted = pats.clone();
        sorted.dedup();
        assert_eq!(sorted.len(), pats.len(), "patterns must be deduplicated");
    }

    #[test]
    fn config_round_trips_through_json() {
        let cfg = Config::default();
        let s = serde_json::to_string_pretty(&cfg).unwrap();
        let back: Config = serde_json::from_str(&s).unwrap();
        assert_eq!(back.schema_version, CONFIG_SCHEMA_VERSION);
    }
}
