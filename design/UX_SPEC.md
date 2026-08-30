# superbackup — UX Specification

Version 1.0 · Master document
Companions: `DESIGN_SYSTEM.md` (tokens, components), `COPY.md` (every string),
`WIREFRAMES.md` (monospace layouts at 1100 × 720)

Authoritative domain model: `crates/core/src/model.rs` and
`crates/core/src/state.rs`. Every noun in this document is a type or field from
those files. Where this document names a screen concept that is not a domain
type (e.g. "the run detail sheet"), it is a view over domain types, never a new
entity.

---

## Table of contents

0. [Product frame](#0-product-frame)
1. [Information architecture](#1-information-architecture)
2. [Global chrome](#2-global-chrome)
3. [The locked vault](#3-the-locked-vault)
4. [Onboarding / first run](#4-onboarding--first-run)
5. [Dashboard](#5-dashboard)
6. [Jobs](#6-jobs)
7. [New job wizard](#7-new-job-wizard)
8. [Destinations](#8-destinations)
9. [Storage providers](#9-storage-providers)
10. [Activity](#10-activity)
11. [Restore](#11-restore)
12. [Settings](#12-settings)
13. [About](#13-about)
14. [Tray icon and menu](#14-tray-icon-and-menu)
15. [Notifications](#15-notifications)
16. [Empty, error and destructive states — consolidated](#16-empty-error-and-destructive-states--consolidated)
17. [Validation rules](#17-validation-rules)
18. [Accessibility summary](#18-accessibility-summary)

---

## 0. Product frame

**What it is.** A tray-resident backup manager for developer machines. It drives
the Kopia CLI. It exists because OneDrive collapses under millions of
`node_modules` and `.next/cache` files, and because developers do not want
everything on git.

**Who it is for.** One person, on their own machine, who understands
directories, schedules and buckets, and who does not want to learn Kopia's
command surface to get a reliable backup.

**Shape of the world** (from `model.rs`):

```
StorageProvider  (endpoint + region + credentials, e.g. "StorJ eu-1")
       |  1..n
Destination      (provider + bucket + prefix, or a local path)
       |  n..n
Job              (sources -> many destinations, schedule, exclusions)
       |  n..1
Project          (grouping only)
```

**The four things this UI must never get wrong**

1. A locked vault must be unmistakable, everywhere, because it silently stops
   every scheduled backup.
2. A job that succeeded to two destinations and failed to a third is **not** a
   success. `JobRun::derive_status()` already encodes this; the UI must never
   flatten it.
3. A generated repository passphrase that is not written down is unrecoverable
   data. That screen is the highest-stakes moment in the product.
4. Restore must be as good as backup. A backup tool with a weak restore is a
   file shredder with extra steps.

**Capability floor**: Rclone UI's breadth of remote and job management.
**Quality bar**: Arq 7 and Plakar — calm, dense, professional, no raw CLI flags
on the surface but every one of them reachable.

---

## 1. Information architecture

Eight rail items — seven navigation destinations plus About — in this fixed
order:

| # | Rail item | Icon | Shortcut | Contains |
|---|---|---|---|---|
| 1 | Dashboard | `layout-dashboard` | `Ctrl+1` | Health, next run, job cards, live progress |
| 2 | Jobs | `repeat` | `Ctrl+2` | Job table, job editor, new-job wizard |
| 3 | Destinations | `hard-drive` | `Ctrl+3` | Destination table, editors, repository creation |
| 4 | Storage providers | `key-round` | `Ctrl+4` | Provider table, editors, key rotation |
| 5 | Restore | `history` | `Ctrl+5` | Snapshot browse and restore |
| 6 | Activity | `list` | `Ctrl+6` | Run history, event log |
| — | *(spacer)* | | | |
| 7 | Settings | `settings` | `Ctrl+7` | Nine settings sections |
| 8 | About | `info` | — | Version, licences, links |

`Projects` are **not** a rail item. A `Project` is a grouping only, and it
appears as a group-by control and a filter on the Jobs screen, plus a coloured
2px stripe on job cards. Making it a navigation destination would imply it owns
something; it does not.

Screen IDs used throughout: `O-*` onboarding, `D-*` dashboard, `J-*` jobs,
`W-*` wizard, `T-*` destinations, `P-*` providers, `A-*` activity, `R-*`
restore, `S-*` settings, `AB-*` about, `V-*` vault modals.

---

## 2. Global chrome

### 2.1 Window

- Native OS decorations. No custom title bar (egui custom chrome loses OS snap,
  and this is a utility, not a showpiece).
- Default 1100 × 720. Minimum 900 × 600, enforced by
  `ViewportBuilder::with_min_inner_size`. Size and position are persisted.
- Closing the window **hides to tray**; it does not quit. The first time this
  happens, a toast explains it (`COPY.md` `tray.first_hide`). Quit is only in
  the tray menu and the Settings → General → "Quit superbackup" button.
- `Settings.start_minimised` starts with no window at all.
- The window title is `superbackup — <Health::title()>`; on a running job it is
  `superbackup — Backing up <job name> (42%)`. Windows taskbar progress
  (`ITaskbarList3`) mirrors the aggregate fraction; the taskbar button turns red
  on `Health::Failed`.

### 2.2 Left rail (208px; 64px collapsed below 1000px)

Top to bottom: 16px pad, the machine identity block, 12px gap, the eight rail
items, `remainder` spacer, a 1px divider, the vault state control, 12px pad.

**Machine identity block** (208px mode): 36px tall, `body.strong`
`MachineIdentity.label`, and under it `small` `text.muted` showing
`MachineIdentity.slug`. Clicking it opens Settings → General with the label
field focused. Collapsed mode: a 28 × 28 rounded square with the label's first
two initials, tooltip shows label and slug.

**Vault state control** — the single most important persistent control:

| `StatusSnapshot.unlocked` | Appearance | Click |
|---|---|---|
| `true` | 32px row, 16px `lock-open` in `success.mark`, label `Unlocked`, `small` sub-label `Locks in 27 min` (from `Settings.auto_lock_minutes`; omitted when 0) | Opens a menu: `Lock now (Ctrl+L)`, `Change master passphrase…`, `Auto-lock settings…` |
| `false` | 32px row, `bg` = `danger.tint.bg`, 16px `lock` in `danger.mark`, label `Locked`, `small` sub-label `Schedules are blocked` | Opens the Unlock modal `V-1` |

### 2.3 Header bar (56px)

- Left: screen title (`h1`). On sub-screens (job editor, destination editor, run
  detail) it becomes a breadcrumb: `Jobs / Dev code` where the first segment is
  a link (`text.link`) and the last is `h1` `text.primary`. A 30 × 30 ghost
  `arrow-left` back button sits 0px before the breadcrumb.
- Right: screen-specific actions, right-aligned, 8px apart. Primary action last
  (rightmost).
- The header bar is always present and never scrolls.

### 2.4 Global banners

Pinned under the header bar, above content, full content width, 16px below.
Order when several apply: **locked** → **paused** → **kopia missing** →
**daemon unreachable** → **service error** → **remote config drift**.
Maximum two are shown; further ones collapse into a single
`+N more issues` link that opens Settings → Diagnostics.

### 2.5 Status strip (28px, bottom)

Four segments separated by a 1px `border.subtle` vertical rule and 12px pads,
all `small` `text.muted`, left to right:

1. **Daemon**: a 6px dot + `Daemon running` / `Daemon not running` (from
   `service_running` / IPC reachability). Clicking opens Settings → Diagnostics.
2. **Service**: `Service installed` / `Running at login` / `Not installed`
   (from `service_installed` and `Settings.start_at_login`).
3. **Kopia**: `Kopia <kopia_version>` or `Kopia not found` in `warning.mark`.
   Clicking opens Settings → Kopia binary.
4. **Right-aligned**: the newest `Event` from `recent_events`, elided to the
   available width, prefixed by its severity dot. Clicking opens Activity
   scrolled to that event.

The strip is the only place `uptime_seconds` surfaces, in the daemon tooltip:
`Running for 3d 4h`.

### 2.6 Refresh model

The GUI subscribes to the daemon over IPC and repaints on push. `F5` forces a
`StatusSnapshot` refetch. When the daemon is unreachable the GUI shows the
`DaemonUnreachable` banner, keeps rendering the last snapshot dimmed to 60%
opacity, and disables every action that requires the daemon (run, stop, verify,
restore). Read-only editing of config is still allowed and is written on save,
because config lives on disk, not in the daemon.

---

## 3. The locked vault

Everything secret lives in an encrypted vault (`config.sbvault`) unlocked by a
master passphrase. `StatusSnapshot.unlocked == false` means every scheduled run
is blocked, which is why `derive_health` returns `Health::Attention` for it.

### 3.1 What locking does and does not block

| Blocked while locked | Allowed while locked |
|---|---|
| Running any job (manual or scheduled) | Viewing dashboard, jobs, destinations, providers, activity history |
| Verifying or connecting a destination | Editing job names, sources, schedules, exclusions, retention, hooks, timeouts |
| Creating a repository | Creating a job, if every destination it references already exists |
| Restoring | Editing all Settings except Security and Remote config |
| Testing a provider connection | Reading logs and exporting diagnostics |
| Rotating keys | Quitting, pausing, changing theme |
| Pulling remote config | |

**Rule**: locking never hides information the user already has on disk in
plaintext. It blocks anything that needs a `SecretRef` resolved.

### 3.2 Degradation, screen by screen

| Screen | Locked behaviour |
|---|---|
| Global | `locked` banner pinned under the header on **every** screen. Rail vault control shows `Locked` in danger tint. |
| Dashboard | Health tile shows `Needs attention` with reason `The vault is locked`. `Back up now` and every card `Run now` are disabled with tooltip `copy: locked.action_blocked`. Next-run tile shows the time struck through with the note `blocked while locked`. |
| Jobs list | Fully readable. `Run now` and `Run all` disabled. Editing allowed. |
| Job editor | All tabs editable. Save allowed. Destinations tab shows a lock icon on each destination chip's verify button. |
| Destinations | List readable, `last_verified_at` shown as usual. `Verify`, `Connect`, `Create repository`, `Browse snapshots` disabled. Add/edit allowed up to the point a secret must be written — the credential fields are replaced by a 44px inline unlock prompt (see 3.4). |
| Providers | Same: list readable, secrets never shown anyway, `Test connection` and `Rotate keys` disabled, credential fields replaced by inline unlock. |
| Restore | The whole screen is replaced by a centred unlock panel: 32px `lock`, title, body, `Unlock` primary button. No snapshot data is shown, because listing snapshots requires the repository passphrase. |
| Activity | Fully available. Historical run detail, errors and warnings are local state, not secrets. |
| Settings → Security | Readable. Changing the master passphrase, toggling OS keychain, and resetting the vault all require an unlock first and show the inline unlock prompt in place of their controls. |
| Settings → Remote config | Readable. `Pull now` and `Publish` disabled. |
| Tray | Icon = `Attention` (unless something worse applies). Menu gains `Unlock…` as the first item, and `Back up now` is disabled with the suffix `(vault locked)`. |

Disabled controls keep their position and size; they never disappear. Every
disabled control carries the tooltip `Unlock the vault to use this.` and its
AccessKit label is suffixed `, unavailable while the vault is locked`.

### 3.3 Unlock modal `V-1`

Small modal (420px), **blocking**: no `x`, Escape does not close it when the
user reached it by attempting a locked action. It *is* dismissible when opened
voluntarily from the rail.

Contents, top to bottom:
1. 24px `lock` in `accent`, title `Unlock superbackup` (`h2`).
2. Body `small` `text.secondary`: explains what unlocking enables.
3. Passphrase field (§8.2 of the design system), labelled `Master passphrase`,
   autofocused, 280px wide, reveal toggle present.
4. Checkbox `Remember until I sign out` — visible **only** when
   `Settings.use_os_keychain` is enabled; otherwise absent, not disabled.
5. Error area, reserved 20px height so the modal does not jump.
6. Footer: `Cancel` (ghost, omitted when blocking) and `Unlock` (primary).

Behaviour:
- Enter submits. The button shows the loading state while Argon2 runs; this is
  deliberately slow and the label changes to `Unlocking…`.
- `ErrorCode::BadPassphrase` → inline error `copy: vault.unlock.wrong`, field
  border turns danger, contents are **not** cleared (the user may have made one
  typo), text is selected so retyping replaces it.
- Three consecutive failures adds a `small` `text.muted` line with the recovery
  reality: `copy: vault.unlock.no_recovery`. There is no lockout timer — this is
  a local file, a timer would only punish the legitimate user.
- `ErrorCode::VaultCorrupt` → the modal switches to a danger banner with the
  message and the hint from `Error::hint()`, and offers
  `Open vault backups folder` (from `Paths::vault_backup_dir()`).
- `ErrorCode::VaultVersion` → danger banner naming both versions and a link to
  the release page.
- On success: modal closes, a success toast appears (`copy: vault.unlocked`),
  focus returns to the control that triggered the unlock, and if that control
  was an action (Run now, Verify), **the action is performed automatically**.
  This is the single most important detail of the flow — the user's intent is
  not thrown away by the interruption.

### 3.4 Inline unlock prompt

Used inside forms where only part of the screen needs secrets. A 44px-tall
`bg.raised` row with 1px `border.control`, radius 6: 16px `lock` in
`warning.mark`, `small` text `Unlock to enter credentials`, and a compact
`Unlock` secondary button. On success the row is replaced in place by the real
fields, with focus moving to the first of them.

### 3.5 Auto-lock

`Settings.auto_lock_minutes` counts from the last GUI interaction. At
`0`, the vault locks when the window is hidden. Sixty seconds before an
auto-lock, if the window is visible, a warning toast appears with a
`Stay unlocked` action that resets the timer. Auto-lock never fires while a job
is running; the timer restarts when the last run finishes.

---

## 4. Onboarding / first run

Runs when `config.json` does not exist. Seven steps, presented in a dedicated
frameless-content window at **880 × 640** (no rail, no status strip), centred.
A step indicator sits at the top: seven 6px dots, 8px apart, the current one
20px wide and `accent`, completed ones `text.muted`, upcoming ones
`border.strong`. Below the dots, `small` `text.muted`: `Step 3 of 7`.

Footer is fixed at 72px: `Back` (ghost, hidden on step 1) on the left,
`Continue` (primary, 36px tall) on the right, and `Skip setup` (ghost,
`text.muted`) far left on steps 4–6 only.

The whole flow is skippable **after** step 3. Steps 1–3 are mandatory: without a
master passphrase the application cannot store a single credential.

### O-1 Welcome

- 48px product mark (the tray ring at 48px, `accent`), centred.
- `display` title, `body` `text.secondary` subtitle, max 520px, centred.
- Three feature rows (20px icon + `body.strong` label + `small` description),
  left-aligned in a 520px column: fan-out to many destinations, developer
  exclusions, everything encrypted before it leaves the machine.
- A `small` `text.muted` footnote crediting Kopia with a link.
- Footer: `Continue` only.

### O-2 Create your master passphrase

Left column 360px: an explanation of what the passphrase protects (the vault
holding repository passphrases and storage keys) and what it does not (it is not
the repository passphrase itself).

Right column 400px, the form:
1. `Master passphrase` — passphrase field, 400px, autofocused.
2. Strength meter (§8.17) directly under it, plus the score label.
3. Live requirement list — three rows, each 20px, 14px `check`/`circle` icon:
   - `At least 12 characters` (hard requirement)
   - `Not a password you use elsewhere` (unverifiable; shown as an unchecked
     advisory bullet with a `text.muted` dot, never a green check)
   - `Four or more words is stronger than symbols` (advisory)
4. `Confirm passphrase` — second field; mismatch shows the error only on blur or
   submit, never while typing.
5. A `Suggest a passphrase` ghost button that generates a six-word diceware
   phrase, fills both fields, reveals them, and shows the toast
   `copy: onboarding.suggested`.

**Policy**: `Continue` is enabled at ≥12 characters and matching confirmation.
Strength score 0–1 does not block, but triggers the confirmation described in
O-3. A tool that refuses the user's own choice on a local file is being
paternalistic; a tool that lets them sleepwalk past it is being negligent. The
resolution is friction, not prohibition.

### O-3 There is no recovery

This screen exists to make one fact land. It is **not** a modal, not a
checkbox buried in O-2; it is a full step the user must walk through.

- 32px `alert-triangle` in `warning.mark`, `display` title
  `copy: onboarding.norecovery.title`.
- Body, max 560px, three short paragraphs (`copy: onboarding.norecovery.body`):
  what the passphrase encrypts; that there is no reset, no backdoor, no support
  email that can help; what to do about it (a password manager, or paper in a
  drawer).
- A `bg.raised` panel, 16px padding, radius 10, containing:
  - `Copy passphrase to clipboard` (secondary, `copy` icon) — clears the
    clipboard after 60 seconds and says so in `small` `text.muted` underneath.
  - `Save a recovery sheet…` (secondary, `file-text`) — writes a plain-text file
    the user chooses the location of, containing the passphrase, the machine
    label and slug, the date, and a printed warning. A `small` `text.muted`
    line states plainly that this file is unencrypted.
- One mandatory checkbox, `body`, not `small`:
  `copy: onboarding.norecovery.ack` — "I have stored my master passphrase
  somewhere I can get to it. If I lose it, my backups cannot be recovered."
- If the strength score was 0–1, a second mandatory checkbox appears above it:
  `copy: onboarding.weak_ack`.
- `Continue` is disabled until every checkbox is ticked.

The vault is created when `Continue` is pressed here, not on O-2 — so that a
user who backs out of this screen has not left a half-initialised vault behind.

### O-4 Scanning this machine

Runs three probes with a 12-second overall budget and shows results as they
arrive. Each result is a 64px row in a card: 20px icon, `body.strong` title,
`small` `text.secondary` detail, and a trailing control.

| Probe | Found | Not found |
|---|---|---|
| **Kopia** | `check-circle-2` success, `Kopia <version> at <path>` (`mono`, elided) | `alert-triangle` warning, `copy: onboarding.kopia.missing` + `Download Kopia` primary-compact button (downloads a pinned build to `Paths::bundled_kopia()`, showing a determinate progress bar in the row) and `Choose a file…` secondary |
| **OneDrive** | `check-circle-2` success, `OneDrive — <account>` and the path in `mono`. A checkbox, **ticked by default**: `Create a OneDrive destination here` | `minus-circle` neutral, `copy: onboarding.onedrive.none`, no action. Not an error. |
| **Disk space** | `check-circle-2`, `<free> free on <drive>` | `alert-triangle` when < 20 GB free on the proposed local repository drive |

OneDrive detection detail: when found, the proposed destination is
`DestinationKind::OneDrive { path: <onedrive_root>/superbackup, account }` with
`auto_discovered = true`. A `small` `text.muted` note under the checkbox
explains the interaction honestly (`copy: onboarding.onedrive.explain`): the
repository is a small number of large files rather than millions of small ones,
which is exactly what OneDrive can cope with, and superbackup marks the folder
so OneDrive does not try to convert it to on-demand files.

Multiple OneDrive accounts: one row per account, each with its own checkbox,
first one ticked.

`Continue` is enabled once the probes finish or the budget expires. Kopia
missing does **not** block onboarding — jobs simply cannot run until it is
resolved, and the dashboard will say so.

### O-5 Your first backup job

A compressed run of the wizard (§7) with three steps inside this one step,
using an internal sub-stepper so the outer dot indicator stays at 5 of 7:

**O-5a Template.** Four 96px template cards in a 2 × 2 grid, 16px gutter, each
with a 24px icon, `h2` title, `small` description, and a `small` `text.muted`
line naming what it will exclude:

| Template | Sources prefilled | Exclusions | Schedule |
|---|---|---|---|
| **Development folder** | The first existing of `~/dev`, `~/source`, `~/repos`, `~/Projects`, `%USERPROFILE%\source\repos` | `ExclusionSet::developer_defaults()` | `Daily { 02:00 }` |
| **Documents and desktop** | `~/Documents`, `~/Desktop` | `OsJunk`, `LogsAndTemp` | `Daily { 02:00 }` |
| **Whole user folder** | `~` | `developer_defaults()` + `VirtualMachineImages` | `Daily { 02:00 }` |
| **Start from scratch** | none | none | `Manual` |

The Development folder card is visually primary (1px `accent` border) and is
preselected. Its description names the problem in the user's own terms: it
skips `node_modules`, build output and caches so the backup stays small and
fast.

**O-5b Sources and destinations.** The source list (§7 W-2) plus a destination
picker limited to what onboarding can create without further input:
- `Local repository` at a proposed path (default:
  `<largest local fixed drive>/superbackup/<machine slug>/repository`), ticked.
- `OneDrive repository` at the detected path, ticked if O-4's checkbox was.
- A ghost row `Add S3 or other storage later` (not a control — a statement,
  with an `info` icon) making it clear this is a normal path, not a limitation.

Repository passphrases for these destinations default to
`PassphraseSource::DerivedFromMaster`, with a one-line explanation and a
`Change…` link that opens the full encryption panel (T-4). Deriving from the
master key is the right default here: it means one secret to protect during the
riskiest moment in the product's life, and the vault can always reconstruct it.

**O-5c Review.** A read-only summary: job name (editable inline, default
`Development`), sources with a computed size estimate (a background walk with a
2-second budget, showing `Estimating…` then `~12.4 GB in 84,000 files after
exclusions`), destinations, schedule, and the exclusion count with a
`See all 41 patterns` disclosure.

`Continue` creates the `Job`, the `Destination`s, and — for repository
destinations — queues repository creation to run after step O-7, showing
progress on the dashboard. Repository creation is not blocking here; a first-run
S3 repository can take a minute and the user should not be watching a spinner.

### O-6 Keep it running

Two stacked option cards, 88px each:

1. **Start superbackup when I sign in** — toggle, default **on**
   (`Settings.start_at_login`). Sub-toggle, indented 28px, enabled only when the
   parent is on: **Start minimised to the tray**, default **on**
   (`Settings.start_minimised`).
2. **Install the background service** — toggle, default **off**
   (`Settings.run_as_service`). Body explains the actual trade-off: the service
   runs backups even when nobody is signed in, but it needs the master
   passphrase available without a person to type it, which means enabling the OS
   keychain (`Settings.use_os_keychain`). Turning this toggle on reveals a third
   nested toggle for the keychain, pre-ticked, and a `small` `warning.tint.text`
   line stating what that means: anyone who can run code as this user can ask
   the keychain for the key.

If the service install needs elevation, the toggle shows a `shield` icon and the
elevation prompt appears on `Continue`, not on toggle. A refused prompt leaves
the toggle off and shows an inline warning, never a modal.

### O-7 You are set up

- 32px `check-circle-2` in `success.mark`, `display` title.
- A three-row summary: `1 job`, `2 destinations`, `Next run tonight at 02:00`.
- Primary button `Back up now` (36px) which starts the job and opens the
  dashboard; secondary `Go to dashboard`.
- A `small` `text.muted` line telling the user where the app lives now: the tray
  icon, and that closing the window does not stop backups.

### Onboarding edge cases

| Case | Behaviour |
|---|---|
| User quits mid-flow before O-3 | Nothing is written. Onboarding restarts from O-1. |
| User quits after O-3 | Vault exists. Restart resumes at O-4 with a `small` note that the passphrase is already set. |
| `config.json` exists but the vault does not | Danger screen: the config references secrets that cannot be resolved. Offers `Restore config.sbvault from a backup` (opens `vault-backups`) or `Start over and re-enter every credential` (destructive confirmation, typed `superbackup` to confirm). |
| Remote config configured via CLI before first GUI run | O-4 gains a fourth probe row: `Remote configuration found`, offering to pull. Pull requires the master passphrase, which by then exists. |
| No writable location for a local repository | O-5b's local repository option is disabled with the reason; OneDrive or S3 must be used. |

---

## 5. Dashboard `D-1`

The default screen. Answers, in order: *is everything fine, what is happening
right now, when does it next run, what do I do about it.*

### 5.1 Layout (1100 × 720)

Content column 819px wide inside 24px padding.

```
Health strip            819 × 88     (3 tiles, 16px gutters: 275 / 264 / 264)
16px gap
Active runs section     819 × auto   (only when active_runs is non-empty)
16px gap
Jobs section header     819 × 28
Job card grid           2 columns × 401px, 16px gutter, 96px cards, 16px rows
```

### 5.2 Health strip

Three tiles, each a card (radius 10, 1px `border.subtle`, 16px padding, 88px).

**Tile 1 — Overall health.** A 40px health ring (the tray mark at 40px, drawn
live, same five states), then a text column: `h2` `Health::title()` and a
`small` `text.secondary` reason line. The tile background is `bg.surface` in all
states except `Failed`, where it becomes `danger.tint.bg` with a 1px
`danger.mark` @ 40% border. Reason line by health:

| Health | Reason line |
|---|---|
| `Idle` | `Last backup <relative>` or `No backups yet` |
| `Running` | `<n> job(s) running` |
| `Attention` | The single most important reason, in priority order: vault locked → kopia missing → `<n>` jobs have not succeeded in `<stale_after_days>` days → destination unverified |
| `Paused` | `Paused until <time>` / `Paused until you resume` (+ `PauseState.reason` when set) |
| `Failed` | `<job name> failed <relative>` (+ `and <n> others` when more) |

Trailing action, right-aligned in the tile: `Paused` → `Resume` (primary
compact); `Failed` → `View error` (secondary compact); `Attention` with a locked
vault → `Unlock` (primary compact); otherwise nothing.

**Tile 2 — Next scheduled run.** From `next_scheduled`. `micro` label
`Next scheduled run`, `h2` value = relative time (`in 4 hours`), `small`
`text.muted` = `<job name> · <absolute time>`. When `paused` the value is struck
through and the sub-line reads `blocked while paused`. When the vault is locked,
`blocked while locked`. When no job has an automatic `Schedule`, the value is
`Not scheduled` and the sub-line is a link `Set up a schedule`.

**Tile 3 — Last 7 days.** `micro` label, then a 7-column bar strip 28px tall:
one 24px-wide column per day, each column a stack of 4px segments coloured by
that day's run outcomes (success / warnings / failed), plus `text.muted` day
initials beneath. To its right, `h2` with the total uploaded bytes over 7 days
(summed from `JobSummary.last_uploaded_bytes` per completed run in history) and
`small` `<n> runs, <m> failed`. Clicking the tile opens Activity filtered to 7
days. Hovering a column shows a tooltip with that day's counts.

### 5.3 Active runs section

Present only when `active_runs` is non-empty. Header row: `h2` `Running now`,
a count pill, and a right-aligned `Stop all` danger-ghost button (confirmation
required).

One **run panel** per `JobRun`, full width, `bg.surface`, radius 10, 1px
`border.control`, 16px padding, 16px between panels:

```
Row 1  [refresh-cw ⟳]  Dev code                    [Running]      Stop
       h2                                          badge          ghost btn
Row 2  small text.muted:  Started 4m 12s ago · triggered by Schedule
Row 3  aggregate progress bar, 8px, JobRun::overall_fraction()
Row 4  small mono: 84,214 of ~120,000 files · 6.2 GB of ~9.1 GB · 18.2 MB/s · ~3m left
Row 5  small mono text.muted: Scanning C:\Users\andreas\…\web\src\components
6px gap, 1px border.subtle divider, 12px gap
Per-destination rows (one per DestinationRun):
       [icon] Local repo        ▓▓▓▓▓▓▓▓░░░░  72%   4.1 GB uploaded   [Succeeded]
       90px name  |  6px bar remainder  |  40px %  |  120px bytes  |  badge
```

Rules:
- `current_path` (row 5) is elided from the left, keeping the tail, and is only
  shown for the destination currently reading the source tree — for a fan-out,
  the scan happens once, so this line belongs to the run, not a destination.
- `estimated_seconds_remaining` renders as `~3m left`; when `None`, the field is
  omitted entirely rather than showing a placeholder.
- When `bytes_total` is `None` the bar is indeterminate and the label reads
  `Estimating…` (see design system §8.8).
- `files_cached` appears in the tooltip on the file counter as
  `<n> files unchanged since last run`, which is where the dedup win becomes
  visible.
- `errors_ignored > 0` turns the bar `progress.fill.warn` and adds
  `· <n> files skipped` in `warning.tint.text` to row 4, linking to the run
  detail's warnings list.
- A destination that has already finished keeps its row and shows its terminal
  badge; a destination that failed shows a `danger` badge and an inline
  `View error` link. The run continues to other destinations when
  `Job.continue_on_destination_error` is true; when false, remaining
  destinations show `Cancelled`.

**Stop**: `Stop` on a run panel opens a small confirmation
(`copy: run.stop.confirm`) explaining that the partial snapshot is discarded and
the next run starts fresh. Confirming sets the run to `Cancelled`. `Stop all`
uses the plural copy and lists the affected job names.

### 5.4 Jobs section

Header row: `h2` `Jobs`, count pill, right-aligned `Back up now` (primary,
runs every enabled job) and a 30 × 30 ghost `⋯` menu (`Run all now`,
`Disable all jobs`, `New job…`).

**Job card** (96px, per design system §8.6). Two columns at ≥1000px, one below.

```
│ 3px status spine (mark colour)
│ 16px pad
│  Dev code                          [Succeeded]   [⋯]
│  h2                                badge         ghost 26px
│  Last run 2 hours ago · 4m 12s · 842 MB uploaded          small text.muted
│  [🖴 Local repo] [☁ OneDrive] [🗄 StorJ offsite]           chips, 24px
│                                                  Run now   ghost compact,
│                                                            right-aligned
```

Card content by state:

| Card state | Row 2 (meta) | Row 3 |
|---|---|---|
| Succeeded | `Last run <rel> · <duration> · <bytes> uploaded` | destination chips |
| Completed with warnings | same + `· <n> files skipped` in warning tint | chips; the chip whose destination warned carries a warning dot |
| Failed | `Failed <rel> · <n>th consecutive failure` in danger tint (the ordinal comes from `consecutive_failures`) | chips (failed chip carries a danger dot) + `View error` link replacing `Run now` position, with `Run now` moving left |
| Running | `Started <rel> · <trigger>` | 6px aggregate progress bar + `<percent>% · <rate>` |
| Queued | `Queued behind <job name>` | chips, dimmed to 70% |
| Never run | `Never run · <n> sources · next run <rel>` | chips |
| Disabled (`Job.enabled == false`) | `Disabled` badge in the badge slot; meta shows the last known result | whole card at 60% opacity except the badge; `Run now` becomes `Enable` |
| Stale (`JobSummary::is_stale`) | `Last success <rel>` in warning tint | chips; a `warning` badge takes the badge slot |

A `Project` colour, when set, is a 2px vertical stripe immediately right of the
status spine, 4px gap.

The `⋯` menu: `Run now`, `Stop` (only while running), `Edit…`, `Duplicate`,
`Browse snapshots…`, `View history`, `Disable` / `Enable`, separator,
`Delete…` (danger).

**Card click** opens the job editor. **Double-click** does nothing extra.
`Run now` never confirms — running a backup is safe.

### 5.5 Dashboard empty state

No jobs at all: the health strip is replaced by a single 88px card reading
`No jobs yet`, and the jobs area shows the empty state (§8.13) with icon
`repeat`, title/body from `COPY.md` `empty.jobs.*`, primary `Create your first
job`, ghost `Import from another machine…` (opens Settings → Remote config).

Jobs exist but none has run: cards show the `Never run` state; the health tile
reads `Idle` with reason `No backups yet` and a `Back up now` action.

### 5.6 Dashboard at 900 × 600

Health strip tiles stack from 3 across to 3 across at 249px each (they fit;
`Last 7 days` drops its `h2` byte total and keeps the bar strip). The job grid
becomes one column. The active-run panel drops the per-destination `bytes
uploaded` column. Nothing is removed that is not duplicated elsewhere.

---

## 6. Jobs

### 6.1 Jobs list `J-1`

Header actions: a 280px search field (`Ctrl+F`), a `Group by` combo
(`None` / `Project` / `Schedule`), a `Filter` combo (`All` / `Enabled` /
`Disabled` / `Failing` / `Stale`), and `New job` (primary).

Table (design system §8.7), 36px rows:

| Column | Width @1100 | Content | Drop order @900 |
|---|---|---|---|
| Status | 32 | Status icon only, tooltip = `RunStatus::title()` | keep |
| Name | 220 | `body.strong` name; `small` `text.muted` second line with `Job.description` when set (row grows to 48px when any row has a description) | keep |
| Sources | 60 | Count, `mono.small`, tooltip lists paths | 4th to drop |
| Destinations | 150 | Up to 3 chips, then `+N` | keep |
| Schedule | 130 | Human schedule string (§6.4) | keep |
| Last run | 120 | Relative time + duration | keep |
| Next run | 120 | Relative time | 3rd to drop |
| Uploaded | 90 | `last_uploaded_bytes`, `mono.small`, right-aligned | 1st to drop |
| Actions | remainder (≥76) | `Run now` / `Stop` compact ghost + `⋯` | keep |

Sorting: click a header; default sort is `Status` descending (problems first),
then `Name`. Sort persists per session.

Grouping by `Project` inserts a 28px group header row: `h3` project name, a 8px
project-colour dot, a count pill, and a right-aligned `Run group` ghost link.
Jobs without a project are grouped last under `Ungrouped`.

Multi-select: Shift-click and Ctrl-click select rows; a 44px selection bar
replaces the header actions showing `<n> selected` and the bulk actions
`Run`, `Enable`, `Disable`, `Delete…`. Escape clears.

Empty state: `empty.jobs.*`.
Filtered-to-nothing state: `empty.jobs.filtered` with a `Clear filters` ghost.

### 6.2 Job editor `J-2`

Reached from a card, a row, or the wizard's `Advanced` exit. Breadcrumb header
`Jobs / <name>`, header actions `Run now` (secondary), `⋯`, `Cancel` (ghost),
`Save changes` (primary, disabled until dirty).

Layout: a 30px segmented control (design system §8.5) directly under the header,
five tabs, then the tab body in a `ScrollArea`. A dirty tab shows a 6px `accent`
dot after its label.

Leaving with unsaved changes opens a small modal: `Save`, `Discard`, `Cancel`.

Form geometry throughout: label column is above the field (not beside), fields
are 400px wide unless stated, groups are separated by a 1px `border.subtle`
divider with 20px above and 16px below, and each group has an `h3` header plus
an optional `small` `text.muted` description.

#### Tab 1 — Sources

- **Name** (`TextEdit`, 400px, max 64 chars) and **Description** (400px, single
  line, optional).
- **Project** combo (400px) listing projects plus `— None —` and
  `New project…` (opens a small modal: name, description, a 10-swatch colour
  picker of fixed hues).
- **Tags** — a chip input: existing `Job.tags` as 24px removable chips, plus a
  120px inline text field; Enter or comma commits a tag. Tags are free text,
  lower-cased, deduplicated.
- **Source folders** — a table, 36px rows, container width 819px:

  | Column | Width | Content |
  |---|---|---|
  | Path | remainder | `mono`, middle-elided, tooltip = full path. A 14px `alert-triangle` prefix when the path does not currently exist. |
  | Size | 110 | Background-computed `<size> · <n> files` after exclusions, `mono.small`. Shows `Estimating…` until the walk finishes, `—` if the walk is cancelled. |
  | Symlinks | 90 | Toggle bound to `Source.follow_symlinks` |
  | One FS | 90 | Toggle bound to `Source.one_filesystem` |
  | | 32 | Ghost `trash-2` |

  Below the table: `Add folder…` (secondary, opens the native picker,
  multi-select allowed) and a `small` `text.muted` hint. Dropping folders onto
  the window adds them here (design system L15).

  `follow_symlinks` carries a tooltip that states the real risk, taken from the
  model's own comment: following symlinks is how a "back up my project" job
  swallows the whole disk.

  Duplicate paths are rejected on add with a toast. A path that is a child of an
  existing source is rejected with an explanation naming the parent. A path that
  is a *parent* of existing sources offers to replace them.

- **Empty state**: `empty.sources.*` inside the table container, 160px tall.

#### Tab 2 — Destinations

The fan-out tab. Header line: `h3` `Send this backup to` + `small`
`text.secondary` explaining that every ticked destination receives a complete
copy.

A list of **selectable destination rows**, one per `Destination` in config,
64px tall, `bg.surface`, 1px `border.subtle`, radius 6, 8px apart:

```
[✓]  [icon]  StorJ offsite                                [Verified 2d ago]   [⋯]
             S3 bucket · storj-backups / superbackup/andreas-pc-a3f9c2d1/
             small mono text.muted
```

- The checkbox binds to membership in `Job.destination_ids`.
- The trailing badge shows verification state from
  `Destination.last_verified_at`: `Verified <rel>` (success tint, ≤ 7 days),
  `Verified <rel>` (neutral, > 7 days), `Never verified` (warning),
  `Unreachable` (danger, when the last attempt failed).
- Disabled destinations (`Destination.enabled == false`) render at 60% opacity
  with a `Disabled` badge, and cannot be ticked; the row's tooltip explains why
  and offers `Enable in Destinations`.
- `⋯` menu: `Verify now`, `Edit destination…`, `Browse snapshots…`.
- Rows are ordered: ticked first, then by kind (Local repository, OneDrive, S3
  bucket, Folder mirror), then by name. Re-ordering happens only on save, so
  ticking a box does not make the list jump under the cursor.

Below the list: `New destination…` (secondary) which opens the destination
editor `T-2` as a **full screen push**, not a modal, and returns here with the
new destination ticked. This is the one cross-screen flow in the app and it
must preserve unsaved job edits — they are held in memory and restored.

Then a group `When a destination fails`:
- Toggle `Keep going to the other destinations`, bound to
  `Job.continue_on_destination_error` (default on). `small` `text.muted`
  explains: with it off, the first broken destination stops the run and the
  others are marked `Cancelled`.

Validation: a job with zero destinations cannot be saved. The Save button is
disabled and the tab shows an inline danger message
(`copy: job.err.no_destinations`).

Mixed-kind warning: when a job targets both a repository destination and a
`LocalMirror`, an `info` banner explains the difference in one sentence —
mirrors are plain readable copies with no history and no deduplication, so
retention and encryption settings do not apply to them.

#### Tab 3 — Schedule

A 6-option radio list mapping exactly to `Schedule`. Selecting one reveals its
controls indented 28px; unselected options show no controls at all.

| Radio | Controls |
|---|---|
| **Manual only** | none. `small`: runs only when you or the CLI ask. |
| **Every N minutes** (`Interval`) | `DragValue` 1–10080 with a `minutes` suffix, plus quick chips `15m`, `30m`, `1h`, `4h`. `small` warning when < 15: frequent runs on a large source keep the disk busy. |
| **Daily at** (`Daily`) | A row of time chips; each is `HH:MM` with a ghost `x`. `Add time…` opens a 2-`DragValue` popover (hour 0–23, minute 0–59 stepping 5, free typing allowed). Max 24 times. |
| **Weekly on** (`Weekly`) | Seven 36 × 30 toggle buttons `Mo Tu We Th Fr Sa Su` (0 = Monday, matching the model), plus the same time chip row. |
| **Cron expression** (`Cron`) | 400px `mono` field, live-validated with `croner`. Under it, `small`: either the parse error in danger tint, or `Next five runs: <five local timestamps>` in `text.muted`. A `Cron help` link opens a 480px popover with the five-field layout and four worked examples. |
| **When files change** (`OnChange`) | Two `DragValue`s: `Wait for quiet` (`debounce_seconds`, 5–3600, suffix `seconds`) and `At most once every` (`min_interval_minutes`, 1–1440, suffix `minutes`). `small` explains the semantics: the job runs once the watched folders have been quiet for the debounce period, and never more often than the minimum interval. An `info` banner appears when any source has more than 50,000 files, noting that filesystem watching on very large trees costs memory. |

Below the radio group, a `bg.raised` 44px summary strip, always visible:
`Next five runs: 02:00 tonight, 02:00 tomorrow, …` (or
`This job runs only when you ask`). This is computed live from the edited
schedule, not the saved one.

Then a group `Run conditions` (per-job, distinct from global settings):
- Toggle `Skip when on a metered connection` — defaults to the global
  `Settings.skip_on_metered` and shows `Using the global setting` in `small`
  `text.muted` until the user changes it, after which it shows
  `Overriding the global setting` with a `Reset` link.
- Toggle `Skip when on battery` — same pattern against
  `Settings.skip_on_battery`.
- **Timeout**: checkbox `Stop the run after` + `DragValue` (1–1440, suffix
  `minutes`), bound to `Job.timeout_minutes` (`None` when unchecked). `small`
  explains a timed-out run is recorded as `Failed`, not `Cancelled`, because
  something went wrong.

#### Tab 4 — Exclusions

The tab that justifies the product. Header: `h3` `Exclusions` and `small`
`text.secondary` stating the point plainly — skipping regenerable files is what
keeps a developer backup small enough to be reliable.

**Preset list.** One row per `ExclusionPreset::all()` (12 rows), 56px tall,
`bg.surface`, 1px `border.subtle`, radius 6, 8px apart:

```
[✓]  node_modules                                                    3 patterns
     Reinstallable from your lockfile. Usually the single largest win.
     small text.muted  ← ExclusionPreset::rationale(), verbatim
```

- The rationale string is taken **verbatim** from
  `ExclusionPreset::rationale()`. It is not paraphrased in the GUI, so the CLI
  and the GUI never disagree.
- `is_risky()` presets (`GitObjects`, `VirtualMachineImages`) get a 14px
  `alert-triangle` in `warning.mark` before the title and a `warning.tint.bg`
  row background at 30% alpha. They are never ticked by any template.
- The trailing `<n> patterns` count is a link that expands the row by
  `n × 20px` to list the patterns in `mono.small`, `text.muted`.
- A `Select developer defaults` ghost button above the list applies exactly
  `ExclusionSet::developer_defaults()` — ten presets, `respect_cachedir_tag`
  on, `use_gitignore` off — and shows a toast naming what changed.
  `Clear all` sits beside it.

**Additional options** group:
- Toggle `Use .gitignore files found in the sources` (`use_gitignore`).
  `small`: honours each repository's own ignore rules; slower on very large
  trees because every directory is checked for a `.gitignore`.
- Toggle `Skip folders tagged with CACHEDIR.TAG` (`respect_cachedir_tag`),
  default on. `small` names the standard in one clause.
- Checkbox + `DragValue` `Skip files larger than <n> MB` (`max_file_size_mb`,
  `None` when unchecked). `small` warns that this silently drops large files
  from every snapshot, and that skipped files appear in the run's warnings.

**Custom patterns** group: a multiline `mono` `TextEdit`, 819 × 140px,
one pattern per line, `.gitignore` syntax, bound to `ExclusionSet.patterns`.
Placeholder shows three example patterns. Under it, `small` `text.muted` with a
link to the gitignore syntax reference, and live validation: each invalid line
is listed by number with the reason (see §17).

**Effective patterns** disclosure, collapsed by default:
`Show all effective patterns (<n>)` → a code block (design system §8.18) with
`ExclusionSet::effective_patterns()` output, sorted and deduplicated exactly as
the model produces it, with a `copy` button. Preset-sourced lines are
`text.muted`, user lines are `text.primary`, so the origin is visible.

**Impact preview** — a 44px `bg.raised` strip pinned at the bottom of the tab:
`These rules exclude about 8.2 GB in 412,000 files from your sources.` computed
by a background walk with a 4-second budget, refreshed 600 ms after the last
edit, showing `Calculating…` in between and `Could not estimate` if the walk
fails. This is the number that makes the exclusion system tangible.

#### Tab 5 — Advanced

Four groups.

**Bandwidth** (`Job.bandwidth: Option<BandwidthSettings>`):
- Radio: `Use the global limit` (default) / `Set a limit for this job`.
- When overridden: two `Upload` / `Download` rows, each a checkbox +
  `DragValue` with a `kB/s` suffix; unchecked = `None` = unlimited.
- A `small` `text.muted` line always states what the global limit currently is,
  so the override is comparable: `Global limit: 2,000 kB/s up, unlimited down`.
- The per-job daily window is deliberately **not** offered — the daily window
  belongs to global settings (§12.3) and duplicating it per job produces
  unresolvable conflicts. A `small` note says so and links to Settings.

**Retention** (`Job.retention: Option<RetentionPolicy>`):
- Radio: `Use each destination's policy` (default) / `Set a policy for this job`.
- When overridden, six `DragValue`s in a 3 × 2 grid, 120px each, labelled
  `Latest`, `Hourly`, `Daily`, `Weekly`, `Monthly`, `Annual`, bound to the
  `keep_*` fields, plus `Run maintenance every <n> successful runs`
  (`maintenance_every_n_runs`).
- Under the grid, a plain-English rendering of the policy in `small`:
  `Keeps the 10 most recent snapshots, then 24 hourly, 14 daily, 8 weekly, 12
  monthly and 3 annual snapshots.` and, when every field is 0, a danger message
  (`copy: retention.err.all_zero`).
- A `small` `text.muted` note: retention applies only to repository
  destinations; folder mirrors always hold exactly one copy.

**Hooks** (`JobHooks`): three 400px `mono` single-line fields —
`Before the backup`, `After a successful backup`, `After a failed backup`. Under
the first, a checkbox `Cancel the backup if this command fails`
(`abort_on_before_failure`). A `small` `text.muted` block lists the environment
variables passed to hooks (`SUPERBACKUP_JOB_NAME`, `SUPERBACKUP_RUN_ID`,
`SUPERBACKUP_STATUS`, `SUPERBACKUP_DESTINATIONS`) and states the timeout
(120 seconds each, non-configurable) and that hook output is captured into the
run's event log. A `warning` banner sits above the group: commands run with this
user's privileges, and superbackup does not sandbox them.

**Danger zone**: `bg.surface` card with a 1px `danger.mark` @ 40% border, 16px
padding, containing `Delete this job` (danger button) and a `small` line stating
precisely what deletion does and does not do — it removes the job definition; it
never touches snapshots already written to any destination.

### 6.3 Job editor keyboard focus order

Header back → tab segments → tab content in visual order → header
`Cancel` → `Save changes`. Within Sources: Name → Description → Project → Tags
field → each source row (path, symlinks, one-fs, delete) → `Add folder…`.

### 6.4 Human schedule strings

Rendered identically in the job list, cards, and the wizard:

| `Schedule` | String |
|---|---|
| `Manual` | `Manual only` |
| `Interval { minutes: 30 }` | `Every 30 minutes` (`Every hour`, `Every 4 hours` when exact) |
| `Daily { times: [02:00] }` | `Daily at 02:00` |
| `Daily { times: [09:00, 18:00] }` | `Daily at 09:00 and 18:00` |
| `Daily` with ≥3 times | `Daily, 4 times a day` (tooltip lists them) |
| `Weekly { [0,2,4], [02:00] }` | `Mon, Wed, Fri at 02:00` |
| `Weekly` all 7 days | rendered as `Daily at …` |
| `Cron { expression }` | `Cron: 0 2 * * *` in `mono`, tooltip shows the next five runs |
| `OnChange { 120, 30 }` | `When files change (2 min quiet, at most every 30 min)` |

---

## 7. New job wizard

Reached from `New job`, `Ctrl+N`, the dashboard empty state, and the tray menu.
A **large modal** (760px wide, height `min(620, window_height − 96)`), six
steps. Not a separate window: the user is already in context.

Header: `h2` `New job`, `small` `text.muted` `Step 2 of 6 · Sources`, ghost `x`.
Footer: `Back` (ghost) left, `Cancel` (ghost) then `Continue` (primary) right.
On step 6 the primary becomes `Create job`.

A left step list is **not** used — at 760px it would cost 180px of width for six
labels. The header sub-line carries the same information.

### W-1 Template

Four cards (2 × 2, 340 × 104, 16px gutter), exactly the four in §O-5a.
The `Development folder` card is preselected and carries a
`small.strong` `accent` eyebrow `Recommended for developers`.

Each card shows, in `small` `text.muted`: the folder it will propose (elided
path) and, for the two developer templates, `Applies 10 exclusion presets`.

Selecting `Development folder` sets `ExclusionSet::developer_defaults()` on the
draft job. This is the one place the template concept touches the model, and it
calls exactly that constructor — the GUI does not assemble its own preset list.

### W-2 Sources

The source table from J-2 Tab 1, at modal width (712px inside padding), plus the
name field above it (prefilled from the template: `Development`, `Documents`,
`<machine label>`, or empty). The size estimate runs here and its result is
carried to W-6.

Cannot continue with zero sources.

### W-3 Destinations

The destination row list from J-2 Tab 2. When the user has **no** destinations
yet, this step instead shows a four-option chooser — the four `DestinationKind`
variants as 160px-tall cards with icon, title, one-line description, and a
`small` `text.muted` line naming the trade-off:

| Kind | Trade-off line |
|---|---|
| Local repository | `Fastest. Same building, so it does not survive a fire or a theft.` |
| OneDrive repository | `Offsite through a folder you already sync. Limited by your OneDrive quota.` |
| S3 bucket | `Offsite and independent. Costs money and needs an account.` |
| Folder mirror | `A plain readable copy. No history, no deduplication, no encryption.` |

Choosing one pushes the relevant destination editor (`T-2`) inside the same
modal as a sub-flow, then returns.

Cannot continue with zero destinations.

### W-4 Schedule

The schedule radio list from J-2 Tab 3, prefilled from the template
(`Daily { 02:00 }` for the three folder templates, `Manual` for scratch), plus
the next-five-runs strip. Run conditions are not shown here — they default to
the globals and live in the editor.

### W-5 Exclusions

The preset list from J-2 Tab 4, prefilled by the template, plus the impact
preview strip. Custom patterns and the effective-pattern disclosure are present;
the `max_file_size_mb` and `.gitignore` toggles are present. Nothing is hidden
here relative to the editor — a wizard that hides options teaches the user that
the wizard is the wrong path.

### W-6 Review

A read-only summary in key/value form (design system §8.16), 160px label column:

```
Name             Development
Project          — none —
Sources          C:\Users\andreas\dev              12.4 GB · 84,102 files
                 C:\Users\andreas\source\repos      3.1 GB · 21,880 files
                 after exclusions
Destinations     Local repo · OneDrive · StorJ offsite
Schedule         Daily at 02:00 — next run tonight at 02:00
Exclusions       10 presets, 41 patterns · excludes ~8.2 GB in 412,000 files
Retention        Each destination's own policy
Bandwidth        Global limit (2,000 kB/s up)
```

Below it, a checkbox `Run this job now`, ticked by default, and a `small`
`text.muted` line stating that the first run copies everything and later runs
copy only what changed.

`Create job` writes the `Job`, closes the modal, navigates to the dashboard, and
— when the checkbox is ticked and the vault is unlocked — starts the run. When
the vault is locked, the job is created and a warning toast explains the run did
not start, with an `Unlock and run` action.

### Wizard edge cases

| Case | Behaviour |
|---|---|
| Escape / `x` mid-wizard | Small confirm modal `Discard this job?` with `Keep editing` / `Discard`. Skipped entirely if the user has not advanced past W-1. |
| Vault locked at W-3 while adding an S3 destination | Inline unlock prompt (§3.4) inside the credential group only. The wizard does not restart. |
| Template folder does not exist | The source table is empty and shows an inline `info` note naming the paths that were checked. |
| A repository destination created in W-3 has no repository yet | W-6's Destinations row shows `will be created` after that destination's name, and creation runs as the first phase of the job's first run, with its own progress row. |

---

## 8. Destinations

### 8.1 Destinations list `T-1`

Header actions: 240px search, a `Kind` filter combo (All / Local repository /
OneDrive repository / S3 bucket / Folder mirror), `New destination` (primary).

Table, 44px rows (taller than the jobs table because the location line matters):

| Column | Width @1100 | Content | Drop @900 |
|---|---|---|---|
| Kind | 36 | Kind icon, tooltip `DestinationKind::label()` | keep |
| Name | 200 | `body.strong` name; a 14px `sparkles` badge with tooltip `Found automatically` when `auto_discovered` | keep |
| Location | remainder (≥240) | `mono.small` `text.muted`, middle-elided. Local kinds: the path. S3: `<provider name> · <bucket>/<prefix>` | keep |
| Used by | 90 | `<n> jobs` count pill, tooltip lists names | 3rd |
| Size | 100 | Repository size from the last successful `kopia` maintenance/status, `mono.small`; `—` for mirrors and unconnected repos | 1st |
| Last verified | 120 | Relative time, or `Never` in warning tint | 2nd |
| Status | 100 | Badge: `Ready` / `Not connected` / `Unreachable` / `Disabled` | keep |
| Actions | 76 | `Verify` compact ghost + `⋯` | keep |

`⋯`: `Verify now`, `Browse snapshots…`, `Edit…`, `Duplicate`, `Run maintenance
now` (repository kinds only), `Enable`/`Disable`, separator, `Remove…` (danger).

Empty state: `empty.destinations.*`, primary `Add a destination`.

### 8.2 Destination editor `T-2`

A pushed full screen (breadcrumb `Destinations / <name>`), not a modal, because
repository creation inside it needs room. When entered from the job editor or
the wizard, it renders inside that modal at 712px and the breadcrumb becomes a
back link.

**Common fields** (all kinds), in one card:
- `Name` (400px, required, unique, max 64).
- `Kind` — a 4-segment control on create; **read-only** on edit, rendered as a
  disabled chip with a tooltip explaining that changing the kind would orphan
  the repository, and that the way to change it is to create a new destination.
- `Enabled` toggle (`Destination.enabled`), with `small` explaining a disabled
  destination is skipped by every job without failing them.

**Kind-specific sections** follow.

#### T-2a Local repository (`DestinationKind::LocalRepository`)

- **Folder** — path field + `Browse…`. Live checks under the field, each a 20px
  row with an icon:
  - path exists / will be created
  - free space (`<n> GB free of <m> GB`), warning below 20 GB
  - filesystem type and whether it is removable (`Removable drive — backups run
    only when it is connected`) or a UNC/network path (`Network location —
    availability depends on the share`)
  - an existing repository was detected here → the panel switches to Connect
    mode (§8.3)
- **Encryption panel** (§8.4) when no repository exists yet.
- **Retention** — the six `keep_*` `DragValue`s plus
  `maintenance_every_n_runs`, bound to `Destination.retention`, with the same
  plain-English rendering as J-2.
- **Bandwidth** — `Destination.bandwidth` override, same control set as J-2's
  bandwidth group. Present for local kinds too, because a UNC path over a slow
  VPN needs it.

#### T-2b OneDrive repository (`DestinationKind::OneDrive`)

Everything from T-2a, plus:
- **Account** (`Option<String>`) — a combo listing detected accounts plus
  `Other…` which reveals a text field. `small` explains this is a label for the
  user's benefit; superbackup does not authenticate to OneDrive and does not
  need to.
- A permanent `info` banner at the top of the section explaining the mechanism
  honestly (`copy: dest.onedrive.explain`): the backup is written as a Kopia
  repository — a modest number of large pack files — and OneDrive syncs those
  fine, which is precisely the problem this application solves.
- A checkbox, ticked by default: `Keep these files available offline` — writes
  the platform marker that stops OneDrive converting the repository to
  cloud-only placeholders. `small` states the consequence of unticking it: a
  restore may have to download before it can read.
- A **re-detect** ghost button when `auto_discovered` is true:
  `Check for OneDrive again`, which updates the path if the account moved.

#### T-2c S3 bucket (`DestinationKind::S3`)

- **Storage provider** — combo listing `Config.providers` by name with their
  endpoint in `small` `text.muted`, plus `New provider…` which pushes `P-2`.
  Under the combo, a 44px `bg.raised` read-only strip showing the chosen
  provider's endpoint, region and flavour, with an `Edit provider` link. This
  strip is what stops the user re-entering credentials per bucket.
- **Bucket** (280px, required). A `List buckets` ghost button appears when the
  vault is unlocked and the provider verifies; it fills a combo instead of a
  free-text field once the list is known, with `Other…` to go back to typing.
- **Key prefix** (400px, `mono`), defaulted to
  `default_s3_prefix(machine.slug)` = `superbackup/<machine-slug>/`.
  Normalised through `normalise_prefix()` **on blur**, and the normalised value
  is shown back to the user immediately so surprises happen at edit time, not
  at run time. Under it, `small` `text.muted`:
  `Full path: s3://<bucket>/<normalised prefix>` in `mono.small`, live.
  A `small` note explains why the default contains the machine slug: it is what
  keeps several PCs and several jobs apart inside one bucket.
- **Credentials for this bucket** — radio:
  - `Use the provider's credentials` (default) — `small` shows which provider
    key is used, by name, never by value.
  - `Use a separate key pair for this bucket` — reveals `Access key ID` and
    `Secret access key` fields, and an optional `Session token`, bound to
    `S3Credentials::for_destination(id)`. A `small` note explains when this is
    right: a bucket-scoped key limits what a leaked credential can reach.
  - While the vault is locked this whole group is the inline unlock prompt.
- **Encryption panel** (§8.4), **Retention**, **Bandwidth** as above.

#### T-2d Folder mirror (`DestinationKind::LocalMirror`)

- **Folder** — path field with the same live checks.
- A permanent `info` banner stating what a mirror is and is not
  (`copy: dest.mirror.explain`): a plain readable copy of the newest version of
  each file; no snapshots, no history, no deduplication, no encryption, and
  anyone who can read the folder can read the files.
- **No** encryption panel, **no** retention section (the model's
  `Destination.encryption` is `None` and retention does not apply); both are
  omitted entirely rather than shown disabled.
- **Bandwidth** override is present.
- A checkbox `Delete files in the mirror that no longer exist in the sources`,
  default **off**, with a danger-tinted `small` line: with it on, deleting a
  source file removes it from the mirror on the next run, so the mirror stops
  being a safety net for accidental deletion.

### 8.3 Connect and verify

**Verify** (any destination, any time): a compact ghost button in the list and
the editor. Runs a non-destructive reachability check appropriate to the kind:
local kinds stat the path and write/delete a probe file under `_superbackup/`;
S3 issues a `HeadBucket` and a prefix listing; repository kinds additionally run
`kopia repository status`. Result updates `Destination.last_verified_at` and
appends an `Event`.

While running, the button enters the loading state and a 44px `bg.raised`
progress strip appears under the destination's name with the current step:
`Checking the path…` → `Writing a test file…` → `Opening the repository…`.

Outcomes:

| Outcome | UI |
|---|---|
| Success | Success toast, badge → `Verified just now` |
| `ErrorCode::Io` / path missing | Danger banner in the editor with the OS message, plus `Create the folder` action when the parent exists |
| `ErrorCode::RepoNotConnected` | Info banner offering `Connect to this repository…` (§8.3 Connect) |
| `ErrorCode::RepoExists` during creation | Info banner offering `Connect instead` |
| `ErrorCode::BadPassphrase` | Danger banner: the repository passphrase in the vault does not open this repository. Offers `Enter a different passphrase…` |
| `ErrorCode::KopiaMissing` | Danger banner with the hint from `Error::hint()` and a `Fix in Settings` action |
| Network / S3 auth failure | Danger banner naming the provider and the HTTP status; offers `Test provider connection` which jumps to `P-3` |

**Connect** (an existing repository was found at the location): a medium modal.
Fields: a read-only location, a `Repository passphrase` field with a radio above
it — `Derive it from my master passphrase` (only offered when this destination's
`passphrase_source` was `DerivedFromMaster`) or `I will type it`. On success the
passphrase is stored under `Destination.passphrase_ref` and the encryption
settings are read back from the repository and displayed read-only, since they
are fixed at creation time.

### 8.4 Repository creation and the encryption panel `T-4`

Shown inside the destination editor for repository kinds when no repository
exists. A card with an `h2` header `Encryption`, a `small` `text.secondary`
line, and a **collapsed-by-default** disclosure:

> **Recommended settings** — AES-256-GCM, BLAKE2B-256, dynamic 4 MB blocks, no
> error correction. `Change…`

Expanding reveals the full panel. Every control maps 1:1 onto
`EncryptionSettings`. Defaults come from `EncryptionSettings::default()` and are
never silently overridden.

**Algorithm** (`EncryptionAlgorithm`) — a 2-item radio list. Each option shows
its `kopia_id()` in `mono.small` `text.muted` and its `describe()` string
verbatim as the helper line. `Aes256GcmHmacSha256` carries a `Recommended`
`small.strong` `accent` tag.

**Hash** (`HashAlgorithm`) — combo listing `HashAlgorithm::all()` in the model's
order, each rendered as its `kopia_id()`. Helper lines:

| Option | Helper |
|---|---|
| `BLAKE2B-256` | `Default. Fast and well studied.` |
| `BLAKE2B-256-128` | `Half-length hashes. Slightly smaller indexes, slightly higher collision risk.` |
| `BLAKE3-256` | `Fastest on modern CPUs. Newer than the others.` |
| `BLAKE2S-256` | `Tuned for 32-bit CPUs.` |
| `HMAC-SHA256` | `Widely audited. Slower than BLAKE2.` |
| `HMAC-SHA256-128` | `Half-length variant of HMAC-SHA256.` |

**Splitter** (`Splitter`) — combo over `Splitter::all()`, each shown by
`kopia_id()`. Directly beneath, a `bg.raised` 44px suggestion strip that appears
only when the job(s) targeting this destination have developer exclusions or a
source with a high small-file ratio:

> Your sources contain a lot of small files. `DYNAMIC-2M-BUZHASH` deduplicates
> them better. **Use it**

Pressing `Use it` sets `Splitter::recommended_for_many_small_files()`. The strip
never changes the value on its own.

**Error correction** (`ecc` / `ecc_overhead_percent`) — a toggle
`Add error-correcting data`, default off. When on, a `DragValue` appears:
`Overhead <n>%`, range 1–20, default 5, with the algorithm shown as read-only
`REED-SOLOMON-CRC32` (the only `EccAlgorithm`). Helper text states the trade
plainly: it costs that percentage of extra storage and lets a repository survive
a limited amount of bit rot; it does nothing about a whole disk failing, and it
is most worth having on optical or archival media.

**Passphrase source** (`PassphraseSource`) — a 3-item radio, the single most
consequential control on the screen:

| Option | Sub-controls | Helper |
|---|---|---|
| `Generated` (default) | none until creation, then the write-it-down screen T-5 | `superbackup generates 256 random bits and stores them in your vault. You will be shown the passphrase once and asked to save it.` |
| `UserSupplied` | Passphrase + confirm fields, strength meter, minimum 12 characters | `You choose it. Use this if you also open this repository with the kopia command line.` |
| `DerivedFromMaster` | none | `Worked out from your master passphrase and this destination's id. Nothing extra to store — but if you lose your master passphrase, this repository is lost with it.` |

A `small` `text.muted` line under the radio group, always visible:
`These settings are fixed when the repository is created and cannot be changed
afterwards.` — because that is true of Kopia, and discovering it later is a bad
day.

**Footer of the panel**: a `Create repository` primary button. It is disabled
while the vault is locked (with the standard tooltip) and while any encryption
field is invalid.

**Creation progress**: the panel is replaced by a 5-row checklist, each row
20px with an icon that moves from `circle` → spinner → `check-circle-2`:
`Checking the location` · `Creating the repository` · `Storing the passphrase in
your vault` · `Applying the retention policy` · `Writing the machine record`.
The last step writes `_superbackup/machines/<uuid>.json` and `README.txt` per
the layout in `model.rs`, and its helper line says so, because a user browsing a
shared drive later deserves to know why those files exist.

Failure at any step: the checklist stops, the failed row turns danger with the
`RunError.message`, and a `Retry` / `Change settings` pair appears. A partially
created repository is not left behind silently — the failure message names
exactly what exists on disk and offers `Open the folder`.

### 8.5 Write this down `T-5`

Only for `PassphraseSource::Generated`, shown immediately after successful
creation, as a **blocking modal** (no `x`, Escape does nothing).

- 24px `key-round` in `warning.mark`, `h2` title `copy: repo.writedown.title`.
- Body `copy: repo.writedown.body`: this passphrase opens this repository. It is
  stored in your vault, so day to day you will not need it. You will need it if
  you ever restore on another machine, or if you lose your vault.
- The passphrase in a `bg.code` block, 819px (or modal width), 16px padding,
  `mono.strong` 13px, wrapped into **four groups of words / 8-character
  chunks per line** with a wide space between chunks so it can be transcribed by
  hand without ambiguity. A `small` `text.muted` line under it notes that
  characters are grouped only to make copying easier and the groups are not part
  of the passphrase.
- Three actions in a row: `Copy` (secondary, `copy` icon — clears the clipboard
  after 60 s and says so), `Save to a file…` (secondary — plain text, user
  chooses the location, with a `small` note that the file is unencrypted),
  `Print…` (ghost — only where the platform supports it; otherwise omitted).
- Mandatory checkbox: `copy: repo.writedown.ack`.
- Footer primary `Done`, disabled until the checkbox is ticked. No secondary.

Re-showing it later is impossible and the UI says so: the destination's detail
page shows `Passphrase: generated, stored in your vault` with a `small` line
`It cannot be shown again.` The passphrase *is* still recoverable by exporting
it deliberately from Settings → Security → `Export repository passphrases…`,
which requires the master passphrase to be re-entered and writes a plain-text
file the user chooses. That escape hatch is named on this screen in a `small`
`text.muted` footnote so nobody panics.

### 8.6 Destination deletion

`Remove…` opens a small danger modal. It states, as a bulleted list:

- the number of jobs that will lose this destination (from
  `Config::jobs_using()`), by name, and that they will keep running to their
  other destinations;
- any job that would be left with **zero** destinations, called out in danger
  tint — those jobs will be **disabled**, not deleted;
- that the data at the destination is **not** touched.

A second, separate checkbox — off by default, only shown for local kinds —
offers `Also delete the repository files at <path>` and, when ticked, promotes
the modal to a typed confirmation requiring the destination's exact name and
changes the primary button to `Delete destination and its files`. S3 destinations
never offer bulk object deletion from this UI; the copy explains that objects
must be removed from the bucket, and offers `Copy the prefix` so the user can do
it with their provider's tools.

---

## 9. Storage providers

A `StorageProvider` is credentials + endpoint, defined once and reused. The
whole screen exists to make that reuse visible, so that rotating one key is
understood as touching many buckets.

### 9.1 Providers list `P-1`

Header: 240px search, `New provider` (primary).

Table, 44px rows:

| Column | Width | Content | Drop @900 |
|---|---|---|---|
| Flavour | 36 | Icon; tooltip `S3Flavour::title()` | keep |
| Name | 220 | `body.strong`; `small` `text.muted` second line with `notes` when set | keep |
| Endpoint | remainder | `mono.small`, `<endpoint>` + ` · ` + `<region>`; a 14px `shield-off` in warning tint when `tls == false` | keep |
| Used by | 130 | `<n> destinations` count pill; **`Not used yet`** in `text.muted` when zero | keep |
| Last verified | 120 | Relative or `Never` | 1st |
| Actions | 96 | `Test` compact + `⋯` | keep |

`⋯`: `Test connection`, `Edit…`, `Rotate keys…`, `Duplicate`, separator,
`Delete…` (danger, blocked when in use — see 9.5).

Empty state: `empty.providers.*`, primary `Add a storage provider`, and a
`small` `text.muted` line explaining that providers are only needed for S3 —
local and OneDrive destinations do not use one.

### 9.2 Provider editor `P-2`

Pushed screen or in-modal sub-flow, same rules as T-2.

**Identity group**
- `Name` (400px, required, unique). Placeholder shows the shape the user should
  aim for: `StorJ eu-1 (personal)`.
- `Notes` (400px multiline, 3 rows, optional) — `small` helper: what this
  account is for, so future-you knows.

**Connection group**
- `Provider type` — a combo over `S3Flavour::all()` rendered with `title()`.
  Selecting one applies `default_endpoint()` and `default_region()` to the
  fields **only when they are empty or still hold the previous flavour's
  defaults**, and sets `path_style` from `wants_path_style()`. A `small`
  `text.muted` line confirms what was filled in: `Endpoint and region filled in
  for StorJ. Change them if your account differs.`
- `Endpoint` (400px, `mono`, required). Helper shows the parsed form:
  `https://gateway.storjshare.io — TLS on, port 443`. Accepts values with or
  without a scheme, matching the model's comment; a missing scheme is shown as
  normalised, not rejected.
- `Region` (200px, required for AWS-style flavours, optional otherwise; helper
  names which).
- Toggle `Use TLS` (`tls`), default on. Turning it off shows a persistent
  `warning` line: credentials and data would travel unencrypted; only reasonable
  for a MinIO instance on localhost.
- Toggle `Path-style addressing` (`path_style`). Helper: required by MinIO and
  some gateways; StorJ and AWS accept the default. Auto-set by flavour, and the
  helper notes when the value came from the flavour default.

**Credentials group** (or the inline unlock prompt when locked)
- `Access key ID` — a normal `TextEdit`, 400px. Not masked: an access key ID is
  an identifier, and masking it makes verification harder for no benefit.
- `Secret access key` — passphrase field, 400px, with reveal.
- `Session token` (optional, collapsed under a `Use a session token` checkbox) —
  passphrase field, multiline (3 rows), because STS tokens are long.
- On **edit** of an existing provider, the secret fields show
  `••••••••••••` as a placeholder with a `small` `text.muted` line
  `Stored in your vault. Leave blank to keep it.` and a `Replace…` ghost button
  that clears the field for entry. The stored value is never rendered.
- A `small` `text.muted` footnote states where these go:
  `Stored in your encrypted vault and passed to kopia through the environment,
  never on a command line.` This is true of the implementation and worth saying.

**Footer**: `Test connection` (secondary) and `Save provider` (primary).
Saving without testing is allowed but shows a confirm the first time
(`copy: provider.save_untested`).

### 9.3 Test connection `P-3`

Inline result panel under the credentials group, 44px collapsed / auto
expanded. Steps shown as a checklist, same pattern as repository creation:
`Resolving the endpoint` · `Negotiating TLS` · `Signing a request` ·
`Listing buckets`.

Success: success tint panel, `Connected. Found 7 buckets.` with a
`Show buckets` disclosure listing them in `mono.small` (max 20, then
`and N more`). Sets `last_verified_at`.

Failures, each with a specific message rather than a raw dump:

| Cause | Message | Action |
|---|---|---|
| DNS failure | `That endpoint could not be found.` + the host in `mono` | `Check the endpoint` focuses the field |
| TLS failure | `The secure connection could not be established.` + the OpenSSL/rustls reason | `Turn off TLS` (only offered for `localhost`/private IPs) |
| 403 / signature | `The endpoint answered, but rejected these credentials.` | `Check the keys` |
| 403 on list, 200 on head | `These credentials work, but they cannot list buckets. You can still use a bucket by typing its name.` (info, not danger) | `Continue anyway` |
| Timeout | `The endpoint did not answer within 15 seconds.` | `Retry` |
| Wrong addressing style | `The endpoint answered but did not recognise the bucket path. Some gateways need path-style addressing.` | `Turn on path-style addressing` |

Every failure panel offers `Copy diagnostic details`, which copies a redacted
block (through `redact::scrub`) containing the endpoint, region, flavour,
`path_style`, TLS state, the HTTP status, and the error code — never the keys.

### 9.4 Impact display — "used by N destinations"

Present in three places, always computed from `Config::destinations_using()`:

1. The list column.
2. A 44px `bg.raised` strip at the top of the provider editor:
   `Used by 3 destinations across 5 jobs.` with a `Show them` disclosure that
   expands a 3-column read-only table (Destination · Bucket / prefix · Jobs).
3. Inside the key-rotation and delete modals, as the list of things that will be
   affected.

Destinations that override credentials
(`DestinationKind::S3 { credential_override: Some(_) }`) are listed separately
under the heading `Not affected — these use their own key pair`, with their own
count. This distinction is the whole reason the override exists, and hiding it
would make rotation dangerous.

### 9.5 Key rotation `P-4`

Reached from `⋯ → Rotate keys…` or a `Rotate keys` button in the editor.
Medium modal, three internal steps, no nesting (design system L13).

**Step 1 — Impact.** The affected/unaffected destination lists from §9.4, plus
a `warning` banner stating the sequence honestly: superbackup cannot create keys
at the provider; the user creates a new key pair in their provider's console,
enters it here, superbackup verifies it against every affected destination, and
only then replaces the stored one. The old key stays valid until the user
revokes it themselves, and the last screen reminds them to.

**Step 2 — New credentials.** New `Access key ID`, `Secret access key`, optional
`Session token`. A `Verify against all destinations` primary button runs a
`HeadBucket` plus a prefix listing for each affected destination and shows a
per-destination checklist with pass/fail. Any failure blocks step 3, with
`Continue anyway` available only when at least one destination passed and with a
danger-tinted explanation of what will break.

**Step 3 — Done.** Confirms the vault was updated, states that jobs will use the
new key from their next run, and shows a checklist-style reminder with the old
key ID (`mono`, truncated) and the sentence
`Revoke this key in your provider's console when you are ready.` plus a
`Copy key ID` button. An `Event` of kind `provider.keys_rotated` is written.

Rotation is atomic in the vault: either every affected `SecretRef` is updated or
none is. If the vault write fails, the modal shows the error and states
explicitly that nothing was changed.

### 9.6 Provider deletion

Blocked while `destinations_using()` is non-empty: the danger modal lists the
destinations and the primary button is disabled with the reason
(`copy: provider.delete.in_use`). The modal offers `Go to destinations`.

When unused, deletion is a normal danger confirmation naming the provider and
stating that the stored keys are removed from the vault and that nothing at the
provider is touched.

---

## 10. Activity

### 10.1 Activity screen `A-1`

Two views behind a segmented control in the header: **Runs** (default) and
**Events**. Header also holds a 240px search field, a time-range combo
(`Last 24 hours` / `Last 7 days` / `Last 30 days` / `All (200 runs)`), and an
`Export…` ghost.

`MAX_HISTORY` is 200, and the UI says so rather than pretending to be infinite:
the `All` option is labelled `All (200 runs)` and a `small` `text.muted` line
under the table reads `superbackup keeps the last 200 runs. Older activity is in
the event log.`

**Runs table**, 36px rows, newest first:

| Column | Width @1100 | Content | Drop @900 |
|---|---|---|---|
| Status | 32 | Status icon, tooltip `RunStatus::title()` | keep |
| Started | 130 | `DD MMM HH:MM`, `mono.small`; today's runs show `HH:MM` plus `today` | keep |
| Job | 180 | `body.strong` `JobRun.job_name` (the stored name, so a renamed job's history stays honest) | keep |
| Trigger | 90 | `Trigger` as a word: `Schedule` / `Manual` / `Command line` / `File change` / `Catch-up` / `Retry` | 2nd |
| Destinations | 160 | One 14px status dot per `DestinationRun`, in order, plus `<n succeeded>/<n>`; tooltip lists each destination and its status | keep |
| Duration | 80 | `duration_seconds()` formatted, `mono.small`, right | 3rd |
| Uploaded | 90 | Sum of `bytes_uploaded`, `mono.small`, right | 1st |
| | 32 | `chevron-right` | keep |

Filters as a chip row under the header when active: `Job: Dev code ×`,
`Status: Failed ×`, `Destination: StorJ offsite ×`, plus `Clear all`. Filters
are set from the combos, from a row's context menu (`Show only this job`), and
from deep links (dashboard `View error`, tray notification click).

Row click opens `A-2`. The failed-run rows carry a 3px `danger.mark` left spine.

**Events table** (`Event`), 28px compact rows:

| Column | Width | Content |
|---|---|---|
| Severity | 28 | 14px dot in the severity colour + shape (Debug `circle-dashed`, Info `circle`, Warning `alert-triangle`, Error `x-octagon`) |
| Time | 130 | `DD MMM HH:MM:SS`, `mono.small` |
| Kind | 160 | `Event.kind` in `mono.small` (`job.started`, `repo.created`, `vault.unlocked`, `service.error`) |
| Message | remainder | `Event.message`, single line, elided |
| Job | 130 | Job name resolved from `job_id`, or `—` |

A severity filter (`All` / `Info and above` / `Warnings and errors` /
`Errors only`) sits with the search field; the default is `Info and above`, and
`Debug` rows are only reachable by choosing `All`, which shows a `small`
`text.muted` note that debug events are only recorded when
`Settings.log_level` is `Debug` or `Trace`.

Row click expands the row in place to 200px showing `Event.fields` as a
key/value block in `mono.small` plus the `run_id` / `destination_id` as links.

**Export…**: a small modal offering `Runs as CSV`, `Events as NDJSON` (the raw
`events.ndjson` slice), and `Diagnostic bundle…` (jumps to Settings →
Diagnostics). All exports pass through `redact::scrub`, and the modal says so.

### 10.2 Run detail `A-2`

Pushed screen, breadcrumb `Activity / <job name> — <started at>`.

**Summary card** (full width, 132px): status badge, `h1` job name, and a
key/value grid in two columns:

```
Status        Completed with warnings        Started    12 Mar 02:00:04
Trigger       Schedule                       Finished   12 Mar 02:04:16
Duration      4m 12s                         Run id     3f9c…  [copy]
Destinations  2 succeeded, 1 failed          Job id     a12b…  [copy]
```

`Status` uses `JobRun.status`; a `small` `text.muted` note under it explains a
partial outcome when the status is `SucceededWithWarnings`: `Some destinations
did not complete. See below.` — the UI never flattens the fan-out.

**Per-destination sections**, one card each, in the order they appear in
`JobRun.destinations`. Header row: kind icon, destination name (`h2`), status
badge, and the destination's duration. Body is a key/value block:

```
Snapshot      k9f2ab7c31de…                   [copy]  [Browse this snapshot]
Files         84,214 processed · 71,090 unchanged · 12 skipped
Data          9.1 GB read · 842 MB uploaded
Throughput    18.2 MB/s average
Started       02:00:06        Finished  02:03:58
```

- `files_cached` is rendered as `unchanged`, which is what it means to a user.
- `errors_ignored` renders as `skipped` and links to the warnings list.
- `snapshot_id` is present only when one was created; otherwise the row reads
  `No snapshot was created`.
- `Browse this snapshot` jumps to `R-3` scoped to that snapshot.

**Warnings** (`DestinationRun.warnings`): a collapsed disclosure
`<n> warnings` opening a code block (design system §8.18) with one line per
warning, `mono.small`, max height 240px, with a `copy` button.

**Error** (`DestinationRun.error: RunError`): a danger-tinted card:

```
[x-octagon]  Failed
h2:      RunError.message
small:   Error code: kopia · 12 Mar 02:03:58
banner:  RunError.hint   (info tint, only when Some)
disclosure "Show technical details" →
         code block with RunError.detail (already redacted by the driver)
actions: [Retry this job]  [Copy details]  [Open the destination]
```

The `ErrorCode` is shown as a `mono.small` value, not hidden, because it is the
stable identifier a user will paste into an issue. `RunError.detail` is
displayed exactly as stored — the driver has already redacted it — and the UI
adds a `small` `text.muted` line saying credentials were removed, so nobody
wonders where the output went.

**Footer actions**: `Retry this job`, `Browse snapshots`, `Copy run summary`
(a plain-text block for pasting into a bug report, redacted).

---

## 11. Restore

A backup tool is judged here. The flow is: choose what to restore *from*,
browse, select, choose where it goes, confirm, watch, verify.

### 11.1 Entry `R-1`

The Restore screen opens on a **source picker** rather than jumping straight
into a file tree, because "restore" usually starts with "which copy".

Layout: a 280px left pane and the remainder as the right pane.

**Left pane — sources.** A list of repository destinations, grouped by job:

```
Dev code
  [🖴] Local repo            18 snapshots   newest 2h ago
  [☁] OneDrive              18 snapshots   newest 2h ago
  [🗄] StorJ offsite         17 snapshots   newest 2h ago
Documents
  [🖴] Local repo             9 snapshots   newest yesterday
```

Rows are 48px, selectable, showing kind icon, destination name, snapshot count
and newest snapshot age. `LocalMirror` destinations appear in a separate group
at the bottom titled `Folder mirrors` with the note
`Open these in your file manager — there is nothing to restore from.` and an
`Open folder` action instead of selection.

An unreachable destination shows a danger dot and, on selection, a full-pane
error state with `Verify` and `Edit destination` actions.

**Right pane — snapshots `R-2`.** A table of snapshots for the selected
destination, 36px rows:

| Column | Width | Content |
|---|---|---|
| When | 150 | `DD MMM HH:MM` + relative in `text.muted` |
| Source | remainder | The source root the snapshot covers, `mono.small`, elided |
| Files | 90 | count, `mono.small`, right |
| Size | 90 | `mono.small`, right |
| Id | 120 | first 12 chars, `mono.small`, `copy` on hover |

A `Compare with previous` ghost on row hover opens a small modal listing added /
changed / removed counts — cheap to compute from Kopia and disproportionately
reassuring.

Above the table, a **date filter strip**: `All` / `Today` / `This week` /
`This month` / a date picker, plus a `small` `text.muted` line
`Retention keeps 10 latest, 24 hourly, 14 daily, 8 weekly, 12 monthly, 3 annual
snapshots.` rendered from the effective `RetentionPolicy`, so the user
understands why old snapshots are absent.

Selecting a snapshot opens the browser.

### 11.2 Snapshot browser `R-3`

Breadcrumb header: `Restore / <destination> / <snapshot time>`, then a second
28px path breadcrumb bar inside the content area:
`⌂ / Users / andreas / dev / web / src` with each segment a click target and
`Alt+←` / `Alt+→` navigating history.

Header actions: a 240px `Filter files` field (matches within the current
directory, not a global search), a `Show hidden files` toggle, and
`Restore selected` (primary, disabled at zero selection, label shows the count:
`Restore 3 items`).

**The listing** is a flat, virtualised table of the current directory only
(design system L4), 28px compact rows:

| Column | Width @1100 | Content |
|---|---|---|
| Select | 32 | Checkbox; the header cell is a tri-state select-all for the current directory |
| Name | remainder | 14px `folder` / file-type icon + name; folders sort first; folder names are click targets that navigate |
| Size | 90 | `mono.small`, right; folders show `—` until expanded once, then their computed total |
| Modified | 130 | `DD MMM HH:MM`, `mono.small` |
| | 32 | `⋯` → `Restore this…`, `Restore this to…`, `Copy path`, `Show in previous snapshot` |

Selection rules:
- Selecting a folder selects the whole subtree; the checkbox shows a filled
  square (not a tick) to signal "everything inside".
- Selection survives navigation. A 36px `bg.raised` strip appears above the
  table once anything is selected: `3 items selected · about 1.2 GB` with a
  `Show selection` disclosure listing the chosen paths and a `Clear` link.
- `Ctrl/Cmd+A` selects everything in the current directory only, and the strip
  says so.

**Time travel**: a `Snapshot` combo in the path bar's right end showing the
current snapshot's time. Changing it stays in the same directory when that
directory exists in the target snapshot; when it does not, the browser walks up
to the nearest existing ancestor and shows a `small` `text.muted` note saying
which directory it landed in and why.

**Empty directory**: inline empty state inside the table, 120px, `folder-open`
icon, `This folder was empty in this snapshot.`

**Loading**: rows render as 28px skeleton bars (`bg.raised`, radius 4) rather
than a spinner, so the layout does not jump when data lands. First load of a
huge directory shows `Reading directory…` after 400 ms.

### 11.3 Restore options `R-4`

Medium modal, opened by `Restore selected`.

1. **What** — a read-only summary: `3 items · about 1.2 GB · from <snapshot
   time>` with a disclosure listing the paths.
2. **Where** — radio:
   - `Back to the original location` — shows the resolved target path in `mono`.
     Selecting this makes the conflict question mandatory (below).
   - `To another folder` — path field + `Browse…`, defaulting to
     `<Downloads>/superbackup-restore-<YYYYMMDD-HHMM>`. A checkbox
     `Recreate the full folder structure`, default on; when off, files land flat
     and a `small` warning notes that same-named files from different folders
     will collide.
3. **If a file already exists there** — radio, no default preselected when
   restoring to the original location (the user must choose):
   - `Skip it` — `small`: leaves what is on disk untouched.
   - `Overwrite it` — `small` in danger tint: replaces the file on disk; this
     cannot be undone.
   - `Keep both` — `small`: restores as `name (restored 12 Mar 14:02).ext`.
4. **Also restore** — two checkboxes: `File timestamps` (default on) and
   `Permissions and ownership` (default on; disabled with an explanation on
   Windows where it is not meaningfully portable).
5. A `small` `text.muted` line naming the free space at the target and warning
   when the estimate exceeds it, which disables the primary button.

Footer: `Cancel` and `Restore` (primary; **Danger** variant when `Overwrite it`
is chosen, with the label `Overwrite and restore`).

Choosing `Overwrite it` with the original location additionally requires a
typed confirmation of the word `overwrite` in a 200px field that appears
directly above the footer. This is the only destructive path in the product that
writes over live user data, and it earns the friction.

### 11.4 Restore progress and result `R-5`

The modal converts in place into a progress view (no new modal, design system
L13):

- 8px determinate bar, `<n> of <m> files · <bytes> of <bytes> · <rate>`.
- Current file path, `mono.small`, `text.muted`, elided from the left.
- `Cancel restore` (danger ghost). Cancelling asks for confirmation and states
  that already-restored files stay where they are; it does not roll back.

On completion the modal shows the result:

| Result | UI |
|---|---|
| All restored | `check-circle-2` success, `Restored 3 items (1.2 GB) to <path>`, actions `Open folder` (primary) and `Done` |
| Partial | `alert-triangle` warning, `Restored 2 of 3 items`, a table of the failures with per-file reasons, actions `Retry failed items`, `Copy the list`, `Open folder`, `Done` |
| Failed | `x-octagon` danger, the `RunError.message`, `hint` when present, `Show technical details` disclosure, actions `Retry`, `Copy details`, `Done` |

A restore writes `Event`s (`restore.started`, `restore.finished`) so it appears
in Activity, and finishing raises a notification when the window is hidden
(§15).

### 11.5 Restore while locked

The entire Restore screen is replaced by a centred unlock panel (§3.2). No
snapshot metadata is shown, because listing snapshots needs the repository
passphrase and showing a stale cached tree would be a lie.

### 11.6 Restore empty states

| Condition | State |
|---|---|
| No repository destinations exist | `empty.restore.no_destinations` — icon `history`, primary `Add a destination` |
| Destinations exist, no snapshots yet | `empty.restore.no_snapshots` — primary `Back up now` |
| Selected destination unreachable | Error state with `Verify` and `Edit destination` |
| Only folder mirrors exist | `empty.restore.mirrors_only` with an `Open folder` action per mirror |

---

## 12. Settings

A two-pane screen: a 200px section list on the left (not the rail — this is
within-screen navigation), content on the right. Sections in this order. Every
setting maps to a field on `Settings`, `NotificationSettings`,
`BandwidthSettings` or `RemoteConfigSource`.

All settings apply **immediately** and are saved on change (hence toggles, not
checkboxes — design system §8.4). Text and numeric fields commit on blur or
Enter, and show a 1.5-second `Saved` inline confirmation in `success.mark`
`small` beside the label. There is no global Save button, and no Cancel; the two
settings that cannot work that way (master passphrase change, vault reset) are
explicit modal flows.

### S-1 General

- `Machine label` (400px) — `MachineIdentity.label`. Under it, read-only
  `small` `text.muted`: `Folder name: <slug>` in `mono.small` with the note
  `The folder name is fixed for this install and does not change with the
  label.` — true to the model comment, and it prevents a nasty surprise.
- Read-only key/value block: `Machine id` (full UUID, `mono.small`, copyable),
  `Host name`, `Operating system` (`<os> <os_version>`), `Architecture`,
  `User`, `First set up` (`created_at`).
- `Theme` — 3-segment control (`System` / `Light` / `Dark`), bound to
  `Settings.theme`.
- `Start superbackup when I sign in` toggle (`start_at_login`).
- `Start minimised to the tray` toggle (`start_minimised`), indented, disabled
  when the parent is off.
- `Run backups as a background service` toggle (`run_as_service`) with the
  elevation and keychain explanation from O-6, plus a live status line:
  `Service: installed and running` / `installed, not running` /
  `not installed`, and `Install` / `Start` / `Uninstall` buttons as appropriate.
- `Maximum jobs running at once` — `DragValue` 1–8 (`max_parallel_jobs`), with
  `small`: kopia already uses many threads inside one snapshot, so more than
  two rarely helps and can make everything slower.
- A `Quit superbackup` danger-ghost button at the bottom with `small` noting
  that quitting stops all scheduled backups until it is started again.

### S-2 Scheduling

- `Run schedules that were missed while the computer was off` toggle
  (`run_missed_on_start`), `small` explaining catch-up runs are marked
  `Catch-up` in Activity.
- `Skip scheduled runs on a metered connection` toggle (`skip_on_metered`,
  default on) — `small` notes the run is recorded as `Skipped`, not failed, and
  that per-job overrides exist.
- `Skip scheduled runs on battery` toggle (`skip_on_battery`, default off).
- **Pause** panel (`PauseState`): when not paused, five buttons
  `1 hour` / `2 hours` / `4 hours` / `8 hours` / `Until I resume`, plus an
  optional 400px `Reason` field written to `PauseState.reason`. When paused, the
  panel becomes a `warning` banner: `Paused until 18:20 — <reason>` with
  `Resume now` and `Extend by 1 hour`.
- A read-only `Upcoming runs` table (next 10), columns: `When` (absolute +
  relative), `Job`, `Trigger` (`Schedule` / `Catch-up`), `Blocked by` (empty, or
  `Paused` / `Vault locked` / `Job disabled` in warning tint). This table is the
  answer to "why did nothing run last night".

### S-3 Bandwidth

Global `BandwidthSettings`.

- `Upload limit` — checkbox + `DragValue` with a `kB/s` suffix
  (`upload_kbps`, `None` = unlimited). Beside it, `small` `text.muted` showing
  the conversion: `≈ 16 Mbit/s`.
- `Download limit` — same (`download_kbps`). `small` notes downloads only occur
  during restores and repository maintenance.
- **Daily window** (`BandwidthSettings.schedule: Option<BandwidthWindow>`) —
  a toggle `Use a different limit during part of the day`. When on:
  - Two time controls `From` / `To`, each a pair of `DragValue`s, stored as
    `start_minute` / `end_minute` (minutes past local midnight).
  - Seven day toggles (`weekdays`, 0 = Monday); none selected means every day,
    and the helper says exactly that rather than leaving it ambiguous.
  - Its own `Upload` / `Download` checkbox + value pair.
  - A **24-hour strip** visualisation, 819 × 40px: a `bg.raised` track with hour
    ticks every 60px-equivalent, the window drawn as an `accent` @ 30% block
    with its limit label inside, and the out-of-window limit labelled at both
    ends. A window crossing midnight (`end_minute < start_minute`) renders as
    two blocks and the helper text confirms it wraps.
  - `small` summary sentence, always present:
    `Between 09:00 and 18:00 on weekdays, uploads are limited to 500 kB/s.
    Outside that window, uploads are limited to 2,000 kB/s.`
- A `small` `text.muted` footnote: limits are handed to kopia and apply per
  destination, so two destinations running at once can each use the limit.
  This is a real behaviour and hiding it would make the setting untrustworthy.

### S-4 Notifications

Bound to `NotificationSettings`.

- Master toggle `Show desktop notifications` (`enabled`). Everything below is
  disabled (not hidden) when off.
- `When a backup fails` toggle (`on_failure`, default on).
- `When a backup succeeds` toggle (`on_success`, default off), `small`: most
  people want silence when things work.
- `When a job has not succeeded for` — `DragValue` (`stale_after_days`, 0–90,
  suffix `days`), `small`: set to 0 to turn this off. A value of 0 also stops
  the dashboard and tray treating jobs as stale, matching
  `JobSummary::is_stale`.
- `When the background service has a problem` toggle (`on_service_error`).
- `Do not repeat the same problem within` — `DragValue`
  (`dedupe_minutes`, 0–1440, suffix `minutes`, default 60).
- A `Send a test notification` secondary button, and — where the OS can report
  it — a `warning` line when notifications are blocked at the OS level, with an
  `Open system settings` action.

### S-5 Security

- **Vault state** panel: `Unlocked` / `Locked` with `Lock now` or `Unlock…`.
- `Lock automatically after` — `DragValue` (`auto_lock_minutes`, 0–1440, suffix
  `minutes`), `small`: set to 0 to lock as soon as the window is closed. A
  `warning.tint` line appears whenever the value is 0 **and**
  `run_as_service`/`start_at_login` is on and the keychain is off, because in
  that combination no scheduled backup will ever run unattended. The line names
  that consequence directly.
- `Store the vault key in the <Windows Credential Manager / macOS Keychain /
  Secret Service>` toggle (`use_os_keychain`, default off). Turning it **on**
  opens a confirmation modal that states the trade in one paragraph: unattended
  runs stop needing a person, and anything that can run as this user can ask the
  OS for the key. The modal requires re-entering the master passphrase.
  Turning it **off** immediately purges the stored key and says so.
- `Change master passphrase…` — a medium modal: current passphrase, new
  passphrase with the strength meter, confirm. On success it re-wraps the vault,
  writes a fresh backup into `vault-backups/`, and shows a success screen
  reminding the user that repository passphrases derived from the master
  (`PassphraseSource::DerivedFromMaster`) are re-derived automatically and no
  repository needs re-creating. That reassurance is necessary or nobody will
  ever change their passphrase.
- `Export repository passphrases…` — danger-ghost. Requires the master
  passphrase, then writes a plain-text file the user chooses, listing each
  repository destination and its passphrase. The confirmation states in plain
  words that the file is unencrypted and should be treated like the passphrases
  themselves.
- **Vault backups** — read-only list of files in `Paths::vault_backup_dir()`
  (name, size, date), with `Open folder` and `Restore a backup…` (danger, typed
  confirmation, states the current vault will be replaced).
- **Danger zone** — `Reset the vault and start over`: destroys every stored
  secret. The modal enumerates what becomes unreachable (each repository
  destination by name), requires typing `superbackup`, and states that
  repositories whose passphrase was `Generated` or `DerivedFromMaster` cannot be
  opened again without the exported passphrase.

### S-6 Kopia binary

- **Status** panel: `Kopia <version> at <path>` (`mono`, elided, with
  `Open folder`), or a `danger` banner with `ErrorCode::KopiaMissing`'s message
  and hint.
- Radio: `Find it automatically` (default; `Settings.kopia_path = None`) /
  `Use a specific file` (reveals a path field + `Browse…`).
- `Check again` secondary — re-runs discovery and reports the result inline.
- `Download a tested build` secondary — fetches the pinned version into
  `Paths::bundled_kopia()`, showing a determinate progress bar, the exact version
  and the SHA-256 it verifies against (`mono.small`), and the download URL. It
  never runs an installer and never touches a system-wide kopia.
- A read-only `small` `text.muted` block naming the two directories superbackup
  keeps separate from the user's own kopia — `Paths::kopia_config_dir()` and
  `Paths::kopia_cache_dir()` — with `Open folder` links, and a one-line
  explanation that this is so the two never fight over `repository.config`.
- **Compatibility**: a `warning` banner when the discovered version is outside
  the tested range, naming both the found and the tested version, and stating
  that superbackup will still try.

### S-7 Remote configuration

Bound to `RemoteConfigSource`. Header explains the model in two sentences: the
file kept in the repository is the **sealed vault**; the plain `config.json` is
never pushed; the vault is opened only in memory after the master passphrase is
supplied.

- Toggle `Sync configuration from a Git repository` (creates/clears
  `Config.remote`).
- `Repository URL` (400px, `mono`), `Branch` (200px, default `main`),
  `File path in the repository` (300px, `mono`, default `config.sbvault`).
- `Authentication` radio → `RemoteAuth`:
  - `None — public repository or system credentials`
  - `Personal access token` → a passphrase field stored as `token_ref`, plus a
    `small` line naming the minimum scope needed (read access to the repository;
    write only if publishing is enabled).
  - `SSH key` → path field + `Browse…` (`key_path`), `small` noting the key is
    read from where it is and never copied into the vault.
- `Check for changes automatically` toggle (`auto_pull`) + `DragValue`
  `every <n> minutes` (`pull_interval_minutes`, default 60).
- `Allow publishing from this machine` toggle (`allow_push`, default off) with
  `small`: publishing is always an explicit act; nothing is pushed
  automatically. When off, the `Publish` button below is disabled with that
  reason as its tooltip.
- **Trusted signers** (`trusted_signers`): a chip list of fingerprints with an
  `Add fingerprint…` field. `small`: when this list is not empty, a pulled vault
  whose signature does not verify against one of these is rejected. An empty
  list shows a `warning` line saying any vault at that URL will be accepted.
- **Status** panel: `Last pulled <relative>` (`last_pull_at`), `Commit
  <last_known_commit[..8]>` (`mono.small`, copyable), and one of:
  `Up to date` / `<n> changes available` (info tint) / `Never pulled` /
  the last error in danger tint.
- Buttons: `Pull now` (secondary), `Publish…` (secondary, gated by
  `allow_push`), `Open the repository` (ghost, `external-link`).

**Pull flow** — a medium modal, three phases:
1. Fetching and verifying. Shows the commit, author and date, and the signature
   verification result against `trusted_signers`.
2. **Diff review** — a read-only three-column change list (`Added` /
   `Changed` / `Removed`) over providers, destinations and jobs, by name, with
   the specific fields that changed for each. A `warning` banner appears when
   the pull would remove a destination that jobs use, or a provider that
   destinations use, naming them.
3. Apply. A `small` line states what is **not** touched: local run history and
   `state.json`, per the model's own separation of config and state.

Conflicts (local edits since the last pull) are surfaced in phase 2 as a
`Local changes will be replaced` warning listing the affected objects, with
`Save a copy of my config first` (writes a timestamped `config.json` next to
the current one) offered before `Apply`.

### S-8 Advanced

- `Log level` — combo over `LogLevel` (`Error` / `Warn` / `Info` / `Debug` /
  `Trace`), with `small` warning that `Debug` and `Trace` write a lot and may
  include file paths.
- `Keep logs for` — `DragValue` (`log_retention_days`, 1–365, suffix `days`).
- **File locations** — read-only key/value list with `Open folder` buttons:
  config directory, data directory, log directory, cache directory,
  `config.json`, `config.sbvault`, `state.json`, `events.ndjson`. All in
  `mono.small`, middle-elided, full path in the tooltip.
- `Clear the kopia cache` secondary, with the current cache size and a `small`
  note that the next run will be slower.
- `Export a diagnostic bundle…` — a modal listing exactly what goes in
  (config with secrets removed, the last 200 runs, the event log tail, kopia
  version, OS details, the last 2,000 log lines) and what does not (any secret,
  any file content, any file name from your sources). It states that everything
  passes through the redaction filter, and offers `Preview the bundle` before
  writing the zip.
- `Run diagnostics` (`stethoscope`) — the equivalent of `superbackup doctor`,
  rendered as a checklist with per-check pass/warn/fail and a `Fix` action where
  one exists: kopia present and runnable; vault readable; config schema version
  (`CONFIG_SCHEMA_VERSION`); every destination reachable; every provider
  verified; disk space at each local destination; service state; daemon IPC
  endpoint; clock skew against the S3 endpoint (a classic cause of signature
  failures).

### S-9 Reset

Separated so it is never adjacent to a routine control.
`Reset all settings to their defaults` (does not touch jobs, destinations,
providers or the vault) and `Remove all configuration and start over` (typed
confirmation `superbackup`, enumerates what is deleted and what is left on disk
at each destination).

---

## 13. About `AB-1`

A single centred 560px column, 24px padding.

1. 64px product mark, product name in `display`, `superbackup <VERSION>` in
   `body` `text.muted`, and the build line in `mono.small`:
   `<target_os>-<target_arch> · built <date>` from `BuildInfo`.
2. A one-sentence description.
3. Key/value block: `Kopia` (version and path, or `Not found`),
   `Machine` (label and slug), `Config` (schema version), `Data folder`
   (`Open folder`).
4. **Licences** — a card with a `small` block:
   - `superbackup is released under the MIT licence.` + `View licence`
     (opens the bundled `LICENSE`).
   - `superbackup uses Kopia, which is released under the Apache Licence 2.0.
     Kopia is a separate program; superbackup runs it and does not modify it.` +
     `View the Apache 2.0 licence` and `kopia.io`.
   - `Third-party licences` — a disclosure opening a scrollable, searchable list
     generated at build time by `cargo-about`, one entry per crate with its
     name, version, licence identifier and full text. Icons: Lucide (ISC).
     Fonts: Inter and JetBrains Mono (SIL Open Font Licence 1.1).
   - A `Copy all licence text` button, because that is what someone auditing a
     build actually wants.
5. **Links** row of ghost buttons with `external-link` icons: `Website`,
   `Documentation`, `Report an issue` (pre-fills version and OS in the URL),
   `Kopia documentation`, `Release notes`.
6. `small` `text.muted` copyright line: `© 2026 Andreas Wiren`.

The Kopia attribution is mandatory and appears in three places: here, the
onboarding welcome footnote, and the diagnostic bundle's `README`.

---

## 14. Tray icon and menu

The tray is the primary interface for most of this application's life.

### 14.1 Icon

The five `Health` states, drawn exactly as specified in `DESIGN_SYSTEM.md` §7.
The icon is driven by `StatusSnapshot.health`, which the daemon computes with
`StatusSnapshot::derive_health`. The GUI and the tray never derive it
independently.

Tooltip: `DESIGN_SYSTEM.md` §7.5.

Click behaviour:

| Gesture | Windows | macOS | Linux |
|---|---|---|---|
| Left click | Show/focus the window | Open the menu (platform convention) | Show/focus the window |
| Right click | Open the menu | Open the menu | Open the menu |
| Double click | Show/focus the window | — | Show/focus the window |

### 14.2 Menu — idle

Widths are OS-native; items marked `›` are submenus. Disabled items stay
visible and are never removed, so the menu's shape is stable and muscle memory
works.

```
superbackup — Up to date                      (header item, disabled)
Last backup 2 hours ago                       (header item, disabled)
──────────────────────────────────────────────
Back up now                                   (runs all enabled jobs)
Back up                                     › (one item per enabled job)
──────────────────────────────────────────────
Pause                                       › 1 hour
                                              2 hours
                                              4 hours
                                              8 hours
                                              Until I resume
Disable all jobs                              (checkbox item)
──────────────────────────────────────────────
Open superbackup
Activity…
Settings…
──────────────────────────────────────────────
Quit superbackup
```

- The two header items show `Health::title()` and a context line matching the
  tray tooltip's second line.
- `Back up ›` lists enabled jobs by name with their last-result icon; more than
  12 jobs collapses to the first 12 plus `More…` which opens the Jobs screen.
- `Disable all jobs` is a **checkbox item**, not a button: it reflects the state
  of `Job.enabled` across all jobs (ticked when every job is disabled, and the
  platform's mixed state where supported). Unticking it re-enables exactly the
  jobs it disabled — the set is remembered — and never enables jobs the user had
  disabled by hand.
- `Pause` and `Disable all jobs` are different things and the menu keeps them
  apart deliberately: pause is time-boxed and global (`PauseState`); disabling
  is per-job and indefinite (`Job.enabled`).

### 14.3 Menu — while a job is running

This is the state the brief asks to be pinned down exactly. When
`active_runs` is non-empty:

```
superbackup — Backing up                      (disabled)
Dev code — 42% · 18.2 MB/s · ~3m left         (disabled)
──────────────────────────────────────────────
Stop “Dev code”                               (one item per active run)
Stop all backups                              (only when 2+ runs are active)
──────────────────────────────────────────────
Back up now                                   (DISABLED while any run is active,
                                               with the suffix “(already running)”)
Back up                                     › (each job that is running is
                                               disabled and suffixed “(running)”;
                                               others remain enabled)
──────────────────────────────────────────────
Pause                                       › 1 hour / 2 / 4 / 8 / Until I resume
                                               (pausing does NOT stop the current
                                               run; the submenu's header item
                                               reads “Current backups finish first”)
Disable all jobs
──────────────────────────────────────────────
Open superbackup
Activity…
Settings…
──────────────────────────────────────────────
Quit superbackup                              (opens a confirmation, see below)
```

Rules while running:
- The second header line updates at most **once per second** and shows the
  job with the highest `overall_fraction()` when several run. With more than one
  active run it reads `2 backups running — 42%` and the per-run detail moves
  into the `Stop “…”` item labels: `Stop “Dev code” (42%)`.
- `Stop “<job>”` shows a confirmation **only** when the window is visible; from
  the tray it acts immediately and raises a toast/notification saying what was
  stopped and that the partial snapshot was discarded. A tray menu is a
  fire-and-forget surface; a modal that the user cannot see is worse than no
  confirmation.
- `Quit superbackup` while a run is active opens a small window-level
  confirmation naming the running jobs: `Quit and stop 1 backup?` with
  `Keep running` / `Quit and stop`.
- Progress is never rendered as a bar in the menu — no platform supports it
  reliably. It is text, and it is precise.

### 14.4 Menu — other states

| State | Differences |
|---|---|
| **Paused** | Header lines: `superbackup — Paused` / `Paused until 18:20` (or `Paused until you resume`, plus `PauseState.reason` when set, elided to 48 chars). The `Pause ›` submenu is replaced by a single `Resume backups` item at the same position, plus `Extend ›` with the same five durations. `Back up now` stays **enabled** — a manual run is an explicit act and pause is about schedules. |
| **Locked** | Header: `superbackup — Needs attention` / `The vault is locked`. `Unlock…` is inserted as the first action item and opens the window with `V-1`. `Back up now`, `Back up ›` and every stop item are disabled with the suffix `(vault locked)`. |
| **Failed** | Header: `superbackup — Backup failed` / `Dev code failed 20 minutes ago`. An item `View the error…` is inserted first, opening `A-2` for that run. |
| **Attention (stale)** | Header second line: `Dev code has not succeeded for 4 days`. `Back up “Dev code”` is inserted as the first action item. |
| **Kopia missing** | Header second line: `Kopia was not found`. First action item `Fix in Settings…`, and all run items are disabled with the suffix `(kopia not found)`. |
| **Daemon unreachable** | Header: `superbackup — Not running`. Only `Open superbackup`, `Start the background service` and `Quit` are enabled. |

### 14.5 Tray accessibility

The tray icon exposes an accessible name equal to the tooltip's first line and a
description equal to the second. Every menu item's accessible name includes its
state suffix (`, disabled, vault locked`). On Windows the menu is a standard
`HMENU` and inherits system keyboard navigation; item mnemonics are assigned
and stable: `B`ack up now, `P`ause, `O`pen, `S`ettings, `Q`uit.

---

## 15. Notifications

Desktop notifications via `notify-rust`. Governed by `NotificationSettings`.

### 15.1 What fires

| Event | Setting gate | Severity | Title | Body | Click action |
|---|---|---|---|---|---|
| Job failed | `on_failure` | Critical/normal | `Backup failed: <job>` | `<RunError.message>` (first 120 chars, already redacted) | Opens `A-2` for that run |
| Job partially failed (`SucceededWithWarnings` **with** a failed destination) | `on_failure` | Normal | `Backup finished with problems: <job>` | `Succeeded to 2 of 3 destinations. <destination> failed.` | Opens `A-2` |
| Job succeeded | `on_success` | Low | `Backup finished: <job>` | `<n> files · <bytes> uploaded · <duration>` | Opens `A-2` |
| Job stale | `stale_after_days > 0` | Normal | `<job> has not backed up for <n> days` | `Last success <date>.` | Opens the Dashboard |
| Service error | `on_service_error` | Normal | `superbackup service problem` | `<Error message>` | Opens Settings → Diagnostics |
| Kopia missing at run time | `on_service_error` | Normal | `Kopia was not found` | `Backups cannot run until this is fixed.` | Opens Settings → Kopia binary |
| Vault locked when a schedule was due | `on_service_error` | Normal | `A backup was skipped` | `<job> was due at 02:00. Unlock superbackup to run it.` | Opens `V-1` |
| Restore finished | always, when the window is hidden | Normal | `Restore finished` | `<n> items restored to <folder>` | Opens the target folder |
| Remote config changed | always, when `auto_pull` finds changes | Low | `Configuration changes are available` | `<n> changes on <branch>.` | Opens Settings → Remote config |

### 15.2 What does not fire

Deliberately silent, all of it:

- Job started, queued, or skipped by policy (metered, battery, disabled, paused)
  — these are `RunStatus::Skipped` and belong in Activity, not on screen.
- Any run triggered manually **while the window is visible** — the user is
  looking at the result already; a toast covers it.
- Progress milestones of any kind.
- Vault locked or unlocked by the user's own action.
- Successful verification, successful provider test, successful maintenance.
- Anything at all while the OS reports Do Not Disturb / Focus Assist, except
  `Critical` severity, which on Windows means only `on_failure` notifications
  when `consecutive_failures >= 3`.
- Config saved, job created, job edited.

### 15.3 Dedupe

`NotificationSettings.dedupe_minutes` (default 60). The dedupe key is
`(kind, job_id, destination_id, ErrorCode)`. Within the window, a repeat is
suppressed entirely — not coalesced into a counter, because a "3 more failures"
notification tells the user nothing new. A single `Event` is still written for
every occurrence, and the suppressed count appears in Activity.

Additional rules:
- A **success after failure** always fires when `on_failure` is on, even if
  `on_success` is off, with the title `Backup recovered: <job>`. Recovery is
  news.
- A change of `ErrorCode` for the same job resets the dedupe window; a different
  failure is a different problem.
- At most **3** notifications in any 60-second window across the whole
  application; further ones are dropped and a single summary fires:
  `<n> backups need attention` linking to the Dashboard.
- Notifications never fire during the first 30 seconds after launch, so a
  cold start with three stale jobs does not carpet-bomb the user.

### 15.4 Click behaviour and presentation

- Every notification's click **shows and focuses the window** and navigates to
  the exact screen in the table above, with the relevant filter applied
  (Activity opens filtered to that job, not to everything).
- Where the platform supports actions, at most two are attached:
  failures get `Retry` and `Show details`; the stale notification gets
  `Back up now`; the locked notification gets `Unlock`.
- Notification bodies never contain a path from the user's sources, a secret, or
  raw kopia output. Everything passes through `redact::scrub` first.
- When notifications are disabled or blocked by the OS, the equivalent
  information always still appears as an in-app toast (if the window is open),
  the tray icon state, and an `Event`. No information exists only in a
  notification.

---

## 16. Empty, error and destructive states — consolidated

### 16.1 Empty states

| Screen | Condition | Icon | Primary | Secondary | Copy key |
|---|---|---|---|---|---|
| Dashboard | No jobs | `repeat` | Create your first job | Import from another machine… | `empty.jobs` |
| Dashboard | Jobs, none run | — | Back up now | — | inline |
| Jobs | No jobs | `repeat` | New job | — | `empty.jobs` |
| Jobs | Filter matches nothing | `search-x` | Clear filters | — | `empty.jobs.filtered` |
| Job editor · Sources | No sources | `folder` | Add folder… | — | `empty.sources` |
| Job editor · Destinations | No destinations exist | `hard-drive` | Add a destination | — | `empty.destinations.injob` |
| Destinations | None | `hard-drive` | Add a destination | Learn about the kinds | `empty.destinations` |
| Providers | None | `key-round` | Add a storage provider | — | `empty.providers` |
| Activity · Runs | No runs | `list` | Back up now | — | `empty.activity` |
| Activity · Runs | Filter matches nothing | `search-x` | Clear filters | — | `empty.activity.filtered` |
| Activity · Events | No events | `list` | — | — | `empty.events` |
| Restore | No repository destinations | `history` | Add a destination | — | `empty.restore.no_destinations` |
| Restore | No snapshots | `history` | Back up now | — | `empty.restore.no_snapshots` |
| Restore | Only mirrors | `folder-sync` | Open folder | — | `empty.restore.mirrors_only` |
| Restore browser | Empty directory | `folder-open` | — | — | `empty.snapshot.dir` |
| Settings · Vault backups | None yet | `archive` | — | — | `empty.vault_backups` |

Every empty state is a *state*, not a placeholder: it explains what the thing is
in one sentence before offering the action.

### 16.2 Error surfaces by `ErrorCode`

Each `ErrorCode` has exactly one presentation, used everywhere it occurs.

| `ErrorCode` | Surface | Presentation |
|---|---|---|
| `Config` | Banner on the affected screen | Message + `Open config.json` |
| `Io` | Inline under the field, or a run-detail error card | OS message + the path in `mono` + `Open folder` when the parent exists |
| `Locked` | Never shown as an error | The locked banner and disabled controls handle it; a raw `Locked` error reaching the UI is a bug |
| `BadPassphrase` | Inline in the unlock/connect modal | Field error + the `hint()` after three attempts |
| `VaultCorrupt` | Full-screen blocking state | Message, `hint()`, `Open vault backups`, `Restore a backup…` |
| `VaultVersion` | Full-screen blocking state | Both versions named + `Get the newer version` link |
| `Crypto` | Modal danger | Message + `Export a diagnostic bundle` |
| `Kopia` | Run detail error card | `RunError.message`, `hint`, `Show technical details` with `detail` |
| `KopiaMissing` | Global banner + Settings badge | Message, `hint()`, `Fix in Settings` |
| `RepoNotConnected` | Destination banner | `Connect to this repository…` |
| `RepoExists` | Inline in repository creation | `Connect instead` |
| `Schedule` | Inline under the cron/interval field | The parse error verbatim |
| `JobNotFound` | Toast | `That job no longer exists.` + refresh |
| `JobRunning` | Toast | `That job is already running.` + `Show it` |
| `JobCancelled` | Not an error | Rendered as the `Cancelled` badge |
| `Ipc` / `DaemonUnreachable` | Global banner | Message, `hint()`, `Start the service`, dimmed UI (§2.6) |
| `Service` | Settings → General banner + notification | Message + `Reinstall the service` |
| `Platform` | Inline, next to whatever failed | Message + `Copy details` |
| `Remote` | Settings → Remote config panel | Message + `Retry` |
| `Validation` | Inline field error | The message verbatim — these are already written for humans |
| `Internal` | Toast + Activity error event | `Something went wrong inside superbackup.` + `Export a diagnostic bundle` |

Rules: an error is shown **once**, at the place the user can act on it. The same
failure never produces a toast *and* a banner *and* a notification. Errors
attached to a run live in Activity; errors attached to configuration live
inline; errors attached to the whole application live in the global banner.

### 16.3 Destructive-action confirmations

| Action | Modal | Confirmation strength | States that it does not… |
|---|---|---|---|
| Delete a job | Small danger | Button only | …delete any snapshot already written |
| Delete several jobs | Small danger, lists names | Button only | same |
| Remove a destination | Small danger, lists affected jobs | Button only | …touch the data at the destination |
| Remove a destination **and** its files | Small danger | **Type the destination name** | — |
| Delete a provider | Small danger | Button; **blocked** while in use | …change anything at the provider |
| Rotate provider keys | Medium, 3 steps | Per-destination verification must pass | …revoke the old key |
| Stop a running job | Small | Button (no confirm from the tray) | …keep the partial snapshot |
| Stop all running jobs | Small, lists jobs | Button | same |
| Disable all jobs | None (reversible, reflected in the tray) | — | — |
| Restore over the original location with Overwrite | Medium | **Type `overwrite`** | …create a copy of what it replaces |
| Change the master passphrase | Medium | Current passphrase required | …invalidate any repository |
| Turn on the OS keychain | Small | Master passphrase required | — |
| Export repository passphrases | Small danger | Master passphrase required | …encrypt the exported file |
| Restore a vault backup | Small danger | **Type the backup file name** | — |
| Reset the vault | Small danger, enumerates losses | **Type `superbackup`** | …delete anything at a destination |
| Reset all settings | Small | Button | …touch jobs, destinations or providers |
| Remove all configuration | Small danger, enumerates | **Type `superbackup`** | …delete backup data |
| Quit while running | Small | Button | …leave the partial snapshot |

Every confirmation modal follows the same shape: title is a question naming the
object (`Delete “Dev code”?`), body is one sentence of consequence plus a
bulleted list of specifics, and the primary button carries the verb and the
object (`Delete job`), never `OK` or `Yes`.

---

## 17. Validation rules

Validation is inline, on blur, and never blocks typing. A form with errors
disables its Save/Continue button and the button's tooltip names the count.

| Field | Rule | Message key |
|---|---|---|
| Job name | 1–64 chars, unique (case-insensitive), no leading/trailing space | `valid.job.name.*` |
| Job sources | ≥1; each must be absolute; no duplicates; no source inside another | `valid.source.*` |
| Job destinations | ≥1 enabled destination | `job.err.no_destinations` |
| Interval minutes | 1–10,080 | `valid.schedule.interval` |
| Daily/Weekly times | ≥1 time; ≤24 times; no duplicates | `valid.schedule.times` |
| Weekly weekdays | ≥1 day | `valid.schedule.weekdays` |
| Cron expression | Parses with `croner`; the parse error is shown verbatim | `valid.schedule.cron` |
| OnChange debounce | 5–3,600 seconds | `valid.schedule.debounce` |
| OnChange min interval | 1–1,440 minutes | `valid.schedule.min_interval` |
| Timeout minutes | 1–1,440 when enabled | `valid.timeout` |
| Custom exclusion patterns | Each line non-empty after trim; compiles with `globset`; no absolute Windows paths (`C:\…`) — the message explains that patterns are relative to the source root | `valid.pattern.*` |
| max_file_size_mb | 1–1,048,576 when enabled | `valid.max_file_size` |
| Retention keep_* | 0–10,000 each; **not all zero** | `retention.err.all_zero` |
| maintenance_every_n_runs | 0–1,000 (0 = never) | `valid.maintenance` |
| Destination name | 1–64, unique | `valid.dest.name.*` |
| Local path | Absolute; parent must exist or be creatable; not inside an existing destination's root; not inside any source of a job that uses it (a backup that contains its own destination) | `valid.dest.path.*` |
| S3 bucket | 3–63 chars, lowercase letters/digits/hyphens/dots, not an IP address | `valid.bucket` |
| S3 prefix | Normalised by `normalise_prefix()`; the normalised value is shown | `valid.prefix` |
| Provider name | 1–64, unique | `valid.provider.name.*` |
| Endpoint | Parses as a host or URL; scheme defaults to `https`; a warning (not an error) when TLS is off and the host is not loopback or private | `valid.endpoint.*` |
| Region | Required for `AwsS3`; optional otherwise | `valid.region` |
| Access key ID / secret | Non-empty when the credential group is in use | `valid.credentials` |
| Master passphrase | ≥12 chars; confirmation must match | `valid.master.*` |
| Repository passphrase (`UserSupplied`) | ≥12 chars; confirmation must match | `valid.repo_pass.*` |
| Bandwidth values | 1–10,000,000 kB/s when enabled | `valid.bandwidth` |
| Bandwidth window | `start_minute != end_minute`; both 0–1,439 | `valid.bw_window` |
| Remote URL | Parses as an http(s) or ssh Git URL | `valid.remote.url` |
| Trusted signer fingerprint | Hex or base64, 16–128 chars | `valid.signer` |
| auto_lock_minutes | 0–1,440 | `valid.autolock` |
| stale_after_days | 0–90 | `valid.stale` |
| dedupe_minutes | 0–1,440 | `valid.dedupe` |
| max_parallel_jobs | 1–8 | `valid.parallel` |
| log_retention_days | 1–365 | `valid.logdays` |

Cross-field warnings (non-blocking, `warning` tint):

- A job whose only destination is a `LocalMirror` — no history, no encryption.
- A job whose destinations are all on the same physical drive — one failure
  loses everything.
- A job with `OnChange` and a source of >50,000 files.
- A source path that lies inside a destination path (recursive backup).
- `auto_lock_minutes = 0` with `run_as_service` on and `use_os_keychain` off.
- A destination with `Never verified` used by an enabled job.
- Every `keep_*` value 0 while `maintenance_every_n_runs > 0`.

---

## 18. Accessibility summary

Full rules are in `DESIGN_SYSTEM.md` §9. Screen-specific requirements:

| Screen | Requirement |
|---|---|
| Global | The locked-vault banner is an `alert` and is announced when it appears. The rail is a `Role::List` of `Role::ListItem`s with the current item marked selected. |
| Dashboard | Each job card is a single focusable element announcing name, status, last run, next run and destination count (design system §9.3.3). The health ring announces `Health::title()` plus the reason line. |
| Running job | Progress announcements are throttled to one per 10 seconds per run and phrased as complete sentences. |
| Tables | `Role::Table` with row and column counts; every row announces its cells in reading order; sort state is announced on change. |
| Job editor | Tabs are a `Role::TabList`; the dirty dot is announced as `, has unsaved changes`. Save announces the error count while invalid. |
| Exclusions | Each preset row announces title, ticked state, rationale and pattern count; risky presets append `, may lose data`. |
| Encryption panel | Each radio announces its option, its `kopia_id()` and its helper text. The passphrase-source radio additionally announces the recovery consequence. |
| Write this down | The passphrase block is focusable and readable by a screen reader (it must be — a blind user has to be able to hear it); it announces once, in character-by-character mode, and only on explicit focus, never automatically. |
| Restore browser | The path breadcrumb is a `Role::List`; navigation announces the new directory and its item count. Tri-state select-all announces its state. |
| Modals | Announced by title on open; focus trapped; closing announces the result. |
| Toasts | A single live region, at most one announcement per second. |
| Tray | Icon and menu names per §14.5. |
| Colour | No information is carried by colour alone anywhere in this specification. Verified case by case: badges (icon + word), tray (shape), progress (label), health (title text), validation (icon + message), project colour (also a name in the group header). |
| Contrast | Every foreground/background pair in `DESIGN_SYSTEM.md` §2 is measured; text meets 4.5:1, control boundaries and status marks meet 3:1. |
| Zoom | egui's `pixels_per_point` is bound to `Ctrl/Cmd +` / `-` / `0` over the range 0.8–2.0. All layouts are specified in logical px and reflow per §4.4 at every zoom level. |
