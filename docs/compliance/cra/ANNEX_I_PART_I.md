# Annex I, Part I — Essential cybersecurity requirements

Regulation (EU) 2024/2847, Annex I, Part I. Version 1, for superbackup 0.1.x.
Last reviewed 2026-08-31.

Each requirement below is quoted, then answered with the specific control that
meets it and a pointer to the code, test or document that proves it. Where a
requirement is not met, it says so and states the plan. Nothing here is
asserted that is not in the repository.

Read this alongside [`RISK_ASSESSMENT.md`](RISK_ASSESSMENT.md), which is the
Article 13(2)–(3) assessment these controls answer, and
[`THREAT_MODEL.md`](../THREAT_MODEL.md), which is where the security design is
argued rather than tabulated. Where this document and the threat model appear
to disagree, the threat model is right.

## Status legend

| | Meaning |
|---|---|
| **Met** | Implemented in the repository, with evidence named. |
| **Core met, surface pending** | The mechanism exists and is tested in `superbackup-core`; the user-facing surface that exposes it is specified in `design/` but not yet wired in `crates/app` (`main.rs` still carries `TODO(integration)` for tray, GUI and CLI dispatch). |
| **Partial** | Some of the requirement is met and some is not. Both halves are stated. |
| **Gap** | Not met. The plan is stated and the item appears in [`CONFORMITY_CHECKLIST.md`](CONFORMITY_CHECKLIST.md). |

superbackup 0.1.0 has not been placed on the market and the CRA does not
currently apply to it (see [`README.md`](README.md)). "Core met, surface
pending" items are therefore not overdue obligations; they are work that has to
be finished before a release could honestly claim conformity.

---

## Point (1) — Appropriate level of cybersecurity based on the risks

> *"Products with digital elements shall be designed, developed and produced in
> such a way that they ensure an appropriate level of cybersecurity based on the
> risks."*

**Status: Met.**

A backup tool reads everything its user owns and holds the keys that make it
recoverable. The risk profile is stated in [`THREAT_MODEL.md`](../THREAT_MODEL.md)
§1–§3 and assessed in [`RISK_ASSESSMENT.md`](RISK_ASSESSMENT.md), which
identifies eight in-scope adversaries and states, for each, the defence and the
residual risk. Article 13(3) requires the assessment to say *whether* and *how*
each Part I point (2) requirement applies; that mapping is this document.

The design decisions that follow from the risk profile, in order of weight:

- Every secret lives in an authenticated, passphrase-sealed vault rather than in
  the configuration file (`crates/core/src/crypto/vault.rs`,
  `crates/core/src/model.rs` — the model contains `SecretRef` handles and no
  secret material of any kind).
- No secret ever reaches a child process's `argv`, because `argv` is readable by
  other local users on every supported platform. Enforced mechanically, not by
  convention (`KopiaCommand::audit_argv`, `crates/core/src/kopia/command.rs`).
- The IPC endpoint is treated as a privilege boundary, not a convenience
  (`crates/core/src/ipc/security.rs`).
- Nothing cryptographic is hand-rolled (`THREAT_MODEL.md` §4, design rule 5).
- The dependency tree is policed rather than observed (`deny.toml`, enforced in
  `.github/workflows/ci.yml`).

---

## Point (2)(a) — No known exploitable vulnerabilities

> *"be made available on the market without known exploitable vulnerabilities"*

**Status: Met.**

Two independent gates, both blocking:

- `cargo deny check` runs on every push and pull request
  (`.github/workflows/ci.yml`, job `audit`). Its policy (`deny.toml`) sets
  `[advisories] version = 2`, `yanked = "deny"` and an empty `ignore` list, so a
  RustSec advisory or a yanked crate fails the build rather than producing a
  warning somebody reads later.
- The SBOM is scanned on every push to `main` and on release
  (`.github/workflows/sbom.yml`, job `scan`), failing at severity `high`. This
  is deliberately a second opinion: `cargo-deny` reads RustSec from
  `Cargo.lock`, the scanner reads the SBOM. Disagreement between them means the
  SBOM is wrong, which is worth finding out.

`RUSTFLAGS: -D warnings` and `cargo clippy --workspace --all-targets
--all-features` mean a lint that catches a defect class fails the build too.

**Known limit, stated rather than implied.** No such gate covers Kopia. Kopia
is a separate executable with its own release cadence and its own advisory
stream. superbackup manages that dependency by refusing to run a version below
a documented floor (`MINIMUM_KOPIA_VERSION`, currently 0.17.0, in
`crates/core/src/kopia/binary.rs`), by refusing a downgrade
(`a_downgrade_is_refused` in `crates/core/tests/kopia_install.rs`), and by
reporting the resolved binary in `doctor`. Raising the floor in response to a
Kopia advisory is a maintainer action, not an automated one. See
[`ANNEX_I_PART_II.md`](ANNEX_I_PART_II.md) for the policy.

---

## Point (2)(b) — Secure by default configuration

> *"be made available on the market with a secure by default configuration …
> including the possibility to reset the product to its original state"*

**Status: Partial.**

**Secure defaults — met.** `Settings::default()` in `crates/core/src/model.rs`:

| Default | Value | Why it is the secure choice |
|---|---|---|
| `use_os_keychain` | `false` | The master key stays in memory only. Caching it in the platform credential store is the convenient option and the weaker one, so it is opt-in, and the interface states the trade-off at the point of choice (`design/COPY.md`, `onboarding.service.keychain_warn`). |
| `auto_lock_minutes` | `30` | The key is dropped after inactivity. A locked vault blocks scheduled runs and the tray shows `Attention`, so it is never silent (`THREAT_MODEL.md` §5). |
| `kopia.auto_update` | `UpdatePolicy::Notify` | Updates are surfaced, not applied behind the user's back. See (2)(c). |
| `kopia.allow_prerelease` | `false` | Release builds only. |
| `kopia.prefer_system_binary` | `true` | A kopia the user installed deliberately wins over one superbackup fetched. |
| `skip_on_metered` | `true` | Does not spend someone's mobile data without being asked. |
| `max_parallel_jobs` | `1` | Two processes driving one Kopia repository is a corruption risk (`ARCHITECTURE.md`). |
| `log_retention_days` | `30` | Bounded retention rather than unbounded accumulation. See (2)(g). |

Filesystem defaults: config and data directories are created `0700` and files
`0600` on Unix (`crates/core/src/paths.rs`, `harden_dir` / `harden_file`). The
IPC endpoint defaults to owner-only with no fallback to a permissive one — if
the platform will not give the protection asked for, binding fails
(`crates/core/src/ipc/security.rs`).

Vault defaults: Argon2id at m=256 MiB, t=3, p=1
(`DEFAULT_MEMORY_KIB`/`DEFAULT_ITERATIONS`/`DEFAULT_PARALLELISM` in
`crates/core/src/crypto/kdf.rs`), with a floor of m=64 MiB, t=3 that a new
vault cannot be created below — `validate_for_new_vault`, asserted by
`weak_test_params_cannot_create_a_real_vault`.

**Reset to original state — gap.** There is no command that returns an
installation to its as-installed state. `superbackup service uninstall` removes
the service and `superbackup autostart` manages the login entry, but nothing
removes the configuration, the vault, the state file, the event log and the
Kopia cache. See (2)(m), where the same gap has a second consequence.

---

## Point (2)(c) — Vulnerabilities addressable through security updates

> *"ensure that vulnerabilities can be addressed through security updates,
> including, where applicable, through automatic security updates that are
> installed within an appropriate timeframe enabled as a default setting, with a
> clear and easy-to-use opt-out mechanism, through the notification of available
> updates to users, and the option to temporarily postpone them"*

**Status: Partial. This is the largest gap in Part I.**

**For the Kopia dependency — met, and met well.** `KopiaManagement`
(`crates/core/src/model.rs`) and the installer
(`crates/core/src/kopia/install.rs`) implement exactly the shape this point
describes: update checks on an interval (`check_interval_hours`, default 24), a
policy the user controls (`UpdatePolicy`, default `Notify` — the user is told,
and chooses), a version pin for reproducible deployments (`pinned_version`), a
minimum version below which the driver refuses to run, and a deferral when a
job is running (`an_automatic_update_defers_while_a_job_is_running` in
`crates/core/tests/kopia_install.rs`). Secure delivery of that update is
covered under (2)(f) and in [`ANNEX_I_PART_II.md`](ANNEX_I_PART_II.md).

**For superbackup itself — gap.** There is no self-update mechanism, no update
check, and no in-product notification of a new superbackup release. Nothing in
`crates/app` or `crates/core` looks for one, and `PRIVACY.md` states as a
feature that there is "no update check that phones home". A user learns about a
new version from GitHub or from their package manager.

That is a defensible position for a tool with no telemetry, and it is the
honest one, but it does not satisfy this point on its own terms. The plan, in
[`CONFORMITY_CHECKLIST.md`](CONFORMITY_CHECKLIST.md):

1. Publish releases through channels that carry their own update mechanism
   (winget, Homebrew, distribution packages), so "how do I get security
   updates" has an answer that does not depend on superbackup implementing one.
2. Add an opt-in update check against the GitHub releases API, reusing the
   host-allowlisted, redirect-refusing HTTP client already written for the Kopia
   installer, notifying rather than installing, with the same postpone
   behaviour.
3. Document the answer in the About screen and the README — see
   [`ANNEX_II_USER_INFORMATION.md`](ANNEX_II_USER_INFORMATION.md).

The Regulation's "enabled as a default setting" wording is qualified by "where
applicable". A single-binary desktop tool distributed through package managers
has a reasonable argument that automatic self-update is not applicable. The
project does not lean on that argument to avoid doing item 1 and item 2.

---

## Point (2)(d) — Protection from unauthorised access

> *"ensure protection from unauthorised access by appropriate control
> mechanisms, including but not limited to authentication, identity or access
> management systems, and report on possible unauthorised access"*

**Status: Met.**

There are three access paths into superbackup, and each has a control.

**1. The vault.** Opening it requires the master passphrase. The key is derived
by Argon2id with parameters recorded in the file header, and the header is
authenticated as associated data, so an attacker cannot substitute weaker KDF
parameters and re-present the file — tampering makes it fail to open rather
than open cheaply. Proved by `a_tampered_header_breaks_decryption_rather_than_
being_accepted` in `crates/core/tests/vault_format.rs`, which weakens
`memory_kib` and `iterations`, corrupts the salt, and rewrites `vault_id` and
`updated_at`, and requires each to be rejected. A wrong passphrase and a
corrupt file are the same class of failure
(`a_wrong_passphrase_is_rejected`, `a_bit_flipped_ciphertext_is_rejected`,
`a_bit_flipped_authentication_tag_is_rejected`).

Passphrase strength is measured and the verdict shown live
(`secret::estimate_strength`, `crates/core/src/secret.rs`), a list of common
passphrases is refused an acceptable rating, and `Strength::is_acceptable`
requires at least `Fair` before a master passphrase is taken without an explicit
acknowledgement. It guides; it does not block a user who insists. That is stated
in the code and in `THREAT_MODEL.md` §A1 rather than dressed up.

**2. The IPC endpoint.** A privilege boundary, treated as one
(`crates/core/src/ipc/security.rs`):

- Windows: an explicit DACL granting `FILE_ALL_ACCESS` to the daemon's own
  account, `NT AUTHORITY\SYSTEM` and `BUILTIN\Administrators`. No "everyone"
  ACE, no inherited ACE, no null DACL. Remote clients are refused —
  `interprocess` sets `PIPE_REJECT_REMOTE_CLIENTS`, so the endpoint is
  unreachable over SMB. If the descriptor cannot be built, **binding fails**
  rather than falling back to a default-permissioned pipe.
- Unix: socket mode `0600` applied by `fchmod` before `bind` (closing the umask
  race), inside a `0700` directory, plus a second `SO_PEERCRED` effective-uid
  check on every accepted connection.

Above that: the connection count is capped, requests are rate-limited per
connection with a token bucket, and a line-length cap means a client cannot
exhaust memory before the protocol sees it. Tested in
`crates/core/tests/ipc_server.rs` —
`requests_are_rate_limited_per_connection`, `the_connection_limit_refuses_
politely`, `an_oversized_line_is_refused_without_buffering_it`,
`a_slow_subscriber_is_told_it_lagged_instead_of_stalling_the_daemon`.

**3. The shared configuration repository.** A pulled vault is an untrusted
encrypted blob until proven otherwise. It is never written over the local vault
until it has decrypted under a passphrase supplied in this session, and where
`trusted_signers` is populated its detached Ed25519 signature must verify
against a pinned key or the pull is rejected — including in a build that cannot
verify, which fails closed. Tested in `crates/core/tests/config_remote_sync.rs`:
`a_pull_with_the_wrong_passphrase_never_reaches_the_disk`,
`a_tampered_remote_vault_is_rejected_before_anything_is_written`,
`garbage_served_instead_of_a_vault_is_rejected`,
`pinning_a_signer_fails_closed_in_a_build_that_cannot_verify`.

**"Report on possible unauthorised access" — met at the log level, surface
pending.** A rejected peer, a refused connection and an accept failure are all
recorded (`tracing::warn!` at `crates/core/src/ipc/server.rs` lines 258, 320 and
413). A failed vault unlock is a typed error that reaches the event stream.
What does not yet exist is a user-visible security event view; the tray's
`Attention` state and the activity log are specified in `design/UX_SPEC.md` but
not wired. Recorded as **core met, surface pending** in the checklist.

---

## Point (2)(e) — Confidentiality of data

> *"protect the confidentiality of stored, transmitted or otherwise processed
> data, personal or other, such as by encrypting relevant data at rest or in
> transit by state of the art mechanisms, and by using other technical means"*

**Status: Met, with one documented and deliberate exception.**

**At rest — secrets.** XChaCha20-Poly1305 over a key derived by Argon2id, with
purpose-separated subkeys from HKDF-SHA256 so the master key is never used
directly for two jobs (`crates/core/src/crypto/`, summarised in
`THREAT_MODEL.md` §4). A 192-bit nonce means random nonces are safe without a
counter; `every_seal_uses_a_fresh_nonce` asserts they are fresh anyway.
`the_sealed_file_contains_no_plaintext_secret_and_no_plaintext_config` asserts
the file leaks neither.

**At rest — backup contents.** Kopia encrypts client-side before anything
leaves the machine (AES-256-GCM-HMAC-SHA256 by default). The destination holds
ciphertext. This is Kopia's guarantee, not superbackup's, and
`THREAT_MODEL.md` §7 says so explicitly rather than claiming it.

**In transit.** `rustls` is required by policy: `reqwest` is configured
`default-features = false` with the `rustls` feature
(`crates/core/Cargo.toml`), and `deny.toml` bans `openssl`, `openssl-sys` and
`native-tls` outright, so the TLS stack is identical on every platform and no C
TLS library enters a security-sensitive binary. The Kopia installer additionally
restricts every request and every redirect to a GitHub host allowlist, refusing
rather than following a redirect that leaves it
(`DEFAULT_ALLOWED_HOSTS`, `crates/core/src/kopia/install.rs`; tested by
`a_download_redirected_off_github_is_refused` and
`the_default_allowlist_refuses_a_non_github_endpoint`).

**In memory.** Secrets exist only inside `Secret`
(`crates/core/src/secret.rs`), which has no `Display`, no `Serialize`, and a
`Debug` that prints a redaction marker; the only way to see the bytes is the
deliberately verbose `expose()`. The buffer is zeroed on drop.
`a_passphrase_never_appears_in_debug_output` asserts it.

**Against third-party output.** Everything that can reach a log, an event, an
IPC response or a notification passes through `redact::scrub`
(`crates/core/src/redact.rs`) first — deliberately over-eager, because a
redacted diagnostic is a nuisance and a leaked repository key is unrecoverable.
`outbound_frames_are_scrubbed` in `crates/core/tests/ipc_protocol.rs` asserts
the IPC path.

**The exception.** `DestinationKind::LocalMirror` is a plain, unencrypted file
copy. That is its entire purpose. It is documented in `THREAT_MODEL.md` §A3, in
`PRIVACY.md`, and stated in the interface at the point of choice
(`design/COPY.md`, `dest.mirror.explain`: "…no deduplication and no encryption
— anyone who can read the folder can read your files"). Under Annex I this is a
disclosed limitation of a user-selected mode, not a failure to protect: the
requirement is qualified by "relevant data", the user selects the mode
knowingly, and the alternative is available in the same dialog. It is recorded
in [`RISK_ASSESSMENT.md`](RISK_ASSESSMENT.md) as an accepted residual risk.

Also unencrypted, by design and disclosed in `PRIVACY.md`: the `_superbackup/`
manifest directory at each destination, and object sizes and write timing.

**Not claimed.** Memory hygiene is best-effort. An OS that pages, hibernates or
writes a crash dump can persist plaintext. `THREAT_MODEL.md` §4 rule 2 says so;
so does `PRIVACY.md`. Neither this document nor the product claims otherwise.

---

## Point (2)(f) — Integrity of data, commands, programs and configuration

> *"protect the integrity of stored, transmitted or otherwise processed data,
> personal or other, commands, programs and configuration against any
> manipulation or modification not authorised by the user, and report on
> corruptions"*

**Status: Met.**

| Asset | Integrity control | Evidence |
|---|---|---|
| The vault | AEAD over the whole body with the header authenticated as associated data | `crates/core/src/crypto/vault.rs`; `a_tampered_header_breaks_decryption_rather_than_being_accepted` |
| Configuration on disk | Atomic write: temp file → `write_all` → `flush` → `sync_all` → `harden_file` → rename → directory `fsync` on Unix. A crash leaves the old file or the new one, never a mixture | `paths::write_atomic`, `crates/core/src/paths.rs`; `saving_an_invalid_config_writes_nothing` |
| Configuration content | Validated before it is persisted, and refused rather than silently repaired | `set_config_validates_before_it_persists`, `a_config_that_is_present_but_broken_is_never_silently_replaced`, `a_document_from_the_future_is_refused_and_left_alone` |
| Pulled shared configuration | Decrypt-then-verify, optional pinned Ed25519 signature, local vault backed up before replacement, never silent | `crates/core/src/remote.rs`; the four tests named under (2)(d) |
| The Kopia binary | SHA-256 over the bytes actually received, compared against the `checksums.txt` published with the same release, before anything is moved into place; archive extraction is path-traversal guarded; the installed binary must report the version the release promised or it is not kept | `crates/core/src/kopia/install.rs`; `a_checksum_mismatch_refuses_to_install_and_leaves_nothing_behind`, `a_release_without_checksums_is_refused`, `a_zip_slip_archive_is_rejected`, `a_binary_that_lies_about_its_version_is_not_installed` |
| Backup contents at the destination | Kopia's own content hashing and authenticated encryption | `THREAT_MODEL.md` §7 |
| Commands to the daemon | Length-capped, schema-checked, protocol-versioned, from an owner-only endpoint | `crates/core/src/ipc/`; `a_protocol_mismatch_is_refused_per_request_with_a_clear_message` |
| The mirror destination | Every write and delete re-checked against the canonical target root after canonicalisation, so a crafted name or a symlink cannot escape; refuses a destination nested inside its own source | `crates/core/src/engine/mirror.rs`, `guard_containment` |

**Reporting corruptions — met.** A corrupt vault, a truncated file, a
structurally broken document and a hostile header are all typed errors, never
panics, and all reach the user
(`truncated_files_are_errors_not_panics`, `structurally_broken_files_are_
errors_not_panics`, `a_hostile_header_cannot_make_us_allocate_gigabytes`). A
run whose destinations partly failed resolves to `SucceededWithWarnings` and
never to `Succeeded` — `JobRun::derive_status()` is deliberately the only place
that decides, because a backup tool that says "succeeded" when a destination was
skipped is worse than one that says nothing (`ARCHITECTURE.md`).

**Known limit.** Kopia publishes `checksums.txt.sig` alongside its checksum
file, but the signing key is not published in a form the installer can pin, so
the signature is **not** verified. The checksum proves the bytes were not
altered in transit or at a CDN edge; its authenticity rests on TLS to
`github.com` and on GitHub. An attacker able to publish a release to
`kopia/kopia` defeats it — exactly as they would defeat a user running
`curl | tar x`. This is stated in the installer's module documentation, carried
in `InstallOutcome::signature_verified` (always `false`), argued in
[`THREAT_MODEL.md`](../THREAT_MODEL.md) §A8, and recorded as an accepted
residual risk (R-13) in [`RISK_ASSESSMENT.md`](RISK_ASSESSMENT.md). §3 of the
threat model places a compromise of the Kopia project or of GitHub itself out
of scope, and this document does not claim otherwise.

---

## Point (2)(g) — Data minimisation

> *"process only data, personal or other, that are adequate, relevant and
> limited to what is necessary in relation to the intended purpose of the
> product with digital elements (data minimisation)"*

**Status: Met.**

The full analysis is [`PRIVACY.md`](../PRIVACY.md). The load-bearing facts:

- **Nothing is collected.** No telemetry, no analytics, no crash reporting, no
  account, and no update check that phones home. There is no server to send
  anything to. `cargo deny` would flag an analytics dependency arriving
  transitively, because `[sources]` restricts crates to crates.io and the
  licence allowlist has no exceptions.
- **Three outbound connections exist**, all user-configured: the S3 endpoint
  (made by Kopia), the Git host for shared configuration, and GitHub for the
  Kopia binary. `PRIVACY.md` states "there is no fourth", and the claim is
  checkable — `reqwest` appears only in `remote.rs` and `kopia/install.rs`.
- **The machine identifier is a random UUID**, deliberately not derived from any
  hardware serial, MAC address or disk ID, so it cannot be correlated with
  anything outside this application. Delete the config directory and it is gone.
- **Logs record counts and sizes**, and file paths only when a specific file
  fails. Everything written to a log passes through credential redaction first.
- **Retention is bounded**: `log_retention_days` defaults to 30 and the event
  log is rotated against it.
- **The configuration contains no secret material of any kind** — only
  `SecretRef` handles (`crates/core/src/model.rs`). That is what makes
  `config.json` safe to read, diff and share, and it is what
  `the_sealed_file_contains_no_plaintext_secret_and_no_plaintext_config` and
  `a_pull_plan_never_renders_secret_material` protect.

The product does process a large volume of the user's own data — that is what a
backup tool is for. Article 3's data minimisation limb is about what the product
processes *in relation to its intended purpose*; reading the folders the user
nominated is the purpose.

---

## Point (2)(h) — Availability of essential and basic functions

> *"protect the availability of essential and basic functions, also after an
> incident, including through resilience and mitigation measures against
> denial-of-service attacks"*

**Status: Met.**

The essential function is: scheduled backups run, and their outcome is reported
truthfully.

**Resilience within a run.** A destination failing does not take the others
down — `Job::continue_on_destination_error` defaults to carrying on, because a
broken offsite link is not a reason to skip the local copy
(`a_failing_destination_does_not_abort_the_others`). Transient failures are
retried with bounded exponential backoff; deterministic ones are not retried at
all (`transient_failures_are_retried_with_backoff`,
`deterministic_failures_are_not_retried`, `retry_is_bounded`). A run that
exceeds its timeout is stopped and recorded as a failure, not left hanging
(`the_timeout_stops_the_run_and_reports_it_as_a_failure`).

**Resilience across a crash or power loss.** Configuration, vault and state are
written atomically, so an interrupted write leaves the old file or the new one.
`run_missed_on_start` defaults to `true`, so a schedule that elapsed while the
machine was asleep or off is caught up rather than silently skipped.

**Resistance to local denial of service.** The IPC endpoint caps concurrent
connections, rate-limits requests per connection, caps line length before
buffering, and drops oldest with a "you missed N events" marker for a slow
subscriber rather than buffering without bound — an unresponsive client must not
be able to stall a backup. A panicking handler costs one request, not the
daemon (`a_panicking_handler_costs_one_request_not_the_daemon`); a hanging one
is abandoned rather than owning the connection
(`a_hanging_handler_is_abandoned_rather_than_owning_the_connection`).

**Resistance to resource exhaustion from hostile input.** A hostile vault header
cannot make the process allocate gigabytes
(`a_hostile_header_cannot_make_us_allocate_gigabytes`); an oversized config is
refused before parsing (`an_oversized_config_is_refused_before_parsing`);
captured Kopia stdout is capped at 64 MiB and stderr at a bounded tail; collected
warnings are capped so a source tree with a million unreadable files cannot turn
the run history into a memory leak (`crates/core/src/kopia/command.rs`).

**Not claimed.** superbackup does not defend against a network-level
denial-of-service attack on the user's storage provider, because it has no
position from which to. A destination being unreachable is handled as a failure
with retry, and is visible.

---

## Point (2)(i) — Minimising impact on the availability of other services

> *"minimise the negative impact by the products themselves or connected devices
> on the availability of services provided by other devices or networks"*

**Status: Met.**

- **Bandwidth throttling**, globally and on a schedule, upload and download
  separately, passed through to Kopia as `--upload-bytes-per-second` /
  `--download-bytes-per-second` (`crates/core/src/engine/throttle.rs`,
  `crates/core/src/kopia/driver.rs`).
- **`skip_on_metered` defaults to `true`** and `skip_on_battery` is available,
  so a scheduled run does not consume someone's tethered connection uninvited.
- **`max_parallel_jobs` defaults to 1.**
- **`check_interval_hours` defaults to 24** specifically so that a machine which
  restarts often cannot hammer GitHub's per-IP-rate-limited API
  (`crates/core/src/model.rs`), and an update check that is too soon is skipped
  (`an_update_check_is_skipped_when_it_is_too_soon`).
- **Progress events are coalesced** before they reach subscribers: a snapshot of
  a `node_modules` tree can generate tens of thousands of events per second, and
  neither a 60 fps interface nor a socket benefits from all of them. Coalescing
  never drops the final state of a run (`ARCHITECTURE.md`).
- **Kopia's pipes are drained continuously**, because a blocked pipe deadlocks
  the child — which in a backup tool presents as "it hangs at 40% on large
  folders".

superbackup makes no unsolicited network connections, listens on no network
port, and has no discovery or peer-to-peer behaviour. It cannot be enrolled into
an attack on a third party by design rather than by configuration.

---

## Point (2)(j) — Limiting attack surfaces

> *"be designed, developed and produced to limit attack surfaces, including
> external interfaces"*

**Status: Met.**

Attack surface, enumerated exhaustively:

| Interface | Exposure | Control |
|---|---|---|
| IPC endpoint | Local only | Owner-restricted, remote clients refused, capped, rate-limited. **No request returns a plaintext secret**: the protocol offers `SetSecret` and deliberately no `GetSecret`, asserted by `there_is_no_request_that_reads_a_secret_back` |
| `config.json`, `state.json`, `events.ndjson` | Local files | `0700` directories, `0600` files on Unix; validated and size-capped on read |
| `config.sbvault` | Designed to be shared, and assumed to leak | Authenticated encryption under a memory-hard KDF; that assumption is the design centre (`THREAT_MODEL.md` §A1) |
| Kopia subprocess | Outbound only | Environment built **from empty**, not inherited, so an ambient `AWS_ACCESS_KEY_ID` cannot silently redirect a backup to the wrong bucket; pinned `--config-file` and cache directory so a hand-run `kopia` is never raced; all output treated as untrusted text |
| HTTPS to a Git host | User-configured | rustls only, limited redirects, no `git` subprocess at all — a token handed to `git` ends up in an askpass helper, in `.git/config` and in `git`'s own error messages (`crates/core/src/remote.rs`) |
| HTTPS to GitHub for Kopia | Optional | Host allowlist enforced on the redirect policy itself, so no request is ever issued to a foreign host — not even a rejected one |
| GUI | Local | egui/eframe: immediate-mode, pure Rust. **No webview, no bundled browser engine, no Node build step** — a ~10 MB self-contained binary against a ~100 MB Electron application or a Tauri build carrying a WebView2 dependency (`ARCHITECTURE.md`) |

There is no network listener, no plugin system, no scripting engine, no
auto-loaded extension mechanism, and no embedded HTTP server. The only code
superbackup runs that it did not ship is the user's own configured hooks and
the Kopia binary.

The IPC protocol also serves a machine-readable description of itself,
generated from the same definitions the dispatcher uses, so the documented
surface cannot drift from the implemented one
(`every_request_variant_is_in_the_schema`,
`schema_parameter_names_are_the_ones_serde_accepts`,
`schema_marks_secrets_and_mutations`).

---

## Point (2)(k) — Reducing the impact of an incident

> *"be designed, developed and produced to reduce the impact of an incident
> using appropriate exploitation mitigation mechanisms and techniques"*

**Status: Partial.**

**Language-level — met.** The entire product is Rust. `crates/core/src/lib.rs`
carries `#![forbid(unsafe_op_in_unsafe_fn)]`, and CI compiles with
`RUSTFLAGS: -D warnings` and runs `clippy --all-targets --all-features` on
Windows, Linux and macOS. Whole classes of memory-corruption exploit primitive
are absent by construction rather than by mitigation.

**Design-level — met.**

- `panic = "abort"` in the release profile (`Cargo.toml`): a corrupted invariant
  terminates rather than unwinding through code that assumed it held.
- Locked and unlocked are different types, not a boolean. A locked `Vault`
  physically cannot answer `get()`; the secret-bearing accessors live on
  `OpenVault`, obtainable only through a `Result` (`crates/core/src/crypto/vault.rs`).
- Constant-time comparison of secret values (`subtle`), and zeroing on drop
  (`zeroize`).
- Blast-radius limits: purpose-separated HKDF subkeys, per-destination
  repository passphrase derivation, and a per-installation vault that does not
  open with another installation's passphrase
  (`a_vault_from_another_installation_does_not_open_with_our_passphrase`).
- Redaction as a second line behind the argv rule, so a single mistake in the
  primary control does not become a credential leak.

**Binary-level — gap.** No explicit exploit-mitigation build flags are
configured: no Control Flow Guard on Windows, no `-z relro,now` or fortify
settings on Linux, no hardened-runtime configuration on macOS. The release
profile sets `opt-level = 3`, `lto = "thin"`, `codegen-units = 1`,
`strip = true`, `panic = "abort"` and nothing security-specific. Platform
defaults (ASLR, DEP/NX) apply because the toolchain enables them, not because
this project asked. Recorded in
[`CONFORMITY_CHECKLIST.md`](CONFORMITY_CHECKLIST.md) with the plan to set
target-specific hardening flags and verify them with `winchecksec` /
`checksec`.

**Also a gap: no code signing.** Releases are not Authenticode-signed on
Windows or notarised on macOS. For a product whose whole job is to hold
credentials, an unsigned binary is a real weakness, and the checklist says so.

---

## Point (2)(l) — Security-relevant logging, with opt-out

> *"provide security related information by recording and monitoring relevant
> internal activity, including the access to or modification of data, services
> or functions, with an opt-out mechanism for the user"*

**Status: Partial.**

**Recording — met.** `events.ndjson` is an append-only activity log; `state.json`
records runs, per-destination progress and outcomes. Security-relevant events
that are recorded include: rejected IPC peers, refused connections and accept
failures (`crates/core/src/ipc/server.rs`); vault unlock failures, as typed
errors; every scheduling decision, including each `Skipped` with the reason it
was skipped — because "it didn't run and I don't know why" is the failure mode
that makes people stop trusting a backup tool (`ARCHITECTURE.md`); every
destination outcome; and, from `InstallOutcome`, the SHA-256 of every Kopia
archive installed and the fact that its signature was not verified.

Everything written passes through `redact::scrub` first.

**Retention — met.** `log_retention_days`, default 30, is user-configurable.

**Opt-out — partial.** `log_level` is configurable down to `Error`
(`LogLevel::{Error, Warn, Info, Debug, Trace}`, `crates/core/src/model.rs`), so
a user can reduce logging substantially. There is no setting that disables
activity recording entirely, and `state.json` is not optional — a backup tool
that does not record whether a backup happened is not doing its job. Whether
"reduce to errors only" satisfies "an opt-out mechanism for the user" is
arguable; the project's position is that it substantially does, and the
checklist carries an item to make the trade-off explicit in Settings rather than
implicit in a log level.

**Monitoring — surface pending.** The five-valued `Health` state behind the tray
icon and the activity log view are specified in `design/UX_SPEC.md` and not yet
wired in `crates/app`.

---

## Point (2)(m) — Secure and permanent removal of data and settings

> *"provide the possibility for users to securely and easily remove on a
> permanent basis all data and settings and, where such data can be transferred
> to other products or systems, ensure that this is done in a secure manner"*

**Status: Gap.**

There is no `superbackup uninstall` or `superbackup purge`.
`superbackup service uninstall` removes the service; `superbackup autostart
disable` removes the login entry; neither touches user data. Decommissioning
today means deleting the config, data and cache directories by hand — which is
sufficient and is what `PRIVACY.md` says ("delete the config directory and it is
gone"), but it is not "easily", and a user who does not know the directory
layout will leave a sealed vault and an event log behind.

The transfer limb is met: the only supported data transfer is publishing the
sealed vault to a shared Git repository, which is encrypted before it leaves and
signed where signers are pinned (`crates/core/src/remote.rs`), and
`superbackup config export` writes configuration with secrets never included.

**Plan**, in [`CONFORMITY_CHECKLIST.md`](CONFORMITY_CHECKLIST.md): add a
`superbackup uninstall [--purge]` command that stops the daemon, removes the
service and autostart entry, and — with explicit confirmation — deletes the
configuration, vault, state, event log and Kopia cache, reporting exactly what
was removed and what was deliberately left (backup destinations are the user's
data and must never be touched). Cover it in
[`ANNEX_II_USER_INFORMATION.md`](ANNEX_II_USER_INFORMATION.md) point 8(d).

A note on "securely": on a modern SSD with wear levelling and an
overprovisioned flash translation layer, overwriting a file does not reliably
destroy its previous contents. The honest answer is that the vault's security
does not depend on deletion — it is encrypted at rest under a passphrase that
was never stored — so removing it is a tidiness and residual-metadata measure,
not the control that protects the secrets. The command's copy should say that
rather than implying a secure-erase guarantee the storage stack cannot give.

---

## Summary

| Point | Requirement | Status |
|---|---|---|
| (1) | Appropriate level of cybersecurity based on the risks | Met |
| (2)(a) | No known exploitable vulnerabilities | Met |
| (2)(b) | Secure by default configuration; reset to original state | Partial — reset missing |
| (2)(c) | Security updates, automatic where applicable | Partial — no self-update for superbackup itself |
| (2)(d) | Protection from unauthorised access; report on it | Met (reporting surface pending) |
| (2)(e) | Confidentiality of data | Met, with the disclosed `LocalMirror` exception |
| (2)(f) | Integrity of data, commands, programs, configuration | Met |
| (2)(g) | Data minimisation | Met |
| (2)(h) | Availability of essential and basic functions | Met |
| (2)(i) | Minimise impact on other devices and networks | Met |
| (2)(j) | Limit attack surfaces | Met |
| (2)(k) | Reduce the impact of an incident | Partial — no binary hardening flags, no code signing |
| (2)(l) | Security-relevant logging with opt-out | Partial — opt-out is a log level, monitoring surface pending |
| (2)(m) | Secure permanent removal of data and settings | Gap |

Every Partial and Gap above appears in the consolidated list at the end of
[`CONFORMITY_CHECKLIST.md`](CONFORMITY_CHECKLIST.md).
