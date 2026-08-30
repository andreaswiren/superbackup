# Conformity checklist

Version 1, for superbackup 0.1.x. Last reviewed 2026-08-31.

A working checklist, not a summary. It is meant to be opened, worked through
and edited — by the maintainer before a release, and by a downstream commercial
distributor deciding what they have inherited and what they still have to do.

Status values:

| | |
|---|---|
| **Done** | Implemented and evidenced. The evidence column names the file, test or document |
| **Partial** | Some of it exists. What is missing is on the gap list |
| **Pending** | The mechanism exists in `superbackup-core`; the user-facing surface is specified in `design/` but not yet wired in `crates/app` |
| **Gap** | Not done. On the gap list, with an owner-level action |
| **N/A** | Does not apply, with the reason |
| **Conditional** | Applies only once superbackup is in scope — see [`README.md`](README.md) |

Nothing here is marked **Done** on the strength of a plan.

---

## A. Applicability — do this first

| # | Item | Status | Evidence |
|---|---|---|---|
| A1 | Determine whether the product is a "product with digital elements" with a data connection (Art. 2(1), 3(1)) | Done — yes | [`README.md`](README.md) §1 |
| A2 | Determine whether it is "made available on the market" in the course of a commercial activity (Art. 3(22), Recitals 15, 18) | Done — **no** | [`README.md`](README.md) §2 |
| A3 | Determine whether anyone is a "manufacturer" (Art. 3(13)) | Done — no | [`README.md`](README.md) §3 |
| A4 | Determine whether anyone is an "open-source software steward" (Art. 3(14), Art. 24) | Done — no; natural person, not intended for commercial activities | [`README.md`](README.md) §4 |
| A5 | Classify: default / important Annex III class I or II / critical Annex IV (Art. 6, 7, 8) | Done — arguably Annex III Class I (password managers); documented to that standard | [`README.md`](README.md) §5 |
| A6 | Check the classification against Commission Implementing Regulation (EU) 2025/2392 technical descriptions | Done | [`README.md`](README.md) §5 |
| A7 | Identify what would bring it into scope, and tell whoever needs to know | Done | [`README.md`](README.md) §6 |
| A8 | Document what a downstream commercial distributor inherits | Done | [`README.md`](README.md) §7 |
| A9 | **Re-run A2 whenever the funding, pricing or distribution model changes** | Ongoing | Review trigger in [`README.md`](README.md) |

---

## B. Annex I Part I — essential cybersecurity requirements

Full detail in [`ANNEX_I_PART_I.md`](ANNEX_I_PART_I.md).

| # | Requirement | Status | Evidence |
|---|---|---|---|
| B1 | (1) Appropriate level of cybersecurity based on the risks | Done | `THREAT_MODEL.md`, [`RISK_ASSESSMENT.md`](RISK_ASSESSMENT.md) |
| B2 | (2)(a) No known exploitable vulnerabilities | Done | `cargo deny check` in `ci.yml`; SBOM scan in `sbom.yml` |
| B3 | (2)(b) Secure by default configuration | Done | `Settings::default()`, `crates/core/src/model.rs`; `harden_dir`/`harden_file`; KDF floor |
| B4 | (2)(b) Possibility to reset to the original state | **Gap** | G-04 |
| B5 | (2)(c) Vulnerabilities addressable through security updates — Kopia | Done | `crates/core/src/kopia/install.rs`, `crates/core/tests/kopia_install.rs` |
| B6 | (2)(c) Security updates for superbackup itself; notification; postpone | **Gap** | G-01, G-02 |
| B7 | (2)(d) Protection from unauthorised access | Done | `crates/core/src/ipc/security.rs`; `crates/core/src/crypto/vault.rs`; `there_is_no_request_that_reads_a_secret_back` |
| B8 | (2)(d) Report on possible unauthorised access | Partial / Pending | Logged (`ipc/server.rs` lines 258, 320, 413); no user-visible surface — G-12 |
| B9 | (2)(e) Confidentiality of data at rest, in transit, in memory | Done | Argon2id + XChaCha20-Poly1305; Kopia client-side encryption; rustls-only by policy; `Secret` + `zeroize`; `redact::scrub` |
| B10 | (2)(e) `LocalMirror` is unencrypted | N/A — disclosed, deliberate | `THREAT_MODEL.md` §A3; `design/COPY.md` `dest.mirror.explain`; R-06 |
| B11 | (2)(f) Integrity of data, commands, programs, configuration | Done | `write_atomic`; AAD-authenticated vault header; checksum-verified Kopia install; `guard_containment` |
| B12 | (2)(f) Report on corruptions | Done | `truncated_files_are_errors_not_panics`; `JobRun::derive_status()` |
| B13 | (2)(g) Data minimisation | Done | [`PRIVACY.md`](../PRIVACY.md); no telemetry; random machine UUID; bounded log retention |
| B14 | (2)(h) Availability of essential and basic functions | Done | Retry with backoff; `continue_on_destination_error`; atomic writes; IPC caps |
| B15 | (2)(i) Minimise impact on other devices and networks | Done | Throttling; `skip_on_metered`; `max_parallel_jobs = 1`; `check_interval_hours`; progress coalescing |
| B16 | (2)(j) Limit attack surfaces | Done | No network listener, no plugins, no webview; child environment built from empty; no `GetSecret` |
| B17 | (2)(k) Exploitation mitigation — language and design | Done | Rust; `#![forbid(unsafe_op_in_unsafe_fn)]`; `panic = "abort"`; typed locked/unlocked vault; `subtle`, `zeroize` |
| B18 | (2)(k) Exploitation mitigation — binary hardening flags | **Gap** | G-05 |
| B19 | (2)(k) Signed binaries | **Gap** | G-01 |
| B20 | (2)(l) Record security-relevant internal activity | Done | `events.ndjson`; IPC rejections; install outcomes with SHA-256 |
| B21 | (2)(l) Opt-out mechanism for the user | Partial | `log_level` only — G-13 |
| B22 | (2)(m) Securely and easily remove all data and settings | **Gap** | G-04 |

---

## C. Annex I Part II — vulnerability handling

Full detail in [`ANNEX_I_PART_II.md`](ANNEX_I_PART_II.md).

| # | Requirement | Status | Evidence |
|---|---|---|---|
| C1 | (1) SBOM, machine-readable, at least top-level dependencies | Done — full transitive graph | [`sbom/`](../../../sbom/); [`SBOM.md`](../SBOM.md) |
| C2 | (1) SBOM stays current | Done | `sbom/generate.py --check` gated in `sbom.yml` |
| C3 | (1) SBOM schema-valid | Done | Validated against `bom-1.5.schema.json` and `bom-1.5.xsd` in CI |
| C4 | (1) SBOM covers non-cargo runtime dependencies | Done | `EXTERNAL_COMPONENTS` in `sbom/generate.py` — Kopia and the Linux system libraries |
| C5 | (1) SBOM records the toolchain | Partial — records the resolving `rustc`, cannot bind one to a binary | G-10 |
| C6 | (2) Remediate without delay; documented triage | Done as policy | [`ANNEX_I_PART_II.md`](ANNEX_I_PART_II.md) point (2) |
| C7 | (2) Dependency vulnerability policy, with written exceptions only | Done | `deny.toml` `ignore = []`; policy in `ANNEX_I_PART_II.md` |
| C8 | (2) Kopia advisories watched | **Gap** | G-03 |
| C9 | (2) Security updates separate from functionality updates | Partial — from 1.0 | G-09 |
| C10 | (3) Regular automated security testing | Done | `ci.yml`: tests on 3 platforms, clippy `-D warnings`, `cargo deny`, MSRV, cross-compile |
| C11 | (3) Adversarial tests for each asserted security property | Done | Named list in [`ANNEX_I_PART_II.md`](ANNEX_I_PART_II.md) point (3) |
| C12 | (3) Fuzzing of parsers consuming hostile input | **Gap** | G-06 |
| C13 | (3) Independent security review | **Gap** | G-07 |
| C14 | (3) Code coverage measurement | **Gap** | G-14 |
| C15 | (3) Dependency update automation | **Gap** | G-08 |
| C16 | (4) Publicly disclose fixed vulnerabilities | Done as process, not yet exercised | GitHub Security Advisories; `CHANGELOG.md` |
| C17 | (5) Coordinated vulnerability disclosure policy | Done | [`SECURITY.md`](../../../SECURITY.md) |
| C18 | (6) Contact address for vulnerability reports | Done in the repository | `SECURITY.md`; GitHub private reporting |
| C19 | (6) That contact reaches a user who never visits the repository | **Gap** | G-11 (Annex II item A2) |
| C20 | (6) Facilitate sharing about third-party component vulnerabilities | Done | Published SBOM with per-component target properties |
| C21 | (7) Secure update distribution — Kopia | Done | Verification chain in `crates/core/src/kopia/install.rs` |
| C22 | (7) Secure update distribution — superbackup | **Gap** | G-01 |
| C23 | (8) Security updates free of charge | Done | MIT, no paid tier, no support contract |
| C24 | (8) Advisory messages accompanying updates | Partial | Published advisories yes; in-product notice no — G-02 |

---

## D. Annex II — information to the user

Full detail, with the ten wiring items, in
[`ANNEX_II_USER_INFORMATION.md`](ANNEX_II_USER_INFORMATION.md).

| # | Item | Status | Where |
|---|---|---|---|
| D1 | 1. Manufacturer identity and contact | Partial | `LICENSE`, About screen copyright line — item A1 |
| D2 | 2. Single point of contact and CVD policy | Partial | `SECURITY.md`; **not in the product** — item A2 |
| D3 | 3. Unique product identification | Done | `superbackup version --json`; the SBOM |
| D4 | 4. Intended purpose, security environment, security properties | Done | README; `ARCHITECTURE.md`; `THREAT_MODEL.md` |
| D5 | 5. Known circumstances leading to significant cybersecurity risk | Partial | Documented; not collected in one place — item A3 |
| D6 | 6. Internet address of the EU declaration of conformity | N/A | None exists and none may be issued |
| D7 | 7. Type of security support and support end date | Partial | [`SUPPORT_POLICY.md`](SUPPORT_POLICY.md) only — items A4, A5 |
| D8 | 8(a) Secure commissioning and use | Partial / Pending | README, `design/COPY.md`; onboarding not wired |
| D9 | 8(b) How changes affect the security of data | **Gap** | item A6 |
| D10 | 8(c) How to install security-relevant updates | **Gap** | item A7 — blocked on G-01 |
| D11 | 8(d) Secure decommissioning and data removal | **Gap** | item A8 / G-04 |
| D12 | 8(e) How to turn off automatic security updates | Partial | Documented in `model.rs`; not in the README — item A9 |
| D13 | 8(f) Information for integrators | Done | [`README.md`](README.md) §7 |
| D14 | 9. Where the SBOM can be accessed | Partial | `sbom/README.md`, `SBOM.md`; not in the product or top-level README — item A10 |

---

## E. Technical documentation, conformity assessment and declaration

| # | Item | Status | Evidence |
|---|---|---|---|
| E1 | Annex VII technical documentation, drawn up and indexed | Done | [`ANNEX_VII_TECHNICAL_DOCUMENTATION.md`](ANNEX_VII_TECHNICAL_DOCUMENTATION.md) |
| E2 | Art. 13(2)–(3) cybersecurity risk assessment, documented | Done | [`RISK_ASSESSMENT.md`](RISK_ASSESSMENT.md) |
| E3 | Art. 13(3) mapping of Part I point (2) applicability | Done | [`ANNEX_I_PART_I.md`](ANNEX_I_PART_I.md) |
| E4 | Annex VII(2)(a) architecture description | Done | [`ARCHITECTURE.md`](../../ARCHITECTURE.md) |
| E5 | Annex VII(4) information used to determine the support period | Done | [`SUPPORT_POLICY.md`](SUPPORT_POLICY.md) |
| E6 | Annex VII(5) standards applied, or the solutions adopted instead | Done | [`ANNEX_VII_TECHNICAL_DOCUMENTATION.md`](ANNEX_VII_TECHNICAL_DOCUMENTATION.md) §5 — no harmonised standard exists to apply |
| E7 | Annex VII(6) test reports | Done | The test suite; CI run logs |
| E8 | Annex VII(7) copy of the EU declaration of conformity | N/A | None exists; unsigned template only |
| E9 | Annex VII(8) SBOM available on request | Done — published unconditionally | [`sbom/`](../../../sbom/) |
| E10 | Art. 32 conformity assessment carried out | N/A / Conditional | Not in scope; no assessment performed |
| E11 | Art. 28 / Annex V EU declaration of conformity drawn up | N/A / Conditional | [`ANNEX_V_DECLARATION_OF_CONFORMITY.md`](ANNEX_V_DECLARATION_OF_CONFORMITY.md) — **template only** |
| E12 | Art. 30 CE marking affixed | N/A | **Must not be.** No assessment, no manufacturer |
| E13 | Art. 13(13) retain documentation for 10 years or the support period | Conditional | Git history is the mechanism |
| E14 | Art. 31(2) documentation continuously updated | Ongoing | Review triggers in each document |

---

## F. Article 14 reporting readiness

Full detail in [`VULNERABILITY_REPORTING.md`](VULNERABILITY_REPORTING.md).
Applies from 11 September 2026 to those in scope.

| # | Item | Status |
|---|---|---|
| F1 | Understand what triggers a report, and what does not | Done |
| F2 | Know the 24 h / 72 h / 14 day and 24 h / 72 h / 1 month clocks and their anchors | Done |
| F3 | Runbook written and followable | Done |
| F4 | Record the timestamp of becoming aware, every time | Ongoing discipline |
| F5 | Determine the Member State of main establishment (Art. 14(7)) | **Conditional** — must not be asserted before the project is in scope |
| F6 | Register on the Article 16 single reporting platform | **Conditional** — G-15 |
| F7 | Article 14(8) user notification channel | Done — GHSA, machine-readable |
| F8 | Voluntary reporting position under Article 15 | Done — stated intent to report voluntarily on the Article 14 timetable |

---

## G. Consolidated gap list

Everything not done, in one place, ordered by how much it matters. This is the
part of the package worth reading if you read nothing else.

Each gap names what is missing, why it matters, and the concrete action. Nothing
here is a wish; they are all small enough to be done.

---

### G-01 · No release pipeline: no signed binaries, no checksums, no distribution
**Severity: highest. Blocks G-02, D10, and half of Annex I Part II point (7).**
Risk register: **R-17**.

`ci.yml` builds release binaries and uploads them as 14-day workflow artefacts.
There is no `release.yml`, no published binary, no `SHA256SUMS`, no Authenticode
signature on Windows, no macOS notarisation, and no package-manager presence.
SLSA build level for the binaries is **0**.

For a product whose entire job is holding credentials, an unsigned,
unverifiable binary is the weakest link in the chain, and every other update
obligation depends on fixing it first.

*Action.* A `release.yml` that, on a tag: builds per-platform binaries;
publishes a `SHA256SUMS` file; signs the Windows binary with Authenticode and
notarises the macOS build; runs `actions/attest-build-provenance` on the
binaries; and attaches everything to the release.
[`.github/workflows/sbom.yml`](../../../.github/workflows/sbom.yml) is already
written to bind the SBOM to those binaries with `actions/attest-sbom` the moment
they exist, and currently emits a warning saying so. Then: winget, Homebrew, and
a `.deb`/AUR package.

---

### G-02 · No update mechanism or update notification for superbackup itself
**Severity: high.** Annex I Part I (2)(c), Part II (8). Depends on G-01.

Nothing tells a running installation that a newer, fixed version exists.
`PRIVACY.md` presents the absence of a phone-home update check as a feature, and
for privacy it is; for vulnerability handling it means users run vulnerable
builds indefinitely.

*Action.* An **opt-in** update check against the GitHub releases API, reusing the
host-allowlisted, redirect-refusing HTTP client already written for
`crates/core/src/kopia/install.rs`. Notify, do not install. Offer postpone. Keep
it off by default so the privacy claim stays true for anyone who does not turn
it on, and say clearly in Settings what turning it on sends (nothing but a
request to a public API).

---

### G-03 · No automated watch on Kopia security advisories
**Severity: high.** Annex I Part II (2). Risk register: **R-18**.

Kopia's security posture is superbackup's (`THREAT_MODEL.md` §7). The mechanism
to respond is strong — raise `MINIMUM_KOPIA_VERSION` and the driver refuses an
affected build — but the trigger is a human noticing.

*Action.* Subscribe to Kopia's GitHub security advisories. Add a scheduled CI
job that queries Kopia's advisory feed and latest release and fails when the
configured floor is below a release carrying a security fix.

---

### G-04 · No secure decommissioning, and no reset to original state
**Severity: medium-high.** Annex I Part I (2)(b) and (2)(m); Annex II 8(d).

Nothing removes the configuration, the vault, the state file, the event log and
the Kopia cache. A user decommissioning a machine leaves a sealed vault and an
activity log behind.

*Action.* `superbackup uninstall [--purge]`: stop the daemon, remove the service
and autostart entry, and — with explicit confirmation — delete the config, data
and cache directories, reporting exactly what was removed and what was
deliberately left. **Never touch the destinations**; they are the user's
backups. A README "Decommissioning" section must additionally cover revoking S3
keys and the GitHub token at their source, which no command can do.

The copy must not imply a secure-erase guarantee: on an SSD with wear levelling,
overwriting does not reliably destroy prior contents. The vault's protection is
that it is encrypted under a passphrase that was never stored.

---

### G-05 · No binary exploit-mitigation flags
**Severity: medium.** Annex I Part I (2)(k).

The release profile sets `opt-level`, `lto`, `codegen-units`, `strip` and
`panic = "abort"` and nothing security-specific. No Control Flow Guard on
Windows, no `-z relro,now` on Linux, no macOS hardened runtime. Platform
defaults apply because the toolchain enables them, not because the project asked.

*Action.* Set target-specific `rustflags` in `.cargo/config.toml`, and verify
with `winchecksec` on Windows and `checksec` on Linux in CI so a regression is
caught. Note that `Cargo.toml` and `.cargo/config.toml` are outside this
package's ownership and this needs the maintainer.

---

### G-06 · No fuzzing
**Severity: medium.** Annex I Part II (3).

The parsers that consume hostile input are covered by hand-written adversarial
tests, which are good and are not a fuzzer.

*Action.* `cargo-fuzz` targets for `crypto::envelope` (vault parsing),
`kopia::progress` and `kopia::manifest` (untrusted subprocess output), and
`ipc::codec` (untrusted local input). Run them in a scheduled CI job rather than
per-commit, and commit the corpus.

---

### G-07 · No independent security review
**Severity: medium, and irreducible without funding.** Annex I Part II (3).

Nobody outside the project has audited the cryptographic design or the IPC
boundary. For software holding repository encryption keys this is material, and
users should weigh it.

*Action.* Short of a paid audit: publish the threat model and this package
prominently and invite review; apply to a foundation-funded audit programme
(OSTIF, Sovereign Tech Fund, OpenSSF) if the project ever has the standing;
until then, state the limitation rather than let a reader assume otherwise.

---

### G-08 · No dependency update automation
**Severity: medium.** Annex I Part II (3).

Dependency currency depends on the maintainer running `cargo update`. `cargo
deny` catches an advisory, which is the emergency case, not the routine one.

*Action.* Enable Dependabot or Renovate for cargo and for GitHub Actions,
grouped weekly so it does not generate noise.

---

### G-09 · Security updates are not separable from feature updates
**Severity: medium, and time-limited.** Annex I Part II (2).

During 0.x only the latest release is supported, so a security fix ships in
whatever release comes next.

*Action.* From 1.0: patch releases cut from the release branch containing the
fix and nothing else. Stated as a commitment in
[`SUPPORT_POLICY.md`](SUPPORT_POLICY.md) so it can be held to.

---

### G-10 · The SBOM cannot bind a toolchain to a released binary
**Severity: low-medium.** Annex I Part II (1). Depends on G-01.

`metadata.tools` records the `rustc` version and host triple that *resolved* the
SBOM. That is not necessarily the toolchain that compiled a given release
binary, and the SBOM says so rather than overclaiming. A consumer asking "was
this binary built with a compiler carrying a known bug" therefore still cannot
get an answer from the SBOM alone.

*Action.* Closing this needs a release pipeline (G-01) that pins its toolchain
and emits the SBOM from the same job that produces the binaries, so that the
recorded `rustc` is the one that did the compiling.

---

### G-11 · The security contact does not reach a user who never visits the repository
**Severity: medium.** Annex I Part II (6); Annex II item 2.

`SECURITY.md` is the policy and GitHub surfaces it, but the product itself
offers no route to report a vulnerability. Annex II point 2 is one of the few
obligations that must reach the *user*, not the repository visitor.

*Action.* A "Report a security issue" link in the About screen, visually
distinct from the existing public "Report an issue" button, pointing at GitHub
private vulnerability reporting. See
[`ANNEX_II_USER_INFORMATION.md`](ANNEX_II_USER_INFORMATION.md) item A2.

---

### G-12 · Security-relevant events are recorded but never surfaced
**Severity: medium.** Annex I Part I (2)(d) and (2)(l). Risk register: **R-20**.

Rejected IPC peers, refused connections and failed unlocks are logged. Nothing
shows them to the user. The tray `Health` state and the activity view are
specified in `design/UX_SPEC.md` and not wired — `crates/app/src/main.rs` still
carries `TODO(integration)` for tray, GUI and CLI dispatch.

*Action.* Wire the tray and the activity view. Until then, several rows in this
checklist read **Pending** rather than **Done**, which is the honest state of a
0.1.0 pre-release.

---

### G-13 · The activity-logging opt-out is a log level, not a setting
**Severity: low.** Annex I Part I (2)(l).

`log_level` can be reduced to `Error`, which substantially reduces recording.
There is no explicit setting that says "do not record activity", and `state.json`
is not optional — a backup tool that does not record whether a backup happened
is not doing its job.

*Action.* Make the trade-off explicit in Settings: state what is recorded, why
run history cannot be turned off, and what reducing the log level does. This is
a copy change, not an architecture change.

---

### G-14 · No code coverage measurement
**Severity: low.** Annex I Part II (3).

"Effective testing" is asserted from the shape of the tests rather than measured.

*Action.* `cargo-llvm-cov` in CI, reported not gated. A coverage threshold that
people game is worse than no threshold.

---

### G-15 · Article 16 reporting platform registration
**Severity: conditional, but pre-work is cheap.** Article 14(1), 14(7).

Registering on ENISA's single reporting platform during a live incident wastes
hours of a 24-hour budget.

*Action.* Only once the project is in scope: determine the Member State of main
establishment, identify its CSIRT designated as coordinator under Article 12(1)
of Directive (EU) 2022/2555, register, and record both in
[`VULNERABILITY_REPORTING.md`](VULNERABILITY_REPORTING.md). **Do not assert a
Member State or a CSIRT before then** — it would be a legal position the project
does not hold.

---

### G-16 · Licences in the dependency tree that `deny.toml` does not allow
**Severity: medium. Not a CRA gap — a licence-compliance one found while
building the SBOM, and it should be resolved before any release.**

The generated SBOM records four components whose licence expression contains no
identifier from `deny.toml`'s `allow` list, and `exceptions = []`:

| Licence | Crate | In which targets |
|---|---|---|
| `CC0-1.0` | `notify` 8.2.0 | all — a **direct** dependency of `superbackup-core` |
| `CC0-1.0` | `hexf-parse` 0.2.1 | all |
| `0BSD` | `doctest-file` 1.1.1 | all |
| `0BSD` | `recvmsg` 1.0.0 | `x86_64-pc-windows-msvc` |

[`THIRD_PARTY.md`](../THIRD_PARTY.md) lists `notify` as "CC0-1.0 /
Artistic-2.0" in its notable-dependencies table, but neither identifier appears
in the allowlist that the same document reproduces. `deny.toml`'s comment is
explicit that there is no exceptions list because a licence question is "a
release blocker, not review it later".

Both `CC0-1.0` and `0BSD` are permissive and compatible with redistributing an
MIT binary — this is a policy inconsistency, not a licensing violation. But it
means either `cargo deny check licenses` does not currently pass on this tree,
or it passes for a reason nobody has written down, and both are worth knowing
before a release.

*Action.* Verify by running `cargo deny check licenses`. Then either add
`CC0-1.0`, `0BSD` and `Artistic-2.0` to the `allow` list with a one-line reason
each, or record explicit exceptions. Update
[`THIRD_PARTY.md`](../THIRD_PARTY.md)'s allowlist to match whichever is chosen.
`deny.toml` and `THIRD_PARTY.md` are outside this package's ownership, so this
needs the maintainer.

Two smaller inconsistencies in the same file, worth a look while it is open:
`deny.toml` allows the `OpenSSL` licence identifier while `[bans]` denies the
`openssl` and `openssl-sys` crates outright; and `THIRD_PARTY.md`'s reproduction
of the allowlist omits `OpenSSL`, which `deny.toml` includes.

---

### G-17 · Supply-chain posture is not measured
**Severity: low.** Good practice rather than a CRA requirement.

*Action.* Run OpenSSF Scorecard in CI and publish the badge; consider
`cargo-vet` or `cargo-crev` review records for the cryptographic dependencies
specifically, which is where a review has the most value per hour spent.

---

### G-18 · *(closed 2026-08-31)* Threat model coverage of the managed Kopia download

Raised while this package was being written: `crates/core/src/model.rs` pointed
the reader at `THREAT_MODEL.md` **§A8** for the supply-chain reasoning behind
the managed Kopia download, and the threat model had adversaries A1 through A7
only.

**Closed.** §A8 "A malicious `kopia` binary" has since landed, promoting the
Kopia binary from an operational caveat to an in-scope adversary and stating the
residual — the checksum is verified, `checksums.txt.sig` is not — and §3 now
excludes *"a compromise of the Kopia project or of GitHub itself"* rather than
"a malicious kopia binary". R-13 in
[`RISK_ASSESSMENT.md`](RISK_ASSESSMENT.md) is written to agree with it.

Kept in the list rather than deleted, because a gap list that only ever grows is
easier to trust than one that quietly rewrites itself.

---

## H. For a downstream commercial distributor

If you are bundling superbackup into something you sell, none of the **Done**
rows above transfer to you. Work through
[`README.md`](README.md) §7, then this list:

| # | You must | Reference |
|---|---|---|
| H1 | Do your own cybersecurity risk assessment for your product | Art. 13(2)–(4) |
| H2 | Exercise due diligence on integrated third-party components, including this one | Art. 13(5) |
| H3 | Determine and publish your own support period, at least five years | Art. 13(8), 13(19) |
| H4 | Draw up technical documentation to Annex VII and keep it 10 years or the support period | Art. 31, 13(13) |
| H5 | Carry out a conformity assessment under Article 32 — and note that **Article 32(5)'s open-source route is unavailable to you** unless your product is itself FOSS with public technical documentation | Art. 32 |
| H6 | Draw up the EU declaration of conformity and affix CE marking | Art. 28, 30, Annex V |
| H7 | Supply Annex II information under your own identity and contact point | Annex II |
| H8 | Report actively exploited vulnerabilities and severe incidents in your product | Art. 14 |
| H9 | Report vulnerabilities you find in superbackup to this project, and share any fix | Art. 13(6), [`SECURITY.md`](../../../SECURITY.md) |
| H10 | Meet the Apache-2.0 obligations if you bundle a Kopia binary | [`THIRD_PARTY.md`](../THIRD_PARTY.md) |

---

## Review

Reviewed at each minor release, whenever a gap is closed, and whenever the
applicability analysis in [`README.md`](README.md) could change. Closing a gap
here usually changes a residual rating in
[`RISK_ASSESSMENT.md`](RISK_ASSESSMENT.md), so update both.

| Version | Date | Change |
|---|---|---|
| 1 | 2026-08-31 | Initial checklist for 0.1.0. 18 gaps identified, one (G-18) closed the same day. |
