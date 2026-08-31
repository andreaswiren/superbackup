//! Loading, migrating, validating and saving [`crate::model::Config`], and the
//! combined [`Store`] that pairs it with the encrypted vault.
//!
//! # Three separate jobs
//!
//! 1. **[`ConfigStore`]** owns `config.json`: read it, migrate it forward,
//!    normalise it, validate it, write it atomically.
//! 2. **[`validate`]** is the gate. Almost every rule here exists because
//!    breaking it produces a backup system that *appears* to work and silently
//!    does not — a job pointing at a deleted destination, a cron expression
//!    that never fires, a destination folder living inside its own source.
//!    Those are worse than a crash, because the user finds out when they need
//!    a restore.
//! 3. **[`Store`]** holds the configuration and the vault together, because
//!    almost every caller needs both and the interesting state is the
//!    combination: *loaded but locked* is a real, normal, long-lived state
//!    (the daemon runs in it until someone types the passphrase) and the API
//!    has to make it obvious rather than incidental.
//!
//! # Why validation is a report rather than a bool
//!
//! Errors block a save; warnings do not. A job with no destinations yet is a
//! perfectly ordinary intermediate state in a GUI wizard and must be saveable;
//! a job pointing at a destination that does not exist is a corrupt document
//! and must not be. Collapsing both into "invalid" would force the GUI to
//! either refuse legitimate edits or skip validation entirely, and it would
//! pick the second.

use crate::crypto::{
    self, BackupReason, DerivedRepository, Rekey, RekeyAcknowledgement, Vault, VaultFile,
};
use crate::error::{Error, IoContext, Result};
use crate::model::{
    self, Config, Destination, DestinationKind, Job, ProviderKind, RemoteAuth, Schedule, SecretRef,
    CONFIG_SCHEMA_VERSION,
};
use crate::paths::{self, Paths};
use crate::secret::Secret;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};
use std::str::FromStr;
use uuid::Uuid;

/// Largest `config.json` we will read (8 MiB).
///
/// The document is a few kilobytes of job definitions. Anything larger is a
/// broken file or a hostile one, and finding that out before parsing is
/// cheaper than finding it out during.
pub const MAX_CONFIG_BYTES: u64 = 8 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// One problem found in a configuration.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Issue {
    /// Dotted path to the offending field, e.g. `jobs[dev-code].sources[0]`.
    /// Written for a human reading a CLI error, not for machine parsing.
    pub location: String,
    pub message: String,
}

impl std::fmt::Display for Issue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.location, self.message)
    }
}

/// The outcome of validating a configuration.
///
/// Errors make the document unsaveable; warnings are things the user probably
/// wants to know about but which do not make the file incoherent.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct ValidationReport {
    pub errors: Vec<Issue>,
    pub warnings: Vec<Issue>,
}

impl ValidationReport {
    fn error(&mut self, location: impl Into<String>, message: impl Into<String>) {
        self.errors.push(Issue { location: location.into(), message: message.into() });
    }
    fn warn(&mut self, location: impl Into<String>, message: impl Into<String>) {
        self.warnings.push(Issue { location: location.into(), message: message.into() });
    }

    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }

    /// Collapse into a `Result`, joining every error into one message.
    ///
    /// All errors are reported at once on purpose: fixing a config one error
    /// per save round-trip is miserable, and the CLI has no way to show a list
    /// unless we produce one.
    pub fn into_result(self) -> Result<ValidationReport> {
        if self.errors.is_empty() {
            return Ok(self);
        }
        let joined = self.errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; ");
        Err(Error::Validation(format!("configuration is not valid: {joined}")))
    }
}

impl std::fmt::Display for ValidationReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for e in &self.errors {
            writeln!(f, "error: {e}")?;
        }
        for w in &self.warnings {
            writeln!(f, "warning: {w}")?;
        }
        Ok(())
    }
}

/// Rewrite the fields that have one canonical form.
///
/// Called before validation and before every save, so that a prefix typed as
/// `/backups//pc1/` and one typed as `backups/pc1` are the same configuration
/// rather than two that merely behave the same. Normalising *before*
/// validating also means the S3-prefix rule can be a hard error without
/// tripping over the user's typing.
pub fn normalise(config: &mut Config) {
    for destination in &mut config.destinations {
        if let DestinationKind::S3 { prefix, .. } = &mut destination.kind {
            *prefix = model::normalise_prefix(prefix);
        }
        destination.name = destination.name.trim().to_string();
    }
    for job in &mut config.jobs {
        job.name = job.name.trim().to_string();
        job.tags.iter_mut().for_each(|t| *t = t.trim().to_string());
    }
    for provider in &mut config.providers {
        provider.name = provider.name.trim().to_string();
    }
    for project in &mut config.projects {
        project.name = project.name.trim().to_string();
    }
}

/// Check every rule. See the module documentation for errors vs warnings.
pub fn validate(config: &Config) -> ValidationReport {
    let mut report = ValidationReport::default();

    if config.schema_version > CONFIG_SCHEMA_VERSION {
        report.error(
            "schema_version",
            format!(
                "this configuration was written by a newer version of superbackup \
                 (schema {} vs {CONFIG_SCHEMA_VERSION})",
                config.schema_version
            ),
        );
    }

    validate_identity(config, &mut report);
    validate_providers(config, &mut report);
    validate_destinations(config, &mut report);
    validate_replication(config, &mut report);
    validate_jobs(config, &mut report);
    validate_remote(config, &mut report);

    report
}

fn validate_identity(config: &Config, report: &mut ValidationReport) {
    if config.machine.slug.trim().is_empty() {
        report.error("machine.slug", "the machine slug is what keeps this PC's backups separate from every other PC's inside a shared destination; it cannot be empty");
    }
    if config.machine.label.trim().is_empty() {
        report.warn(
            "machine.label",
            "this machine has no label; it will be hard to identify in a shared destination",
        );
    }
}

/// Names are resolved case-insensitively by `Config::resolve_*`, so two names
/// differing only in case are genuinely ambiguous and must be rejected rather
/// than silently shadowing each other on the command line.
fn check_unique_names<'a>(
    kind: &str,
    items: impl Iterator<Item = (&'a Uuid, &'a String)>,
    report: &mut ValidationReport,
) {
    let mut by_name: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_id: BTreeMap<Uuid, usize> = BTreeMap::new();
    for (id, name) in items {
        if name.trim().is_empty() {
            report.error(format!("{kind}[{id}].name"), "name cannot be empty");
            continue;
        }
        *by_name.entry(name.to_lowercase()).or_insert(0) += 1;
        *by_id.entry(*id).or_insert(0) += 1;
    }
    for (name, count) in by_name.into_iter().filter(|(_, c)| *c > 1) {
        report.error(
            format!("{kind}.name"),
            format!(
                "{count} {kind} share the name {name:?}; names are matched \
                 case-insensitively on the command line, so this is ambiguous"
            ),
        );
    }
    for (id, count) in by_id.into_iter().filter(|(_, c)| *c > 1) {
        report.error(format!("{kind}[{id}]"), format!("{count} {kind} share the id {id}"));
    }
}

fn validate_providers(config: &Config, report: &mut ValidationReport) {
    check_unique_names("providers", config.providers.iter().map(|p| (&p.id, &p.name)), report);
    for provider in &config.providers {
        let ProviderKind::S3 { endpoint, .. } = &provider.kind;
        let location = format!("providers[{}].endpoint", provider.name);
        if endpoint.trim().is_empty() {
            report.error(location, "an S3 provider needs an endpoint");
        } else if endpoint.starts_with("http://") {
            report.warn(
                location,
                "this endpoint is plain HTTP; credentials and backup data will cross the network unencrypted",
            );
        }
        if config.destinations_using(&provider.id).is_empty() {
            report
                .warn(format!("providers[{}]", provider.name), "no destination uses this provider");
        }
    }
}

fn validate_destinations(config: &Config, report: &mut ValidationReport) {
    check_unique_names(
        "destinations",
        config.destinations.iter().map(|d| (&d.id, &d.name)),
        report,
    );

    for destination in &config.destinations {
        let name = &destination.name;
        match &destination.kind {
            DestinationKind::S3 { provider_id, bucket, prefix, .. } => {
                if config.provider(provider_id).is_none() {
                    report.error(
                        format!("destinations[{name}].provider_id"),
                        format!("no provider with id {provider_id}"),
                    );
                }
                if bucket.trim().is_empty() {
                    report.error(format!("destinations[{name}].bucket"), "bucket cannot be empty");
                }
                if *prefix != model::normalise_prefix(prefix) {
                    report.error(
                        format!("destinations[{name}].prefix"),
                        format!(
                            "prefix {prefix:?} is not normalised; expected {:?}",
                            model::normalise_prefix(prefix)
                        ),
                    );
                }
            }
            DestinationKind::LocalRepository { path }
            | DestinationKind::OneDrive { path, .. }
            | DestinationKind::LocalMirror { path } => {
                if path.as_os_str().is_empty() {
                    report.error(format!("destinations[{name}].path"), "path cannot be empty");
                } else if !path.is_absolute() {
                    report.error(
                        format!("destinations[{name}].path"),
                        format!(
                            "{} is a relative path; a backup destination must be absolute, \
                             because the working directory of a scheduled service is not \
                             anything the user chose",
                            path.display()
                        ),
                    );
                }
            }
        }

        // A replica has no encryption settings *by construction* — it inherits
        // the format blob, and therefore the cipher suite, of the repository it
        // is copied from. Warning about it here would be telling the user to
        // fix something the validator forbids them from setting.
        if destination.kind.is_repository()
            && destination.encryption.is_none()
            && !destination.is_replica()
        {
            report.warn(
                format!("destinations[{name}].encryption"),
                "no encryption settings; kopia defaults will be used",
            );
        }
        if !destination.kind.is_repository() && destination.passphrase_ref.is_some() {
            report.warn(
                format!("destinations[{name}].passphrase_ref"),
                "a folder mirror has no repository, so this passphrase is never used",
            );
        }
        if config.jobs_using(&destination.id).is_empty() {
            report.warn(format!("destinations[{name}]"), "no job writes to this destination");
        }
    }
}

/// Rules for chained destinations.
///
/// Every one of these exists because breaking it produces an offsite copy that
/// either cannot be written or cannot be opened — and the user finds out at
/// restore time. The two that matter most:
///
/// * **A replica cannot have its own key.** `kopia repository sync-to` copies
///   the source's `kopia.repository` format blob into an empty destination and
///   refuses a destination whose format blob has a different unique id. The
///   replica therefore *is* the source repository, opened with the source's
///   passphrase. A configuration that says otherwise is describing something
///   kopia will not do, and the belief it encodes — "my offsite copy has a
///   separate key" — is precisely the belief that gets someone hurt.
/// * **The chain must be a DAG.** A cycle would leave the runner with a set of
///   destinations none of which can go first.
fn validate_replication(config: &Config, report: &mut ValidationReport) {
    for destination in &config.destinations {
        let Some(source_id) = destination.replicate_from else { continue };
        let name = &destination.name;
        let location = format!("destinations[{name}].replicate_from");

        if source_id == destination.id {
            report.error(&location, "a destination cannot be replicated from itself");
            continue;
        }

        if !destination.kind.is_repository() {
            report.error(
                &location,
                format!(
                    "{name:?} is a folder mirror, which holds plain files rather than \
                     repository blobs; only a repository destination can be a replica"
                ),
            );
            continue;
        }

        let Some(source) = config.destination(&source_id) else {
            report.error(&location, format!("no destination with id {source_id}"));
            continue;
        };

        if !source.kind.is_repository() {
            report.error(
                &location,
                format!(
                    "{:?} is a folder mirror, so it has no repository blobs to replicate; \
                     a chain must start from a repository destination",
                    source.name
                ),
            );
            continue;
        }

        // Walk up. `replication_chain` returns an empty vector for a broken or
        // cyclic chain, which is exactly the case that needs naming here.
        let chain = config.replication_chain(destination);
        if chain.is_empty() {
            match cycle_path(config, destination) {
                Some(path) => report.error(
                    &location,
                    format!(
                        "these destinations replicate from each other in a cycle ({path}); \
                         a chain has to start somewhere that is backed up from the sources"
                    ),
                ),
                None => report.error(
                    &location,
                    format!(
                        "the replication chain above {name:?} is broken or longer than \
                         {} hops",
                        model::MAX_REPLICATION_DEPTH
                    ),
                ),
            }
            continue;
        }

        if destination.passphrase_ref.is_some() || destination.encryption.is_some() {
            let root = chain.first().map(|d| d.name.as_str()).unwrap_or("its source");
            report.error(
                format!("destinations[{name}].passphrase_ref"),
                format!(
                    "{name:?} is replicated from {root:?}, so it is the same kopia repository \
                     and opens with {root:?}'s passphrase and encryption settings — it cannot \
                     have its own. Remove them, or back this destination up from the sources \
                     instead of chaining it."
                ),
            );
        }

        // Cross-account S3 → S3 is legal but subtle: kopia takes the *source*
        // repository's credentials from its stored connection profile and the
        // *destination's* from the environment, so the two must not be
        // assumed interchangeable. Worth saying once; not worth refusing.
        if let (DestinationKind::S3 { .. }, DestinationKind::S3 { .. }) =
            (&source.kind, &destination.kind)
        {
            let source_provider = source.kind.provider_id();
            let replica_provider = destination.kind.provider_id();
            if source_provider != replica_provider {
                report.warn(
                    format!("destinations[{name}]"),
                    format!(
                        "{name:?} replicates from {:?} across two different storage providers; \
                         kopia reads the source through its stored connection profile and the \
                         destination through this destination's credentials, so both sets have \
                         to stay valid",
                        source.name
                    ),
                );
            }
        }
    }
}

/// Render the cycle `destination` sits in as `a -> b -> a`, or `None` when the
/// chain is broken rather than cyclic.
fn cycle_path(config: &Config, destination: &Destination) -> Option<String> {
    let mut names: Vec<&str> = vec![destination.name.as_str()];
    let mut ids: Vec<Uuid> = vec![destination.id];
    let mut current = destination;
    for _ in 0..=model::MAX_REPLICATION_DEPTH {
        let parent_id = current.replicate_from?;
        let parent = config.destination(&parent_id)?;
        names.push(parent.name.as_str());
        if ids.contains(&parent_id) {
            return Some(names.join(" -> "));
        }
        ids.push(parent_id);
        current = parent;
    }
    None
}

fn validate_jobs(config: &Config, report: &mut ValidationReport) {
    check_unique_names("jobs", config.jobs.iter().map(|j| (&j.id, &j.name)), report);

    for job in &config.jobs {
        let name = &job.name;

        if let Some(project_id) = &job.project_id {
            if config.project(project_id).is_none() {
                report.error(
                    format!("jobs[{name}].project_id"),
                    format!("no project with id {project_id}"),
                );
            }
        }

        if job.sources.is_empty() {
            report.error(format!("jobs[{name}].sources"), "a job with no sources backs up nothing");
        }
        for (i, source) in job.sources.iter().enumerate() {
            let location = format!("jobs[{name}].sources[{i}]");
            if source.path.as_os_str().is_empty() {
                report.error(location, "source path cannot be empty");
                continue;
            }
            if !source.path.is_absolute() {
                report.error(
                    location,
                    format!(
                        "{} is a relative path; sources must be absolute, because a \
                         scheduled run has no meaningful working directory",
                        source.path.display()
                    ),
                );
            }
        }

        if job.destination_ids.is_empty() {
            report.warn(
                format!("jobs[{name}].destination_ids"),
                "this job has no destination and will never write anything",
            );
        }
        let mut seen = BTreeSet::new();
        for destination_id in &job.destination_ids {
            if !seen.insert(*destination_id) {
                report.error(
                    format!("jobs[{name}].destination_ids"),
                    format!("destination {destination_id} is listed twice"),
                );
            }
            if config.destination(destination_id).is_none() {
                report.error(
                    format!("jobs[{name}].destination_ids"),
                    format!("no destination with id {destination_id}"),
                );
            }
        }

        validate_replication_within_job(config, job, report);
        validate_schedule(job, report);
        validate_no_self_nesting(config, job, report);
    }
}

fn validate_schedule(job: &Job, report: &mut ValidationReport) {
    let location = format!("jobs[{}].schedule", job.name);
    match &job.schedule {
        Schedule::Manual => {}
        Schedule::Interval { minutes } => {
            if *minutes == 0 {
                report.error(location, "an interval of zero minutes would run continuously");
            }
        }
        Schedule::Daily { times } => {
            if times.is_empty() {
                report.error(location.clone(), "a daily schedule with no times never runs");
            }
            check_times(times, &location, report);
        }
        Schedule::Weekly { weekdays, times } => {
            if times.is_empty() {
                report.error(location.clone(), "a weekly schedule with no times never runs");
            }
            if weekdays.is_empty() {
                report.error(location.clone(), "a weekly schedule with no weekdays never runs");
            }
            for day in weekdays {
                if *day > 6 {
                    report.error(
                        location.clone(),
                        format!("weekday {day} is out of range (0 = Monday .. 6 = Sunday)"),
                    );
                }
            }
            check_times(times, &location, report);
        }
        Schedule::Cron { expression } => {
            // Parsed with the same library the scheduler will use, so a config
            // that saves is a config that runs. Accepting an expression here
            // and failing at 03:00 is the failure mode this rule exists to
            // prevent.
            if let Err(e) = croner::Cron::from_str(expression) {
                report.error(location, format!("cron expression {expression:?} is invalid: {e}"));
            }
        }
        Schedule::OnChange { debounce_seconds, min_interval_minutes } => {
            if *debounce_seconds == 0 {
                report.error(
                    location.clone(),
                    "a zero debounce would start a run on every single file write",
                );
            }
            if *min_interval_minutes == 0 {
                report.warn(
                    location,
                    "no minimum interval; a busy source tree can queue back-to-back runs",
                );
            }
        }
    }
}

fn check_times(times: &[model::TimeOfDay], location: &str, report: &mut ValidationReport) {
    for time in times {
        if time.hour > 23 || time.minute > 59 {
            report.error(location.to_string(), format!("{time} is not a valid time of day"));
        }
    }
}

/// Reject a destination that lives inside one of its own job's sources.
///
/// This is the infinite-growth footgun: the job copies the source into the
/// destination, which is inside the source, so the next run copies the copy,
/// and so on until the disk fills. It is not hypothetical — "back up
/// `C:\Users\me` to `C:\Users\me\Backups`" is the most natural thing in the
/// world to type. Equality is rejected for the same reason.
///
/// Only local paths participate: an S3 prefix cannot contain a filesystem
/// source.
/// A job that writes to a replica must also write to what the replica is
/// copied from.
///
/// The replication step reads the source repository *after this run has
/// updated it*; that ordering is the entire benefit of chaining. If the source
/// is not part of the same run, the offsite copy would replicate whatever
/// happened to be there — quite possibly a week old — while reporting a fresh
/// success. Requiring both in one job is what makes "the offsite copy is as
/// new as the local one" true rather than merely likely.
fn validate_replication_within_job(config: &Config, job: &Job, report: &mut ValidationReport) {
    for destination_id in &job.destination_ids {
        let Some(destination) = config.destination(destination_id) else { continue };
        let Some(source_id) = destination.replicate_from else { continue };
        if job.destination_ids.contains(&source_id) {
            continue;
        }
        let source_name = config
            .destination(&source_id)
            .map(|d| format!("{:?}", d.name))
            .unwrap_or_else(|| source_id.to_string());
        report.error(
            format!("jobs[{}].destination_ids", job.name),
            format!(
                "{:?} is replicated from {source_name}, which this job does not back up; \
                 add it to the job, or the offsite copy would be made from whatever this \
                 run did not update",
                destination.name
            ),
        );
    }
}

fn validate_no_self_nesting(config: &Config, job: &Job, report: &mut ValidationReport) {
    for destination_id in &job.destination_ids {
        let Some(destination) = config.destination(destination_id) else { continue };
        let Some(destination_path) = destination.kind.local_path() else { continue };
        for source in &job.sources {
            if path_within(destination_path, &source.path) {
                report.error(
                    format!("jobs[{}].destination_ids", job.name),
                    format!(
                        "destination {:?} ({}) is inside this job's own source {}; \
                         each run would back up the previous run's output and grow without bound",
                        destination.name,
                        destination_path.display(),
                        source.path.display()
                    ),
                );
            }
        }
    }
}

/// True when `child` is `ancestor` or lives inside it.
///
/// Compares path components rather than string prefixes, so `/data/backups` is
/// not treated as living inside `/data/back`. Comparison is case-insensitive
/// on Windows, where `C:\Users` and `c:\users` are the same directory.
/// Deliberately does *not* canonicalise: these paths frequently do not exist
/// yet (the destination is created on first run), and a validation rule that
/// only works for existing paths would not fire in the case that matters —
/// the user setting the job up for the first time.
fn path_within(child: &Path, ancestor: &Path) -> bool {
    let normalise = |c: Component<'_>| -> String {
        let s = c.as_os_str().to_string_lossy().into_owned();
        if cfg!(windows) {
            s.to_lowercase()
        } else {
            s
        }
    };
    let ancestor: Vec<String> = ancestor.components().map(normalise).collect();
    if ancestor.is_empty() {
        return false;
    }
    let child: Vec<String> = child.components().map(normalise).collect();
    child.len() >= ancestor.len() && child[..ancestor.len()] == ancestor[..]
}

fn validate_remote(config: &Config, report: &mut ValidationReport) {
    let Some(remote) = &config.remote else { return };
    if remote.url.trim().is_empty() {
        report.error("remote.url", "a remote config source needs a URL");
    } else if !remote.url.starts_with("https://") {
        // The vault is encrypted, so plain HTTP does not leak its contents —
        // but it does let anyone on the path substitute a different vault, and
        // an access token sent to fetch a private repository would be in clear
        // text. There is no reason to allow it.
        report.error(
            "remote.url",
            format!("{:?} is not an https:// URL; remote config must use TLS", remote.url),
        );
    }
    if remote.path.trim().is_empty() {
        report.error("remote.path", "the path to the vault inside the repository cannot be empty");
    }
    if remote.auto_pull && remote.pull_interval_minutes == 0 {
        report.error(
            "remote.pull_interval_minutes",
            "an automatic pull interval of zero would poll continuously",
        );
    }
    if let RemoteAuth::Token { token_ref } = &remote.auth {
        if token_ref.as_str().trim().is_empty() {
            report.error("remote.auth.token_ref", "token handle cannot be empty");
        }
    }
    if remote.trusted_signers.is_empty() {
        report.warn(
            "remote.trusted_signers",
            "no signers are pinned, so any vault served at this URL that opens with your \
             passphrase will be accepted",
        );
    }
    if remote.allow_push {
        report.warn("remote.allow_push", "publishing is enabled for this remote");
    }
}

// ---------------------------------------------------------------------------
// Migration
// ---------------------------------------------------------------------------

/// Bring a raw configuration document up to [`CONFIG_SCHEMA_VERSION`].
///
/// Operates on `serde_json::Value` rather than on `Config`, because the whole
/// point is to handle documents that the *current* `Config` cannot represent.
/// Deserialising first and migrating afterwards would mean serde had already
/// thrown away, defaulted, or rejected exactly the fields the migration exists
/// to fix.
///
/// # Errors
///
/// [`Error::Config`] for a document newer than this build understands. Refusing
/// is the only safe option: opening it would mean silently dropping the fields
/// we do not know about and then writing the truncated version back over the
/// user's file.
pub fn migrate(mut value: Value) -> Result<(Value, Vec<String>)> {
    let mut notes = Vec::new();

    if !value.is_object() {
        return Err(Error::Config("config.json does not contain a JSON object".into()));
    }

    // An absent `schema_version` means "written before versioning existed".
    // `Config` has a serde default for the field, so this distinction is only
    // visible here, on the raw document — which is precisely why migration
    // runs on the raw document.
    let mut version = match value.get("schema_version") {
        None => 0,
        Some(Value::Number(n)) => n
            .as_u64()
            .ok_or_else(|| Error::Config("schema_version is not a non-negative integer".into()))?
            as u32,
        Some(_) => return Err(Error::Config("schema_version is not a number".into())),
    };

    if version > CONFIG_SCHEMA_VERSION {
        return Err(Error::Config(format!(
            "config.json was written by a newer version of superbackup (schema version \
             {version}; this build understands up to {CONFIG_SCHEMA_VERSION}). Upgrade \
             superbackup, or restore an older config.json; it has NOT been modified."
        )));
    }

    while version < CONFIG_SCHEMA_VERSION {
        match version {
            0 => {
                migrate_0_to_1(&mut value, &mut notes)?;
                version = 1;
            }
            1 => {
                migrate_1_to_2(&mut value)?;
                version = 2;
            }
            other => {
                return Err(Error::Config(format!(
                    "no migration is defined from schema version {other}"
                )))
            }
        }
    }

    if let Some(object) = value.as_object_mut() {
        object.insert("schema_version".into(), Value::from(CONFIG_SCHEMA_VERSION));
    }
    Ok((value, notes))
}

/// Pre-versioning documents (development builds before 0.1) to schema 1.
///
/// Two real problems to fix:
///
/// * `machine` became a required field with no serde default, so an old
///   document without one fails to parse at all. A fresh identity is minted,
///   which is correct: an old document that never had a machine identity was
///   never part of a multi-machine destination.
/// * S3 prefixes were stored exactly as typed, including leading slashes and
///   `..` segments. They are run through [`model::normalise_prefix`] so the
///   normalisation rule in [`validate`] can be a hard error.
fn migrate_0_to_1(value: &mut Value, notes: &mut Vec<String>) -> Result<()> {
    let object = value
        .as_object_mut()
        .ok_or_else(|| Error::Config("config.json does not contain a JSON object".into()))?;

    for key in ["providers", "destinations", "projects", "jobs"] {
        let entry = object.entry(key.to_string()).or_insert_with(|| Value::Array(Vec::new()));
        if !entry.is_array() {
            return Err(Error::Config(format!("`{key}` must be an array")));
        }
    }

    if !object.contains_key("machine") {
        let identity = serde_json::to_value(model::MachineIdentity::default())
            .map_err(|e| Error::Config(format!("could not build a machine identity: {e}")))?;
        object.insert("machine".into(), identity);
        notes
            .push("this configuration predates machine identities; a new one was generated".into());
    }

    let mut normalised = 0usize;
    if let Some(Value::Array(destinations)) = object.get_mut("destinations") {
        for destination in destinations.iter_mut() {
            let Some(kind) = destination.get_mut("kind").and_then(|k| k.as_object_mut()) else {
                continue;
            };
            if kind.get("type").and_then(|t| t.as_str()) != Some("s3") {
                continue;
            }
            let raw = kind.get("prefix").and_then(|p| p.as_str()).unwrap_or("").to_string();
            let clean = model::normalise_prefix(&raw);
            if clean != raw {
                normalised += 1;
            }
            kind.insert("prefix".into(), Value::String(clean));
        }
    }
    if normalised > 0 {
        notes.push(format!("{normalised} S3 prefix(es) were normalised"));
    }

    Ok(())
}

/// Schema 1 to 2: chained destinations.
///
/// Structurally there is nothing to repair — every schema-1 destination is
/// written from the job's sources, which is exactly what an absent
/// `replicate_from` means. The key is written out explicitly anyway so the
/// saved document states the answer rather than implying it, and no note is
/// produced because nothing happened that a user needs to be told about.
///
/// The version bump itself is the point: see [`CONFIG_SCHEMA_VERSION`].
fn migrate_1_to_2(value: &mut Value) -> Result<()> {
    let object = value
        .as_object_mut()
        .ok_or_else(|| Error::Config("config.json does not contain a JSON object".into()))?;
    let Some(Value::Array(destinations)) = object.get_mut("destinations") else {
        return Ok(());
    };
    for destination in destinations.iter_mut() {
        let Some(fields) = destination.as_object_mut() else { continue };
        fields.entry("replicate_from".to_string()).or_insert(Value::Null);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// ConfigStore
// ---------------------------------------------------------------------------

/// What happened during a load, so the caller can tell the user.
#[derive(Debug, Clone, Default)]
pub struct LoadOutcome {
    /// Schema version found on disk, before migration.
    pub found_version: u32,
    /// True when the document had to be migrated forward.
    pub migrated: bool,
    /// Human-readable notes from the migration steps.
    pub notes: Vec<String>,
    /// Warnings from validation. Errors are returned as an `Err` instead.
    pub warnings: Vec<Issue>,
}

/// Reads and writes `config.json`.
#[derive(Debug, Clone)]
pub struct ConfigStore {
    paths: Paths,
}

impl ConfigStore {
    pub fn new(paths: Paths) -> ConfigStore {
        ConfigStore { paths }
    }

    pub fn path(&self) -> std::path::PathBuf {
        self.paths.config_file()
    }

    pub fn exists(&self) -> bool {
        self.path().is_file()
    }

    /// Load, migrate and validate.
    ///
    /// A missing file yields [`Config::default`] and an empty outcome, because
    /// "first run" is not an error. A file that exists but cannot be parsed
    /// *is* an error and is never silently replaced with defaults — that would
    /// turn one bad character into a wiped configuration.
    pub fn load(&self) -> Result<(Config, LoadOutcome)> {
        let (config, outcome, report) = self.load_lenient()?;
        report.into_result()?;
        Ok((config, outcome))
    }

    /// [`ConfigStore::load`] without failing on validation errors.
    ///
    /// The repair path: a configuration with a dangling destination id is
    /// unrunnable, but the user still has to be able to open the editor and
    /// fix it. A strict-only loader would leave them with a file they can
    /// neither run nor edit, and their only remaining move would be to delete
    /// it — which is the outcome this whole module exists to prevent.
    ///
    /// Everything that makes the document *unreadable* — bad JSON, a schema
    /// from the future, a size bomb — is still an `Err`, because there is no
    /// document to repair in those cases.
    pub fn load_lenient(&self) -> Result<(Config, LoadOutcome, ValidationReport)> {
        let path = self.path();
        if !path.is_file() {
            return Ok((Config::default(), LoadOutcome::default(), ValidationReport::default()));
        }

        let metadata =
            std::fs::metadata(&path).ctx(format!("reading metadata of {}", path.display()))?;
        if metadata.len() > MAX_CONFIG_BYTES {
            return Err(Error::Config(format!(
                "{} is {} bytes; the maximum is {MAX_CONFIG_BYTES}",
                path.display(),
                metadata.len()
            )));
        }

        let bytes = std::fs::read(&path).ctx(format!("reading {}", path.display()))?;
        let raw: Value = serde_json::from_slice(&bytes)
            .map_err(|e| Error::Config(format!("{} is not valid JSON: {e}", path.display())))?;

        let found_version = raw.get("schema_version").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let (migrated_value, notes) = migrate(raw)?;

        let mut config: Config = serde_json::from_value(migrated_value).map_err(|e| {
            Error::Config(format!("{} is not a valid configuration: {e}", path.display()))
        })?;
        normalise(&mut config);

        let report = validate(&config);
        let outcome = LoadOutcome {
            found_version,
            migrated: found_version != CONFIG_SCHEMA_VERSION,
            notes,
            warnings: report.warnings.clone(),
        };
        Ok((config, outcome, report))
    }

    /// Normalise, validate and write atomically.
    ///
    /// Takes `&Config` and writes a normalised copy, so a caller cannot end up
    /// with an in-memory document that differs from the one on disk without
    /// noticing. Use [`ConfigStore::save_mut`] when you want the caller's copy
    /// normalised too.
    pub fn save(&self, config: &Config) -> Result<ValidationReport> {
        let mut copy = config.clone();
        self.save_mut(&mut copy)
    }

    /// [`ConfigStore::save`], normalising the caller's document in place.
    pub fn save_mut(&self, config: &mut Config) -> Result<ValidationReport> {
        normalise(config);
        config.schema_version = CONFIG_SCHEMA_VERSION;
        config.updated_at = Some(chrono::Utc::now());
        let report = validate(config).into_result()?;

        let bytes = serde_json::to_vec_pretty(config)
            .map_err(|e| Error::Config(format!("configuration could not be serialised: {e}")))?;
        let path = self.path();
        self.paths.ensure()?;
        paths::write_atomic(&path, &bytes)?;
        paths::harden_file(&path)?;
        Ok(report)
    }
}

// ---------------------------------------------------------------------------
// Orphan collection
// ---------------------------------------------------------------------------

/// What a vault garbage collection would remove.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct GcReport {
    /// Handles that no live object references and that are safe to delete.
    pub orphans: Vec<SecretRef>,
    /// Handles left alone because nothing proves they are ours to delete.
    pub unrecognised: Vec<SecretRef>,
    /// Handles still in use.
    pub live: usize,
}

impl GcReport {
    pub fn is_empty(&self) -> bool {
        self.orphans.is_empty()
    }
}

/// Every secret handle the configuration currently refers to.
fn live_refs(config: &Config) -> BTreeSet<SecretRef> {
    let mut live = BTreeSet::new();
    for provider in &config.providers {
        live.extend(provider.secret_refs().into_iter().cloned());
    }
    for destination in &config.destinations {
        live.extend(destination.secret_refs().into_iter().cloned());
    }
    if let Some(remote) = &config.remote {
        if let RemoteAuth::Token { token_ref } = &remote.auth {
            live.insert(token_ref.clone());
        }
    }
    live
}

/// Every UUID that names something still alive in the configuration.
fn live_ids(config: &Config) -> BTreeSet<Uuid> {
    let mut ids = BTreeSet::new();
    ids.insert(config.machine.id);
    ids.extend(config.providers.iter().map(|p| p.id));
    ids.extend(config.destinations.iter().map(|d| d.id));
    ids.extend(config.jobs.iter().map(|j| j.id));
    ids.extend(config.projects.iter().map(|p| p.id));
    ids
}

/// Work out which vault entries no longer belong to anything.
///
/// # Why this is conservative
///
/// Deleting a secret is unrecoverable, and this vault is shared between
/// machines: a handle that looks orphaned here may belong to a destination
/// that only exists in the *other* machine's configuration, which has not been
/// pulled yet. So an entry is only ever a deletion candidate when **both**
/// conditions hold:
///
/// 1. nothing in the current configuration references it, and
/// 2. its handle has this codebase's `kind:uuid` shape and that UUID names
///    nothing that currently exists.
///
/// Anything else — a handle written by a future version, a hand-added entry, a
/// handle whose UUID still names a live object — is reported as
/// `unrecognised` and left strictly alone. Being wrong in that direction costs
/// a few hundred bytes; being wrong in the other direction costs a repository.
pub fn plan_gc(config: &Config, vault: &Vault) -> Result<GcReport> {
    let stored = vault.list_refs()?;
    let live = live_refs(config);
    let ids = live_ids(config);

    let mut report = GcReport::default();
    for handle in stored {
        if live.contains(&handle) {
            report.live += 1;
            continue;
        }
        match handle.as_str().rsplit_once(':').map(|(_, id)| Uuid::parse_str(id)) {
            Some(Ok(owner)) if !ids.contains(&owner) => report.orphans.push(handle),
            _ => report.unrecognised.push(handle),
        }
    }
    Ok(report)
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

/// The configuration and the vault, together.
///
/// The vault may be locked; the configuration is always loaded. That asymmetry
/// is the normal steady state of the daemon — it knows what the jobs are and
/// when they should run long before anyone types a passphrase — so it is
/// modelled directly rather than hidden behind an `Option`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreState {
    /// The jobs, destinations and schedules are known; no secret is readable.
    /// Scheduling, health reporting and the whole GUI work in this state.
    ConfigOnly,
    /// The vault is open: secrets resolve and backups can actually run.
    Unlocked,
}

impl StoreState {
    /// Whether a backup can actually be started right now.
    pub fn can_run_jobs(&self) -> bool {
        matches!(self, StoreState::Unlocked)
    }

    pub fn title(&self) -> &'static str {
        match self {
            StoreState::ConfigOnly => "Locked",
            StoreState::Unlocked => "Unlocked",
        }
    }
}

#[derive(Debug)]
pub struct Store {
    config_store: ConfigStore,
    config: Config,
    vault: VaultFile,
    outcome: LoadOutcome,
}

impl Store {
    /// Open an existing installation. The vault is left locked.
    ///
    /// # Errors
    ///
    /// [`Error::Config`] when there is no vault yet; use [`Store::initialise`]
    /// for a first run. Failing loudly here rather than creating an empty
    /// vault matters: an accidental "create" over a vault that is merely
    /// temporarily unreadable (a network drive that has not mounted, a
    /// half-finished restore) would destroy every key on the machine.
    pub fn open(paths: Paths) -> Result<Store> {
        let (store, report) = Store::open_for_repair(paths)?;
        report.into_result()?;
        Ok(store)
    }

    /// [`Store::open`] that tolerates a configuration which does not validate,
    /// returning the problems alongside the store.
    ///
    /// The editor uses this. A configuration with a dangling destination id
    /// cannot be *run*, but the user still has to be able to open the app and
    /// fix it — and if the only way in is a loader that refuses, their next
    /// move is to delete the file. The scheduler and the run path use
    /// [`Store::open`] instead, so nothing acts on a configuration that did
    /// not validate.
    pub fn open_for_repair(paths: Paths) -> Result<(Store, ValidationReport)> {
        if !VaultFile::exists(&paths) {
            return Err(Error::Config(format!(
                "no vault at {}; run `superbackup init` to create one",
                paths.vault_file().display()
            )));
        }
        let config_store = ConfigStore::new(paths.clone());
        let (config, outcome, report) = config_store.load_lenient()?;
        let vault = VaultFile::load(&paths)?;
        Ok((Store { config_store, config, vault, outcome }, report))
    }

    /// First run: create the vault, write a default configuration, and return
    /// an unlocked store.
    pub fn initialise(paths: Paths, passphrase: &Secret) -> Result<Store> {
        Self::initialise_with(paths, Vault::create(passphrase)?)
    }

    /// [`Store::initialise`] with a pre-built vault, so the setup wizard can
    /// supply calibrated KDF parameters.
    pub fn initialise_with(paths: Paths, vault: Vault) -> Result<Store> {
        paths.ensure()?;
        let vault = VaultFile::create_from(&paths, vault)?;
        let config_store = ConfigStore::new(paths.clone());
        let mut config = Config::default();
        config_store.save_mut(&mut config)?;
        Ok(Store { config_store, config, vault, outcome: LoadOutcome::default() })
    }

    /// What the load reported: migrations applied, warnings raised.
    pub fn load_outcome(&self) -> &LoadOutcome {
        &self.outcome
    }

    pub fn paths(&self) -> &Paths {
        &self.config_store.paths
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Replace the configuration, validating and saving in one step.
    ///
    /// There is deliberately no `config_mut()`: an in-memory mutation that is
    /// never validated and never saved is how a GUI ends up believing it has
    /// applied a change it has not. Callers clone, edit and hand it back.
    pub fn set_config(&mut self, mut config: Config) -> Result<ValidationReport> {
        let report = self.config_store.save_mut(&mut config)?;
        self.config = config;
        Ok(report)
    }

    /// Re-read `config.json` from disk, discarding the in-memory copy.
    pub fn reload_config(&mut self) -> Result<()> {
        let (config, outcome) = self.config_store.load()?;
        self.config = config;
        self.outcome = outcome;
        Ok(())
    }

    /// The combined state, as an explicit enum.
    ///
    /// "Configuration loaded, vault locked" is not a transient hiccup — it is
    /// how the daemon spends most of its life, and every caller has to branch
    /// on it. Naming it makes the branch obvious instead of leaving it to a
    /// bare boolean that reads the same whichever way round it is.
    pub fn state(&self) -> StoreState {
        if self.vault.vault().is_locked() {
            StoreState::ConfigOnly
        } else {
            StoreState::Unlocked
        }
    }

    pub fn is_locked(&self) -> bool {
        self.vault.vault().is_locked()
    }

    pub fn unlock(&mut self, passphrase: &Secret) -> Result<()> {
        self.vault.vault_mut().unlock_in_place(passphrase)
    }

    pub fn lock(&mut self) {
        self.vault.vault_mut().lock();
    }

    pub fn vault(&self) -> &Vault {
        self.vault.vault()
    }

    pub fn vault_file(&self) -> &VaultFile {
        &self.vault
    }

    pub fn vault_file_mut(&mut self) -> &mut VaultFile {
        &mut self.vault
    }

    /// Resolve a handle against the unlocked vault.
    pub fn secret(&self, handle: &SecretRef) -> Result<Option<Secret>> {
        self.vault.vault().get(handle)
    }

    /// Resolve a handle, treating "missing" as an error.
    ///
    /// Used on the run path, where a missing repository passphrase is a
    /// configuration fault and not something to paper over with a default.
    pub fn require_secret(&self, handle: &SecretRef) -> Result<Secret> {
        self.secret(handle)?.ok_or_else(|| {
            Error::Config(format!(
                "the vault has no entry for {handle}; the configuration refers to a secret \
                 that was never stored, or that a garbage collection removed"
            ))
        })
    }

    /// Store a secret and persist the vault.
    pub fn put_secret(&mut self, handle: SecretRef, value: Secret) -> Result<()> {
        self.vault.vault_mut().put(handle, value)?;
        self.vault.save()
    }

    /// Every destination whose repository password is derived from the master
    /// key, and which a passphrase rotation would therefore invalidate.
    ///
    /// The GUI must show this list *before* the user commits to a rotation.
    /// It is cheap and needs no unlocked vault, so there is no excuse for a
    /// confirmation dialog that does not include it.
    pub fn derived_repositories(&self) -> Vec<DerivedRepository> {
        crypto::derived_repositories(&self.config)
    }

    /// Rotate the master passphrase — only when nothing has to be migrated.
    ///
    /// # Errors
    ///
    /// [`Error::Validation`], naming the destinations, when any of them derive
    /// their repository password from the master key. This is the loud failure
    /// at the API boundary that the hazard deserves: rotating without
    /// migrating would leave those repositories permanently unopenable, and
    /// the user would find out at the next scheduled run. Use
    /// [`Store::change_passphrase_migrating`] and act on the returned plan.
    pub fn change_passphrase(&mut self, old: &Secret, new: &Secret) -> Result<Rekey> {
        let derived = self.derived_repositories();
        if !derived.is_empty() {
            let names: Vec<&str> = derived.iter().map(|r| r.destination_name.as_str()).collect();
            return Err(Error::Validation(format!(
                "{} repositor{} derive their password from the master passphrase and would \
                 become unopenable if it changed without them ({}). Use \
                 `change_passphrase_migrating` and re-password each one with \
                 `kopia repository change-password`.",
                names.len(),
                if names.len() == 1 { "y" } else { "ies" },
                names.join(", ")
            )));
        }
        self.vault.change_passphrase(old, new, &RekeyAcknowledgement::NoDerivedRepositories)
    }

    /// Rotate the master passphrase and take responsibility for migrating the
    /// repositories it invalidates.
    ///
    /// The acknowledgement is built from this store's own configuration, so it
    /// cannot omit a destination. On return the new vault is already on disk
    /// and the listed repositories are still on their **old** password: the
    /// caller must now walk [`Rekey::repositories`], call
    /// [`Rekey::credentials`] for each, run `kopia repository change-password`,
    /// and record the outcome with [`Rekey::mark_migrated`] or
    /// [`Rekey::mark_failed`].
    ///
    /// If that walk is interrupted, [`Store::resume_rekey`] picks it up. See
    /// [`crate::crypto::rekey`] for why the vault is committed first.
    pub fn change_passphrase_migrating(&mut self, old: &Secret, new: &Secret) -> Result<Rekey> {
        let ack = RekeyAcknowledgement::for_config(&self.config);
        self.vault.change_passphrase(old, new, &ack)
    }

    /// Rebuild the migration plan for an interrupted rotation.
    ///
    /// `backup` is [`crate::crypto::MigrationReport::recovery_backup`] from the
    /// interrupted run, or [`VaultFile::latest_rekey_backup`] when that report
    /// did not survive the crash. Every repository comes back as pending;
    /// re-running a completed one is safe because
    /// [`crate::crypto::RepositoryCredentials`] carries both passwords.
    pub fn resume_rekey(&self, backup: &Path, old: &Secret, new: &Secret) -> Result<Rekey> {
        self.vault.resume_rekey(backup, old, new, &self.derived_repositories())
    }

    /// What [`Store::collect_garbage`] would delete. Never mutates anything.
    pub fn gc_dry_run(&self) -> Result<GcReport> {
        plan_gc(&self.config, self.vault.vault())
    }

    /// Delete orphaned vault entries.
    ///
    /// Takes a backup first, because "the GC was wrong" must be recoverable.
    /// The vault must be unlocked, and nothing is written when there is
    /// nothing to remove.
    pub fn collect_garbage(&mut self) -> Result<GcReport> {
        let report = self.gc_dry_run()?;
        if report.is_empty() {
            return Ok(report);
        }
        self.vault.backup(BackupReason::Manual)?;
        for handle in &report.orphans {
            self.vault.vault_mut().remove(handle)?;
        }
        self.vault.save()?;
        Ok(report)
    }

    /// Copy the current configuration into the vault so that sealing it
    /// publishes both. Explicit, because publishing is a decision.
    pub fn stage_for_publication(&mut self) -> Result<()> {
        let config = self.config.clone();
        self.vault.vault_mut().set_embedded_config(Some(config))?;
        self.vault.save()
    }

    /// The bytes to hand to [`crate::remote::PushRequest`].
    ///
    /// Stages the configuration first, so what is published is always the
    /// configuration the user is looking at rather than whatever happened to
    /// be embedded the last time somebody remembered to stage it.
    pub fn publication_payload(&mut self) -> Result<Vec<u8>> {
        self.stage_for_publication()?;
        Ok(self.vault.vault().sealed_bytes().to_vec())
    }
}

/// Convenience for the destinations that derive their passphrase from the
/// master key rather than storing one.
///
/// Kept here rather than in `crypto` because deciding *which* destinations do
/// that is a configuration question: it reads
/// [`model::PassphraseSource`] and dispatches accordingly.
pub fn destination_passphrase(store: &Store, destination: &Destination) -> Result<Secret> {
    use model::PassphraseSource::*;
    // A replica is the *same kopia repository* as the destination it is
    // synchronised from — `sync-to` copies the format blob — so it opens with
    // that repository's passphrase and never with one of its own. Resolving
    // the chain here rather than at every call site is what keeps the
    // guarantee in one place: connect, verify, restore and browse all arrive
    // through this function.
    let destination = match store.config().replication_root(destination) {
        Some(root) => root,
        None if destination.is_replica() => {
            return Err(Error::Config(format!(
                "destination {:?} is replicated from a destination that no longer exists, \
                 or from a chain that loops; its passphrase cannot be resolved",
                destination.name
            )))
        }
        None => destination,
    };
    let source = destination.encryption.as_ref().map(|e| e.passphrase_source).unwrap_or(Generated);
    match source {
        DerivedFromMaster => store.vault().derive_repo_passphrase(&destination.id),
        Generated | UserSupplied => {
            let handle = destination.passphrase_ref.clone().ok_or_else(|| {
                Error::Config(format!(
                    "destination {:?} has no passphrase handle, but its passphrase is not \
                     derived from the master key",
                    destination.name
                ))
            })?;
            store.require_secret(&handle)
        }
    }
}
