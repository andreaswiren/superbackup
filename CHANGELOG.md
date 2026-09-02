# Changelog

All notable changes to superbackup are recorded here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

While the version is `0.x`, the on-disk formats may change between minor
versions. Every such change ships with a forward migration, and the vault
format carries an explicit version so an older build refuses a newer file
rather than mangling it.

## [Unreleased]

Nothing yet.

## [0.2.9] - 2026-09-03

### Fixed

- **Your own exclusions were reported as warnings.** A job that excluded
  `node_modules` said so through an amber "Warnings" badge on every single run,
  for the crime of honouring the rules you wrote - and buried the real warnings,
  unreadable files and genuine errors, in the same list. Exclusions are now a
  *note* rather than a warning: a run that excluded things and hit no problems
  is green, and the counts are shown on the run as information. Nothing is
  hidden - what the rules kept out is stated, it simply stops pretending to be
  a fault.

  This also answers the "let me suppress these warnings" ask by removing the
  warning: there is nothing left to suppress.

## [0.2.8] - 2026-09-03

### Fixed

- **No release had ever been published, and CI had never been green.** The
  Windows build succeeds and always has; the release job required *every*
  platform, so a clippy lint on macOS silently withheld a Windows artefact that
  had already compiled, tested and packaged. The release now publishes what
  built - and refuses outright if the Windows artefact is missing, because
  Windows is the priority platform and a release without it is not a release.
  Which platforms are present is stated in the job log rather than left to be
  inferred from the file list.
- **`shortcut.rs` did not compile on Linux at all.** Reading a desktop entry
  passed the whole argument vector where a path was wanted. Two constants were
  also dead on the platforms that do not use them, which fails a `-D warnings`
  clippy run. Both were introduced with the applications-menu work and were
  invisible here, because the only platform built locally is the one they
  happened to work on.

## [0.2.7] - 2026-09-02

### Fixed

- **Opening "New destination" froze the window.** The Google Drive detection
  added earlier walked `A:` to `Z:` and stat-ed every letter, and opening an
  empty floppy or optical drive blocks until the device times out. It now reads
  the mounted-letters bitmask, which answers which drives exist without
  touching any of them, and skips removable media. A test asserts detection
  finishes in well under a frame.
- **Buttons inside a table row did nothing visible.** Making rows clickable
  meant a click on a button inside one landed on the row as well, so "Verify"
  verified *and* navigated to the editor, and "Run now" on the dashboard ran
  the job *and* opened it - in both cases the navigation is what you saw, so
  the button looked broken. A control inside a row now wins.
- **A job name sat above the rest of its row.** The cell wrapped its content in
  a vertical layout, which lays out from the top and so opted out of the
  table's own centring; with no description that left a single line high.
- **`destination repository contains incompatible data` is explained.** It is
  what `sync-to` says when the destination already holds a *different*
  repository, which is the one failure that looks like a bug and is really a
  statement about what a copy is: a copy is the same repository in a second
  place, so it has to start empty. It now says so, and says what to do.

### Added

- **The encryption keys export as JSON as well as prose**, so they can be read
  back one at a time rather than retyped. Same keys, same sensitivity; the
  prose document is for a person and a safe, the JSON is for getting a machine
  back. Each entry carries the destination id, so an import can match a key to
  its destination even after a rename.

## [0.2.6] - 2026-09-02

### Fixed

- **The vault was never backed up before an ordinary change.** Rotation and
  remote-pull both took a copy first; `save` did not - and `save` is the path
  every stored secret goes through. So the directory the interface describes as
  "written before every change to the vault" stayed empty through every change
  that actually happened, and the one thing those copies exist for was missing
  exactly when it would have been needed.
- **A destination could never be turned into a copy of another.** A replica has
  no key of its own, so the configuration is invalid while it carries a
  passphrase handle - but the update path preserved that handle
  unconditionally, so the editor cleared it, the daemon put it back, and
  validation then rejected the result. Clearing is now allowed, which is the
  one change to a handle a client may legitimately make; repointing one at
  another destination's secret still is not.
- **The explanation of that error was redacted away.** `passphrase_ref` names
  *where* a secret is stored and never the secret, but "passphrase" is a
  redaction hint, so the message came out as
  `destinations[storj-s3].passphrase_ref: [redacted]` - the user was told there
  was a problem and not what it was. A `*_ref` key is a handle and is no longer
  masked; every key naming an actual credential still is, with a test pinning
  both directions.

### Added

- **Double-click a file in the restore browser** to restore a copy into a
  private cache and open it with whatever the system associates with it, so
  "is this the version I want?" can be answered without restoring over
  anything. Programs and scripts are shown in their folder rather than
  launched: restoring something from a backup is not consent to run it.
- **A restore queue below the browser**, listing everything marked wherever it
  was marked, with per-item removal and a clear-all. The selection accumulates
  across directories and used to be visible in full only in the confirmation
  dialog, after the choosing was over.

### Changed

- **Every destination kind is named for what it is, not where it sits.** Three
  read `... repository` and the fourth read `S3 bucket`, so an S3 destination
  looked like a container while the others looked like contents. A bucket is
  somewhere a repository is put, and holds one per key prefix - which is how
  several machines share one. `Folder mirror (no repository)` stays a mirror on
  purpose: it is the one destination that deliberately is not a repository, and
  that is why "destination" remains the umbrella word.

## [0.2.5] - 2026-09-02

### Fixed

- **No table row in the application was clickable.** egui tables sense hover
  unless told otherwise, and not one of them said otherwise - so
  `row.response().clicked()` was false on every row of every table, and every
  list that opens something by being clicked did nothing at all. Activity runs
  and events, Destinations, Jobs, Storage providers, the snapshot list and the
  restore file browser were all inert. Six screens, one line each.
- **Restore said "Loading..." for ever.** A destination with no *known*
  snapshots showed the loading label whether or not anything had ever been
  asked - so every unselected destination sat there loading permanently, and a
  destination that genuinely holds nothing looked identical to one still
  working. The three states are now told apart.

### Added

- **A folder button on each snapshot row** that opens the browser, and a
  **Repository column** showing where the snapshot physically lives. The row
  stays clickable, but a row that opens something with no affordance on it is
  a row nobody clicks.
- The **bandwidth slider now appears everywhere a limit is set** - the job
  editor and the destination editor had their own plain number boxes, and only
  Settings got the slider when it was added.

### Changed

- **Chained destinations are no longer a decision on every destination.** The
  choice was a mandatory two-way radio on a screen where almost nobody needs
  it. It is now a single opt-in, and appears only when there is something to
  copy from or the destination is already a copy. The relationship itself
  still lives on the destination rather than the job, because a replica *is*
  the same repository as its source permanently - `sync-to` copies the format
  blob - and two jobs cannot hold different opinions about that without one of
  them creating a separately-keyed repository where the other expects a copy.

## [0.2.4] - 2026-09-02

### Fixed

- **Clicking the tray icon did nothing.** The click was received and the
  interface was launched - and the launch failed every time, because the tray
  process detaches its console at startup, which leaves its own standard
  handles dangling, and `Command` hands those to `CreateProcess` by default.
  The log said "could not open the interface (os error 50)"; the user saw
  nothing at all. The child now inherits no handles, since a window has no use
  for a console. Reproduced in isolation - a process that allocates a console,
  frees it, then spawns with inherited stdio fails, and with null stdio
  succeeds.
- **The restore browser could never list anything.** `snapshot.list` reports a
  snapshot's *manifest* id, and that is what every client held and passed back
  - but `kopia show`, which is what browsing runs, addresses the *object* id of
  the snapshot's root directory and rejects a manifest id outright: `invalid
  content ID: "3e0f..." (17 vs 33)`. The daemon now resolves one to the other,
  so callers keep passing the id they know, and restoring a path inside a
  snapshot addresses the same object browsing does rather than the two
  disagreeing. The path is still validated before anything is opened, so a
  `..` is refused on its own terms.

## [0.2.3] - 2026-09-02

### Added

- **superbackup can put itself in the applications menu.** The Start menu on
  Windows (a real `.lnk`, written through the shell rather than hand-rolled),
  an XDG desktop entry on Linux - one file, read by GNOME, KDE, XFCE and LXQt
  alike - and a `~/Applications` link on macOS. Always under the user's own
  profile: the all-users location needs administrator rights, and asking for
  elevation to add a shortcut teaches people that elevation prompts are
  routine. New command `app.set_shortcut`, and the entry's state and path are
  reported alongside the service.
- **First run asks.** The setup flow now offers the menu entry, starting at
  login, and installing the background service - with a tooltip on the last
  one explaining what it buys: without a service superbackup runs only while
  you are signed in, so a machine at the login screen backs up nothing, while
  a service runs from boot whether or not anyone has logged in. The tray icon
  still appears and still manages everything either way. Being findable is
  defaulted on; running at login and installing a service are not, because
  those are impositions and have to be asked for.

### Fixed

- **The setup switches did nothing.** "Start superbackup when I sign in" and
  "Install the background service" were rendered, stored, and then dropped:
  nothing read either field, so a user who asked for both got neither - and no
  error either, because nothing had been attempted. They are now applied, each
  as its own request, so a refused elevation prompt for the service does not
  silently cost the menu entry as well.

## [0.2.2] - 2026-09-02

### Changed

- **A locked vault now locks the window.** It used to be announced in four
  places at once - a banner on the dashboard, a pill in the status strip,
  per-screen empty states, and a modal raised by the next action that needed a
  key - while the user could still walk through Jobs, Destinations and Storage
  providers. That is both nagging and backwards: repeating a message four times
  teaches people to dismiss it, and showing the configuration while the keys are
  locked protects the keys and publishes the map to them. There is now one lock
  screen, with one unlock control, and the only thing readable without a
  passphrase is the five most recent runs and how each ended - Completed,
  Completed with warnings, Error, Missed scheduled run. That is the one question
  worth answering while locked, and answering it needs nothing secret.

### Fixed

- **Scheduled runs piled up for ever against a locked vault.** A run is
  announced as active the moment it is queued, and the skip that follows carried
  no run id - so nothing could retire it. Every scheduler tick left another
  "Queued" card behind: seven of them, started an hour apart, sitting in
  "Running now" and unable to finish because none had started. The skip now
  names the run it retires, and a missed scheduled run is recorded in history as
  one instead of being dropped.
- A sentence in the new lock screen rendered with a gap in the middle of it -
  the same broken string continuation swept earlier. A test now scans `copy.rs`
  for runs of spaces mid-sentence, so the whole class fails the build rather
  than reaching a screen. Verified by injecting one.

## [0.2.1] - 2026-09-02

### Fixed

- **Restore could not find any snapshot to restore.** kopia prefixes every tag
  key with `tag:` when it stores it, so a snapshot created with
  `--tags=superbackup-job:<id>` comes back as `{"tag:superbackup-job": …}`.
  The lookup used the bare name, so a snapshot's job was always unknown — which
  made every job-filtered query empty. `superbackup restore Development`
  answered "Development has no snapshots to restore from" about a repository
  holding 134,833 files, and `superbackup snapshots Development` said none had
  ever been taken. Verified against kopia 0.23.1 by writing a tag and reading it
  back, and end to end by restoring a file out of the real 15.2 GB snapshot and
  byte-comparing it with the original.

## [0.2.0] - 2026-09-01

The release in which the backups actually run. 0.1.0 could not create a
repository on any platform: every kopia invocation carried a malformed boolean
flag, and the one error message that would have explained it was being discarded
before it reached anyone. Both are fixed, and a backup of 134,833 files to
OneDrive and a repository on StorJ were made with this code.

### Added

- **A live throughput graph** on each running job. A single "89 MB/s" reading
  cannot tell a slow backup from a stopped one — a number that stopped updating
  looks exactly like a healthy one — so the recent rate is now drawn as a
  shape, scaled to its own peak and labelled with it. The series is kept in the
  window, bounded, and dropped when the run ends.

- **Google Drive**, as a detected folder rather than an API integration.
  Google Drive for Desktop mounts your Drive as a filesystem, as you, against
  the storage you pay for; the destination editor now finds those mounts and
  offers them. kopia's own `gdrive` backend was deliberately not used: it is
  marked `[Not maintained]` upstream, and it authenticates as a *service
  account*, whose files are owned by that account and count against a quota a
  consumer Google plan does not grant it — so backing up "to Google Drive" that
  way would not use the storage the user bought. Streaming mode is detected and
  warned about, because a repository made of placeholders is read on every
  operation and will stall. Detection was written against Google's documented
  layouts and could not be exercised against a live client; every route
  degrades to "not found", leaving the path typeable by hand.

- **Export and import the configuration as a file**, for moving a setup between
  machines without a Git remote. `superbackup remote export FILE` writes the
  same sealed document `remote push` publishes — encrypted under the master
  passphrase — and `remote import FILE` verifies it and reports what applying
  it would change, through the *same* checks a pull goes through: signature,
  decryption, validation, the rollback guard and the different-vault guard. A
  file carried on a stick is not more trustworthy than a Git remote for having
  been carried by hand. Nothing is written until `remote apply`.
- **"New storage provider…" in the destination editor now works.** It set a
  flag nothing read, so choosing it did nothing at all. It now opens the
  provider editor, says why you are there, and on save returns to the
  destination with the new provider selected. The provider is still kept
  separately, so other destinations can use it.

- **Bandwidth limits have a slider**, marked off in 10 Mbit/s notches from 0 to
  1000, with upload and download on one shared label column so their boxes,
  units and Mbit readouts line up. The number box stays authoritative: dragging
  snaps to a notch, but a typed value is left exactly as typed rather than
  rounded to the nearest one.

- **Chained destinations have an interface.** A destination can now be filled
  by copying an existing repository from another destination instead of reading
  the job's folders a second time — back up to OneDrive, then copy that
  repository to StorJ, with the folders read once. The destination editor asks
  where the data comes from, offers only the destinations that would not form a
  loop, and states the one thing that must not be misunderstood: a copy **is
  the same repository in a second place**, opened with the source's passphrase.
  It has no separate key, because `kopia repository sync-to` copies the format
  blob. So the encryption panel is removed for a copy rather than shown
  disabled — a greyed-out algorithm picker would still imply a second key
  behind it. Ticking a copy in a job adds the destination it copies from as
  well, and says so, because a copy made from a source the same run did not
  update would replicate stale data and still report success. Runs show which
  destinations were copies, from where, and why a skipped one was skipped.
- **superbackup talks to S3 directly.** A small signed client
  (`crates/core/src/s3.rs`) implements Signature Version 4 — canonical request,
  string to sign, the four-step key derivation, `x-amz-content-sha256`,
  `x-amz-date` and the `Authorization` header — against AWS's own published
  test vectors, and reads `ListBuckets` and `ListObjectsV2` with a bounded
  parser written for those two shapes rather than a general XML library. No AWS
  SDK and no second TLS stack: it reuses the `reqwest`/rustls already in the
  tree. New IPC commands: `provider.list_buckets` and `provider.list_objects`.
- **Testing a storage provider works before any destination exists.**
  `provider.test` used to borrow the first destination that used the provider
  and go through kopia, so before the first bucket existed it could only answer
  "there is nothing to test against" — exactly when someone has just pasted a
  key pair. It now signs a real `ListBuckets`, which proves the endpoint
  resolves, TLS succeeds, the clock is close enough and both halves of the key
  are right, and it returns the bucket names.
- **A bucket picker in the destination editor**, populated from the provider,
  beside a manual field that is never disabled. Offline, a locked vault, a key
  scoped to one bucket, or a provider that has not been saved yet all leave the
  list unavailable with the reason shown — and none of them can stop a
  destination being created.
- **An optional administration-panel URL on a storage provider.** Where you log
  in to manage the account and rotate its keys, prefilled for StorJ and Amazon
  S3 and clearable, reachable from the provider editor and from any destination
  that uses it. Documentation only: nothing connects to it, and it is kept out
  of the plain-text key-export document.

- **A real Kopia page in Settings.** It shows the full resolved path of the
  binary in use, its version, and which of the four resolution routes produced
  it — with every route listed, chosen or not, so "why this kopia?" has an
  answer on screen. A "Run the checks" action executes `kopia --version` and
  `repository status` against a chosen destination and shows the exact command
  line, the exit code and both output streams verbatim. The command line is
  safe to display and worth displaying: secrets reach kopia through the
  environment and never through `argv`, and the names of those variables are
  shown while their values are not. New IPC command: `kopia.probe`.
- **Job preview (dry run) in the interface.** The engine has supported
  rehearsals end to end for some time and none of it was reachable from the
  window. A Preview action now exists on the jobs list, the job editor and the
  dashboard job card, and opens a screen with one card per destination — the
  fan-out is never flattened — showing what would be copied, what is already up
  to date, and, where a figure genuinely cannot be known, saying so instead of
  printing a zero. A rehearsal is recorded with its own `Trigger::Preview`, so
  the history can never mistake it for a backup.
- **Encryption keys: validate and export.** A "Check the stored key" action on
  a repository destination opens the repository with the key and reports what
  happened — a real connect attempt, not a format check (`dest.check_key`). An
  export writes every repository encryption key, its destination, location,
  algorithms and the `kopia repository connect` command that opens it, to a
  plain-text file the user chooses, so a repository can be recovered years
  later with the kopia CLI alone (`vault.export_keys`).
- **A machine manifest next to the backups.** Every run now writes or refreshes
  `_superbackup/machines/<id>.json` and a human-readable README at each
  destination with a local path, so a drive holding several computers' backups
  can be understood during a recovery. On by default, switchable off, and
  reported honestly as unavailable for object storage. The destination editor
  lists the computers that have backed up to a destination.

### Changed

- **The tray icon is the superbackup mark again.** It was an abstract ring with
  a status pip — good at encoding five states, and it looked nothing like the
  application, so the one place the program is seen all day did not say which
  program it was. Every tray mark is now the interlock from
  `assets/icons/superbackup.svg`, in one ink, with a status badge in a well
  knocked out of its bottom-right corner: a filled disc for `idle`, that same
  circle opened into a spinning ring for `running`, a triangle for `attention`,
  two bars for `paused`, a cross for `failed`. The state is carried by the
  badge's *silhouette*, so it survives greyscale and the macOS template where
  colour is discarded entirely, and the mark is identical in all five states so
  the set reads as one application. Drawn at the 16 px floor, which retires the
  separate large/small size profiles: it is now one drawing at every size.
  Every badge ink is variant-aware and clears WCAG 1.4.11 on the taskbar it is
  drawn on — the worst is 4.95:1, where the old `attention` pip was 1.92:1 on a
  light taskbar and the old `failed` pip 2.94:1 on a dark one.

- **"Can I reach this place?" and "is there a repository here?" are separate
  answers.** `dest.test` used to build a kopia driver, which needs the
  repository encryption key — which does not exist until the repository does —
  so a destination that had been added but not yet created reported as
  *unreachable* even though it was plainly reachable. Reachability is now
  established with no key at all (a signed `ListObjectsV2` plus a bounded write
  probe for S3; the directory probe for a folder), and repository presence is
  reported separately in a new `repository_present` field by *looking for*
  kopia's `kopia.repository` blob, never by opening it. Opening it with a key
  remains `dest.check_key`. A reachable destination with no repository yet is a
  success with a note, not a failure.
- Errors from an object store are distinguished rather than collapsed: a
  wrong access key, a wrong secret key, a clock more than fifteen minutes out,
  a key that is valid but not permitted to list buckets, a bucket that does not
  exist, a wrong region, DNS, TLS and connection failures, and an endpoint that
  answers but is not S3 each get their own sentence and their own next step.

- `vault.export_keys` is the first and only IPC command that returns secret
  material. It requires an unlocked vault *and* the master passphrase
  re-presented, is rate limited, is logged, and writes no file itself. The
  "no plaintext secret over IPC" rule in `THREAT_MODEL.md` §A7 has been
  rewritten to record the exception, its bounds and its residual risk rather
  than quietly ceasing to be true.
- The vault badge in the sidebar is sized to its content. It was a fixed 32px
  with two lines of text inside it, so "Locked / Schedules are blocked" ran to
  the edge and read as clipped.

### Fixed

- **The CLI could not find a daemon it had just started.** The IPC endpoint is
  named from a hash of the configuration directory, and the hash was taken over
  the raw path bytes — so `SUPERBACKUP_HOME=C:/x` and the same directory written
  with backslashes produced two different pipes, as did a difference in case on
  a filesystem that ignores case. Separators, trailing separators and (where
  the platform is case-insensitive) case are now normalised first. Genuinely
  different homes still get their own endpoint, which is the whole reason the
  tag exists.

- **Sentences broke apart mid-line across the interface.** Multi-line string
  literals had their `\` continuations collapsed into runs of literal spaces,
  which rendered as gaps in the middle of a sentence — "every repository key.
  &nbsp;&nbsp;&nbsp;&nbsp;Anyone who has both". 54 of them, in schedule
  descriptions, exclusion explanations, platform messages and CLI output.

- **Every kopia command was malformed.** Kopia's CLI is kingpin, which declares
  booleans as `--[no-]flag` — they take no value, so `--flag=false` parses as
  `--flag` plus a stray positional `false`. superbackup rendered the `=false`
  form, with a comment asserting it was "kingpin's", and put
  `--persist-credentials=false` on *every* invocation. So every real kopia
  operation died with `expected command but got "false"`, which the classifier
  could not recognise and reported as the useless "kopia reported an error".
  Creating a repository, connecting to one, restoring — none of it could ever
  have worked. Kopia genuinely uses both spellings (`maintenance set
  --enable-full=true` is a value flag), and a test now pins the two apart.
- **kopia's actual words were thrown away.** The driver captured stderr,
  classified it, carried it to the daemon — and then the mapping to the wire
  error passed the generic headline and dropped the detail. An unrecognised
  failure now carries kopia's own text, and every kopia failure is logged in
  full. Finding the bug above took one run once this was in place.
- **A repository destination was created with a passphrase reference pointing
  at nothing.** The handle was minted on the theory that something would store
  a passphrase against it later; nothing did. Every operation that needed it
  failed with "the vault has no entry for repo-passphrase:…" — verify, restore,
  and repository creation alike. The passphrase is now generated and stored
  first, and the handle written only once it resolves.
- A local repository that had just been created still reported "no backup
  repository here yet": kopia's filesystem backend suffixes blob names, so the
  format blob is on disk as `kopia.repository.f`, and only the bare name was
  looked for.
- **This machine never learned its own name.** `Config::default` minted a
  placeholder identity — label "this-pc", hostname "unknown" — and although
  `platform::identity::detect` and `refresh` both existed and were tested,
  nothing in the running application called either. A first run now detects the
  real machine before the first save, so the destination folder is named after
  it, and every start refreshes hostname, OS build and user. The slug is never
  touched: it is the folder name under every destination root, and moving it
  would leave repositories where kopia cannot find them.
- **The machine label could not be typed into.** The field rebuilt itself from
  the daemon's snapshot every frame, so each keystroke was discarded and the
  box snapped back — and there was no command behind it to save to anyway.
  There is now `machine.rename`, which changes the label and deliberately not
  the folder name.
- Verifying a destination reported the result twice: once as the banner the
  destinations list already draws, and again as a toast on top of it, once per
  destination. The toast is now only raised where the result is not already on
  screen.
- The empty state in the job wizard and job editor sat near the bottom of its
  box. It centres itself within the height it is given, and inside a layout
  that grows to fit, that height was the space left in the parent rather than
  the box — so the centring padding inflated the box it was centring in.

- **A fresh install did nothing at all.** The daemon refuses to start without a
  vault, and a vault needs a passphrase only a person can supply — so
  double-clicking the executable on a new machine printed "run `superbackup
  init`" to a console that had already been detached, and exited. No window, no
  tray icon, no message. The setup flow existed, was designed and was
  screenshot-tested, but nothing in the shipped application ever started it:
  its only caller was the screenshot harness. A first run now opens setup,
  which writes the vault itself — as `superbackup init` does, and for the same
  reason: the process that would answer an IPC request is the process that will
  not start — and the tray starts once there is a vault. Setup refuses to write
  over a vault that already exists.

- A multi-line code block rendered every line side by side rather than one per
  line: the block's scroll area inherited a horizontal layout from its parent.
- The activity table drew its card past the window's right edge at the minimum
  window size — its trailing column was not in the width budget the fit
  calculation adds up. The providers table had the same latent gap.
- `--json` errors lost the daemon's own hint, so a locked vault printed with no
  "run `superbackup unlock`" next to it.

## [0.1.0] - 2026-08-31

**First testing release.** Security updates until at least August 2031; see
[`docs/compliance/cra/SUPPORT_POLICY.md`](docs/compliance/cra/SUPPORT_POLICY.md).

> This is a pre-1.0 build published for testing. It has been exercised against a
> scriptable fake Kopia and, on Windows, against a real Kopia and a real
> repository — but **not** against a real StorJ bucket or a OneDrive folder
> holding millions of files, which is the load case it exists for. Linux and
> macOS compile in CI and are otherwise untested. Do not make it your only copy
> of anything yet.

### Added

**Backing up**

- Jobs that fan out to many destinations at once — a fast local repository, a
  Kopia repository inside OneDrive, and an offsite S3 bucket — where one
  destination failing does not stop the others, and a partial success is never
  reported as a clean success.
- Reusable storage providers: an endpoint, region and credential pair defined
  once and shared by every bucket and job that uses it, with per-bucket
  credential overrides and key prefixes.
- Kopia repositories on a local path, a network share, a detected OneDrive
  folder, or any S3-compatible bucket, plus plain unencrypted folder mirrors
  for when a readable copy is the point.
- OneDrive discovery that reads the real account registration rather than
  guessing at `%USERPROFILE%\OneDrive`, handles several personal and business
  accounts, and refuses to put a repository where Files On-Demand would
  dehydrate it.
- Exclusion presets aimed at developer folders — `node_modules`, framework and
  bundler caches, Rust `target`, Python virtualenvs, .NET, Java, Go, IDE state
  — each carrying the reason it is safe to skip.
- Scheduling by cron, daily, weekly, interval, or debounced file change, with
  DST handled in both directions and catch-up that fires **once** after the
  machine was off rather than once per missed interval.
- Bandwidth ceilings, a lower ceiling inside a daily window, and "pause for N
  hours" from the tray or the command line.
- Dry runs that genuinely write nothing: no directory is created, no file
  copied, no snapshot taken, while still reporting the counts that would have
  been produced.

**Trusting it**

- A vault sealed with XChaCha20-Poly1305 under an Argon2id-derived key, with
  the header authenticated so KDF parameters cannot be weakened and replayed.
- Ed25519 signing for shared configuration, with the signer fingerprint bound
  to the key that actually signs.
- Master passphrase rotation that enumerates the repositories it will affect
  *before* the user commits, and is resumable rather than a cliff.
- Optional OS keychain storage as a split secret: the platform store holds a
  random wrap key and nothing else, so reading the keychain alone yields noise.
- Credential redaction over everything that leaves the process, and secrets
  passed to Kopia through the environment rather than argv.

**Living with it**

- A tray icon whose five states are distinguished by shape rather than colour,
  so they survive greyscale and the macOS template renderer.
- A graphical interface covering onboarding through restore, which never
  flattens a job's fan-out into a single number.
- A CLI where every command accepts `--json`, exit codes distinguish "your
  backup failed" from "I could not reach the daemon", and `superbackup schema`
  emits the whole command surface generated from the parser itself.
- Runs without you logged in, as a Windows service, a systemd unit or a
  launchd daemon — and says honestly which destination kinds still work in
  that configuration.
- Kopia installed automatically on first run from the upstream releases, with
  its SHA-256 verified against the published checksum before anything touches
  disk.

**Documentation**

- A threat model with eight in-scope adversaries, each with its residual risk,
  and an explicit out-of-scope list.
- An EU Cyber Resilience Act package with an honest applicability analysis, a
  CycloneDX 1.5 SBOM, and a consolidated gap list.

### Known limitations

- Projects, remote-config settings and folder size estimates are **stubbed and
  say so on screen**; the CLI likewise refuses commands it cannot honestly
  implement rather than pretending.
- Kopia publishes a signature alongside its checksums but no key this project
  can pin, so the auto-installer proves **integrity, not authenticity**.
- An S3 destination gets no machine manifest, because there is no local path to
  write one to.
- Notifications on Windows need a Start-menu shortcut carrying an
  AppUserModelID to be attributed correctly.

[Unreleased]: https://github.com/andreaswiren/superbackup/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/andreaswiren/superbackup/releases/tag/v0.1.0
