# Annex II — Information and instructions to the user

Regulation (EU) 2024/2847, Annex II. Version 1, for superbackup 0.1.x.
Last reviewed 2026-08-31.

> *"At minimum, the product with digital elements shall be accompanied by:"*

Annex II is the only part of the CRA that dictates what the user must be able
to see. Nine numbered items, each answered below with the required content and
**exactly where it lives or must live**.

Article 13(18) additionally requires this information to be provided in a
durable form, accessible and user-friendly, and — where provided online — to
remain available for at least 10 years after the product is placed on the
market, or for the support period, whichever is longer.

Items marked **TO WIRE** are not yet in the product or the README. They are
collected in a single list at the end of this document so they can be actioned
in one pass, and they appear in
[`CONFORMITY_CHECKLIST.md`](CONFORMITY_CHECKLIST.md).

---

## 1. Manufacturer identity and contact

> *"the name, registered trade name or registered trademark of the manufacturer,
> and the postal address, the email address or other digital contact as well as,
> where available, the website at which the manufacturer can be contacted"*

**Content.** superbackup has no legal manufacturer, because it is not placed on
the market — see [`README.md`](README.md). The equivalent information is the
maintainer and the project home:

- Maintained by **Andreas Wiren** (`Cargo.toml`, `[workspace.package] authors`).
- Digital contact: <https://github.com/andreaswiren/superbackup> — issues for
  general contact, private vulnerability reporting for security.
- No postal address is published, and none is required of a project that is not
  a manufacturer. A downstream commercial distributor **must** publish its own;
  see [`README.md`](README.md) §7.

**Where.** Partly present: `LICENSE` and the About screen's copyright line
(`design/UX_SPEC.md` §13, item 6) carry the name. The contact route is in
`SECURITY.md` and `CONTRIBUTING.md`.

**TO WIRE — A1.** One "Maintainer and contact" line in the About screen and a
one-line statement in the README, so a user does not have to open the licence
file to find out who is responsible.

---

## 2. Single point of contact for vulnerability reports and the CVD policy

> *"the single point of contact where information about vulnerabilities of the
> product with digital elements can be reported and received, and where the
> manufacturer's policy on coordinated vulnerability disclosure can be found"*

**Content.** GitHub private vulnerability reporting on
`github.com/andreaswiren/superbackup` (Security → Report a vulnerability). The
coordinated vulnerability disclosure policy is
[`SECURITY.md`](../../../SECURITY.md).

**Where.** Present in `SECURITY.md`, surfaced by GitHub's own Security tab, and
linked from the README's Security section. **Not present in the product.**

**TO WIRE — A2.** A "Report a security issue" link in the About screen, next to
the existing "Report an issue" button but distinct from it, pointing at the
private reporting flow rather than the public issue tracker. This is the item a
CRA auditor looks for first, because it is the one obligation that has to reach
a user who never visits the repository.

---

## 3. Unique product identification

> *"name and type and any additional information enabling the unique
> identification of the product with digital elements"*

**Content.**

| | |
|---|---|
| Name | superbackup |
| Type | Desktop backup management application (software) |
| Version | From `Cargo.toml` `[workspace.package] version` |
| Build identity | `superbackup version --json` returns version, target OS and target architecture (`superbackup_core::build_info()`, `crates/core/src/lib.rs`) |
| Components | The SBOM in [`sbom/`](../../../sbom/) |

**Where.** Met. `superbackup version` and `superbackup version --json` are
implemented in `crates/app/src/main.rs`. The About screen specification
(`design/UX_SPEC.md` §13, item 1) shows the product name, version and build
line.

---

## 4. Intended purpose, security environment, essential functionality and
security properties

> *"the intended purpose of the product with digital elements, including the
> security environment provided by the manufacturer, as well as the product's
> essential functionalities and information about the security properties"*

**Content.**

**Intended purpose.** superbackup schedules and runs backups of a personal
developer machine to one or more destinations — Kopia repositories on local
disk, an external drive, a network share, a OneDrive folder or an S3-compatible
bucket, and plain folder mirrors — and reports truthfully whether they ran. It
does not implement deduplication, chunking or backup encryption; Kopia does
(`ARCHITECTURE.md`).

**Security environment assumed.** A single-user or trusted-multi-user desktop
or laptop, running a supported operating system with its own updates applied,
where the user controls the account superbackup runs as. Explicitly **not**
assumed to be secure: the destination (`THREAT_MODEL.md` §A3), the shared
configuration repository (§A4), the source filesystem (§A5), the output of the
Kopia subprocess (§A6), and other local processes running as the same user
(§A7).

**Security properties.** Summarised in the README's Security section and
specified in full in [`THREAT_MODEL.md`](../THREAT_MODEL.md) §4: Argon2id
key derivation with parameters in an authenticated header, XChaCha20-Poly1305
vault encryption, HKDF-SHA256 purpose-separated subkeys, Ed25519 configuration
signing, and OS CSPRNG for generated material.

**Where.** Met. README "What it does" / "What it is not" / "Security";
`ARCHITECTURE.md`; `THREAT_MODEL.md`; `PRIVACY.md`; About screen item 2.

---

## 5. Known or foreseeable circumstances leading to significant cybersecurity
risk

> *"any known or foreseeable circumstance, related to the use of the product
> with digital elements in accordance with its intended purpose or under
> conditions of reasonably foreseeable misuse, which may lead to significant
> cybersecurity risks"*

**Content.** Six, all already documented, none of them softened:

1. **A weak master passphrase.** The whole scheme reduces to it. Argon2id
   multiplies an attacker's cost; it does not turn a guessable passphrase into a
   secret. The vault is designed on the assumption that it will end up somewhere
   more public than intended (`THREAT_MODEL.md` §A1).
2. **There is no passphrase recovery.** Lose it and the backups are
   unrecoverable. `THREAT_MODEL.md` §3 notes this is the single most likely way
   a real user loses their data.
3. **Folder mirrors are unencrypted.** Anyone with the folder has the files
   (`THREAT_MODEL.md` §A3, `PRIVACY.md`).
4. **Enabling `use_os_keychain` is a real trade-off.** It caches the master key
   in the platform credential store so unattended runs work; anything that can
   run programs as the user can then ask the credential store for the key
   (`design/COPY.md`, `onboarding.service.keychain_warn`).
5. **The `_superbackup/` manifest directory and object timing are readable at
   the destination.** Machine label, hostname, OS version, architecture and
   timestamps, plus how much changes and when (`PRIVACY.md`).
6. **A shared configuration repository with no pinned signers extends trust.**
   Anyone who can write the repository and knows the passphrase can redirect a
   job to their own bucket (`THREAT_MODEL.md` §A4).

**Where.** Met in documentation; met in the interface *as specified*
(`design/COPY.md` carries the strings for 2, 3 and 4 at the point of choice),
**not yet met in the shipped product** because the GUI is not wired. Recorded in
the checklist as "core met, surface pending".

**TO WIRE — A3.** A short "Known limitations" subsection in the README
collecting all six in one place. They are currently distributed across the
README, the threat model and the privacy document; a user should not have to
assemble them.

---

## 6. Internet address of the EU declaration of conformity

> *"where applicable, the internet address at which the EU declaration of
> conformity can be accessed"*

**Not applicable.** No EU declaration of conformity has been or may be issued,
because superbackup is not placed on the market and no conformity assessment has
been carried out. The unsigned template is at
[`ANNEX_V_DECLARATION_OF_CONFORMITY.md`](ANNEX_V_DECLARATION_OF_CONFORMITY.md)
and is marked as a template on its first line.

A downstream commercial distributor issues its own and publishes its own
address.

---

## 7. Type of security support and the end date of the support period

> *"the type of technical security support offered by the manufacturer and the
> end-date of the support period during which users can expect vulnerabilities
> to be handled and to receive security updates"*

**Content.** [`SUPPORT_POLICY.md`](SUPPORT_POLICY.md), in full. In summary:

- Support means security fixes and coordinated disclosure handling. It does not
  mean an SLA, a helpdesk, or guaranteed feature work.
- The declared support period for a release is **five years from its release
  date**, which is the Article 13(8) minimum.
- During 0.x, only the latest release receives security fixes
  (`SECURITY.md`).
- End-of-life is announced at least **6 months** in advance.

Article 13(19) requires the end date — *"including at least the month and the
year"* — to be clearly specified at the time of purchase, in an easily
accessible manner, and to be displayed to the user when reached where
technically feasible.

**Where.** `SUPPORT_POLICY.md` only. **Not in the README, not in the product.**

**TO WIRE — A4.** The support end date, as a month and year, in three places:

1. `README.md`, in a short "Support" section.
2. The About screen, in the key/value block — a `Security support until`
   row next to `Kopia` and `Machine`.
3. `CHANGELOG.md`, on each release heading, so the date is attached to the
   version it belongs to rather than to a document that gets edited.

**TO WIRE — A5.** An end-of-support notification: when the running version's
support end date has passed, the tray shows `Attention` and the About screen
says so. This is the Article 13(19) second-subparagraph "where technically
feasible" limb, and for a desktop application with a tray icon it plainly is
feasible. It needs no network: the end date is a build-time constant.

---

## 8. Detailed instructions

### 8(a) Measures for secure initial commissioning and use throughout life

> *"the necessary measures during initial commissioning and throughout the
> lifetime of the product with digital elements to ensure its secure use"*

**Content.** Choose a strong master passphrase and store it somewhere you can
get to; understand that there is no recovery; choose whether a destination is a
Kopia repository (encrypted) or a folder mirror (not); pin `trusted_signers` on
any shared configuration repository with more than one writer; decide
deliberately about `use_os_keychain`; run `superbackup doctor` to confirm which
Kopia binary is in use.

**Where.** README "Getting started"; onboarding flow in `design/UX_SPEC.md` and
`design/COPY.md` (`O-3 There is no recovery`, the destination-kind explanation,
the keychain warning); `THREAT_MODEL.md` §A4 on pinned signers. **Met in
documentation and specification; surface pending.**

### 8(b) How changes affect the security of data

> *"how changes to the product with digital elements can affect the security of
> data"*

**Content.** The changes that matter, and what each does:

| Change | Effect |
|---|---|
| Turning on `use_os_keychain` | The master key becomes reachable by anything running as the user |
| Adding a folder-mirror destination to a job | That copy is unencrypted |
| Pointing `kopia.source_repo` at a mirror | Moves the supply-chain trust anchor. `crates/core/src/model.rs` says so, and the interface must |
| Setting `kopia_path` by hand | You are driving an untested Kopia build |
| Lowering `kopia.minimum_version` | Weakens the downgrade guard. The hard floor still applies |
| Adding a shared config repository without pinned signers | Extends trust to everyone who can write it |
| Raising `auto_lock_minutes` | The key stays in memory longer |
| Changing a provider's credentials | Rotates them for every bucket and job using that provider, unless a destination pinned an override (`ARCHITECTURE.md`) |

**Where.** Scattered across the threat model, the architecture document and
per-setting documentation comments in `crates/core/src/model.rs`. **Not
collected anywhere a user would find it.**

**TO WIRE — A6.** This table, in the Settings screen's help or in a "Settings
that change your security posture" section of the documentation. It is the
single highest-value piece of user-facing security writing this project does not
yet have.

### 8(c) How to install security-relevant updates

> *"how security-relevant updates can be installed"*

**Content.** Today: replace the binary, from source or from the release page.
For the Kopia component: `superbackup doctor --fix`, or the update policy in
Settings.

**Where.** README "Install" covers building from source. There is no released
binary and no update mechanism — see
[`ANNEX_I_PART_II.md`](ANNEX_I_PART_II.md) point (7).

**TO WIRE — A7.** A README "How to get security updates" subsection, written to
be true of whatever the release channel turns out to be. This has to follow the
release pipeline, not precede it.

### 8(d) Secure decommissioning, including removal of user data

> *"the secure decommissioning of the product with digital elements, including
> information on how user data can be securely removed"*

**Content.** What must be removed, and in which order:

1. Stop the daemon and remove the service: `superbackup service uninstall`.
2. Remove the autostart entry: `superbackup autostart disable`.
3. Delete the configuration directory — `config.json`, `config.sbvault`.
4. Delete the data directory — `state.json`, `events.ndjson`, the managed Kopia
   binary.
5. Delete the cache directory — the Kopia cache, the remote-config clone.
6. Decide separately what happens to the **destinations**. They are the user's
   backups and superbackup will never delete them as part of decommissioning.
   Removing a Kopia repository is a Kopia operation; removing a folder mirror is
   a file deletion.
7. Revoke credentials at their source: S3 keys at the provider, the GitHub token
   in GitHub's settings. Deleting the vault does not revoke anything.

Step 7 is the one people forget and the one that actually matters.

**Where.** Steps 1 and 2 exist as commands. Steps 3–5 are manual and the exact
paths are resolved at runtime by `crates/core/src/paths.rs`; `PRIVACY.md` says
"delete the config directory and it is gone" but does not print the path.

**TO WIRE — A8.** The `superbackup uninstall [--purge]` command described in
[`ANNEX_I_PART_I.md`](ANNEX_I_PART_I.md) point (2)(m), plus a
"Decommissioning" section in the README carrying steps 6 and 7, which no command
can do for the user.

On "securely": on an SSD with wear levelling, overwriting a file does not
reliably destroy its previous contents. The vault's protection is that it is
encrypted at rest under a passphrase that was never stored — not that its bytes
were scrubbed. Documentation should say that rather than imply a secure-erase
guarantee the storage stack cannot provide.

### 8(e) How to turn off automatic security updates

> *"how the default setting enabling the automatic installation of security
> updates, as required by Part I, point (2)(c), of Annex I, can be turned off"*

**Content.** superbackup has no automatic self-update, so there is nothing to
turn off. For the Kopia component, `kopia.auto_update` defaults to
`UpdatePolicy::Notify` — updates are surfaced, not applied — and can be set to
never check; `kopia.pinned_version` freezes an exact version for reproducible
deployments; `kopia.auto_install` can be turned off entirely, in which case
superbackup will use only a Kopia the user provides.

**Where.** Documented in `crates/core/src/model.rs`. **Not in the README.**

**TO WIRE — A9.** Include in the README "How to get security updates"
subsection (A7).

### 8(f) Information for integrators

> *"where the product with digital elements is intended for integration into
> other products with digital elements, the information necessary for the
> integrator to comply with the essential cybersecurity requirements set out in
> Annex I and the documentation requirements set out in Annex VII"*

**Content.** superbackup is an end-user application, not a component intended
for integration. It is nonetheless integrable — someone can bundle the binary —
and [`README.md`](README.md) §7, "What a downstream commercial distributor
inherits", is written precisely for that reader: what can be reused, what must
be redone, and the fact that Article 32(5)'s open-source route is not available
to a closed-source integrator.

**Where.** Met, in [`README.md`](README.md) §7.

---

## 9. Where the SBOM can be accessed

> *"If the manufacturer decides to make available the software bill of materials
> to the user, information on where the software bill of materials can be
> accessed."*

**Content.** The SBOM is published, so this applies. It is at
[`sbom/`](../../../sbom/) in the repository and is attached to every GitHub
release as `superbackup-<version>.cdx.json` and `.cdx.xml`. How to consume and
verify it is in [`SBOM.md`](../SBOM.md).

**Where.** Present in `sbom/README.md` and `docs/compliance/SBOM.md`. **Not in
the product or the top-level README.**

**TO WIRE — A10.** A "Software bill of materials" row in the About screen's
key/value block linking to the release asset, and a line in the README's
Documentation table pointing at `docs/compliance/SBOM.md`.

---

## Consolidated list of things to wire

Ten items. Grouped by where the change goes, because that is how they will be
actioned.

### In `README.md`

| Id | Change |
|---|---|
| **A1** | One line naming the maintainer and the contact route |
| **A3** | A "Known limitations" subsection collecting the six circumstances in item 5 |
| **A4** | A "Support" section carrying the support period and the end date as month and year, linking to `SUPPORT_POLICY.md` |
| **A7 / A9** | A "How to get security updates" subsection: how to update superbackup, how the Kopia update policy works, and how to turn it off or pin a version |
| **A8** | A "Decommissioning" section: the removal steps, and the reminder to revoke S3 keys and the GitHub token at their source |
| **A10** | Rows in the Documentation table for `docs/compliance/SBOM.md` and `docs/compliance/cra/README.md` |

### In `CHANGELOG.md`

| Id | Change |
|---|---|
| **A4** | On each release heading, the security-support end date for that release, as month and year |
| — | An entry for this compliance package under `[Unreleased] / Added` |

### In the app's About screen (`design/UX_SPEC.md` §13, then `crates/app`)

| Id | Change |
|---|---|
| **A1** | A "Maintainer" line: `Andreas Wiren` with the project URL |
| **A2** | A **"Report a security issue"** link to GitHub private vulnerability reporting, visually distinct from the existing public "Report an issue" button |
| **A4** | A `Security support until` row in the key/value block, populated from a build-time constant |
| **A5** | When that date has passed: an `Attention` tray state and a clear statement in About that this version is out of support |
| **A10** | A `Software bill of materials` row linking to the published SBOM |

### Elsewhere

| Id | Change |
|---|---|
| **A6** | The "changes that affect the security of your data" table in item 8(b), as Settings help or a documentation page |
| **A8** | The `superbackup uninstall [--purge]` command |

None of these are large. Together they are the difference between a product that
has the information somewhere in its repository and one that gives it to the
user, which is the whole point of Annex II.
