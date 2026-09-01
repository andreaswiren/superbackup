//! Every user-facing string, transcribed from `design/COPY.md`.
//!
//! The copy deck is the authority. Keys there map to `const` items here by the
//! obvious transformation (`vault.unlock.wrong` → `vault::UNLOCK_WRONG`), and
//! strings with `{placeholders}` become small functions so a caller cannot get
//! the substitution order wrong.
//!
//! Nothing in this module is paraphrased. Where a string is produced by the
//! core instead — `RunStatus::title()`, `ExclusionPreset::rationale()` — the
//! interface reads the core, so the CLI and the GUI can never disagree.

#![allow(dead_code)]

use crate::gui::format;

// ---------------------------------------------------------------------------
// 1. Application-wide
// ---------------------------------------------------------------------------

pub const APP_NAME: &str = "superbackup";
pub const APP_TAGLINE: &str = "Backups for machines full of code.";

pub fn window_title(health: &str) -> String {
    format!("superbackup — {health}")
}
pub fn window_title_running(job: &str, percent: i64) -> String {
    format!("superbackup — Backing up {job} ({percent}%)")
}

pub mod action {
    pub const SAVE: &str = "Save";
    pub const SAVE_CHANGES: &str = "Save changes";
    pub const CANCEL: &str = "Cancel";
    pub const BACK: &str = "Back";
    pub const CONTINUE: &str = "Continue";
    pub const DONE: &str = "Done";
    pub const CLOSE: &str = "Close";
    pub const DELETE: &str = "Delete";
    pub const REMOVE: &str = "Remove";
    pub const EDIT: &str = "Edit";
    pub const DUPLICATE: &str = "Duplicate";
    pub const ADD: &str = "Add";
    pub const BROWSE: &str = "Browse…";
    pub const OPEN_FOLDER: &str = "Open folder";
    pub const COPY: &str = "Copy";
    pub const COPIED: &str = "Copied";
    pub const RETRY: &str = "Retry";
    pub const VERIFY: &str = "Verify";
    pub const VERIFY_NOW: &str = "Verify now";
    pub const TEST_CONNECTION: &str = "Test connection";
    pub const RUN_NOW: &str = "Run now";
    pub const BACK_UP_NOW: &str = "Back up now";
    pub const STOP: &str = "Stop";
    pub const ENABLE: &str = "Enable";
    pub const DISABLE: &str = "Disable";
    pub const UNLOCK: &str = "Unlock";
    pub const LOCK_NOW: &str = "Lock now";
    pub const SHOW_DETAILS: &str = "Show technical details";
    pub const HIDE_DETAILS: &str = "Hide technical details";
    pub const COPY_DETAILS: &str = "Copy details";
    pub const CLEAR_FILTERS: &str = "Clear filters";
    pub const LEARN_MORE: &str = "Learn more";
}

pub mod state {
    pub const NEVER: &str = "Never";
    pub const NONE: &str = "— none —";
    pub const UNKNOWN: &str = "Unknown";
    pub const CALCULATING: &str = "Calculating…";
    pub const ESTIMATING: &str = "Estimating…";
    pub const LOADING: &str = "Loading…";
}

pub mod badge {
    pub const WARNINGS_SHORT: &str = "Warnings";
    pub const NEVER_RUN: &str = "Never run";
    pub const DISABLED: &str = "Disabled";
}

/// The four destination kinds. These mirror `DestinationKind::label()`; the
/// interface reads the model where it has one to hand, and these where it is
/// naming a kind that does not exist yet.
pub mod kind {
    pub const LOCAL_REPOSITORY: &str = "Local repository";
    pub const ONEDRIVE: &str = "OneDrive repository";
    pub const S3: &str = "S3 bucket";
    pub const MIRROR: &str = "Folder mirror";
}

/// `Trigger` as a word. The core has no `title()` for triggers, so the mapping
/// lives here and is the only place it exists.
pub fn trigger(t: superbackup_core::state::Trigger) -> &'static str {
    use superbackup_core::state::Trigger as T;
    match t {
        T::Schedule => "Schedule",
        T::Manual => "Manual",
        T::Cli => "Command line",
        T::FileChange => "File change",
        T::CatchUp => "Catch-up",
        T::Retry => "Retry",
        // Named for what it is, not for what started it: the one thing a
        // reader of the history has to know about this run is that nothing
        // was written.
        T::Preview => "Preview (nothing written)",
    }
}

// ---------------------------------------------------------------------------
// 2. Onboarding
// ---------------------------------------------------------------------------

pub mod onboarding {
    pub const VAULT_NO_PATHS: &str =
        "This window does not know where to store the vault, so setup cannot be completed here. Run `superbackup init` in a terminal instead.";
    pub const VAULT_ALREADY: &str =
        "A vault already exists. Another window or a terminal created one while this was open —          close this and open superbackup again to unlock it.";

    pub const WELCOME_TITLE: &str = "Welcome to superbackup";
    pub const WELCOME_BODY: &str =
        "Set up encrypted, scheduled backups of the folders you work in. This takes about two minutes.";
    pub const WELCOME_F1_TITLE: &str = "One job, many copies";
    pub const WELCOME_F1_BODY: &str =
        "Send the same folders to a fast local disk, a folder you already sync, and offsite storage.";
    pub const WELCOME_F2_TITLE: &str = "Skips what you can rebuild";
    pub const WELCOME_F2_BODY: &str =
        "node_modules, build output and caches are left out, so backups stay small and finish quickly.";
    pub const WELCOME_F3_TITLE: &str = "Encrypted before it leaves";
    pub const WELCOME_F3_BODY: &str =
        "Everything is encrypted on this machine. Storage providers only ever see ciphertext.";
    pub const WELCOME_KOPIA: &str =
        "superbackup runs Kopia, an open-source backup engine. See kopia.io.";

    pub const PASS_TITLE: &str = "Create your master passphrase";
    pub const PASS_LEAD: &str = "This passphrase unlocks the vault that holds your repository encryption keys and storage keys. You will type it when you start superbackup, and when a backup needs to run unattended.";
    pub const PASS_NOT_REPO: &str = "This is not the passphrase of any single backup repository. It is the one that protects all of them.";
    pub const PASS_FIELD: &str = "Master passphrase";
    pub const PASS_CONFIRM: &str = "Confirm passphrase";
    pub const PASS_SUGGEST: &str = "Suggest a passphrase";
    pub const PASS_SUGGESTED: &str =
        "A six-word passphrase has been filled in. Save it before you continue.";
    pub const PASS_REQ_LENGTH: &str = "At least 12 characters";
    pub const PASS_REQ_UNIQUE: &str = "Not a password you use anywhere else";
    pub const PASS_REQ_WORDS: &str = "Four or more words is stronger than a short mix of symbols";

    pub const NORECOVERY_TITLE: &str = "There is no way to recover this";
    pub const NORECOVERY_BODY: &str = "Your master passphrase encrypts the vault on this machine. It is never sent anywhere, and it is not stored in a form anyone can read.\n\nThat means there is no reset link, no backdoor and no support address that can open your vault for you. If the passphrase is lost, the repository keys inside are lost with it, and the backups they protect cannot be read again.\n\nPut it in a password manager now, or write it down and keep the paper somewhere you would keep a spare key.";
    pub const NORECOVERY_COPY: &str = "Copy passphrase to clipboard";
    pub const NORECOVERY_COPIED: &str = "Copied. The clipboard will be cleared in 60 seconds.";
    pub const NORECOVERY_SAVE: &str = "Save a recovery sheet…";
    pub const NORECOVERY_SAVE_NOTE: &str =
        "The recovery sheet is a plain text file. Anyone who can read the file can read the passphrase.";
    pub const NORECOVERY_ACK: &str = "I have stored my master passphrase somewhere I can get to it. If I lose it, my backups cannot be recovered.";
    pub const WEAK_ACK: &str = "I understand this passphrase is weak and I want to use it anyway.";

    pub const SCAN_TITLE: &str = "Checking this machine";
    pub const SCAN_LEAD: &str = "Looking for the pieces superbackup can use.";
    pub const KOPIA_MISSING: &str = "Kopia was not found";
    pub const KOPIA_MISSING_BODY: &str = "superbackup uses Kopia to write and read backups. You can download a tested build now, or point superbackup at a copy you already have.";
    pub const KOPIA_DOWNLOAD: &str = "Download Kopia";
    pub const KOPIA_CHOOSE: &str = "Choose a file…";
    pub const KOPIA_SKIP_NOTE: &str =
        "You can set this up later. Backups will not run until Kopia is available.";
    pub const ONEDRIVE_CREATE: &str = "Create a OneDrive destination here";
    pub const ONEDRIVE_EXPLAIN: &str = "A repository is a small number of large files, not the millions of small ones that make OneDrive struggle. superbackup also marks the folder so OneDrive keeps it on this disk instead of turning it into an online-only placeholder.";
    pub const ONEDRIVE_NONE: &str = "No OneDrive folder was found. That is fine — you can back up to a local disk or to object storage instead.";

    pub const JOB_TITLE: &str = "Your first backup job";
    pub const JOB_LEAD: &str = "Pick a starting point. You can change everything afterwards.";
    pub const JOB_SOURCES: &str = "Folders to back up";
    pub const JOB_DESTINATIONS: &str = "Where to keep the copies";
    pub const JOB_LATER: &str =
        "Object storage such as StorJ or S3 can be added later, in Destinations.";
    pub const JOB_DERIVED: &str = "The repository encryption key is worked out from your master passphrase, so there is only one secret to keep safe.";
    pub const JOB_DERIVED_CHANGE: &str = "Change…";
    pub const JOB_NAME: &str = "Job name";
    pub const JOB_REVIEW: &str = "Ready to create";
    pub const JOB_ESTIMATE_NONE: &str = "Size could not be estimated. The job will still work.";

    pub const RUN_TITLE: &str = "Keep it running";
    pub const RUN_LEAD: &str = "Backups only help if they happen without you thinking about them.";
    pub const AUTOSTART_TITLE: &str = "Start superbackup when I sign in";
    pub const AUTOSTART_BODY: &str = "superbackup sits in the tray and runs your schedules.";
    pub const MINIMISED_TITLE: &str = "Start minimised to the tray";
    pub const MINIMISED_BODY: &str = "No window on sign-in. The tray icon shows the current state.";
    pub const SERVICE_TITLE: &str = "Install the background service";
    pub const SERVICE_BODY: &str = "The service runs backups even when nobody is signed in. To do that it needs the master passphrase without a person to type it, which means storing the key in this computer's credential store.";
    pub const SERVICE_KEYCHAIN_WARN: &str =
        "Anything that can run programs as you can then ask the credential store for the key.";
    pub const SERVICE_ELEVATE: &str = "Installing the service asks for administrator permission.";
    pub const SERVICE_DECLINED: &str =
        "The service was not installed. Backups will run while you are signed in.";

    pub const DONE_TITLE: &str = "You are set up";
    pub const DONE_TRAY: &str = "superbackup now lives in the tray. Closing the window does not stop backups — use Quit in the tray menu for that.";
    pub const DONE_PRIMARY: &str = "Back up now";
    pub const DONE_SECONDARY: &str = "Go to dashboard";

    pub const RESUME: &str =
        "Your master passphrase is already set. Continuing where you left off.";
    pub const ORPHAN_TITLE: &str = "The configuration is here, but the vault is missing";
    pub const ORPHAN_BODY: &str = "config.json refers to stored keys and passphrases that cannot be found. Restoring config.sbvault from a backup keeps everything as it was. Starting over keeps your jobs but needs every credential entered again.";
    pub const ORPHAN_RESTORE: &str = "Restore config.sbvault from a backup";
    pub const ORPHAN_STARTOVER: &str = "Start over and re-enter every credential";
    pub const REMOTE_FOUND: &str = "Remote configuration found";
}

pub fn onboarding_kopia_found(version: &str) -> String {
    format!("Kopia {version}")
}
pub fn onboarding_onedrive_found(account: &str) -> String {
    format!("OneDrive — {account}")
}
pub fn onboarding_disk_ok(free: u64, drive: &str) -> String {
    format!("{} free on {drive}", format::bytes(free))
}
pub fn onboarding_disk_low(free: u64, drive: &str) -> String {
    format!(
        "Only {} free on {drive}. A first backup of a development folder is often several gigabytes.",
        format::bytes(free)
    )
}
pub fn onboarding_job_estimate(size: u64, files: u64) -> String {
    format!("About {} in {} files after exclusions", format::bytes(size), format::count(files))
}
pub fn onboarding_service_keychain(keychain_name: &str) -> String {
    format!("Store the vault key in {keychain_name}")
}
pub fn onboarding_done_summary(jobs: &str, destinations: &str, next: &str) -> String {
    format!("{jobs} · {destinations} · Next run {next}")
}
pub fn onboarding_remote_body(url: &str) -> String {
    format!("A shared configuration is set up at {url}. Pulling it brings this machine's jobs and destinations in line with it.")
}

pub mod template {
    pub const DEV_TITLE: &str = "Development folder";
    pub const DEV_BODY: &str = "Your code, without the parts that rebuild themselves.";
    pub const DEV_DETAIL: &str =
        "Skips node_modules, build output, caches and virtualenvs. Applies 10 exclusion presets.";
    pub const DEV_EYEBROW: &str = "Recommended for developers";
    pub const DOCS_TITLE: &str = "Documents and desktop";
    pub const DOCS_BODY: &str = "The folders most people lose first.";
    pub const DOCS_DETAIL: &str = "Skips operating-system junk files and temporary files.";
    pub const HOME_TITLE: &str = "Whole user folder";
    pub const HOME_BODY: &str = "Everything under your home folder.";
    pub const HOME_DETAIL: &str =
        "Skips build output, caches and virtual machine images. Expect a large first run.";
    pub const BLANK_TITLE: &str = "Start from scratch";
    pub const BLANK_BODY: &str = "Choose the folders yourself.";
    pub const BLANK_DETAIL: &str = "No exclusions and no schedule until you add them.";
}

pub mod strength {
    pub const TOO_WEAK: &str = "Too weak";
    pub const WEAK: &str = "Weak";
    pub const GOOD: &str = "Good";
    pub const STRONG: &str = "Strong";
}

pub fn strength_label(level: &str) -> String {
    format!("Passphrase strength: {level}")
}

// ---------------------------------------------------------------------------
// 3. Vault and locking
// ---------------------------------------------------------------------------

pub mod vault {
    pub const UNLOCKED: &str = "Unlocked";
    pub const LOCKED: &str = "Locked";
    pub const LOCKED_SUB: &str = "Schedules are blocked";
    pub const LOCK_MENU_LOCK: &str = "Lock now";
    pub const LOCK_MENU_CHANGE: &str = "Change master passphrase…";
    pub const LOCK_MENU_SETTINGS: &str = "Auto-lock settings…";

    pub const BANNER_TITLE: &str = "The vault is locked";
    pub const BANNER_BODY: &str =
        "Scheduled backups will not start, and destinations cannot be reached, until it is unlocked.";
    pub const BANNER_ACTION: &str = "Unlock";

    pub const UNLOCK_TITLE: &str = "Unlock superbackup";
    pub const UNLOCK_BODY: &str = "Your master passphrase decrypts the repository encryption keys and storage keys needed to run backups.";
    pub const UNLOCK_FIELD: &str = "Master passphrase";
    pub const UNLOCK_REMEMBER: &str = "Remember until I sign out";
    pub const UNLOCK_BUTTON: &str = "Unlock";
    pub const UNLOCK_BUSY: &str = "Unlocking…";
    pub const UNLOCK_WRONG: &str =
        "That passphrase did not open the vault. Passphrases are case sensitive.";
    pub const UNLOCK_NO_RECOVERY: &str = "There is no way to recover a lost master passphrase. If you have a recovery sheet, this is the moment for it.";
    pub const UNLOCKED_TOAST: &str = "Vault unlocked";
    pub const LOCKED_TOAST: &str = "Vault locked";

    pub const AUTOLOCK_WARNING: &str = "superbackup will lock in one minute.";
    pub const AUTOLOCK_STAY: &str = "Stay unlocked";
}

pub mod locked {
    pub const ACTION_BLOCKED: &str = "Unlock the vault to use this.";
    pub const INLINE_PROMPT: &str = "Unlock to enter credentials";
    pub const RESTORE_TITLE: &str = "Unlock to browse your backups";
    pub const RESTORE_BODY: &str =
        "Listing snapshots needs the repository encryption key, which is kept in the vault.";
    pub const NEXT_RUN: &str = "blocked while locked";
    pub const PAUSED_NEXT_RUN: &str = "blocked while paused";
}

pub fn vault_locks_in(duration: &str) -> String {
    format!("Locks in {duration}")
}

// ---------------------------------------------------------------------------
// 4. Empty states
// ---------------------------------------------------------------------------

/// The shape every empty state shares: it explains what the thing is before it
/// offers the action.
pub struct Empty {
    pub title: &'static str,
    pub body: &'static str,
    pub primary: Option<&'static str>,
    pub secondary: Option<&'static str>,
}

pub mod empty {
    use super::Empty;

    pub const PREVIEW_WAITING: Empty = Empty {
        title: "Working out what would be copied",
        body: "superbackup is asking each destination what a backup would cost. Nothing is being \
               written.",
        primary: None,
        secondary: None,
    };
    pub const PREVIEW_NONE: Empty = Empty {
        title: "No preview yet",
        body: "Run a preview and the result appears here, one card per destination.",
        primary: None,
        secondary: None,
    };
    pub const MACHINES: Empty = Empty {
        title: "No computer has left a record here",
        body: "A record appears the first time a backup runs to this destination, so a human \
               opening the drive later can tell whose backups these are.",
        primary: None,
        secondary: None,
    };

    pub const JOBS: Empty = Empty {
        title: "No backup jobs yet",
        body: "A job is a set of folders, a schedule, and the places the copies go.",
        primary: Some("Create your first job"),
        secondary: Some("Import from another machine…"),
    };
    pub const JOBS_FILTERED: Empty = Empty {
        title: "No jobs match those filters",
        body: "Try a different search term, or clear the filters to see all your jobs.",
        primary: Some("Clear filters"),
        secondary: None,
    };
    /// Deliberately carries **no** action button.
    ///
    /// Both places that show it — the new-job wizard and the job editor —
    /// already have an "Add folder…" button directly beneath the list, so an
    /// identical button inside the empty state gave the user the same choice
    /// twice, one of which vanished as soon as a folder was added.
    pub const SOURCES: Empty = Empty {
        title: "No folders added yet",
        body: "Add folders below, or drag and drop them onto this window.",
        primary: None,
        secondary: None,
    };
    pub const DESTINATIONS: Empty = Empty {
        title: "No destinations yet",
        body: "A destination is a place backups are written to: a local disk, a folder you already sync, or object storage.",
        primary: Some("Add a destination"),
        secondary: Some("Learn about the four kinds"),
    };
    pub const DESTINATIONS_INJOB: Empty = Empty {
        title: "This job has nowhere to go",
        body: "Add a destination first, then choose it here.",
        primary: Some("Add a destination"),
        secondary: None,
    };
    pub const PROVIDERS: Empty = Empty {
        title: "No storage providers yet",
        body: "A provider holds the endpoint and keys for an object-storage account. Define it once and reuse it for every bucket. Local disks and OneDrive folders do not need one.",
        primary: Some("Add a storage provider"),
        secondary: None,
    };
    pub const ACTIVITY: Empty = Empty {
        title: "Nothing has run yet",
        body: "Runs appear here as soon as a job starts, whether it was scheduled or you started it yourself.",
        primary: Some("Back up now"),
        secondary: None,
    };
    pub const ACTIVITY_FILTERED: Empty = Empty {
        title: "No runs match those filters",
        body: "superbackup keeps the last 200 runs. Older activity is in the event log.",
        primary: Some("Clear filters"),
        secondary: None,
    };
    pub const EVENTS: Empty = Empty {
        title: "No events recorded",
        body: "Events are written as things happen: jobs starting, repositories being created, the vault being unlocked.",
        primary: None,
        secondary: None,
    };
    pub const RESTORE_NO_DESTINATIONS: Empty = Empty {
        title: "Nothing to restore from yet",
        body: "Restoring needs a repository destination. Folder mirrors can be opened directly in your file manager instead.",
        primary: Some("Add a destination"),
        secondary: None,
    };
    pub const RESTORE_NO_SNAPSHOTS: Empty = Empty {
        title: "No snapshots yet",
        body: "Once a job has run successfully, its snapshots appear here and you can browse them file by file.",
        primary: Some("Back up now"),
        secondary: None,
    };
    pub const RESTORE_MIRRORS_ONLY: Empty = Empty {
        title: "Your destinations are folder mirrors",
        body: "A mirror is a plain copy of your files. Open the folder and copy what you need — there is no snapshot history to browse.",
        primary: None,
        secondary: None,
    };
    pub const SNAPSHOT_DIR: Empty = Empty {
        title: "This folder was empty in this snapshot",
        body: "Try an earlier snapshot from the picker above.",
        primary: None,
        secondary: None,
    };
    pub const VAULT_BACKUPS: Empty = Empty {
        title: "No vault backups yet",
        body: "A copy of the vault is written here before every change to it.",
        primary: None,
        secondary: None,
    };
}

pub fn empty_jobs_filtered_body(total: usize) -> String {
    format!("Try a different search term, or clear the filters to see all {total} jobs.")
}

// ---------------------------------------------------------------------------
// 5. Dashboard
// ---------------------------------------------------------------------------

pub mod dash {
    pub const HEALTH_LABEL: &str = "Overall health";
    pub const HEALTH_IDLE_NEVER: &str = "No backups yet";
    pub const HEALTH_PAUSED_FOREVER: &str = "Paused until you resume";
    pub const HEALTH_ATT_LOCKED: &str = "The vault is locked";
    pub const HEALTH_ATT_KOPIA: &str = "Kopia was not found";

    pub const NEXT_LABEL: &str = "Next scheduled run";
    pub const NEXT_NONE: &str = "Not scheduled";
    pub const NEXT_NONE_ACTION: &str = "Set up a schedule";

    pub const WEEK_LABEL: &str = "Last 7 days";
    pub const WEEK_NONE: &str = "Nothing has run in the last 7 days";

    pub const RUNNING_TITLE: &str = "Running now";
    pub const RUNNING_STOP_ALL: &str = "Stop all";

    pub const JOBS_TITLE: &str = "Jobs";
    pub const JOBS_RUN_ALL: &str = "Run all now";
    pub const JOBS_DISABLE_ALL: &str = "Disable all jobs";
    pub const JOBS_NEW: &str = "New job…";
    pub const VIEW_ERROR: &str = "View error";
}

pub fn dash_health_idle_last(relative: &str) -> String {
    format!("Last backup {relative}")
}
pub fn dash_health_running(count: usize) -> String {
    format!("{count} running")
}
pub fn dash_health_paused_until(time: &str) -> String {
    format!("Paused until {time}")
}
pub fn dash_health_paused_reason(time: &str, reason: &str) -> String {
    format!("Paused until {time} — {reason}")
}
pub fn dash_health_failed(job: &str, relative: &str) -> String {
    format!("{job} failed {relative}")
}
pub fn dash_health_failed_more(job: &str, relative: &str, count: usize) -> String {
    format!("{job} failed {relative}, and {count} others")
}
pub fn dash_health_att_stale(count: usize, days: u32) -> String {
    format!("{count} jobs have not succeeded for {days} days")
}
pub fn dash_health_att_unverified(count: usize) -> String {
    format!("{count} destinations have never been verified")
}
pub fn dash_next_value(job: &str, absolute: &str) -> String {
    format!("{job} · {absolute}")
}
pub fn dash_week_summary(runs: usize, failed: usize) -> String {
    format!("{runs} runs, {failed} failed")
}
pub fn dash_week_day_tooltip(date: &str, succeeded: usize, warned: usize, failed: usize) -> String {
    format!("{date}: {succeeded} succeeded, {warned} with warnings, {failed} failed")
}
pub fn dash_running_started(relative: &str, trigger: &str) -> String {
    format!("Started {relative} · triggered by {trigger}")
}
pub fn dash_running_counts(
    files_done: u64,
    files_total: u64,
    bytes_done: u64,
    bytes_total: u64,
    rate: f64,
) -> String {
    format!(
        "{} of {} files · {} of {} · {}",
        format::count(files_done),
        format::count(files_total),
        format::bytes(bytes_done),
        format::bytes(bytes_total),
        format::rate(rate)
    )
}
pub fn dash_running_counts_partial(files_done: u64, bytes_done: u64, rate: f64) -> String {
    format!(
        "{} files · {} · {}",
        format::count(files_done),
        format::bytes(bytes_done),
        format::rate(rate)
    )
}
pub fn dash_running_eta(duration: &str) -> String {
    format!("~{duration} left")
}
pub fn dash_running_scanning(path: &str) -> String {
    format!("Scanning {path}")
}
pub fn dash_running_cached_tooltip(count: u64) -> String {
    format!("{} files unchanged since the last run", format::count(count))
}
pub fn dash_running_skipped(count: u64) -> String {
    format!("{} files skipped", format::count(count))
}

pub fn card_meta_succeeded(relative: &str, duration: &str, bytes: u64) -> String {
    format!("Last run {relative} · {duration} · {} uploaded", format::bytes(bytes))
}
pub fn card_meta_warnings(relative: &str, duration: &str, skipped: u64) -> String {
    format!("Last run {relative} · {duration} · {} files skipped", format::count(skipped))
}
pub fn card_meta_failed(relative: &str, ordinal: &str) -> String {
    format!("Failed {relative} · {ordinal} failure in a row")
}
pub fn card_meta_failed_first(relative: &str) -> String {
    format!("Failed {relative}")
}
pub fn card_meta_running(relative: &str, trigger: &str) -> String {
    format!("Started {relative} · {trigger}")
}
pub fn card_meta_queued(job: &str) -> String {
    format!("Queued behind {job}")
}
pub fn card_meta_never(sources: usize, relative: &str) -> String {
    format!("Never run · {sources} folders · next run {relative}")
}
pub fn card_meta_never_manual(sources: usize) -> String {
    format!("Never run · {sources} folders · runs only when you ask")
}
pub fn card_meta_disabled(status: &str, relative: &str) -> String {
    format!("Disabled · last result {status} {relative}")
}
pub fn card_meta_stale(relative: &str) -> String {
    format!("Last success {relative}")
}

// ---------------------------------------------------------------------------
// 6. Jobs
// ---------------------------------------------------------------------------

pub mod jobs {
    pub const TITLE: &str = "Jobs";
    pub const NEW: &str = "New job";
    pub const SEARCH: &str = "Search jobs";
    pub const GROUP_BY: &str = "Group by";
    pub const GROUP_NONE: &str = "None";
    pub const GROUP_PROJECT: &str = "Project";
    pub const GROUP_SCHEDULE: &str = "Schedule";
    pub const FILTER: &str = "Filter";
    pub const FILTER_ALL: &str = "All";
    pub const FILTER_ENABLED: &str = "Enabled";
    pub const FILTER_DISABLED: &str = "Disabled";
    pub const FILTER_FAILING: &str = "Failing";
    pub const FILTER_STALE: &str = "Not backed up recently";
    pub const UNGROUPED: &str = "Ungrouped";
    pub const RUN_GROUP: &str = "Run group";
}

pub fn jobs_selected(count: usize) -> String {
    format!("{count} selected")
}

pub mod col {
    pub const STATUS: &str = "Status";
    pub const NAME: &str = "Name";
    pub const SOURCES: &str = "Folders";
    pub const DESTINATIONS: &str = "Destinations";
    pub const SCHEDULE: &str = "Schedule";
    pub const LAST_RUN: &str = "Last run";
    pub const NEXT_RUN: &str = "Next run";
    pub const UPLOADED: &str = "Uploaded";
    pub const LOCATION: &str = "Location";
    pub const USED_BY: &str = "Used by";
    pub const SIZE: &str = "Size";
    pub const LAST_VERIFIED: &str = "Last verified";
    pub const ENDPOINT: &str = "Endpoint";
    pub const STARTED: &str = "Started";
    pub const JOB: &str = "Job";
    pub const TRIGGER: &str = "Started by";
    pub const DURATION: &str = "Duration";
    pub const SEVERITY: &str = "Severity";
    pub const TIME: &str = "Time";
    pub const EVENT: &str = "Event";
    pub const MESSAGE: &str = "Message";
    pub const MODIFIED: &str = "Modified";
    pub const WHEN: &str = "When";
    pub const FILES: &str = "Files";
    pub const ID: &str = "Id";
    pub const ACTIONS: &str = "Actions";
    pub const KIND: &str = "Kind";
    pub const FLAVOUR: &str = "Flavour";
}

pub mod job {
    pub const TAB_SOURCES: &str = "Folders";
    pub const TAB_DESTINATIONS: &str = "Destinations";
    pub const TAB_SCHEDULE: &str = "Schedule";
    pub const TAB_EXCLUSIONS: &str = "Exclusions";
    pub const TAB_ADVANCED: &str = "Advanced";

    pub const NAME: &str = "Name";
    pub const NAME_PLACEHOLDER: &str = "Dev code";
    pub const DESCRIPTION: &str = "Description";
    pub const DESCRIPTION_PLACEHOLDER: &str = "What this job is for";
    pub const PROJECT: &str = "Project";
    pub const PROJECT_NEW: &str = "New project…";
    pub const TAGS: &str = "Tags";
    pub const TAGS_PLACEHOLDER: &str = "Add a tag";

    pub const SOURCES_TITLE: &str = "Folders to back up";
    pub const SOURCES_ADD: &str = "Add folder…";
    pub const SOURCES_HINT: &str = "Everything under each folder is included, minus your exclusions. You can also drop folders onto this window.";
    pub const FOLLOW_SYMLINKS: &str = "Follow symbolic links";
    pub const FOLLOW_TOOLTIP: &str = "Off by default. Following links out of the folder is how a backup of one project quietly grows to cover the whole disk.";
    pub const ONE_FILESYSTEM: &str = "Stay on one filesystem";
    pub const ONE_FS_TOOLTIP: &str =
        "Do not cross into mounted drives or network shares found inside this folder.";
    pub const SOURCES_MISSING: &str =
        "This folder is not there at the moment. The job will skip it and record a warning.";
    pub const SOURCES_DUP: &str = "That folder is already in this job.";

    pub const DEST_TITLE: &str = "Send this backup to";
    pub const DEST_LEAD: &str =
        "Every destination you tick receives a complete copy. A failure at one does not stop the others.";
    pub const DEST_NEW: &str = "New destination…";
    pub const DEST_NEVER_VERIFIED: &str = "Never verified";
    pub const DEST_UNREACHABLE: &str = "Unreachable";
    pub const DEST_DISABLED_ROW: &str = "This destination is switched off, so jobs skip it.";
    pub const DEST_DISABLED_ENABLE: &str = "Enable in Destinations";
    pub const DEST_CONTINUE_ON_ERROR: &str = "Keep going to the other destinations";
    pub const DEST_CONTINUE_BODY: &str = "With this off, the first destination that fails stops the run and the rest are recorded as cancelled.";
    pub const DEST_MIXED_WARNING: &str = "This job writes to both a repository and a folder mirror. Mirrors hold one plain copy with no history, so retention and encryption settings do not apply to them.";
    pub const ERR_NO_DESTINATIONS: &str =
        "Choose at least one destination. A job with nowhere to write cannot run.";

    pub const SCHEDULE_MANUAL: &str = "Manual only";
    pub const SCHEDULE_MANUAL_BODY: &str = "Runs when you ask, or when the command line asks.";
    /// "Every so often" said nothing about how often, which is the only thing
    /// the option is choosing. The interval field beside it supplies the number.
    pub const SCHEDULE_INTERVAL: &str = "Every hour";
    /// Shown on hover, so the summary line can stay short.
    pub const SCHEDULE_MANUAL_TIP: &str =
        "Nothing runs on its own. Use \"Back up now\", or `superbackup run <job>`.";
    pub const SCHEDULE_INTERVAL_TIP: &str =
        "Repeats on a fixed clock from when superbackup started, regardless of the time of day. Set the interval beside it — hourly is the default.";
    pub const SCHEDULE_DAILY_TIP: &str =
        "Runs at the times you list, every day, in this machine's local time. Handles daylight saving in both directions.";
    pub const SCHEDULE_WEEKLY_TIP: &str =
        "Runs at the times you list, but only on the weekdays you tick.";
    pub const SCHEDULE_WORKHOURS_TIP: &str =
        "Hourly between 08:00 and 17:00, Monday to Friday. Nothing runs overnight or at the weekend, so a large job cannot tie up the machine while you are away.";
    pub const SCHEDULE_CRON_TIP: &str =
        "A five-field cron expression, evaluated in local time. Use this when the other options cannot express what you need.";
    pub const SCHEDULE_ONCHANGE_TIP: &str =
        "Watches the folders and runs once they have been quiet for the waiting period. Files matched by your exclusions never wake it.";
    pub const SCHEDULE_WORKHOURS: &str = "During work hours";
    pub const SCHEDULE_INTERVAL_UNIT: &str = "minutes";
    pub const SCHEDULE_INTERVAL_WARN: &str =
        "Running more often than every 15 minutes keeps the disk busy on large folders.";
    pub const SCHEDULE_DAILY: &str = "Daily at";
    pub const SCHEDULE_WEEKLY: &str = "Weekly on";
    pub const SCHEDULE_ADD_TIME: &str = "Add time…";
    pub const SCHEDULE_CRON: &str = "Cron expression";
    pub const SCHEDULE_CRON_HELP: &str = "Cron help";
    pub const SCHEDULE_ONCHANGE: &str = "When files change";
    pub const SCHEDULE_DEBOUNCE: &str = "Wait for quiet";
    pub const SCHEDULE_DEBOUNCE_UNIT: &str = "seconds";
    pub const SCHEDULE_MIN_INTERVAL: &str = "At most once every";
    pub const SCHEDULE_MIN_UNIT: &str = "minutes";
    pub const SCHEDULE_ONCHANGE_BODY: &str = "The job runs once the folders have been quiet for the waiting period, and never more often than the minimum interval.";
    pub const SCHEDULE_ONCHANGE_LARGE: &str = "One of these folders holds more than 50,000 files. Watching a tree that size uses noticeable memory.";
    pub const SCHEDULE_NEXT_NONE: &str = "This job runs only when you ask.";

    pub const CONDITIONS_TITLE: &str = "Run conditions";
    pub const CONDITIONS_METERED: &str = "Skip when on a metered connection";
    pub const CONDITIONS_BATTERY: &str = "Skip when on battery";
    pub const CONDITIONS_USING_GLOBAL: &str = "Using the global setting";
    pub const CONDITIONS_OVERRIDING: &str = "Overriding the global setting";
    pub const CONDITIONS_RESET: &str = "Reset";
    pub const TIMEOUT: &str = "Stop the run after";
    pub const TIMEOUT_UNIT: &str = "minutes";
    pub const TIMEOUT_BODY: &str = "A run stopped by its timeout is recorded as failed, because something took longer than it should have.";

    pub const EXCL_TITLE: &str = "Exclusions";
    pub const EXCL_LEAD: &str = "Leaving out files you can rebuild is what keeps a developer backup small enough to finish every night.";
    pub const EXCL_SELECT_DEFAULTS: &str = "Select developer defaults";
    pub const EXCL_CLEAR_ALL: &str = "Clear all";
    pub const EXCL_RISKY: &str = "Excluding this can lose work that exists nowhere else.";
    pub const EXCL_TOTAL_TITLE: &str = "Everything this job will skip";
    pub const EXCL_TOTAL_BODY: &str =
        "The complete list, from the presets above plus any patterns you added. Anything not matched here is backed up.";
    pub const EXCL_TOTAL_EMPTY: &str =
        "Nothing is excluded. Every file under your chosen folders will be backed up.";
    /// Shown under the total, so the count is not mistaken for a file count.
    pub fn excl_total_count(patterns: usize) -> String {
        if patterns == 1 {
            "1 pattern".to_string()
        } else {
            format!("{patterns} patterns")
        }
    }
    pub const EXCL_GITIGNORE: &str = "Use .gitignore files found in the folders";
    pub const EXCL_GITIGNORE_BODY: &str = "Honours each repository's own ignore rules. Slower on very large trees, because every directory is checked.";
    pub const EXCL_CACHEDIR: &str = "Skip folders tagged with CACHEDIR.TAG";
    pub const EXCL_CACHEDIR_BODY: &str =
        "A standard marker that tools use to say a folder holds only regenerable cache.";
    pub const EXCL_MAX_SIZE: &str = "Skip files larger than";
    pub const EXCL_MAX_SIZE_UNIT: &str = "MB";
    pub const EXCL_MAX_SIZE_BODY: &str =
        "Files over this size are left out of every snapshot and listed in the run's warnings.";
    pub const EXCL_CUSTOM: &str = "Your own patterns";
    pub const EXCL_CUSTOM_BODY: &str =
        "One pattern per line, in .gitignore syntax, relative to each folder you back up.";
    pub const EXCL_CUSTOM_PLACEHOLDER: &str = "/**/*.psd\n/**/coverage/\nsecrets.local.json";
    pub const EXCL_IMPACT_NONE: &str =
        "These rules do not match anything in the folders you chose.";
    pub const EXCL_IMPACT_FAILED: &str = "The size of the excluded files could not be worked out.";

    pub const BANDWIDTH_TITLE: &str = "Bandwidth";
    pub const BANDWIDTH_GLOBAL: &str = "Use the global limit";
    pub const BANDWIDTH_CUSTOM: &str = "Set a limit for this job";
    pub const BANDWIDTH_UPLOAD: &str = "Upload limit";
    pub const BANDWIDTH_DOWNLOAD: &str = "Download limit";
    pub const BANDWIDTH_UNIT: &str = "kB/s";
    pub const BANDWIDTH_NO_WINDOW: &str = "The daily window is a global setting, so two jobs can never disagree about it. Set it in Settings › Bandwidth.";

    pub const RETENTION_TITLE: &str = "Retention";
    pub const RETENTION_PER_DEST: &str = "Use each destination's policy";
    pub const RETENTION_CUSTOM: &str = "Set a policy for this job";
    pub const RETENTION_LATEST: &str = "Latest";
    pub const RETENTION_HOURLY: &str = "Hourly";
    pub const RETENTION_DAILY: &str = "Daily";
    pub const RETENTION_WEEKLY: &str = "Weekly";
    pub const RETENTION_MONTHLY: &str = "Monthly";
    pub const RETENTION_ANNUAL: &str = "Annual";
    pub const RETENTION_MAINTENANCE: &str = "Run maintenance every";
    pub const RETENTION_MAINTENANCE_UNIT: &str = "successful runs";
    pub const RETENTION_MIRROR_NOTE: &str =
        "Retention applies to repositories. A folder mirror always holds exactly one copy.";

    pub const HOOKS_TITLE: &str = "Hooks";
    pub const HOOKS_BEFORE: &str = "Before the backup";
    pub const HOOKS_AFTER_SUCCESS: &str = "After a successful backup";
    pub const HOOKS_AFTER_FAILURE: &str = "After a failed backup";
    pub const HOOKS_ABORT: &str = "Cancel the backup if this command fails";
    pub const HOOKS_WARNING: &str =
        "Hooks run as you, with your permissions. superbackup does not restrict what they can do.";
    pub const HOOKS_ENV: &str = "Available to the command: SUPERBACKUP_JOB_NAME, SUPERBACKUP_RUN_ID, SUPERBACKUP_STATUS, SUPERBACKUP_DESTINATIONS. Each command is stopped after 120 seconds, and its output is kept with the run.";

    pub const DANGER_TITLE: &str = "Danger zone";
    pub const DANGER_DELETE: &str = "Delete this job";
    pub const DANGER_BODY: &str = "Deleting removes the job definition from this machine. Snapshots already written to any destination are left exactly as they are.";
    pub const UNSAVED_DISCARD: &str = "Discard";
}

pub const RETENTION_ERR_ALL_ZERO: &str = "At least one of these needs to be above zero, or every snapshot would be removed as soon as it is written.";

pub fn job_sources_child(parent: &str) -> String {
    format!("That folder is already covered by {parent}.")
}
pub fn job_sources_parent(path: &str, count: usize) -> String {
    format!(
        "{path} contains {count} folders already in this job. Replace them with the parent folder?"
    )
}
pub fn job_dest_verified(relative: &str) -> String {
    format!("Verified {relative}")
}
pub fn job_schedule_next_five(times: &str) -> String {
    format!("Next five runs: {times}")
}
pub fn job_excl_defaults_applied(patterns: usize) -> String {
    format!("Developer defaults applied: 10 presets, {patterns} patterns.")
}
pub fn job_excl_patterns_count(count: usize) -> String {
    format!("{count} patterns")
}
pub fn job_excl_show_effective(count: usize) -> String {
    format!("Show all effective patterns ({count})")
}
pub fn job_excl_impact(size: u64, files: u64) -> String {
    format!(
        "These rules leave out about {} in {} files.",
        format::bytes(size),
        format::count(files)
    )
}
pub fn job_bandwidth_current_global(upload: &str, download: &str) -> String {
    format!("Global limit: {upload} up, {download} down")
}
pub fn job_retention_summary(
    latest: u32,
    hourly: u32,
    daily: u32,
    weekly: u32,
    monthly: u32,
    annual: u32,
) -> String {
    format!("Keeps the {latest} most recent snapshots, then {hourly} hourly, {daily} daily, {weekly} weekly, {monthly} monthly and {annual} annual snapshots.")
}
pub fn job_unsaved_title(job: &str) -> String {
    format!("Save your changes to {job}?")
}
pub fn job_unsaved_body(tabs: &str) -> String {
    format!("You have unsaved changes on the {tabs} tab.")
}

// ---------------------------------------------------------------------------
// 7. Destinations
// ---------------------------------------------------------------------------

pub mod dest {
    pub const KIND_BUCKET: &str = "bucket";
    pub const KIND_ONEDRIVE: &str = "OneDrive folder";
    pub const KIND_FOLDER: &str = "folder";

    pub const REPO_CREATE_BUTTON: &str = "Set up repository";
    pub const REPO_CREATE_BULLET_KEY: &str =
        "A repository encryption key is generated and stored in your vault. You can write it down afterwards from this destination's Encryption panel.";
    pub const REPO_CREATE_BULLET_SETTINGS: &str =
        "Encryption settings are fixed when the repository is created and cannot be changed later. The recommended defaults are used unless you have changed them.";
    pub const REPO_CREATE_BULLET_LATER: &str =
        "You can skip this and set it up later from the destination, but no job can write here until you do.";
    pub const TITLE: &str = "Destinations";
    pub const NEW: &str = "New destination";
    pub const SEARCH: &str = "Search destinations";
    pub const FILTER_KIND: &str = "Kind";
    pub const AUTO_FOUND: &str = "Found automatically";
    pub const STATUS_READY: &str = "Ready";
    pub const STATUS_NOT_CONNECTED: &str = "Not connected";
    pub const STATUS_UNREACHABLE: &str = "Unreachable";
    pub const USED_BY_NONE: &str = "Not used yet";

    pub const NAME: &str = "Name";
    pub const NAME_PLACEHOLDER: &str = "Local repo";
    pub const KIND: &str = "Kind";
    pub const KIND_LOCKED: &str = "The kind is fixed once a destination exists. Create a new destination to use a different one.";
    pub const ENABLED: &str = "Enabled";
    pub const ENABLED_BODY: &str =
        "A switched-off destination is skipped by every job, without failing them.";

    pub const FOLDER: &str = "Folder";
    pub const FOLDER_WILL_CREATE: &str = "This folder will be created.";
    pub const FOLDER_REMOVABLE: &str = "Removable drive — backups run only while it is connected.";
    pub const FOLDER_NETWORK: &str =
        "Network location — backups depend on the share being reachable.";
    pub const FOLDER_FOUND_REPO: &str = "There is already a repository here.";
    pub const FOLDER_FOUND_REPO_ACTION: &str = "Connect to it";

    pub const ONEDRIVE_ACCOUNT: &str = "Account";
    pub const ONEDRIVE_ACCOUNT_BODY: &str =
        "A label for your own benefit. superbackup does not sign in to OneDrive and does not need to.";
    pub const ONEDRIVE_EXPLAIN: &str = "The backup is written as a repository: a modest number of large files rather than the millions of small ones that make OneDrive struggle. That is the whole point of putting it here.";
    pub const ONEDRIVE_PIN: &str = "Keep these files on this disk";
    pub const ONEDRIVE_PIN_BODY: &str = "Stops OneDrive turning the repository into online-only placeholders. With this off, a restore may have to download before it can read.";
    pub const ONEDRIVE_REDETECT: &str = "Check for OneDrive again";

    pub const S3_PROVIDER: &str = "Storage provider";
    pub const S3_PROVIDER_NEW: &str = "New provider…";
    pub const S3_PROVIDER_EDIT: &str = "Edit provider";
    pub const S3_BUCKET: &str = "Bucket";
    pub const S3_LIST_BUCKETS: &str = "List buckets";
    pub const S3_BUCKET_HELPER: &str = "Type the name, or list what this provider can see.";
    pub const S3_BUCKET_CHOOSE: &str = "Choose a bucket";
    pub const S3_BUCKET_TYPE: &str = "Type a name instead";
    pub const S3_BUCKET_LISTING: &str = "Asking the provider…";
    pub const S3_BUCKET_RETRY: &str = "Try again";
    pub const S3_BUCKET_LOCKED: &str =
        "Unlock the vault to list this provider's buckets. You can type the name meanwhile.";
    pub const S3_BUCKET_UNSAVED: &str =
        "Save the provider first to list its buckets. You can type the name meanwhile.";
    pub const S3_PREFIX_UNKNOWN: &str = "What is already stored here could not be checked.";
    pub const S3_ADMIN_OPEN: &str = "Administration panel";
    pub const S3_PREFIX_CHECK: &str = "Check this prefix";
    pub const S3_PREFIX_HAS_REPO: &str = "A repository already exists at this prefix. Adding a destination here will connect to it rather than create a new one.";
    pub const S3_PREFIX_EMPTY: &str = "Nothing is stored at this prefix yet.";
    pub const S3_PREFIX_OCCUPIED: &str =
        "This prefix already holds other objects, but no repository.";
    pub const S3_PREFIX: &str = "Key prefix";
    pub const S3_PREFIX_BODY: &str = "The default contains this machine's folder name, which is what keeps several computers and several jobs apart inside one bucket.";
    pub const S3_CREDS: &str = "Credentials for this bucket";
    pub const S3_CREDS_INHERIT: &str = "Use the provider's credentials";
    pub const S3_CREDS_OWN: &str = "Use a separate key pair for this bucket";
    pub const S3_CREDS_OWN_BODY: &str =
        "A key that only reaches this bucket limits what a leaked credential can touch.";

    pub const MIRROR_EXPLAIN: &str = "A mirror is a plain, readable copy of the newest version of each file. There are no snapshots, no history, no deduplication and no encryption — anyone who can read the folder can read your files.";
    pub const MIRROR_PRUNE: &str = "Delete files in the mirror that no longer exist in the folders";
    pub const MIRROR_PRUNE_BODY: &str = "With this on, deleting a file removes it from the mirror on the next run, so the mirror stops protecting you from an accidental deletion.";

    pub const VERIFY_CHECKING_PATH: &str = "Checking the path…";
    pub const VERIFY_WRITING: &str = "Writing a test file…";
    pub const VERIFY_OPENING: &str = "Opening the repository…";
    pub const VERIFY_HEAD: &str = "Reaching the bucket…";
    pub const VERIFY_OK: &str = "Verified. Everything needed is in place.";

    pub const CONNECT_TITLE: &str = "Connect to this repository";
    pub const CONNECT_BODY: &str = "There is already a repository at this location. Its passphrase is needed once, and is then kept in your vault.";
    pub const CONNECT_DERIVE: &str = "Work it out from my master passphrase";
    pub const CONNECT_TYPE: &str = "I will type it";
    pub const CONNECT_FIELD: &str = "Repository encryption key";
    pub const CONNECT_WRONG: &str = "That passphrase did not open this repository.";
    pub const CONNECT_SETTINGS_NOTE: &str =
        "These settings were chosen when the repository was created and cannot be changed.";

    pub const DELETE_BODY: &str =
        "This removes the destination from superbackup. The data at the destination is not touched.";
    pub const DELETE_ALSO_FILES_WARN: &str =
        "Every snapshot in this repository would be gone. There is no undo.";
    pub const DELETE_BUTTON: &str = "Remove destination";
    pub const DELETE_BUTTON_FILES: &str = "Delete destination and its files";
    pub const DELETE_S3_NOTE: &str = "Objects in a bucket are not deleted from here. Remove them with your provider's tools if you want the space back.";
    pub const DELETE_COPY_PREFIX: &str = "Copy the prefix";
}

pub fn dest_used_by(count: usize) -> String {
    format::plural(count, "job", "jobs")
}
pub fn dest_folder_free(free: u64, total: u64) -> String {
    format!("{} free of {}", format::bytes(free), format::bytes(total))
}
pub fn dest_folder_low(free: u64) -> String {
    format!(
        "Only {} free. A first backup of a development folder is often several gigabytes.",
        format::bytes(free)
    )
}
pub fn dest_s3_full_path(bucket: &str, prefix: &str) -> String {
    format!("Full path: s3://{bucket}/{prefix}")
}
pub fn dest_s3_prefix_normalised(prefix: &str) -> String {
    format!("Saved as {prefix}")
}
pub fn dest_s3_creds_inherit_body(provider: &str) -> String {
    format!("Uses the keys stored on {provider}.")
}
pub fn dest_verify_ok_toast(name: &str) -> String {
    format!("{name} verified")
}
pub fn dest_delete_title(name: &str) -> String {
    format!("Remove {name}?")
}
pub fn dest_delete_jobs(count: usize, names: &str) -> String {
    format!("{count} jobs write here and will keep running to their other destinations: {names}")
}
pub fn dest_delete_orphans(names: &str) -> String {
    format!("These jobs would be left with nowhere to write and will be switched off: {names}")
}
pub fn dest_delete_also_files(path: &str) -> String {
    format!("Also delete the repository files at {path}")
}
pub fn confirm_type_name(name: &str) -> String {
    format!("Type {name} to confirm")
}

/// Chained destinations: filling one destination from another rather than
/// from the sources a second time.
///
/// The whole difficulty of this feature is a single fact that the interface
/// has to say out loud, because guessing wrong about it loses data: a replica
/// **is the same repository as its source**, with the same encryption key and
/// the same passphrase. `kopia repository sync-to` copies the format blob, so
/// there is no version of this where the offsite copy has a key of its own.
/// Someone who believes otherwise will store one passphrase, lose the other,
/// and discover at restore time that their "independent" second copy was never
/// independent.
pub mod chain {
    pub const TITLE: &str = "Where this destination gets its data";

    pub const FROM_SOURCES: &str = "From the job's folders";
    pub const FROM_SOURCES_HELP: &str =
        "Read the folders, then pack and encrypt them into this repository.";

    pub const FROM_DESTINATION: &str = "Copied from another destination";
    pub const FROM_DESTINATION_HELP: &str =
        "Copy an existing repository here block for block. The folders are read once, by the first destination, instead of once per destination — which is what makes a slow offsite upload cheap.";

    pub const PICK_LABEL: &str = "Copy from";
    pub const PICK_PLACEHOLDER: &str = "Choose a destination…";
    pub const PICK_EMPTY: &str =
        "There is no other repository destination to copy from yet. Add one first — a local folder or OneDrive is the usual choice — and it can then feed this one.";

    /// The one thing a user must not misunderstand.
    pub const SHARED_KEY_TITLE: &str = "This copy shares the source's encryption key";
    pub const SHARED_KEY_BODY: &str =
        "A copy is the same repository in a second place, not a second repository. It is opened with the source's passphrase, and it has no separate key of its own. Keep that one passphrase safe and both copies stay readable; lose it and neither can be restored.";

    /// Shown in place of the encryption panel.
    pub const ENCRYPTION_INHERITED: &str = "Encryption is inherited from the source";
    pub const ENCRYPTION_INHERITED_BODY: &str =
        "Algorithm, hash, block splitting and passphrase all come from the source destination. There is nothing to choose here, and nothing separate to write down.";

    /// Shown in place of "Create repository".
    pub const NO_CREATE: &str = "Nothing to create here";
    pub const NO_CREATE_BODY: &str =
        "The first copy creates this repository as part of the job. Creating one here first would make a different, empty repository that the copy would then refuse to write over.";

    pub const NEEDS_SOURCE_IN_JOB: &str = "Also back up the source";
    pub const CHAIN_BADGE: &str = "Copy";

    /// How the chain reads on the job editor and the run detail.
    pub fn chain_line(source: &str) -> String {
        format!("Copied from {source} after it finishes")
    }

    pub fn source_missing(id: &str) -> String {
        format!("The destination this one copies from ({id}) no longer exists")
    }
}

pub mod enc {
    pub const TITLE: &str = "Encryption";
    pub const LEAD: &str =
        "These settings are fixed when the repository is created and cannot be changed afterwards.";
    pub const SUMMARY: &str =
        "Recommended settings — AES-256-GCM, BLAKE2B-256, dynamic 4 MB blocks, no error correction.";
    pub const CHANGE: &str = "Change…";

    pub const ALGORITHM: &str = "Encryption";
    pub const HASH: &str = "Hash";
    pub const SPLITTER: &str = "Block splitter";
    pub const RECOMMENDED: &str = "Recommended";

    pub const HASH_BLAKE2B256: &str = "Default. Fast and well studied.";
    pub const HASH_BLAKE2B256128: &str =
        "Half-length hashes. Slightly smaller indexes, slightly higher chance of a collision.";
    pub const HASH_BLAKE3256: &str = "Fastest on modern processors. Newer than the others.";
    pub const HASH_BLAKE2S256: &str = "Tuned for 32-bit processors.";
    pub const HASH_HMACSHA256: &str = "Widely audited. Slower than BLAKE2.";
    pub const HASH_HMACSHA256128: &str = "Half-length variant of HMAC-SHA256.";

    pub const SPLITTER_BODY: &str = "How files are cut into blocks before they are stored. Smaller blocks deduplicate small files better and make the index larger.";
    pub const SPLITTER_SUGGEST: &str =
        "Your folders hold a lot of small files. DYNAMIC-2M-BUZHASH deduplicates them better.";
    pub const SPLITTER_SUGGEST_ACTION: &str = "Use it";

    pub const ECC: &str = "Add error-correcting data";
    pub const ECC_BODY: &str = "Stores extra data so a repository survives a limited amount of corruption. It costs the overhead you choose in extra storage, and it does nothing about a whole disk failing. Most worth having on optical or archival media.";
    pub const ECC_OVERHEAD: &str = "Overhead";
    pub const ECC_ALGORITHM: &str = "Reed-Solomon with CRC32";

    pub const PASS_TITLE: &str = "Repository encryption key";
    pub const PASS_GENERATED: &str = "Generate one for me";
    pub const PASS_GENERATED_BODY: &str = "superbackup generates 256 random bits and keeps them in your vault. You are shown the passphrase once and asked to save it.";
    pub const PASS_SUPPLIED: &str = "I will choose it";
    pub const PASS_SUPPLIED_BODY: &str =
        "Use this if you also open this repository with the kopia command line.";
    pub const PASS_DERIVED: &str = "Work it out from my master passphrase";
    pub const PASS_DERIVED_BODY: &str =
        "Nothing extra to store. If you lose your master passphrase, this repository is lost with it.";

    pub const CREATE: &str = "Create repository";
    pub const STEP_CHECK: &str = "Checking the location";
    pub const STEP_CREATE: &str = "Creating the repository";
    pub const STEP_STORE: &str = "Storing the passphrase in your vault";
    pub const STEP_POLICY: &str = "Applying the retention policy";
    pub const STEP_MANIFEST: &str = "Writing the machine record";
    pub const MANIFEST_BODY: &str = "A small folder called _superbackup is written alongside the data, so anyone browsing this drive later can tell which computer each backup belongs to.";
    pub const CREATE_FAILED: &str = "The repository was not created.";
    pub const CREATE_CHANGE: &str = "Change settings";
}

pub fn enc_create_partial(path: &str) -> String {
    format!("Some files were written at {path} before this failed. Check the folder before trying again.")
}

pub mod writedown {
    pub const TITLE: &str = "Write this down now";
    pub const GROUPING: &str = "The passphrase is shown in groups only to make it easier to copy. The spaces are not part of it.";
    pub const COPY: &str = "Copy";
    pub const COPIED: &str = "Copied. The clipboard will be cleared in 60 seconds.";
    pub const SAVE: &str = "Save to a file…";
    pub const SAVE_NOTE: &str =
        "The file is plain text. Treat it the way you would treat the passphrase.";
    pub const PRINT: &str = "Print…";
    pub const ACK: &str = "I have saved this passphrase somewhere safe.";
    pub const ESCAPE: &str = "If you skip this, the passphrase can still be exported later from Settings › Security, using your master passphrase.";
    pub const CANNOT_SHOW: &str = "It cannot be shown again.";
    pub const PASS_STORED: &str = "Generated, stored in your vault";
    pub const PASS_DERIVED: &str = "Worked out from your master passphrase";
    pub const PASS_SUPPLIED: &str = "Chosen by you, stored in your vault";
}

pub fn writedown_body(location: &str) -> String {
    format!("This passphrase opens the repository at {location}. It is stored in your vault, so you will not normally be asked for it.\n\nYou will need it if you ever restore on a different computer, or if your vault is lost.")
}

// ---------------------------------------------------------------------------
// 8. Storage providers
// ---------------------------------------------------------------------------

pub mod prov {
    pub const FOR_DESTINATION: &str =
        "Creating a storage provider for the destination you were adding";
    pub const FOR_DESTINATION_BODY: &str =
        "Save it and you will be taken back, with this provider already selected. The provider is kept separately, so other destinations can use it too.";
    pub const FOR_DESTINATION_BACK: &str = "Back to the destination";
    pub const TITLE: &str = "Storage providers";
    pub const NEW: &str = "Add a storage provider";
    pub const SEARCH: &str = "Search providers";
    pub const USED_BY_NONE: &str = "Not used yet";
    pub const NO_TLS: &str = "This provider is set to plain HTTP.";

    pub const NAME: &str = "Name";
    pub const NAME_PLACEHOLDER: &str = "StorJ eu-1 (personal)";
    pub const NOTES: &str = "Notes";
    pub const NOTES_BODY: &str = "What this account is for, so it still makes sense in a year.";
    pub const TYPE: &str = "Provider type";
    pub const ENDPOINT: &str = "Endpoint";
    pub const REGION: &str = "Region";
    pub const REGION_REQUIRED: &str = "Required for this provider.";
    pub const REGION_OPTIONAL: &str = "Optional for this provider.";
    pub const TLS: &str = "Use TLS";
    pub const TLS_OFF_WARNING: &str = "Without TLS, your keys and your data travel unencrypted. Reasonable only for a server on this machine or your own network.";
    pub const PATH_STYLE: &str = "Path-style addressing";
    pub const PATH_STYLE_BODY: &str =
        "Required by MinIO and some gateways. StorJ and Amazon S3 accept the default.";

    pub const ADMIN_URL: &str = "Administration panel";
    pub const ADMIN_URL_BODY: &str = "Optional. Where you log in to manage this account and rotate its keys — a note to yourself, nothing connects to it.";
    pub const ADMIN_URL_PLACEHOLDER: &str = "https://storj.io/login";
    pub const ADMIN_URL_OPEN: &str = "Open";
    pub const ADMIN_URL_INVALID: &str = "Only http:// and https:// addresses are allowed here.";

    pub const CREDS_TITLE: &str = "Credentials";
    pub const ACCESS_KEY: &str = "Access key ID";
    pub const SECRET_KEY: &str = "Secret access key";
    pub const SESSION_TOKEN: &str = "Session token";
    pub const USE_SESSION_TOKEN: &str = "Use a session token";
    pub const SESSION_BODY: &str = "For temporary credentials issued by an identity service.";
    pub const CREDS_STORED: &str = "Stored in your vault. Leave blank to keep it.";
    pub const CREDS_REPLACE: &str = "Replace…";
    pub const CREDS_FOOTNOTE: &str = "Stored in your encrypted vault and handed to kopia through the environment, never on a command line.";

    pub const SAVE: &str = "Save provider";
    pub const SAVE_UNTESTED_TITLE: &str = "Save without testing?";
    pub const SAVE_UNTESTED_BODY: &str =
        "Testing takes a few seconds and catches a wrong key before a backup does, at two in the morning.";
    pub const SAVE_UNTESTED_TEST: &str = "Test first";
    pub const SAVE_UNTESTED_SAVE: &str = "Save anyway";

    pub const TEST_RESOLVING: &str = "Resolving the endpoint";
    pub const TEST_TLS: &str = "Negotiating TLS";
    pub const TEST_SIGNING: &str = "Signing a request";
    pub const TEST_LISTING: &str = "Listing buckets";
    pub const TEST_OK_NONE: &str = "Connected. This account has no buckets yet.";
    pub const TEST_SHOW_BUCKETS: &str = "Show buckets";
    pub const TEST_HIDE_BUCKETS: &str = "Hide buckets";
    pub const TEST_RUNNING: &str = "Checking the credentials against the endpoint…";
    pub const TEST_DENIED_HINT: &str = "Type the bucket name when you add a destination.";

    pub const ERR_DNS: &str = "That endpoint could not be found. Check the address for a typo.";
    pub const ERR_TLS_ACTION: &str = "Turn off TLS";
    pub const ERR_AUTH: &str = "The endpoint answered, but rejected these credentials.";
    pub const ERR_AUTH_ACTION: &str = "Check the keys";
    pub const ERR_NO_LIST: &str = "These credentials work, but they are not allowed to list buckets. You can still use a bucket by typing its name.";
    pub const ERR_NO_LIST_ACTION: &str = "Continue anyway";
    pub const ERR_TIMEOUT: &str = "The endpoint did not answer within 15 seconds.";
    pub const ERR_ADDRESSING: &str = "The endpoint answered but did not recognise the bucket path. Some gateways need path-style addressing.";
    pub const ERR_ADDRESSING_ACTION: &str = "Turn on path-style addressing";
    pub const ERR_COPY_DIAG: &str = "Copy diagnostic details";
    pub const ERR_DIAG_NOTE: &str = "Your keys are removed from the copied text.";

    pub const IMPACT_SHOW: &str = "Show them";
    pub const IMPACT_UNAFFECTED: &str = "Not affected — these use their own key pair";

    pub const ROTATE_LEAD: &str = "superbackup cannot create keys for you. Create a new key pair in your provider's console, enter it here, and it will be checked against every destination before anything is replaced.";
    pub const ROTATE_OLD_VALID: &str = "Your old key keeps working until you revoke it yourself.";
    pub const ROTATE_NEW_CREDS: &str = "New credentials";
    pub const ROTATE_VERIFY: &str = "Verify against all destinations";
    pub const ROTATE_PASS: &str = "Reachable with the new key";
    pub const ROTATE_BLOCKED: &str = "Fix the failures above, or continue and accept that these destinations will fail on their next run.";
    pub const ROTATE_CONTINUE_ANYWAY: &str = "Continue anyway";
    pub const ROTATE_DONE_TITLE: &str = "Keys replaced";
    pub const ROTATE_DONE_BODY: &str = "Jobs will use the new key from their next run.";
    pub const ROTATE_ATOMIC_FAIL: &str =
        "The vault could not be updated, so nothing was changed. Your old keys are still in place.";

    pub const DELETE_BODY: &str =
        "The stored keys are removed from your vault. Nothing at the provider is changed.";
    pub const DELETE_GOTO: &str = "Go to destinations";
}

pub fn prov_used_by(count: usize) -> String {
    format::plural(count, "destination", "destinations")
}
pub fn prov_type_filled(flavour: &str) -> String {
    format!("Endpoint and region filled in for {flavour}. Change them if your account differs.")
}
pub fn prov_endpoint_parsed(scheme: &str, host: &str, tls_state: &str, port: u16) -> String {
    format!("{scheme}://{host} — TLS {tls_state}, port {port}")
}
pub fn prov_path_style_from_flavour(flavour: &str) -> String {
    format!("Set automatically for {flavour}.")
}
pub fn prov_test_ok(count: usize) -> String {
    match count {
        0 => prov::TEST_OK_NONE.to_string(),
        1 => "Connected. Found 1 bucket.".to_string(),
        n => format!("Connected. Found {n} buckets."),
    }
}
pub fn prov_test_more_buckets(count: usize) -> String {
    format!("and {count} more")
}
pub fn prov_err_tls(reason: &str) -> String {
    format!("The secure connection could not be established. {reason}")
}
pub fn prov_err_clock(skew: &str) -> String {
    format!("The endpoint rejected the request because this computer's clock is {skew} out. Signatures depend on the time being right.")
}
pub fn prov_impact(destinations: usize, jobs: usize) -> String {
    format!("Used by {destinations} destinations across {jobs} jobs.")
}
pub fn prov_rotate_title(name: &str) -> String {
    format!("Rotate the keys on {name}?")
}
pub fn prov_rotate_verifying(name: &str) -> String {
    format!("Checking {name}…")
}
pub fn prov_rotate_fail(reason: &str) -> String {
    format!("Not reachable with the new key: {reason}")
}
pub fn prov_rotate_done_revoke(key_id: &str) -> String {
    format!("Revoke this key in your provider's console when you are ready: {key_id}")
}
pub fn prov_delete_title(name: &str) -> String {
    format!("Delete {name}?")
}
pub fn prov_delete_in_use(count: usize) -> String {
    format!("{count} destinations use this provider. Remove or move them first.")
}

// ---------------------------------------------------------------------------
// 9. Activity
// ---------------------------------------------------------------------------

pub mod activity {
    pub const TITLE: &str = "Activity";
    pub const TAB_RUNS: &str = "Runs";
    pub const TAB_EVENTS: &str = "Events";
    pub const SEARCH: &str = "Search activity";
    pub const RANGE_24H: &str = "Last 24 hours";
    pub const RANGE_7D: &str = "Last 7 days";
    pub const RANGE_30D: &str = "Last 30 days";
    pub const RANGE_ALL: &str = "All (200 runs)";
    pub const HISTORY_NOTE: &str =
        "superbackup keeps the last 200 runs. Older activity is in the event log.";
    pub const EXPORT: &str = "Export…";
    pub const EXPORT_RUNS: &str = "Runs as CSV";
    pub const EXPORT_EVENTS: &str = "Events as NDJSON";
    pub const EXPORT_BUNDLE: &str = "Diagnostic bundle…";
    pub const EXPORT_NOTE: &str =
        "Anything that looks like a credential is removed before the file is written.";
    pub const ONLY_THIS_JOB: &str = "Show only this job";
    pub const SEVERITY: &str = "Severity";
    pub const SEVERITY_ALL: &str = "All";
    pub const SEVERITY_INFO: &str = "Info and above";
    pub const SEVERITY_WARN: &str = "Warnings and errors";
    pub const SEVERITY_ERROR: &str = "Errors only";
    pub const DEBUG_NOTE: &str =
        "Debug events are only recorded while the log level is Debug or Trace.";
}

pub fn activity_filter_job(name: &str) -> String {
    format!("Job: {name}")
}
pub fn activity_filter_status(status: &str) -> String {
    format!("Status: {status}")
}
pub fn activity_filter_destination(name: &str) -> String {
    format!("Destination: {name}")
}
pub fn activity_dest_summary(succeeded: usize, total: usize) -> String {
    format!("{succeeded} of {total} succeeded")
}

pub mod run {
    pub const DETAIL_STATUS: &str = "Status";
    pub const DETAIL_PARTIAL: &str = "Some destinations did not complete. See below.";
    pub const DETAIL_TRIGGER: &str = "Started by";
    pub const DETAIL_DURATION: &str = "Duration";
    pub const DETAIL_DESTINATIONS: &str = "Destinations";
    pub const DETAIL_STARTED: &str = "Started";
    pub const DETAIL_FINISHED: &str = "Finished";
    pub const DETAIL_RUN_ID: &str = "Run id";
    pub const DETAIL_JOB_ID: &str = "Job id";
    pub const DETAIL_SNAPSHOT: &str = "Snapshot";
    pub const DETAIL_NO_SNAPSHOT: &str = "No snapshot was created";
    pub const DETAIL_BROWSE: &str = "Browse this snapshot";
    pub const DETAIL_REDACTED: &str =
        "Anything that looked like a credential has been removed from this output.";
    pub const DETAIL_RETRY: &str = "Retry this job";
    pub const DETAIL_COPY_SUMMARY: &str = "Copy run summary";
    pub const STOP_BODY: &str = "The partial snapshot is discarded, and the next run starts from where the last successful one left off.";
    pub const STOP_BUTTON: &str = "Stop backup";
    pub const STOP_ALL_BUTTON: &str = "Stop all backups";
    pub const DETAIL_FILES_LABEL: &str = "Files";
    pub const DETAIL_DATA_LABEL: &str = "Data";
    pub const DETAIL_THROUGHPUT_LABEL: &str = "Throughput";
}

pub fn run_detail_title(job: &str, started: &str) -> String {
    format!("{job} — {started}")
}
pub fn run_detail_files(processed: u64, cached: u64, skipped: u64) -> String {
    format!(
        "{} processed · {} unchanged · {} skipped",
        format::count(processed),
        format::count(cached),
        format::count(skipped)
    )
}
pub fn run_detail_data(read: u64, uploaded: u64) -> String {
    format!("{} read · {} uploaded", format::bytes(read), format::bytes(uploaded))
}
pub fn run_detail_throughput(rate: f64) -> String {
    format!("{} average", format::rate(rate))
}
pub fn run_detail_warnings(count: usize) -> String {
    format!("{count} warnings")
}
pub fn run_detail_error_code(code: &str, time: &str) -> String {
    format!("Error code: {code} · {time}")
}
pub fn run_stop_title(job: &str) -> String {
    format!("Stop {job}?")
}
pub fn run_stop_all_title(count: usize) -> String {
    format!("Stop {count} running backups?")
}
pub fn run_stop_all_body(names: &str) -> String {
    format!("These will be stopped: {names}. Partial snapshots are discarded.")
}
pub fn run_stopped_toast(job: &str) -> String {
    format!("{job} stopped. The partial snapshot was discarded.")
}

// ---------------------------------------------------------------------------
// 10. Restore
// ---------------------------------------------------------------------------

pub mod restore {
    pub const TITLE: &str = "Restore";
    pub const SOURCES: &str = "Restore from";
    pub const MIRRORS_GROUP: &str = "Folder mirrors";
    pub const MIRRORS_NOTE: &str =
        "Open these in your file manager — there is nothing to restore from.";
    pub const COMPARE: &str = "Compare with previous";

    pub const BROWSE_FILTER: &str = "Filter files";
    pub const BROWSE_HIDDEN: &str = "Show hidden files";
    pub const BROWSE_SHOW_SELECTION: &str = "Show selection";
    pub const BROWSE_CLEAR: &str = "Clear";
    pub const BROWSE_READING: &str = "Reading directory…";
    pub const BROWSE_SNAPSHOT: &str = "Snapshot";
    pub const BROWSE_RESTORE_ONE: &str = "Restore 1 item";
    pub const BROWSE_RESTORE_THIS: &str = "Restore this…";
    pub const BROWSE_RESTORE_TO: &str = "Restore this to…";
    pub const BROWSE_COPY_PATH: &str = "Copy path";
    pub const BROWSE_PREVIOUS: &str = "Show in previous snapshot";

    pub const OPTIONS_WHERE: &str = "Where should these go?";
    pub const OPTIONS_ORIGINAL: &str = "Back to the original location";
    pub const OPTIONS_ELSEWHERE: &str = "To another folder";
    pub const OPTIONS_STRUCTURE: &str = "Recreate the full folder structure";
    pub const OPTIONS_FLAT_WARN: &str = "Without the folder structure, files with the same name from different folders will overwrite each other.";
    pub const OPTIONS_CONFLICT: &str = "If a file already exists there";
    pub const OPTIONS_SKIP: &str = "Skip it";
    pub const OPTIONS_SKIP_BODY: &str = "Leaves what is on disk untouched.";
    pub const OPTIONS_OVERWRITE: &str = "Overwrite it";
    pub const OPTIONS_OVERWRITE_BODY: &str = "Replaces the file on disk. This cannot be undone.";
    pub const OPTIONS_KEEP_BOTH: &str = "Keep both";
    pub const OPTIONS_KEEP_BOTH_BODY: &str = "Restores as “name (restored 12 Mar 14:02).ext”.";
    pub const OPTIONS_ALSO: &str = "Also restore";
    pub const OPTIONS_TIMESTAMPS: &str = "File timestamps";
    pub const OPTIONS_PERMISSIONS: &str = "Permissions and ownership";
    pub const OPTIONS_PERMS_WINDOWS: &str =
        "Not restored on Windows, where these do not carry across usefully.";
    pub const OPTIONS_TYPE_CONFIRM: &str = "Type overwrite to confirm";
    pub const OPTIONS_BUTTON: &str = "Restore";
    pub const OPTIONS_BUTTON_DANGER: &str = "Overwrite and restore";

    pub const PROGRESS_CANCEL: &str = "Cancel restore";
    pub const CANCEL_TITLE: &str = "Cancel this restore?";
    pub const CANCEL_BODY: &str = "Files already written stay where they are. Nothing is put back.";
    pub const CANCEL_BUTTON: &str = "Cancel restore";

    pub const DONE_TITLE: &str = "Restore finished";
    pub const PARTIAL_TITLE: &str = "Restore finished with problems";
    pub const PARTIAL_RETRY: &str = "Retry failed items";
    pub const PARTIAL_COPY: &str = "Copy the list";
    pub const FAILED_TITLE: &str = "Restore failed";
}

pub fn restore_snapshot_count(count: usize) -> String {
    format!("{count} snapshots")
}
pub fn restore_newest(relative: &str) -> String {
    format!("newest {relative}")
}
pub fn restore_retention_note(
    latest: u32,
    hourly: u32,
    daily: u32,
    weekly: u32,
    monthly: u32,
    annual: u32,
) -> String {
    format!("Retention keeps {latest} latest, {hourly} hourly, {daily} daily, {weekly} weekly, {monthly} monthly and {annual} annual snapshots.")
}
pub fn restore_compare_result(added: u64, changed: u64, removed: u64) -> String {
    format!("{added} added · {changed} changed · {removed} removed")
}
pub fn restore_browse_selected(count: usize, size: u64) -> String {
    // A folder's size is not known until it has been walked, and claiming
    // "about 0 B" for a directory tree would be a lie.
    if size == 0 {
        format!("{count} items selected")
    } else {
        format!("{count} items selected · about {}", format::bytes(size))
    }
}
pub fn restore_browse_moved_up(path: &str) -> String {
    format!("That folder does not exist in this snapshot. Showing {path} instead.")
}
pub fn restore_browse_restore_n(count: usize) -> String {
    if count == 1 {
        restore::BROWSE_RESTORE_ONE.to_string()
    } else {
        format!("Restore {count} items")
    }
}
pub fn restore_options_title(count: usize) -> String {
    format!("Restore {count} items")
}
pub fn restore_options_what(count: usize, size: u64, snapshot: &str) -> String {
    format!("{count} items · about {} · from {snapshot}", format::bytes(size))
}
pub fn restore_options_free_space(free: u64) -> String {
    format!("{} free at the destination.", format::bytes(free))
}
pub fn restore_options_not_enough(needed: u64, free: u64) -> String {
    format!(
        "There is not enough room: {} needed, {} free.",
        format::bytes(needed),
        format::bytes(free)
    )
}
pub fn restore_progress_current(path: &str) -> String {
    format!("Restoring {path}")
}
pub fn restore_done_body(count: usize, size: u64, path: &str) -> String {
    format!("Restored {count} items ({}) to {path}", format::bytes(size))
}
pub fn restore_partial_body(done: usize, total: usize) -> String {
    format!("Restored {done} of {total} items. The rest are listed below with the reason.")
}

// ---------------------------------------------------------------------------
// 11. Settings
// ---------------------------------------------------------------------------

pub mod settings {
    pub const TITLE: &str = "Settings";
    pub const SAVED: &str = "Saved";

    pub const SECTION_GENERAL: &str = "General";
    pub const SECTION_SCHEDULING: &str = "Scheduling";
    pub const SECTION_BANDWIDTH: &str = "Bandwidth";
    pub const SECTION_NOTIFICATIONS: &str = "Notifications";
    pub const SECTION_SECURITY: &str = "Security";
    pub const SECTION_KOPIA: &str = "Kopia binary";
    pub const SECTION_REMOTE: &str = "Remote configuration";
    pub const SECTION_ADVANCED: &str = "Advanced";
    pub const SECTION_RESET: &str = "Reset";
}

pub mod set {
    pub const MACHINE_LABEL: &str = "Machine label";
    pub const MACHINE_LABEL_EMPTY: &str = "A machine needs a name.";
    pub const MACHINE_SLUG_NOTE: &str =
        "The folder name is fixed for this install and does not change when you rename the machine.";
    pub const MACHINE_ID: &str = "Machine id";
    pub const HOSTNAME: &str = "Host name";
    pub const OS: &str = "Operating system";
    pub const ARCH: &str = "Architecture";
    pub const USER: &str = "User";
    pub const FIRST_SETUP: &str = "First set up";
    pub const THEME: &str = "Theme";
    pub const THEME_SYSTEM: &str = "System";
    pub const THEME_LIGHT: &str = "Light";
    pub const THEME_DARK: &str = "Dark";
    pub const AUTOSTART: &str = "Start superbackup when I sign in";
    pub const START_MINIMISED: &str = "Start minimised to the tray";
    pub const SERVICE: &str = "Run backups as a background service";
    pub const SERVICE_INSTALLED_RUNNING: &str = "Service: installed and running";
    pub const SERVICE_INSTALLED_STOPPED: &str = "Service: installed, not running";
    pub const SERVICE_NOT_INSTALLED: &str = "Service: not installed";
    pub const SERVICE_INSTALL: &str = "Install";
    pub const SERVICE_START: &str = "Start";
    pub const SERVICE_UNINSTALL: &str = "Uninstall";
    pub const PARALLEL: &str = "Maximum jobs running at once";
    pub const PARALLEL_BODY: &str = "Kopia already uses many threads inside one backup. More than two at a time rarely helps and can make everything slower.";
    pub const QUIT: &str = "Quit superbackup";
    pub const QUIT_BODY: &str = "Scheduled backups stop until superbackup is started again.";

    pub const CATCHUP: &str = "Run schedules that were missed while the computer was off";
    pub const CATCHUP_BODY: &str =
        "Missed runs start shortly after superbackup does, and are recorded as catch-up runs.";
    pub const METERED: &str = "Skip scheduled runs on a metered connection";
    pub const METERED_BODY: &str =
        "Skipped runs are recorded as skipped, not failed. Individual jobs can override this.";
    pub const BATTERY: &str = "Skip scheduled runs on battery";

    pub const PAUSE_TITLE: &str = "Pause backups";
    pub const PAUSE_BODY: &str = "Pausing stops schedules. Backups you start yourself still run.";
    pub const PAUSE_1H: &str = "1 hour";
    pub const PAUSE_2H: &str = "2 hours";
    pub const PAUSE_4H: &str = "4 hours";
    pub const PAUSE_8H: &str = "8 hours";
    pub const PAUSE_FOREVER: &str = "Until I resume";
    pub const PAUSE_REASON: &str = "Reason";
    pub const PAUSE_REASON_PLACEHOLDER: &str = "On the road until Friday";
    pub const PAUSE_ACTIVE_FOREVER: &str = "Paused until you resume";
    pub const PAUSE_RESUME: &str = "Resume now";
    pub const PAUSE_EXTEND: &str = "Extend by 1 hour";

    pub const UPCOMING: &str = "Upcoming runs";
    pub const UPCOMING_BLOCKED_BY: &str = "Blocked by";
    pub const UPCOMING_BLOCKED_PAUSED: &str = "Paused";
    pub const UPCOMING_BLOCKED_LOCKED: &str = "Vault locked";
    pub const UPCOMING_BLOCKED_DISABLED: &str = "Job disabled";
    pub const UPCOMING_NONE: &str = "Nothing is scheduled.";

    pub const BW_UPLOAD: &str = "Upload limit";
    pub const BW_DOWNLOAD: &str = "Download limit";
    pub const BW_UNIT: &str = "kB/s";
    pub const BW_UNLIMITED: &str = "No limit";
    pub const BW_DOWNLOAD_BODY: &str =
        "Downloads happen during restores and repository maintenance.";
    pub const BW_WINDOW: &str = "Use a different limit during part of the day";
    pub const BW_FROM: &str = "From";
    pub const BW_TO: &str = "To";
    pub const BW_DAYS: &str = "Days";
    pub const BW_DAYS_NONE: &str = "No days chosen, so the window applies every day.";
    pub const BW_WRAPS: &str = "This window runs past midnight into the next day.";
    pub const BW_PER_DESTINATION: &str = "Limits are applied per destination, so two destinations running at once can each use the full limit.";

    pub const NOTIF_ENABLED: &str = "Show desktop notifications";
    pub const NOTIF_ON_FAILURE: &str = "When a backup fails";
    pub const NOTIF_ON_SUCCESS: &str = "When a backup succeeds";
    pub const NOTIF_ON_SUCCESS_BODY: &str = "Most people prefer silence when everything works.";
    pub const NOTIF_STALE: &str = "When a job has not succeeded for";
    pub const NOTIF_STALE_UNIT: &str = "days";
    pub const NOTIF_STALE_BODY: &str = "Set to 0 to turn this off, here and on the dashboard.";
    pub const NOTIF_SERVICE: &str = "When the background service has a problem";
    pub const NOTIF_DEDUPE: &str = "Do not repeat the same problem within";
    pub const NOTIF_DEDUPE_UNIT: &str = "minutes";
    pub const NOTIF_TEST: &str = "Send a test notification";
    pub const NOTIF_TEST_BODY: &str = "Test notification sent. If nothing appeared on your desktop, notifications may be switched off for superbackup in your system settings.";
    /// The title of the notification the operating system actually shows.
    pub const NOTIF_TEST_TITLE: &str = "superbackup";
    pub const NOTIF_TEST_SENT: &str = "This is a test notification. Backups are not affected.";
    pub const NOTIF_TEST_UNAVAILABLE: &str =
        "This system has no notification service available, so nothing was shown.";
    pub const NOTIF_TEST_FAILED: &str = "The system refused the notification.";
    pub const NOTIF_TEST_SUPPRESSED: &str = "The notification was suppressed before it was sent.";
    pub const NOTIF_BLOCKED: &str = "Your system is not showing notifications from superbackup.";
    pub const NOTIF_BLOCKED_ACTION: &str = "Open system settings";

    pub const SEC_VAULT: &str = "Vault";
    pub const SEC_AUTOLOCK: &str = "Lock automatically after";
    pub const SEC_AUTOLOCK_UNIT: &str = "minutes";
    pub const SEC_AUTOLOCK_BODY: &str = "Set to 0 to lock as soon as the window is closed. Auto-lock never happens while a backup is running.";
    pub const SEC_AUTOLOCK_CONFLICT: &str = "With auto-lock set to 0 and the credential store switched off, no scheduled backup will ever run without you typing the passphrase first.";
    pub const SEC_KEYCHAIN_CONFIRM: &str = "Enter your master passphrase to confirm";
    pub const SEC_CHANGE: &str = "Change master passphrase…";
    pub const SEC_CHANGE_CURRENT: &str = "Current passphrase";
    pub const SEC_CHANGE_NEW: &str = "New passphrase";
    pub const SEC_CHANGE_CONFIRM: &str = "Confirm new passphrase";
    pub const SEC_CHANGE_DONE_TITLE: &str = "Master passphrase changed";
    pub const SEC_CHANGE_DONE_BODY: &str = "The vault has been re-encrypted and a backup of the old one saved. Repository encryption keys worked out from the master passphrase have been recalculated, so no repository needs creating again.";
    pub const SEC_EXPORT: &str = "Export repository encryption keys…";
    pub const SEC_EXPORT_TITLE: &str = "Export repository encryption keys";
    pub const SEC_EXPORT_BODY: &str = "This writes every repository encryption key to a plain text file you choose. The file is not encrypted. Treat it exactly as you would treat the passphrases.";
    pub const SEC_EXPORT_CONFIRM: &str = "Enter your master passphrase to continue";
    pub const SEC_EXPORT_BUTTON: &str = "Choose a file and export";
    pub const SEC_BACKUPS: &str = "Vault backups";
    pub const SEC_BACKUPS_BODY: &str = "A copy is written here before every change to the vault.";
    pub const SEC_BACKUPS_RESTORE: &str = "Restore a backup…";
    pub const SEC_RESET: &str = "Reset the vault and start over";
    pub const SEC_RESET_TITLE: &str = "Reset the vault?";
    pub const SEC_RESET_BODY: &str = "Every stored secret is destroyed: repository encryption keys, storage keys and tokens. Repositories whose passphrase was generated or worked out from your master passphrase cannot be opened again unless you have exported the passphrase.";
    pub const SEC_RESET_CONFIRM: &str = "Type superbackup to confirm";
    pub const SEC_RESET_BUTTON: &str = "Reset the vault";

    pub const KOPIA_AUTO: &str = "Find it automatically";
    pub const KOPIA_SPECIFIC: &str = "Use a specific file";
    pub const KOPIA_CHECK: &str = "Check again";
    pub const KOPIA_DOWNLOAD: &str = "Download a tested build";
    pub const KOPIA_FOLDERS: &str = "superbackup keeps its own kopia configuration and cache, separate from any kopia you run yourself, so the two never fight over repository.config.";

    pub const REMOTE_LEAD: &str = "Several machines can share one configuration through a Git repository. The file kept in the repository is the sealed vault; the plain config.json is never pushed, and the vault is only opened in memory after you supply your master passphrase.";
    pub const REMOTE_ENABLED: &str = "Sync configuration from a Git repository";
    pub const REMOTE_URL: &str = "Repository URL";
    pub const REMOTE_BRANCH: &str = "Branch";
    pub const REMOTE_PATH: &str = "File path in the repository";
    pub const REMOTE_AUTH: &str = "Authentication";
    pub const REMOTE_AUTH_NONE: &str =
        "None — public repository, or credentials your system git already has";
    pub const REMOTE_AUTH_TOKEN: &str = "Personal access token";
    pub const REMOTE_AUTH_TOKEN_SCOPE: &str =
        "Read access to the repository is enough, unless you also publish from this machine.";
    pub const REMOTE_AUTH_SSH: &str = "SSH key";
    pub const REMOTE_AUTH_SSH_BODY: &str =
        "The key is read from where it is. It is not copied into the vault.";
    pub const REMOTE_AUTO_PULL: &str = "Check for changes automatically";
    pub const REMOTE_INTERVAL: &str = "every";
    pub const REMOTE_INTERVAL_UNIT: &str = "minutes";
    pub const REMOTE_ALLOW_PUSH: &str = "Allow publishing from this machine";
    pub const REMOTE_ALLOW_PUSH_BODY: &str =
        "Nothing is ever pushed automatically. Publishing is always something you do on purpose.";
    pub const REMOTE_SIGNERS: &str = "Trusted signers";
    pub const REMOTE_SIGNERS_ADD: &str = "Add fingerprint…";
    pub const REMOTE_SIGNERS_BODY: &str =
        "When this list is not empty, a pulled vault whose signature does not match one of these is rejected.";
    pub const REMOTE_SIGNERS_EMPTY: &str =
        "With no fingerprints listed, any vault found at that address will be accepted.";
    pub const REMOTE_UP_TO_DATE: &str = "Up to date";
    pub const REMOTE_NEVER: &str = "Never pulled";
    pub const REMOTE_PULL: &str = "Pull now";
    pub const REMOTE_PUBLISH: &str = "Publish…";
    pub const REMOTE_OPEN: &str = "Open the repository";
    pub const REMOTE_PULL_KEEPS_LOCAL: &str =
        "Your run history and job state stay on this machine. Only configuration is replaced.";

    pub const ADV_LOG_LEVEL: &str = "Log level";
    pub const ADV_LOG_LEVEL_BODY: &str =
        "Debug and Trace write a lot, and can include the paths of files being backed up.";
    pub const ADV_LOG_DAYS: &str = "Keep logs for";
    pub const ADV_LOG_DAYS_UNIT: &str = "days";
    pub const ADV_LOCATIONS: &str = "File locations";
    pub const ADV_CLEAR_CACHE: &str = "Clear the kopia cache";
    pub const ADV_CACHE_BODY: &str = "The next run will be slower while the cache is rebuilt.";
    pub const ADV_BUNDLE: &str = "Export a diagnostic bundle…";
    pub const ADV_BUNDLE_INCLUDES: &str = "Included: your configuration with every secret removed, the last 200 runs, the tail of the event log, the kopia version, your operating system details, and the last 2,000 log lines.";
    pub const ADV_BUNDLE_EXCLUDES: &str = "Not included: any passphrase, key or token, the contents of any file you back up, and the names of files inside your folders.";
    pub const ADV_BUNDLE_PREVIEW: &str = "Preview the bundle";
    pub const ADV_BUNDLE_WRITE: &str = "Save the bundle…";
    pub const ADV_DOCTOR: &str = "Run diagnostics";

    pub const RESET_SETTINGS: &str = "Reset all settings to their defaults";
    pub const RESET_SETTINGS_BODY: &str =
        "Jobs, destinations, providers and the vault are left alone.";
    pub const RESET_ALL: &str = "Remove all configuration and start over";
    pub const RESET_ALL_BODY: &str = "Deletes every job, destination, provider and stored secret on this machine. Data already written to your destinations is not touched, and can be reached again by connecting to the repositories with their passphrases.";
    pub const RESET_ALL_CONFIRM: &str = "Type superbackup to confirm";
}

pub fn set_machine_slug(slug: &str) -> String {
    format!("Folder name: {slug}")
}
pub fn set_pause_active(time: &str) -> String {
    format!("Paused until {time}")
}
pub fn set_bw_summary(
    start: &str,
    end: &str,
    days: &str,
    window_up: &str,
    base_up: &str,
) -> String {
    format!("Between {start} and {end} on {days}, uploads are limited to {window_up}. Outside that window, uploads are limited to {base_up}.")
}
pub fn set_sec_keychain(keychain_name: &str) -> String {
    format!("Store the vault key in {keychain_name}")
}
pub fn set_sec_keychain_on_title(keychain_name: &str) -> String {
    format!("Store the vault key in {keychain_name}?")
}
pub const SEC_KEYCHAIN_ON_BODY: &str = "Unattended backups stop needing a person to type the passphrase. In exchange, anything that can run programs as you can ask the credential store for the key.";
pub fn set_sec_keychain_off(keychain_name: &str) -> String {
    format!("The stored key has been removed from {keychain_name}.")
}
pub fn set_sec_backups_restore_title(file: &str) -> String {
    format!("Restore {file}?")
}
pub fn set_sec_backups_restore_body(date: &str) -> String {
    format!("The current vault will be replaced by this backup. Any credential added since {date} will be gone.")
}
pub fn set_sec_reset_affected(names: &str) -> String {
    format!("These repositories would become unreadable: {names}")
}
pub fn set_kopia_found(version: &str) -> String {
    format!("Kopia {version}")
}
pub fn set_kopia_untested(found: &str, tested: &str) -> String {
    format!("This is Kopia {found}. superbackup is tested with {tested}. It will still try.")
}
pub fn set_remote_last_pull(relative: &str) -> String {
    format!("Last pulled {relative}")
}
pub fn set_remote_commit(short: &str) -> String {
    format!("Commit {short}")
}
pub fn set_remote_changes(count: usize) -> String {
    format!("{count} changes available")
}
pub fn set_adv_cache_size(size: u64) -> String {
    format!("Currently {}.", format::bytes(size))
}

// ---------------------------------------------------------------------------
// 11b. Kopia binary — "prove it works"
// ---------------------------------------------------------------------------

/// The Kopia settings page. Every string here exists so a user can check the
/// application's claims rather than take them on trust, which is why the raw
/// command line and the raw output are shown and labelled as such.
pub mod kopia {
    pub const WHERE_TITLE: &str = "Which kopia is being used";
    pub const WHERE_LEAD: &str =
        "superbackup does not do the backing up itself. Kopia does, and this is the exact copy of \
         it being run.";
    pub const PATH: &str = "Full path";
    pub const VERSION: &str = "Version";
    pub const PROVENANCE: &str = "Found by";
    pub const BANNER: &str = "Reports itself as";
    pub const MINIMUM: &str = "Minimum accepted";
    pub const REVEAL: &str = "Show in folder";
    pub const REVEAL_FAILED: &str = "This system could not open a file browser at that folder.";
    pub const NOT_FOUND: &str = "No usable kopia was found";
    pub const NOT_FOUND_BODY: &str =
        "Until one is available, repository destinations cannot be created, connected to, or \
         backed up. Folder mirrors still work.";

    pub const ROUTES_TITLE: &str = "Where superbackup looked";
    pub const ROUTES_LEAD: &str =
        "In this order. The first one that produces a working kopia new enough to drive is the \
         one used.";
    pub const ROUTE_CHOSEN: &str = "In use";

    pub const VERIFY_TITLE: &str = "Check it for yourself";
    pub const VERIFY_LEAD: &str =
        "This runs kopia now and shows you everything it printed. Nothing is written and no \
         backup is started.";
    pub const VERIFY_BUTTON: &str = "Run the checks";
    pub const VERIFY_AGAINST: &str = "Check a repository as well";
    pub const VERIFY_AGAINST_NONE: &str = "Version only";
    pub const VERIFY_RUNNING: &str = "Running kopia…";
    pub const VERIFY_EMPTY: &str =
        "Nothing has been run yet. Press the button and the exact command, its exit code and its \
         output appear here.";
    pub const COMMAND_LINE: &str = "Command";
    pub const COMMAND_LINE_NOTE: &str =
        "Safe to show and safe to paste into a terminal: encryption keys and storage keys are \
         passed to kopia through environment variables, never on the command line.";
    pub const SECRET_ENV: &str = "Secrets passed in";
    pub const EXIT_CODE: &str = "Exit code";
    pub const STDOUT: &str = "Output";
    pub const STDERR: &str = "Errors";
    pub const NO_OUTPUT: &str = "(nothing printed)";
    pub const NOT_ATTEMPTED: &str = "Not run";

    pub const MANAGED_TITLE: &str = "The build superbackup manages";
    pub const MANAGED_LEAD: &str =
        "When no kopia is installed, superbackup downloads one from Kopia's own releases and \
         checks it against the SHA-256 published with it.";
    pub const MANAGED_PATH: &str = "Kept at";
    pub const MANAGED_VERSION: &str = "Installed version";
    pub const MANAGED_NONE: &str = "Not installed";
    pub const UPDATE_POLICY: &str = "When a newer kopia is released";
    pub const UPDATE_OFF: &str = "Do nothing";
    pub const UPDATE_NOTIFY: &str = "Tell me, and let me decide";
    pub const UPDATE_AUTOMATIC: &str = "Install it";
    pub const UPDATE_CHECK: &str = "Check for an update now";
    pub const UPDATE_NONE: &str = "No update check has been made in this window yet.";
    pub const PREFER_SYSTEM: &str = "Prefer a kopia already installed on this computer";
}

pub fn kopia_update_available(version: &str) -> String {
    format!("kopia {version} is available.")
}
pub fn kopia_ran_in(ms: u64) -> String {
    format!("{ms} ms")
}

// ---------------------------------------------------------------------------
// 11c. Encryption keys — validate and back up
// ---------------------------------------------------------------------------

pub mod keys {
    pub const CHECK: &str = "Check this key";
    pub const CHECK_STORED: &str = "Check the stored key";
    pub const CHECKING: &str = "Opening the repository…";
    pub const CHECK_OK: &str = "The repository opened with this key.";
    pub const CHECK_BAD: &str = "The repository did not accept this key.";
    pub const CHECK_NONE: &str =
        "There is no repository here yet, so there is nothing to check the key against.";
    pub const CHECK_NOTE: &str =
        "This is not a format check. superbackup opens the repository with the key and tells you \
         what happened.";

    pub const EXPORT_TITLE: &str = "Back up your encryption keys";
    pub const EXPORT_LEAD: &str =
        "A repository encryption key that is lost cannot be recovered by anyone, including us. \
         Write these somewhere safe.";
    pub const EXPORT_WARN_TITLE: &str = "This file is worth as much as the backups";
    pub const EXPORT_WARN_BODY: &str =
        "It is not encrypted. Anyone holding it can read your backups. A password manager, a \
         safe, or paper — not Downloads, and not email.";
    pub const EXPORT_NOT_SHOWN: &str =
        "The keys are not shown here on purpose. They go straight into the file you choose, so \
         they never appear on your screen.";
    pub const EXPORT_CONFIRM: &str = "Enter your master passphrase to continue";
    pub const EXPORT_SAVE: &str = "Choose a file and save";
    pub const EXPORT_COPY: &str = "Copy as text instead";
    pub const EXPORT_COPIED: &str = "Copied. Your clipboard now holds your encryption keys.";
    pub const EXPORT_CANCELLED: &str = "Nothing was saved.";
    pub const EXPORT_OMITTED: &str = "Not included";
    pub const EXPORT_EMPTY: &str =
        "There are no repository encryption keys to export yet. Create a repository first.";
    pub const EXPORT_PREVIEW: &str = "What the file will contain";
}

pub fn keys_export_saved(path: &str) -> String {
    format!("Encryption keys written to {path}. Move it somewhere safe now.")
}
pub fn keys_export_count(count: u32) -> String {
    match count {
        1 => "1 repository".to_string(),
        n => format!("{n} repositories"),
    }
}

// ---------------------------------------------------------------------------
// 11d. Machine manifest
// ---------------------------------------------------------------------------

pub mod machines {
    pub const SETTING: &str = "Leave a note next to the backups saying which computer wrote them";
    pub const SETTING_BODY: &str =
        "Writes a small `_superbackup` folder at each destination holding this computer's label, \
         host name, operating system and the date it last backed up, plus a plain-text README. It \
         is what makes a shared drive readable during a recovery. It contains no file names and \
         no keys.";
    pub const SETTING_S3: &str =
        "Object storage cannot hold it. A bucket has no folder superbackup can write to outside \
         the repository kopia manages, so S3 and StorJ destinations get no note.";
    pub const TITLE: &str = "Computers backing up here";
    pub const EMPTY: &str = "No computer has left a record at this destination yet.";
    pub const UNSUPPORTED: &str = "Not recorded for object storage.";
    pub const THIS_PC: &str = "This computer";
    pub const LAST_SEEN: &str = "Last backed up";
    pub const REFRESH: &str = "Look again";
    pub const UNREADABLE: &str = "That location could not be read from this window.";
}

pub fn machines_last_seen(when: &str) -> String {
    format!("Last backed up {when}")
}

// ---------------------------------------------------------------------------
// 11e. Dry run / preview
// ---------------------------------------------------------------------------

pub mod preview {
    pub const ACTION: &str = "Preview";
    pub const TITLE: &str = "Preview";
    pub const TOOLTIP: &str = "See what this job would copy, without copying anything";
    pub const NOTHING_WRITTEN: &str = "Nothing was written. This was a rehearsal.";
    pub const NOTHING_WRITTEN_BODY: &str =
        "No file was copied, no snapshot was created and nothing was deleted at any destination. \
         The figures below are what a real run would have produced.";
    pub const RUNNING: &str = "Working out what would be copied…";
    pub const EMPTY: &str = "Run a preview and the result appears here, one card per destination.";
    pub const PER_DESTINATION: &str =
        "One card per destination. superbackup never adds these together: a job that reached two \
         destinations out of three has not backed up.";
    pub const FILES: &str = "Files";
    pub const TOTAL_SIZE: &str = "Total size";
    pub const WOULD_COPY: &str = "Would be copied";
    pub const UNCHANGED: &str = "Already up to date";
    pub const UNKNOWN_SPLIT: &str =
        "kopia estimates the whole source and does not say how much of it is already stored, so \
         the new-versus-unchanged split is not available for a repository.";
    pub const NO_PATHS: &str = "Individual paths are not available";
    pub const NO_PATHS_REPO: &str =
        "`kopia snapshot estimate` reports totals only; it does not list the files it counted.";
    pub const NO_PATHS_MIRROR: &str =
        "The mirror rehearsal counts every file as it walks the tree but does not keep the list, \
         so there is nothing to show.";
    pub const NOT_REHEARSABLE: &str = "This destination could not be rehearsed";
    pub const RERUN: &str = "Preview again";
    pub const RUN_FOR_REAL: &str = "Back up now";
}

pub fn preview_title(job: &str) -> String {
    format!("Preview of {job}")
}
pub fn preview_and_more(count: usize) -> String {
    format!("and {count} more")
}
pub fn preview_started(job: &str) -> String {
    format!("Working out what \"{job}\" would copy. Nothing is being written.")
}

pub mod doctor {
    pub const KOPIA: &str = "Kopia is present and runs";
    pub const VAULT: &str = "The vault can be read";
    pub const SCHEMA: &str = "The configuration format is understood";
    pub const DESTINATIONS: &str = "Every destination is reachable";
    pub const PROVIDERS: &str = "Every provider has been verified";
    pub const SPACE: &str = "There is room at each local destination";
    pub const SERVICE: &str = "The background service is in the state you asked for";
    pub const IPC: &str = "The daemon can be reached";
    pub const CLOCK: &str = "This computer's clock matches the storage endpoint";
    pub const FIX: &str = "Fix";
    pub const PASS: &str = "Passed";
    pub const WARN: &str = "Worth a look";
    pub const FAIL: &str = "Needs fixing";
    pub const SKIPPED: &str = "Not checked";
}

// ---------------------------------------------------------------------------
// 12. About
// ---------------------------------------------------------------------------

pub mod about {
    pub const TAGLINE: &str = "Backups for machines full of code.";
    pub const KOPIA: &str = "Kopia";
    pub const KOPIA_MISSING: &str = "Not found";
    pub const MACHINE: &str = "Machine";
    pub const SCHEMA: &str = "Configuration format";
    pub const DATA_FOLDER: &str = "Data folder";

    pub const LICENCES: &str = "Licences";
    pub const LICENCE_SELF: &str = "superbackup is released under the MIT licence.";
    pub const LICENCE_SELF_VIEW: &str = "View licence";
    pub const LICENCE_KOPIA: &str = "superbackup uses Kopia, which is released under the Apache Licence 2.0. Kopia is a separate program: superbackup runs it and does not modify it.";
    pub const LICENCE_KOPIA_VIEW: &str = "View the Apache 2.0 licence";
    pub const LICENCE_KOPIA_SITE: &str = "kopia.io";
    pub const LICENCE_THIRD_PARTY: &str = "Third-party licences";
    pub const LICENCE_FONTS: &str = "Inter and JetBrains Mono are used under the SIL Open Font Licence 1.1. Icons are from Lucide, under the ISC licence.";
    pub const LICENCE_COPY_ALL: &str = "Copy all licence text";

    pub const LINK_WEBSITE: &str = "Website";
    pub const LINK_DOCS: &str = "Documentation";
    pub const LINK_ISSUE: &str = "Report an issue";
    pub const LINK_KOPIA_DOCS: &str = "Kopia documentation";
    pub const LINK_RELEASES: &str = "Release notes";
    pub const COPYRIGHT: &str = "© 2026 Andreas Wiren";
}

pub fn about_version(version: &str) -> String {
    format!("superbackup {version}")
}
pub fn about_daemon_build(build: &str) -> String {
    format!("background service is running {build}")
}
pub fn about_build(os: &str, arch: &str) -> String {
    format!("{os}-{arch}")
}

// ---------------------------------------------------------------------------
// 13. Tray (rendered by the tray workstream; kept here so one deck holds them)
// ---------------------------------------------------------------------------

pub const TRAY_FIRST_HIDE: &str =
    "superbackup is still running in the tray. Use Quit in the tray menu to stop it completely.";

// ---------------------------------------------------------------------------
// 15. Errors
// ---------------------------------------------------------------------------

pub mod err {
    pub const LOCKED: &str = "The vault is locked.";
    pub const BAD_PASSPHRASE: &str =
        "That passphrase did not work. Passphrases are case sensitive.";
    pub const VAULT_CORRUPT: &str = "The vault file is damaged or has been altered. Restore config.sbvault from a backup rather than overwriting it.";
    pub const VAULT_VERSION_ACTION: &str = "Get the newer version";
    pub const KOPIA: &str = "Kopia stopped with an error.";
    pub const KOPIA_MISSING: &str =
        "Kopia was not found. superbackup needs it to read and write backups.";
    pub const KOPIA_MISSING_ACTION: &str = "Fix in Settings";
    pub const REPO_NOT_CONNECTED_ACTION: &str = "Connect to this repository…";
    pub const REPO_EXISTS_ACTION: &str = "Connect instead";
    pub const JOB_NOT_FOUND: &str = "That job no longer exists.";
    pub const JOB_RUNNING: &str = "That job is already running.";
    pub const JOB_RUNNING_ACTION: &str = "Show it";
    pub const JOB_CANCELLED: &str = "The run was cancelled.";
    pub const IPC: &str = "superbackup lost contact with its background process.";
    pub const DAEMON_UNREACHABLE: &str =
        "The superbackup background process is not running. Schedules will not fire until it is.";
    pub const DAEMON_UNREACHABLE_ACTION: &str = "Start the background service";
    pub const SERVICE_ACTION: &str = "Reinstall the service";
    pub const INTERNAL: &str = "Something went wrong inside superbackup.";
    pub const INTERNAL_ACTION: &str = "Export a diagnostic bundle";
    pub const PATH_MISSING_CREATE: &str = "Create the folder";
    pub const OPEN_CONFIG: &str = "Open config.json";
    pub const OPEN_VAULT_BACKUPS: &str = "Open vault backups";
}

pub fn err_config(detail: &str) -> String {
    format!("The configuration file could not be read. {detail}")
}
pub fn err_crypto(detail: &str) -> String {
    format!("A cryptographic operation failed. {detail}")
}
pub fn err_repo_not_connected(location: &str) -> String {
    format!("superbackup is not connected to the repository at {location} yet.")
}
pub fn err_repo_exists(location: &str) -> String {
    format!("There is already a repository at {location}.")
}
pub fn err_schedule(detail: &str) -> String {
    format!("That schedule could not be understood: {detail}")
}
pub fn err_service(detail: &str) -> String {
    format!("The background service could not be controlled. {detail}")
}
pub fn err_platform(detail: &str) -> String {
    format!("Something the operating system provides did not work: {detail}")
}
pub fn err_remote(detail: &str) -> String {
    format!("The shared configuration could not be reached. {detail}")
}
pub fn err_vault_version(found: &str, supported: &str) -> String {
    format!("This vault was written by a newer version of superbackup (format {found}; this build understands up to {supported}).")
}
pub fn err_path_missing(path: &str) -> String {
    format!("{path} could not be found.")
}

// ---------------------------------------------------------------------------
// 16. Validation
// ---------------------------------------------------------------------------

pub mod valid {
    pub const JOB_NAME_EMPTY: &str = "Give the job a name.";
    pub const JOB_NAME_LONG: &str = "Names can be up to 64 characters.";
    pub const SOURCE_NONE: &str = "Add at least one folder.";
    pub const SOURCE_RELATIVE: &str = "Use a full path, starting from the drive or the root.";
    pub const SOURCE_DUP: &str = "That folder is already in this job.";
    pub const SCHEDULE_INTERVAL: &str = "Choose between 1 minute and 7 days.";
    pub const SCHEDULE_TIMES: &str = "Add at least one time. Up to 24 are allowed.";
    pub const SCHEDULE_TIMES_DUP: &str = "That time is already in the list.";
    pub const SCHEDULE_WEEKDAYS: &str = "Choose at least one day.";
    pub const SCHEDULE_DEBOUNCE: &str = "Choose between 5 seconds and 1 hour.";
    pub const SCHEDULE_MIN_INTERVAL: &str = "Choose between 1 minute and 24 hours.";
    pub const TIMEOUT: &str = "Choose between 1 minute and 24 hours.";
    pub const MAX_FILE_SIZE: &str = "Choose between 1 MB and 1,048,576 MB.";
    pub const MAINTENANCE: &str =
        "Choose between 0 and 1,000. Zero means maintenance never runs on a schedule.";
    pub const DEST_NAME_EMPTY: &str = "Give the destination a name.";
    pub const DEST_PATH_RELATIVE: &str = "Use a full path.";

    // -- chained destinations -----------------------------------------------
    pub const REPLICA_SELF: &str = "A destination cannot be copied from itself.";
    pub const REPLICA_NOT_REPOSITORY: &str =
        "A folder mirror holds plain files rather than repository blocks, so it cannot be a copy of a repository. Choose a repository destination, or turn this back to reading the job's folders.";
    pub const REPLICA_TOO_DEEP: &str =
        "This chain is too long. Copy from a destination nearer the start of it.";
    pub const BUCKET: &str =
        "Bucket names are 3 to 63 characters, using lowercase letters, digits, dots and hyphens.";
    pub const BUCKET_IP: &str = "A bucket name cannot look like an IP address.";
    pub const PROVIDER_NAME_EMPTY: &str = "Give the provider a name.";
    pub const ENDPOINT_EMPTY: &str = "Enter the endpoint for this account.";
    pub const ENDPOINT_INVALID: &str = "That does not look like a host or a URL.";
    pub const ENDPOINT_INSECURE: &str =
        "This endpoint is not on this machine or a private network, and TLS is switched off.";
    pub const REGION: &str = "Amazon S3 needs a region, for example us-east-1.";
    pub const CREDENTIALS: &str = "Enter both the access key ID and the secret access key.";
    pub const MASTER_SHORT: &str = "Use at least 12 characters.";
    pub const MASTER_MISMATCH: &str = "The two passphrases are different.";
    pub const REPO_PASS_SHORT: &str = "Use at least 12 characters.";
    pub const REPO_PASS_MISMATCH: &str = "The two passphrases are different.";
    pub const BANDWIDTH: &str = "Choose between 1 and 10,000,000 kB/s.";
    pub const BW_WINDOW: &str = "The start and end times need to be different.";
    pub const REMOTE_URL: &str = "That does not look like a Git address.";
    pub const SIGNER: &str = "A fingerprint is 16 to 128 characters of hex or base64.";
    pub const AUTOLOCK: &str = "Choose between 0 and 1,440 minutes.";
    pub const STALE: &str = "Choose between 0 and 90 days.";
    pub const DEDUPE: &str = "Choose between 0 and 1,440 minutes.";
    pub const PARALLEL: &str = "Choose between 1 and 8.";
    pub const LOGDAYS: &str = "Choose between 1 and 365 days.";
    pub const FORM_PROBLEM: &str = "1 problem to fix";
}

pub fn valid_job_name_dup(name: &str) -> String {
    format!("There is already a job called {name}.")
}
pub fn valid_dest_name_dup(name: &str) -> String {
    format!("There is already a destination called {name}.")
}
pub fn valid_replica_source_mirror(name: &str) -> String {
    format!(
        "{name} is a folder mirror, which holds plain files rather than a repository. Only a repository destination can be copied from."
    )
}
pub fn valid_replica_cycle(name: &str) -> String {
    format!(
        "That would form a loop: {name} is already fed, directly or indirectly, by this destination. One of them has to go first."
    )
}
pub fn valid_replica_source_absent(replica: &str, source: &str) -> String {
    format!(
        "{replica} is copied from {source}, which this job does not back up. Add {source} to this job as well — otherwise the copy would be made from whatever this run did not update, and still report success."
    )
}
pub fn valid_provider_name_dup(name: &str) -> String {
    format!("There is already a provider called {name}.")
}
pub fn valid_source_nested(parent: &str) -> String {
    format!("That folder is already covered by {parent}.")
}
pub fn valid_source_in_destination(destination: &str) -> String {
    format!("That folder is inside {destination}, so the backup would contain itself.")
}
pub fn valid_dest_path_parent(parent: &str) -> String {
    format!("{parent} does not exist, so this folder cannot be created.")
}
pub fn valid_dest_path_inside_dest(other: &str) -> String {
    format!("That path is inside {other}. Two destinations cannot share a folder.")
}
pub fn valid_dest_path_inside_source(source: &str) -> String {
    format!(
        "That path is inside {source}, which this job backs up. The backup would contain itself."
    )
}
pub fn valid_pattern_empty(line: usize) -> String {
    format!("Line {line} is empty.")
}
pub fn valid_pattern_invalid(line: usize, reason: &str) -> String {
    format!("Line {line} could not be read as a pattern: {reason}")
}
pub fn valid_pattern_absolute(line: usize) -> String {
    format!("Line {line} looks like a full path. Patterns are matched relative to each folder you back up.")
}
pub fn valid_prefix(normalised: &str) -> String {
    format!("Saved as {normalised}.")
}
pub fn valid_form_problems(count: usize) -> String {
    if count == 1 {
        valid::FORM_PROBLEM.to_string()
    } else {
        format!("{count} problems to fix")
    }
}

pub mod warn {
    pub const MIRROR_ONLY: &str = "This job's only destination is a folder mirror, so there is no history to go back to and nothing is encrypted.";
    pub const RETENTION_NO_MAINTENANCE: &str =
        "Every retention value is zero, so nothing would be kept.";
    pub const AUTOLOCK_SERVICE: &str = "With auto-lock at 0 minutes and the credential store off, no scheduled backup will run without you first typing the passphrase.";
}

pub fn warn_same_drive(drive: &str) -> String {
    format!("Every destination for this job is on {drive}. If that drive fails, all the copies go with it.")
}
pub fn warn_onchange_large(files: u64) -> String {
    format!(
        "This job watches {} files. Watching a tree that size uses noticeable memory.",
        format::count(files)
    )
}
pub fn warn_recursive(source: &str, destination: &str) -> String {
    format!("{source} is inside {destination}, so each backup would include the previous one.")
}
pub fn warn_unverified_dest(name: &str) -> String {
    format!("{name} has never been verified, and a job is scheduled to write to it.")
}

// ---------------------------------------------------------------------------
// 17. Toasts
// ---------------------------------------------------------------------------

pub mod toast {
    pub const COPIED_CLIPBOARD: &str = "Copied to the clipboard";
    pub const COPIED_CLEARED: &str = "The clipboard has been cleared";
    pub const RESUMED: &str = "Backups resumed";
    pub const SETTINGS_SAVED: &str = "Saved";
    pub const JOBS_DISABLED_ALL: &str = "All jobs disabled";
    pub const JOBS_ENABLED_ALL: &str = "Jobs re-enabled";
}

pub fn toast_chain_source_added(name: &str) -> String {
    format!("{name} was added too — the copy is made from it, so it has to run in the same job.")
}
pub fn onboarding_vault_failed(detail: &str) -> String {
    format!("The vault could not be created: {detail}")
}
pub fn dest_repo_create_title(name: &str) -> String {
    format!("Set up a repository in {name}?")
}
pub fn dest_repo_create_body(kind: &str) -> String {
    format!(
        "The {kind} is reachable. A repository is the encrypted store superbackup writes snapshots into — until one exists here, this destination cannot be backed up to."
    )
}
pub fn toast_creating_repository(name: &str) -> String {
    format!("Setting up the repository in {name}…")
}
pub fn toast_created(name: &str) -> String {
    format!("{name} created")
}
pub fn toast_saved(name: &str) -> String {
    format!("{name} saved")
}
pub fn toast_deleted(name: &str) -> String {
    format!("{name} deleted")
}
pub fn toast_removed(name: &str) -> String {
    format!("{name} removed")
}
pub fn toast_enabled(name: &str) -> String {
    format!("{name} enabled")
}
pub fn toast_disabled(name: &str) -> String {
    format!("{name} disabled")
}
pub fn toast_run_started(name: &str) -> String {
    format!("Backing up {name}")
}
pub fn toast_run_finished(name: &str, bytes: u64, duration: &str) -> String {
    format!("{name} finished — {} uploaded in {duration}", format::bytes(bytes))
}
pub fn toast_run_warnings(name: &str, count: usize) -> String {
    format!("{name} finished with {count} warnings")
}
pub fn toast_run_failed(name: &str, message: &str) -> String {
    format!("{name} failed — {message}")
}
pub fn toast_paused(time: &str) -> String {
    format!("Backups paused until {time}")
}
pub fn toast_repo_created(location: &str) -> String {
    format!("Repository created at {location}")
}
pub fn toast_export_done(path: &str) -> String {
    format!("Saved to {path}")
}

// ---------------------------------------------------------------------------
// 18. Accessibility
// ---------------------------------------------------------------------------

pub mod a11y {
    pub const PASSPHRASE_BLOCK: &str = "Repository encryption key. Focus this and use your screen reader's character-by-character reading to hear it.";
}

pub fn a11y_rail_item(label: &str, index: usize, total: usize) -> String {
    format!("{label}, section {index} of {total}")
}
pub fn a11y_rail_selected(label: &str, index: usize, total: usize) -> String {
    format!("{label}, section {index} of {total}, current")
}
pub fn a11y_rail_attention(label: &str) -> String {
    format!("{label}, needs attention")
}
pub fn a11y_vault_unlocked(duration: &str) -> String {
    format!("Vault unlocked, locks in {duration}. Activate for vault options.")
}
pub const A11Y_VAULT_LOCKED: &str =
    "Vault locked, scheduled backups are blocked. Activate to unlock.";
pub fn a11y_health(health: &str, reason: &str) -> String {
    format!("Overall health: {health}. {reason}")
}
pub fn a11y_job_card(
    name: &str,
    status: &str,
    last: &str,
    next: &str,
    destinations: usize,
) -> String {
    format!("{name}, {status}, last run {last}, next run {next}, {destinations} destinations")
}
pub fn a11y_job_card_running(name: &str, percent: i64) -> String {
    format!("{name}, running, {percent} percent complete")
}
pub fn a11y_job_card_disabled(name: &str, status: &str, last: &str) -> String {
    format!("{name}, disabled, {status} {last}")
}
pub fn a11y_progress(job: &str, destination: &str, percent: i64, done: u64, total: u64) -> String {
    format!(
        "Backing up {job} to {destination}, {percent} percent, {} of {} files",
        format::count(done),
        format::count(total)
    )
}
pub fn a11y_progress_estimating(job: &str, destination: &str) -> String {
    format!("Backing up {job} to {destination}, still working out how much there is")
}
pub fn a11y_progress_restore(percent: i64, done: u64, total: u64) -> String {
    format!(
        "Restoring, {percent} percent, {} of {} files",
        format::count(done),
        format::count(total)
    )
}
pub fn a11y_strength(level: &str) -> String {
    format!("Passphrase strength: {level}")
}
pub fn a11y_exclusion(title: &str, checked: &str, count: usize, rationale: &str) -> String {
    format!("{title}, {checked}, {count} patterns. {rationale}")
}
pub fn a11y_exclusion_risky(title: &str, checked: &str, count: usize, rationale: &str) -> String {
    format!("{title}, {checked}, {count} patterns. {rationale} This one may lose data.")
}
pub fn a11y_destination_row(name: &str, kind: &str, status: &str, checked: &str) -> String {
    format!("{name}, {kind}, {status}, {checked}")
}
pub fn a11y_table(name: &str, rows: usize, columns: usize) -> String {
    format!("{name} table, {rows} rows, {columns} columns")
}
pub fn a11y_table_sorted(column: &str, direction: &str) -> String {
    format!("Sorted by {column}, {direction}")
}
pub fn a11y_disabled_locked(label: &str) -> String {
    format!("{label}, unavailable while the vault is locked")
}
pub fn a11y_busy(label: &str) -> String {
    format!("{label}, busy")
}
pub fn a11y_dirty_tab(label: &str) -> String {
    format!("{label}, has unsaved changes")
}
pub fn a11y_form_invalid(label: &str, count: usize) -> String {
    format!("{label}, {count} problems to fix")
}
pub fn a11y_toast(title: &str, body: &str) -> String {
    format!("{title}. {body}")
}
pub fn a11y_breadcrumb(path: &str, items: usize) -> String {
    format!("Location: {path}, {items} items")
}

/// Outcome of testing a storage provider, shown wherever the user pressed it.
pub fn toast_provider_reachable(name: &str, detail: &str) -> String {
    if detail.is_empty() {
        format!("{name} answered.")
    } else {
        format!("{name} answered. {detail}")
    }
}

pub fn toast_provider_unreachable(name: &str, detail: &str) -> String {
    if detail.is_empty() {
        format!("{name} could not be reached.")
    } else {
        format!("{name} could not be reached. {detail}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_deck_has_no_exclamation_marks() {
        // Voice rule 2 in COPY.md, enforced on the strings this module owns.
        for s in [
            onboarding::NORECOVERY_BODY,
            vault::UNLOCK_WRONG,
            dest::MIRROR_EXPLAIN,
            job::HOOKS_WARNING,
            err::DAEMON_UNREACHABLE,
        ] {
            assert!(!s.contains('!'), "exclamation mark in: {s}");
        }
    }

    #[test]
    fn formatted_strings_substitute_in_the_documented_order() {
        assert_eq!(window_title("Up to date"), "superbackup — Up to date");
        assert_eq!(dash_health_failed("Dev code", "2 hours ago"), "Dev code failed 2 hours ago");
        assert_eq!(valid_form_problems(1), "1 problem to fix");
        assert_eq!(valid_form_problems(2), "2 problems to fix");
        assert_eq!(restore_browse_restore_n(1), "Restore 1 item");
        assert_eq!(restore_browse_restore_n(3), "Restore 3 items");
    }

    #[test]
    fn triggers_render_as_words() {
        use superbackup_core::state::Trigger;
        assert_eq!(trigger(Trigger::Cli), "Command line");
        assert_eq!(trigger(Trigger::CatchUp), "Catch-up");
    }
}
