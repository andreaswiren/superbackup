<div align="center">

<img src="assets/tray/idle.svg" width="72" height="72" alt="">

# superbackup

**Back up your development folders properly — locally, to OneDrive, and offsite —
without OneDrive choking on three million cache files.**

A single Rust executable. Tray icon, real interface, real scheduler,
[Kopia](https://kopia.io) underneath.
Windows first, Linux second, macOS third.

[![CI](https://github.com/andreaswiren/superbackup/actions/workflows/ci.yml/badge.svg)](https://github.com/andreaswiren/superbackup/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

</div>

---

## Why this exists

OneDrive cannot cope with a developer's disk. Point it at a folder of Next.js
projects and it will spend the rest of its life trying to sync several million
`node_modules` and `.next/cache` files, achieving nothing except a permanently
spinning sync icon and a laptop that never sleeps.

Git is not the answer either. Plenty of what matters is not in a repository —
`.env` files, scratch branches, local databases, the half-finished thing you
have not committed and would be genuinely upset to lose.

superbackup sits between the two. It snapshots the folders you actually care
about into deduplicated, encrypted Kopia repositories: a fast local copy, a copy
inside OneDrive that OneDrive can *actually* handle (a handful of large blobs
instead of millions of tiny files), and an offsite copy in S3-compatible storage.
It knows what a build cache is and skips it. It runs on a schedule, tells you
when it fails, and stays out of the way when it does not.

## What it does

- **Multiple destinations per job.** One job writes to a fast local disk, an
  OneDrive folder, and an offsite bucket. A broken offsite link does not stop
  the local copy.
- **Reusable storage providers.** Configure "StorJ eu-1" once with its endpoint
  and keys; every bucket and every job reuses it. Buckets that need their own
  key pair can override it. Rotating a key rotates it everywhere.
- **OneDrive detection that works.** Reads the actual OneDrive account
  registration rather than guessing at `%USERPROFILE%\OneDrive`, handles
  multiple personal and business accounts, checks free space, and warns when
  Files On-Demand would dehydrate the repository out from under the backup.
- **Developer-aware exclusions.** One-click presets for `node_modules`, bundler
  and framework caches, Rust `target`, Python virtualenvs, .NET and Java build
  output, IDE metadata — each explaining what it costs you.
- **Per-PC destination layout.** Every destination gets a self-describing
  `_superbackup/` manifest so a human opening a shared drive or bucket can see
  which folder belongs to which machine, without needing this application.
- **Real encryption control.** Full access to Kopia's algorithm, hash, splitter
  and error-correction settings, with a generated 256-bit passphrase and a
  "write this down" moment that does not let you skip past it.
- **Scheduling that behaves.** Cron, daily, weekly, interval, or debounced
  on-change. Skips metered connections and battery if you ask it to. Catches up
  once after the machine was off, not once per missed interval.
- **Bandwidth limits and pause.** A global ceiling, a lower one during work
  hours, and "pause for 4 hours" in the tray for when you need your uplink.
- **Runs without you logged in.** Installs as a Windows service, a systemd
  unit, or a launchd daemon — and tells you honestly which destination kinds
  still work in that configuration.
- **Encrypted config sync.** Pull job definitions from a Git repository across
  machines. Only the sealed vault is ever pushed, and a pulled vault is
  verified and decrypted before it replaces anything local.
- **A CLI built for automation.** Every action the interface can take, with
  `--json` on everything and a self-describing command schema, so an agent can
  drive it without guessing.

## What it is not

- Not a sync tool. It backs up; it does not two-way merge.
- Not a file-sharing product. (The `_superbackup` manifest layout leaves room
  for a shared-team direction later. That is not today.)
- Not a Kopia replacement. Kopia is excellent and does the hard part. This is
  the management layer around it that a workstation deserves.

## Install

> **Status: in active development.** The foundations, security model, and
> documentation are in place; see [CHANGELOG.md](CHANGELOG.md) for exactly what
> is built. Release binaries are not published yet.

### From source

```bash
git clone https://github.com/andreaswiren/superbackup.git
cd superbackup
cargo build --release
```

The binary lands in `target/release/`. It is self-contained — no runtime, no
webview, no Node.

On Linux you will need the GUI and tray headers first:

```bash
sudo apt-get install libgtk-3-dev libxdo-dev libayatana-appindicator3-dev pkg-config
```

### Kopia

superbackup drives the `kopia` binary. Install it from
[kopia.io/docs/installation](https://kopia.io/docs/installation/), or let
superbackup fetch a pinned build:

```bash
superbackup doctor --fix
```

## Getting started

Run it once and the tray icon appears; open the window and it walks you through
creating a master passphrase, detecting OneDrive, and setting up a first job.

Or from the command line:

```bash
superbackup init
superbackup destination add --onedrive
superbackup job add --name dev --source ~/code --template developer
superbackup run dev
```

## Command line

The CLI is a thin client. It does not open repositories itself — it asks the
running instance, which is the only process that ever touches a repository.

```bash
superbackup status                      # health, running jobs, next run
superbackup status --json               # the same, machine-readable

superbackup job list
superbackup run dev-code                # by name, id, or unambiguous prefix
superbackup run dev-code --wait         # block until it finishes
superbackup stop dev-code
superbackup stop --all

superbackup pause 4h                    # or: superbackup pause --until-resumed
superbackup resume

superbackup snapshots dev-code
superbackup restore dev-code --at 2026-08-29T14:00 --to ~/restored

superbackup watch                       # stream live events as NDJSON
superbackup doctor                      # diagnose everything, exit non-zero if broken
```

### For automation and agents

Every command accepts `--json` and returns a stable envelope with a
machine-readable `error.code` rather than English prose to parse. Exit codes are
meaningful. And the command surface describes itself:

```bash
superbackup schema --json
```

That schema is generated from the same definitions the dispatcher uses, so it
cannot drift away from what the program actually accepts. An agent can discover
the full surface without reading this file.

## Security

superbackup holds every key that makes your backups recoverable. The design is
documented in full, including its limits:

- **[Threat model](docs/compliance/THREAT_MODEL.md)** — seven in-scope
  adversaries with defences and residual risk, and an explicit list of what is
  out of scope.
- **[Privacy](docs/compliance/PRIVACY.md)** — every network connection the
  binary can make, and how to verify that yourself.
- **[Security policy](SECURITY.md)** — how to report a vulnerability.

The short version: secrets live in a vault sealed with XChaCha20-Poly1305 under
an Argon2id-derived key, never reach a child process's command line, and are
scrubbed from anything that gets logged or displayed. Backup contents are
encrypted client-side by Kopia before they leave your machine.

Two things worth knowing up front:

- **Folder mirrors are plain, unencrypted copies.** That is what they are for.
  The interface says so where you choose it.
- **There is no passphrase recovery.** Lose it and the backups are gone. This
  is a property of the design, not an oversight.

## Documentation

| | |
|---|---|
| [Architecture](docs/ARCHITECTURE.md) | How it is built and why |
| [UX specification](design/UX_SPEC.md) | Every screen, state, and string |
| [Threat model](docs/compliance/THREAT_MODEL.md) | Security design and its limits |
| [Privacy](docs/compliance/PRIVACY.md) | What leaves your machine |
| [Third-party licences](docs/compliance/THIRD_PARTY.md) | Compliance record |
| [Contributing](CONTRIBUTING.md) | The rules that do not bend |
| [Changelog](CHANGELOG.md) | What changed |

## Credits

**[Kopia](https://github.com/kopia/kopia)** does the actual work —
deduplication, chunking, and encryption. Apache License 2.0, copyright Jarek
Kowalski and the Kopia contributors. superbackup invokes it as a separate
process and contains none of its code. This project is not affiliated with or
endorsed by the Kopia project; please report superbackup bugs here rather than
to them.

Interface scope owes a debt to
**[Rclone UI](https://github.com/rclone-ui/rclone-ui)**, and interface quality
to **[Arq](https://www.arqbackup.com/)** and
**[Plakar](https://github.com/PlakarKorp/plakar)**. No code or assets from any
of them are used.

## Licence

MIT — see [LICENSE](LICENSE).
