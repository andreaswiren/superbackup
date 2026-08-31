# superbackup — Copy Deck

Version 1.0 · Every user-facing string in the application.

## How to use this file

- Keys are stable identifiers. They map to `const` items in
  `crates/app/src/copy.rs`, grouped by the module comments below.
- `{placeholders}` are substituted at runtime. Their types are noted where it is
  not obvious.
- Strings are shown in fenced blocks as `key = "value"` so they can be pasted
  and mechanically converted.
- Multi-line strings use `"""…"""`; the line breaks are literal paragraph
  breaks.

## Voice rules (enforced across this file)

1. **Plain English.** No "utilise", no "leverage", no "initialise". A backup is
   a backup, a folder is a folder.
2. **No exclamation marks.** Anywhere. Not in success messages, not in errors.
3. **Never blame the user.** Not "you entered an invalid path" but "that folder
   could not be found". The subject of a failure sentence is the thing, not the
   person.
4. **Say what happened, then what to do.** Errors are two clauses, in that
   order.
5. **Sentence case** for every label, button, heading and column header.
6. **Second person** for instructions ("Choose a folder"), **third person** for
   state ("The vault is locked").
7. **No jargon on the surface, exact terms in the detail.** "Skips build output"
   on the toggle; `DYNAMIC-2M-BUZHASH` in the field beside it.
8. **Numbers are specific.** "Kept for 14 days", not "kept for a while".
9. **No apologies.** "Sorry" appears nowhere in this file.
10. **British-neutral spelling**, matching the codebase (`normalise`,
    `behaviour`), except where a term is a proper noun or a `kopia` identifier.

---

## 1. Application-wide

```
app.name                  = "superbackup"
app.tagline               = "Backups for machines full of code."
app.window_title          = "superbackup — {health}"
app.window_title_running  = "superbackup — Backing up {job} ({percent}%)"

action.save               = "Save"
action.save_changes       = "Save changes"
action.cancel             = "Cancel"
action.back               = "Back"
action.continue           = "Continue"
action.done               = "Done"
action.close              = "Close"
action.delete             = "Delete"
action.remove             = "Remove"
action.edit               = "Edit"
action.duplicate          = "Duplicate"
action.add                = "Add"
action.browse             = "Browse…"
action.open_folder        = "Open folder"
action.copy               = "Copy"
action.copied             = "Copied"
action.retry              = "Retry"
action.verify             = "Verify"
action.verify_now         = "Verify now"
action.test_connection    = "Test connection"
action.run_now            = "Run now"
action.back_up_now        = "Back up now"
action.stop               = "Stop"
action.enable             = "Enable"
action.disable            = "Disable"
action.unlock             = "Unlock"
action.lock_now           = "Lock now"
action.show_details       = "Show technical details"
action.hide_details       = "Hide technical details"
action.copy_details       = "Copy details"
action.clear_filters      = "Clear filters"
action.learn_more         = "Learn more"

state.never               = "Never"
state.none                = "— none —"
state.unknown             = "Unknown"
state.calculating         = "Calculating…"
state.estimating          = "Estimating…"
state.loading             = "Loading…"
```

### 1.1 Status vocabulary

These mirror `RunStatus::title()` and `Health::title()` exactly. The GUI reads
them from the core rather than redefining them; they are listed here so writers
can see the full vocabulary in one place.

```
health.idle               = "Up to date"
health.running            = "Backing up"
health.attention          = "Needs attention"
health.paused             = "Paused"
health.failed             = "Backup failed"

run.queued                = "Queued"
run.preparing             = "Preparing"
run.running               = "Running"
run.finalising            = "Finalising"
run.succeeded             = "Succeeded"
run.warnings              = "Completed with warnings"
run.failed                = "Failed"
run.cancelled             = "Cancelled"
run.skipped               = "Skipped"

badge.warnings_short      = "Warnings"
badge.never_run           = "Never run"
badge.disabled            = "Disabled"

trigger.schedule          = "Schedule"
trigger.manual            = "Manual"
trigger.cli               = "Command line"
trigger.file_change       = "File change"
trigger.catch_up          = "Catch-up"
trigger.retry             = "Retry"

kind.local_repository     = "Local repository"
kind.onedrive             = "OneDrive repository"
kind.s3                   = "S3 bucket"
kind.mirror               = "Folder mirror"
```

---

## 2. Onboarding

### O-1 Welcome

```
onboarding.welcome.title      = "Welcome to superbackup"
onboarding.welcome.body       = "Set up encrypted, scheduled backups of the folders you work in. This takes about two minutes."

onboarding.welcome.f1.title   = "One job, many copies"
onboarding.welcome.f1.body    = "Send the same folders to a fast local disk, a folder you already sync, and offsite storage."
onboarding.welcome.f2.title   = "Skips what you can rebuild"
onboarding.welcome.f2.body    = "node_modules, build output and caches are left out, so backups stay small and finish quickly."
onboarding.welcome.f3.title   = "Encrypted before it leaves"
onboarding.welcome.f3.body    = "Everything is encrypted on this machine. Storage providers only ever see ciphertext."

onboarding.welcome.kopia      = "superbackup runs Kopia, an open-source backup engine. See kopia.io."
```

### O-2 Master passphrase

```
onboarding.pass.title         = "Create your master passphrase"
onboarding.pass.lead          = "This passphrase unlocks the vault that holds your repository encryption keys and storage keys. You will type it when you start superbackup, and when a backup needs to run unattended."
onboarding.pass.not_repo      = "This is not the passphrase of any single backup repository. It is the one that protects all of them."

onboarding.pass.field         = "Master passphrase"
onboarding.pass.confirm       = "Confirm passphrase"
onboarding.pass.suggest       = "Suggest a passphrase"
onboarding.pass.suggested     = "A six-word passphrase has been filled in. Save it before you continue."

onboarding.pass.req_length    = "At least 12 characters"
onboarding.pass.req_unique    = "Not a password you use anywhere else"
onboarding.pass.req_words     = "Four or more words is stronger than a short mix of symbols"

strength.too_weak             = "Too weak"
strength.weak                 = "Weak"
strength.good                 = "Good"
strength.strong               = "Strong"
strength.label                = "Passphrase strength: {level}"
```

### O-3 There is no recovery

```
onboarding.norecovery.title   = "There is no way to recover this"
onboarding.norecovery.body    = """
Your master passphrase encrypts the vault on this machine. It is never sent anywhere, and it is not stored in a form anyone can read.

That means there is no reset link, no backdoor and no support address that can open your vault for you. If the passphrase is lost, the repository keys inside are lost with it, and the backups they protect cannot be read again.

Put it in a password manager now, or write it down and keep the paper somewhere you would keep a spare key.
"""
onboarding.norecovery.copy    = "Copy passphrase to clipboard"
onboarding.norecovery.copied  = "Copied. The clipboard will be cleared in 60 seconds."
onboarding.norecovery.save    = "Save a recovery sheet…"
onboarding.norecovery.save_note = "The recovery sheet is a plain text file. Anyone who can read the file can read the passphrase."
onboarding.norecovery.ack     = "I have stored my master passphrase somewhere I can get to it. If I lose it, my backups cannot be recovered."
onboarding.weak_ack           = "I understand this passphrase is weak and I want to use it anyway."
```

### O-4 Scan

```
onboarding.scan.title         = "Checking this machine"
onboarding.scan.lead          = "Looking for the pieces superbackup can use."

onboarding.kopia.found        = "Kopia {version}"
onboarding.kopia.missing      = "Kopia was not found"
onboarding.kopia.missing_body = "superbackup uses Kopia to write and read backups. You can download a tested build now, or point superbackup at a copy you already have."
onboarding.kopia.download     = "Download Kopia"
onboarding.kopia.choose       = "Choose a file…"
onboarding.kopia.downloading  = "Downloading Kopia {version}…"
onboarding.kopia.verify       = "Checking the download…"
onboarding.kopia.skip_note    = "You can set this up later. Backups will not run until Kopia is available."

onboarding.onedrive.found     = "OneDrive — {account}"
onboarding.onedrive.create    = "Create a OneDrive destination here"
onboarding.onedrive.explain   = "A repository is a small number of large files, not the millions of small ones that make OneDrive struggle. superbackup also marks the folder so OneDrive keeps it on this disk instead of turning it into an online-only placeholder."
onboarding.onedrive.none      = "No OneDrive folder was found. That is fine — you can back up to a local disk or to object storage instead."

onboarding.disk.ok            = "{free} free on {drive}"
onboarding.disk.low           = "Only {free} free on {drive}. A first backup of a development folder is often several gigabytes."
```

### O-5 First job

```
onboarding.job.title          = "Your first backup job"
onboarding.job.lead           = "Pick a starting point. You can change everything afterwards."

template.dev.title            = "Development folder"
template.dev.body             = "Your code, without the parts that rebuild themselves."
template.dev.detail           = "Skips node_modules, build output, caches and virtualenvs. Applies 10 exclusion presets."
template.dev.eyebrow          = "Recommended for developers"
template.docs.title           = "Documents and desktop"
template.docs.body            = "The folders most people lose first."
template.docs.detail          = "Skips operating-system junk files and temporary files."
template.home.title           = "Whole user folder"
template.home.body            = "Everything under your home folder."
template.home.detail          = "Skips build output, caches and virtual machine images. Expect a large first run."
template.blank.title          = "Start from scratch"
template.blank.body           = "Choose the folders yourself."
template.blank.detail         = "No exclusions and no schedule until you add them."

onboarding.job.sources        = "Folders to back up"
onboarding.job.destinations   = "Where to keep the copies"
onboarding.job.later          = "Object storage such as StorJ or S3 can be added later, in Destinations."
onboarding.job.derived        = "The repository encryption key is worked out from your master passphrase, so there is only one secret to keep safe."
onboarding.job.derived_change = "Change…"
onboarding.job.name           = "Job name"
onboarding.job.review         = "Ready to create"
onboarding.job.estimate       = "About {size} in {files} files after exclusions"
onboarding.job.estimate_none  = "Size could not be estimated. The job will still work."
```

### O-6 Keep it running

```
onboarding.run.title          = "Keep it running"
onboarding.run.lead           = "Backups only help if they happen without you thinking about them."

onboarding.autostart.title    = "Start superbackup when I sign in"
onboarding.autostart.body     = "superbackup sits in the tray and runs your schedules."
onboarding.minimised.title    = "Start minimised to the tray"
onboarding.minimised.body     = "No window on sign-in. The tray icon shows the current state."

onboarding.service.title      = "Install the background service"
onboarding.service.body       = "The service runs backups even when nobody is signed in. To do that it needs the master passphrase without a person to type it, which means storing the key in this computer's credential store."
onboarding.service.keychain   = "Store the vault key in {keychain_name}"
onboarding.service.keychain_warn = "Anything that can run programs as you can then ask the credential store for the key."
onboarding.service.elevate    = "Installing the service asks for administrator permission."
onboarding.service.declined   = "The service was not installed. Backups will run while you are signed in."
```

### O-7 Done

```
onboarding.done.title         = "You are set up"
onboarding.done.summary       = "{jobs} · {destinations} · Next run {next}"
onboarding.done.tray          = "superbackup now lives in the tray. Closing the window does not stop backups — use Quit in the tray menu for that."
onboarding.done.primary       = "Back up now"
onboarding.done.secondary     = "Go to dashboard"
```

### Onboarding edge cases

```
onboarding.resume             = "Your master passphrase is already set. Continuing where you left off."
onboarding.orphan.title       = "The configuration is here, but the vault is missing"
onboarding.orphan.body        = "config.json refers to stored keys and passphrases that cannot be found. Restoring config.sbvault from a backup keeps everything as it was. Starting over keeps your jobs but needs every credential entered again."
onboarding.orphan.restore     = "Restore config.sbvault from a backup"
onboarding.orphan.startover   = "Start over and re-enter every credential"
onboarding.remote.found       = "Remote configuration found"
onboarding.remote.body        = "A shared configuration is set up at {url}. Pulling it brings this machine's jobs and destinations in line with it."
```

---

## 3. Vault and locking

```
vault.unlocked                = "Unlocked"
vault.locked                  = "Locked"
vault.locked_sub              = "Schedules are blocked"
vault.locks_in                = "Locks in {duration}"
vault.lock_menu.lock          = "Lock now"
vault.lock_menu.change        = "Change master passphrase…"
vault.lock_menu.settings      = "Auto-lock settings…"

vault.banner.title            = "The vault is locked"
vault.banner.body             = "Scheduled backups will not start, and destinations cannot be reached, until it is unlocked."
vault.banner.action           = "Unlock"

vault.unlock.title            = "Unlock superbackup"
vault.unlock.body             = "Your master passphrase decrypts the repository encryption keys and storage keys needed to run backups."
vault.unlock.field            = "Master passphrase"
vault.unlock.remember         = "Remember until I sign out"
vault.unlock.button           = "Unlock"
vault.unlock.busy             = "Unlocking…"
vault.unlock.wrong            = "That passphrase did not open the vault. Passphrases are case sensitive."
vault.unlock.no_recovery      = "There is no way to recover a lost master passphrase. If you have a recovery sheet, this is the moment for it."
vault.unlocked_toast          = "Vault unlocked"
vault.locked_toast            = "Vault locked"

vault.autolock.warning        = "superbackup will lock in one minute."
vault.autolock.stay           = "Stay unlocked"

locked.action_blocked         = "Unlock the vault to use this."
locked.inline.prompt          = "Unlock to enter credentials"
locked.restore.title          = "Unlock to browse your backups"
locked.restore.body           = "Listing snapshots needs the repository encryption key, which is kept in the vault."
locked.next_run               = "blocked while locked"
locked.paused_next_run        = "blocked while paused"
```

---

## 4. Empty states

```
empty.jobs.title              = "No backup jobs yet"
empty.jobs.body               = "A job is a set of folders, a schedule, and the places the copies go."
empty.jobs.primary            = "Create your first job"
empty.jobs.secondary          = "Import from another machine…"

empty.jobs.filtered.title     = "No jobs match those filters"
empty.jobs.filtered.body      = "Try a different search term, or clear the filters to see all {count} jobs."

empty.sources.title           = "No folders yet"
empty.sources.body            = "Add the folders this job should back up. You can drop folders onto the window as well."
empty.sources.primary         = "Add folder…"

empty.destinations.title      = "No destinations yet"
empty.destinations.body       = "A destination is a place backups are written to: a local disk, a folder you already sync, or object storage."
empty.destinations.primary    = "Add a destination"
empty.destinations.secondary  = "Learn about the four kinds"

empty.destinations.injob.title = "This job has nowhere to go"
empty.destinations.injob.body = "Add a destination first, then choose it here."

empty.providers.title         = "No storage providers yet"
empty.providers.body          = "A provider holds the endpoint and keys for an object-storage account. Define it once and reuse it for every bucket. Local disks and OneDrive folders do not need one."
empty.providers.primary       = "Add a storage provider"

empty.activity.title          = "Nothing has run yet"
empty.activity.body           = "Runs appear here as soon as a job starts, whether it was scheduled or you started it yourself."
empty.activity.primary        = "Back up now"

empty.activity.filtered.title = "No runs match those filters"
empty.activity.filtered.body  = "superbackup keeps the last 200 runs. Older activity is in the event log."

empty.events.title            = "No events recorded"
empty.events.body             = "Events are written as things happen: jobs starting, repositories being created, the vault being unlocked."

empty.restore.no_destinations.title = "Nothing to restore from yet"
empty.restore.no_destinations.body  = "Restoring needs a repository destination. Folder mirrors can be opened directly in your file manager instead."
empty.restore.no_destinations.primary = "Add a destination"

empty.restore.no_snapshots.title = "No snapshots yet"
empty.restore.no_snapshots.body  = "Once a job has run successfully, its snapshots appear here and you can browse them file by file."
empty.restore.no_snapshots.primary = "Back up now"

empty.restore.mirrors_only.title = "Your destinations are folder mirrors"
empty.restore.mirrors_only.body  = "A mirror is a plain copy of your files. Open the folder and copy what you need — there is no snapshot history to browse."

empty.snapshot.dir.title      = "This folder was empty in this snapshot"
empty.snapshot.dir.body       = "Try an earlier snapshot from the picker above."

empty.vault_backups.title     = "No vault backups yet"
empty.vault_backups.body      = "A copy of the vault is written here before every change to it."
```

---

## 5. Dashboard

```
dash.health.label             = "Overall health"
dash.health.idle.last         = "Last backup {relative}"
dash.health.idle.never        = "No backups yet"
dash.health.running           = "{count} running"
dash.health.paused_until      = "Paused until {time}"
dash.health.paused_forever    = "Paused until you resume"
dash.health.paused_reason     = "Paused until {time} — {reason}"
dash.health.failed            = "{job} failed {relative}"
dash.health.failed_more       = "{job} failed {relative}, and {count} others"
dash.health.att.locked        = "The vault is locked"
dash.health.att.kopia         = "Kopia was not found"
dash.health.att.stale         = "{count} jobs have not succeeded for {days} days"
dash.health.att.unverified    = "{count} destinations have never been verified"

dash.next.label               = "Next scheduled run"
dash.next.none                = "Not scheduled"
dash.next.none_action         = "Set up a schedule"
dash.next.value               = "{job} · {absolute}"

dash.week.label               = "Last 7 days"
dash.week.summary             = "{runs} runs, {failed} failed"
dash.week.day_tooltip         = "{date}: {succeeded} succeeded, {warned} with warnings, {failed} failed"
dash.week.none                = "Nothing has run in the last 7 days"

dash.running.title            = "Running now"
dash.running.stop_all         = "Stop all"
dash.running.started          = "Started {relative} · triggered by {trigger}"
dash.running.counts           = "{files_done} of {files_total} files · {bytes_done} of {bytes_total} · {rate}/s"
dash.running.counts_partial   = "{files_done} files · {bytes_done} · {rate}/s"
dash.running.eta              = "~{duration} left"
dash.running.scanning         = "Scanning {path}"
dash.running.cached_tooltip   = "{count} files unchanged since the last run"
dash.running.skipped          = "{count} files skipped"

dash.jobs.title               = "Jobs"
dash.jobs.run_all             = "Run all now"
dash.jobs.disable_all         = "Disable all jobs"
dash.jobs.new                 = "New job…"

card.meta.succeeded           = "Last run {relative} · {duration} · {bytes} uploaded"
card.meta.warnings            = "Last run {relative} · {duration} · {skipped} files skipped"
card.meta.failed              = "Failed {relative} · {ordinal} failure in a row"
card.meta.failed_first        = "Failed {relative}"
card.meta.running             = "Started {relative} · {trigger}"
card.meta.queued              = "Queued behind {job}"
card.meta.never               = "Never run · {sources} folders · next run {relative}"
card.meta.never_manual        = "Never run · {sources} folders · runs only when you ask"
card.meta.disabled            = "Disabled · last result {status} {relative}"
card.meta.stale               = "Last success {relative}"
card.action.view_error        = "View error"
```

---

## 6. Jobs

```
jobs.title                    = "Jobs"
jobs.new                      = "New job"
jobs.search                   = "Search jobs"
jobs.group_by                 = "Group by"
jobs.group.none               = "None"
jobs.group.project            = "Project"
jobs.group.schedule           = "Schedule"
jobs.filter                   = "Filter"
jobs.filter.all               = "All"
jobs.filter.enabled           = "Enabled"
jobs.filter.disabled          = "Disabled"
jobs.filter.failing           = "Failing"
jobs.filter.stale             = "Not backed up recently"
jobs.ungrouped                = "Ungrouped"
jobs.run_group                = "Run group"
jobs.selected                 = "{count} selected"

col.status                    = "Status"
col.name                      = "Name"
col.sources                   = "Folders"
col.destinations              = "Destinations"
col.schedule                  = "Schedule"
col.last_run                  = "Last run"
col.next_run                  = "Next run"
col.uploaded                  = "Uploaded"
col.location                  = "Location"
col.used_by                   = "Used by"
col.size                      = "Size"
col.last_verified             = "Last verified"
col.endpoint                  = "Endpoint"
col.started                   = "Started"
col.job                       = "Job"
col.trigger                   = "Started by"
col.duration                  = "Duration"
col.severity                  = "Severity"
col.time                      = "Time"
col.event                     = "Event"
col.message                   = "Message"
col.modified                  = "Modified"
col.when                      = "When"
col.files                     = "Files"
col.id                        = "Id"
```

### Job editor

```
job.tab.sources               = "Folders"
job.tab.destinations          = "Destinations"
job.tab.schedule              = "Schedule"
job.tab.exclusions            = "Exclusions"
job.tab.advanced              = "Advanced"

job.name                      = "Name"
job.name.placeholder          = "Dev code"
job.description               = "Description"
job.description.placeholder   = "What this job is for"
job.project                   = "Project"
job.project.new               = "New project…"
job.tags                      = "Tags"
job.tags.placeholder          = "Add a tag"

job.sources.title             = "Folders to back up"
job.sources.add               = "Add folder…"
job.sources.hint              = "Everything under each folder is included, minus your exclusions. You can also drop folders onto this window."
job.sources.follow_symlinks   = "Follow symbolic links"
job.sources.follow_tooltip    = "Off by default. Following links out of the folder is how a backup of one project quietly grows to cover the whole disk."
job.sources.one_filesystem    = "Stay on one filesystem"
job.sources.one_fs_tooltip    = "Do not cross into mounted drives or network shares found inside this folder."
job.sources.missing           = "This folder is not there at the moment. The job will skip it and record a warning."
job.sources.dup               = "That folder is already in this job."
job.sources.child             = "That folder is already covered by {parent}."
job.sources.parent            = "{path} contains {count} folders already in this job. Replace them with the parent folder?"
job.sources.parent_replace    = "Replace them"

job.dest.title                = "Send this backup to"
job.dest.lead                 = "Every destination you tick receives a complete copy. A failure at one does not stop the others."
job.dest.new                  = "New destination…"
job.dest.verified             = "Verified {relative}"
job.dest.never_verified       = "Never verified"
job.dest.unreachable          = "Unreachable"
job.dest.disabled_row         = "This destination is switched off, so jobs skip it."
job.dest.disabled_enable      = "Enable in Destinations"
job.dest.continue_on_error    = "Keep going to the other destinations"
job.dest.continue_body        = "With this off, the first destination that fails stops the run and the rest are recorded as cancelled."
job.dest.mixed_warning        = "This job writes to both a repository and a folder mirror. Mirrors hold one plain copy with no history, so retention and encryption settings do not apply to them."
job.err.no_destinations       = "Choose at least one destination. A job with nowhere to write cannot run."

job.schedule.manual           = "Manual only"
job.schedule.manual_body      = "Runs when you ask, or when the command line asks."
job.schedule.interval         = "Every so often"
job.schedule.interval_unit    = "minutes"
job.schedule.interval_warn    = "Running more often than every 15 minutes keeps the disk busy on large folders."
job.schedule.daily            = "Daily at"
job.schedule.weekly           = "Weekly on"
job.schedule.add_time         = "Add time…"
job.schedule.cron             = "Cron expression"
job.schedule.cron_help        = "Cron help"
job.schedule.cron_next        = "Next five runs: {times}"
job.schedule.onchange         = "When files change"
job.schedule.debounce         = "Wait for quiet"
job.schedule.debounce_unit    = "seconds"
job.schedule.min_interval     = "At most once every"
job.schedule.min_unit         = "minutes"
job.schedule.onchange_body    = "The job runs once the folders have been quiet for the waiting period, and never more often than the minimum interval."
job.schedule.onchange_large   = "One of these folders holds more than 50,000 files. Watching a tree that size uses noticeable memory."
job.schedule.next_five        = "Next five runs: {times}"
job.schedule.next_none        = "This job runs only when you ask."

job.conditions.title          = "Run conditions"
job.conditions.metered        = "Skip when on a metered connection"
job.conditions.battery        = "Skip when on battery"
job.conditions.using_global   = "Using the global setting"
job.conditions.overriding     = "Overriding the global setting"
job.conditions.reset          = "Reset"
job.timeout                   = "Stop the run after"
job.timeout.unit              = "minutes"
job.timeout.body              = "A run stopped by its timeout is recorded as failed, because something took longer than it should have."

job.excl.title                = "Exclusions"
job.excl.lead                 = "Leaving out files you can rebuild is what keeps a developer backup small enough to finish every night."
job.excl.select_defaults      = "Select developer defaults"
job.excl.clear_all            = "Clear all"
job.excl.defaults_applied     = "Developer defaults applied: 10 presets, {patterns} patterns."
job.excl.patterns_count       = "{count} patterns"
job.excl.risky                = "Excluding this can lose work that exists nowhere else."
job.excl.gitignore            = "Use .gitignore files found in the folders"
job.excl.gitignore_body       = "Honours each repository's own ignore rules. Slower on very large trees, because every directory is checked."
job.excl.cachedir             = "Skip folders tagged with CACHEDIR.TAG"
job.excl.cachedir_body        = "A standard marker that tools use to say a folder holds only regenerable cache."
job.excl.max_size             = "Skip files larger than"
job.excl.max_size_unit        = "MB"
job.excl.max_size_body        = "Files over this size are left out of every snapshot and listed in the run's warnings."
job.excl.custom               = "Your own patterns"
job.excl.custom_body          = "One pattern per line, in .gitignore syntax, relative to each folder you back up."
job.excl.custom_placeholder   = """
/**/*.psd
/**/coverage/
secrets.local.json
"""
job.excl.show_effective       = "Show all effective patterns ({count})"
job.excl.impact               = "These rules leave out about {size} in {files} files."
job.excl.impact_none          = "These rules do not match anything in the folders you chose."
job.excl.impact_failed        = "The size of the excluded files could not be worked out."

job.bandwidth.title           = "Bandwidth"
job.bandwidth.global          = "Use the global limit"
job.bandwidth.custom          = "Set a limit for this job"
job.bandwidth.upload          = "Upload limit"
job.bandwidth.download        = "Download limit"
job.bandwidth.unit            = "kB/s"
job.bandwidth.current_global  = "Global limit: {upload} up, {download} down"
job.bandwidth.unlimited       = "unlimited"
job.bandwidth.no_window       = "The daily window is a global setting, so two jobs can never disagree about it. Set it in Settings › Bandwidth."

job.retention.title           = "Retention"
job.retention.per_dest        = "Use each destination's policy"
job.retention.custom          = "Set a policy for this job"
job.retention.latest          = "Latest"
job.retention.hourly          = "Hourly"
job.retention.daily           = "Daily"
job.retention.weekly          = "Weekly"
job.retention.monthly         = "Monthly"
job.retention.annual          = "Annual"
job.retention.maintenance     = "Run maintenance every"
job.retention.maintenance_unit = "successful runs"
job.retention.summary         = "Keeps the {latest} most recent snapshots, then {hourly} hourly, {daily} daily, {weekly} weekly, {monthly} monthly and {annual} annual snapshots."
job.retention.mirror_note     = "Retention applies to repositories. A folder mirror always holds exactly one copy."
retention.err.all_zero        = "At least one of these needs to be above zero, or every snapshot would be removed as soon as it is written."

job.hooks.title               = "Hooks"
job.hooks.before              = "Before the backup"
job.hooks.after_success       = "After a successful backup"
job.hooks.after_failure       = "After a failed backup"
job.hooks.abort               = "Cancel the backup if this command fails"
job.hooks.warning             = "Hooks run as you, with your permissions. superbackup does not restrict what they can do."
job.hooks.env                 = "Available to the command: SUPERBACKUP_JOB_NAME, SUPERBACKUP_RUN_ID, SUPERBACKUP_STATUS, SUPERBACKUP_DESTINATIONS. Each command is stopped after 120 seconds, and its output is kept with the run."

job.danger.title              = "Danger zone"
job.danger.delete             = "Delete this job"
job.danger.body               = "Deleting removes the job definition from this machine. Snapshots already written to any destination are left exactly as they are."
job.unsaved.title             = "Save your changes to {job}?"
job.unsaved.body              = "You have unsaved changes on the {tabs} tab."
job.unsaved.discard           = "Discard"
```

---

## 7. Destinations

```
dest.title                    = "Destinations"
dest.new                      = "New destination"
dest.search                   = "Search destinations"
dest.filter.kind              = "Kind"
dest.auto_found               = "Found automatically"
dest.status.ready             = "Ready"
dest.status.not_connected     = "Not connected"
dest.status.unreachable       = "Unreachable"
dest.used_by                  = "{count} jobs"
dest.used_by_none             = "Not used yet"

dest.name                     = "Name"
dest.name.placeholder         = "Local repo"
dest.kind                     = "Kind"
dest.kind.locked              = "The kind is fixed once a destination exists. Create a new destination to use a different one."
dest.enabled                  = "Enabled"
dest.enabled.body             = "A switched-off destination is skipped by every job, without failing them."

dest.folder                   = "Folder"
dest.folder.will_create       = "This folder will be created."
dest.folder.free              = "{free} free of {total}"
dest.folder.low               = "Only {free} free. A first backup of a development folder is often several gigabytes."
dest.folder.removable         = "Removable drive — backups run only while it is connected."
dest.folder.network           = "Network location — backups depend on the share being reachable."
dest.folder.found_repo        = "There is already a repository here."
dest.folder.found_repo_action = "Connect to it"

dest.onedrive.account         = "Account"
dest.onedrive.account_body    = "A label for your own benefit. superbackup does not sign in to OneDrive and does not need to."
dest.onedrive.explain         = "The backup is written as a repository: a modest number of large files rather than the millions of small ones that make OneDrive struggle. That is the whole point of putting it here."
dest.onedrive.pin             = "Keep these files on this disk"
dest.onedrive.pin_body        = "Stops OneDrive turning the repository into online-only placeholders. With this off, a restore may have to download before it can read."
dest.onedrive.redetect        = "Check for OneDrive again"

dest.s3.provider              = "Storage provider"
dest.s3.provider_new          = "New provider…"
dest.s3.provider_edit         = "Edit provider"
dest.s3.bucket                = "Bucket"
dest.s3.list_buckets          = "List buckets"
dest.s3.prefix                = "Key prefix"
dest.s3.prefix_body           = "The default contains this machine's folder name, which is what keeps several computers and several jobs apart inside one bucket."
dest.s3.full_path             = "Full path: s3://{bucket}/{prefix}"
dest.s3.prefix_normalised     = "Saved as {prefix}"
dest.s3.creds                 = "Credentials for this bucket"
dest.s3.creds.inherit         = "Use the provider's credentials"
dest.s3.creds.inherit_body    = "Uses the keys stored on {provider}."
dest.s3.creds.own             = "Use a separate key pair for this bucket"
dest.s3.creds.own_body        = "A key that only reaches this bucket limits what a leaked credential can touch."

dest.mirror.explain           = "A mirror is a plain, readable copy of the newest version of each file. There are no snapshots, no history, no deduplication and no encryption — anyone who can read the folder can read your files."
dest.mirror.prune             = "Delete files in the mirror that no longer exist in the folders"
dest.mirror.prune_body        = "With this on, deleting a file removes it from the mirror on the next run, so the mirror stops protecting you from an accidental deletion."

dest.verify.checking_path     = "Checking the path…"
dest.verify.writing           = "Writing a test file…"
dest.verify.opening           = "Opening the repository…"
dest.verify.head              = "Reaching the bucket…"
dest.verify.ok                = "Verified. Everything needed is in place."
dest.verify.ok_toast          = "{name} verified"

dest.connect.title            = "Connect to this repository"
dest.connect.body             = "There is already a repository at this location. Its passphrase is needed once, and is then kept in your vault."
dest.connect.derive           = "Work it out from my master passphrase"
dest.connect.type             = "I will type it"
dest.connect.field            = "Repository encryption key"
dest.connect.wrong            = "That passphrase did not open this repository."
dest.connect.settings_note    = "These settings were chosen when the repository was created and cannot be changed."

dest.delete.title             = "Remove {name}?"
dest.delete.body              = "This removes the destination from superbackup. The data at the destination is not touched."
dest.delete.jobs              = "{count} jobs write here and will keep running to their other destinations: {names}"
dest.delete.orphans           = "These jobs would be left with nowhere to write and will be switched off: {names}"
dest.delete.also_files        = "Also delete the repository files at {path}"
dest.delete.also_files_warn   = "Every snapshot in this repository would be gone. There is no undo."
dest.delete.confirm_name      = "Type {name} to confirm"
dest.delete.button            = "Remove destination"
dest.delete.button_files      = "Delete destination and its files"
dest.delete.s3_note           = "Objects in a bucket are not deleted from here. Remove them with your provider's tools if you want the space back."
dest.delete.copy_prefix       = "Copy the prefix"
```

### Encryption panel

```
enc.title                     = "Encryption"
enc.lead                      = "These settings are fixed when the repository is created and cannot be changed afterwards."
enc.summary                   = "Recommended settings — AES-256-GCM, BLAKE2B-256, dynamic 4 MB blocks, no error correction."
enc.change                    = "Change…"

enc.algorithm                 = "Encryption"
enc.hash                      = "Hash"
enc.splitter                  = "Block splitter"
enc.recommended               = "Recommended"

enc.hash.blake2b256           = "Default. Fast and well studied."
enc.hash.blake2b256128        = "Half-length hashes. Slightly smaller indexes, slightly higher chance of a collision."
enc.hash.blake3256            = "Fastest on modern processors. Newer than the others."
enc.hash.blake2s256           = "Tuned for 32-bit processors."
enc.hash.hmacsha256           = "Widely audited. Slower than BLAKE2."
enc.hash.hmacsha256128        = "Half-length variant of HMAC-SHA256."

enc.splitter.body             = "How files are cut into blocks before they are stored. Smaller blocks deduplicate small files better and make the index larger."
enc.splitter.suggest          = "Your folders hold a lot of small files. DYNAMIC-2M-BUZHASH deduplicates them better."
enc.splitter.suggest_action   = "Use it"

enc.ecc                       = "Add error-correcting data"
enc.ecc.body                  = "Stores extra data so a repository survives a limited amount of corruption. It costs the overhead you choose in extra storage, and it does nothing about a whole disk failing. Most worth having on optical or archival media."
enc.ecc.overhead              = "Overhead"
enc.ecc.algorithm             = "Reed-Solomon with CRC32"

enc.pass.title                = "Repository encryption key"
enc.pass.generated            = "Generate one for me"
enc.pass.generated_body       = "superbackup generates 256 random bits and keeps them in your vault. You are shown the passphrase once and asked to save it."
enc.pass.supplied             = "I will choose it"
enc.pass.supplied_body        = "Use this if you also open this repository with the kopia command line."
enc.pass.derived              = "Work it out from my master passphrase"
enc.pass.derived_body         = "Nothing extra to store. If you lose your master passphrase, this repository is lost with it."

enc.create                    = "Create repository"
enc.create.step_check         = "Checking the location"
enc.create.step_create        = "Creating the repository"
enc.create.step_store         = "Storing the passphrase in your vault"
enc.create.step_policy        = "Applying the retention policy"
enc.create.step_manifest      = "Writing the machine record"
enc.create.manifest_body      = "A small folder called _superbackup is written alongside the data, so anyone browsing this drive later can tell which computer each backup belongs to."
enc.create.failed             = "The repository was not created."
enc.create.partial            = "Some files were written at {path} before this failed. Check the folder before trying again."
enc.create.change             = "Change settings"
```

### Write this down

```
repo.writedown.title          = "Write this down now"
repo.writedown.body           = """
This passphrase opens the repository at {location}. It is stored in your vault, so you will not normally be asked for it.

You will need it if you ever restore on a different computer, or if your vault is lost.
"""
repo.writedown.grouping       = "The passphrase is shown in groups only to make it easier to copy. The spaces are not part of it."
repo.writedown.copy           = "Copy"
repo.writedown.copied         = "Copied. The clipboard will be cleared in 60 seconds."
repo.writedown.save           = "Save to a file…"
repo.writedown.save_note      = "The file is plain text. Treat it the way you would treat the passphrase."
repo.writedown.print          = "Print…"
repo.writedown.ack            = "I have saved this passphrase somewhere safe."
repo.writedown.escape         = "If you skip this, the passphrase can still be exported later from Settings › Security, using your master passphrase."
repo.pass.cannot_show         = "It cannot be shown again."
repo.pass.stored              = "Generated, stored in your vault"
repo.pass.derived             = "Worked out from your master passphrase"
repo.pass.supplied            = "Chosen by you, stored in your vault"
```

---

## 8. Storage providers

```
prov.title                    = "Storage providers"
prov.new                      = "Add a storage provider"
prov.search                   = "Search providers"
prov.used_by                  = "{count} destinations"
prov.used_by_none             = "Not used yet"
prov.no_tls                   = "This provider is set to plain HTTP."

prov.name                     = "Name"
prov.name.placeholder         = "StorJ eu-1 (personal)"
prov.notes                    = "Notes"
prov.notes.body               = "What this account is for, so it still makes sense in a year."
prov.type                     = "Provider type"
prov.type.filled              = "Endpoint and region filled in for {flavour}. Change them if your account differs."
prov.endpoint                 = "Endpoint"
prov.endpoint.parsed          = "{scheme}://{host} — TLS {tls_state}, port {port}"
prov.region                   = "Region"
prov.region.required          = "Required for this provider."
prov.region.optional          = "Optional for this provider."
prov.tls                      = "Use TLS"
prov.tls.off_warning          = "Without TLS, your keys and your data travel unencrypted. Reasonable only for a server on this machine or your own network."
prov.path_style               = "Path-style addressing"
prov.path_style.body          = "Required by MinIO and some gateways. StorJ and Amazon S3 accept the default."
prov.path_style.from_flavour  = "Set automatically for {flavour}."

prov.creds.title              = "Credentials"
prov.access_key               = "Access key ID"
prov.secret_key               = "Secret access key"
prov.session_token            = "Session token"
prov.use_session_token        = "Use a session token"
prov.session_body             = "For temporary credentials issued by an identity service."
prov.creds.stored             = "Stored in your vault. Leave blank to keep it."
prov.creds.replace            = "Replace…"
prov.creds.footnote           = "Stored in your encrypted vault and handed to kopia through the environment, never on a command line."

prov.save                     = "Save provider"
prov.save_untested.title      = "Save without testing?"
prov.save_untested.body       = "Testing takes a few seconds and catches a wrong key before a backup does, at two in the morning."
prov.save_untested.test       = "Test first"
prov.save_untested.save       = "Save anyway"

prov.test.resolving           = "Resolving the endpoint"
prov.test.tls                 = "Negotiating TLS"
prov.test.signing             = "Signing a request"
prov.test.listing             = "Listing buckets"
prov.test.ok                  = "Connected. Found {count} buckets."
prov.test.ok_none             = "Connected. This account has no buckets yet."
prov.test.show_buckets        = "Show buckets"
prov.test.more_buckets        = "and {count} more"

prov.err.dns                  = "That endpoint could not be found. Check the address for a typo."
prov.err.tls                  = "The secure connection could not be established. {reason}"
prov.err.tls_action           = "Turn off TLS"
prov.err.auth                 = "The endpoint answered, but rejected these credentials."
prov.err.auth_action          = "Check the keys"
prov.err.no_list              = "These credentials work, but they are not allowed to list buckets. You can still use a bucket by typing its name."
prov.err.no_list_action       = "Continue anyway"
prov.err.timeout              = "The endpoint did not answer within 15 seconds."
prov.err.addressing           = "The endpoint answered but did not recognise the bucket path. Some gateways need path-style addressing."
prov.err.addressing_action    = "Turn on path-style addressing"
prov.err.clock                = "The endpoint rejected the request because this computer's clock is {skew} out. Signatures depend on the time being right."
prov.err.copy_diag            = "Copy diagnostic details"
prov.err.diag_note            = "Your keys are removed from the copied text."

prov.impact                   = "Used by {destinations} destinations across {jobs} jobs."
prov.impact.show              = "Show them"
prov.impact.unaffected        = "Not affected — these use their own key pair"

prov.rotate.title             = "Rotate the keys on {name}"
prov.rotate.lead              = "superbackup cannot create keys for you. Create a new key pair in your provider's console, enter it here, and it will be checked against every destination before anything is replaced."
prov.rotate.old_valid         = "Your old key keeps working until you revoke it yourself."
prov.rotate.new_creds         = "New credentials"
prov.rotate.verify            = "Verify against all destinations"
prov.rotate.verifying         = "Checking {name}…"
prov.rotate.pass              = "Reachable with the new key"
prov.rotate.fail              = "Not reachable with the new key: {reason}"
prov.rotate.blocked           = "Fix the failures above, or continue and accept that these destinations will fail on their next run."
prov.rotate.continue_anyway   = "Continue anyway"
prov.rotate.done.title        = "Keys replaced"
prov.rotate.done.body         = "Jobs will use the new key from their next run."
prov.rotate.done.revoke       = "Revoke this key in your provider's console when you are ready: {key_id}"
prov.rotate.atomic_fail       = "The vault could not be updated, so nothing was changed. Your old keys are still in place."

prov.delete.title             = "Delete {name}?"
prov.delete.body              = "The stored keys are removed from your vault. Nothing at the provider is changed."
prov.delete.in_use            = "{count} destinations use this provider. Remove or move them first."
prov.delete.goto              = "Go to destinations"
```

---

## 9. Activity

```
activity.title                = "Activity"
activity.tab.runs             = "Runs"
activity.tab.events           = "Events"
activity.search               = "Search activity"
activity.range.24h            = "Last 24 hours"
activity.range.7d             = "Last 7 days"
activity.range.30d            = "Last 30 days"
activity.range.all            = "All (200 runs)"
activity.history_note         = "superbackup keeps the last 200 runs. Older activity is in the event log."
activity.export               = "Export…"
activity.export.runs          = "Runs as CSV"
activity.export.events        = "Events as NDJSON"
activity.export.bundle        = "Diagnostic bundle…"
activity.export.note          = "Anything that looks like a credential is removed before the file is written."
activity.filter.job           = "Job: {name}"
activity.filter.status        = "Status: {status}"
activity.filter.destination   = "Destination: {name}"
activity.only_this_job        = "Show only this job"
activity.severity             = "Severity"
activity.severity.all         = "All"
activity.severity.info        = "Info and above"
activity.severity.warn        = "Warnings and errors"
activity.severity.error       = "Errors only"
activity.debug_note           = "Debug events are only recorded while the log level is Debug or Trace."
activity.dest_summary         = "{succeeded} of {total} succeeded"

run.detail.title              = "{job} — {started}"
run.detail.status             = "Status"
run.detail.partial            = "Some destinations did not complete. See below."
run.detail.trigger            = "Started by"
run.detail.duration           = "Duration"
run.detail.destinations       = "Destinations"
run.detail.started            = "Started"
run.detail.finished           = "Finished"
run.detail.run_id             = "Run id"
run.detail.job_id             = "Job id"
run.detail.snapshot           = "Snapshot"
run.detail.no_snapshot        = "No snapshot was created"
run.detail.browse             = "Browse this snapshot"
run.detail.files              = "{processed} processed · {cached} unchanged · {skipped} skipped"
run.detail.data               = "{read} read · {uploaded} uploaded"
run.detail.throughput         = "{rate}/s average"
run.detail.warnings           = "{count} warnings"
run.detail.error_code         = "Error code: {code} · {time}"
run.detail.redacted           = "Anything that looked like a credential has been removed from this output."
run.detail.retry              = "Retry this job"
run.detail.copy_summary       = "Copy run summary"

run.stop.title                = "Stop {job}?"
run.stop.body                 = "The partial snapshot is discarded, and the next run starts from where the last successful one left off."
run.stop.button               = "Stop backup"
run.stop_all.title            = "Stop {count} running backups?"
run.stop_all.body             = "These will be stopped: {names}. Partial snapshots are discarded."
run.stop_all.button           = "Stop all backups"
run.stopped_toast             = "{job} stopped. The partial snapshot was discarded."
```

---

## 10. Restore

```
restore.title                 = "Restore"
restore.sources               = "Restore from"
restore.snapshot_count        = "{count} snapshots"
restore.newest                = "newest {relative}"
restore.mirrors_group         = "Folder mirrors"
restore.mirrors_note          = "Open these in your file manager — there is nothing to restore from."
restore.retention_note        = "Retention keeps {latest} latest, {hourly} hourly, {daily} daily, {weekly} weekly, {monthly} monthly and {annual} annual snapshots."
restore.compare               = "Compare with previous"
restore.compare.result        = "{added} added · {changed} changed · {removed} removed"

restore.browse.filter         = "Filter files"
restore.browse.hidden         = "Show hidden files"
restore.browse.selected       = "{count} items selected · about {size}"
restore.browse.show_selection = "Show selection"
restore.browse.clear          = "Clear"
restore.browse.reading        = "Reading directory…"
restore.browse.snapshot       = "Snapshot"
restore.browse.moved_up       = "That folder does not exist in this snapshot. Showing {path} instead."
restore.browse.restore_n      = "Restore {count} items"
restore.browse.restore_one    = "Restore 1 item"
restore.browse.restore_this   = "Restore this…"
restore.browse.restore_to     = "Restore this to…"
restore.browse.copy_path      = "Copy path"
restore.browse.previous       = "Show in previous snapshot"

restore.options.title         = "Restore {count} items"
restore.options.what          = "{count} items · about {size} · from {snapshot}"
restore.options.where         = "Where should these go?"
restore.options.original      = "Back to the original location"
restore.options.elsewhere     = "To another folder"
restore.options.structure     = "Recreate the full folder structure"
restore.options.flat_warn     = "Without the folder structure, files with the same name from different folders will overwrite each other."
restore.options.conflict      = "If a file already exists there"
restore.options.skip          = "Skip it"
restore.options.skip_body     = "Leaves what is on disk untouched."
restore.options.overwrite     = "Overwrite it"
restore.options.overwrite_body = "Replaces the file on disk. This cannot be undone."
restore.options.keep_both     = "Keep both"
restore.options.keep_both_body = "Restores as “name (restored 12 Mar 14:02).ext”."
restore.options.also          = "Also restore"
restore.options.timestamps    = "File timestamps"
restore.options.permissions   = "Permissions and ownership"
restore.options.perms_windows = "Not restored on Windows, where these do not carry across usefully."
restore.options.free_space    = "{free} free at the destination."
restore.options.not_enough    = "There is not enough room: {needed} needed, {free} free."
restore.options.type_confirm  = "Type overwrite to confirm"
restore.options.button        = "Restore"
restore.options.button_danger = "Overwrite and restore"

restore.progress.counts       = "{files_done} of {files_total} files · {bytes_done} of {bytes_total} · {rate}/s"
restore.progress.current      = "Restoring {path}"
restore.progress.cancel       = "Cancel restore"
restore.cancel.title          = "Cancel this restore?"
restore.cancel.body           = "Files already written stay where they are. Nothing is put back."
restore.cancel.button         = "Cancel restore"

restore.done.title            = "Restore finished"
restore.done.body             = "Restored {count} items ({size}) to {path}"
restore.partial.title         = "Restore finished with problems"
restore.partial.body          = "Restored {done} of {total} items. The rest are listed below with the reason."
restore.partial.retry         = "Retry failed items"
restore.partial.copy          = "Copy the list"
restore.failed.title          = "Restore failed"
```

---

## 11. Settings

```
settings.title                = "Settings"
settings.saved                = "Saved"

settings.section.general      = "General"
settings.section.scheduling   = "Scheduling"
settings.section.bandwidth    = "Bandwidth"
settings.section.notifications = "Notifications"
settings.section.security     = "Security"
settings.section.kopia        = "Kopia binary"
settings.section.remote       = "Remote configuration"
settings.section.advanced     = "Advanced"
settings.section.reset        = "Reset"
```

### General

```
set.machine_label             = "Machine label"
set.machine_slug              = "Folder name: {slug}"
set.machine_slug_note         = "The folder name is fixed for this install and does not change when you rename the machine."
set.machine_id                = "Machine id"
set.hostname                  = "Host name"
set.os                        = "Operating system"
set.arch                      = "Architecture"
set.user                      = "User"
set.first_setup               = "First set up"
set.theme                     = "Theme"
set.theme.system              = "System"
set.theme.light               = "Light"
set.theme.dark                = "Dark"
set.autostart                 = "Start superbackup when I sign in"
set.start_minimised           = "Start minimised to the tray"
set.service                   = "Run backups as a background service"
set.service.installed_running = "Service: installed and running"
set.service.installed_stopped = "Service: installed, not running"
set.service.not_installed     = "Service: not installed"
set.service.install           = "Install"
set.service.start             = "Start"
set.service.uninstall         = "Uninstall"
set.parallel                  = "Maximum jobs running at once"
set.parallel.body             = "Kopia already uses many threads inside one backup. More than two at a time rarely helps and can make everything slower."
set.quit                      = "Quit superbackup"
set.quit.body                 = "Scheduled backups stop until superbackup is started again."
```

### Scheduling

```
set.catchup                   = "Run schedules that were missed while the computer was off"
set.catchup.body              = "Missed runs start shortly after superbackup does, and are recorded as catch-up runs."
set.metered                   = "Skip scheduled runs on a metered connection"
set.metered.body              = "Skipped runs are recorded as skipped, not failed. Individual jobs can override this."
set.battery                   = "Skip scheduled runs on battery"

set.pause.title               = "Pause backups"
set.pause.body                = "Pausing stops schedules. Backups you start yourself still run."
set.pause.1h                  = "1 hour"
set.pause.2h                  = "2 hours"
set.pause.4h                  = "4 hours"
set.pause.8h                  = "8 hours"
set.pause.forever             = "Until I resume"
set.pause.reason              = "Reason"
set.pause.reason_placeholder  = "On the road until Friday"
set.pause.active              = "Paused until {time}"
set.pause.active_forever      = "Paused until you resume"
set.pause.resume              = "Resume now"
set.pause.extend              = "Extend by 1 hour"

set.upcoming                  = "Upcoming runs"
set.upcoming.blocked_by       = "Blocked by"
set.upcoming.blocked.paused   = "Paused"
set.upcoming.blocked.locked   = "Vault locked"
set.upcoming.blocked.disabled = "Job disabled"
set.upcoming.none             = "Nothing is scheduled."
```

### Bandwidth

```
set.bw.upload                 = "Upload limit"
set.bw.download               = "Download limit"
set.bw.unit                   = "kB/s"
set.bw.approx                 = "≈ {mbits} Mbit/s"
set.bw.download_body          = "Downloads happen during restores and repository maintenance."
set.bw.window                 = "Use a different limit during part of the day"
set.bw.from                   = "From"
set.bw.to                     = "To"
set.bw.days                   = "Days"
set.bw.days_none              = "No days chosen, so the window applies every day."
set.bw.wraps                  = "This window runs past midnight into the next day."
set.bw.summary                = "Between {start} and {end} on {days}, uploads are limited to {window_up}. Outside that window, uploads are limited to {base_up}."
set.bw.per_destination        = "Limits are applied per destination, so two destinations running at once can each use the full limit."
```

### Notifications

```
set.notif.enabled             = "Show desktop notifications"
set.notif.on_failure          = "When a backup fails"
set.notif.on_success          = "When a backup succeeds"
set.notif.on_success_body     = "Most people prefer silence when everything works."
set.notif.stale               = "When a job has not succeeded for"
set.notif.stale_unit          = "days"
set.notif.stale_body          = "Set to 0 to turn this off, here and on the dashboard."
set.notif.service             = "When the background service has a problem"
set.notif.dedupe              = "Do not repeat the same problem within"
set.notif.dedupe_unit         = "minutes"
set.notif.test                = "Send a test notification"
set.notif.test_body           = "A test notification was sent. If nothing appeared, notifications may be switched off for superbackup in your system settings."
set.notif.blocked             = "Your system is not showing notifications from superbackup."
set.notif.blocked_action      = "Open system settings"
```

### Security

```
set.sec.vault                 = "Vault"
set.sec.autolock              = "Lock automatically after"
set.sec.autolock_unit         = "minutes"
set.sec.autolock_body         = "Set to 0 to lock as soon as the window is closed. Auto-lock never happens while a backup is running."
set.sec.autolock_conflict     = "With auto-lock set to 0 and the credential store switched off, no scheduled backup will ever run without you typing the passphrase first."
set.sec.keychain              = "Store the vault key in {keychain_name}"
set.sec.keychain.on_title     = "Store the vault key in {keychain_name}?"
set.sec.keychain.on_body      = "Unattended backups stop needing a person to type the passphrase. In exchange, anything that can run programs as you can ask the credential store for the key."
set.sec.keychain.confirm      = "Enter your master passphrase to confirm"
set.sec.keychain.off          = "The stored key has been removed from {keychain_name}."

set.sec.change                = "Change master passphrase…"
set.sec.change.current        = "Current passphrase"
set.sec.change.new            = "New passphrase"
set.sec.change.confirm        = "Confirm new passphrase"
set.sec.change.done_title     = "Master passphrase changed"
set.sec.change.done_body      = "The vault has been re-encrypted and a backup of the old one saved. Repository encryption keys worked out from the master passphrase have been recalculated, so no repository needs creating again."

set.sec.export                = "Export repository encryption keys…"
set.sec.export.title          = "Export repository encryption keys"
set.sec.export.body           = "This writes every repository encryption key to a plain text file you choose. The file is not encrypted. Treat it exactly as you would treat the passphrases."
set.sec.export.confirm        = "Enter your master passphrase to continue"
set.sec.export.button         = "Choose a file and export"

set.sec.backups               = "Vault backups"
set.sec.backups.body          = "A copy is written here before every change to the vault."
set.sec.backups.restore       = "Restore a backup…"
set.sec.backups.restore_title = "Restore {file}?"
set.sec.backups.restore_body  = "The current vault will be replaced by this backup. Any credential added since {date} will be gone."
set.sec.backups.confirm       = "Type {file} to confirm"

set.sec.reset                 = "Reset the vault and start over"
set.sec.reset.title           = "Reset the vault?"
set.sec.reset.body            = "Every stored secret is destroyed: repository encryption keys, storage keys and tokens. Repositories whose passphrase was generated or worked out from your master passphrase cannot be opened again unless you have exported the passphrase."
set.sec.reset.affected        = "These repositories would become unreadable: {names}"
set.sec.reset.confirm         = "Type superbackup to confirm"
set.sec.reset.button          = "Reset the vault"
```

### Kopia binary

```
set.kopia.found               = "Kopia {version}"
set.kopia.auto                = "Find it automatically"
set.kopia.specific            = "Use a specific file"
set.kopia.check               = "Check again"
set.kopia.download            = "Download a tested build"
set.kopia.download_body       = "Version {version}, verified against SHA-256 {hash}, from {url}. It is placed in superbackup's own folder and no installer is run."
set.kopia.folders             = "superbackup keeps its own kopia configuration and cache, separate from any kopia you run yourself, so the two never fight over repository.config."
set.kopia.untested            = "This is Kopia {found}. superbackup is tested with {tested}. It will still try."
```

### Remote configuration

```
set.remote.lead               = "Several machines can share one configuration through a Git repository. The file kept in the repository is the sealed vault; the plain config.json is never pushed, and the vault is only opened in memory after you supply your master passphrase."
set.remote.enabled            = "Sync configuration from a Git repository"
set.remote.url                = "Repository URL"
set.remote.branch             = "Branch"
set.remote.path               = "File path in the repository"
set.remote.auth               = "Authentication"
set.remote.auth.none          = "None — public repository, or credentials your system git already has"
set.remote.auth.token         = "Personal access token"
set.remote.auth.token_scope   = "Read access to the repository is enough, unless you also publish from this machine."
set.remote.auth.ssh           = "SSH key"
set.remote.auth.ssh_body      = "The key is read from where it is. It is not copied into the vault."
set.remote.auto_pull          = "Check for changes automatically"
set.remote.interval           = "every"
set.remote.interval_unit      = "minutes"
set.remote.allow_push         = "Allow publishing from this machine"
set.remote.allow_push_body    = "Nothing is ever pushed automatically. Publishing is always something you do on purpose."
set.remote.signers            = "Trusted signers"
set.remote.signers.add        = "Add fingerprint…"
set.remote.signers.body       = "When this list is not empty, a pulled vault whose signature does not match one of these is rejected."
set.remote.signers.empty      = "With no fingerprints listed, any vault found at that address will be accepted."
set.remote.last_pull          = "Last pulled {relative}"
set.remote.commit             = "Commit {short}"
set.remote.up_to_date         = "Up to date"
set.remote.changes            = "{count} changes available"
set.remote.never              = "Never pulled"
set.remote.pull               = "Pull now"
set.remote.publish            = "Publish…"
set.remote.open               = "Open the repository"

set.remote.pull.fetching      = "Fetching…"
set.remote.pull.verifying     = "Checking the signature…"
set.remote.pull.signed_ok     = "Signed by {signer}, which is on your trusted list."
set.remote.pull.unsigned      = "This vault is not signed."
set.remote.pull.untrusted     = "This vault is signed by {signer}, which is not on your trusted list. It was not applied."
set.remote.pull.commit_info   = "{short} by {author}, {relative}"
set.remote.pull.review        = "Review changes"
set.remote.pull.added         = "Added"
set.remote.pull.changed       = "Changed"
set.remote.pull.removed       = "Removed"
set.remote.pull.no_changes    = "Nothing has changed since the last pull."
set.remote.pull.removes_used  = "This would remove {names}, which are still in use here."
set.remote.pull.local_changes = "Your local changes to {names} will be replaced."
set.remote.pull.save_copy     = "Save a copy of my configuration first"
set.remote.pull.apply         = "Apply changes"
set.remote.pull.keeps_local   = "Your run history and job state stay on this machine. Only configuration is replaced."
set.remote.pull.done          = "Configuration updated to {short}."
```

### Advanced and reset

```
set.adv.log_level             = "Log level"
set.adv.log_level_body        = "Debug and Trace write a lot, and can include the paths of files being backed up."
set.adv.log_days              = "Keep logs for"
set.adv.log_days_unit         = "days"
set.adv.locations             = "File locations"
set.adv.clear_cache           = "Clear the kopia cache"
set.adv.cache_size            = "Currently {size}."
set.adv.cache_body            = "The next run will be slower while the cache is rebuilt."
set.adv.bundle                = "Export a diagnostic bundle…"
set.adv.bundle.includes       = "Included: your configuration with every secret removed, the last 200 runs, the tail of the event log, the kopia version, your operating system details, and the last 2,000 log lines."
set.adv.bundle.excludes       = "Not included: any passphrase, key or token, the contents of any file you back up, and the names of files inside your folders."
set.adv.bundle.preview        = "Preview the bundle"
set.adv.bundle.write          = "Save the bundle…"

set.adv.doctor                = "Run diagnostics"
doctor.kopia                  = "Kopia is present and runs"
doctor.vault                  = "The vault can be read"
doctor.schema                 = "The configuration format is understood"
doctor.destinations           = "Every destination is reachable"
doctor.providers              = "Every provider has been verified"
doctor.space                  = "There is room at each local destination"
doctor.service                = "The background service is in the state you asked for"
doctor.ipc                    = "The daemon can be reached"
doctor.clock                  = "This computer's clock matches the storage endpoint"
doctor.fix                    = "Fix"
doctor.pass                   = "Passed"
doctor.warn                   = "Worth a look"
doctor.fail                   = "Needs fixing"

set.reset.settings            = "Reset all settings to their defaults"
set.reset.settings_body       = "Jobs, destinations, providers and the vault are left alone."
set.reset.all                 = "Remove all configuration and start over"
set.reset.all_body            = "Deletes every job, destination, provider and stored secret on this machine. Data already written to your destinations is not touched, and can be reached again by connecting to the repositories with their passphrases."
set.reset.all_confirm         = "Type superbackup to confirm"
```

---

## 12. About

```
about.tagline                 = "Backups for machines full of code."
about.version                 = "superbackup {version}"
about.build                   = "{os}-{arch} · built {date}"
about.kopia                   = "Kopia"
about.kopia_missing           = "Not found"
about.machine                 = "Machine"
about.schema                  = "Configuration format"
about.data_folder             = "Data folder"

about.licences                = "Licences"
about.licence.self            = "superbackup is released under the MIT licence."
about.licence.self_view       = "View licence"
about.licence.kopia           = "superbackup uses Kopia, which is released under the Apache Licence 2.0. Kopia is a separate program: superbackup runs it and does not modify it."
about.licence.kopia_view      = "View the Apache 2.0 licence"
about.licence.kopia_site      = "kopia.io"
about.licence.third_party     = "Third-party licences"
about.licence.fonts           = "Inter and JetBrains Mono are used under the SIL Open Font Licence 1.1. Icons are from Lucide, under the ISC licence."
about.licence.copy_all        = "Copy all licence text"

about.link.website            = "Website"
about.link.docs               = "Documentation"
about.link.issue              = "Report an issue"
about.link.kopia_docs         = "Kopia documentation"
about.link.releases           = "Release notes"
about.copyright               = "© 2026 Andreas Wiren"
```

---

## 13. Tray

```
tray.header.idle              = "superbackup — Up to date"
tray.header.running           = "superbackup — Backing up"
tray.header.attention         = "superbackup — Needs attention"
tray.header.paused            = "superbackup — Paused"
tray.header.failed            = "superbackup — Backup failed"
tray.header.not_running       = "superbackup — Not running"

tray.sub.last                 = "Last backup {relative}"
tray.sub.never                = "No backups yet"
tray.sub.next                 = "Next run {relative}"
tray.sub.running_one          = "{job} — {percent}% · {rate}/s · ~{eta} left"
tray.sub.running_many         = "{count} backups running — {percent}%"
tray.sub.paused_until         = "Paused until {time}"
tray.sub.paused_forever       = "Paused until you resume"
tray.sub.locked               = "The vault is locked"
tray.sub.stale                = "{job} has not succeeded for {days} days"
tray.sub.failed               = "{job} failed {relative}"
tray.sub.kopia_missing        = "Kopia was not found"

tray.back_up_now              = "Back up now"
tray.back_up                  = "Back up"
tray.stop_job                 = "Stop “{job}”"
tray.stop_job_pct             = "Stop “{job}” ({percent}%)"
tray.stop_all                 = "Stop all backups"
tray.pause                    = "Pause"
tray.pause_note               = "Current backups finish first"
tray.resume                   = "Resume backups"
tray.extend                   = "Extend"
tray.disable_all              = "Disable all jobs"
tray.unlock                   = "Unlock…"
tray.view_error               = "View the error…"
tray.fix_kopia                = "Fix in Settings…"
tray.start_service            = "Start the background service"
tray.open                     = "Open superbackup"
tray.activity                 = "Activity…"
tray.settings                 = "Settings…"
tray.quit                     = "Quit superbackup"

tray.suffix.running           = "(running)"
tray.suffix.already_running   = "(already running)"
tray.suffix.locked            = "(vault locked)"
tray.suffix.kopia_missing     = "(kopia not found)"

tray.quit_running.title       = "Quit and stop {count} backup?"
tray.quit_running.body        = "These are running: {names}. Their partial snapshots are discarded."
tray.quit_running.keep        = "Keep running"
tray.quit_running.quit        = "Quit and stop"

tray.first_hide               = "superbackup is still running in the tray. Use Quit in the tray menu to stop it completely."
```

---

## 14. Notifications

Titles are limited to 60 characters and bodies to 160, which is what the
narrowest supported platform shows without truncating.

```
notif.failed.title            = "Backup failed: {job}"
notif.failed.body             = "{message}"
notif.failed.action_retry     = "Retry"
notif.failed.action_details   = "Show details"

notif.partial.title           = "Backup finished with problems: {job}"
notif.partial.body            = "Succeeded to {ok} of {total} destinations. {failed_name} failed."

notif.success.title           = "Backup finished: {job}"
notif.success.body            = "{files} files · {bytes} uploaded · {duration}"

notif.recovered.title         = "Backup recovered: {job}"
notif.recovered.body          = "This job is working again after {count} failures."

notif.stale.title             = "{job} has not backed up for {days} days"
notif.stale.body              = "Last success {date}."
notif.stale.action            = "Back up now"

notif.service.title           = "superbackup service problem"
notif.service.body            = "{message}"

notif.kopia.title             = "Kopia was not found"
notif.kopia.body              = "Backups cannot run until this is fixed."

notif.locked.title            = "A backup was skipped"
notif.locked.body             = "{job} was due at {time}. Unlock superbackup to run it."
notif.locked.action           = "Unlock"

notif.restore.title           = "Restore finished"
notif.restore.body            = "{count} items restored to {folder}"

notif.remote.title            = "Configuration changes are available"
notif.remote.body             = "{count} changes on {branch}."

notif.summary.title           = "{count} backups need attention"
notif.summary.body            = "Open superbackup to see what happened."

notif.test.title              = "superbackup notifications are working"
notif.test.body               = "This is what a notification from superbackup looks like."
```

---

## 15. Errors

Each entry is the message shown to the user. The technical text from
`Error::to_string()` is kept in the details disclosure, never in the headline.

```
err.config                    = "The configuration file could not be read. {detail}"
err.io                        = "{operation} failed. {os_message}"
err.io.path                   = "That path cannot be used: {reason}"
err.locked                    = "The vault is locked."
err.bad_passphrase            = "That passphrase did not work. Passphrases are case sensitive."
err.vault_corrupt             = "The vault file is damaged or has been altered. Restore config.sbvault from a backup rather than overwriting it."
err.vault_version             = "This vault was written by a newer version of superbackup (format {found}; this build understands up to {supported})."
err.vault_version.action      = "Get the newer version"
err.crypto                    = "A cryptographic operation failed. {detail}"
err.kopia                     = "Kopia stopped with an error."
err.kopia_missing             = "Kopia was not found. superbackup needs it to read and write backups."
err.kopia_missing.action      = "Fix in Settings"
err.repo_not_connected        = "superbackup is not connected to the repository at {location} yet."
err.repo_not_connected.action = "Connect to this repository…"
err.repo_exists               = "There is already a repository at {location}."
err.repo_exists.action        = "Connect instead"
err.schedule                  = "That schedule could not be understood: {detail}"
err.job_not_found             = "That job no longer exists."
err.job_running               = "That job is already running."
err.job_running.action        = "Show it"
err.job_cancelled             = "The run was cancelled."
err.ipc                       = "superbackup lost contact with its background process."
err.daemon_unreachable        = "The superbackup background process is not running. Schedules will not fire until it is."
err.daemon_unreachable.action = "Start the background service"
err.service                   = "The background service could not be controlled. {detail}"
err.service.action            = "Reinstall the service"
err.platform                  = "Something the operating system provides did not work: {detail}"
err.remote                    = "The shared configuration could not be reached. {detail}"
err.internal                  = "Something went wrong inside superbackup."
err.internal.action           = "Export a diagnostic bundle"

err.disk_full                 = "There is no room left at {location}. {free} free, {needed} needed."
err.path_missing              = "{path} could not be found."
err.path_missing.create       = "Create the folder"
err.permission                = "superbackup is not allowed to read {path}."
err.source_missing            = "{path} was not there when the backup ran, so it was skipped."
err.dest_offline              = "{name} could not be reached."
err.timeout                   = "The run was stopped after {minutes} minutes."
err.hook_failed               = "The {phase} command exited with status {status}."
err.no_space_restore          = "There is not enough room to restore: {needed} needed, {free} free at {path}."
```

---

## 16. Validation messages

```
valid.job.name.empty          = "Give the job a name."
valid.job.name.long           = "Names can be up to 64 characters."
valid.job.name.dup            = "There is already a job called {name}."
valid.source.none             = "Add at least one folder."
valid.source.relative         = "Use a full path, starting from the drive or the root."
valid.source.dup              = "That folder is already in this job."
valid.source.nested           = "That folder is already covered by {parent}."
valid.source.in_destination   = "That folder is inside {destination}, so the backup would contain itself."
valid.schedule.interval       = "Choose between 1 minute and 7 days."
valid.schedule.times          = "Add at least one time. Up to 24 are allowed."
valid.schedule.times_dup      = "That time is already in the list."
valid.schedule.weekdays       = "Choose at least one day."
valid.schedule.cron           = "{parse_error}"
valid.schedule.debounce       = "Choose between 5 seconds and 1 hour."
valid.schedule.min_interval   = "Choose between 1 minute and 24 hours."
valid.timeout                 = "Choose between 1 minute and 24 hours."
valid.pattern.empty           = "Line {line} is empty."
valid.pattern.invalid         = "Line {line} could not be read as a pattern: {reason}"
valid.pattern.absolute        = "Line {line} looks like a full path. Patterns are matched relative to each folder you back up."
valid.max_file_size           = "Choose between 1 MB and 1,048,576 MB."
valid.maintenance             = "Choose between 0 and 1,000. Zero means maintenance never runs on a schedule."
valid.dest.name.empty         = "Give the destination a name."
valid.dest.name.dup           = "There is already a destination called {name}."
valid.dest.path.relative      = "Use a full path."
valid.dest.path.parent        = "{parent} does not exist, so this folder cannot be created."
valid.dest.path.inside_dest   = "That path is inside {other}. Two destinations cannot share a folder."
valid.dest.path.inside_source = "That path is inside {source}, which this job backs up. The backup would contain itself."
valid.bucket                  = "Bucket names are 3 to 63 characters, using lowercase letters, digits, dots and hyphens."
valid.bucket.ip               = "A bucket name cannot look like an IP address."
valid.prefix                  = "Saved as {normalised}."
valid.provider.name.empty     = "Give the provider a name."
valid.provider.name.dup       = "There is already a provider called {name}."
valid.endpoint.empty          = "Enter the endpoint for this account."
valid.endpoint.invalid        = "That does not look like a host or a URL."
valid.endpoint.insecure       = "This endpoint is not on this machine or a private network, and TLS is switched off."
valid.region                  = "Amazon S3 needs a region, for example us-east-1."
valid.credentials             = "Enter both the access key ID and the secret access key."
valid.master.short            = "Use at least 12 characters."
valid.master.mismatch         = "The two passphrases are different."
valid.repo_pass.short         = "Use at least 12 characters."
valid.repo_pass.mismatch      = "The two passphrases are different."
valid.bandwidth               = "Choose between 1 and 10,000,000 kB/s."
valid.bw_window               = "The start and end times need to be different."
valid.remote.url              = "That does not look like a Git address."
valid.signer                  = "A fingerprint is 16 to 128 characters of hex or base64."
valid.autolock                = "Choose between 0 and 1,440 minutes."
valid.stale                   = "Choose between 0 and 90 days."
valid.dedupe                  = "Choose between 0 and 1,440 minutes."
valid.parallel                = "Choose between 1 and 8."
valid.logdays                 = "Choose between 1 and 365 days."

valid.form.problems           = "{count} problems to fix"
valid.form.problem            = "1 problem to fix"
```

### Cross-field warnings (not blocking)

```
warn.mirror_only              = "This job's only destination is a folder mirror, so there is no history to go back to and nothing is encrypted."
warn.same_drive               = "Every destination for this job is on {drive}. If that drive fails, all the copies go with it."
warn.onchange_large           = "This job watches {files} files. Watching a tree that size uses noticeable memory."
warn.recursive                = "{source} is inside {destination}, so each backup would include the previous one."
warn.autolock_service         = "With auto-lock at 0 minutes and the credential store off, no scheduled backup will run without you first typing the passphrase."
warn.unverified_dest          = "{name} has never been verified, and a job is scheduled to write to it."
warn.retention_no_maintenance = "Every retention value is zero, so nothing would be kept."
```

---

## 17. Toasts

```
toast.job.created             = "{name} created"
toast.job.saved               = "{name} saved"
toast.job.deleted             = "{name} deleted"
toast.job.enabled             = "{name} enabled"
toast.job.disabled            = "{name} disabled"
toast.jobs.disabled_all       = "All jobs disabled"
toast.jobs.enabled_all        = "Jobs re-enabled"
toast.dest.created            = "{name} created"
toast.dest.saved              = "{name} saved"
toast.dest.removed            = "{name} removed"
toast.prov.created            = "{name} created"
toast.prov.saved              = "{name} saved"
toast.prov.removed            = "{name} removed"
toast.run.started             = "Backing up {name}"
toast.run.finished            = "{name} finished — {bytes} uploaded in {duration}"
toast.run.warnings            = "{name} finished with {count} warnings"
toast.run.failed              = "{name} failed — {message}"
toast.run.queued              = "{name} is queued behind {other}"
toast.copied.clipboard        = "Copied to the clipboard"
toast.copied.cleared          = "The clipboard has been cleared"
toast.paused                  = "Backups paused until {time}"
toast.resumed                 = "Backups resumed"
toast.settings.saved          = "Saved"
toast.repo.created            = "Repository created at {location}"
toast.maintenance.done        = "Maintenance finished on {name}"
toast.export.done             = "Saved to {path}"
```

---

## 18. Accessibility strings

Announcement text for `WidgetInfo` and live regions. `{…}` is substituted the
same way as elsewhere.

```
a11y.rail.item                = "{label}, section {index} of {total}"
a11y.rail.selected            = "{label}, section {index} of {total}, current"
a11y.rail.attention           = "{label}, needs attention"

a11y.vault.unlocked           = "Vault unlocked, locks in {duration}. Activate for vault options."
a11y.vault.locked             = "Vault locked, scheduled backups are blocked. Activate to unlock."

a11y.health                   = "Overall health: {health}. {reason}"
a11y.job_card                 = "{name}, {status}, last run {last}, next run {next}, {count} destinations"
a11y.job_card.running         = "{name}, running, {percent} percent complete"
a11y.job_card.disabled        = "{name}, disabled, {status} {last}"

a11y.progress                 = "Backing up {job} to {destination}, {percent} percent, {done} of {total} files"
a11y.progress.estimating      = "Backing up {job} to {destination}, still working out how much there is"
a11y.progress.restore         = "Restoring, {percent} percent, {done} of {total} files"

a11y.strength                 = "Passphrase strength: {level}"
a11y.exclusion                = "{title}, {checked}, {count} patterns. {rationale}"
a11y.exclusion.risky          = "{title}, {checked}, {count} patterns. {rationale} This one may lose data."
a11y.destination_row          = "{name}, {kind}, {status}, {checked}"
a11y.table                    = "{name} table, {rows} rows, {columns} columns"
a11y.table.sorted             = "Sorted by {column}, {direction}"
a11y.row                      = "{cells}"
a11y.disabled_locked          = "{label}, unavailable while the vault is locked"
a11y.busy                     = "{label}, busy"
a11y.dirty_tab                = "{label}, has unsaved changes"
a11y.form_invalid             = "{label}, {count} problems to fix"
a11y.toast                    = "{title}. {body}"
a11y.breadcrumb               = "Location: {path}, {items} items"
a11y.passphrase_block         = "Repository encryption key. Focus this and use your screen reader's character-by-character reading to hear it."

a11y.tray.name                = "superbackup, {health}"
a11y.tray.description         = "{reason}"
```

---

## 19. Formatting templates

Referenced by `DESIGN_SYSTEM.md` §10 so that the same value never appears in two
shapes.

```
fmt.bytes.small               = "{value} {unit}"          # 842 MB
fmt.rate                      = "{value} {unit}/s"        # 18.2 MB/s
fmt.duration.ms               = "{n}s"
fmt.duration.m                = "{m}m {s}s"
fmt.duration.h                = "{h}h {m:02}m"
fmt.duration.d                = "{d}d {h}h"
fmt.rel.now                   = "just now"
fmt.rel.minutes               = "{n} minutes ago"
fmt.rel.hours                 = "{n} hours ago"
fmt.rel.yesterday             = "yesterday {time}"
fmt.rel.absolute              = "{day} {month} {time}"
fmt.future.minutes            = "in {n} minutes"
fmt.future.hours              = "in {n} hours"
fmt.future.tomorrow           = "tomorrow {time}"
fmt.future.weekday            = "{weekday} {time}"
fmt.count.files               = "{n} files"
fmt.count.jobs                = "{n} jobs"
fmt.count.one_job             = "1 job"
fmt.count.destinations        = "{n} destinations"
fmt.count.one_destination     = "1 destination"
fmt.percent                   = "{n}%"
fmt.ordinal.2                 = "2nd"
fmt.ordinal.3                 = "3rd"
fmt.ordinal.n                 = "{n}th"
```
