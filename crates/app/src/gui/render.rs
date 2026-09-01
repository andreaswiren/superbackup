//! An offscreen rasteriser, so the interface can be looked at rather than
//! merely compiled.
//!
//! egui is a tessellator: `Context::run` produces triangle meshes and a texture
//! atlas, and a backend turns those into pixels. This module is a small
//! software backend — barycentric triangle fill, bilinear texture sampling, and
//! premultiplied blending in linear space, which is what `egui_glow`'s shader
//! does — so a screenshot can be produced with no window, no GPU and no
//! display server.
//!
//! It exists for the design review: `cargo test -p superbackup --test gui_app
//! -- --ignored screenshots` writes `design/screenshots/`.

// The interface is a library-shaped tree inside a binary crate. Its components,
// view models and fixtures are also compiled by `crates/app/tests/gui_app.rs`
// as a separate crate, so items that are used and tested there look unused from
// the binary's side. The allow is scoped to this module rather than the crate.
#![allow(dead_code)]
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use egui::{Color32, Context, Pos2, Rect, Vec2};

use super::app::App;
use super::daemon::MockDaemon;
use super::fixtures;
use super::nav::{Route, SettingsSection};

/// A bucket listing for the gallery.
///
/// `listed` false with `credentials_ok` true is the scoped-key case: the
/// endpoint verified the signature and then declined to enumerate buckets.
fn sample_buckets(
    listed: bool,
    credentials_ok: bool,
) -> superbackup_core::ipc::protocol::BucketsReply {
    use superbackup_core::ipc::protocol::{BucketInfo, BucketsReply};
    BucketsReply {
        provider_id: fixtures::PROVIDER_STORJ,
        buckets: if listed {
            // `storj-backups` is the fixture destination's own bucket, so the
            // picker reads as "this one is selected" rather than as a list
            // that happens not to contain what the field says.
            ["storj-backups", "dev-backups", "photos-archive", "scratch"]
                .into_iter()
                .map(|name| BucketInfo { name: name.into(), created_at: None })
                .collect()
        } else {
            Vec::new()
        },
        listed,
        credentials_ok,
        detail: (!listed).then(|| {
            "The credentials were accepted, but this key is not allowed to list the buckets \
             in this account. That is normal for a key scoped to a single bucket, and it \
             does not mean the key is wrong."
                .to_string()
        }),
        latency_ms: Some(96),
    }
}

fn sample_objects(holds_repository: bool) -> superbackup_core::ipc::protocol::ObjectsReply {
    use superbackup_core::ipc::protocol::{ObjectInfo, ObjectsReply};
    ObjectsReply {
        bucket: "dev-backups".into(),
        prefix: "superbackup/andreas-pc/".into(),
        keys: vec![ObjectInfo {
            key: "superbackup/andreas-pc/kopia.repository".into(),
            size: 661,
            last_modified: None,
        }],
        truncated: false,
        holds_repository,
        listed: true,
        detail: None,
    }
}

/// One image in the gallery.
pub struct Shot {
    pub name: &'static str,
    pub size: [f32; 2],
    pub dark: bool,
    /// Put the window into the state this shot is about.
    pub setup: fn(&mut App),
    /// Applied again *after* the first frame.
    ///
    /// Editors load their draft from `Data` on their first frame and reset
    /// their per-screen flags while doing it, so anything `setup` sets that an
    /// editor owns is wiped before it is ever drawn. This hook runs on the far
    /// side of that, which is the only place a "the panel is open" or "the
    /// list is expanded" state can be established for a screenshot.
    pub refine: Option<fn(&mut App)>,
}

/// The screens the review looks at, in the order it looks at them.
pub fn gallery() -> Vec<Shot> {
    vec![
        Shot {
            name: "01-dashboard-dark",
            size: [1100.0, 720.0],
            dark: true,
            refine: None,
            setup: |app| app.go(Route::Dashboard),
        },
        Shot {
            name: "02-dashboard-light",
            size: [1100.0, 720.0],
            dark: false,
            refine: None,
            setup: |app| app.go(Route::Dashboard),
        },
        Shot {
            name: "03-dashboard-locked",
            size: [1100.0, 720.0],
            dark: true,
            refine: None,
            setup: |app| {
                if let Some(s) = &mut app.data.snapshot {
                    s.unlocked = false;
                    s.health = superbackup_core::state::Health::Attention;
                    s.active_runs.clear();
                }
                app.go(Route::Dashboard);
            },
        },
        Shot {
            name: "04-dashboard-900",
            size: [900.0, 600.0],
            dark: true,
            refine: None,
            setup: |app| app.go(Route::Dashboard),
        },
        Shot {
            name: "05-jobs",
            size: [1100.0, 720.0],
            dark: true,
            refine: None,
            setup: |app| app.go(Route::Jobs),
        },
        Shot {
            name: "06-job-editor-destinations",
            size: [1100.0, 720.0],
            dark: true,
            refine: None,
            setup: |app| {
                app.screens.job_editor.open_tab(1);
                app.go(Route::JobEditor(fixtures::JOB_DEV));
            },
        },
        Shot {
            name: "07-job-editor-exclusions",
            size: [1100.0, 720.0],
            dark: true,
            refine: None,
            setup: |app| {
                app.screens.job_editor.open_tab(3);
                app.go(Route::JobEditor(fixtures::JOB_DEV));
            },
        },
        Shot {
            name: "08-job-editor-schedule",
            size: [1100.0, 720.0],
            dark: false,
            refine: None,
            setup: |app| {
                app.screens.job_editor.open_tab(2);
                app.go(Route::JobEditor(fixtures::JOB_DEV));
            },
        },
        Shot {
            name: "09-destinations",
            size: [1100.0, 720.0],
            dark: true,
            refine: None,
            setup: |app| app.go(Route::Destinations),
        },
        Shot {
            name: "10-destination-editor-s3",
            size: [1100.0, 720.0],
            dark: true,
            refine: None,
            setup: |app| app.go(Route::DestinationEditor(fixtures::DEST_S3)),
        },
        Shot {
            name: "11-destination-editor-mirror",
            size: [1100.0, 720.0],
            dark: true,
            refine: None,
            setup: |app| app.go(Route::DestinationEditor(fixtures::DEST_MIRROR)),
        },
        Shot {
            name: "12-encryption-panel",
            size: [1100.0, 720.0],
            dark: true,
            setup: |app| app.go(Route::NewDestination),
            // `load` clears `encryption_open` on the editor's first frame, so
            // setting it before that frame set it on a draft that was then
            // thrown away — this shot has been of a *closed* panel.
            refine: Some(|app| app.screens.destination_editor.encryption_open = true),
        },
        Shot {
            name: "13-providers",
            size: [1100.0, 720.0],
            dark: true,
            refine: None,
            setup: |app| app.go(Route::Providers),
        },
        Shot {
            name: "14-provider-editor",
            size: [1100.0, 720.0],
            dark: false,
            refine: None,
            setup: |app| app.go(Route::ProviderEditor(fixtures::PROVIDER_STORJ)),
        },
        Shot {
            name: "14b-provider-editor-tested",
            // Tall on purpose: the panel this shot is about sits below the
            // fold of a 720-pixel window, and a review screenshot that shows
            // only the top of the form reviews nothing.
            size: [1100.0, 1800.0],
            dark: true,
            setup: |app| app.go(Route::ProviderEditor(fixtures::PROVIDER_STORJ)),
            // After the editor has loaded its draft, or the load resets it.
            refine: Some(|app| {
                // The outcome the whole feature exists to produce: the
                // credentials were accepted, and here is what they can see.
                app.screens
                    .provider_editor
                    .probe(fixtures::PROVIDER_STORJ, &sample_buckets(true, true));
            }),
        },
        Shot {
            name: "14c-provider-editor-scoped-key",
            // Tall on purpose: the panel this shot is about sits below the
            // fold of a 720-pixel window, and a review screenshot that shows
            // only the top of the form reviews nothing.
            size: [1100.0, 1800.0],
            dark: true,
            setup: |app| app.go(Route::ProviderEditor(fixtures::PROVIDER_STORJ)),
            refine: Some(|app| {
                // The case that must not read as a failure: a key that is
                // correct and simply may not enumerate buckets.
                app.screens
                    .provider_editor
                    .probe(fixtures::PROVIDER_STORJ, &sample_buckets(false, true));
            }),
        },
        Shot {
            name: "10b-destination-editor-bucket-picker",
            // Tall on purpose: the panel this shot is about sits below the
            // fold of a 720-pixel window, and a review screenshot that shows
            // only the top of the form reviews nothing.
            size: [1100.0, 1800.0],
            dark: true,
            setup: |app| app.go(Route::DestinationEditor(fixtures::DEST_S3)),
            refine: Some(|app| {
                app.screens
                    .destination_editor
                    .buckets_arrived(fixtures::PROVIDER_STORJ, &sample_buckets(true, true));
                app.screens
                    .destination_editor
                    .objects_arrived(fixtures::PROVIDER_STORJ, &sample_objects(true));
            }),
        },
        Shot {
            name: "10c-destination-editor-bucket-typing",
            // Tall on purpose: the panel this shot is about sits below the
            // fold of a 720-pixel window, and a review screenshot that shows
            // only the top of the form reviews nothing.
            size: [1100.0, 1800.0],
            dark: true,
            setup: |app| app.go(Route::DestinationEditor(fixtures::DEST_S3)),
            // Listing unavailable — offline, or a scoped key. Typing must
            // still be fully available, and nothing may be blocked.
            refine: Some(|app| {
                app.screens
                    .destination_editor
                    .buckets_arrived(fixtures::PROVIDER_STORJ, &sample_buckets(false, true));
            }),
        },
        Shot {
            name: "15-activity",
            size: [1100.0, 720.0],
            dark: true,
            refine: None,
            setup: |app| {
                app.screens.activity.range = super::viewmodel::TimeRange::All;
                app.go(Route::Activity);
            },
        },
        Shot {
            name: "16-run-detail-partial-failure",
            size: [1100.0, 720.0],
            dark: true,
            refine: None,
            setup: |app| {
                app.screens.run_detail.expanded_details = Some(fixtures::DEST_S3);
                app.go(Route::RunDetail(fixtures::RUN_PARTIAL));
            },
        },
        Shot {
            name: "17-restore",
            size: [1100.0, 720.0],
            dark: true,
            refine: None,
            setup: |app| {
                app.screens.restore.select(fixtures::DEST_LOCAL);
                app.screens.restore.snapshots_arrived(fixtures::DEST_LOCAL, sample_snapshots());
                app.go(Route::Restore);
            },
        },
        Shot {
            name: "18-restore-browser",
            size: [1100.0, 720.0],
            dark: true,
            refine: None,
            setup: |app| {
                app.screens.restore.select(fixtures::DEST_LOCAL);
                app.screens.restore.snapshots_arrived(fixtures::DEST_LOCAL, sample_snapshots());
                app.screens.restore.selected_snapshot = Some("k9f2ab7c31de4f0a".into());
                app.screens.restore.listing_arrived(
                    fixtures::DEST_LOCAL,
                    "Users/andreas/dev".into(),
                    sample_listing(),
                );
                app.screens.restore.selection =
                    vec!["Users/andreas/dev/web".into(), "Users/andreas/dev/api".into()];
                app.go(Route::Restore);
            },
        },
        Shot {
            name: "19-restore-locked",
            size: [1100.0, 720.0],
            dark: true,
            refine: None,
            setup: |app| {
                if let Some(s) = &mut app.data.snapshot {
                    s.unlocked = false;
                }
                app.go(Route::Restore);
            },
        },
        Shot {
            name: "20-settings-general",
            size: [1100.0, 720.0],
            dark: true,
            refine: None,
            setup: |app| app.go(Route::Settings(SettingsSection::General)),
        },
        Shot {
            name: "21-settings-security",
            size: [1100.0, 720.0],
            dark: true,
            refine: None,
            setup: |app| app.go(Route::Settings(SettingsSection::Security)),
        },
        Shot {
            name: "22-settings-bandwidth",
            size: [1100.0, 720.0],
            dark: false,
            refine: None,
            setup: |app| {
                app.data.settings.bandwidth.upload_kbps = Some(2000);
                app.data.settings.bandwidth.schedule =
                    Some(superbackup_core::model::BandwidthWindow {
                        start_minute: 9 * 60,
                        end_minute: 18 * 60,
                        upload_kbps: Some(500),
                        download_kbps: None,
                        weekdays: vec![0, 1, 2, 3, 4],
                    });
                app.go(Route::Settings(SettingsSection::Bandwidth));
            },
        },
        Shot {
            name: "23-about",
            size: [1100.0, 720.0],
            dark: true,
            refine: None,
            setup: |app| app.go(Route::About),
        },
        Shot {
            name: "24-unlock-modal",
            size: [1100.0, 720.0],
            dark: true,
            refine: None,
            setup: |app| {
                if let Some(s) = &mut app.data.snapshot {
                    s.unlocked = false;
                }
                app.go(Route::Dashboard);
                app.open_modal(
                    super::modals::Modal::Unlock(super::modals::UnlockState::blocking()),
                );
            },
        },
        Shot {
            name: "25-wizard-template",
            size: [1100.0, 720.0],
            dark: true,
            refine: None,
            setup: |app| {
                app.go(Route::Jobs);
                let wizard = super::screens::wizard::WizardState::new(&app.data);
                app.open_modal(super::modals::Modal::Wizard(Box::new(wizard)));
            },
        },
        Shot {
            name: "26-write-it-down",
            size: [1100.0, 720.0],
            dark: true,
            refine: None,
            setup: |app| {
                app.go(Route::Destinations);
                app.open_modal(super::modals::Modal::WriteDown(super::modals::WriteDownState {
                    destination: fixtures::DEST_LOCAL,
                    location: "D:\\superbackup\\andreas-pc\\repository".into(),
                    passphrase: "kX7fQ2mNbR4tYw8ZaP1sDv6HgJ3eLc0UqT5xWn9BiM2r".into(),
                    acknowledged: false,
                    copied: false,
                }));
            },
        },
        Shot {
            name: "27-remove-destination-confirm",
            size: [1100.0, 720.0],
            dark: true,
            refine: None,
            setup: |app| {
                app.go(Route::Destinations);
                let confirm =
                    super::modals::remove_destination_confirm(&app.data, fixtures::DEST_LOCAL);
                app.open_modal(super::modals::Modal::Confirm(confirm));
            },
        },
        Shot {
            name: "28-onboarding-passphrase",
            size: [880.0, 640.0],
            dark: true,
            refine: None,
            setup: |app| {
                app.begin_onboarding();
                app.onboarding_goto(super::validation::OnboardingStep::Passphrase);
                if let Some(o) = &mut app.onboarding {
                    o.passphrase = "correct-horse-battery-staple".into();
                    o.confirm = "correct-horse-battery-staple".into();
                }
            },
        },
        Shot {
            name: "29-onboarding-no-recovery",
            size: [880.0, 640.0],
            dark: true,
            refine: None,
            setup: |app| {
                app.begin_onboarding();
                app.onboarding_goto(super::validation::OnboardingStep::NoRecovery);
                if let Some(o) = &mut app.onboarding {
                    o.passphrase = "correct-horse-battery-staple".into();
                }
            },
        },
        Shot {
            name: "30-onboarding-welcome",
            size: [880.0, 640.0],
            dark: false,
            refine: None,
            setup: |app| {
                app.begin_onboarding();
                app.onboarding_goto(super::validation::OnboardingStep::Welcome);
            },
        },
        Shot {
            name: "31-empty-dashboard",
            size: [1100.0, 720.0],
            dark: true,
            refine: None,
            setup: |app| {
                app.data.jobs.clear();
                if let Some(s) = &mut app.data.snapshot {
                    s.active_runs.clear();
                    s.jobs.clear();
                }
                app.go(Route::Dashboard);
            },
        },
        Shot {
            name: "33-settings-kopia",
            size: [1100.0, 720.0],
            dark: true,
            refine: None,
            setup: |app| {
                app.screens.settings.kopia_probe = Some(sample_kopia_probe());
                app.go(Route::Settings(SettingsSection::Kopia));
            },
        },
        Shot {
            name: "34-settings-kopia-light",
            size: [1100.0, 720.0],
            dark: false,
            refine: None,
            setup: |app| {
                app.screens.settings.kopia_probe = Some(sample_kopia_probe());
                app.go(Route::Settings(SettingsSection::Kopia));
            },
        },
        Shot {
            name: "35-job-preview",
            size: [1100.0, 720.0],
            dark: true,
            refine: None,
            setup: |app| {
                let run = sample_preview_run();
                let run_id = run.run_id;
                app.data.history.insert(0, run);
                app.screens.preview.started(fixtures::JOB_DEV, "Dev folders".into());
                app.screens.preview.accepted(fixtures::JOB_DEV, run_id);
                app.go(Route::Preview(fixtures::JOB_DEV));
            },
        },
        Shot {
            name: "36-export-encryption-keys",
            size: [1100.0, 720.0],
            dark: true,
            refine: None,
            setup: |app| {
                app.go(Route::Settings(SettingsSection::Security));
                app.open_modal(super::modals::Modal::ExportKeys(
                    super::modals::ExportKeysState::default(),
                ));
            },
        },
        Shot {
            name: "37-export-encryption-keys-ready",
            size: [1100.0, 720.0],
            dark: true,
            refine: None,
            setup: |app| {
                app.go(Route::Settings(SettingsSection::Security));
                let mut state = super::modals::ExportKeysState::default();
                state.document_arrived(sample_key_export());
                app.open_modal(super::modals::Modal::ExportKeys(state));
            },
        },
        Shot {
            name: "38-destination-key-check",
            // Taller than the rest on purpose: the encryption panel this shot
            // is about sits below the folder and validation sections.
            size: [1100.0, 1500.0],
            dark: true,
            refine: None,
            setup: |app| {
                app.screens.destination_editor.key_check = Some((
                    fixtures::DEST_LOCAL,
                    super::screens::destination_editor::KeyCheckOutcome::Opened,
                ));
                app.go(Route::DestinationEditor(fixtures::DEST_LOCAL));
            },
        },
        Shot {
            name: "32-daemon-unreachable",
            size: [1100.0, 720.0],
            dark: true,
            refine: None,
            setup: |app| {
                app.data.link_up = false;
                if let Some(s) = &mut app.data.snapshot {
                    s.active_runs.clear();
                }
                app.go(Route::Jobs);
            },
        },
    ]
}

/// A kopia probe that found a system binary and ran both commands.
fn sample_kopia_probe() -> superbackup_core::ipc::protocol::KopiaProbeReply {
    use superbackup_core::ipc::protocol::{
        KopiaInvocation, KopiaProbeReply, KopiaProvenance, KopiaRoute,
    };
    let exe =
        if cfg!(windows) { r"C:\Program Files\kopia\kopia.exe" } else { "/usr/local/bin/kopia" };
    KopiaProbeReply {
        path: Some(exe.to_string()),
        provenance: KopiaProvenance::SystemPath,
        version: Some("0.21.1".into()),
        banner: Some("0.21.1 build: 8f0e1c2d from: kopia/kopia".into()),
        routes: vec![
            KopiaRoute {
                provenance: KopiaProvenance::Configured,
                path: None,
                outcome: "No path is pinned in Settings, so discovery continues.".into(),
                chosen: false,
            },
            KopiaRoute {
                provenance: KopiaProvenance::SystemPath,
                path: Some(exe.to_string()),
                outcome: "Found on PATH, and preferred over the managed build.".into(),
                chosen: true,
            },
            KopiaRoute {
                provenance: KopiaProvenance::Bundled,
                path: Some(if cfg!(windows) {
                    r"C:\Users\andreas\AppData\Local\superbackup\kopia\kopia.exe".into()
                } else {
                    "/home/andreas/.local/share/superbackup/kopia/kopia".to_string()
                }),
                outcome: "Not installed yet.".into(),
                chosen: false,
            },
        ],
        managed_path: if cfg!(windows) {
            r"C:\Users\andreas\AppData\Local\superbackup\kopia\kopia.exe".into()
        } else {
            "/home/andreas/.local/share/superbackup/kopia/kopia".to_string()
        },
        managed_version: None,
        update_policy: "notify".into(),
        update_available: None,
        update_summary: Some("kopia 0.21.1 is up to date.".into()),
        minimum_version: "0.17.0".into(),
        invocations: vec![
            KopiaInvocation {
                label: "--version".into(),
                command_line: format!("\"{exe}\" --version"),
                secret_env: vec![],
                exit_code: Some(0),
                stdout: "0.21.1 build: 8f0e1c2d from: kopia/kopia".into(),
                stderr: String::new(),
                duration_ms: 38,
                ok: true,
            },
            KopiaInvocation {
                label: "repository status".into(),
                command_line: format!(
                    "\"{exe}\" --config-file=repository.config --log-level=warning \
                     --persist-credentials=false repository status --json"
                ),
                secret_env: vec!["KOPIA_PASSWORD".into()],
                exit_code: Some(0),
                stdout: "{\n  \"configFile\": \"repository.config\",\n  \"uniqueId\": \
                         \"5f2a9c31de4f0a8c\",\n  \"hash\": \"BLAKE2B-256-128\",\n  \
                         \"encryption\": \"AES256-GCM-HMAC-SHA256\",\n  \"splitter\": \
                         \"DYNAMIC-4M-BUZHASH\",\n  \"formatVersion\": \"3\"\n}"
                    .into(),
                stderr: String::new(),
                duration_ms: 412,
                ok: true,
            },
        ],
        detail: None,
    }
}

/// A finished rehearsal over three destinations, including one that could not
/// be rehearsed — the case the screen must not round off.
fn sample_preview_run() -> superbackup_core::state::JobRun {
    use superbackup_core::state::{DestinationRun, JobRun, Progress, RunError, RunStatus, Trigger};
    let destination = |id, name: &str, status, progress, error: Option<&str>| DestinationRun {
        destination_id: id,
        destination_name: name.to_string(),
        status,
        started_at: Some(chrono::Utc::now() - chrono::Duration::seconds(40)),
        finished_at: Some(chrono::Utc::now()),
        progress,
        snapshot_id: None,
        error: error.map(|message| RunError {
            code: superbackup_core::error::ErrorCode::RepoNotConnected,
            message: message.to_string(),
            hint: None,
            detail: None,
            occurred_at: chrono::Utc::now(),
        }),
        warnings: vec![],
        replicated_from: None,
        skipped_reason: None,
    };
    JobRun {
        run_id: uuid::Uuid::new_v4(),
        job_id: fixtures::JOB_DEV,
        job_name: "Dev folders".into(),
        trigger: Trigger::Preview,
        status: RunStatus::SucceededWithWarnings,
        started_at: chrono::Utc::now() - chrono::Duration::seconds(40),
        finished_at: Some(chrono::Utc::now()),
        destinations: vec![
            destination(
                fixtures::DEST_LOCAL,
                "Local repository",
                RunStatus::Succeeded,
                Progress {
                    files_processed: 118_402,
                    files_total: Some(118_402),
                    bytes_processed: 9_770_000_000,
                    bytes_total: Some(9_770_000_000),
                    ..Progress::default()
                },
                None,
            ),
            destination(
                fixtures::DEST_MIRROR,
                "External drive mirror",
                RunStatus::Succeeded,
                Progress {
                    files_processed: 118_402,
                    files_cached: 112_884,
                    bytes_processed: 9_770_000_000,
                    bytes_uploaded: 431_000_000,
                    ..Progress::default()
                },
                None,
            ),
            destination(
                fixtures::DEST_S3,
                "StorJ offsite",
                RunStatus::Failed,
                Progress::default(),
                Some("There is no repository at \"StorJ offsite\" yet."),
            ),
        ],
    }
}

fn sample_key_export() -> superbackup_core::ipc::protocol::KeyExportReply {
    superbackup_core::ipc::protocol::KeyExportReply {
        document: "SUPERBACKUP - REPOSITORY ENCRYPTION KEYS\n\
                   =========================================\n\n\
                   READ THIS FIRST\n\n\
                   This file contains the encryption keys for the backups made by the computer\n\
                   named below. Anyone who has this file AND can reach the storage listed in it\n\
                   can read every file in those backups. Treat it exactly as you would treat the\n\
                   backed-up files themselves: a locked drawer, a safe, or a password manager.\n"
            .into(),
        destinations: 2,
        omitted: vec![
            "External drive mirror: a folder mirror holds plain copies and has no encryption key"
                .into(),
        ],
        suggested_file_name: "superbackup-encryption-keys-andreas-pc-20260831.txt".into(),
        generated_at: chrono::Utc::now(),
    }
}

fn sample_snapshots() -> Vec<superbackup_core::ipc::protocol::SnapshotInfo> {
    use chrono::{Duration, Utc};
    (0..24)
        .map(|i| superbackup_core::ipc::protocol::SnapshotInfo {
            id: format!("k9f2ab7c31de4f0a{i:04x}"),
            destination_id: fixtures::DEST_LOCAL,
            job_id: Some(fixtures::JOB_DEV),
            created_at: Utc::now() - Duration::hours(i * 7),
            source_path: if cfg!(windows) {
                r"C:\Users\andreas\dev".to_string()
            } else {
                "/home/andreas/dev".to_string()
            },
            file_count: Some(120_000 - i as u64 * 37),
            total_bytes: Some(9_770_000_000 - i as u64 * 1_000_000),
            incomplete: false,
            tags: vec![],
        })
        .collect()
}

fn sample_listing() -> superbackup_core::ipc::protocol::ListingReply {
    use chrono::{Duration, Utc};
    use superbackup_core::ipc::protocol::{EntryKind, ListingReply, SnapshotEntry};
    let entry = |name: &str, kind: EntryKind, size: u64, age: i64| SnapshotEntry {
        name: name.to_string(),
        kind,
        size_bytes: size,
        modified_at: Some(Utc::now() - Duration::hours(age)),
        object_id: None,
    };
    ListingReply {
        path: "Users/andreas/dev".into(),
        entries: vec![
            entry("api", EntryKind::Directory, 0, 5),
            entry("web", EntryKind::Directory, 0, 2),
            entry("scripts", EntryKind::Directory, 0, 48),
            entry("infra", EntryKind::Directory, 0, 120),
            entry("README.md", EntryKind::File, 4_812, 30),
            entry("Cargo.toml", EntryKind::File, 1_204, 30),
            entry("docker-compose.yml", EntryKind::File, 2_940, 96),
            entry("notes.txt", EntryKind::File, 812, 5),
            entry(".env.example", EntryKind::File, 402, 400),
            entry("design.excalidraw", EntryKind::File, 1_204_882, 72),
        ],
        truncated: false,
    }
}

/// Render every shot into `dir`.
pub fn write_gallery(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    for shot in gallery() {
        let image = capture(&shot);
        let path = dir.join(format!("{}.png", shot.name));
        image.save(&path).map_err(|e| std::io::Error::other(format!("{}: {e}", path.display())))?;
    }
    Ok(())
}

/// Lay out and rasterise one shot at 2× for a legible screenshot.
pub fn capture(shot: &Shot) -> image::RgbaImage {
    let ctx = Context::default();
    // `Visuals::dark_mode` is what the window reads as "the OS theme".
    ctx.set_visuals(if shot.dark { egui::Visuals::dark() } else { egui::Visuals::light() });

    let handler = Arc::new(superbackup_core::ipc::testing::MockHandler::new());
    let mut app = App::new_with_daemon(&ctx, Arc::new(MockDaemon::new(handler)));
    fixtures::seed(&mut app.data);
    app.data.settings.theme = if shot.dark {
        superbackup_core::model::Theme::Dark
    } else {
        superbackup_core::model::Theme::Light
    };
    app.preview_mode();
    (shot.setup)(&mut app);
    let mut refined = shot.refine.is_none();

    let pixels_per_point = 2.0;
    let width = (shot.size[0] * pixels_per_point) as usize;
    let height = (shot.size[1] * pixels_per_point) as usize;
    let mut canvas = Canvas::new(width, height);
    let mut output = None;
    // Several frames: fonts, layout and the animation clock all settle. The
    // font atlas is built incrementally, so every frame's texture delta is
    // applied — keeping only the last one would leave most glyphs unrasterised.
    for _ in 0..4 {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                Pos2::ZERO,
                Vec2::new(shot.size[0], shot.size[1]),
            )),
            viewports: std::iter::once((
                egui::ViewportId::ROOT,
                egui::ViewportInfo {
                    native_pixels_per_point: Some(pixels_per_point),
                    ..Default::default()
                },
            ))
            .collect(),
            ..Default::default()
        };
        ctx.set_pixels_per_point(pixels_per_point);
        let frame = ctx.run(input, |ctx| app.frame(ctx));
        canvas.textures.apply(&frame.textures_delta);
        output = Some(frame);
        if !refined {
            if let Some(refine) = shot.refine {
                refine(&mut app);
            }
            refined = true;
        }
    }

    let output = output.expect("at least one frame was run");
    let primitives = ctx.tessellate(output.shapes, pixels_per_point);
    canvas.draw(&primitives, pixels_per_point);
    canvas.into_image()
}

// ---------------------------------------------------------------------------
// The software backend
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Textures {
    images: HashMap<egui::TextureId, Texture>,
}

struct Texture {
    width: usize,
    height: usize,
    pixels: Vec<Color32>,
}

impl Textures {
    fn apply(&mut self, delta: &egui::TexturesDelta) {
        for (id, image) in &delta.set {
            let (size, pixels): ([usize; 2], Vec<Color32>) = match &image.image {
                egui::ImageData::Color(colour) => (colour.size, colour.pixels.clone()),
                egui::ImageData::Font(font) => (font.size, font.srgba_pixels(None).collect()),
            };
            match image.pos {
                None => {
                    self.images.insert(*id, Texture { width: size[0], height: size[1], pixels });
                }
                Some([x, y]) => {
                    if let Some(existing) = self.images.get_mut(id) {
                        for row in 0..size[1] {
                            for column in 0..size[0] {
                                let target = (y + row) * existing.width + (x + column);
                                if let Some(slot) = existing.pixels.get_mut(target) {
                                    *slot = pixels[row * size[0] + column];
                                }
                            }
                        }
                    }
                }
            }
        }
        for id in &delta.free {
            self.images.remove(id);
        }
    }

    fn sample(&self, id: egui::TextureId, uv: Pos2) -> [f32; 4] {
        let Some(texture) = self.images.get(&id) else {
            return [1.0, 1.0, 1.0, 1.0];
        };
        if texture.width == 0 || texture.height == 0 {
            return [1.0, 1.0, 1.0, 1.0];
        }
        // Bilinear, which is what the GPU backends use for the font atlas.
        let x = (uv.x * texture.width as f32 - 0.5).clamp(0.0, texture.width as f32 - 1.0);
        let y = (uv.y * texture.height as f32 - 0.5).clamp(0.0, texture.height as f32 - 1.0);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(texture.width - 1), (y0 + 1).min(texture.height - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let at = |cx: usize, cy: usize| linear(texture.pixels[cy * texture.width + cx]);
        let (a, b, c, d) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
        let mut out = [0.0; 4];
        for i in 0..4 {
            let top = a[i] + (b[i] - a[i]) * fx;
            let bottom = c[i] + (d[i] - c[i]) * fx;
            out[i] = top + (bottom - top) * fy;
        }
        out
    }
}

/// sRGB byte → linear float, matching what egui's shaders do to the
/// premultiplied vertex and texture colours.
fn linear(colour: Color32) -> [f32; 4] {
    let f = |v: u8| {
        let s = v as f32 / 255.0;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    [f(colour.r()), f(colour.g()), f(colour.b()), colour.a() as f32 / 255.0]
}

fn to_srgb(value: f32) -> u8 {
    let v = value.clamp(0.0, 1.0);
    let s = if v <= 0.0031308 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
    (s * 255.0).round().clamp(0.0, 255.0) as u8
}

struct Canvas {
    width: usize,
    height: usize,
    /// Linear, premultiplied.
    pixels: Vec<[f32; 4]>,
    textures: Textures,
}

impl Canvas {
    fn new(width: usize, height: usize) -> Canvas {
        Canvas {
            width,
            height,
            pixels: vec![[0.0, 0.0, 0.0, 0.0]; width * height],
            textures: Textures::default(),
        }
    }

    fn draw(&mut self, primitives: &[egui::ClippedPrimitive], pixels_per_point: f32) {
        for primitive in primitives {
            let clip = primitive.clip_rect;
            let clip = Rect::from_min_max(
                Pos2::new(clip.min.x * pixels_per_point, clip.min.y * pixels_per_point),
                Pos2::new(clip.max.x * pixels_per_point, clip.max.y * pixels_per_point),
            );
            match &primitive.primitive {
                egui::epaint::Primitive::Mesh(mesh) => {
                    self.mesh(mesh, clip, pixels_per_point);
                }
                // Callbacks are GPU-only; nothing in this interface uses one.
                egui::epaint::Primitive::Callback(_) => {}
            }
        }
    }

    fn mesh(&mut self, mesh: &egui::Mesh, clip: Rect, pixels_per_point: f32) {
        for triangle in mesh.indices.chunks_exact(3) {
            let vertices: Vec<&egui::epaint::Vertex> =
                triangle.iter().filter_map(|i| mesh.vertices.get(*i as usize)).collect();
            if vertices.len() != 3 {
                continue;
            }
            let p: Vec<Pos2> = vertices
                .iter()
                .map(|v| Pos2::new(v.pos.x * pixels_per_point, v.pos.y * pixels_per_point))
                .collect();

            let min_x = p.iter().map(|v| v.x).fold(f32::MAX, f32::min).max(clip.min.x).max(0.0);
            let max_x = p
                .iter()
                .map(|v| v.x)
                .fold(f32::MIN, f32::max)
                .min(clip.max.x)
                .min(self.width as f32);
            let min_y = p.iter().map(|v| v.y).fold(f32::MAX, f32::min).max(clip.min.y).max(0.0);
            let max_y = p
                .iter()
                .map(|v| v.y)
                .fold(f32::MIN, f32::max)
                .min(clip.max.y)
                .min(self.height as f32);
            if max_x <= min_x || max_y <= min_y {
                continue;
            }

            let area = edge(p[0], p[1], p[2]);
            if area.abs() < 1e-6 {
                continue;
            }

            for y in (min_y.floor() as usize)..(max_y.ceil() as usize).min(self.height) {
                for x in (min_x.floor() as usize)..(max_x.ceil() as usize).min(self.width) {
                    let point = Pos2::new(x as f32 + 0.5, y as f32 + 0.5);
                    let w0 = edge(p[1], p[2], point) / area;
                    let w1 = edge(p[2], p[0], point) / area;
                    let w2 = edge(p[0], p[1], point) / area;
                    if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                        continue;
                    }
                    let uv = Pos2::new(
                        w0 * vertices[0].uv.x + w1 * vertices[1].uv.x + w2 * vertices[2].uv.x,
                        w0 * vertices[0].uv.y + w1 * vertices[1].uv.y + w2 * vertices[2].uv.y,
                    );
                    let c0 = linear(vertices[0].color);
                    let c1 = linear(vertices[1].color);
                    let c2 = linear(vertices[2].color);
                    let texel = self.textures.sample(mesh.texture_id, uv);
                    let mut src = [0.0f32; 4];
                    for i in 0..4 {
                        src[i] = (w0 * c0[i] + w1 * c1[i] + w2 * c2[i]) * texel[i];
                    }
                    let dst = &mut self.pixels[y * self.width + x];
                    let inv = 1.0 - src[3];
                    for i in 0..4 {
                        dst[i] = src[i] + dst[i] * inv;
                    }
                }
            }
        }
    }

    fn into_image(self) -> image::RgbaImage {
        let mut buffer = Vec::with_capacity(self.width * self.height * 4);
        for pixel in &self.pixels {
            // Un-premultiply for a PNG, then flatten onto opaque black-free
            // ground: every screen paints its own background, so alpha is 1.
            let alpha = pixel[3].clamp(0.0, 1.0);
            let (r, g, b) = if alpha > 0.0 {
                (pixel[0] / alpha, pixel[1] / alpha, pixel[2] / alpha)
            } else {
                (0.0, 0.0, 0.0)
            };
            buffer.push(to_srgb(r));
            buffer.push(to_srgb(g));
            buffer.push(to_srgb(b));
            buffer.push((alpha * 255.0).round() as u8);
        }
        image::RgbaImage::from_raw(self.width as u32, self.height as u32, buffer)
            .unwrap_or_else(|| image::RgbaImage::new(self.width as u32, self.height as u32))
    }
}

fn edge(a: Pos2, b: Pos2, c: Pos2) -> f32 {
    (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gallery_covers_every_section() {
        let names: Vec<&str> = gallery().iter().map(|s| s.name).collect();
        for needle in [
            "dashboard",
            "jobs",
            "destinations",
            "providers",
            "activity",
            "restore",
            "settings",
            "about",
            "onboarding",
        ] {
            assert!(
                names.iter().any(|n| n.contains(needle)),
                "the gallery has no shot of {needle}"
            );
        }
    }

    #[test]
    fn a_shot_produces_a_non_empty_image() {
        let shot = Shot {
            name: "test",
            size: [900.0, 600.0],
            dark: true,
            refine: None,
            setup: |app| app.go(Route::Dashboard),
        };
        let image = capture(&shot);
        assert_eq!(image.width(), 1800);
        assert_eq!(image.height(), 1200);
        // Something was actually drawn: the canvas is not uniformly empty.
        let distinct: std::collections::BTreeSet<[u8; 4]> = image.pixels().map(|p| p.0).collect();
        assert!(distinct.len() > 8, "the render produced {} colours", distinct.len());
    }
}
