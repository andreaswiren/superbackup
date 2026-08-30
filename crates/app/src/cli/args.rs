//! The command-line surface.
//!
//! Two audiences use this, and they want opposite things:
//!
//! - **A person** at a terminal wants short commands, tolerant argument
//!   parsing (a job by name or by prefix, not a UUID), and readable output.
//! - **An automation agent** wants a stable contract: predictable JSON on
//!   stdout, machine-readable error codes rather than English prose, and
//!   meaningful exit codes.
//!
//! Both are served by the same definitions. `--json` is accepted on every
//! command rather than existing on some and not others, because a caller that
//! has to remember which subcommands support it will get it wrong.
//!
//! The CLI is a **thin client**. It never opens a repository or touches the
//! vault; it asks the running instance over IPC. Only one process is ever
//! allowed to drive a Kopia repository, and two would risk corrupting it.

use std::path::PathBuf;

use clap::{ArgAction, Args, Parser, Subcommand, ValueEnum};

/// Exit codes. Stable across releases: a script may branch on them.
///
/// Anything above 2 says something specific about *why*, so a caller can
/// distinguish "your backup failed" from "I could not reach the daemon"
/// without parsing text.
pub mod exit {
    /// The command did what was asked.
    pub const OK: i32 = 0;
    /// The command ran and the answer was "no" — a job failed, a check did
    /// not pass, `doctor` found a problem.
    pub const FAILED: i32 = 1;
    /// Bad usage: unknown job, malformed argument, contradictory flags.
    pub const USAGE: i32 = 2;
    /// No superbackup instance is listening.
    pub const DAEMON_UNREACHABLE: i32 = 3;
    /// The vault is locked and this command needs it open.
    pub const LOCKED: i32 = 4;
    /// The user (or a signal) cancelled the operation.
    pub const CANCELLED: i32 = 5;
}

#[derive(Debug, Parser)]
#[command(
    name = "superbackup",
    version,
    about = "Back up your folders locally, to OneDrive, and offsite.",
    long_about = "superbackup manages Kopia repositories and folder mirrors for \
                  developer machines.\n\n\
                  With no arguments it starts the tray icon and scheduler. With a \
                  subcommand it talks to the already-running instance.\n\n\
                  Every command accepts --json. Run `superbackup schema --json` for \
                  a machine-readable description of this entire surface.",
    propagate_version = true,
    disable_help_subcommand = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    #[command(flatten)]
    pub global: GlobalArgs,
}

#[derive(Debug, Args, Clone)]
pub struct GlobalArgs {
    /// Emit JSON on stdout instead of formatted text.
    ///
    /// The envelope is stable: `{"ok":true,"data":…}` or
    /// `{"ok":false,"error":{"code":…,"message":…,"hint":…}}`.
    #[arg(long, global = true)]
    pub json: bool,

    /// Suppress progress and informational output. Errors still print.
    #[arg(long, short = 'q', global = true, conflicts_with = "verbose")]
    pub quiet: bool,

    /// Increase log verbosity. Repeat for more (-vv, -vvv).
    #[arg(long, short = 'v', global = true, action = ArgAction::Count)]
    pub verbose: u8,

    /// Never prompt. Commands that would ask for confirmation fail instead.
    ///
    /// Set this in scripts and agents: without it, a destructive command
    /// waiting on a prompt looks exactly like a hang.
    #[arg(long, global = true)]
    pub no_input: bool,

    /// Use this configuration root instead of the default location.
    #[arg(long, global = true, env = "SUPERBACKUP_HOME", value_name = "DIR")]
    pub home: Option<PathBuf>,

    /// Talk to the machine-wide service instance rather than the user one.
    #[arg(long, global = true)]
    pub service: bool,

    /// Seconds to wait for the daemon before giving up.
    #[arg(long, global = true, default_value = "30", value_name = "SECS")]
    pub timeout: u64,

    /// Colour output. Defaults to auto-detecting a terminal.
    #[arg(long, global = true, value_enum, default_value = "auto")]
    pub color: ColorChoice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ColorChoice {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    // -- Everyday ---------------------------------------------------------
    /// Show overall health, running jobs, and what runs next.
    #[command(visible_alias = "st")]
    Status(StatusArgs),

    /// Run a job now.
    Run(RunArgs),

    /// Stop a running job.
    Stop(StopArgs),

    /// Pause all scheduled backups for a while.
    Pause(PauseArgs),

    /// Resume scheduled backups after a pause.
    Resume,

    /// Stream events as they happen, one JSON object per line.
    Watch(WatchArgs),

    // -- Objects ----------------------------------------------------------
    /// Create, inspect and edit backup jobs.
    #[command(subcommand)]
    Job(JobCommand),

    /// Manage where backups are written.
    #[command(subcommand, visible_alias = "dest")]
    Destination(DestinationCommand),

    /// Manage reusable storage accounts (endpoint, region, credentials).
    #[command(subcommand)]
    Provider(ProviderCommand),

    /// Group jobs together.
    #[command(subcommand)]
    Project(ProjectCommand),

    // -- Data -------------------------------------------------------------
    /// List snapshots taken by a job.
    Snapshots(SnapshotsArgs),

    /// Restore files from a snapshot.
    Restore(RestoreArgs),

    /// Browse the contents of a snapshot without restoring.
    Browse(BrowseArgs),

    // -- Vault ------------------------------------------------------------
    /// Unlock the vault so scheduled backups can run.
    Unlock(UnlockArgs),

    /// Lock the vault. Scheduled backups will not run until it is unlocked.
    Lock,

    /// Change the master passphrase.
    ChangePassphrase,

    // -- Setup ------------------------------------------------------------
    /// Set up superbackup on this machine.
    Init(InitArgs),

    /// Install, remove, or inspect the background service.
    #[command(subcommand)]
    Service(ServiceCommand),

    /// Control whether superbackup starts when you log in.
    #[command(subcommand)]
    Autostart(AutostartCommand),

    /// Read and write settings.
    #[command(subcommand)]
    Config(ConfigCommand),

    /// Pull or publish shared configuration from a Git repository.
    #[command(subcommand)]
    Remote(RemoteCommand),

    // -- Meta -------------------------------------------------------------
    /// Check that everything is set up correctly and report what is not.
    Doctor(DoctorArgs),

    /// Open the graphical interface.
    Gui,

    /// Run the scheduler in the foreground without a tray icon.
    Daemon(DaemonArgs),

    /// Print a machine-readable description of every command.
    ///
    /// Generated from the same definitions the parser uses, so it cannot
    /// drift from what the program actually accepts. This is the intended
    /// entry point for an automation agent discovering the surface.
    Schema,

    /// Print version and build information.
    Version,
}

// ---------------------------------------------------------------------------
// Everyday
// ---------------------------------------------------------------------------

#[derive(Debug, Args)]
pub struct StatusArgs {
    /// Limit to one job.
    #[arg(value_name = "JOB")]
    pub job: Option<String>,

    /// Include the recent activity log.
    #[arg(long)]
    pub events: bool,

    /// How many recent events to include.
    #[arg(long, default_value = "20", value_name = "N")]
    pub events_limit: usize,

    /// Repeat every N seconds until interrupted.
    #[arg(long, value_name = "SECS")]
    pub watch: Option<u64>,
}

#[derive(Debug, Args)]
pub struct RunArgs {
    /// Job to run: a name, an id, or an unambiguous name prefix.
    #[arg(value_name = "JOB", required_unless_present = "all")]
    pub job: Option<String>,

    /// Run every enabled job.
    #[arg(long, conflicts_with = "job")]
    pub all: bool,

    /// Only write to these destinations, by name or id. Repeatable.
    #[arg(long = "destination", short = 'd', value_name = "DEST")]
    pub destinations: Vec<String>,

    /// Report what would be backed up without writing anything.
    #[arg(long)]
    pub dry_run: bool,

    /// Wait for the run to finish and exit non-zero if it failed.
    ///
    /// Without this the command queues the run and returns immediately, which
    /// is rarely what a script wants.
    #[arg(long, short = 'w')]
    pub wait: bool,

    /// Run even if the schedule would normally be skipped — paused, on
    /// battery, or on a metered connection.
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct StopArgs {
    /// Job or run id to stop.
    #[arg(value_name = "JOB", required_unless_present = "all")]
    pub job: Option<String>,

    /// Stop every running job.
    #[arg(long, conflicts_with = "job")]
    pub all: bool,
}

#[derive(Debug, Args)]
pub struct PauseArgs {
    /// How long to pause: `30m`, `4h`, `2h30m`.
    #[arg(value_name = "DURATION", required_unless_present = "until_resumed")]
    pub duration: Option<String>,

    /// Pause indefinitely, until `superbackup resume`.
    #[arg(long, conflicts_with = "duration")]
    pub until_resumed: bool,

    /// A note shown in the interface explaining why.
    #[arg(long, value_name = "TEXT")]
    pub reason: Option<String>,
}

#[derive(Debug, Args)]
pub struct WatchArgs {
    /// Only events for this job.
    #[arg(value_name = "JOB")]
    pub job: Option<String>,

    /// Event kinds to include, e.g. `job.failed`. Repeatable. Default: all.
    #[arg(long = "kind", value_name = "KIND")]
    pub kinds: Vec<String>,

    /// Include live progress updates as well as discrete events.
    #[arg(long)]
    pub progress: bool,

    /// Exit after this many events.
    #[arg(long, value_name = "N")]
    pub limit: Option<usize>,
}

// ---------------------------------------------------------------------------
// Jobs
// ---------------------------------------------------------------------------

#[derive(Debug, Subcommand)]
pub enum JobCommand {
    /// List jobs and their last result.
    #[command(visible_alias = "ls")]
    List(JobListArgs),

    /// Show one job in full.
    Show {
        #[arg(value_name = "JOB")]
        job: String,
    },

    /// Create a job.
    Add(JobAddArgs),

    /// Change an existing job.
    Edit(JobEditArgs),

    /// Delete a job. Backups already taken are not touched.
    Remove(JobRemoveArgs),

    /// Allow a job to run on its schedule.
    Enable {
        #[arg(value_name = "JOB")]
        job: String,
    },

    /// Stop a job running on its schedule. Does not delete anything.
    Disable {
        #[arg(value_name = "JOB")]
        job: String,
    },

    /// Show what a job would include and exclude, without backing up.
    Preview(JobPreviewArgs),
}

#[derive(Debug, Args)]
pub struct JobListArgs {
    /// Only jobs in this project.
    #[arg(long, value_name = "PROJECT")]
    pub project: Option<String>,

    /// Only jobs whose last run failed.
    #[arg(long)]
    pub failed: bool,

    /// Only enabled jobs.
    #[arg(long)]
    pub enabled: bool,
}

#[derive(Debug, Args)]
pub struct JobAddArgs {
    /// Name for the job. Used to refer to it everywhere else.
    #[arg(long, short = 'n', value_name = "NAME")]
    pub name: String,

    /// Folder to back up. Repeatable.
    #[arg(long = "source", short = 's', value_name = "PATH", required = true)]
    pub sources: Vec<PathBuf>,

    /// Where to write, by destination name or id. Repeatable.
    ///
    /// Omit to be prompted, or to attach destinations later with `job edit`.
    #[arg(long = "destination", short = 'd', value_name = "DEST")]
    pub destinations: Vec<String>,

    /// A starting point for exclusions and schedule.
    #[arg(long, value_enum, default_value = "developer")]
    pub template: JobTemplate,

    /// When to run: `manual`, `hourly`, `daily@02:00`, `weekly@mon,thu@09:00`,
    /// `every 30m`, `on-change`, or a five-field cron expression.
    #[arg(long, value_name = "SPEC")]
    pub schedule: Option<String>,

    /// Extra ignore pattern, gitignore syntax. Repeatable.
    #[arg(long = "exclude", short = 'x', value_name = "PATTERN")]
    pub excludes: Vec<String>,

    /// Group this job under a project.
    #[arg(long, value_name = "PROJECT")]
    pub project: Option<String>,

    /// Free-text description.
    #[arg(long, value_name = "TEXT")]
    pub description: Option<String>,

    /// Create it disabled.
    #[arg(long)]
    pub disabled: bool,
}

/// Starting points offered by `job add`, matching the wizard in the interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum JobTemplate {
    /// Excludes build caches, `node_modules`, virtualenvs and IDE state. The
    /// reason this application exists.
    Developer,
    /// Excludes only OS junk. Everything else is backed up.
    Documents,
    /// No exclusions at all.
    Everything,
}

#[derive(Debug, Args)]
pub struct JobEditArgs {
    #[arg(value_name = "JOB")]
    pub job: String,

    #[arg(long, value_name = "NAME")]
    pub name: Option<String>,

    /// Add a source folder. Repeatable.
    #[arg(long = "add-source", value_name = "PATH")]
    pub add_sources: Vec<PathBuf>,

    /// Remove a source folder. Repeatable.
    #[arg(long = "remove-source", value_name = "PATH")]
    pub remove_sources: Vec<PathBuf>,

    /// Add a destination. Repeatable.
    #[arg(long = "add-destination", value_name = "DEST")]
    pub add_destinations: Vec<String>,

    /// Remove a destination. Repeatable.
    #[arg(long = "remove-destination", value_name = "DEST")]
    pub remove_destinations: Vec<String>,

    #[arg(long, value_name = "SPEC")]
    pub schedule: Option<String>,

    /// Add an ignore pattern. Repeatable.
    #[arg(long = "add-exclude", value_name = "PATTERN")]
    pub add_excludes: Vec<String>,

    /// Remove an ignore pattern. Repeatable.
    #[arg(long = "remove-exclude", value_name = "PATTERN")]
    pub remove_excludes: Vec<String>,

    /// Upload ceiling in KB/s for this job. `0` means unlimited.
    #[arg(long, value_name = "KBPS")]
    pub upload_limit: Option<u32>,

    /// Abort a run that exceeds this many minutes. `0` removes the limit.
    #[arg(long, value_name = "MINUTES")]
    pub timeout_minutes: Option<u32>,
}

#[derive(Debug, Args)]
pub struct JobRemoveArgs {
    #[arg(value_name = "JOB")]
    pub job: String,

    /// Do not ask for confirmation.
    #[arg(long, short = 'y')]
    pub yes: bool,
}

#[derive(Debug, Args)]
pub struct JobPreviewArgs {
    #[arg(value_name = "JOB")]
    pub job: String,

    /// Also list the paths that would be excluded, and by which rule. This
    /// answers "why is my folder not being backed up?".
    #[arg(long)]
    pub show_excluded: bool,

    /// Stop after examining this many files.
    #[arg(long, default_value = "100000", value_name = "N")]
    pub limit: usize,
}

// ---------------------------------------------------------------------------
// Destinations
// ---------------------------------------------------------------------------

#[derive(Debug, Subcommand)]
pub enum DestinationCommand {
    /// List destinations, their kind, and whether they are reachable.
    #[command(visible_alias = "ls")]
    List,

    /// Show one destination in full, including its encryption settings.
    Show {
        #[arg(value_name = "DEST")]
        destination: String,
    },

    /// Add a destination and, for repositories, create or connect to it.
    Add(DestinationAddArgs),

    /// Change a destination's name, prefix, or bandwidth ceiling.
    Edit(DestinationEditArgs),

    /// Remove a destination from the configuration.
    ///
    /// This never deletes the repository or its contents — only superbackup's
    /// record of it. Deleting the data is a deliberate act you perform on the
    /// storage itself.
    Remove(DestinationRemoveArgs),

    /// Check that the destination is reachable and the credentials work.
    Test {
        #[arg(value_name = "DEST")]
        destination: String,
    },

    /// Connect to a repository that already exists.
    Connect {
        #[arg(value_name = "DEST")]
        destination: String,
    },

    /// Report how much space the repository occupies.
    Stats {
        #[arg(value_name = "DEST")]
        destination: String,
    },

    /// Run Kopia maintenance to reclaim space from expired snapshots.
    Maintain(DestinationMaintainArgs),

    /// List the machines that have written to this destination.
    Machines {
        #[arg(value_name = "DEST")]
        destination: String,
    },
}

#[derive(Debug, Args)]
pub struct DestinationAddArgs {
    #[arg(long, short = 'n', value_name = "NAME")]
    pub name: Option<String>,

    /// A Kopia repository in a local folder, on an external drive, or on a
    /// network share.
    #[arg(long, value_name = "PATH", group = "kind")]
    pub local: Option<PathBuf>,

    /// A Kopia repository inside a detected OneDrive folder.
    ///
    /// With no value, uses the OneDrive account superbackup detects. Pass an
    /// account name when you have more than one.
    #[arg(long, value_name = "ACCOUNT", num_args = 0..=1, default_missing_value = "", group = "kind")]
    pub onedrive: Option<String>,

    /// A Kopia repository in a bucket on a configured storage provider.
    /// Requires --bucket.
    #[arg(long, value_name = "PROVIDER", group = "kind", requires = "bucket")]
    pub s3: Option<String>,

    /// Bucket name, with `--s3`.
    #[arg(long, value_name = "BUCKET")]
    pub bucket: Option<String>,

    /// Key prefix inside the bucket. Defaults to `superbackup/<machine>/`,
    /// which is what keeps several machines apart in one bucket.
    #[arg(long, value_name = "PREFIX")]
    pub prefix: Option<String>,

    /// A plain, unencrypted folder copy. No repository, no deduplication.
    ///
    /// Anyone who can read the folder can read your files. Use this when you
    /// want a copy you can open without any tooling.
    #[arg(long, value_name = "PATH", group = "kind")]
    pub mirror: Option<PathBuf>,

    /// Encryption algorithm for a new repository.
    #[arg(long, value_name = "ALGO")]
    pub encryption: Option<String>,

    /// Content hash algorithm for a new repository.
    #[arg(long, value_name = "ALGO")]
    pub hash: Option<String>,

    /// Object splitter for a new repository. Smaller averages deduplicate
    /// millions of tiny files better.
    #[arg(long, value_name = "ALGO")]
    pub splitter: Option<String>,

    /// Where the repository passphrase comes from.
    #[arg(long, value_enum, default_value = "generated")]
    pub passphrase: PassphraseMode,

    /// Connect to an existing repository instead of creating one.
    #[arg(long)]
    pub connect_existing: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum PassphraseMode {
    /// 256 bits from the system random number generator. You are shown it
    /// once and told to write it down.
    Generated,
    /// Typed by you, and checked for strength.
    Prompt,
    /// Derived from your master passphrase and this destination's id, so any
    /// machine sharing your vault can reconstruct it.
    Derived,
}

#[derive(Debug, Args)]
pub struct DestinationEditArgs {
    #[arg(value_name = "DEST")]
    pub destination: String,

    #[arg(long, value_name = "NAME")]
    pub name: Option<String>,

    #[arg(long, value_name = "PREFIX")]
    pub prefix: Option<String>,

    /// Upload ceiling in KB/s for this destination. `0` means unlimited.
    #[arg(long, value_name = "KBPS")]
    pub upload_limit: Option<u32>,

    #[arg(long)]
    pub enable: bool,

    #[arg(long, conflicts_with = "enable")]
    pub disable: bool,
}

#[derive(Debug, Args)]
pub struct DestinationRemoveArgs {
    #[arg(value_name = "DEST")]
    pub destination: String,

    #[arg(long, short = 'y')]
    pub yes: bool,
}

#[derive(Debug, Args)]
pub struct DestinationMaintainArgs {
    #[arg(value_name = "DEST")]
    pub destination: String,

    /// Run full maintenance, which is slower but reclaims more.
    #[arg(long)]
    pub full: bool,
}

// ---------------------------------------------------------------------------
// Providers
// ---------------------------------------------------------------------------

#[derive(Debug, Subcommand)]
pub enum ProviderCommand {
    /// List storage accounts and how many destinations use each.
    #[command(visible_alias = "ls")]
    List,

    /// Show one provider. Credentials are never printed.
    Show {
        #[arg(value_name = "PROVIDER")]
        provider: String,
    },

    /// Add a storage account that destinations can reuse.
    Add(ProviderAddArgs),

    /// Change a provider's name, endpoint, or region.
    Edit(ProviderEditArgs),

    /// Remove a provider. Fails if destinations still use it, unless forced.
    Remove(ProviderRemoveArgs),

    /// Check the endpoint and credentials.
    Test {
        #[arg(value_name = "PROVIDER")]
        provider: String,
    },

    /// Replace the access key pair. Affects every destination that inherits
    /// these credentials.
    Rotate {
        #[arg(value_name = "PROVIDER")]
        provider: String,
    },

    /// List destinations and jobs that depend on this provider.
    UsedBy {
        #[arg(value_name = "PROVIDER")]
        provider: String,
    },
}

#[derive(Debug, Args)]
pub struct ProviderAddArgs {
    #[arg(long, short = 'n', value_name = "NAME")]
    pub name: String,

    /// Which S3-compatible service. Prefills endpoint and region.
    #[arg(long, value_enum, default_value = "storj")]
    pub flavour: ProviderFlavour,

    /// S3 endpoint, e.g. `https://gateway.storjshare.io`.
    #[arg(long, value_name = "URL")]
    pub endpoint: Option<String>,

    /// Region, e.g. `eu-1`.
    #[arg(long, value_name = "REGION")]
    pub region: Option<String>,

    /// Access key. Omit to be prompted — passing a key on the command line
    /// puts it in your shell history and in the process list.
    #[arg(long, value_name = "KEY")]
    pub access_key: Option<String>,

    // Deliberately no `--path-style`. Kopia's S3 backend selects path-style
    // addressing automatically and exposes no flag to override it, so offering
    // the switch would mean shipping a control that does nothing. The property
    // still exists on `ProviderKind::S3` for future backends; it is simply not
    // presented as something the user can act on.
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ProviderFlavour {
    Storj,
    Aws,
    BackblazeB2,
    Wasabi,
    Minio,
    Cloudflare,
    Other,
}

#[derive(Debug, Args)]
pub struct ProviderEditArgs {
    #[arg(value_name = "PROVIDER")]
    pub provider: String,

    #[arg(long, value_name = "NAME")]
    pub name: Option<String>,

    #[arg(long, value_name = "URL")]
    pub endpoint: Option<String>,

    #[arg(long, value_name = "REGION")]
    pub region: Option<String>,
}

#[derive(Debug, Args)]
pub struct ProviderRemoveArgs {
    #[arg(value_name = "PROVIDER")]
    pub provider: String,

    #[arg(long, short = 'y')]
    pub yes: bool,

    /// Remove even though destinations still reference it.
    #[arg(long)]
    pub force: bool,
}

// ---------------------------------------------------------------------------
// Projects
// ---------------------------------------------------------------------------

#[derive(Debug, Subcommand)]
pub enum ProjectCommand {
    /// List projects and how many jobs each contains.
    #[command(visible_alias = "ls")]
    List,
    /// Create a project to group jobs under.
    Add {
        #[arg(long, short = 'n', value_name = "NAME")]
        name: String,
        #[arg(long, value_name = "TEXT")]
        description: Option<String>,
    },
    /// Delete a project. Its jobs are kept and become ungrouped.
    Remove {
        #[arg(value_name = "PROJECT")]
        project: String,
        #[arg(long, short = 'y')]
        yes: bool,
    },
}

// ---------------------------------------------------------------------------
// Snapshots and restore
// ---------------------------------------------------------------------------

#[derive(Debug, Args)]
pub struct SnapshotsArgs {
    #[arg(value_name = "JOB")]
    pub job: String,

    /// Only snapshots at this destination.
    #[arg(long, short = 'd', value_name = "DEST")]
    pub destination: Option<String>,

    #[arg(long, default_value = "20", value_name = "N")]
    pub limit: usize,

    /// Include snapshots from other machines writing to the same destination.
    #[arg(long)]
    pub all_machines: bool,
}

#[derive(Debug, Args)]
pub struct RestoreArgs {
    #[arg(value_name = "JOB")]
    pub job: String,

    /// Where to write the restored files.
    #[arg(long, value_name = "PATH", required = true)]
    pub to: PathBuf,

    /// Restore the snapshot nearest this time. Accepts `2026-08-29T14:00`,
    /// `yesterday`, or `3 days ago`. Defaults to the most recent.
    #[arg(long, value_name = "WHEN", conflicts_with = "snapshot")]
    pub at: Option<String>,

    /// Restore this exact snapshot id.
    #[arg(long, value_name = "ID")]
    pub snapshot: Option<String>,

    /// Restore only this path within the snapshot. Repeatable.
    #[arg(long = "path", short = 'p', value_name = "PATH")]
    pub paths: Vec<String>,

    #[arg(long, short = 'd', value_name = "DEST")]
    pub destination: Option<String>,

    /// What to do when a file already exists at the target.
    #[arg(long, value_enum, default_value = "skip")]
    pub on_conflict: ConflictPolicy,

    /// Report what would be restored without writing anything.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ConflictPolicy {
    /// Leave the existing file alone.
    Skip,
    /// Replace the existing file.
    Overwrite,
    /// Write alongside it with a suffix.
    KeepBoth,
    /// Stop the restore.
    Fail,
}

#[derive(Debug, Args)]
pub struct BrowseArgs {
    #[arg(value_name = "JOB")]
    pub job: String,

    /// Directory within the snapshot. Defaults to the root.
    #[arg(value_name = "PATH", default_value = "/")]
    pub path: String,

    #[arg(long, value_name = "ID")]
    pub snapshot: Option<String>,

    #[arg(long, value_name = "WHEN")]
    pub at: Option<String>,

    #[arg(long, short = 'd', value_name = "DEST")]
    pub destination: Option<String>,

    /// Descend into subdirectories.
    #[arg(long, short = 'r')]
    pub recursive: bool,
}

// ---------------------------------------------------------------------------
// Vault
// ---------------------------------------------------------------------------

#[derive(Debug, Args)]
pub struct UnlockArgs {
    /// Read the passphrase from this file, or from stdin when given `-`.
    ///
    /// There is deliberately no `--passphrase` flag: a passphrase in `argv`
    /// is visible to every other process on the machine and lands in shell
    /// history.
    #[arg(long, value_name = "FILE")]
    pub passphrase_file: Option<PathBuf>,

    /// Also cache the key in the OS keychain so the service can run
    /// unattended. Read the security note before enabling this.
    #[arg(long)]
    pub remember: bool,
}

// ---------------------------------------------------------------------------
// Setup
// ---------------------------------------------------------------------------

#[derive(Debug, Args)]
pub struct InitArgs {
    /// Accept every default without prompting. Requires `--passphrase-file`.
    #[arg(long)]
    pub non_interactive: bool,

    #[arg(long, value_name = "FILE")]
    pub passphrase_file: Option<PathBuf>,

    /// Name for this machine. Defaults to the hostname.
    #[arg(long, value_name = "NAME")]
    pub machine_name: Option<String>,

    /// Do not look for OneDrive.
    #[arg(long)]
    pub skip_onedrive: bool,

    /// Do not register to start at login.
    #[arg(long)]
    pub skip_autostart: bool,
}

#[derive(Debug, Subcommand)]
pub enum ServiceCommand {
    /// Install the background service. Requires administrator privileges.
    Install(ServiceInstallArgs),
    /// Remove the background service.
    Uninstall {
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Start the installed service.
    Start,
    /// Stop the running service. Scheduled backups will not run until it is
    /// started again.
    Stop,
    /// Report whether the service is installed and running.
    Status,
    /// Service entry point. Invoked by the operating system, not by you.
    #[command(hide = true)]
    Run,
}

#[derive(Debug, Args)]
pub struct ServiceInstallArgs {
    /// Run the service as this account rather than the system account.
    ///
    /// Needed for OneDrive destinations and mapped network drives, which a
    /// system account cannot see.
    #[arg(long, value_name = "USER")]
    pub user: Option<String>,

    /// Start it immediately after installing.
    #[arg(long, default_value = "true")]
    pub start: bool,

    /// Install a per-user unit rather than a system one. Linux only.
    #[arg(long)]
    pub user_scope: bool,
}

#[derive(Debug, Subcommand)]
pub enum AutostartCommand {
    /// Start superbackup automatically when you log in.
    Enable,
    /// Stop starting superbackup at login. Scheduled backups will not run
    /// unless the service is installed.
    Disable,
    /// Report whether it is registered, and whether it points at this binary.
    Status,
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Print the whole configuration. Secrets are never included.
    Show,
    /// Read one setting.
    Get {
        #[arg(value_name = "KEY")]
        key: String,
    },
    /// Write one setting.
    Set {
        #[arg(value_name = "KEY")]
        key: String,
        #[arg(value_name = "VALUE")]
        value: String,
    },
    /// Check the configuration for problems without changing anything.
    Validate,
    /// Write the configuration to a file. Secrets are never included.
    Export {
        #[arg(long, value_name = "FILE")]
        to: PathBuf,
    },
    /// Reload the configuration from disk.
    Reload,
}

#[derive(Debug, Subcommand)]
pub enum RemoteCommand {
    /// Point at a Git repository holding shared configuration.
    Set(RemoteSetArgs),
    /// Fetch the shared configuration and show what would change.
    Pull(RemotePullArgs),
    /// Show the difference between local and shared configuration.
    Diff,
    /// Publish local configuration. Always explicit, never automatic.
    Push(RemotePushArgs),
    /// Show which repository is configured and when it was last pulled.
    Status,
}

#[derive(Debug, Args)]
pub struct RemoteSetArgs {
    #[arg(value_name = "URL")]
    pub url: String,

    #[arg(long, default_value = "main", value_name = "BRANCH")]
    pub branch: String,

    #[arg(long, default_value = "config.sbvault", value_name = "PATH")]
    pub path: String,

    /// Only accept a vault signed by one of these key fingerprints.
    /// Repeatable. Strongly recommended for any shared repository.
    #[arg(long = "trust", value_name = "FINGERPRINT")]
    pub trusted_signers: Vec<String>,
}

#[derive(Debug, Args)]
pub struct RemotePullArgs {
    /// Apply the changes. Without this, only the difference is shown.
    #[arg(long)]
    pub apply: bool,

    #[arg(long, short = 'y')]
    pub yes: bool,
}

#[derive(Debug, Args)]
pub struct RemotePushArgs {
    #[arg(long, value_name = "TEXT")]
    pub message: Option<String>,

    #[arg(long, short = 'y')]
    pub yes: bool,
}

// ---------------------------------------------------------------------------
// Meta
// ---------------------------------------------------------------------------

#[derive(Debug, Args)]
pub struct DoctorArgs {
    /// Fix what can be fixed safely: download a pinned kopia, repair a stale
    /// autostart entry, recreate missing directories.
    #[arg(long)]
    pub fix: bool,

    /// Also verify that every destination is reachable. Slower, and makes
    /// network requests.
    #[arg(long)]
    pub check_destinations: bool,
}

#[derive(Debug, Args)]
pub struct DaemonArgs {
    /// Start without a tray icon even on a desktop session.
    #[arg(long)]
    pub no_tray: bool,

    /// Exit rather than starting if another instance is already running.
    #[arg(long)]
    pub fail_if_running: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn definition_is_internally_consistent() {
        // Catches conflicting flags, duplicate short options, and bad
        // argument groups at test time rather than at a user's terminal.
        Cli::command().debug_assert();
    }

    #[test]
    fn json_is_accepted_on_every_subcommand() {
        // An agent should never have to remember which commands support
        // --json. Sample a few across different subcommand shapes.
        for argv in [
            vec!["superbackup", "status", "--json"],
            vec!["superbackup", "job", "list", "--json"],
            vec!["superbackup", "destination", "test", "d1", "--json"],
            vec!["superbackup", "provider", "used-by", "storj", "--json"],
            vec!["superbackup", "doctor", "--json"],
            vec!["superbackup", "schema", "--json"],
        ] {
            let cli =
                Cli::try_parse_from(&argv).unwrap_or_else(|e| panic!("{argv:?} should parse: {e}"));
            assert!(cli.global.json, "--json did not take effect for {argv:?}");
        }
    }

    #[test]
    fn no_command_takes_a_passphrase_as_an_argument() {
        // A passphrase in argv is readable by every other process on the
        // machine and is written to shell history. If this test ever fails,
        // someone has added a flag that must not exist.
        //
        // This test was previously VACUOUS and passed without checking
        // anything. `Cli::command()` returns an *unbuilt* command, where
        // `get_num_args()` is `None` for every derive-generated argument — so
        // `takes_value` was always false and the assertion body never ran. The
        // exact same `.build()` omission was found and fixed in `schema.rs`,
        // and not carried across to here. A security test that cannot fail is
        // worse than no test, because it reads as coverage.
        //
        // Assert the precondition explicitly so it can never silently regress
        // into doing nothing again.
        fn walk(cmd: &clap::Command, path: &mut Vec<String>, checked: &mut usize) {
            for arg in cmd.get_arguments() {
                let id = arg.get_id().as_str();
                let takes_value = arg.get_num_args().map(|r| r.takes_values()).unwrap_or(false);
                if !takes_value {
                    continue;
                }
                *checked += 1;
                // `--passphrase-file` names a file, not a secret.
                if id.ends_with("_file") {
                    continue;
                }
                // A closed set of choices cannot smuggle a secret:
                // `destination add --passphrase generated|prompt|derived`.
                if !arg.get_possible_values().is_empty() {
                    continue;
                }
                assert!(
                    !(id.contains("passphrase")
                        || id.contains("password")
                        || id.contains("secret")
                        || id.contains("token")),
                    "`{}` in `{}` accepts a secret on the command line",
                    id,
                    path.join(" ")
                );
            }
            for sub in cmd.get_subcommands() {
                path.push(sub.get_name().to_string());
                walk(sub, path, checked);
                path.pop();
            }
        }

        let mut cmd = Cli::command();
        cmd.build();
        let mut checked = 0usize;
        walk(&cmd, &mut vec!["superbackup".into()], &mut checked);
        assert!(
            checked > 30,
            "only {checked} value-taking arguments were examined — the command \
             was probably not built, and this test is passing vacuously again"
        );
    }

    #[test]
    fn every_command_has_help_text() {
        // The schema output is generated from these strings, so an
        // undocumented command is an undocumented API.
        fn walk(cmd: &clap::Command, path: &str) {
            for sub in cmd.get_subcommands() {
                let full = format!("{path} {}", sub.get_name());
                if !sub.is_hide_set() {
                    assert!(
                        sub.get_about().is_some(),
                        "`{full}` has no description; an agent reading the schema would be guessing"
                    );
                }
                walk(sub, &full);
            }
        }
        walk(&Cli::command(), "superbackup");
    }

    #[test]
    fn job_can_be_named_by_prefix_without_quoting_gymnastics() {
        let cli = Cli::try_parse_from(["superbackup", "run", "dev", "--wait"]).unwrap();
        match cli.command {
            Some(Command::Run(a)) => {
                assert_eq!(a.job.as_deref(), Some("dev"));
                assert!(a.wait);
            }
            other => panic!("parsed as {other:?}"),
        }
    }

    #[test]
    fn run_requires_a_job_or_all() {
        assert!(Cli::try_parse_from(["superbackup", "run"]).is_err());
        assert!(Cli::try_parse_from(["superbackup", "run", "--all"]).is_ok());
        assert!(Cli::try_parse_from(["superbackup", "run", "x", "--all"]).is_err());
    }

    #[test]
    fn destination_kinds_are_mutually_exclusive() {
        assert!(Cli::try_parse_from([
            "superbackup",
            "destination",
            "add",
            "--local",
            "/tmp/a",
            "--mirror",
            "/tmp/b"
        ])
        .is_err());
    }

    #[test]
    fn s3_destination_requires_a_bucket() {
        assert!(
            Cli::try_parse_from(["superbackup", "destination", "add", "--s3", "storj"]).is_err()
        );
        assert!(Cli::try_parse_from([
            "superbackup",
            "destination",
            "add",
            "--s3",
            "storj",
            "--bucket",
            "backups"
        ])
        .is_ok());
    }

    #[test]
    fn no_arguments_means_run_the_tray() {
        let cli = Cli::try_parse_from(["superbackup"]).unwrap();
        assert!(cli.command.is_none());
    }
}
