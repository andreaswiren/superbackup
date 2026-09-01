# Changelog

All notable changes to superbackup are recorded here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

While the version is `0.x`, the on-disk formats may change between minor
versions. Every such change ships with a forward migration, and the vault
format carries an explicit version so an older build refuses a newer file
rather than mangling it.

## [Unreleased]

### Added

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
