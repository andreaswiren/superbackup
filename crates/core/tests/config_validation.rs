//! One test per validation rule, plus the "a valid config really is accepted"
//! control that stops the whole suite from passing vacuously.

use std::path::PathBuf;

use superbackup_core::config::{normalise, validate, ValidationReport};
use superbackup_core::model::*;
use uuid::Uuid;

/// An absolute path that is actually absolute on the host platform.
fn abs(tail: &str) -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(format!(r"C:\{}", tail.replace('/', "\\")))
    } else {
        PathBuf::from(format!("/{tail}"))
    }
}

fn provider(name: &str) -> StorageProvider {
    let id = Uuid::new_v4();
    StorageProvider {
        id,
        name: name.into(),
        kind: ProviderKind::S3 {
            endpoint: "https://gateway.storjshare.io".into(),
            region: "eu-1".into(),
            credentials: S3Credentials::for_provider(&id),
            tls: true,
            path_style: false,
            flavour: S3Flavour::Storj,
        },
        notes: String::new(),
        created_at: chrono::Utc::now(),
        last_verified_at: None,
    }
}

fn local_destination(name: &str, path: PathBuf) -> Destination {
    Destination {
        id: Uuid::new_v4(),
        name: name.into(),
        kind: DestinationKind::LocalRepository { path },
        encryption: Some(EncryptionSettings::default()),
        passphrase_ref: Some(SecretRef("repo.passphrase:0".into())),
        retention: RetentionPolicy::default(),
        enabled: true,
        auto_discovered: false,
        bandwidth: None,
        created_at: chrono::Utc::now(),
        last_verified_at: None,
    }
}

fn s3_destination(name: &str, provider_id: Uuid, prefix: &str) -> Destination {
    let id = Uuid::new_v4();
    Destination {
        id,
        name: name.into(),
        kind: DestinationKind::S3 {
            provider_id,
            bucket: "backups".into(),
            prefix: prefix.into(),
            credential_override: None,
        },
        encryption: Some(EncryptionSettings::default()),
        passphrase_ref: Some(SecretRef(format!("repo.passphrase:{id}"))),
        retention: RetentionPolicy::default(),
        enabled: true,
        auto_discovered: false,
        bandwidth: None,
        created_at: chrono::Utc::now(),
        last_verified_at: None,
    }
}

fn job(name: &str, sources: Vec<PathBuf>, destinations: Vec<Uuid>) -> Job {
    Job {
        id: Uuid::new_v4(),
        name: name.into(),
        project_id: None,
        description: String::new(),
        sources: sources.into_iter().map(Source::new).collect(),
        destination_ids: destinations,
        schedule: Schedule::Daily { times: vec![TimeOfDay { hour: 2, minute: 30 }] },
        exclusions: ExclusionSet::developer_defaults(),
        bandwidth: None,
        retention: None,
        enabled: true,
        timeout_minutes: None,
        hooks: JobHooks::default(),
        continue_on_destination_error: true,
        created_at: chrono::Utc::now(),
        tags: Vec::new(),
    }
}

/// A configuration that must validate cleanly, and the base for every negative
/// test below.
fn valid_config() -> Config {
    let p = provider("StorJ eu-1");
    let provider_id = p.id;
    let local = local_destination("Fast local", abs("backups/local"));
    let remote = s3_destination("Offsite", provider_id, "superbackup/pc-1/");
    let ids = vec![local.id, remote.id];

    let mut config = Config::default();
    config.machine.slug = "pc-1".into();
    config.machine.label = "Workstation".into();
    config.providers.push(p);
    config.destinations.push(local);
    config.destinations.push(remote);
    config.jobs.push(job("dev-code", vec![abs("code")], ids));
    config
}

fn errors(config: &Config) -> Vec<String> {
    validate(config).errors.into_iter().map(|e| e.to_string()).collect()
}

fn assert_error_mentioning(report: &ValidationReport, needle: &str) {
    assert!(
        report.errors.iter().any(|e| e.to_string().contains(needle)),
        "expected an error mentioning {needle:?}, got: {:#?}",
        report.errors
    );
}

// ---------------------------------------------------------------------------

#[test]
fn the_control_config_is_accepted() {
    let config = valid_config();
    let report = validate(&config);
    assert!(report.is_ok(), "the baseline must validate: {:#?}", report.errors);
    assert!(report.into_result().is_ok());
}

#[test]
fn duplicate_names_are_rejected_case_insensitively() {
    for mutate in [
        // Jobs.
        (|c: &mut Config| {
            let mut clone = c.jobs[0].clone();
            clone.id = Uuid::new_v4();
            clone.name = "DEV-CODE".into();
            c.jobs.push(clone);
        }) as fn(&mut Config),
        // Destinations.
        |c: &mut Config| {
            let mut clone = c.destinations[0].clone();
            clone.id = Uuid::new_v4();
            clone.name = "fast LOCAL".into();
            c.destinations.push(clone);
        },
        // Providers.
        |c: &mut Config| {
            let mut clone = c.providers[0].clone();
            clone.id = Uuid::new_v4();
            clone.name = "storj EU-1".into();
            c.providers.push(clone);
        },
    ] {
        let mut config = valid_config();
        mutate(&mut config);
        let report = validate(&config);
        assert_error_mentioning(&report, "ambiguous");
    }
}

#[test]
fn duplicate_ids_are_rejected() {
    let mut config = valid_config();
    let clone = config.jobs[0].clone();
    let mut clone = clone;
    clone.name = "different-name".into();
    config.jobs.push(clone);
    assert_error_mentioning(&validate(&config), "share the id");
}

#[test]
fn empty_names_are_rejected() {
    let mut config = valid_config();
    config.jobs[0].name = "   ".into();
    normalise(&mut config);
    assert_error_mentioning(&validate(&config), "name cannot be empty");
}

#[test]
fn a_job_pointing_at_a_missing_destination_is_rejected() {
    let mut config = valid_config();
    let ghost = Uuid::new_v4();
    config.jobs[0].destination_ids.push(ghost);
    assert_error_mentioning(&validate(&config), &format!("no destination with id {ghost}"));
}

#[test]
fn a_job_listing_the_same_destination_twice_is_rejected() {
    let mut config = valid_config();
    let existing = config.destinations[0].id;
    config.jobs[0].destination_ids.push(existing);
    assert_error_mentioning(&validate(&config), "listed twice");
}

#[test]
fn a_job_pointing_at_a_missing_project_is_rejected() {
    let mut config = valid_config();
    config.jobs[0].project_id = Some(Uuid::new_v4());
    assert_error_mentioning(&validate(&config), "no project with id");
}

#[test]
fn an_s3_destination_pointing_at_a_missing_provider_is_rejected() {
    let mut config = valid_config();
    let ghost = Uuid::new_v4();
    if let DestinationKind::S3 { provider_id, .. } = &mut config.destinations[1].kind {
        *provider_id = ghost;
    }
    assert_error_mentioning(&validate(&config), &format!("no provider with id {ghost}"));
}

#[test]
fn relative_source_paths_are_rejected() {
    let mut config = valid_config();
    config.jobs[0].sources = vec![Source::new("relative/path")];
    assert_error_mentioning(&validate(&config), "relative path");

    // And an empty one.
    let mut config = valid_config();
    config.jobs[0].sources = vec![Source::new("")];
    assert_error_mentioning(&validate(&config), "cannot be empty");
}

#[test]
fn relative_destination_paths_are_rejected() {
    let mut config = valid_config();
    config.destinations[0].kind =
        DestinationKind::LocalRepository { path: PathBuf::from("backups") };
    assert_error_mentioning(&validate(&config), "relative path");
}

#[test]
fn unnormalised_s3_prefixes_are_rejected_and_normalise_fixes_them() {
    let mut config = valid_config();
    if let DestinationKind::S3 { prefix, .. } = &mut config.destinations[1].kind {
        *prefix = "/superbackup//pc-1/../x/".into();
    }
    assert_error_mentioning(&validate(&config), "not normalised");

    normalise(&mut config);
    assert!(validate(&config).is_ok(), "normalisation must make it valid");
    match &config.destinations[1].kind {
        DestinationKind::S3 { prefix, .. } => assert_eq!(prefix, "superbackup/pc-1/x/"),
        other => panic!("unexpected kind {other:?}"),
    }
}

#[test]
fn invalid_cron_expressions_are_rejected_and_valid_ones_accepted() {
    for bad in ["", "not a cron", "* * *", "99 * * * *", "* * * * * * * *"] {
        let mut config = valid_config();
        config.jobs[0].schedule = Schedule::Cron { expression: bad.into() };
        let report = validate(&config);
        assert!(
            !report.is_ok(),
            "{bad:?} should not be accepted as a cron expression; \
             accepting it here means failing silently at 03:00"
        );
    }
    for good in ["0 3 * * *", "*/15 * * * *", "0 0 * * FRI", "30 2 1 * *"] {
        let mut config = valid_config();
        config.jobs[0].schedule = Schedule::Cron { expression: good.into() };
        let report = validate(&config);
        assert!(report.is_ok(), "{good:?} should be accepted: {:#?}", report.errors);
    }
}

#[test]
fn degenerate_schedules_are_rejected() {
    let cases: Vec<(Schedule, &str)> = vec![
        (Schedule::Interval { minutes: 0 }, "continuously"),
        (Schedule::Daily { times: vec![] }, "never runs"),
        (
            Schedule::Weekly { weekdays: vec![], times: vec![TimeOfDay { hour: 1, minute: 0 }] },
            "never runs",
        ),
        (
            Schedule::Weekly { weekdays: vec![9], times: vec![TimeOfDay { hour: 1, minute: 0 }] },
            "out of range",
        ),
        (Schedule::Daily { times: vec![TimeOfDay { hour: 25, minute: 0 }] }, "not a valid time"),
        (
            Schedule::OnChange { debounce_seconds: 0, min_interval_minutes: 10 },
            "every single file write",
        ),
    ];
    for (schedule, needle) in cases {
        let mut config = valid_config();
        config.jobs[0].schedule = schedule.clone();
        assert_error_mentioning(&validate(&config), needle);
    }
}

/// The infinite-growth footgun.
#[test]
fn a_destination_inside_its_own_jobs_source_is_rejected() {
    let mut config = valid_config();
    // "Back up C:\code to C:\code\backups" — the most natural thing to type,
    // and it fills the disk.
    config.jobs[0].sources = vec![Source::new(abs("code"))];
    config.destinations[0].kind = DestinationKind::LocalRepository { path: abs("code/backups") };
    assert_error_mentioning(&validate(&config), "grow without bound");

    // Exactly equal is the same problem.
    let mut config = valid_config();
    config.jobs[0].sources = vec![Source::new(abs("code"))];
    config.destinations[0].kind = DestinationKind::LocalRepository { path: abs("code") };
    assert_error_mentioning(&validate(&config), "grow without bound");
}

#[test]
fn a_sibling_directory_with_a_shared_prefix_is_not_nesting() {
    // `C:\data\backups` is not inside `C:\data\back`, even though the string
    // is a prefix. A component-wise comparison is the only way to get this
    // right, and getting it wrong would block a legitimate configuration.
    let mut config = valid_config();
    config.jobs[0].sources = vec![Source::new(abs("data/back"))];
    config.destinations[0].kind = DestinationKind::LocalRepository { path: abs("data/backups") };
    let report = validate(&config);
    assert!(report.is_ok(), "siblings must be allowed: {:#?}", report.errors);
}

#[cfg(windows)]
#[test]
fn nesting_detection_is_case_insensitive_on_windows() {
    let mut config = valid_config();
    config.jobs[0].sources = vec![Source::new(PathBuf::from(r"C:\Users\Andreas\Code"))];
    config.destinations[0].kind =
        DestinationKind::LocalRepository { path: PathBuf::from(r"c:\users\andreas\code\backup") };
    assert_error_mentioning(&validate(&config), "grow without bound");
}

#[test]
fn a_destination_inside_another_jobs_source_is_allowed() {
    // The rule is deliberately scoped to a job's *own* sources: putting job
    // A's destination inside job B's source is a user decision (B backs up
    // A's repository, on purpose), and it does not run away.
    let mut config = valid_config();
    let other = job("docs", vec![abs("backups")], vec![]);
    config.jobs.push(other);
    let report = validate(&config);
    assert!(report.is_ok(), "{:#?}", report.errors);
}

#[test]
fn a_non_https_remote_is_rejected() {
    let mut config = valid_config();
    config.remote = Some(RemoteConfigSource {
        url: "http://github.com/me/cfg".into(),
        branch: "main".into(),
        path: "config.sbvault".into(),
        auth: RemoteAuth::None,
        auto_pull: false,
        pull_interval_minutes: 60,
        allow_push: false,
        last_pull_at: None,
        last_known_commit: None,
        trusted_signers: vec![],
    });
    assert_error_mentioning(&validate(&config), "https://");

    if let Some(remote) = &mut config.remote {
        remote.url = "https://github.com/me/cfg".into();
    }
    assert!(validate(&config).is_ok());
}

#[test]
fn a_config_from_the_future_is_rejected() {
    let mut config = valid_config();
    config.schema_version = CONFIG_SCHEMA_VERSION + 1;
    assert_error_mentioning(&validate(&config), "newer version");
}

#[test]
fn incompleteness_is_a_warning_not_an_error() {
    // A GUI wizard legitimately saves a half-built job. Refusing would make
    // the editor unusable; saying nothing would hide a job that never runs.
    let mut config = valid_config();
    config.jobs[0].destination_ids.clear();
    let report = validate(&config);
    assert!(report.is_ok(), "an incomplete job must still be saveable: {:#?}", report.errors);
    assert!(
        report.warnings.iter().any(|w| w.to_string().contains("never write anything")),
        "{:#?}",
        report.warnings
    );
}

#[test]
fn a_job_with_no_sources_is_an_error() {
    let mut config = valid_config();
    config.jobs[0].sources.clear();
    assert_error_mentioning(&validate(&config), "backs up nothing");
}

#[test]
fn every_error_is_reported_at_once() {
    let mut config = valid_config();
    config.jobs[0].sources = vec![Source::new("relative")];
    config.jobs[0].schedule = Schedule::Cron { expression: "nonsense".into() };
    config.jobs[0].destination_ids.push(Uuid::new_v4());
    let found = errors(&config);
    assert!(found.len() >= 3, "fixing one error per save round-trip is miserable: {found:#?}");

    let message = validate(&config).into_result().expect_err("must fail").to_string();
    assert!(message.contains("relative"), "{message}");
    assert!(message.contains("cron"), "{message}");
    assert!(message.contains("no destination with id"), "{message}");
}
