# Changelog

All notable changes to superbackup are recorded here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

While the version is `0.x`, the on-disk formats may change between minor
versions. Every such change ships with a forward migration, and the vault
format carries an explicit version so an older build refuses a newer file
rather than mangling it.

## [Unreleased]

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
