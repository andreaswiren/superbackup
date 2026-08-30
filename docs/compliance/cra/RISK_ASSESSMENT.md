# Cybersecurity risk assessment

Article 13(2)–(3) of Regulation (EU) 2024/2847. Version 1, for superbackup
0.1.x. Last reviewed 2026-08-31.

Article 13(3) requires the assessment to comprise *"at least an analysis of
cybersecurity risks based on the intended purpose and reasonably foreseeable
use, as well as the conditions of use … such as the operational environment or
the assets to be protected, taking into account the length of time the product
is expected to be in use"*, and to indicate whether and how each Part I point
(2) requirement applies.

This document is the risk analysis. The requirement-by-requirement mapping the
same paragraph asks for is [`ANNEX_I_PART_I.md`](ANNEX_I_PART_I.md).

**This is derived from [`THREAT_MODEL.md`](../THREAT_MODEL.md), not a
replacement for it.** The threat model is where the security design is argued;
this document restates it in the form Article 13 asks for and adds risks the
threat model does not cover because they are process risks rather than design
risks. Where the two appear to disagree, the threat model is right and this
document is a bug. In particular, every adversary the threat model places out of
scope in §3 is still out of scope here, and is recorded as an **accepted** risk
rather than quietly promoted into a treated one.

---

## 1. Methodology

Qualitative, asset-and-adversary-centric. No pretence of quantitative
precision: there is no incident data for this product, and a number derived
from nothing is worse than a judgement stated as one.

**Scope of the assessment.** The superbackup application and the processes
around it, in its intended environment: a single-user or trusted-multi-user
desktop or laptop, running Windows, Linux or macOS, where the user controls the
account superbackup runs as. Kopia is assessed as a dependency — as a component
whose security posture superbackup inherits and manages — not as a product.

**Expected time in use.** Article 13(3) requires this to be taken into account.
superbackup is a persistently installed background application expected to run
for years on the same machine, with a declared support period of five years per
release ([`SUPPORT_POLICY.md`](SUPPORT_POLICY.md)). Two consequences shape the
assessment: cryptographic parameters must be raisable without breaking existing
vaults, which is why Argon2id parameters live in the file header; and a
credential stored today must be assumed to remain valuable for years.

**Likelihood**, judged for a typical installation over the expected time in use:

| | |
|---|---|
| **Rare** | Requires a capability or circumstance that the threat model excludes, or an attacker with resources disproportionate to the target |
| **Unlikely** | Plausible but requires an unusual combination of user action and attacker position |
| **Possible** | Will happen to some installations; a competent attacker in the right position achieves it |
| **Likely** | Should be expected to occur during the product's life |

**Impact**, on the user's confidentiality, integrity and availability:

| | |
|---|---|
| **Low** | Disclosure of non-sensitive metadata; recoverable inconvenience |
| **Moderate** | Disclosure of configuration or machine metadata; a backup does not run and the user is told |
| **High** | Disclosure of some backed-up content; a backup silently does not run; a credential for one destination is exposed |
| **Severe** | The master passphrase or master key is exposed; all backed-up content is readable by an attacker; all backups are permanently unrecoverable |

**Risk rating**, likelihood × impact:

| | Low | Moderate | High | Severe |
|---|---|---|---|---|
| **Likely** | Low | Medium | High | Critical |
| **Possible** | Low | Medium | High | High |
| **Unlikely** | Low | Low | Medium | High |
| **Rare** | Low | Low | Low | Medium |

**Treatment.** Each risk is *mitigated* (a control reduces it), *accepted* (the
residual is tolerated and stated), or *transferred* (it belongs to the user or
to a third party, and is disclosed so that the user can act). Nothing is
*avoided* by removing a feature, because the features that carry the risk are
the product.

**Residual risk** is the rating after the stated mitigations, and it is the
column that matters. A risk assessment whose residual column is all "Low" has
been written backwards.

---

## 2. Assets

From [`THREAT_MODEL.md`](../THREAT_MODEL.md) §1, unchanged:

| Asset | Sensitivity | Where it lives |
|---|---|---|
| Master passphrase | **Severe** — unlocks everything, not recoverable | The user's head; transiently in process memory |
| Master key | **Severe** — Argon2id output, encrypts the vault | Process memory only, while unlocked. Optionally the OS keychain |
| Repository passphrases | **Severe** — decrypt the backups themselves | Vault |
| S3 access and secret keys | **High** — read, write and *delete* the offsite copy | Vault |
| GitHub token | **High** — read/write the shared configuration repository | Vault |
| Backed-up content | **Severe** — the user's source code and documents | Kopia repositories at the destination; plaintext in a `LocalMirror` |
| Configuration | **Moderate** — job definitions, paths, schedules | `config.json`, plaintext, local; and inside the sealed vault when published |
| Machine identity and manifests | **Low** — deliberately not secret | Destination manifests, plaintext by design |
| Run history and event log | **Low–Moderate** — timings, byte counts, failing paths | `state.json`, `events.ndjson`, local |

The configuration and machine manifests are deliberately not secret. Making them
readable is the feature that lets a human open a shared drive and work out
whose backup is whose. A user who considers folder paths sensitive should treat
the destination as sensitive — that is the threat model's position and it is
unchanged here.

---

## 3. Threat actors

| | Capability | Motivation |
|---|---|---|
| **TA-1 Opportunistic finder** | Reads a repository, a lost drive or a misconfigured bucket that they came across | Curiosity, opportunistic credential harvesting |
| **TA-2 Local unprivileged user** | Runs code as a different account on the same machine | Access another user's data |
| **TA-3 Local process running as the user** | Runs code as the same account | Escalate from "can read files" to "can read keys" |
| **TA-4 Colleague or collaborator** | Legitimate write access to a shared configuration repository | Misuse of granted trust; a compromised collaborator account |
| **TA-5 Network adversary** | On-path between the machine and a destination or a release host | Interception, substitution |
| **TA-6 Supply-chain adversary** | Publishes a malicious crate version, or compromises an upstream release | Broad compromise of many downstream users |
| **TA-7 Well-resourced attacker with the vault** | Offline compute against a captured vault file | Targeted access to a specific person's data |
| **Out of scope** | Malware running as the user with the vault unlocked; a compromised OS, firmware or hypervisor; physical attacks including cold boot and DMA; a compromise of the Kopia project or of GitHub itself; traffic analysis of the destination | Excluded in `THREAT_MODEL.md` §3 |

---

## 4. Risk register

Sixteen risks arising from the design, and four from the process. Each maps to a
threat-model section where one exists.

### Design risks

---

**R-01 · Sealed vault obtained and cracked offline against a weak passphrase**
*Threat model §A1 · TA-1, TA-7*

The vault is intended to be committable to a Git repository, which means
assuming it will end up somewhere more public than intended. If the master
passphrase is drawn from a small space, Argon2id only raises the price.

| | |
|---|---|
| Inherent | Likely × Severe = **Critical** |
| Mitigations | XChaCha20-Poly1305 under an Argon2id key at m=256 MiB, t=3, p=1, with a floor of m=64 MiB, t=3 that a new vault cannot be created below (`crates/core/src/crypto/kdf.rs`, `validate_for_new_vault`). Parameters are recorded in the header **and authenticated as associated data**, so they cannot be weakened and the file re-presented — `a_tampered_header_breaks_decryption_rather_than_being_accepted`. Strength is measured live and a common passphrase is refused an acceptable rating (`secret::estimate_strength`); `Strength::is_acceptable` requires at least `Fair`. The interface states at creation time that there is no recovery (`design/COPY.md`, `O-3`). |
| Residual | Possible × Severe = **High** |
| Treatment | Mitigated. The residual is genuinely high and is the design's central assumption: the whole scheme reduces to the passphrase. `THREAT_MODEL.md` §A1 states plainly that resistance to a well-funded attacker holding the vault *and* a passphrase from a small space is **not claimed**. |

---

**R-02 · Sealed vault published to a public repository by accident**
*Threat model §A1 · TA-1*

The vault is designed to be committed. A user who commits it to a public
repository has done what the design invited, in the wrong place.

| | |
|---|---|
| Inherent | Possible × Severe = **High** |
| Mitigations | This is precisely the case the design assumes: R-01's controls are the mitigation, and they were chosen for it. The vault is ASCII-only so Git on Windows cannot corrupt it (`the_file_is_pure_ascii_so_git_on_windows_cannot_corrupt_it`) — deliberate, because a design that expects to be committed must survive being committed. |
| Residual | Possible × Severe = **High**, collapsing to R-01 |
| Treatment | Mitigated, and disclosed. The remedy after exposure is passphrase rotation and rotation of every credential the vault held, which is a documentation item — see the gap list. |

---

**R-03 · Another local user reads secrets or drives the daemon**
*Threat model §A2, §A7 · TA-2*

| | |
|---|---|
| Inherent | Possible × Severe = **High** |
| Mitigations | Config and data directories `0700`, files `0600` on Unix (`crates/core/src/paths.rs`). The IPC endpoint is owner-restricted with **no permissive fallback** — if the platform will not grant the protection asked for, binding fails (`crates/core/src/ipc/security.rs`). On Windows, an explicit DACL with no "everyone" ACE and `PIPE_REJECT_REMOTE_CLIENTS`; on Unix, `fchmod` before `bind` plus an `SO_PEERCRED` uid check on every accepted connection. **No IPC request returns a plaintext secret** — `SetSecret` exists and `GetSecret` deliberately does not, asserted by `there_is_no_request_that_reads_a_secret_back`. Secrets never enter `argv`, because `argv` is readable through `/proc` on Linux and WMI on Windows; enforced mechanically by `KopiaCommand::audit_argv` and tested by `secrets_never_reach_argv` and `the_audit_catches_a_secret_placed_in_argv`. The vault is useless without the passphrase regardless of file permissions. |
| Residual | Unlikely × High = **Medium** |
| Treatment | Mitigated. |

---

**R-04 · A local administrator or root reads process memory**
*Threat model §A2 residual, §3 · out of scope*

| | |
|---|---|
| Inherent | Unlikely × Severe = **High** |
| Mitigations | None available. Nothing in userspace prevents it. Secrets are zeroed on drop (`zeroize`) and the key is dropped after `auto_lock_minutes`, which narrows the window without closing it. |
| Residual | Unlikely × Severe = **High** |
| Treatment | **Accepted and disclosed.** `THREAT_MODEL.md` §A2 says so and does not pretend otherwise. Memory hygiene is explicitly best-effort: paging, hibernation and crash dumps can persist plaintext, and §4 rule 2 states it. |

---

**R-05 · The destination is obtained — a Kopia repository**
*Threat model §A3 · TA-1, TA-5*

A stolen external drive, a compromised OneDrive account, leaked StorJ
credentials, a bucket accidentally made public.

| | |
|---|---|
| Inherent | Possible × Severe = **High** |
| Mitigations | Content is encrypted client-side by Kopia before it leaves the machine (AES-256-GCM-HMAC-SHA256 by default); the destination holds ciphertext. Transport to an S3 endpoint is HTTPS. Repository passphrases are derived per destination UUID from purpose-separated HKDF subkeys, so one destination's compromise does not hand over another's. |
| Residual | Unlikely × Severe = **High** |
| Treatment | Mitigated, and **transferred in part**. The confidentiality guarantee is Kopia's, not superbackup's. `THREAT_MODEL.md` §7 states this rather than claiming it, and manages it by pinning a floor version and reporting the resolved binary in `doctor`. |

---

**R-06 · The destination is obtained — a folder mirror**
*Threat model §A3 explicit exception · TA-1*

`DestinationKind::LocalMirror` is a plain, unencrypted file copy.

| | |
|---|---|
| Inherent | Possible × Severe = **High** |
| Mitigations | **None, by design.** A readable copy you can open without any tooling is the entire purpose. The mitigation is disclosure at the moment of choice: `design/COPY.md`, `dest.mirror.explain` — "no deduplication and no encryption — anyone who can read the folder can read your files" — repeated for removable and network drives, and stated in `PRIVACY.md`, `THREAT_MODEL.md` §A3 and the README. |
| Residual | Possible × Severe = **High** |
| Treatment | **Accepted, and transferred to the user by informed choice.** The one obligation this creates is absolute: the interface must never let a user believe a mirror is encrypted. That is a threat-model requirement, and a regression in that copy is a security defect. |

---

**R-07 · Metadata disclosure at the destination**
*Threat model §A3 · TA-1*

The `_superbackup/` manifest directory holds machine label, hostname, OS and
version, architecture, an application-scoped UUID and first/last-seen
timestamps, in clear. Object sizes and write timing leak how much changes and
when.

| | |
|---|---|
| Inherent | Likely × Moderate = **Medium** |
| Mitigations | Intentional and disclosed: readable manifests are the feature that lets a person open a shared drive and work out which folder belongs to which PC. The machine identifier is a random UUID, deliberately **not** derived from any hardware serial, MAC address or disk ID, so it cannot be correlated with anything outside this application (`PRIVACY.md`). |
| Residual | Likely × Moderate = **Medium** |
| Treatment | **Accepted and disclosed.** `PRIVACY.md`: "If either matters to you, use a destination only you can read." Traffic analysis of the destination is out of scope per `THREAT_MODEL.md` §3. |

---

**R-08 · A malicious or compromised shared configuration repository**
*Threat model §A4 · TA-4, TA-6*

Configuration pulled from a repository the user does not solely control is an
input channel from a potential attacker. The interesting attack is redirecting a
job to the attacker's bucket.

| | |
|---|---|
| Inherent | Possible × Severe = **High** |
| Mitigations | A pulled vault is treated as an untrusted encrypted blob and is never written over the local vault until it has decrypted under a passphrase supplied in this session. Where `trusted_signers` is populated, a detached Ed25519 signature must verify against a pinned key or the pull is rejected — **including in a build that cannot verify, which fails closed** (`pinning_a_signer_fails_closed_in_a_build_that_cannot_verify`). The local vault is backed up before replacement. A pull shows a diff first; push is always explicit. No `git` subprocess is used at all, so a token never reaches an askpass helper, `.git/config`, or `git`'s own error text (`crates/core/src/remote.rs`). Tested by `a_pull_with_the_wrong_passphrase_never_reaches_the_disk`, `a_tampered_remote_vault_is_rejected_before_anything_is_written`, `garbage_served_instead_of_a_vault_is_rejected`. |
| Residual | Unlikely × High = **Medium**, rising to **High** where no signers are pinned |
| Treatment | Mitigated. Residual is stated in `THREAT_MODEL.md` §A4: a user who pins no signers and shares a passphrase has extended trust to that person — correctly, since they can already read everything. Pinned signers are the mitigation, and the interface should push them for any repository with more than one writer. |

---

**R-09 · A hostile source filesystem**
*Threat model §A5 · TA-3*

Symlinks, junctions, reparse points, cycles and adversarially long paths,
especially in `node_modules`.

| | |
|---|---|
| Inherent | Possible × High = **High** |
| Mitigations | Symlinks are not followed out of the source tree unless the user opts in per source. The mirror engine re-checks every write and delete against the canonical target root *after* canonicalisation, so a crafted name or a symlink cannot escape; it refuses to operate on a filesystem root, refuses a destination nested inside its own source, and is long-path safe on Windows (`crates/core/src/engine/mirror.rs`, `guard_containment`). Destination paths are validated against the job's own sources at save time, because a destination inside a source is an unbounded-growth footgun that should never reach the scheduler. |
| Residual | Unlikely × Moderate = **Low** |
| Treatment | Mitigated. |

---

**R-10 · Credentials echoed back in third-party output**
*Threat model §A6 · TA-3*

Kopia's stderr, a Git transport error and an S3 SDK message are third-party text
that superbackup displays, logs and puts in notifications. Such text has
historically echoed credentials back.

| | |
|---|---|
| Inherent | Possible × Severe = **High** |
| Mitigations | Two layers. Primary: secrets never enter `argv` and reach the child through the environment and stdin, with the child environment built **from empty** rather than inherited. Secondary: `redact::scrub` runs over everything before it can reach a log, an event, an IPC response or a notification, masking URL userinfo and credential-shaped assignments, deliberately over-eager (`crates/core/src/redact.rs`). Tested by `outbound_frames_are_scrubbed`; `a_passphrase_never_appears_in_debug_output` covers the type-level half. |
| Residual | Unlikely × High = **Medium** |
| Treatment | Mitigated. The residual is that redaction is pattern-based and a novel credential shape could pass it — which is exactly why it is described as a safety net behind the primary control rather than as the control. |

---

**R-11 · A malicious local process talking to the IPC endpoint**
*Threat model §A7 · TA-3*

| | |
|---|---|
| Inherent | Possible × High = **High** |
| Mitigations | The endpoint is a privilege boundary: owner-restricted, remote clients refused, line length capped so a client cannot exhaust memory, concurrent connections capped, requests rate-limited per connection. No request returns a plaintext secret. Destructive requests — delete a snapshot, change a passphrase — require the vault to be unlocked. Tested by `requests_are_rate_limited_per_connection`, `the_connection_limit_refuses_politely`, `an_oversized_line_is_refused_without_buffering_it`, `a_panicking_handler_costs_one_request_not_the_daemon`. |
| Residual | Unlikely × Moderate = **Low** |
| Treatment | Mitigated. `THREAT_MODEL.md` §A7 is right that a process running as the same user can already read the user's files directly, so the endpoint does not meaningfully widen that; what it grants is the ability to *trigger* operations, which is why the destructive ones are gated. |

---

**R-12 · Unattended operation with the OS keychain enabled**
*Threat model §5 · TA-3*

`use_os_keychain` caches the master key in the platform credential store so a
service can run when nobody is logged in.

| | |
|---|---|
| Inherent | Possible × Severe = **High** |
| Mitigations | **Off by default.** The interface states the trade-off at the point of choice rather than in a footnote — `design/COPY.md`, `onboarding.service.keychain_warn`: "Anything that can run programs as you can then ask the credential store for the key." The store is the platform's own (DPAPI-backed Credential Manager, Keychain, Secret Service), so the protection is the platform's. |
| Residual | Unlikely × Severe = **High**, but only for users who opt in |
| Treatment | **Accepted by informed user choice.** `THREAT_MODEL.md` §5 states the tension honestly — schedules must run without the user, and the key must not sit on disk — and resolves it by refusing to hide the trade-off. |

---

**R-13 · A substituted or malicious `kopia` binary**
*Threat model §A8, §7 · TA-6*

| | |
|---|---|
| Inherent | Unlikely × Severe = **High** |
| Mitigations at the *delivery* stage | The archive is fetched over TLS from a pinned upstream repository, restricted to a GitHub host allowlist enforced **on the redirect policy itself**, so no request is ever issued to a foreign host. It is held **in memory** and its SHA-256 compared against the `checksums.txt` published with the same release *before anything touches disk*, so a mismatch has nothing to clean up. A release with no checksum file is refused. Every archive member is checked for path traversal. Installation is temp file → `--version` probe → atomic rename, so a partially written executable is never left where the resolver would find it. A version floor is enforced, downgrades are refused, and a binary the user installed themselves is never replaced. (`crates/core/src/kopia/install.rs`, tested throughout `crates/core/tests/kopia_install.rs`.) |
| Residual | Unlikely × Severe = **High** |
| Treatment | **Mitigated at delivery; the authenticity limb accepted.** `THREAT_MODEL.md` §A8 promoted this from an operational caveat to an in-scope adversary when the application started installing Kopia itself, and states the residual: Kopia publishes `checksums.txt.sig` but not a signing key this project can pin, so **the signature is not verified**. The checksum proves the download matches what that release published; it does not prove who published it. Authenticity rests on TLS to `github.com` and on GitHub — the same exposure as `curl \| tar x`, and no worse, but not a verified signature and not to be described as one. §3 accordingly places **a compromise of the Kopia project or of GitHub itself** out of scope. Two further points: a substituted Kopia has the repository passphrases by construction, since superbackup hands them to whatever binary it resolves; and `doctor` reporting the resolved path, version and whether the binary is the managed one is what makes a substitution visible. Users wanting a stronger chain should install Kopia through a package manager that verifies signatures and point `Settings::kopia_path` at it; auto-install can be turned off entirely. |

---

**R-14 · A compromised crate in the dependency tree**
*TA-6*

| | |
|---|---|
| Inherent | Unlikely × Severe = **High** |
| Mitigations | `deny.toml` restricts sources to crates.io with `unknown-registry = "deny"` and `unknown-git = "deny"`; bans `openssl`, `openssl-sys` and `native-tls` so no C TLS library enters a security-sensitive binary; fails on any RustSec advisory with `ignore = []` and on any yanked crate; and holds a licence allowlist with no exceptions. `Cargo.lock` is committed, so builds are pinned. The SBOM republishes each crate's SHA-256 and is scanned independently. |
| Residual | Unlikely × Severe = **High** |
| Treatment | Mitigated. The residual is irreducible for any project consuming a public registry: `cargo-deny` catches a *known* bad crate, not a crate that is bad and not yet known. Reducing it further means fewer dependencies, and the 647-component graph is largely the GUI stack. `cargo-vet` or `cargo-crev` review is on the gap list. |

---

**R-15 · Denial of service or resource exhaustion from hostile input**
*Threat model §A6, §A7 · TA-3*

| | |
|---|---|
| Inherent | Possible × Moderate = **Medium** |
| Mitigations | A hostile vault header cannot cause a large allocation (`a_hostile_header_cannot_make_us_allocate_gigabytes`); an oversized config is refused before parsing; captured Kopia stdout is capped at 64 MiB and stderr at a bounded tail; collected warnings are capped; the IPC line length, connection count and request rate are all capped; a slow subscriber is dropped-oldest with a "you missed N events" marker rather than buffered without bound; a hanging handler is abandoned. Malformed input is answered and the connection survives. |
| Residual | Unlikely × Low = **Low** |
| Treatment | Mitigated. |

---

**R-16 · Loss of the master passphrase**
*Threat model §3 · not an attack*

| | |
|---|---|
| Inherent | Possible × Severe = **High** |
| Mitigations | None cryptographic, by design: there is no recovery path and no escrow, because either would be an alternative way in. The mitigation is entirely in the interface — `design/COPY.md` `O-3 There is no recovery` requires an explicit acknowledgement ("If I lose it, my backups cannot be recovered"), offers a copy-to-clipboard with a 60-second clear, and offers a recovery sheet with the warning that it is a plain text file. |
| Residual | Possible × Severe = **High** |
| Treatment | **Accepted, and disclosed as a property rather than a gap.** `THREAT_MODEL.md` §3 names this as the single most likely way a real user loses their data, which is why passphrase creation is treated as a serious moment rather than a form field. This is an availability risk that a competing design would trade for a confidentiality risk; the trade is deliberate and stated. |

---

### Process risks

---

**R-17 · No signed releases and no update distribution channel**
*Not in the threat model — a process risk arising from the gaps in
[`ANNEX_I_PART_II.md`](ANNEX_I_PART_II.md) point (7)*

There is no release build workflow, no code signing, no published checksums, no
package-manager presence and no update mechanism. Two consequences: users
continue running versions with known vulnerabilities because nothing tells them
not to; and a binary distributed by any route has no integrity property a user
can check.

| | |
|---|---|
| Inherent | Likely × High = **High** |
| Mitigations | Partial only. `sbom.yml` produces SLSA build provenance for the SBOM artefacts and is already written to bind the SBOM to release binaries the moment any exist. `cargo deny` and the SBOM scan keep the *source* free of known advisories, which does nothing for a user running last year's build. |
| Residual | Likely × High = **High** |
| Treatment | **Open.** The highest-priority item on [`CONFORMITY_CHECKLIST.md`](CONFORMITY_CHECKLIST.md). This is the largest real risk in this document, and it is a process risk rather than a design one — which is why a threat model focused on adversaries did not surface it and a CRA assessment does. |

---

**R-18 · A Kopia advisory goes unnoticed**
*Threat model §7 · TA-6*

Kopia's security posture is superbackup's. No automation watches Kopia's
advisories; raising `MINIMUM_KOPIA_VERSION` is a manual maintainer action.

| | |
|---|---|
| Inherent | Possible × High = **High** |
| Mitigations | The *mechanism* to act exists and is strong: raising the floor makes superbackup refuse to drive an affected build with an actionable message, the installer refuses a release below the configured minimum and below the hard floor however the settings are written, and downgrades are refused. `SECURITY.md` asks reporters of Kopia issues for a heads-up so the floor can be raised. What is missing is the trigger, not the response. |
| Residual | Possible × High = **High** |
| Treatment | **Open.** On the gap list: subscribe to Kopia's security advisories, and add a scheduled CI job that fails when the floor is below the newest Kopia release carrying a security fix. |

---

**R-19 · Single-maintainer dependency**
*Not in the threat model*

One person triages, fixes and releases. Unavailability means no security
response. There is no successor, no second committer, and no organisation.

| | |
|---|---|
| Inherent | Possible × High = **High** |
| Mitigations | Disclosure, and only disclosure: `SECURITY.md` states that this is a personal open-source project rather than a funded programme, that there is no bug bounty, and that timelines depend on maintainer availability. [`SUPPORT_POLICY.md`](SUPPORT_POLICY.md) states what happens if the project is abandoned. MIT licensing means anyone may fork and continue. |
| Residual | Possible × High = **High** |
| Treatment | **Accepted and disclosed.** Users choosing a backup tool for data they cannot lose should weigh this explicitly. It is also the reason the support commitment in `SUPPORT_POLICY.md` is deliberately modest: an unkeepable promise is worse than a small one. |

---

**R-20 · False confidence from a silent backup failure**
*Not in the threat model — an integrity-of-reporting risk*

A backup tool that reports success when a destination was skipped is worse than
one that says nothing, because the user stops checking.

| | |
|---|---|
| Inherent | Possible × Severe = **High** |
| Mitigations | Structural. `JobRun::derive_status()` is deliberately the only place that decides whether a run succeeded, and a run whose destinations partly failed or were skipped resolves to `SucceededWithWarnings`, never `Succeeded`. Every scheduling rejection is a recorded `Skipped` with a reason the interface can show. A locked vault blocks scheduled runs and the tray shows `Attention`, so a locked vault is never silent. A destination failing does not silently take the others down. Warnings downgrade success without failing the run (`warnings_downgrade_success_without_failing_the_run`); a run with no destinations is skipped, not counted as a success (`a_run_with_no_destinations_is_skipped_not_succeeded`). |
| Residual | Unlikely × High = **Medium** |
| Treatment | Mitigated in the core; **surface pending** — the tray `Health` state and the activity view that make this visible to a user are specified in `design/UX_SPEC.md` and not yet wired. Until they are, the honest residual for a shipped product would be higher. |

---

## 5. Summary

| Id | Risk | Residual |
|---|---|---|
| R-01 | Vault obtained, weak passphrase | **High** |
| R-02 | Vault published publicly by accident | **High** (collapses to R-01) |
| R-03 | Another local user reads secrets or drives the daemon | Medium |
| R-04 | Local administrator or root reads process memory | **High** — accepted, out of scope |
| R-05 | Kopia repository destination obtained | **High** — largely transferred to Kopia |
| R-06 | Folder mirror destination obtained | **High** — accepted by informed choice |
| R-07 | Metadata disclosure at the destination | Medium — accepted and disclosed |
| R-08 | Malicious shared configuration repository | Medium; High with no pinned signers |
| R-09 | Hostile source filesystem | Low |
| R-10 | Credentials echoed in third-party output | Medium |
| R-11 | Malicious local process on the IPC endpoint | Low |
| R-12 | OS keychain enabled for unattended runs | **High** — accepted by informed choice |
| R-13 | Substituted or malicious `kopia` binary | **High** — mitigated at delivery; the authenticity limb accepted |
| R-14 | Compromised crate in the dependency tree | **High** |
| R-15 | Resource exhaustion from hostile input | Low |
| R-16 | Loss of the master passphrase | **High** — accepted, a property of the design |
| R-17 | No signed releases, no update channel | **High** — **open, highest priority** |
| R-18 | A Kopia advisory goes unnoticed | **High** — open |
| R-19 | Single-maintainer dependency | **High** — accepted and disclosed |
| R-20 | False confidence from a silent failure | Medium — core mitigated, surface pending |

**Eleven risks carry a High residual.** That is not a failure of the assessment;
it is what a backup tool honestly looks like. Seven of the eleven are accepted
and disclosed limits that the threat model already states — the passphrase is
the whole scheme, the mirror is not encrypted, the keychain is a real trade-off,
there is no recovery, root wins, Kopia's posture is ours, one person maintains
it. Two (R-14, R-05) are irreducible properties of consuming a public registry
and depending on another project.

**Only two are actually open work:** R-17 and R-18. Both are process risks, both
are on [`CONFORMITY_CHECKLIST.md`](CONFORMITY_CHECKLIST.md), and R-17 is the one
that should be fixed first.

---

## 6. Review

Reviewed on the same triggers as [`THREAT_MODEL.md`](../THREAT_MODEL.md) §9 —
when the crypto design changes, when a new destination kind or credential type
is added, when the IPC surface gains a request returning sensitive data, and at
each minor release — and additionally when a gap on the conformity checklist is
closed, since closing one changes a residual rating in the table above.

| Version | Date | Change |
|---|---|---|
| 1 | 2026-08-31 | Initial assessment for 0.1.0, derived from threat model version 1. |
