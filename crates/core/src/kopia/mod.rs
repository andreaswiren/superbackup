//! The Kopia command-line driver.
//!
//! Kopia does the actual work of backing up: content-addressed, deduplicated,
//! encrypted, incremental. Superbackup owns the configuration, the schedule,
//! the secrets and the user experience. This module is the seam between them,
//! and everything unpleasant about driving a foreign CLI in a product that
//! holds people's only copy of their data is concentrated here.
//!
//! ```text
//!   engine ──▶ KopiaDriver ──▶ KopiaCommand ──▶ kopia(1)
//!                  │                 │              │
//!                  │                 │              ├─ stdout ─▶ JSON  ─▶ manifest
//!                  │                 │              └─ stderr ─▶ lines ─▶ progress + warnings
//!                  │                 └── secrets go in the environment, never argv
//!                  └── one repository, one config file, one cache directory
//! ```
//!
//! # Getting kopia in the first place
//!
//! Kopia is a prerequisite, and [`install`] makes it superbackup's problem
//! rather than the user's: on first run it fetches a build from Kopia's own
//! GitHub releases, verifies it against the SHA-256 published with that release,
//! and installs it atomically. Discovery still prefers a kopia the user
//! installed themselves; the managed build is only ever the fallback, and only
//! it is ever replaced.
//!
//! # Guarantees this module makes
//!
//! 1. **No secret ever appears in a command line.** Passphrases and object-store
//!    keys travel in the child's environment; [`KopiaCommand::audit_argv`]
//!    mechanically proves it before every spawn. See `command.rs`.
//! 2. **No interference with the user's own kopia.** Every invocation is pinned
//!    to `--config-file <data>/kopia/<destination-id>.config` and its own cache
//!    directory. See `driver.rs`.
//! 3. **Nothing reaches a log, an event, or an error without redaction.** Every
//!    captured stderr line goes through [`crate::redact::scrub`] before it can
//!    be stored or forwarded.
//! 4. **A cancelled or timed-out command dies.** The child is killed and reaped,
//!    leaving no zombie and no kopia holding the repository open.
//! 5. **Malformed kopia output never panics.** Every parser here is total and
//!    degrades to "we know less" rather than to a crash in a backup daemon.
//!
//! # What the engine calls
//!
//! ```no_run
//! # use superbackup_core::kopia::*;
//! # use superbackup_core::{paths::Paths, model::*, secret::Secret};
//! # async fn example(paths: Paths, destination: Destination, provider: StorageProvider,
//! #                  source: Source, settings: Settings) -> Result<(), Box<dyn std::error::Error>> {
//! // Once per process: find and verify kopia.
//! let binary = KopiaBinary::discover(&settings, &paths).await?;
//!
//! // Once per destination: bind it, with the secrets resolved from the vault.
//! let secrets = DestinationSecrets::with_passphrase(Secret::from_str("…"));
//! let driver = KopiaDriver::new(binary, &paths, &destination, Some(&provider), secrets)?;
//!
//! // Live progress for the GUI, plus a cancel button.
//! let (events, mut rx) = EventSink::channel(64);
//! let (cancel_handle, cancel) = cancellation();
//! let ctx = RunContext::new().with_events(events).with_cancel(cancel);
//!
//! driver.connect_repository(&ctx).await?;
//! driver.apply_source_policy(&source, &destination.retention, &Default::default(), &ctx).await?;
//! let outcome = driver.create_snapshot(&source, &SnapshotOptions::default(), &ctx).await?;
//! # let _ = (outcome, rx.recv(), cancel_handle);
//! # Ok(())
//! # }
//! ```
//!
//! # Which kopia this was written against
//!
//! Every flag used here was read from kopia's own source on `master` rather
//! than from its website, and the file it came from is named in the doc comment
//! of the method that uses it. The release-asset and checksum formats in
//! [`install`] were verified against the real `kopia/kopia` v0.23.1 release.
//! [`MINIMUM_KOPIA_VERSION`] is the oldest release the driver will drive;
//! [`KopiaBinary`] refuses anything older with a message that says so.

mod binary;
mod command;
mod driver;
mod error;
pub mod install;
mod manifest;
mod policy;
mod progress;
mod snapshot;

pub use binary::{configured_floor, KopiaBinary, KopiaSource, KopiaVersion, MINIMUM_KOPIA_VERSION};
pub use command::{
    cancellation, CancelHandle, CancelToken, CommandOutput, EventSink, KopiaCommand, KopiaEvent,
    RunContext,
};
pub use driver::{
    s3_endpoint_host, BlobStats, ConnectionTest, ContentStats, DestinationSecrets, KopiaDriver,
    KopiaResult, RepositoryStatus, UnsupportedOption,
};
pub use error::{classify, KopiaError, KopiaFailure};
pub use install::{
    InstallError, InstallOutcome, InstallPhase, InstallProgress, InstallProgressSink,
    KopiaInstaller, ReleaseInfo, SkipReason, UpdateCheck,
};
pub use manifest::{
    DirEntry, DirManifest, DirectorySummary, EntryError, EntryType, SnapshotManifest,
    SnapshotStats, SourceInfo,
};
pub use policy::{MaintenanceMode, MaintenanceSettings, StoredPolicy};
pub use progress::{
    parse_bytes, parse_go_duration, parse_progress_line, parse_restore_progress_line, ProgressLine,
    ProgressTracker, RestoreProgressLine,
};
pub use snapshot::{
    RestoreArchive, RestoreOptions, RestoreOutcome, SnapshotEstimate, SnapshotOptions,
    SnapshotOutcome,
};
