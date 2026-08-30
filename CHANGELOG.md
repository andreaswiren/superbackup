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

**Foundations**

- Cargo workspace with `superbackup-core` (engine) and `superbackup` (single
  binary: tray, GUI, CLI and daemon in one executable).
- Configuration domain model: reusable storage providers, destinations, jobs
  and projects. A provider is defined once and reused by every destination on
  it; a destination pins a bucket and key prefix and may override the
  provider's credentials per bucket. One job fans out to many destinations.
- Exclusion presets aimed squarely at developer folders — `node_modules`,
  Next.js and bundler caches, Rust `target`, Python caches and virtualenvs,
  .NET, Java/Gradle/Maven, Go, IDE metadata, OS junk, VM images, logs — each
  carrying the rationale text shown next to it in the interface.
- Zeroizing `Secret` type with no `Display`, `Serialize`, or revealing `Debug`,
  plus constant-time comparison and an offline passphrase strength estimator
  that rejects low-diversity and common passphrases.
- Credential redaction applied to everything leaving the process — logs,
  events, IPC responses, notifications — as a safety net behind the primary
  control that secrets never enter a child process's `argv`.
- Per-user and machine-wide service path layouts for Windows, Linux and macOS,
  with durable atomic writes so a crash mid-save can never truncate the vault.
- Runtime state model: per-destination progress, run history, and the five-state
  health value that drives the tray icon. A partially successful run is never
  reported as a clean success.

**Project infrastructure**

- CI across Windows, Linux and macOS: formatting, clippy, tests, an MSRV check,
  and a cross-compile check that catches `cfg`-gating mistakes in the platform
  layer before a release build does.
- `cargo-deny` policy failing the build on a known vulnerability, an
  unmaintained crate, a licence outside the MIT-compatible allowlist, or a
  dependency from anywhere other than crates.io. No exceptions are configured.
- Tray icon set: five health states sharing one shield silhouette, with accent
  colours solved for rather than chosen so each clears the WCAG 3:1 threshold
  for non-text graphics on a light *and* a dark taskbar simultaneously. Shape
  carries the state as well as colour, so the set survives greyscale.

**Documentation**

- Threat model covering seven in-scope adversaries with defences and residual
  risk, and an explicit out-of-scope list. States the awkward parts plainly:
  folder mirrors are unencrypted by design, the destination manifest directory
  is readable by design, and the optional keychain integration trades security
  for unattended operation.
- Security policy with reporting process, scope, and disclosure terms.
- Third-party licence compliance, recording that Kopia is Apache-2.0, is
  invoked as a separate process and never linked, and exactly what that
  obliges this project to do.
- Privacy document enumerating every network connection the binary can make,
  and how to verify that claim independently.

[Unreleased]: https://github.com/andreaswiren/superbackup/commits/main
