//! A believable machine, for render tests and for the screenshots the design
//! review works from.
//!
//! The ids are constants so a test can navigate straight to a fixture's editor,
//! and so a screenshot is byte-identical between runs of the same build.

// The interface is a library-shaped tree inside a binary crate. Its components,
// view models and fixtures are also compiled by `crates/app/tests/gui_app.rs`
// as a separate crate, so items that are used and tested there look unused from
// the binary's side. The allow is scoped to this module rather than the crate.
#![allow(dead_code)]
use std::collections::BTreeMap;
use std::path::PathBuf;

use chrono::{Duration, TimeZone, Utc};
use uuid::Uuid;

use superbackup_core::error::ErrorCode;
use superbackup_core::model::{
    BandwidthSettings, Destination, DestinationKind, EncryptionSettings, ExclusionSet, Job,
    JobHooks, ProviderKind, RetentionPolicy, S3Credentials, S3Flavour, Schedule, SecretRef,
    Source, StorageProvider, TimeOfDay,
};
use superbackup_core::state::{
    DestinationRun, Event, Health, JobRun, JobSummary, Progress, RunError, RunStatus, Severity,
    StatusSnapshot, Trigger,
};

use super::data::Data;

pub const JOB_DEV: Uuid = Uuid::from_u128(0x11110000_0000_4000_8000_000000000001);
pub const JOB_DOCS: Uuid = Uuid::from_u128(0x11110000_0000_4000_8000_000000000002);
pub const JOB_PHOTOS: Uuid = Uuid::from_u128(0x11110000_0000_4000_8000_000000000003);
pub const JOB_VM: Uuid = Uuid::from_u128(0x11110000_0000_4000_8000_000000000004);

pub const DEST_LOCAL: Uuid = Uuid::from_u128(0x22220000_0000_4000_8000_000000000001);
pub const DEST_ONEDRIVE: Uuid = Uuid::from_u128(0x22220000_0000_4000_8000_000000000002);
pub const DEST_S3: Uuid = Uuid::from_u128(0x22220000_0000_4000_8000_000000000003);
pub const DEST_MIRROR: Uuid = Uuid::from_u128(0x22220000_0000_4000_8000_000000000004);

pub const PROVIDER_STORJ: Uuid = Uuid::from_u128(0x33330000_0000_4000_8000_000000000001);
pub const PROVIDER_B2: Uuid = Uuid::from_u128(0x33330000_0000_4000_8000_000000000002);

pub const RUN_ACTIVE: Uuid = Uuid::from_u128(0x44440000_0000_4000_8000_000000000001);
pub const RUN_PARTIAL: Uuid = Uuid::from_u128(0x44440000_0000_4000_8000_000000000002);
pub const RUN_CLEAN: Uuid = Uuid::from_u128(0x44440000_0000_4000_8000_000000000003);

const PROJECT_WORK: Uuid = Uuid::from_u128(0x55550000_0000_4000_8000_000000000001);

fn path(windows: &str, unix: &str) -> PathBuf {
    PathBuf::from(if cfg!(windows) { windows } else { unix })
}

pub fn providers() -> Vec<StorageProvider> {
    vec![
        StorageProvider {
            id: PROVIDER_STORJ,
            name: "StorJ eu-1 (personal)".into(),
            kind: ProviderKind::S3 {
                endpoint: "https://gateway.storjshare.io".into(),
                region: "eu-1".into(),
                credentials: S3Credentials::for_provider(&PROVIDER_STORJ),
                tls: true,
                path_style: false,
                flavour: S3Flavour::Storj,
            },
            notes: "Offsite copies. Billed monthly.".into(),
            created_at: Utc::now() - Duration::days(120),
            last_verified_at: Some(Utc::now() - Duration::hours(30)),
        },
        StorageProvider {
            id: PROVIDER_B2,
            name: "Backblaze B2 (archive)".into(),
            kind: ProviderKind::S3 {
                endpoint: "https://s3.eu-central-003.backblazeb2.com".into(),
                region: "eu-central-003".into(),
                credentials: S3Credentials::for_provider(&PROVIDER_B2),
                tls: true,
                path_style: false,
                flavour: S3Flavour::BackblazeB2,
            },
            notes: String::new(),
            created_at: Utc::now() - Duration::days(40),
            last_verified_at: None,
        },
    ]
}

pub fn destinations() -> Vec<Destination> {
    vec![
        Destination {
            id: DEST_LOCAL,
            name: "Local repo".into(),
            kind: DestinationKind::LocalRepository {
                path: path(r"D:\superbackup\andreas-pc\repository", "/srv/backups/andreas-pc"),
            },
            encryption: Some(EncryptionSettings::default()),
            passphrase_ref: Some(SecretRef::new("repo", &DEST_LOCAL)),
            retention: RetentionPolicy::default(),
            enabled: true,
            auto_discovered: false,
            bandwidth: None,
            created_at: Utc::now() - Duration::days(120),
            last_verified_at: Some(Utc::now() - Duration::hours(2)),
        },
        Destination {
            id: DEST_ONEDRIVE,
            name: "OneDrive".into(),
            kind: DestinationKind::OneDrive {
                path: path(
                    r"C:\Users\andreas\OneDrive\superbackup",
                    "/home/andreas/OneDrive/superbackup",
                ),
                account: Some("andreas@example.com".into()),
            },
            encryption: Some(EncryptionSettings::default()),
            passphrase_ref: Some(SecretRef::new("repo", &DEST_ONEDRIVE)),
            retention: RetentionPolicy::default(),
            enabled: true,
            auto_discovered: true,
            bandwidth: None,
            created_at: Utc::now() - Duration::days(120),
            last_verified_at: Some(Utc::now() - Duration::days(2)),
        },
        Destination {
            id: DEST_S3,
            name: "StorJ offsite".into(),
            kind: DestinationKind::S3 {
                provider_id: PROVIDER_STORJ,
                bucket: "storj-backups".into(),
                prefix: "superbackup/andreas-pc-a3f9c2d1/".into(),
                credential_override: None,
            },
            encryption: Some(EncryptionSettings::default()),
            passphrase_ref: Some(SecretRef::new("repo", &DEST_S3)),
            retention: RetentionPolicy::default(),
            enabled: true,
            auto_discovered: false,
            bandwidth: Some(BandwidthSettings {
                upload_kbps: Some(2000),
                download_kbps: None,
                schedule: None,
            }),
            created_at: Utc::now() - Duration::days(110),
            last_verified_at: None,
        },
        Destination {
            id: DEST_MIRROR,
            name: "Desktop mirror".into(),
            kind: DestinationKind::LocalMirror {
                path: path(r"E:\mirror\documents", "/media/usb/mirror/documents"),
            },
            encryption: None,
            passphrase_ref: None,
            retention: RetentionPolicy::default(),
            enabled: true,
            auto_discovered: false,
            bandwidth: None,
            created_at: Utc::now() - Duration::days(30),
            last_verified_at: Some(Utc::now() - Duration::days(9)),
        },
    ]
}

pub fn jobs() -> Vec<Job> {
    let base = |id: Uuid, name: &str| Job {
        id,
        name: name.into(),
        project_id: None,
        description: String::new(),
        sources: vec![],
        destination_ids: vec![],
        schedule: Schedule::Daily { times: vec![TimeOfDay { hour: 2, minute: 0 }] },
        exclusions: ExclusionSet::default(),
        bandwidth: None,
        retention: None,
        enabled: true,
        timeout_minutes: None,
        hooks: JobHooks::default(),
        continue_on_destination_error: true,
        created_at: Utc::now() - Duration::days(120),
        tags: vec![],
    };

    let mut dev = base(JOB_DEV, "Dev code");
    dev.description = "Everything under the projects folder".into();
    dev.project_id = Some(PROJECT_WORK);
    dev.sources = vec![
        Source::new(path(r"C:\Users\andreas\dev", "/home/andreas/dev")),
        Source::new(path(r"C:\Users\andreas\source\repos", "/home/andreas/source/repos")),
    ];
    dev.destination_ids = vec![DEST_LOCAL, DEST_ONEDRIVE, DEST_S3];
    dev.exclusions = ExclusionSet::developer_defaults();
    dev.tags = vec!["work".into(), "code".into()];

    let mut docs = base(JOB_DOCS, "Documents");
    docs.sources = vec![
        Source::new(path(r"C:\Users\andreas\Documents", "/home/andreas/Documents")),
        Source::new(path(r"C:\Users\andreas\Desktop", "/home/andreas/Desktop")),
    ];
    docs.destination_ids = vec![DEST_LOCAL, DEST_ONEDRIVE, DEST_MIRROR];
    docs.schedule = Schedule::Daily {
        times: vec![TimeOfDay { hour: 9, minute: 0 }, TimeOfDay { hour: 18, minute: 0 }],
    };

    let mut photos = base(JOB_PHOTOS, "Photos");
    photos.sources = vec![Source::new(path(r"D:\Photos", "/home/andreas/Pictures"))];
    photos.destination_ids = vec![DEST_S3];
    photos.schedule = Schedule::Weekly {
        weekdays: vec![6],
        times: vec![TimeOfDay { hour: 3, minute: 30 }],
    };

    let mut vm = base(JOB_VM, "Scratch VM");
    vm.sources = vec![Source::new(path(r"D:\VMs", "/var/lib/libvirt/images"))];
    vm.destination_ids = vec![DEST_LOCAL];
    vm.enabled = false;
    vm.schedule = Schedule::Manual;

    vec![dev, docs, photos, vm]
}

fn destination_run(
    id: Uuid,
    name: &str,
    status: RunStatus,
    processed: u64,
    total: Option<u64>,
    uploaded: u64,
) -> DestinationRun {
    DestinationRun {
        destination_id: id,
        destination_name: name.into(),
        status,
        started_at: Some(Utc::now() - Duration::minutes(4)),
        finished_at: status.is_terminal().then(|| Utc::now() - Duration::seconds(20)),
        progress: Progress {
            files_processed: processed,
            files_total: Some(120_000),
            bytes_processed: (processed as f64 * 78_000.0) as u64,
            bytes_total: total,
            bytes_uploaded: uploaded,
            bytes_per_second: 19_100_000.0,
            files_cached: 71_090,
            errors_ignored: 0,
            current_path: None,
            estimated_seconds_remaining: (!status.is_terminal()).then_some(190),
        },
        snapshot_id: status
            .is_terminal()
            .then(|| "k9f2ab7c31de4f0a8c2b".to_string()),
        error: None,
        warnings: vec![],
    }
}

/// A job running to three destinations, at three different points — the shape
/// the whole product exists to make visible.
pub fn active_run() -> JobRun {
    let mut run = JobRun {
        run_id: RUN_ACTIVE,
        job_id: JOB_DEV,
        job_name: "Dev code".into(),
        trigger: Trigger::Schedule,
        status: RunStatus::Running,
        started_at: Utc::now() - Duration::seconds(252),
        finished_at: None,
        destinations: vec![
            destination_run(
                DEST_LOCAL,
                "Local repo",
                RunStatus::Running,
                85_200,
                Some(9_770_000_000),
                4_400_000_000,
            ),
            destination_run(
                DEST_ONEDRIVE,
                "OneDrive",
                RunStatus::Running,
                45_600,
                Some(9_770_000_000),
                2_360_000_000,
            ),
            destination_run(
                DEST_S3,
                "StorJ offsite",
                RunStatus::Succeeded,
                120_000,
                Some(9_770_000_000),
                883_000_000,
            ),
        ],
    };
    if let Some(first) = run.destinations.first_mut() {
        first.progress.current_path = Some(
            path(
                r"C:\Users\andreas\dev\web\src\components\Dashboard.tsx",
                "/home/andreas/dev/web/src/components/Dashboard.tsx",
            )
            .to_string_lossy()
            .into_owned(),
        );
    }
    run.status = run.derive_status();
    run
}

/// A finished run that succeeded to two destinations and failed at the third.
/// `derive_status` refuses to call this a success, and neither does the
/// interface.
pub fn partial_failure_run() -> JobRun {
    let mut failed = destination_run(
        DEST_S3,
        "StorJ offsite",
        RunStatus::Failed,
        18_400,
        Some(9_770_000_000),
        0,
    );
    failed.error = Some(RunError {
        code: ErrorCode::Kopia,
        message: "The endpoint answered, but rejected these credentials.".into(),
        hint: Some("Check the access key on StorJ eu-1 (personal), then verify the destination.".into()),
        detail: Some(
            "kopia: error connecting to repository: unable to list from the blob store: \
             The request signature we calculated does not match the signature you provided.\n\
             AccessDenied: status 403"
                .into(),
        ),
        occurred_at: Utc::now() - Duration::hours(6),
    });
    failed.snapshot_id = None;

    let mut warned = destination_run(
        DEST_ONEDRIVE,
        "OneDrive",
        RunStatus::SucceededWithWarnings,
        119_940,
        Some(9_770_000_000),
        2_360_000_000,
    );
    warned.progress.errors_ignored = 12;
    warned.warnings = vec![
        "unreadable file: C:\\Users\\andreas\\dev\\web\\.next\\cache\\swc\\plugin.bin".into(),
        "skipped a socket: /home/andreas/dev/tmp/agent.sock".into(),
    ];

    let mut run = JobRun {
        run_id: RUN_PARTIAL,
        job_id: JOB_DEV,
        job_name: "Dev code".into(),
        trigger: Trigger::Schedule,
        status: RunStatus::Failed,
        started_at: Utc::now() - Duration::hours(6),
        finished_at: Some(Utc::now() - Duration::hours(6) + Duration::seconds(252)),
        destinations: vec![
            destination_run(
                DEST_LOCAL,
                "Local repo",
                RunStatus::Succeeded,
                120_000,
                Some(9_770_000_000),
                4_400_000_000,
            ),
            warned,
            failed,
        ],
    };
    run.status = run.derive_status();
    run
}

pub fn history() -> Vec<JobRun> {
    let mut runs = vec![partial_failure_run()];
    for day in 0..6 {
        for (index, (job_id, name)) in
            [(JOB_DOCS, "Documents"), (JOB_DEV, "Dev code")].into_iter().enumerate()
        {
            let started = Utc::now() - Duration::days(day) - Duration::hours(index as i64 * 5 + 3);
            let mut run = JobRun {
                run_id: if day == 0 && index == 0 { RUN_CLEAN } else { Uuid::new_v4() },
                job_id,
                job_name: name.into(),
                trigger: Trigger::Schedule,
                status: RunStatus::Succeeded,
                started_at: started,
                finished_at: Some(started + Duration::seconds(62 + day * 11)),
                destinations: vec![
                    destination_run(
                        DEST_LOCAL,
                        "Local repo",
                        RunStatus::Succeeded,
                        84_000,
                        Some(2_400_000_000),
                        46_000_000,
                    ),
                    destination_run(
                        DEST_ONEDRIVE,
                        "OneDrive",
                        RunStatus::Succeeded,
                        84_000,
                        Some(2_400_000_000),
                        46_000_000,
                    ),
                ],
            };
            run.status = run.derive_status();
            runs.push(run);
        }
    }
    runs
}

pub fn events() -> Vec<Event> {
    let mut out = vec![
        Event::info("job.started", "Dev code started (Schedule)")
            .with_job(JOB_DEV)
            .with_run(RUN_ACTIVE),
        Event::warn(
            "dest.warning",
            "OneDrive finished with 12 unreadable files",
        )
        .with_job(JOB_DEV)
        .with_destination(DEST_ONEDRIVE),
        Event::error(
            "dest.failed",
            "StorJ offsite rejected the stored credentials",
        )
        .with_job(JOB_DEV)
        .with_destination(DEST_S3),
        Event::info("vault.unlocked", "The vault was unlocked"),
        Event::info("repo.maintenance", "Maintenance finished on Local repo")
            .with_destination(DEST_LOCAL),
        Event::new(Severity::Debug, "kopia.invoke", "kopia snapshot create (2 sources)"),
    ];
    for (index, event) in out.iter_mut().enumerate() {
        event.at = Utc::now() - Duration::minutes(index as i64 * 37);
    }
    out
}

pub fn summaries() -> BTreeMap<Uuid, JobSummary> {
    let mut map = BTreeMap::new();
    map.insert(
        JOB_DEV,
        JobSummary {
            last_run: Some(Utc::now() - Duration::hours(6)),
            last_success: Some(Utc::now() - Duration::hours(30)),
            last_status: Some(RunStatus::Failed),
            last_error: None,
            next_run: Some(Utc::now() + Duration::hours(4)),
            consecutive_failures: 2,
            total_runs: 412,
            last_uploaded_bytes: 883_000_000,
            average_duration_seconds: Some(252),
        },
    );
    map.insert(
        JOB_DOCS,
        JobSummary {
            last_run: Some(Utc::now() - Duration::hours(6)),
            last_success: Some(Utc::now() - Duration::hours(6)),
            last_status: Some(RunStatus::Succeeded),
            last_error: None,
            next_run: Some(Utc::now() + Duration::hours(3)),
            consecutive_failures: 0,
            total_runs: 390,
            last_uploaded_bytes: 46_100_000,
            average_duration_seconds: Some(62),
        },
    );
    map.insert(
        JOB_PHOTOS,
        JobSummary {
            last_run: Some(Utc::now() - Duration::days(5)),
            last_success: Some(Utc::now() - Duration::days(5)),
            last_status: Some(RunStatus::Succeeded),
            last_error: None,
            next_run: Some(Utc::now() + Duration::days(2)),
            consecutive_failures: 0,
            total_runs: 24,
            last_uploaded_bytes: 1_400_000_000,
            average_duration_seconds: Some(1_840),
        },
    );
    map.insert(
        JOB_VM,
        JobSummary {
            last_run: Some(Utc::now() - Duration::days(11)),
            last_success: Some(Utc::now() - Duration::days(11)),
            last_status: Some(RunStatus::Succeeded),
            last_error: None,
            next_run: None,
            consecutive_failures: 0,
            total_runs: 6,
            last_uploaded_bytes: 12_800_000_000,
            average_duration_seconds: Some(3_120),
        },
    );
    map
}

pub fn snapshot() -> StatusSnapshot {
    StatusSnapshot {
        health: Health::Running,
        version: superbackup_core::VERSION.to_string(),
        machine_label: "ANDREAS-PC".into(),
        machine_slug: "andreas-pc-a3f9c2d1".into(),
        unlocked: true,
        paused: false,
        paused_until: None,
        service_installed: true,
        service_running: true,
        kopia_version: Some("0.17.0".into()),
        active_runs: vec![active_run()],
        jobs: summaries(),
        next_scheduled: Some((JOB_DOCS, Utc::now() + chrono::Duration::hours(4))),
        recent_events: events(),
        uptime_seconds: 273_600,
        generated_at: Utc::now(),
    }
}

/// Fill a `Data` with the fixture machine.
pub fn seed(data: &mut Data) {
    data.snapshot = Some(snapshot());
    data.jobs = jobs();
    data.destinations = destinations();
    data.providers = providers();
    data.history = history();
    data.events = events();
    data.settings = superbackup_core::model::Settings::default();
    data.service = Some(superbackup_core::ipc::protocol::ServiceReply {
        installed: true,
        running: true,
        autostart: true,
        scope: "user".into(),
        detail: None,
    });
    data.version = Some(superbackup_core::ipc::protocol::VersionReply {
        version: superbackup_core::VERSION.to_string(),
        protocol: superbackup_core::ipc::PROTOCOL_VERSION,
        min_protocol: superbackup_core::ipc::MIN_PROTOCOL_VERSION,
        target_os: std::env::consts::OS.to_string(),
        target_arch: std::env::consts::ARCH.to_string(),
        kopia_version: Some("0.17.0".into()),
        service_scope: false,
    });
    data.link_up = true;
    data.loading = false;
}

/// The shapes a daemon should never send, which the interface must survive:
/// a destination pointing at a provider that does not exist, a run for a job
/// that has been deleted, a job referring to a destination that is gone, and
/// text where a name should be.
pub fn corrupt(data: &mut Data) {
    if let Some(d) = data.destinations.iter_mut().find(|d| d.id == DEST_S3) {
        if let DestinationKind::S3 { provider_id, .. } = &mut d.kind {
            *provider_id = Uuid::nil();
        }
    }
    if let Some(job) = data.jobs.iter_mut().find(|j| j.id == JOB_DOCS) {
        job.destination_ids.push(Uuid::nil());
        job.name = "A name that is very much longer than the sixty-four characters the editor allows for one".into();
    }
    if let Some(snapshot) = &mut data.snapshot {
        let mut orphan = active_run();
        orphan.run_id = Uuid::nil();
        orphan.job_id = Uuid::nil();
        orphan.job_name = String::new();
        orphan.destinations.clear();
        orphan.status = orphan.derive_status();
        snapshot.active_runs.push(orphan);
        snapshot.jobs.insert(Uuid::nil(), JobSummary::default());
        snapshot.machine_label = String::new();
        snapshot.kopia_version = None;
        snapshot.next_scheduled = Some((
            Uuid::nil(),
            Utc.timestamp_opt(0, 0).single().unwrap_or_else(Utc::now),
        ));
    }
    data.providers.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_active_run_shows_three_destinations_at_three_points() {
        let run = active_run();
        assert_eq!(run.destinations.len(), 3);
        assert_eq!(run.status, RunStatus::Running);
        assert!(run.overall_fraction().is_some());
    }

    #[test]
    fn the_partial_run_is_a_failure_not_a_success() {
        let run = partial_failure_run();
        assert_eq!(run.status, RunStatus::Failed);
        assert_eq!(run.derive_status(), RunStatus::Failed);
        assert!(run.destinations.iter().any(|d| d.status == RunStatus::Succeeded));
        assert!(run.destinations.iter().any(|d| d.error.is_some()));
    }

    #[test]
    fn the_fixture_machine_is_internally_consistent() {
        let mut data = Data::new();
        seed(&mut data);
        for job in &data.jobs {
            for id in &job.destination_ids {
                assert!(data.destination(id).is_some(), "{} names a missing destination", job.name);
            }
        }
        for destination in &data.destinations {
            if let Some(provider) = destination.kind.provider_id() {
                assert!(data.provider(provider).is_some());
            }
        }
    }

    #[test]
    fn corrupting_the_fixture_breaks_exactly_the_links_the_tests_need() {
        let mut data = Data::new();
        seed(&mut data);
        corrupt(&mut data);
        assert!(data.providers.is_empty());
        assert!(data.destination(&Uuid::nil()).is_none());
        assert!(data.jobs.iter().any(|j| j.destination_ids.contains(&Uuid::nil())));
    }
}
