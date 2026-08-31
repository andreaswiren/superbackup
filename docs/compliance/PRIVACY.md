# Privacy and Data Handling

Version 2, for superbackup 0.1.x.

## The short version

superbackup has no servers. It sends nothing to us, because there is no "us"
to send it to. There is no telemetry, no analytics, no crash reporting, and no
account.

It does ask GitHub whether a newer version has been published — weekly by
default, and switchable off. That request carries nothing about your
installation, and it is described in full below rather than buried: an earlier
version of this document claimed there was "no update check that phones home",
and once the check existed that sentence had to go rather than be quietly
reinterpreted.

Your data goes exactly where you configure it to go, and nowhere else.

---

## What leaves your machine

Only these, and only because you configured them:

| Destination | What goes there | Encrypted in transit | Encrypted at rest |
|---|---|---|---|
| A Kopia repository (local disk, external drive, network share, OneDrive folder) | Your file contents and metadata | N/A (local) or SMB | **Yes** — Kopia encrypts client-side |
| An S3 bucket (StorJ, AWS, Backblaze, Wasabi, MinIO, R2) | Your file contents and metadata | **Yes** — HTTPS | **Yes** — Kopia encrypts client-side |
| A folder mirror | Your files, **as plain readable copies** | N/A (local) | **No — by design** |
| A shared config Git repository | Only the sealed vault file | **Yes** — HTTPS | **Yes** — the vault is encrypted |

Network connections superbackup makes at all:

1. **To your configured S3 endpoint**, by Kopia, when a job runs.
2. **To your configured Git host**, when you pull or push shared config.
3. **To GitHub's releases API for the Kopia project**, to fetch and verify the
   `kopia` binary — on first run, and when checking whether a newer Kopia
   exists.
4. **To GitHub's releases API for superbackup itself**, to see whether a newer
   version has been published. Once a week by default, and whenever you press
   "Check for updates".

That is the complete list. There is no fifth.

### What an update check discloses

Connections 3 and 4 are unauthenticated `GET` requests for a public list of
releases. They send **no information about your installation**: no machine
identifier, no configuration, no job or folder names, no version history, no
telemetry. There is no account and nothing is correlated across checks.

What they unavoidably reveal to GitHub is what any HTTP request reveals: your
IP address, roughly when superbackup was running, and — from the `User-Agent`
— that it was superbackup and which version. Over time that is enough for
GitHub to infer that a machine at your address runs this software.

If that matters to you, both are switchable:

- Turn off **Settings → Check for updates** to stop connection 4 entirely.
- Set the Kopia update policy to **Never check**, and install Kopia through
  your platform's package manager, to stop connection 3.

Neither affects backups. superbackup never installs an update by itself: it
tells you one exists and you decide. Replacing the binary that is holding your
repository keys, possibly mid-backup, is not a decision this application makes
on your behalf.

---

## The folder mirror is not encrypted

`LocalMirror` destinations are plain file copies. That is what they are for: a
readable copy you can open without any special tooling, on a drive you can hand
to someone.

Anyone with access to that folder has your files. superbackup says so in the
interface at the point where you choose the destination type, and repeats it
when the destination is a removable or network drive. It is called out here
because it is the one place where the obvious assumption ("a backup tool
encrypts things") is wrong.

---

## What is readable at your destination

Kopia encrypts your file contents and names. Some things are still visible to
anyone who can read the destination:

- **The `_superbackup/` manifest folder.** Machine label, hostname, OS and
  version, architecture, an application-scoped UUID, and first/last-seen
  timestamps — for every machine writing there. This is intentional: it is what
  lets a person open a shared drive and work out which folder belongs to which
  PC. If several people share a destination, they can see each other's machine
  names.
- **Volume and timing.** Object sizes and write times reveal roughly how much
  you change and when you work, even though the contents are unreadable.

If either matters to you, use a destination only you can read.

---

## What stays on your machine

| File | Contents | Protection |
|---|---|---|
| `config.json` | Job definitions, folder paths, schedules, bucket names, endpoints — **no secrets** | Filesystem permissions (`0700` on Unix) |
| `config.sbvault` | Every repository passphrase, S3 key pair, and token | Encrypted with your master passphrase |
| `state.json` | Run history, timings, byte counts | Filesystem permissions |
| `events.ndjson` | Activity log | Filesystem permissions; rotated per `log_retention_days` |
| Kopia cache | Content indexes for speed | Filesystem permissions |

Logs record file **counts and sizes**, and paths only when a specific file
fails. Everything written to a log passes through credential redaction first.

## The machine identifier

superbackup generates a random UUID on first run to name your machine's folder
at each destination. It is **not** derived from any hardware serial, MAC
address, or disk ID — deliberately, so that it cannot be correlated with
anything outside this application. It identifies a superbackup installation and
nothing else. Delete the config directory and it is gone.

---

## Your master passphrase

- It is never stored anywhere in any form you could recover it from.
- It is never transmitted.
- It exists in memory only while the vault is unlocked, and is erased on lock
  and on exit. That erasure is best-effort: an operating system that pages
  memory to disk, hibernates, or writes a crash dump can still persist it. We
  state this rather than claim memory hygiene we cannot guarantee.
- **There is no recovery.** If you lose it, your backups are unrecoverable.
  This is a property of the design, not an oversight.

If you enable the optional OS keychain integration, a derived key is stored in
Windows Credential Manager, macOS Keychain, or the Linux Secret Service so that
scheduled backups can run when you are not logged in. This is off by default,
and the interface explains the trade-off where you turn it on.

---

## Regulatory notes

superbackup is a local tool, not a service. There is no controller, no
processor, and no transfer to us, because we receive nothing.

If you back up personal data belonging to other people, **you** are the
controller for it, and your destination provider (StorJ, AWS, Microsoft) is
your processor. Your obligations under the GDPR or equivalent law are between
you and them. superbackup helps in two concrete ways: contents are encrypted
client-side before reaching any provider, and you choose the storage region —
`eu-1` for StorJ, for example — so data residency is under your control.

Right-to-erasure requests reach a Kopia repository through snapshot deletion
and maintenance. Note that retention policy may keep older snapshots containing
the data until they expire; set retention accordingly if this matters to you.

---

## Verifying these claims

You do not have to take our word for it.

```bash
# Every outbound connection the binary makes
superbackup doctor --json

# Watch it live: on Windows use Resource Monitor or Process Monitor,
# on Linux `ss -tp` or strace, on macOS Little Snitch or `lsof -i`.
```

The source is public. Searching it for HTTP clients will find `reqwest` used in
exactly three places: `remote.rs` (shared config), `kopia/install.rs` (fetching
and verifying the Kopia binary), and `update.rs` (the release check). There is
no analytics dependency in `Cargo.toml`, and `cargo deny` would flag one
arriving transitively.

---

Last reviewed: 2026-08-31. Questions: open an issue.
