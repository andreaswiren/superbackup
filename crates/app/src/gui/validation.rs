//! Form validation, the wizard's step gating, and the passphrase meter.
//!
//! `UX_SPEC.md` §17 is the source of every rule here. Validation is inline, on
//! blur, and never blocks typing; a form with problems disables its Save button
//! and the button's tooltip names the count.

// The interface is a library-shaped tree inside a binary crate. Its components,
// view models and fixtures are also compiled by `crates/app/tests/gui_app.rs`
// as a separate crate, so items that are used and tested there look unused from
// the binary's side. The allow is scoped to this module rather than the crate.
#![allow(dead_code)]
use std::path::Path;

use superbackup_core::model::{
    Destination, DestinationKind, Job, ProviderKind, RetentionPolicy, S3Flavour, Schedule,
    StorageProvider,
};
use uuid::Uuid;

use super::copy;

/// Which control an error belongs to, so the screen can put the message under
/// the right field rather than in a summary nobody reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Field {
    Name,
    Description,
    Sources,
    Destinations,
    Schedule,
    ScheduleTimes,
    ScheduleWeekdays,
    Cron,
    Debounce,
    MinInterval,
    Timeout,
    Patterns,
    MaxFileSize,
    Retention,
    Maintenance,
    Path,
    Bucket,
    Prefix,
    Provider,
    Endpoint,
    Region,
    Credentials,
    Passphrase,
    PassphraseConfirm,
    Bandwidth,
    BandwidthWindow,
    RemoteUrl,
    Signer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub field: Field,
    pub message: String,
}

/// Blocking problems and non-blocking cross-field warnings, kept apart because
/// the spec treats them differently: one disables Save, the other does not.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub problems: Vec<Problem>,
    pub warnings: Vec<String>,
}

impl Report {
    pub fn ok(&self) -> bool {
        self.problems.is_empty()
    }
    pub fn count(&self) -> usize {
        self.problems.len()
    }
    pub fn for_field(&self, field: Field) -> Option<&str> {
        self.problems.iter().find(|p| p.field == field).map(|p| p.message.as_str())
    }
    pub fn summary(&self) -> Option<String> {
        if self.problems.is_empty() {
            None
        } else {
            Some(copy::valid_form_problems(self.problems.len()))
        }
    }
    fn push(&mut self, field: Field, message: impl Into<String>) {
        self.problems.push(Problem { field, message: message.into() });
    }
}

// ---------------------------------------------------------------------------
// Jobs
// ---------------------------------------------------------------------------

/// Validate a job draft against the rest of the configuration.
pub fn validate_job(
    job: &Job,
    others: &[Job],
    destinations: &[Destination],
) -> Report {
    let mut report = Report::default();

    let name = job.name.trim();
    if name.is_empty() {
        report.push(Field::Name, copy::valid::JOB_NAME_EMPTY);
    } else if name.chars().count() > 64 {
        report.push(Field::Name, copy::valid::JOB_NAME_LONG);
    } else if others
        .iter()
        .any(|o| o.id != job.id && o.name.trim().eq_ignore_ascii_case(name))
    {
        report.push(Field::Name, copy::valid_job_name_dup(name));
    }

    if job.sources.is_empty() {
        report.push(Field::Sources, copy::valid::SOURCE_NONE);
    }
    for (i, source) in job.sources.iter().enumerate() {
        if !source.path.is_absolute() {
            report.push(Field::Sources, copy::valid::SOURCE_RELATIVE);
            break;
        }
        for (j, other) in job.sources.iter().enumerate() {
            if i == j {
                continue;
            }
            if source.path == other.path && i > j {
                report.push(Field::Sources, copy::valid::SOURCE_DUP);
                break;
            }
            if source.path.starts_with(&other.path) && source.path != other.path {
                report.push(
                    Field::Sources,
                    copy::valid_source_nested(&other.path.to_string_lossy()),
                );
                break;
            }
        }
        // A backup that contains its own destination grows without bound.
        for destination in destinations {
            if let Some(root) = destination.kind.local_path() {
                if source.path.starts_with(root) {
                    report.push(
                        Field::Sources,
                        copy::valid_source_in_destination(&destination.name),
                    );
                }
            }
        }
    }

    let enabled_destinations: Vec<&Destination> = destinations
        .iter()
        .filter(|d| job.destination_ids.contains(&d.id) && d.enabled)
        .collect();
    if job.destination_ids.is_empty() || enabled_destinations.is_empty() {
        report.push(Field::Destinations, copy::job::ERR_NO_DESTINATIONS);
    }

    validate_schedule(&job.schedule, &mut report);

    if let Some(minutes) = job.timeout_minutes {
        if !(1..=1440).contains(&minutes) {
            report.push(Field::Timeout, copy::valid::TIMEOUT);
        }
    }

    if let Some(max) = job.exclusions.max_file_size_mb {
        if !(1..=1_048_576).contains(&max) {
            report.push(Field::MaxFileSize, copy::valid::MAX_FILE_SIZE);
        }
    }
    for problem in validate_patterns(&job.exclusions.patterns) {
        report.problems.push(problem);
    }

    if let Some(retention) = &job.retention {
        validate_retention(retention, &mut report);
    }

    if let Some(bandwidth) = &job.bandwidth {
        for value in [bandwidth.upload_kbps, bandwidth.download_kbps].into_iter().flatten() {
            if !(1..=10_000_000).contains(&value) {
                report.push(Field::Bandwidth, copy::valid::BANDWIDTH);
                break;
            }
        }
    }

    // Cross-field warnings: true, worth saying, and never blocking.
    if !enabled_destinations.is_empty()
        && enabled_destinations
            .iter()
            .all(|d| matches!(d.kind, DestinationKind::LocalMirror { .. }))
    {
        report.warnings.push(copy::warn::MIRROR_ONLY.to_string());
    }
    if enabled_destinations.len() > 1 {
        let roots: Vec<String> = enabled_destinations
            .iter()
            .filter_map(|d| d.kind.local_path())
            .map(|p| drive_of(p))
            .collect();
        if roots.len() == enabled_destinations.len()
            && roots.windows(2).all(|w| w[0] == w[1])
            && !roots.is_empty()
        {
            report.warnings.push(copy::warn_same_drive(&roots[0]));
        }
    }
    for destination in &enabled_destinations {
        if destination.last_verified_at.is_none() && job.enabled && job.schedule.is_automatic() {
            report.warnings.push(copy::warn_unverified_dest(&destination.name));
        }
    }

    report
}

fn drive_of(path: &Path) -> String {
    let text = path.to_string_lossy();
    if cfg!(windows) {
        text.chars().take(2).collect()
    } else {
        text.split('/').take(2).collect::<Vec<_>>().join("/")
    }
}

pub fn validate_schedule(schedule: &Schedule, report: &mut Report) {
    match schedule {
        Schedule::Manual => {}
        Schedule::Interval { minutes } => {
            if !(1..=10_080).contains(minutes) {
                report.push(Field::Schedule, copy::valid::SCHEDULE_INTERVAL);
            }
        }
        Schedule::Daily { times } => validate_times(times, report),
        Schedule::Weekly { weekdays, times } => {
            if weekdays.is_empty() {
                report.push(Field::ScheduleWeekdays, copy::valid::SCHEDULE_WEEKDAYS);
            }
            validate_times(times, report);
        }
        Schedule::Cron { expression } => {
            if let Err(message) = parse_cron(expression) {
                report.push(Field::Cron, message);
            }
        }
        Schedule::OnChange { debounce_seconds, min_interval_minutes } => {
            if !(5..=3_600).contains(debounce_seconds) {
                report.push(Field::Debounce, copy::valid::SCHEDULE_DEBOUNCE);
            }
            if !(1..=1_440).contains(min_interval_minutes) {
                report.push(Field::MinInterval, copy::valid::SCHEDULE_MIN_INTERVAL);
            }
        }
    }
}

fn validate_times(times: &[superbackup_core::model::TimeOfDay], report: &mut Report) {
    if times.is_empty() || times.len() > 24 {
        report.push(Field::ScheduleTimes, copy::valid::SCHEDULE_TIMES);
        return;
    }
    let mut seen = times.to_vec();
    seen.sort();
    let before = seen.len();
    seen.dedup();
    if seen.len() != before {
        report.push(Field::ScheduleTimes, copy::valid::SCHEDULE_TIMES_DUP);
    }
}

/// A five-field cron check good enough to tell the user what is wrong without
/// pulling in an evaluator the daemon already owns. The daemon remains the
/// authority; this only stops obviously broken input from being saved.
pub fn parse_cron(expression: &str) -> Result<(), String> {
    let fields: Vec<&str> = expression.split_whitespace().collect();
    if fields.len() != 5 {
        return Err(format!(
            "A cron expression has five fields: minute, hour, day of month, month, day of week. This has {}.",
            fields.len()
        ));
    }
    let ranges: [(u32, u32); 5] = [(0, 59), (0, 23), (1, 31), (1, 12), (0, 7)];
    for (index, field) in fields.iter().enumerate() {
        let (min, max) = ranges[index];
        for part in field.split(',') {
            let part = part.trim();
            if part.is_empty() {
                return Err(format!("Field {} is empty.", index + 1));
            }
            let value = match part.split_once('/') {
                Some((base, step)) => {
                    if step.parse::<u32>().map(|s| s == 0).unwrap_or(true) {
                        return Err(format!("`{part}` has a step that is not a number above zero."));
                    }
                    base
                }
                None => part,
            };
            if value == "*" {
                continue;
            }
            let bounds: Vec<&str> = value.split('-').collect();
            for bound in bounds {
                if bound.is_empty() {
                    return Err(format!("`{part}` is not a range this build understands."));
                }
                match bound.parse::<u32>() {
                    Ok(n) if (min..=max).contains(&n) => {}
                    Ok(n) => {
                        return Err(format!(
                            "`{n}` is outside {min} to {max} for field {}.",
                            index + 1
                        ))
                    }
                    Err(_) => {
                        // Names are legal in the month and weekday fields.
                        if index >= 3 && bound.chars().all(|c| c.is_ascii_alphabetic()) {
                            continue;
                        }
                        return Err(format!("`{bound}` is not a number."));
                    }
                }
            }
        }
    }
    Ok(())
}

/// One problem per offending line, numbered, so the multiline pattern box can
/// point at the line rather than saying "something is wrong".
pub fn validate_patterns(patterns: &[String]) -> Vec<Problem> {
    let mut out = Vec::new();
    for (index, pattern) in patterns.iter().enumerate() {
        let line = index + 1;
        let trimmed = pattern.trim();
        if trimmed.is_empty() {
            out.push(Problem { field: Field::Patterns, message: copy::valid_pattern_empty(line) });
            continue;
        }
        if looks_absolute(trimmed) {
            out.push(Problem {
                field: Field::Patterns,
                message: copy::valid_pattern_absolute(line),
            });
            continue;
        }
        if let Err(reason) = check_glob(trimmed) {
            out.push(Problem {
                field: Field::Patterns,
                message: copy::valid_pattern_invalid(line, &reason),
            });
        }
    }
    out
}

fn looks_absolute(pattern: &str) -> bool {
    let bytes = pattern.as_bytes();
    if bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return matches!(bytes[2], b'\\' | b'/');
    }
    pattern.starts_with("\\\\")
}

/// A conservative glob check. `globset` is not a dependency of this crate, so
/// this catches the mistakes that matter — unbalanced brackets and braces —
/// rather than claiming to be a full parser.
fn check_glob(pattern: &str) -> Result<(), String> {
    let mut bracket = false;
    let mut brace = 0i32;
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                chars.next();
            }
            '[' => {
                if bracket {
                    return Err("a `[` inside a character class".into());
                }
                bracket = true;
            }
            ']' => {
                if !bracket {
                    return Err("a `]` with no `[` before it".into());
                }
                bracket = false;
            }
            '{' => brace += 1,
            '}' => {
                brace -= 1;
                if brace < 0 {
                    return Err("a `}` with no `{` before it".into());
                }
            }
            _ => {}
        }
    }
    if bracket {
        return Err("a `[` that is never closed".into());
    }
    if brace != 0 {
        return Err("a `{` that is never closed".into());
    }
    Ok(())
}

pub fn validate_retention(policy: &RetentionPolicy, report: &mut Report) {
    let all = [
        policy.keep_latest,
        policy.keep_hourly,
        policy.keep_daily,
        policy.keep_weekly,
        policy.keep_monthly,
        policy.keep_annual,
    ];
    if all.iter().all(|v| *v == 0) {
        report.push(Field::Retention, copy::RETENTION_ERR_ALL_ZERO);
    }
    if all.iter().any(|v| *v > 10_000) {
        report.push(Field::Retention, "Keep at most 10,000 of each kind.");
    }
    if policy.maintenance_every_n_runs > 1_000 {
        report.push(Field::Maintenance, copy::valid::MAINTENANCE);
    }
}

// ---------------------------------------------------------------------------
// Destinations
// ---------------------------------------------------------------------------

pub fn validate_destination(
    destination: &Destination,
    others: &[Destination],
    jobs: &[Job],
) -> Report {
    let mut report = Report::default();

    let name = destination.name.trim();
    if name.is_empty() {
        report.push(Field::Name, copy::valid::DEST_NAME_EMPTY);
    } else if name.chars().count() > 64 {
        report.push(Field::Name, copy::valid::JOB_NAME_LONG);
    } else if others
        .iter()
        .any(|o| o.id != destination.id && o.name.trim().eq_ignore_ascii_case(name))
    {
        report.push(Field::Name, copy::valid_dest_name_dup(name));
    }

    match &destination.kind {
        DestinationKind::LocalRepository { path }
        | DestinationKind::OneDrive { path, .. }
        | DestinationKind::LocalMirror { path } => {
            if path.as_os_str().is_empty() || !path.is_absolute() {
                report.push(Field::Path, copy::valid::DEST_PATH_RELATIVE);
            } else {
                if let Some(parent) = path.parent() {
                    if !parent.exists() && !path.exists() {
                        report.push(
                            Field::Path,
                            copy::valid_dest_path_parent(&parent.to_string_lossy()),
                        );
                    }
                }
                for other in others {
                    if other.id == destination.id {
                        continue;
                    }
                    if let Some(other_path) = other.kind.local_path() {
                        if path.starts_with(other_path) || other_path.starts_with(path) {
                            report.push(
                                Field::Path,
                                copy::valid_dest_path_inside_dest(&other.name),
                            );
                            break;
                        }
                    }
                }
                for job in jobs.iter().filter(|j| j.destination_ids.contains(&destination.id)) {
                    for source in &job.sources {
                        if path.starts_with(&source.path) {
                            report.push(
                                Field::Path,
                                copy::valid_dest_path_inside_source(
                                    &source.path.to_string_lossy(),
                                ),
                            );
                        }
                    }
                }
            }
        }
        DestinationKind::S3 { bucket, .. } => {
            if let Err(message) = validate_bucket(bucket) {
                report.push(Field::Bucket, message);
            }
        }
    }

    validate_retention(&destination.retention, &mut report);
    report
}

/// S3 bucket naming, which is worth checking here because the failure at run
/// time is a signature error that explains nothing.
pub fn validate_bucket(bucket: &str) -> Result<(), String> {
    let b = bucket.trim();
    if b.len() < 3 || b.len() > 63 {
        return Err(copy::valid::BUCKET.to_string());
    }
    if !b
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.')
    {
        return Err(copy::valid::BUCKET.to_string());
    }
    if b.starts_with('-') || b.ends_with('-') || b.starts_with('.') || b.ends_with('.') {
        return Err(copy::valid::BUCKET.to_string());
    }
    let looks_like_ip = b.split('.').count() == 4
        && b.split('.').all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()));
    if looks_like_ip {
        return Err(copy::valid::BUCKET_IP.to_string());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Providers
// ---------------------------------------------------------------------------

pub fn validate_provider(
    provider: &StorageProvider,
    others: &[StorageProvider],
    access_key: &str,
    secret_key: &str,
    credentials_required: bool,
) -> Report {
    let mut report = Report::default();

    let name = provider.name.trim();
    if name.is_empty() {
        report.push(Field::Name, copy::valid::PROVIDER_NAME_EMPTY);
    } else if name.chars().count() > 64 {
        report.push(Field::Name, copy::valid::JOB_NAME_LONG);
    } else if others
        .iter()
        .any(|o| o.id != provider.id && o.name.trim().eq_ignore_ascii_case(name))
    {
        report.push(Field::Name, copy::valid_provider_name_dup(name));
    }

    let ProviderKind::S3 { endpoint, region, tls, flavour, .. } = &provider.kind;
    match parse_endpoint(endpoint) {
        Err(message) => report.push(Field::Endpoint, message),
        Ok(parsed) => {
            if !*tls && !parsed.is_local {
                report.warnings.push(copy::valid::ENDPOINT_INSECURE.to_string());
            }
        }
    }
    if *flavour == S3Flavour::AwsS3 && region.trim().is_empty() {
        report.push(Field::Region, copy::valid::REGION);
    }

    if credentials_required && (access_key.trim().is_empty() || secret_key.is_empty()) {
        report.push(Field::Credentials, copy::valid::CREDENTIALS);
    }

    report
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub scheme: String,
    pub host: String,
    pub port: u16,
    pub is_local: bool,
}

/// Accepts a host with or without a scheme, matching the model's comment, and
/// shows the normalised form back rather than rejecting the input.
pub fn parse_endpoint(input: &str) -> Result<Endpoint, String> {
    let raw = input.trim();
    if raw.is_empty() {
        return Err(copy::valid::ENDPOINT_EMPTY.to_string());
    }
    let (scheme, rest) = match raw.split_once("://") {
        Some((s, r)) => (s.to_lowercase(), r),
        None => ("https".to_string(), raw),
    };
    if scheme != "http" && scheme != "https" {
        return Err(copy::valid::ENDPOINT_INVALID.to_string());
    }
    let rest = rest.split('/').next().unwrap_or(rest);
    if rest.is_empty() {
        return Err(copy::valid::ENDPOINT_INVALID.to_string());
    }
    let (host, port) = match rest.rsplit_once(':') {
        Some((h, p)) => match p.parse::<u16>() {
            Ok(port) => (h, port),
            Err(_) => return Err(copy::valid::ENDPOINT_INVALID.to_string()),
        },
        None => (rest, if scheme == "https" { 443 } else { 80 }),
    };
    if host.is_empty()
        || host.starts_with('.')
        || host.ends_with('.')
        || host.contains(' ')
        || !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
    {
        return Err(copy::valid::ENDPOINT_INVALID.to_string());
    }
    let is_local = host == "localhost"
        || host == "127.0.0.1"
        || host == "::1"
        || host.starts_with("10.")
        || host.starts_with("192.168.")
        || host.ends_with(".local")
        || host.ends_with(".internal");
    Ok(Endpoint { scheme, host: host.to_string(), port, is_local })
}

// ---------------------------------------------------------------------------
// Passphrases
// ---------------------------------------------------------------------------

/// A local, zxcvbn-shaped score in 0..=4. No dictionary is shipped, so this
/// rewards length and variety and punishes the shapes that actually appear in
/// weak passphrases: repetition, keyboard runs, and a single short word.
pub fn passphrase_score(passphrase: &str) -> u8 {
    let len = passphrase.chars().count();
    if len == 0 {
        return 0;
    }
    let lower = passphrase.to_lowercase();

    let classes = [
        passphrase.chars().any(|c| c.is_ascii_lowercase()),
        passphrase.chars().any(|c| c.is_ascii_uppercase()),
        passphrase.chars().any(|c| c.is_ascii_digit()),
        passphrase.chars().any(|c| !c.is_alphanumeric()),
    ]
    .iter()
    .filter(|x| **x)
    .count();

    let words = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 3)
        .count();

    let mut score = 0i32;
    score += match len {
        0..=7 => 0,
        8..=11 => 1,
        12..=15 => 2,
        16..=23 => 3,
        _ => 4,
    };
    score += match words {
        0 | 1 => 0,
        2 | 3 => 1,
        _ => 2,
    };
    score += match classes {
        0 | 1 => 0,
        2 | 3 => 1,
        _ => 2,
    };

    // Obvious weaknesses knock the score back down.
    const COMMON: [&str; 10] = [
        "password", "passphrase", "123456", "qwerty", "letmein", "welcome", "admin",
        "superbackup", "backup", "iloveyou",
    ];
    if COMMON.iter().any(|c| lower.contains(c)) {
        score -= 2;
    }
    if is_single_repeated(&lower) {
        score -= 2;
    }
    if len < 12 {
        score = score.min(1);
    }

    score.clamp(0, 6) as u8 * 4 / 6
}

fn is_single_repeated(text: &str) -> bool {
    let distinct: std::collections::BTreeSet<char> = text.chars().collect();
    distinct.len() <= 2 && text.chars().count() > 2
}

pub fn strength_word(score: u8) -> &'static str {
    match score {
        0 | 1 => copy::strength::TOO_WEAK,
        2 => copy::strength::WEAK,
        3 => copy::strength::GOOD,
        _ => copy::strength::STRONG,
    }
}

/// The master-passphrase policy: at least 12 characters and a matching
/// confirmation. A weak score does not block — it adds friction, on O-3.
pub fn master_passphrase(passphrase: &str, confirm: &str) -> Report {
    let mut report = Report::default();
    if passphrase.chars().count() < 12 {
        report.push(Field::Passphrase, copy::valid::MASTER_SHORT);
    }
    if !confirm.is_empty() && passphrase != confirm {
        report.push(Field::PassphraseConfirm, copy::valid::MASTER_MISMATCH);
    }
    if confirm.is_empty() && !passphrase.is_empty() {
        report.push(Field::PassphraseConfirm, copy::valid::MASTER_MISMATCH);
    }
    report
}

pub fn repository_passphrase(passphrase: &str, confirm: &str) -> Report {
    let mut report = Report::default();
    if passphrase.chars().count() < 12 {
        report.push(Field::Passphrase, copy::valid::REPO_PASS_SHORT);
    }
    if passphrase != confirm {
        report.push(Field::PassphraseConfirm, copy::valid::REPO_PASS_MISMATCH);
    }
    report
}

// ---------------------------------------------------------------------------
// Wizard gating (UX_SPEC §7)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WizardStep {
    Template,
    Sources,
    Destinations,
    Schedule,
    Exclusions,
    Review,
}

impl WizardStep {
    pub const ALL: [WizardStep; 6] = [
        WizardStep::Template,
        WizardStep::Sources,
        WizardStep::Destinations,
        WizardStep::Schedule,
        WizardStep::Exclusions,
        WizardStep::Review,
    ];
    pub fn index(self) -> usize {
        WizardStep::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }
    pub fn title(self) -> &'static str {
        match self {
            WizardStep::Template => "Template",
            WizardStep::Sources => copy::job::TAB_SOURCES,
            WizardStep::Destinations => copy::job::TAB_DESTINATIONS,
            WizardStep::Schedule => copy::job::TAB_SCHEDULE,
            WizardStep::Exclusions => copy::job::TAB_EXCLUSIONS,
            WizardStep::Review => "Review",
        }
    }
    pub fn next(self) -> Option<WizardStep> {
        WizardStep::ALL.get(self.index() + 1).copied()
    }
    pub fn previous(self) -> Option<WizardStep> {
        if self.index() == 0 {
            None
        } else {
            WizardStep::ALL.get(self.index() - 1).copied()
        }
    }
}

/// Why `Continue` is disabled on this step, or `None` when it is not.
pub fn wizard_blocked(step: WizardStep, draft: &Job, destinations: &[Destination]) -> Option<String> {
    match step {
        WizardStep::Template => None,
        WizardStep::Sources => {
            if draft.sources.is_empty() {
                Some(copy::valid::SOURCE_NONE.to_string())
            } else if draft.name.trim().is_empty() {
                Some(copy::valid::JOB_NAME_EMPTY.to_string())
            } else {
                None
            }
        }
        WizardStep::Destinations => {
            let usable = destinations
                .iter()
                .any(|d| draft.destination_ids.contains(&d.id) && d.enabled);
            if usable {
                None
            } else {
                Some(copy::job::ERR_NO_DESTINATIONS.to_string())
            }
        }
        WizardStep::Schedule => {
            let mut report = Report::default();
            validate_schedule(&draft.schedule, &mut report);
            report.problems.first().map(|p| p.message.clone())
        }
        WizardStep::Exclusions => {
            validate_patterns(&draft.exclusions.patterns).first().map(|p| p.message.clone())
        }
        WizardStep::Review => {
            let report = validate_job(draft, &[], destinations);
            report.summary()
        }
    }
}

/// The onboarding flow's own gating. Steps 1–3 are mandatory; the rest are
/// skippable, and the vault is created when O-3 is confirmed, not before.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OnboardingStep {
    #[default]
    Welcome,
    Passphrase,
    NoRecovery,
    Scan,
    FirstJob,
    KeepRunning,
    Done,
}

impl OnboardingStep {
    pub const ALL: [OnboardingStep; 7] = [
        OnboardingStep::Welcome,
        OnboardingStep::Passphrase,
        OnboardingStep::NoRecovery,
        OnboardingStep::Scan,
        OnboardingStep::FirstJob,
        OnboardingStep::KeepRunning,
        OnboardingStep::Done,
    ];
    pub fn index(self) -> usize {
        OnboardingStep::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }
    pub fn next(self) -> Option<OnboardingStep> {
        OnboardingStep::ALL.get(self.index() + 1).copied()
    }
    pub fn previous(self) -> Option<OnboardingStep> {
        if self.index() == 0 {
            None
        } else {
            OnboardingStep::ALL.get(self.index() - 1).copied()
        }
    }
    /// `Skip setup` appears on steps 4–6 only.
    pub fn skippable(self) -> bool {
        matches!(
            self,
            OnboardingStep::Scan | OnboardingStep::FirstJob | OnboardingStep::KeepRunning
        )
    }
}

/// Uniqueness helper shared by the three editors.
pub fn unique_name(candidate: &str, taken: &[String]) -> String {
    let mut name = candidate.to_string();
    let mut n = 2;
    while taken.iter().any(|t| t.eq_ignore_ascii_case(&name)) {
        name = format!("{candidate} {n}");
        n += 1;
    }
    name
}

/// A stable id for a draft that has not been saved yet.
pub fn draft_id() -> Uuid {
    Uuid::new_v4()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::path::PathBuf;
    use superbackup_core::model::{Source, TimeOfDay};

    fn destination(name: &str, kind: DestinationKind) -> Destination {
        Destination {
            id: Uuid::new_v4(),
            name: name.into(),
            kind,
            encryption: None,
            passphrase_ref: None,
            retention: RetentionPolicy::default(),
            enabled: true,
            auto_discovered: false,
            bandwidth: None,
            created_at: Utc::now(),
            last_verified_at: Some(Utc::now()),
        }
    }

    fn job(name: &str, destinations: Vec<Uuid>) -> Job {
        Job {
            id: Uuid::new_v4(),
            name: name.into(),
            project_id: None,
            description: String::new(),
            sources: vec![Source::new(if cfg!(windows) {
                r"C:\Users\andreas\dev"
            } else {
                "/home/andreas/dev"
            })],
            destination_ids: destinations,
            schedule: Schedule::Daily { times: vec![TimeOfDay { hour: 2, minute: 0 }] },
            exclusions: Default::default(),
            bandwidth: None,
            retention: None,
            enabled: true,
            timeout_minutes: None,
            hooks: Default::default(),
            continue_on_destination_error: true,
            created_at: Utc::now(),
            tags: vec![],
        }
    }

    #[test]
    fn a_job_with_no_destination_cannot_be_saved() {
        let j = job("Dev code", vec![]);
        let report = validate_job(&j, &[], &[]);
        assert!(!report.ok());
        assert_eq!(
            report.for_field(Field::Destinations),
            Some(copy::job::ERR_NO_DESTINATIONS)
        );
    }

    #[test]
    fn a_duplicate_job_name_is_rejected_case_insensitively() {
        let d = destination(
            "Local",
            DestinationKind::LocalRepository { path: PathBuf::from("/backups/repo") },
        );
        let existing = job("Dev code", vec![d.id]);
        let mut candidate = job("dev CODE", vec![d.id]);
        candidate.id = Uuid::new_v4();
        let report = validate_job(&candidate, std::slice::from_ref(&existing), std::slice::from_ref(&d));
        assert!(report.for_field(Field::Name).is_some());
    }

    #[test]
    fn a_source_inside_a_destination_is_a_recursive_backup() {
        let root = if cfg!(windows) { r"C:\Users\andreas" } else { "/home/andreas" };
        let d = destination(
            "Inside home",
            DestinationKind::LocalRepository { path: PathBuf::from(root) },
        );
        let j = job("Dev code", vec![d.id]);
        let report = validate_job(&j, &[], std::slice::from_ref(&d));
        assert!(
            report.for_field(Field::Sources).is_some(),
            "a backup that contains its own destination must be refused"
        );
    }

    #[test]
    fn a_mirror_only_job_warns_but_still_saves() {
        let d = destination(
            "Mirror",
            DestinationKind::LocalMirror { path: PathBuf::from("/mirror") },
        );
        let j = job("Docs", vec![d.id]);
        let report = validate_job(&j, &[], std::slice::from_ref(&d));
        assert!(report.ok(), "the warning must not block saving");
        assert!(report.warnings.iter().any(|w| w.contains("folder mirror")));
    }

    #[test]
    fn schedule_bounds_come_from_the_specification() {
        let mut report = Report::default();
        validate_schedule(&Schedule::Interval { minutes: 0 }, &mut report);
        assert_eq!(report.for_field(Field::Schedule), Some(copy::valid::SCHEDULE_INTERVAL));

        let mut report = Report::default();
        validate_schedule(&Schedule::Interval { minutes: 10_081 }, &mut report);
        assert!(!report.ok());

        let mut report = Report::default();
        validate_schedule(&Schedule::Daily { times: vec![] }, &mut report);
        assert_eq!(report.for_field(Field::ScheduleTimes), Some(copy::valid::SCHEDULE_TIMES));

        let mut report = Report::default();
        validate_schedule(
            &Schedule::OnChange { debounce_seconds: 1, min_interval_minutes: 0 },
            &mut report,
        );
        assert_eq!(report.problems.len(), 2);
    }

    #[test]
    fn cron_errors_name_the_problem() {
        assert!(parse_cron("0 2 * * *").is_ok());
        assert!(parse_cron("*/15 * * * *").is_ok());
        assert!(parse_cron("0 2 * *").is_err());
        assert!(parse_cron("99 2 * * *").is_err());
        assert!(parse_cron("0 2 * * mon").is_ok());
    }

    #[test]
    fn patterns_are_reported_by_line_number() {
        let patterns = vec![
            "node_modules/".to_string(),
            "".to_string(),
            r"C:\Users\andreas\dev".to_string(),
            "**/[unclosed".to_string(),
        ];
        let problems = validate_patterns(&patterns);
        assert_eq!(problems.len(), 3);
        assert!(problems[0].message.contains("Line 2"));
        assert!(problems[1].message.contains("Line 3"));
        assert!(problems[2].message.contains("Line 4"));
    }

    #[test]
    fn retention_refuses_to_keep_nothing() {
        let mut report = Report::default();
        validate_retention(
            &RetentionPolicy {
                keep_latest: 0,
                keep_hourly: 0,
                keep_daily: 0,
                keep_weekly: 0,
                keep_monthly: 0,
                keep_annual: 0,
                maintenance_every_n_runs: 5,
            },
            &mut report,
        );
        assert_eq!(report.for_field(Field::Retention), Some(copy::RETENTION_ERR_ALL_ZERO));
    }

    #[test]
    fn buckets_follow_the_s3_naming_rules() {
        assert!(validate_bucket("storj-backups").is_ok());
        assert!(validate_bucket("ab").is_err());
        assert!(validate_bucket("Not-Lower").is_err());
        assert!(validate_bucket("192.168.1.1").is_err());
        assert!(validate_bucket("-leading").is_err());
    }

    #[test]
    fn endpoints_accept_a_missing_scheme_and_report_the_normalised_form() {
        let parsed = parse_endpoint("gateway.storjshare.io").expect("a host is enough");
        assert_eq!(parsed.scheme, "https");
        assert_eq!(parsed.port, 443);
        assert!(!parsed.is_local);

        let parsed = parse_endpoint("http://localhost:9000").expect("a local MinIO");
        assert_eq!(parsed.port, 9000);
        assert!(parsed.is_local);

        assert!(parse_endpoint("").is_err());
        assert!(parse_endpoint("ftp://example.com").is_err());
        assert!(parse_endpoint("not a host").is_err());
    }

    #[test]
    fn the_strength_meter_separates_the_four_bands() {
        assert_eq!(passphrase_score(""), 0);
        assert!(passphrase_score("short") <= 1);
        assert!(passphrase_score("password1234") <= 1, "a common word must not score well");
        assert!(passphrase_score("aaaaaaaaaaaaaaaa") <= 1);
        let good = passphrase_score("correct horse battery");
        assert!((2..=4).contains(&good), "score was {good}");
        let strong = passphrase_score("correct-horse-battery-staple-9");
        assert_eq!(strong, 4, "a long multi-word passphrase is strong");
        assert!(strong >= good);
    }

    #[test]
    fn the_master_policy_is_twelve_characters_and_a_match() {
        assert!(!master_passphrase("short", "short").ok());
        assert!(!master_passphrase("a-long-enough-one", "different").ok());
        assert!(master_passphrase("a-long-enough-one", "a-long-enough-one").ok());
        // A weak but long passphrase passes: friction lives on O-3, not here.
        assert!(master_passphrase("aaaaaaaaaaaaaa", "aaaaaaaaaaaaaa").ok());
    }

    #[test]
    fn the_wizard_refuses_to_advance_past_an_empty_step() {
        let d = destination(
            "Local",
            DestinationKind::LocalRepository { path: PathBuf::from("/backups/repo") },
        );
        let mut draft = job("Dev code", vec![]);
        draft.sources.clear();

        assert!(wizard_blocked(WizardStep::Template, &draft, &[]).is_none());
        assert!(wizard_blocked(WizardStep::Sources, &draft, &[]).is_some());

        draft.sources.push(Source::new(if cfg!(windows) { r"C:\dev" } else { "/dev" }));
        assert!(wizard_blocked(WizardStep::Sources, &draft, &[]).is_none());
        assert!(wizard_blocked(WizardStep::Destinations, &draft, std::slice::from_ref(&d)).is_some());

        draft.destination_ids.push(d.id);
        assert!(wizard_blocked(WizardStep::Destinations, &draft, std::slice::from_ref(&d)).is_none());
        assert!(wizard_blocked(WizardStep::Schedule, &draft, std::slice::from_ref(&d)).is_none());
        assert!(wizard_blocked(WizardStep::Exclusions, &draft, std::slice::from_ref(&d)).is_none());
    }

    #[test]
    fn a_disabled_destination_does_not_satisfy_the_wizard() {
        let mut d = destination(
            "Off",
            DestinationKind::LocalRepository { path: PathBuf::from("/backups/repo") },
        );
        d.enabled = false;
        let draft = job("Dev code", vec![d.id]);
        assert!(wizard_blocked(WizardStep::Destinations, &draft, std::slice::from_ref(&d)).is_some());
    }

    #[test]
    fn onboarding_steps_one_to_three_are_not_skippable() {
        assert!(!OnboardingStep::Welcome.skippable());
        assert!(!OnboardingStep::Passphrase.skippable());
        assert!(!OnboardingStep::NoRecovery.skippable());
        assert!(OnboardingStep::Scan.skippable());
        assert!(OnboardingStep::KeepRunning.skippable());
        assert!(!OnboardingStep::Done.skippable());
    }

    #[test]
    fn unique_names_do_not_collide() {
        let taken = vec!["Dev code".to_string(), "Dev code 2".to_string()];
        assert_eq!(unique_name("Dev code", &taken), "Dev code 3");
        assert_eq!(unique_name("Photos", &taken), "Photos");
    }
}
