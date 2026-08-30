# Annex VII — Technical documentation

Regulation (EU) 2024/2847, Article 31 and Annex VII. Version 1, for
superbackup 0.1.x. Last reviewed 2026-08-31.

Article 31(1): the technical documentation *"shall contain all relevant data or
details of the means used by the manufacturer to ensure that the product with
digital elements and the processes put in place by the manufacturer comply with
the essential cybersecurity requirements set out in Annex I"*, and at least the
elements in Annex VII. Article 31(2): it must be drawn up before the product is
placed on the market and continuously updated, at least during the support
period.

**The technical file is this repository.** It is not a separate document that
paraphrases the code; it is the code, the tests, the design documents and the
generated artefacts, indexed here. That is deliberate. A technical file written
as prose about a codebase drifts from it within one release; a technical file
that consists of the codebase plus an index cannot.

Article 32(5) matters to this arrangement: a manufacturer of a product
qualifying as free and open-source software which falls under Annex III may use
the Article 32(1) procedures *"provided that the technical documentation
referred to in Article 31 is made available to the public at the time of the
placing on the market"*. Publishing the technical file is not just tidy here —
it is the thing that keeps the assessment route open. See
[`README.md`](README.md) §5.

Retention, when it applies: Article 13(13) requires the technical documentation
and the declaration of conformity to be kept at the disposal of market
surveillance authorities for at least 10 years after the product is placed on
the market, or for the support period, whichever is longer. The repository's
git history is the retention mechanism, and every artefact below is versioned in
it.

---

## Index against Annex VII

### 1. General description of the product

> *"a general description of the product with digital elements, including:"*

#### 1(a) Its intended purpose

superbackup schedules and runs backups of a personal developer machine to one or
more destinations, and reports truthfully whether they ran. Intended users are
individuals backing up their own machines. Intended environment is a
single-user or trusted-multi-user desktop or laptop.

It implements no deduplication, chunking or backup encryption; Kopia does. What
superbackup provides is repository management, scheduling, execution, credential
custody and reporting.

| Source | Content |
|---|---|
| [`README.md`](../../../README.md) | "Why this exists", "What it does", "What it is not" |
| [`ARCHITECTURE.md`](../../ARCHITECTURE.md) | The role split between superbackup and Kopia |
| [`ANNEX_II_USER_INFORMATION.md`](ANNEX_II_USER_INFORMATION.md) point 4 | Intended purpose and the security environment assumed |

#### 1(b) Versions of software affecting compliance

| | |
|---|---|
| Product version | `[workspace.package] version` in `Cargo.toml`; reported by `superbackup version --json` |
| Minimum supported Rust version | 1.82, declared in `Cargo.toml` and enforced by the `msrv` CI job |
| Every dependency and its exact version | [`Cargo.lock`](../../../Cargo.lock) and the SBOM in [`sbom/`](../../../sbom/) |
| Kopia | Not bound at build time. Minimum 0.17.0 (`MINIMUM_KOPIA_VERSION`, `crates/core/src/kopia/binary.rs`); the resolved version is reported by `superbackup doctor` |
| Vault format | `FORMAT_VERSION` and `BODY_VERSION`, `crates/core/src/crypto/` — versioned separately so a body change and an envelope change fail differently |
| IPC protocol version | Negotiated per request; a mismatch is refused with an actionable message |
| Configuration schema version | Migrated forward on load; a document from the future is refused and left alone |

#### 1(c) Photographs or illustrations

Not applicable — superbackup is software, not hardware. Interface illustrations,
if a reviewer wants them, are in [`design/WIREFRAMES.md`](../../../design/WIREFRAMES.md)
and [`design/UX_SPEC.md`](../../../design/UX_SPEC.md).

#### 1(d) User information and instructions as set out in Annex II

[`ANNEX_II_USER_INFORMATION.md`](ANNEX_II_USER_INFORMATION.md), which maps all
nine Annex II items to their location and lists the ten that are not yet
surfaced to the user.

---

### 2. Design, development, production and vulnerability handling

> *"a description of the design, development and production of the product with
> digital elements and vulnerability handling processes, including:"*

#### 2(a) Design and development, including system architecture

> *"necessary information on the design and development … including, where
> applicable, drawings and schemes and a description of the system architecture
> explaining how software components build on or feed into each other and
> integrate into the overall processing"*

**[`docs/ARCHITECTURE.md`](../../ARCHITECTURE.md) is the answer to this
requirement.** It is not restated here; duplicating it would create two
descriptions that drift apart. What it covers, and why each part is
compliance-relevant:

| Section | Why it matters here |
|---|---|
| "One binary, several personalities" | There is exactly one executable to install, sign and update. The CLI is a thin IPC client and never touches the vault — which is why only one process ever drives a Kopia repository, enforced by a single-instance guard |
| "Crate layout" and "Layering inside the core" | Dependencies point downward only: `model` and `state` know about nothing else. That layering is what makes the security invariants checkable in isolation |
| "`model` — intent" | The configuration contains no secret material of any kind; everything that would be a password is a `SecretRef` resolved against the unlocked vault at the moment of use |
| "`config` and `crypto`" | Atomic writes, and a versioned self-describing vault format whose header is authenticated |
| "`kopia` — the backend driver" | The two rules that shape the module: secrets never enter `argv`, and every byte Kopia prints is untrusted text |
| "`engine`" | The scheduling gate, and why every rejection is a recorded `Skipped` with a reason |
| "`ipc` — the seam" | The endpoint as a privilege boundary; `SetSecret` and deliberately no `GetSecret`; a self-describing protocol so documentation cannot drift from implementation |
| "Cross-platform strategy" | Platform code confined behind traits, with a working implementation for every target and CI compiling all three |
| "Concurrency" | Progress coalescing and bounded queues — the availability properties under Annex I point (2)(h) and (2)(i) |

Supplementary design records: [`design/UX_SPEC.md`](../../../design/UX_SPEC.md)
(every screen, state and string), [`design/COPY.md`](../../../design/COPY.md)
(the exact security-relevant wording shown at each point of choice),
[`design/DESIGN_SYSTEM.md`](../../../design/DESIGN_SYSTEM.md).

The cryptographic design and its data flow diagram are in
[`THREAT_MODEL.md`](../THREAT_MODEL.md) §4 and §6.

#### 2(b) Vulnerability handling processes

> *"necessary information and specifications of the vulnerability handling
> processes put in place by the manufacturer, including the software bill of
> materials, the coordinated vulnerability disclosure policy, evidence of the
> provision of a contact address for the reporting of the vulnerabilities
> discovered in the product with digital elements and a description of the
> technical solutions chosen for the secure distribution of updates"*

Four named items. All four are answered in
[`ANNEX_I_PART_II.md`](ANNEX_I_PART_II.md); the index:

| Required item | Where |
|---|---|
| Software bill of materials | [`sbom/`](../../../sbom/) — CycloneDX 1.5, JSON and XML, generated by [`sbom/generate.py`](../../../sbom/generate.py), schema-validated in CI. Described in [`SBOM.md`](../SBOM.md) |
| Coordinated vulnerability disclosure policy | [`SECURITY.md`](../../../SECURITY.md) — channel, response times, scope, disclosure model, and stated limits |
| Evidence of a contact address | GitHub private vulnerability reporting on the repository, named in `SECURITY.md` and surfaced by GitHub's Security tab. **Not yet surfaced in the product** — see `ANNEX_II_USER_INFORMATION.md` item A2 |
| Technical solutions for secure update distribution | For the Kopia component, the verification chain in `crates/core/src/kopia/install.rs`, tested end to end in `crates/core/tests/kopia_install.rs`, described in [`ANNEX_I_PART_II.md`](ANNEX_I_PART_II.md) point (7). **For superbackup itself, no such mechanism exists** — the largest declared gap |
| Vulnerability remediation policy | [`ANNEX_I_PART_II.md`](ANNEX_I_PART_II.md) point (2): triage classes, targets, the dependency-vulnerability procedure, and the rule that an exception is written down in `deny.toml` with a reason and a review date or it does not exist |

#### 2(c) Production and monitoring processes, and their validation

> *"necessary information and specifications of the production and monitoring
> processes of the product with digital elements and the validation of those
> processes"*

For software, "production" is the build and release pipeline.

**What exists** ([`.github/workflows/ci.yml`](../../../.github/workflows/ci.yml)):
build and test on Windows, Linux and macOS; `clippy` with `-D warnings`;
`rustfmt --check`; `cargo deny check` for advisories, licences, bans and
sources; an MSRV check on 1.82.0; a cross-compile `cfg` check for
`x86_64-pc-windows-msvc` and `aarch64-apple-darwin`; release binaries built and
uploaded as 14-day workflow artefacts.

**And** ([`.github/workflows/sbom.yml`](../../../.github/workflows/sbom.yml)):
SBOM regeneration, a staleness check against `Cargo.lock`, schema validation
against the published CycloneDX 1.5 schemas, a vulnerability scan, and — on
release — build provenance attestation and release asset upload.

**Validation of those processes.** The checks are self-validating in the sense
that each one fails the build when violated, and the security properties are
expressed as named executable assertions rather than as prose (the list is in
[`ANNEX_I_PART_II.md`](ANNEX_I_PART_II.md) point (3)). The SBOM pipeline
additionally validates its own output against the specification's schemas, so a
malformed artefact cannot be published.

**What does not exist, stated plainly:**

- No release build workflow. `ci.yml` uploads per-run artefacts; it does not
  publish signed release binaries. There is no `release.yml`.
- No code signing: no Authenticode on Windows, no notarisation on macOS.
- No reproducible-build verification for the binaries. The SBOM is
  reproducible; the executables are not verified to be.
- SLSA build level: the attestation machinery in `sbom.yml` targets Build
  Level 2 for the SBOM artefacts. The binaries are at **Level 0**, because
  nothing signs or attests them.

All four are in [`CONFORMITY_CHECKLIST.md`](CONFORMITY_CHECKLIST.md).

---

### 3. Cybersecurity risk assessment

> *"an assessment of the cybersecurity risks against which the product … is
> designed, developed, produced, delivered and maintained pursuant to Article
> 13, including how the essential cybersecurity requirements set out in Part I
> of Annex I are applicable"*

Two documents, in this order:

1. **[`RISK_ASSESSMENT.md`](RISK_ASSESSMENT.md)** — the Article 13(2)–(3)
   assessment: methodology, assets, threat actors, sixteen identified risks with
   likelihood and impact, existing mitigations, and residual risk after
   treatment. Derived from and consistent with
   [`THREAT_MODEL.md`](../THREAT_MODEL.md), which remains the authoritative
   statement of the security design.
2. **[`ANNEX_I_PART_I.md`](ANNEX_I_PART_I.md)** — the applicability mapping
   Article 13(3) specifically requires: for each Part I point (2) requirement,
   whether it applies, how it is implemented, and the code or test that proves
   it. Where a requirement is not met, the gap and the plan.

Article 13(4) requires a clear justification where an essential requirement is
not applicable. No Part I requirement is claimed inapplicable. Point (2)(c)'s
automatic-update limb is qualified by "where applicable" and that qualification
is argued in `ANNEX_I_PART_I.md` — but the argument is not used to avoid the
work, and the item is still on the gap list.

---

### 4. Information used to determine the support period

> *"relevant information that was taken into account to determine the support
> period pursuant to Article 13(8) of the product with digital elements"*

[`SUPPORT_POLICY.md`](SUPPORT_POLICY.md), which states the declared period, the
Article 13(8) factors weighed, and — more usefully — what a volunteer project
can actually promise and what it cannot.

---

### 5. Standards and specifications applied

> *"a list of the harmonised standards applied in full or in part … and, where
> those harmonised standards, common specifications or European cybersecurity
> certification schemes have not been applied, descriptions of the solutions
> adopted to meet the essential cybersecurity requirements set out in Parts I
> and II of Annex I, including a list of other relevant technical specifications
> applied"*

**Harmonised standards applied: none.** Not as a choice — as at 31 August 2026
no harmonised standard under the CRA has been cited in the *Official Journal*.
Standardisation request M/606 was accepted by CEN, CENELEC and ETSI in April
2025; the horizontal EN 40000 series from CEN-CENELEC/JTC 13 WG 9 and the
vertical ETSI EN 304 6xx drafts are in enquiry or approval. **The Article 27
presumption of conformity is therefore not available to anyone, for any product
category.**

**Common specifications applied: none.** The Commission has adopted none under
Article 27(2).

**European cybersecurity certification scheme: none.**

Annex VII point 5 therefore requires a description of the solutions adopted
instead. That description is [`ANNEX_I_PART_I.md`](ANNEX_I_PART_I.md) and
[`ANNEX_I_PART_II.md`](ANNEX_I_PART_II.md), requirement by requirement.

#### Other technical specifications and practices actually applied

Claimed only where true today. Aspirational items are marked and appear on the
gap list rather than in this table.

| Reference | Relevance | Status |
|---|---|---|
| **RFC 9106** — Argon2 | Password-based key derivation. superbackup uses Argon2id at m=256 MiB, t=3, p=1 by default, with a hard floor of m=64 MiB, t=3 for new vaults, exceeding the RFC's second recommended option | **Applied**, `crates/core/src/crypto/kdf.rs` |
| **RFC 8439 / draft-irtf-cfrg-xchacha** — ChaCha20-Poly1305 and XChaCha20 | Vault AEAD. The 192-bit nonce is why random nonces are safe without a counter | **Applied**, `crates/core/src/crypto/vault.rs` |
| **RFC 5869** — HKDF | Purpose-separated subkey derivation with versioned `info` strings, so the master key is never used directly for two jobs | **Applied**, `crates/core/src/crypto/keys.rs` |
| **RFC 8032** — EdDSA / Ed25519 | Detached signatures on the shared configuration vault | **Applied**, `crates/core/src/crypto/signing.rs` |
| **CycloneDX 1.5** (ECMA-424 aligns with 1.6) | SBOM format. Both outputs validate against the published JSON schema and XSD | **Applied**, [`sbom/`](../../../sbom/) |
| **SPDX licence expressions** | Licence identifiers throughout the SBOM and `deny.toml` | **Applied** |
| **Semantic Versioning 2.0.0** | Version scheme; the basis for the 1.0 commitment to security-only patch releases | **Applied** |
| **Keep a Changelog** | `CHANGELOG.md` structure | **Applied** |
| **NIST SP 800-218 (SSDF v1.1)** | Secure software development framework. See the practice-level mapping below — partially applied, honestly | **Partial** |
| **ISO/IEC 27002:2022** | Specific controls only; see below | **Partial, control-level only** |
| **OpenSSF Scorecard** | Not run. A supply-chain posture measurement the project would benefit from | **Gap** |
| **SLSA v1.0** | Build Level 2 for the SBOM artefacts via GitHub attestations; **Level 0** for the binaries, which nothing signs or attests | **Partial** |
| **EN 18031-1/-2/-3** | Radio Equipment Directive delegated regulation (EU) 2022/30, for radio equipment. superbackup is not radio equipment. Cited here only to record that it was considered and does not apply | **Not applicable** |
| **ETSI EN 303 645** | Cyber security for consumer IoT. superbackup is not an IoT device and has no device-provisioning, no default credentials and no remote management surface. Its provisions on unique per-device credentials, secure update and minimising exposed attack surfaces have obvious analogues, but claiming conformance to a consumer-IoT standard for a desktop application would be noise | **Not applicable** |
| **ISO/IEC 27001** | An organisational ISMS certification. There is no organisation. Claiming it would be false | **Not applicable** |

#### NIST SP 800-218 (SSDF v1.1), practice by practice

Claimed only where evidenced.

| Practice | Status |
|---|---|
| **PO.1** Define security requirements | Applied — `THREAT_MODEL.md`, `SECURITY.md`, `CONTRIBUTING.md`, and this package |
| **PO.3** Supporting toolchains | Applied — pinned CI, `deny.toml`, `rustfmt.toml`, `Cargo.lock` committed |
| **PO.4** Define and use criteria for software security checks | Applied — `-D warnings`, `cargo deny check` with an empty ignore list, SBOM freshness and scan gates |
| **PO.5** Implement and maintain secure environments | Partial — CI is a hosted runner with pinned actions; no isolated build environment or hermetic build |
| **PS.1** Protect all forms of code | Applied — public git repository, signed-off contributions per `CONTRIBUTING.md` |
| **PS.2** Provide a mechanism for verifying software release integrity | **Gap** — no signed releases, no published checksums. The SBOM has attestations; the binaries do not |
| **PS.3** Archive and protect each software release | Partial — git tags and GitHub release history; no independent archive |
| **PW.1** Design software to meet security requirements | Applied — the layering and invariants in `ARCHITECTURE.md`, argued in `THREAT_MODEL.md` |
| **PW.2** Review the software design | Partial — reviewed by the maintainer; no independent review |
| **PW.4** Reuse well-secured software | Applied — "nothing is rolled by hand" (`THREAT_MODEL.md` §4 rule 5); `deny.toml` restricts sources to crates.io and bans alternative TLS stacks |
| **PW.5** Create source code adhering to secure practices | Applied — Rust, `#![forbid(unsafe_op_in_unsafe_fn)]`, `panic = "abort"`, `Secret`/`zeroize`/`subtle`, the mechanical `argv` audit |
| **PW.7** Review and/or analyse human-readable code | Partial — `clippy` on all targets with warnings denied; no third-party review |
| **PW.8** Test executable code | Applied — adversarial integration tests across three platforms; **no fuzzing** |
| **PW.9** Configure software to have secure settings by default | Applied — the defaults table in `ANNEX_I_PART_I.md` point (2)(b) |
| **RV.1** Identify and confirm vulnerabilities | Applied — `cargo deny`, SBOM scan, private vulnerability reporting |
| **RV.2** Assess, prioritise and remediate | Applied as policy — the triage classes in `ANNEX_I_PART_II.md` point (2) |
| **RV.3** Analyse vulnerabilities to identify root causes | Applied as policy — a fixed vulnerability adds a regression test; that is the project's stated response, and the existing named security tests are the pattern |

#### ISO/IEC 27002:2022 controls honestly claimable

Not an ISMS claim. Individual controls only, where a specific implemented
mechanism corresponds:

| Control | Implementation |
|---|---|
| 8.3 Information access restriction | Vault passphrase; owner-restricted IPC endpoint; `0700`/`0600` filesystem permissions |
| 8.5 Secure authentication | Argon2id with authenticated parameters; strength measurement; constant-time comparison |
| 8.9 Configuration management | Versioned, validated, migrated configuration; atomic writes; refusal to silently repair |
| 8.15 Logging | Event log with redaction and bounded retention |
| 8.24 Use of cryptography | Established primitives only, documented with rationale in `THREAT_MODEL.md` §4 |
| 8.25 Secure development life cycle | CI gates, threat model, review triggers |
| 8.26 Application security requirements | This package |
| 8.28 Secure coding | `CONTRIBUTING.md`, `clippy`, the `argv` audit, the `Secret` type |
| 8.29 Security testing in development | The named adversarial tests listed in `ANNEX_I_PART_II.md` point (3) |
| 8.30 Outsourced development | Not applicable |
| 8.31 Separation of environments | Partial — `kopia` invocations are pinned to superbackup's own config file and cache directory so a hand-run `kopia` is never raced |
| 5.21 Managing security in the ICT supply chain | `deny.toml`, the SBOM, the checksum-verified Kopia installer |

---

### 6. Test reports

> *"reports of the tests carried out to verify the conformity of the product …
> and of the vulnerability handling processes with the applicable essential
> cybersecurity requirements as set out in Parts I and II of Annex I"*

The test suite is the test report, and it is executable.

| | |
|---|---|
| Test sources | `crates/core/tests/` — 21 integration test files, plus unit tests in each module |
| How to reproduce | `cargo test --workspace --all-features` |
| Where results live | GitHub Actions run logs for every commit on `main` and every pull request, on `windows-latest`, `ubuntu-latest` and `macos-latest` |
| Which tests evidence which requirement | Named inline throughout [`ANNEX_I_PART_I.md`](ANNEX_I_PART_I.md) and listed together in [`ANNEX_I_PART_II.md`](ANNEX_I_PART_II.md) point (3) |
| Supply-chain and SBOM checks | `cargo deny check`; `python sbom/generate.py --check --validate`; the scan job in `sbom.yml` |

**Not present:** penetration test report, third-party security assessment,
fuzzing corpus and findings, code coverage report. Their absence is on the gap
list rather than papered over with a coverage number.

---

### 7. Copy of the EU declaration of conformity

> *"a copy of the EU declaration of conformity"*

**None exists**, and none may be issued by this project. The unsigned template
is [`ANNEX_V_DECLARATION_OF_CONFORMITY.md`](ANNEX_V_DECLARATION_OF_CONFORMITY.md),
marked as a template on its first line, with no manufacturer, address, notified
body or signature invented.

---

### 8. The SBOM, on a reasoned request from a market surveillance authority

> *"where applicable, the software bill of materials, further to a reasoned
> request from a market surveillance authority provided that it is necessary in
> order for that authority to be able to check compliance …"*

No request is needed. The SBOM is published unconditionally in
[`sbom/`](../../../sbom/), attached to every release, and described in
[`SBOM.md`](../SBOM.md).

Article 13(25) additionally allows ADCO to conduct a Union-wide dependency
assessment and market surveillance authorities to request SBOMs for that
purpose. Publishing removes the friction.

---

## Maintenance of this file

Article 31(2) requires continuous updating, at least during the support period.
Concretely, this package is reviewed and updated when:

- a new dependency, subprocess, dynamic library or downloaded artefact is
  introduced — which is also a `THREAT_MODEL.md` §9 review trigger, because a
  new external component is always a new trust relationship;
- the cryptographic design changes, or a new destination kind or credential type
  is added;
- the IPC surface gains a request that returns sensitive data;
- a gap on [`CONFORMITY_CHECKLIST.md`](CONFORMITY_CHECKLIST.md) is closed;
- a CRA harmonised standard is cited in the *Official Journal*, at which point
  section 5 above changes materially;
- the applicability analysis in [`README.md`](README.md) could change;
- at each minor release.
