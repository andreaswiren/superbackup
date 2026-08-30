# superbackup — Threat Model

Version 2, for superbackup 0.1.x.

This document states what superbackup defends against, what it does not, and
why. It is written to be falsifiable: every claim below should be checkable
against the code, and a claim that stops being true is a bug report.

Backup software occupies an unusually sharp position. It reads everything the
user owns, holds the keys that make it recoverable, and writes it somewhere the
user does not fully control. A vague security posture is not acceptable here,
so the awkward parts are stated plainly rather than omitted.

---

## 1. What we are protecting

| Asset | Why it matters | Where it lives |
|---|---|---|
| **Master passphrase** | Unlocks everything below. Not recoverable. | The user's head; transiently in process memory |
| **Master key** | Argon2id output; encrypts the vault | Process memory only, while unlocked |
| **Repository passphrases** | Decrypt Kopia repositories — i.e. the backups themselves | Vault |
| **S3 access/secret keys** | Read, write and *delete* the offsite copy | Vault |
| **GitHub token** | Read/write the shared config repository | Vault |
| **Backup contents** | The user's source code and documents | Kopia repositories, at rest at the destination |
| **Configuration** | Job definitions, paths, schedules | `config.json`, plaintext, local |
| **Machine identity** | Which folder belongs to which PC | Destination manifests, plaintext by design |

The configuration and the machine manifests are deliberately *not* secret.
They contain folder paths, hostnames, and an application-scoped UUID. Making
them readable is the feature that lets a human open a shared drive and work out
whose backup is whose. A user who considers their folder paths sensitive should
treat the destination as sensitive.

---

## 2. Adversaries in scope

### A1 — Someone who obtains the sealed vault file

The realistic case, and the one the design is built around. `config.sbvault`
is intended to be committable to a Git repository, which means assuming it will
end up somewhere more public than intended.

**Defence.** The vault is sealed with XChaCha20-Poly1305 under a key derived
from the master passphrase by Argon2id with memory-hard parameters recorded in
the header. The header is authenticated as associated data, so an attacker
cannot weaken the KDF parameters and re-present the file — tampering makes it
fail to open rather than open cheaply.

**Residual risk.** The whole scheme reduces to the passphrase. A weak
passphrase is offline-crackable and Argon2id only raises the price. This is why
the UI meters strength, refuses to treat a common passphrase as acceptable, and
states plainly at creation time that there is no recovery path.

**What we do not claim.** We do not claim resistance to a well-funded attacker
holding the vault *and* a passphrase drawn from a small space.

### A2 — Another local user on a shared machine

**Defence.** Config and data directories are `0700` on Unix. The vault is
useless without the passphrase regardless of file permissions. The IPC endpoint
is restricted to the owning user — a named pipe with a restrictive DACL and
remote clients rejected on Windows, a `0600` socket in a `0700` directory with
peer-UID verification on Unix. Secrets are never passed to child processes in
`argv`, because `argv` is readable by other users through `/proc` on Linux and
through WMI on Windows; they go through the environment and stdin instead.

**Residual risk.** A local administrator or root can read another process's
memory. Nothing in userspace prevents this and we do not pretend otherwise.

### A3 — Someone who obtains the destination

A stolen external drive, a compromised OneDrive account, leaked StorJ
credentials, or an S3 bucket accidentally made public.

**Defence.** Repository destinations are Kopia repositories: content is
encrypted client-side before it leaves the machine. The destination holds
ciphertext. Kopia's encryption is the guarantee here, not ours — see
[§7](#7-what-we-inherit-from-kopia).

**Explicit exception.** `DestinationKind::LocalMirror` is a **plain, unencrypted
file copy**. That is its entire purpose — a readable copy you can open without
any tooling. Anyone with the mirror has the files. The UI must never let a user
believe a mirror is encrypted, and the copy in `design/COPY.md` says so at the
point of choice.

**Also unencrypted at the destination:** the `_superbackup/` manifest directory
(machine labels, hostnames, OS versions, timestamps) and, inherently, object
sizes and write timing. An observer of the bucket learns roughly how much
changes and when, even though they cannot read what changed.

### A4 — A malicious or compromised shared config repository

Config can be pulled from GitHub. A repository the user does not solely control
is an input channel from a potential attacker.

**Defence.** A pulled vault is treated as an untrusted encrypted blob. It is
never written over the local vault until it has decrypted successfully under a
passphrase the user supplied in this session. When `trusted_signers` is
populated, the vault's detached Ed25519 signature must verify against a pinned
key or the pull is rejected. The local vault is backed up before replacement.
Pulls never happen silently in a way that changes what gets backed up without
the user seeing a diff first. Push is always explicit.

**Residual risk.** A user who pins no signers and shares a passphrase with a
colleague has extended trust to that colleague — correctly, since they can
already read everything. Config-level mischief (redirecting a job to an
attacker's bucket) is possible for anyone who can both write the repository and
knows the passphrase. Pinned signers are the mitigation, and the UI should
encourage them for any repository with more than one writer.

### A5 — A hostile filesystem being backed up

Source trees contain symlinks, junctions, reparse points, cycles, and
adversarially long paths — especially in `node_modules`.

**Defence.** Symlinks are not followed out of the source tree unless the user
opts in per source. The mirror engine refuses to write above its own root,
rejects a destination nested inside its own source, and is long-path safe on
Windows. Destination paths are validated against the job's own sources at save
time, because a destination inside a source is an unbounded-growth footgun that
should never reach the scheduler.

### A6 — Hostile output from a subprocess

Kopia's stderr, a Git transport error, and an S3 SDK message are all
third-party text that we display, log, and put in notifications. Such text has
historically echoed credentials back.

**Defence.** `redact::scrub` runs over everything before it can reach a log, an
event, an IPC response, or a notification. It masks URL userinfo and
credential-shaped assignments. It is deliberately over-eager: a redacted
diagnostic is a nuisance, a leaked repository key is unrecoverable. It is a
safety net behind the primary control (secrets never enter `argv`), not a
substitute for it.

### A7 — A malicious local process talking to the IPC endpoint

**Defence.** The endpoint is restricted to the owning user, rejects remote
clients, caps line length so a client cannot exhaust memory, caps concurrent
connections, and rate-limits per connection. **The daemon never returns
plaintext secrets over IPC** — there is no `GetSecret` request, only `SetSecret`.
Reading a secret out of superbackup requires the passphrase and is not a thing
the protocol offers.

**Residual risk.** A process running as the same user can already read the
user's files directly; the endpoint does not meaningfully widen that. It does
grant the ability to *trigger* operations, which is why destructive requests
(delete a snapshot, change a passphrase) require the vault to be unlocked.

On Windows, peer identity is pid-only — `interprocess` exposes no caller token
or SID, and pid-based lookup races with pid reuse. **The DACL is therefore the
only access control on Windows**, which is why `bind` fails outright rather
than falling back to a default-permissioned pipe if the descriptor cannot be
constructed.

One residual copy of a passphrase exists by construction: `Request` is an
internally-tagged enum, so `serde` buffers the frame before dispatching on the
command name, and `vault.unlock`'s passphrase transiently exists in that
buffer. It never lands in a `String` field, never appears in `Debug`, never
reaches a log, and is never returned; the socket read buffer is zeroed after
every frame. Removing the copy entirely would require adjacent tagging
(`{"cmd":…,"params":{…}}`), at the cost of a wire format a human can type by
hand — which for a protocol whose debuggability is a stated design goal was
judged the worse trade.

### A8 — A malicious `kopia` binary

**This was out of scope until superbackup started installing Kopia itself.**
Earlier versions of this document said the mitigation was operational — "pin
and verify the binary you install". Once the application downloads and executes
that binary on first run, the verification is our responsibility, not the
user's, and the honest position has to change with it.

**Defence.** The download is fetched over TLS from a pinned upstream repository
(`kopia/kopia` by default), restricted to GitHub hosts with redirects off
GitHub refused before a connection is made. The archive is held **in memory**
and its SHA-256 compared against the `checksums.txt` published with the same
release *before anything touches disk*, so a mismatch has nothing to clean up.
Every archive member is checked for path traversal even after the wanted
executable has been found. Installation is temp-file → `--version` probe →
atomic rename, so a partially written executable is never left where the
resolver would find it. A version floor is enforced and downgrades are refused.
A binary the user installed themselves — via `Settings::kopia_path` or on
`PATH` — is never replaced.

**Residual risk, stated plainly.** Kopia publishes `checksums.txt.sig`, but not
a signing key in a form this project can pin, so **the signature is not
verified**. The checksum proves the download matches what that release
published; it does not prove who published it. Authenticity therefore rests on
TLS to `github.com` and on GitHub itself. Anyone able to publish a release to
`kopia/kopia` — or to compromise GitHub — defeats this. That is the same
exposure as `curl | tar x`, and no worse, but it is not the same as a verified
signature and must not be described as one.

This is surfaced in the API rather than buried here:
`InstallOutcome::signature_verified` is `false`, so the interface states the
guarantee accurately. Users wanting a stronger chain should install Kopia
through their platform's package manager, which does verify signatures, and
point `Settings::kopia_path` at it. Auto-install can be turned off entirely.

**Also note:** superbackup hands repository passphrases to whatever binary it
resolves. A substituted Kopia has those keys by construction. `doctor` reports
the resolved path, version, and whether the binary is the managed one, so a
substitution is at least visible.

---

## 3. Adversaries out of scope

Stated so the boundary is honest rather than implied:

- **Malware running as the user with the vault unlocked.** It can read process
  memory, keystrokes, and the files directly. No userspace design survives this.
- **A compromised operating system, firmware, or hypervisor.**
- **Physical attacks** — cold boot, DMA, a debugger on a live process.
- **A compromise of the Kopia project or of GitHub itself.** See
  [A8](#a8--a-malicious-kopia-binary): we verify the download's checksum but
  cannot verify its signature, so a release published by an attacker with
  upstream access is not detected.
- **Traffic analysis of the destination.** Sizes and timing leak.
- **The user losing their passphrase.** By design there is no recovery. This is
  a property, not a gap — but it is the single most likely way a real user loses
  their data, so the UI treats passphrase creation as a serious moment.

---

## 4. Cryptographic design

| Purpose | Primitive | Rationale |
|---|---|---|
| Passphrase → key | Argon2id | Memory-hard; the current standard for password hashing. Parameters live in the file header so they can be raised without breaking old vaults. |
| Vault encryption | XChaCha20-Poly1305 | AEAD with a 192-bit nonce, so random nonces are safe without a counter. Authenticates the header as AAD. |
| Subkey derivation | HKDF-SHA256 | Purpose-separated subkeys. The master key is never used directly for two different jobs. |
| Config signing | Ed25519 | Small, fast, misuse-resistant signatures for the shared-config trust path. |
| Random material | OS CSPRNG | Generated passphrases are 256 bits from the OS. |

**Design rules the code is expected to hold to:**

1. No secret is ever `Serialize`d, `Display`ed, or `Debug`-printed. `Secret`
   has no such implementation that reveals contents, and tests assert it.
2. Secret buffers are zeroed on drop. This is best-effort: paging, hibernation
   and crash dumps can still persist plaintext, and we say so rather than
   claiming memory hygiene we cannot deliver on a general-purpose OS.
3. Comparisons of secret values are constant-time.
4. A wrong passphrase and a corrupt file are the same class of failure, and
   neither partially applies a change.
5. Nothing is rolled by hand. Every primitive above comes from an established,
   audited crate.

---

## 5. Key handling and the unattended problem

There is a genuine tension between *"schedules must run without the user"* and
*"the key must not sit on disk"*. We resolve it by refusing to hide it:

- **Default.** The master key exists only in memory, only while unlocked, and
  is dropped after `auto_lock_minutes` of inactivity. A locked vault blocks
  scheduled runs, and the tray shows `Attention` so this is never silent.
- **Opt-in.** `use_os_keychain` caches the key in the platform keychain
  (DPAPI-backed Credential Manager, Keychain, Secret Service) so the service can
  run unattended. This trades a real amount of security for the ability to back
  up a machine nobody is logged into. It is off by default and the UI states the
  trade-off at the point of choice rather than in a footnote.
- **Service caveat.** A Windows service running as `LocalSystem` cannot read
  the user's DPAPI-protected credentials and cannot see the user's OneDrive
  folder. That is a platform fact, not a bug; the service installer must
  surface which destination kinds work in which account configuration.

---

## 6. Data flow

```
  user passphrase
        │  Argon2id (params from vault header, header authenticated as AAD)
        ▼
   master key ──HKDF──┬─► vault key ──XChaCha20-Poly1305──► config.sbvault
                      ├─► repo-passphrase derivation (per destination UUID)
                      └─► Ed25519 signing key (shared-config trust)

  vault (unlocked, in memory)
        │  environment variables + stdin — never argv
        ▼
   kopia child process ──client-side encryption──► destination
        │
        └─ stdout/stderr ──redact::scrub──► logs, events, IPC, notifications
```

---

## 7. What we inherit from Kopia

superbackup does not implement backup encryption. Kopia does. The
confidentiality of backed-up content is Kopia's property, under the algorithms
selected at repository creation (AES-256-GCM-HMAC-SHA256 by default).

This means Kopia's security posture is ours. We manage that dependency by
enforcing a documented minimum version at startup, refusing downgrades,
verifying the checksum of any build we install ourselves, and reporting the
resolved binary path and version in `doctor` so a substituted binary is
visible. The limits of that verification are set out in
[A8](#a8--a-malicious-kopia-binary) — in particular, we check integrity but
not authenticity. Kopia is Apache-2.0; see [`THIRD_PARTY.md`](THIRD_PARTY.md).

---

## 8. Reporting

Security issues: see [`SECURITY.md`](../../SECURITY.md) in the repository root.
Please do not open a public issue for a vulnerability.

## 9. Review

This document is reviewed when the crypto design changes, when a new
destination kind or credential type is added, when the IPC surface gains a
request that returns sensitive data, and at each minor release.

| Version | Date | Change |
|---|---|---|
| 1 | 2026-08-30 | Initial model for 0.1.0. |
| 2 | 2026-08-31 | Added A8 (Kopia binary integrity) after auto-install moved that verification from the user to this application. Recorded the Windows pid-only peer identity and the transient passphrase copy in serde's buffer under A7. |
