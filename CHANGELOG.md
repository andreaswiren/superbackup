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

## [0.9.0] - 2026-09-09

### Added

- **Your backups no longer wait for you to type anything.** This machine can
  open its own vault when you log in, so scheduled runs happen whether or not
  anybody has signed in and unlocked superbackup today. On by default, and
  switchable off in Settings.

  The reason this is safe enough to default to is that an unlocked vault no
  longer means what it used to. It used to assert two things at once — the
  keys are available, and a person is present — and those are different facts
  on a machine that opens its own vault at 3am. They are now separate:

  | | Without typing anything | After typing your passphrase |
  |---|---|---|
  | Backups run | yes | yes |
  | Run, stop, pause, throttle, disable a job | yes | yes |
  | Read status, history, the configuration | yes | yes |
  | Add or change a job, destination or provider | **no** | yes |
  | Restore, git actions, credentials, settings | **no** | yes |
  | Change the master passphrase | **no** | yes |

  A confirmation lasts fifteen minutes of inactivity. When it lapses, the
  backups carry on and the next change asks. What counts as a change is derived
  from the command table rather than kept as a second list, by exclusion — so a
  command added without a thought about it requires the passphrase rather than
  quietly skipping it — and the short list of exceptions is enumerated in a test
  that fails if it changes.

  `THREAT_MODEL.md` §5 is rewritten around this, including what it does *not*
  protect: somebody with your logged-in account can still read what is in a
  backup. What they cannot do is redirect one, add a destination, or read out a
  saved credential.

  The old default was not free. On the author's own machine it cost 51
  consecutive scheduled runs, skipped because the vault had auto-locked and
  nobody had noticed.

### Fixed

- **Auto-lock no longer strands an unattended machine.** With the passphrase
  saved, the inactivity timer withdraws the confirmation rather than locking
  the vault: you are asked again before the next change, and the backups behind
  it never stopped. Without it, it locks as it always did.

- **The setup wizard's "store the vault key" switch did something.** It was
  nested under "install the background service", so it was invisible to
  everyone who did not install one — which is most people, since that needs
  administrator rights — and nothing read it afterwards in any case. It is now
  its own switch, on by default, and the answer is written down.

## [0.8.0] - 2026-09-09

A first run did almost nothing it said it would, and this release is mostly
about that.

### Fixed

- **Finishing setup now sets things up.** The wizard asked four things and
  acted on one of them. The passphrase made a vault, which worked. The
  OneDrive choice and the job template were stored on a struct and read by
  nothing at all. The Start-menu entry, "start at login" and "install the
  service" were sent to the daemon over IPC — during a first run, when the
  daemon does not exist, because it will not start until a vault exists and
  the wizard is what creates one. A request nobody is listening for fails
  quietly, so a user who ticked every box got a vault and nothing else, with
  no error to say so.

  The answers are now carried out in the process that collected them, and what
  happened is reported: a destination made, a job made, and a line for anything
  that did not work. A job that will not validate no longer costs the user
  their destination as well.

- **"Unlocking…" no longer waits for ever.** Pressing Unlock disabled the
  button and put "Unlocking…" on it, and exactly one line in the application
  turned that back off again: the arm for a wrong passphrase. Everything else
  left it on, including the case that mattered — a daemon that has only just
  been started and is not listening yet, which is precisely the state of the
  launch straight after a first run. The only way out was to kill the window.

  An unlock attempt now ends in one place, however it ends, and says why. An
  answer that never arrives at all ends it too.

- **The OneDrive folder is created, and it is the right one.** Backups go to
  `OneDrive/Superbackup/<machine>`: grouped, so opening OneDrive shows one
  folder rather than repository blobs loose among your documents, and per
  machine, because two PCs writing into one kopia repository directory is not
  a shared backup, it is a corrupted one. The folder is made during setup
  rather than left to the first backup at 2am.

- **Kopia is fetched during setup instead of after it.** The setup step that
  reports whether kopia is present asked the daemon, which does not exist
  during a first run — so it said "Kopia was not found" on every machine,
  including the ones that had it, and offered a button that opened a download
  page in a browser. It now looks for itself, and downloads and installs the
  newest supported release when there is none, with progress on the step.

### Added

- **Choose which OneDrive.** A machine signed in to more than one now gets
  asked which should hold the backup, with the free space and the path beside
  each. A personal and a work OneDrive are different places, with different
  quotas and different people able to read them, and picking one silently was
  the wrong answer.

- **The service asks for administrator rights rather than explaining them.**
  Almost nobody sets up a backup tool as an administrator, so "install the
  background service" was a tick box that produced a paragraph. It now raises
  the operating system's own elevation prompt. Superbackup never handles the
  credentials, and declining leaves a working installation minus the service.

### Note on releases

The `v0.7.0` tag was created one commit before the workflow fixes it was
supposed to carry. GitHub Actions takes the workflow from the tagged ref, so
the release run used the previous, broken version of it. This tag is cut at
the fixed workflow.

## [0.7.0] - 2026-09-08

### Added

- **Warn before a disk fills.** Superbackup watches the volumes its
  destinations are written to, and its own folder, every half hour. Two
  thresholds per level, because neither works alone: five per cent of a 4 TB
  drive is 200 GB and not worth a warning, while ten gigabytes free on a
  128 GB laptop genuinely is. Both adjustable, either set to zero to switch
  that rule off. The level is remembered per volume, so the log gets one line
  when a disk starts running out and one when it recovers rather than
  forty-eight a day. Buckets are not checked: they have no free space to read,
  and inventing a figure is worse than silence.

- **A job's page lists its own snapshots**, searchable by date, time or id,
  with a way into the restore browser for each. Restore starts from a
  destination and asks which of its snapshots belong to the job you had in
  mind; this is the other direction, which is the one you have in mind while
  looking at a job.

- **"Pull the ones behind"** on the Git page fast-forwards every repository
  whose remote has moved, one at a time. Only those: `git pull` in an
  up-to-date repository is a network round-trip to do nothing. Repositories
  with local changes, a diverged branch or no upstream are left alone, because
  choosing between a merge and a rebase is your decision and not a refresh
  button's.

- **A globe button on each repository row**, with the address as its tooltip.

- **The GitHub CLI installs from the dialog that wants it**, using winget,
  Homebrew or pacman. Not superbackup's own downloader, which exists for
  kopia: kopia is a private dependency in our own folder, while `gh` is your
  tool — it goes on your PATH, holds your credentials and needs updating, and
  a second copy invisible to `gh --version` in your terminal would be worse
  than none. Where the install would need a password it refuses and shows you
  the command, because a backup tool that asks for a root password is one to
  stop trusting.

- **Destinations can be marked as reachable from your other machines**, and
  those are offered as one-click choices for the key-sharing folder.

- **Release builds now cover ARM**: Linux `aarch64` and Windows `aarch64`
  alongside the existing x86-64 Windows and Linux and both macOS
  architectures. Built on native ARM runners rather than cross-compiled — the
  binary links GTK, libxdo and appindicator for the tray, and building where
  it runs is both shorter and the only version that has actually been run.

### Fixed

- **The test suite has never passed on Linux or macOS**, so the release
  workflow has never produced an artefact for either. Windows was the only
  platform anybody watched go green, and every release run has been red since
  the first one.

  Two causes. Fixtures in the key-export tests wrote `D:ackupsrchive`
  literals, which are *relative* paths on Linux and macOS, so configuration
  validation refused them — a helper that returns an absolute path for the
  running platform was already there and simply not used. And every test that
  binds an IPC endpoint failed on macOS: a Unix socket path lives in
  `sun_path`, which holds 104 bytes there, and a name built from a pid, a test
  tag and a full UUID under `/var/folders/xx/<28 characters>/T/` went past it.
  `bind` says nothing about length when it refuses. The names are now short,
  and still inside a private directory, because the directory is what
  `Server::bind` restricts to 0700 to keep other local users out — short *and*
  private, not short instead of private.

- **A locked vault produced a notification and a history row every hour.** A
  machine that is switched on but idle locks itself after the auto-lock timer,
  and every scheduled run from then until somebody types the passphrase is
  skipped — so one evening produced eight identical "Missed scheduled run"
  entries that crowded the real backups out of the list. It is now recorded
  once per locked stretch. `doctor` also warns when auto-lock is on and the key
  is not cached, because each setting is reasonable alone and the combination
  silently stops scheduled backups.

- **A skipped run said "0 of 0 succeeded"**, which is true, tells the reader
  nothing, and reads like a run that could not reach anything. It now says why
  it did not run.

- **Ten controls were drawn, clickable, and wired to nothing.** A discarded
  `Response` renders identically to a working button, so no rendering test
  would notice and `#[must_use]` cannot help — `let _ =` is exactly the syntax
  that suppresses it. Three were worse: the click was read into an empty body
  under a comment claiming something else handled it, which reads far more
  convincingly. Among them were both buttons on the first-run screen shown
  when kopia is missing, the dashboard's "View error" on a failed destination,
  and the only button on a job editor whose job had been deleted. There is now
  a test that fails on either shape.

- **"Run now" on a dashboard card opened the job instead of running it.** egui
  breaks a hit-test tie by taking the last widget registered, and the card's
  interaction covered the whole rect *after* its buttons were drawn — so it sat
  on top of them and the button never saw the press.

- **The Git page could not scroll.** It was the only screen without a scroll
  area, so with nineteen repositories the "Not in git" section sat below the
  window and could not be reached without maximising. That list is now the
  same data grid as the repositories above it rather than cards in a frame.

- **The Storage providers name column sat above its row**, and the previous
  fix was the cause: wrapping the name in a centred layout of its own replaced
  the table cell's own centred layout with one anchored to the top.

- **The recent-runs header did not line up with its columns.**
  `allocate_ui_with_layout` does not keep the size it is given, so every column
  collapsed to its own text.

- **The Credentials page ran off the right-hand edge.** A checkbox wraps its
  helper text to the width it is given, and in a plain row the first one is
  given everything — so its hint filled the card and "Share with my other
  machines" began past the edge.

- **Timestamps are ISO ordered and always carry their year.** `12 Mar` against
  a snapshot from 2024 read as this year's. Where a timestamp stands alone it
  now names its zone.

- Browsing a snapshot from a job's page reads it, rather than showing "Reading
  directory…" for ever while reading nothing.

- The About page's logo and version are left-aligned with the cards below them.


## [0.6.0] - 2026-09-07

### Superbackup backs itself up

- **Two new job types**, for the two things a file backup cannot protect.

  - **Superbackup's vault.** It holds the password of every repository you
    have. Your file backups survive without it and nothing can read them,
    and it could not be protected by an ordinary job because an ordinary job
    writes into a repository whose password is in the vault. The job carries
    the sealed file as it sits, plus a plain-text `RESTORE.txt` saying where
    it goes and how to put it back.

  - **Your SSH keys.** The keys ticked on the Credentials page are sealed
    into one file under your master passphrase *before* they leave the
    process, so what lands in a bucket or in OneDrive is useless without that
    passphrase. There is no setting that turns that off. Until now the tick
    recorded a choice that no job acted on, and the help text said so.

  Both are made from **Jobs -> New job**, build their payload into a folder
  that deletes itself when the run ends, and fail loudly rather than writing
  an empty snapshot when the vault is locked or nothing is ticked.

- **`superbackup vault restore --from <folder>`**, the other half of the vault
  backup. It refuses while superbackup is running — a running instance would
  write its own vault back over the restored one, which looks like it worked —
  checks the backed-up file actually opens with the passphrase before replacing
  anything, and keeps a dated copy of what was there. `superbackup vault
  backups` lists those copies.

- **`superbackup cred`**: `list`, `role`, `seal`, `unseal`. `cred unseal` is
  the command written into every key backup's `RESTORE.txt`.

### Waking the machine

- **Wake this computer when a backup is due** (Settings -> Scheduling, off by
  default). A desktop asleep at 02:00 never ran the 02:00 backup; the catch-up
  then ran it at 08:40 while you were working.

  It is two things, and doing only the first gives a machine that wakes, sits
  at a black screen for two minutes and sleeps again mid-copy: a wake alarm a
  minute before the run, and a request that holds the machine awake until the
  run finishes. Afterwards the request is released and the machine sleeps on
  its own timer — superbackup does not suspend it, because it cannot tell an
  idle machine from one you have just sat down at.

  Windows wakes from sleep, not from hibernation or a machine that is off, and
  the power plan must allow wake timers; the setting says so. Linux uses the
  kernel RTC alarm, which also covers hibernation but needs the system service.
  macOS reports that `pmset` is the way and does not pretend otherwise.

### Fixed

- **Five tray menu entries did nothing at all.** Activity, Settings, a job's
  own activity, "fix kopia" and Unlock all launch the window with `--screen`,
  and `gui` took no arguments — so the command line was rejected and the child
  exited before drawing anything. Nothing caught it because a process that
  exits 2 looks exactly like one that was never asked for much. The window now
  takes `--screen` and `--job`, and every command line the tray builds is
  asserted to parse.

- **The git repository dialog was unusable.** Its Branches, Working trees and
  Documents tabs did nothing, and so did its Close button. Two separate
  defects: the segmented control updated the selection but never reported the
  change, and the dialog only wrote the new tab back when the change was
  reported; and the Close button's click was discarded with `let _ =`. Four
  more buttons in other dialogs had the same defect and are fixed too.

  **Seven controls in total were drawn, clickable, and wired to nothing.** A
  discarded `Response` renders identically to a working button — it highlights
  on hover and does nothing — so no rendering test would ever have noticed, and
  `#[must_use]` cannot help because `let _ =` is exactly the syntax that
  suppresses it. There is now a test that scans the interface's source and
  fails on a discarded click.

- **A key opened automatically could not be closed again.** Loading a key into
  the agent was reversible only by restarting the agent, which on Windows means
  restarting a service. There is now a "Stop opening it" button, which also
  deletes the copy the Windows service agent keeps in the registry so the key
  is not reloaded at the next boot.

- **A key with its own passphrase can now be opened from the window**, by
  opening a terminal that runs `ssh-add` for it. The passphrase goes from your
  keyboard to `ssh-add` and reaches neither a command line nor superbackup,
  which is what makes this the safe way round. The terminal is opened by the
  window rather than the daemon: a service in session 0 would put it on a
  desktop nobody is looking at.

- **The dashboard's "View error" link did nothing** on a destination that had
  failed — the one moment somebody wants to know what went wrong. It now opens
  that run.

- **The first-run screen offered two buttons that did nothing** on a machine
  without kopia — "Download Kopia" and "Choose a file…" — at the one moment
  the user has no other way forward. Same defect as the dialog buttons above:
  drawn, clicked, discarded. Choosing a file now sets and saves the path so
  the probe above it turns green; Download opens Kopia's own releases page.

- **The Name column on Storage providers sat above the rest of its row.** The
  previous fix was the cause: wrapping the name in a layout of its own replaced
  the table cell's own centred layout with one anchored to the cell's top edge.
  Every other column lines up by doing nothing, and now so does this one.

### Added

- **Destinations can be marked as reachable from your other machines.** A
  label, not a mechanism — but nothing about a path says whether a second
  machine can open it, and it decides whether the shared key bundle lands
  somewhere useful. Marked destinations carry a badge in the list and are
  offered as one-click choices for the key-sharing folder.


## [0.5.0] - 2026-09-07

- **Make a new SSH key pair**, from the Credentials page. Ed25519 by default,
  RSA 4096 for a host too old to take one, with the public half kept on screen
  afterwards because pasting it into a forge is the very next thing anybody
  does.

  The key has **no passphrase**, and the dialog says so before you press the
  button. `ssh-keygen` accepts one only in an argument or from a terminal, and
  an argument is readable by every process on the machine — the same leak the
  kopia driver argv rule exists to prevent, and it would be strange to hold
  repository passphrases to that standard and not SSH keys. A key with no
  passphrase is also exactly what opening it at boot unattended requires. To
  add one, `ssh-keygen -p -f <key>` in your own terminal, once.

- **Open keys without being asked.** The Credentials page shows which agent is
  running, which of your keys it is holding — matched by fingerprint — and
  whether that survives a restart, with a button to load the ones that are not.

  On Windows this deliberately asks the `ssh-agent` **service** rather than
  whichever `ssh-add` is first on PATH. Git for Windows ships a second one
  talking to a different agent, both are on PATH, Git's is usually first, and
  they give opposite answers about the same key on the same machine. Only the
  service keeps keys across reboots, which is what "open it at every boot"
  means here.

  A key with its own passphrase is not loaded from the interface: superbackup
  will not put a passphrase on a command line, so the button says so and gives
  you the one command to run instead.

- **A Credentials page.** The SSH keys this machine signs in with, found in
  `~/.ssh` and reported from their *public* half, their name and their
  permissions — algorithm, comment, and the `SHA256:` fingerprint a forge
  shows in its settings, so a key here can be matched against a key on GitHub
  by eye. A private key file is never read, except a 128-byte header check
  that tells an encrypted key from an unencrypted one.

  The GitHub CLI appears as its own kind of credential when it is signed in:
  superbackup borrows it rather than holding a token, so there is nothing
  here to leak and you revoke it where you granted it.

- **Sharing keys between your machines, sealed.** Keys marked for sharing are
  written to a folder as one bundle, **encrypted under your master
  passphrase**, so what lands in OneDrive or a bucket is useless to whoever
  finds it. Another machine unseals it into its own `~/.ssh` with owner-only
  permissions. There is no plaintext option, because one would exist to be
  chosen by whoever is least able to judge the consequence.

  Both directions ask for the master passphrase again: an unlocked window is
  not consent to gather every private key on a machine into one portable
  file. A bundle from a newer build is refused whole rather than half-read,
  and a file name inside one that tries to escape the key folder stops the
  whole unsealing before anything is written.

- **Folders that are not in git.** The scan now reports the direct children
  of your sources that hold no repository — the most exposed thing on a
  developer disk is a project nobody ever ran `git init` in, and it was
  invisible to a list of repositories. Ones holding a manifest, a source
  folder or a README are marked as looking like real work and sorted first.

- **Start tracking, from that list.** `git init` on the chosen branch, a
  first commit including everything `.gitignore` does not exclude, and
  optionally a repository created on GitHub through the CLI. Private unless
  you turn that off, and **nothing is pushed**: creating an empty repository
  is reversible in one click and pushing a tree that turned out to hold a
  `.env` is not.


## [0.4.0] - 2026-09-07

### Added

- **The Git section is a repository manager.** Clicking a repository opens
  everything about it: every branch with its tracking state (and a plain
  "never pushed" where there is no upstream), every linked working tree,
  every remote with a link to its page, and how each remote authenticates.
  Branches are where unpushed work actually hides — one repository on the
  machine this was written on has two `aw/*` branches that were never pushed,
  which no list of "the current branch is fine" could ever have shown.

  Also `superbackup git show <PATH>`, which prints the same thing.

- **Mark a repository as External.** For a clone you read and will never
  commit to. It stays backed up; it stops being counted among the folders
  holding work you could lose, so "you have not pushed your changes" is no
  longer said about a checkout you have no changes in. Stored in
  superbackup's configuration, never written into the repository — writing
  into a checkout you asked us to leave alone is the one thing the setting
  says not to do. `superbackup git external <PATH>`, `--off` to undo.

- **Which credential each remote uses.** The *mechanism*, never the secret:
  `SSH · id_ed25519`, or `HTTPS · manager` naming the credential helper git
  will ask. A key's path is not secret — it is in your `~/.ssh/config` and in
  every `ssh -v` line — and it is exactly the thing that is hard to find out
  when a push starts failing. The key's contents, a token's value and a
  stored password are never read, displayed, or asked for. Where ssh's own
  config decides, it says so rather than guessing.

- **Read a repository's own documents in place.** README, CHANGELOG, LICENSE
  and CONTRIBUTING, when the root has them, shown rendered rather than raw —
  deciding whether a repository is yours or somebody else's usually starts
  with reading one. Never editable, never navigable, and no images or HTML:
  this displays a file out of a repository that may not be yours, and the
  safe thing for such a file to be able to do is nothing except be read.

- **Live progress on a job's status page.** The same per-destination bars,
  counts and throughput graph the dashboard shows, rather than a second
  lesser rendering of the same run — a job page that went quiet the moment
  the job started would be the one page you would want open at that moment.

### Fixed

- **Verify reported nothing when it worked.** The destinations list renders
  a probe's result in place for a failure and for "reachable but no
  repository yet", and returned nothing at all for plain success — while the
  toast that would have said so is suppressed on that very page on the
  grounds that the list shows it. Pressing Verify on a destination that was
  completely fine therefore blinked and reported nothing, which reads as a
  button that does not work.

- **"Last verified: never", forever.** `Destination::last_verified_at` was
  cleared on a key rotation and set by nothing at all, so every destination
  claimed it had never been checked however many times it had been —
  including ones a backup had just written to. A successful reach-and-write
  now records it, as the provider check already did.

- **A long label pushed the whole page off the left edge.** `kv` draws its
  label right-aligned in a fixed 160px column, so a destination named
  `onedrive-superbackup-awpc34` grew *leftwards* out of its box, out of its
  card, and under the navigation rail — taking the rest of the page with it.
  The label is elided to its column now, with the full text on hover.

- **The repository details panel opened where you could not see it.**
  Nineteen repositories is a table taller than the window, so clicking a row
  near the top appeared to do nothing: the answer was rendered thirteen
  hundred pixels below. The panel is scrolled into view when it opens.

- **Restores never finished, on screen.** The dialog said "Estimating…" and
  span for ever over a restore the daemon had already completed and logged.
  The daemon publishes progress and completion on the same stream a backup
  uses, keyed by the run id the request returns — and nothing was listening.
  The dialog now recognises its own run, shows real progress, and closes
  when it is done, saying where the files went.

- **Double-clicking a file to preview it always failed.** `kopia restore`
  writes a single file *at* the target path it is given, and it was given the
  cache directory — so it tried to replace the directory with the file and
  was refused: "cannot replace …\cache\preview with tempfile … Access is
  denied".

- **Restores were not in the activity log**, and are now — when they start,
  not only when they finish, so an attempt that hangs or is killed still
  leaves a trace. They are attributed to the job that writes the destination
  as well, because "did anyone ever restore from this?" is a question asked
  about a job.

- **The four job templates were four different heights.** Only one has a
  "Recommended for developers" line, and the card height was a floor rather
  than a height, so that one grew and the others sat at the minimum. Every
  card reserves the eyebrow's row now, so the titles line up too.

- **Cron schedules read as sentences.** `Cron: 0 8-17 * * 1-5` is exact and
  tells nobody when the job runs; it now reads "Hourly, 08:00 to 17:00,
  Monday to Friday", with the expression itself on hover. An expression whose
  shape cannot be paraphrased honestly is still shown as written — a wrong
  sentence about when a backup runs would be worse than no sentence.

- **The throughput graph was drawn with `convex_polygon`**, and a throughput
  curve is not convex — egui triangulates on the assumption that it is, which
  is where the crossing slabs and stray wedges came from. It is a triangle
  strip now, correct for any shape, with a gradient that fades downwards, a
  curve smoothed through the readings rather than straight between them, two
  faint gridlines to read the height against, and a dot on the latest value
  so the eye has somewhere to land. The smoothing passes through every
  measurement — these *are* the readings — and its overshoot is clamped, so a
  spike cannot draw a rate below zero or a line outside its own box.

- **The repository details are a dialog.** A panel under a table taller than
  the window is a panel nobody sees; scrolling to it helped and still meant
  losing your place in the list.

- **Clicking a job on the Jobs page** opens its status page, as it already
  did from the dashboard.

- **A real Markdown viewer.** Headings, bold, italic, inline code, fenced
  code with horizontal scroll (a wrapped command line is one you cannot
  copy), nested and numbered lists, block quotes, rules, pipe tables, and
  links that show their destination and open only when clicked.

  Written rather than pulled in, because what it displays is a file out of a
  repository that may not be yours: no HTML, no images, nothing fetched, and
  only `http`, `https` and `mailto` are treated as links at all — a
  `javascript:` URL in somebody else's README is not a link, it is an
  attempt. Eight tests, including that one.

- **Markdown from inside a backup.** Double-clicking a text file in the
  restore browser shows it in that viewer instead of handing it to the
  operating system, which used to open an editor over a copy in a cache
  directory — surprising, and easy to mistake for the live file.

- **The repository dialog is organised.** Overview, Branches, Working trees
  and Documents, each with a count. Branches are a table with tracking state
  and a banner naming how many have never been pushed; working trees say
  which is the main one, which are detached, and which git considers stale.

- **Usage on the storage providers page**: how much each account holds,
  when it was last written to and how much that wrote. A figure that has not
  been measured yet reads as "not measured" rather than as zero, and a
  partial total is marked with a `+`.

### Fixed

- **The three dashboard tiles were three different sizes.** Each was given
  the same box and then drew a card that sized itself to its own content, so
  none of them filled the width it was given and each was as tall as its own
  text needed.

- **The recent runs list was cards inside cards.** It is a table now, with
  the numbers right-aligned to each other, because runs are compared down a
  column — did it get slower, is it uploading less, when did the failures
  start. Failed destinations are named rather than counted.

- **The storage providers name column** sat above the row it belonged to.






## [0.3.0] - 2026-09-06

### Added

- **Git inventory.** A new Git section answers the question this whole program
  exists for: *which of the folders you back up hold work that exists nowhere
  but this disk?* It finds every git repository under your jobs' source
  folders and sorts them by how much of their work is only here - uncommitted
  changes first, then commits that were never pushed, then repositories with no
  remote at all. A repository that is committed and pushed is already safe on a
  forge; those sort last and are green.

  Buttons for the obvious next step, on the rows that need them: **Pull**,
  **Commit** and **Push**. Pull is always `--ff-only` - a branch that has
  diverged is left alone, because choosing between a merge and a rebase is your
  decision and not a backup tool's. Push asks first, being the only one of the
  three that sends code off the machine.

  Also available from the command line: `superbackup git list`, with
  `--at-risk` to show only what is unprotected, and `git pull`, `git commit`,
  `git push`.

- **Ask the remote where it actually is.** `ahead 0, behind 0` in `.git` means
  "in sync as of your last fetch", and a repository last fetched in March will
  cheerfully report itself up to date all summer. Turning on "Ask each remote
  where it is" queries each remote directly with `ls-remote`, which is a
  read-only call: no fetch, no ref update, nothing written into a repository
  you did not ask us to touch. Off by default, since it is the only part that
  uses the network.

- **Folders git refuses to read are now fixable.** Anything created from an
  administrator shell on Windows belongs to `Administrators`, and git will not
  open it - three of nineteen repositories on the machine this was written on.
  Those used to be dead rows reading "Unreadable"; they now say why, and offer
  a **Trust** button that does what git's own error message instructs. The
  dialog says plainly that this turns off a security check for that one folder,
  and why the check exists.

- **GitHub, GitLab, Gitea, Forgejo, Azure DevOps and Bitbucket** are
  recognised from a remote's URL, in every form git accepts - `https://`,
  `ssh://`, and the `git@host:owner/repo` form that is not a URL at all. A
  self-hosted host is reported as self-hosted rather than guessed at, because a
  GitLab and a Gitea at `git.company.com` are indistinguishable from here.

### Changed

- **Bandwidth limits are in Mbit/s everywhere, and the number boxes are number
  boxes.** Connections are sold in megabits and every speed test reports
  megabits, so "half of my 100/100 line" should be `50` - the kB/s the limit is
  stored in is now never shown. The value box was an egui `DragValue`, which
  silently changed its own value when the pointer was dragged across it;
  nothing on screen said so, and the number moved when you meant to select the
  text. It is now a plain field you type into, with the slider beside it for
  dragging.

- **The About page shows the superbackup logo** rather than the tray's idle
  status dot, which at 64px was a plain blue circle.

### Fixed

- **Clearing a destination could have deleted your own folders.** The new
  `dest.clear_repository` matched kopia's blob names by their first letter, so
  a `src`, `notes`, `music`, `photos` or `projects` folder sitting beside a
  repository matched too - and the clear deletes directories recursively. It
  now refuses to run at all unless the folder actually holds a
  `kopia.repository`, deletes only what matches kopia's real on-disk layout,
  and reports by name everything it chose to leave behind, including the
  `_superbackup` folder that carries your machine identities.

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
