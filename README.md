<div align="center">

<img src="assets/icons/png/superbackup-256.png" width="96" height="96" alt="">

# superbackup

**Back up your development folders — locally, to OneDrive, and offsite —
without OneDrive choking on three million cache files.**

One Rust executable: tray, interface, scheduler, CLI.
[Kopia](https://kopia.io) underneath. Windows first, Linux second, macOS third.

[![CI](https://github.com/andreaswiren/superbackup/actions/workflows/ci.yml/badge.svg)](https://github.com/andreaswiren/superbackup/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

</div>

---

## Why

OneDrive cannot sync a developer's disk. Point it at a folder of Next.js
projects and it spends forever on `node_modules` and `.next/cache`, achieving
nothing but a spinning sync icon.

Git isn't the answer either — `.env` files, local databases, and the
half-finished thing you haven't committed all live outside it.

superbackup snapshots the folders you care about into deduplicated, encrypted
Kopia repositories: a fast local copy, one inside OneDrive that OneDrive can
actually handle (a few large blobs, not millions of files), and an offsite copy
in S3. It knows what a build cache is and skips it.

## What it does

- **Many destinations per job** — local disk, OneDrive, offsite bucket. One
  failing doesn't stop the others, and a partial success is never reported as
  success.
- **Reusable storage providers** — define "StorJ eu-1" once; every bucket and
  job shares it. Per-bucket key overrides supported.
- **OneDrive detection that works** — reads the real account registration, not
  a guess at `%USERPROFILE%\OneDrive`. Warns when Files On-Demand would
  dehydrate the repository.
- **Developer-aware exclusions** — presets for `node_modules`, bundler caches,
  Rust `target`, virtualenvs, build output.
- **Per-PC layout** — a self-describing `_superbackup/` manifest so a human can
  tell whose backup is whose without this app.
- **Full encryption control** — Kopia's algorithm, hash, splitter and ECC
  settings, a generated 256-bit key, and a write-it-down step you can't skip.
- **Scheduling that behaves** — cron, daily, weekly, interval, or on-change.
  DST-correct. Catches up *once* after downtime, not once per missed interval.
- **Wakes the machine for a backup**, optionally — a wake alarm a minute
  before the run and a hold that keeps it up until the run finishes, then
  releases so it sleeps on its own. Covers sleep, not hibernation, and says so.
- **Bandwidth limits and pause** — a ceiling, a lower one during work hours,
  and "pause for 4 hours" in the tray.
- **Runs without you logged in** — Windows service, systemd unit, or launchd
  daemon, and it says which destinations still work in that mode.
- **It backs itself up** — two job types for the things a file backup cannot
  protect: the **vault** that opens every other backup you have, and your
  **SSH keys**. Both leave the machine encrypted, and both carry a plain-text
  `RESTORE.txt` saying how to put them back. See
  [Restoring the vault](#restoring-the-vault).
- **Encrypted config sync** — share job definitions across machines via Git.
  Only the sealed vault is pushed.
- **A CLI built for automation** — `--json` everywhere, meaningful exit codes,
  and a self-describing command schema.

## What it isn't

Not a sync tool, not file sharing, not a Kopia replacement. Kopia does the hard
part; this is the management layer a workstation deserves.

## Install

> **Status: in development.** See [CHANGELOG.md](CHANGELOG.md) for what is
> actually built. Not yet suitable as your only copy of anything.

```bash
git clone https://github.com/andreaswiren/superbackup.git
cd superbackup
cargo build --release
```

Self-contained binary — no runtime, no webview, no Node. On Linux:

```bash
sudo apt-get install libgtk-3-dev libxdo-dev libayatana-appindicator3-dev pkg-config
```

**Kopia** is installed for you on first run, fetched from the Kopia project's
GitHub releases with its SHA-256 verified before anything touches disk. A Kopia
already on your `PATH` is preferred, and one you pin explicitly is never
replaced. Updates default to *notifying* rather than installing — swapping the
binary that reads your repositories isn't a decision this app should make for
you.

## Getting started

Run it: the tray icon appears and the window walks you through a master
passphrase, OneDrive detection, and a first job. Or:

```bash
superbackup init
superbackup destination add --onedrive
superbackup job add --name dev --source ~/code --template developer
superbackup run dev
```

## Command line

A thin client — it asks the running instance rather than touching repositories
itself.

```bash
superbackup status              # health, running jobs, next run
superbackup run dev-code --wait # by name, id, or unambiguous prefix
superbackup stop --all
superbackup pause 4h
superbackup snapshots dev-code
superbackup restore dev-code --at 2026-08-29T14:00 --to ~/restored
superbackup watch               # live events as NDJSON
superbackup doctor              # diagnose; non-zero if broken
```

**For agents:** every command takes `--json` and returns a stable envelope with
a machine-readable `error.code`. `superbackup schema --json` emits the entire
command surface, generated from the parser itself so it cannot drift.

## Restoring the vault

The vault (`config.sbvault`) holds the password of every repository you have.
Your file backups are intact without it, but nothing can read them — so it is
worth its own job, and the job is deliberately tiny: the vault is already
encrypted under your master passphrase, so the backup carries the file as it
sits, plus a `RESTORE.txt` written in plain text beside it.

Make the job from **Jobs → New job → Superbackup's vault**. Point it at
whichever destinations you already trust; it is a few kilobytes.

To put it back on a new machine:

```bash
superbackup restore superbackup-vault --to ~/vault-backup
superbackup vault restore --from ~/vault-backup --with-config
```

The second command replaces this machine's vault. Before it does, it refuses
while superbackup is running (a running instance would write its own vault back
over yours), asks for the master passphrase and checks that the backed-up file
actually opens with it, and keeps a dated copy of whatever was there before in
`vault-backups/`. List those copies with `superbackup vault backups`.

Then start superbackup and unlock it. Your destinations, jobs and stored
secrets are back.

**Your master passphrase is not in the backup and cannot be recovered.** Keep
it somewhere other than the folder holding the vault — a password manager, or
written down.

### SSH keys

The same idea, one layer up. Tick the keys you want protected on
**Credentials**, then make a **Jobs → New job → Your SSH keys** job. Every
ticked key is sealed into a single file under your master passphrase *before*
it leaves the process, so what lands in a bucket or in OneDrive is useless
without that passphrase. This is not a setting; there is no plaintext option.

To bring them back:

```bash
superbackup restore ssh-keys --to ~/keys-backup
superbackup cred unseal --from ~/keys-backup
```

Keys are written into `~/.ssh` with owner-only permissions, and files already
there are left alone unless you pass `--overwrite`.

## Security

superbackup holds every key that makes your backups recoverable.

- [Threat model](docs/compliance/THREAT_MODEL.md) — eight adversaries, each
  with its residual risk, and what's explicitly out of scope
- [Privacy](docs/compliance/PRIVACY.md) — every network call it can make
- [Security policy](SECURITY.md) — reporting a vulnerability

Secrets live in a vault sealed with XChaCha20-Poly1305 under an
Argon2id-derived key, never reach a subprocess command line, and are scrubbed
from anything logged. Backup contents are encrypted client-side by Kopia.

Three things worth knowing up front:

- **Folder mirrors are plain, unencrypted copies.** That's their purpose.
- **There is no passphrase recovery.** Lose it and the backups are gone.
- **Kopia's checksum proves integrity, not authenticity** — see
  [A8](docs/compliance/THREAT_MODEL.md).

## Documentation

| | |
|---|---|
| [Architecture](docs/ARCHITECTURE.md) | How it's built and why |
| [UX specification](design/UX_SPEC.md) | Every screen, state and string |
| [Compliance](docs/compliance/) | Threat model, privacy, licences, EU CRA, SBOM |
| [Contributing](CONTRIBUTING.md) | The rules that don't bend |

## Credits

**[Kopia](https://github.com/kopia/kopia)** does the real work — deduplication,
chunking, encryption. Apache-2.0, © Jarek Kowalski and contributors. Invoked as
a separate process; none of its code is included. Not affiliated with or
endorsed by the Kopia project — report superbackup bugs here, not to them.

Scope owes a debt to [Rclone UI](https://github.com/rclone-ui/rclone-ui), and
interface quality to [Arq](https://www.arqbackup.com/) and
[Plakar](https://github.com/PlakarKorp/plakar). No code or assets from any of
them are used.

## Licence

MIT — see [LICENSE](LICENSE).
