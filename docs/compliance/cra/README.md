# EU Cyber Resilience Act — compliance package

Regulation (EU) 2024/2847. Version 1, for superbackup 0.1.x.
Last reviewed 2026-08-31.

This directory answers three questions, in this order:

1. Does the Cyber Resilience Act apply to superbackup as it stands today?
2. What would change that?
3. If it applied, would superbackup meet it?

The answers are **no**, **commercialisation by someone**, and **substantially
yes, with a short list of real gaps**. The gaps are collected in
[`CONFORMITY_CHECKLIST.md`](CONFORMITY_CHECKLIST.md) and are the most useful
part of this package.

Like the rest of `docs/compliance/`, this is written to be falsifiable. Every
control asserted here points at a file, a test, or a document in this
repository. A claim that stops being true is a bug report.

**This is not legal advice.** It is an engineering assessment written by the
project's maintainer, published so that a downstream distributor, an auditor,
or a curious user can check the reasoning rather than take it on trust.

---

## The documents

| Document | What it is |
|---|---|
| [`ANNEX_I_PART_I.md`](ANNEX_I_PART_I.md) | The essential cybersecurity requirements, point by point, each with the specific control that meets it and the code or test that proves it |
| [`ANNEX_I_PART_II.md`](ANNEX_I_PART_II.md) | Vulnerability handling: SBOM, disclosure, update delivery, testing, dependency remediation policy |
| [`ANNEX_II_USER_INFORMATION.md`](ANNEX_II_USER_INFORMATION.md) | The information that must reach the user, and exactly where each item lives |
| [`ANNEX_V_DECLARATION_OF_CONFORMITY.md`](ANNEX_V_DECLARATION_OF_CONFORMITY.md) | A structurally correct EU Declaration of Conformity **template**, unsigned, with placeholders only a real legal manufacturer can fill |
| [`ANNEX_VII_TECHNICAL_DOCUMENTATION.md`](ANNEX_VII_TECHNICAL_DOCUMENTATION.md) | The technical file: what it consists of and where each Annex VII item is satisfied |
| [`RISK_ASSESSMENT.md`](RISK_ASSESSMENT.md) | The Article 13(2)–(3) cybersecurity risk assessment, derived from `THREAT_MODEL.md` |
| [`SUPPORT_POLICY.md`](SUPPORT_POLICY.md) | The declared support period, what support means for a volunteer project, and end-of-life terms |
| [`VULNERABILITY_REPORTING.md`](VULNERABILITY_REPORTING.md) | Article 14: the 24 h / 72 h / 14 day obligations, who does what, and the submission route |
| [`CONFORMITY_CHECKLIST.md`](CONFORMITY_CHECKLIST.md) | An auditable item-by-item checklist with current status, and the consolidated gap list |

They sit on top of documents that already existed and are not restated here:
[`THREAT_MODEL.md`](../THREAT_MODEL.md), [`PRIVACY.md`](../PRIVACY.md),
[`THIRD_PARTY.md`](../THIRD_PARTY.md), [`SECURITY.md`](../../../SECURITY.md),
[`ARCHITECTURE.md`](../../ARCHITECTURE.md), and
[`SBOM.md`](../SBOM.md). Where this package and one of those disagree, the
older document is right and this one is a bug.

---

## When the Regulation applies

Article 71. The CRA entered into force on **10 December 2024** and applies from
**11 December 2027**, with two earlier phases:

| From | What applies |
|---|---|
| **11 June 2026** | Chapter IV, Articles 35–51 — notification of conformity assessment bodies. Machinery for notified bodies; nothing for a manufacturer to do. |
| **11 September 2026** | **Article 14** — the reporting obligations for actively exploited vulnerabilities and severe incidents. This is the first date on which a manufacturer in scope acquires a live duty. |
| **11 December 2027** | Everything else: Annex I requirements, technical documentation, conformity assessment, the EU declaration of conformity, CE marking, support period. |

At the time of writing, 31 August 2026, Article 14 is 11 days away. That is why
[`VULNERABILITY_REPORTING.md`](VULNERABILITY_REPORTING.md) exists now rather
than in 2027, even though the conclusion below is that it does not yet bind
this project.

No harmonised standard under the CRA has been published in the *Official
Journal* yet, so the Article 27 presumption of conformity is not available to
anyone, for any product category. Applied standards are dealt with in
[`ANNEX_VII_TECHNICAL_DOCUMENTATION.md`](ANNEX_VII_TECHNICAL_DOCUMENTATION.md).

---

## Applicability analysis

### 1. Is superbackup a "product with digital elements"?

Yes.

Article 3(1) covers software and hardware products and their remote data
processing solutions. Article 2(1) applies the Regulation to such products
*"made available on the market, the intended purpose or reasonably foreseeable
use of which includes a direct or indirect logical or physical data connection
to a device or network."*

superbackup is software, and it connects: to a configured S3 endpoint through
Kopia, to a configured Git host for shared configuration, and to GitHub's
release API to fetch and update the Kopia binary
(`crates/core/src/kopia/install.rs`). The complete list is in
[`PRIVACY.md`](../PRIVACY.md). The connection limb is comfortably met.

None of the Article 2(2)–(7) exclusions apply: it is not a medical device, not
a motor vehicle, not certified under Regulation (EU) 2018/1139, not marine
equipment, not a spare part, and not developed exclusively for national
security or defence.

So the product limb is satisfied. The question turns entirely on the second
limb.

### 2. Is it "made available on the market"?

**No.** This is where the analysis ends for today.

Article 3(22) defines making available on the market as *"the supply of a
product with digital elements for distribution or use on the Union market **in
the course of a commercial activity**, whether in return for payment or free of
charge."* Free of charge does not exempt anything. Commercial activity does.

Recital 15 lists what makes a supply commercial. Recital 18 applies that
specifically to free and open-source software, and states that *"the provision
of products with digital elements qualifying as free and open-source software
that are not monetised by their manufacturers should not be considered to be a
commercial activity."* The European Commission's guidance on applying the CRA,
C(2026) 5252, approved 27 July 2026, takes the same line.

Against each monetisation trigger in Recital 15:

| Trigger | superbackup |
|---|---|
| Charging a price for the product | No. MIT-licensed, no price, no paid edition, no feature gating. |
| Charging for technical support beyond cost recovery | No. There is no paid support and no support contract. [`SECURITY.md`](../../../SECURITY.md) says plainly that this is a personal project with no funded programme. |
| An intention to monetise — e.g. a platform through which other services are sold | No. There is no service, no account, and no server. [`PRIVACY.md`](../PRIVACY.md): "superbackup has no servers." |
| Requiring processing of personal data for purposes other than security, compatibility or interoperability | No. There is no telemetry, no analytics, no crash reporting, and no phone-home update check. The machine identifier is a random UUID deliberately not derived from any hardware value. |
| Accepting donations exceeding the costs of design, development and provision | No. There is no donation channel at all — no GitHub Sponsors, no Open Collective, no `FUNDING.yml`. |

Two further points from Recital 18, because they are the ones most often got
wrong:

- **How the project is funded or who contributes does not matter.** *"The mere
  circumstances under which the product with digital elements has been
  developed, or how the development has been financed, should therefore not be
  taken into account."* Financial support from a manufacturer, or contributions
  from employees of one, would not by itself make this commercial.
- **Regular releases do not matter.** *"The mere presence of regular releases
  should not in itself lead to the conclusion that a product with digital
  elements is supplied in the course of a commercial activity."*

And Recital 20: hosting the source on GitHub is not, by itself, making it
available on the market.

**Conclusion: superbackup is not made available on the market within the
meaning of Article 3(22), so Article 2(1) is not engaged and no obligation
under the Regulation applies to it today.**

### 3. Is the maintainer a "manufacturer"?

No. Article 3(13) defines a manufacturer as a person who develops a product
with digital elements *"and markets them under its name or trademark, whether
for payment, monetisation or free of charge."* Marketing is supply on the
market, which requires commercial activity, which is absent. Nobody is a
manufacturer of superbackup.

### 4. Is the maintainer an "open-source software steward" under Article 24?

No, on two independent grounds, and this is worth stating precisely because the
steward regime is widely assumed to catch every open-source maintainer.

Article 3(14): an open-source software steward is *"a **legal person**, other
than a manufacturer, that has the purpose or objective of systematically
providing support on a sustained basis for the development of specific products
with digital elements, qualifying as free and open-source software and
**intended for commercial activities**, and that ensures the viability of those
products."*

- superbackup is maintained by a **natural person**. Article 3(14) says legal
  person. A sole individual maintaining their own project is not a steward.
  Recital 19 confirms the intended targets: *"certain foundations as well as
  entities that develop and publish free and open-source software in a business
  context."*
- superbackup is **not intended for commercial activities**. It is an
  end-user application, not a component supplied for integration into monetised
  products. Recital 19 limits the steward regime to products *"ultimately
  intended for commercial activities, such as for integration into commercial
  services or into monetised products."*

Neither limb is met, so **Article 24 does not apply**, and neither do the
Article 14(1) and 14(3)/(8) obligations that Article 24(3) extends to stewards.

Article 25 (voluntary security attestation programmes for FOSS) is enabling
legislation for the Commission, not an obligation on anyone. If such a
programme is established, superbackup would be a reasonable candidate.

### 5. If it were in scope, what class would it be?

This is the part where the honest answer is "arguable", and the project takes
the conservative side.

**Not critical (Annex IV).** All three Annex IV categories — hardware devices
with security boxes, smart meter gateways and secure cryptoprocessing devices,
smartcards and secure elements — are hardware. superbackup is software. Not
close.

**Arguably important, Annex III Class I, category 3: password managers.**
Commission Implementing Regulation (EU) 2025/2392, which supplies the technical
descriptions required by Article 7(4), describes password managers as products
*"that store passwords, locally on a device or on a remote server, including
activities such as generation of passwords as well as password sharing and
integration with local or third-party applications for usage of passwords."*

Read against superbackup, that description is uncomfortably close:

| Element of the description | superbackup |
|---|---|
| Stores passwords locally | Yes. `config.sbvault` holds every repository passphrase, S3 key pair and Git token (`crates/core/src/crypto/vault.rs`). |
| Generation of passwords | Yes. Generated passphrases are 256 bits from the OS CSPRNG (`THREAT_MODEL.md` §4). |
| Password sharing | Arguably. The sealed vault can be published to a shared Git repository and pulled by another machine or another person (`crates/core/src/remote.rs`). |
| Integration with local applications for usage of passwords | Yes. Credentials are injected into the `kopia` child process environment (`crates/core/src/kopia/command.rs`). |

The counter-argument is Article 7(1), which makes a product important only
where it has *"the core functionality"* of an Annex III category. superbackup's
core functionality is scheduling and running backups; the vault exists to serve
that and is not offered as a general credential store. Concretely:

- A user cannot store an arbitrary password in it for their own later use. The
  vault holds `SecretRef` handles referenced by the configuration
  (`crates/core/src/model.rs`), not free-form entries.
- **There is no way to read a secret back out.** The IPC protocol offers
  `SetSecret` and deliberately no `GetSecret`
  (`crates/core/src/ipc/protocol.rs`), asserted by the test
  `there_is_no_request_that_reads_a_secret_back` in
  `crates/core/tests/ipc_protocol.rs`. Giving credentials back to the user is
  the defining function of a password manager, and superbackup refuses to do
  it.
- There is no browser integration, no autofill, no clipboard handling, and no
  per-site credential model.

**The position this project takes:** the "core functionality" argument is
sound, but it is not so overwhelming that a market surveillance authority could
not reach the other view, and a compliance document that assumes the convenient
reading is worth nothing. superbackup therefore **documents itself to the
Annex III Class I standard**, not the default standard.

The practical cost of doing so is close to zero, for a specific reason worth
spelling out:

- A *default* product uses Article 32(1): internal control (Module A) is
  available.
- An *Annex III Class I* product normally falls to Article 32(2) — where no
  harmonised standard has been applied, and none exist, Module B+C or Module H
  is required, meaning a **notified body**.
- But **Article 32(5)** provides that manufacturers of products qualifying as
  free and open-source software which fall under Annex III *"shall be able to
  demonstrate conformity … by using one of the procedures referred to in
  paragraph 1, provided that the technical documentation referred to in Article
  31 is made available to the public at the time of the placing on the market."*

superbackup's technical documentation is this repository, public from the
moment anything is released. So even on the pessimistic classification, a FOSS
manufacturer of superbackup could use Module A. The classification question
changes the paperwork, not the route.

A **closed-source commercial distributor** does not get Article 32(5). See
below.

### 6. What would bring superbackup into scope

Any one of these, and the person doing it becomes a manufacturer with the full
Article 13 obligation set:

| Change | Effect |
|---|---|
| Charging for the product, or for a "pro" tier | Commercial activity. In scope. |
| Selling support, hosting, or managed deployment beyond actual cost recovery | Commercial activity (Recital 15). In scope. |
| Monetising adjacent services through it — a paid storage backend, a paid dashboard | Commercial activity. In scope. |
| Accepting donations beyond the costs of design, development and provision | Commercial activity (Recital 15). Note the threshold is *exceeding costs*, not *any donation*: "accepting donations without the intention of making a profit should not be considered to be a commercial activity." |
| Requiring personal data processing for anything other than security, compatibility or interoperability | Commercial activity. In scope. |
| A company distributing superbackup as part of, or bundled with, a commercial product | **That company** becomes the manufacturer for what it places on the market. This project does not thereby come into scope: Recital 18 makes supply of a FOSS component for integration "making available on the market" only if *the original manufacturer* monetises it. |
| A legal person being formed to sustain the project, where the project is intended for commercial activities | Article 24 steward regime — a lighter set of duties, not the manufacturer set. |

If any of the first five happen, the trigger is immediate for Article 14
reporting (already applicable since 11 September 2026) and applies in full from
11 December 2027, including for products already on the market before that
date.

### 7. What a downstream commercial distributor inherits

Someone who ships superbackup inside a commercial product — an MSP bundling it
into a managed-backup offering, a hardware vendor preloading it, a company
selling a supported build — becomes the manufacturer of *their* product. What
they get from this repository, and what they still have to do themselves:

**What they can take and reuse**

- The SBOM ([`sbom/`](../../../sbom/)), regenerable and schema-validated, as an
  input to their own Annex I Part II(1) SBOM. It describes superbackup's
  components; it does not describe theirs.
- The risk assessment ([`RISK_ASSESSMENT.md`](RISK_ASSESSMENT.md)) and threat
  model as inputs to their Article 13(2) assessment. Their operating
  environment, threat model and user population differ, and the assessment must
  be redone in that light — Article 13(3) requires it to reflect *their*
  intended purpose and conditions of use.
- The architecture description ([`ARCHITECTURE.md`](../../ARCHITECTURE.md)) for
  the Annex VII point 2(a) system-architecture requirement.
- The Annex I Part I mapping in [`ANNEX_I_PART_I.md`](ANNEX_I_PART_I.md) as
  evidence for the components they did not write.
- The licence and attribution analysis in
  [`THIRD_PARTY.md`](../THIRD_PARTY.md), including the Apache-2.0 obligations
  that come with bundling a Kopia binary.

**What they must do themselves — none of this transfers**

1. **Their own cybersecurity risk assessment** (Article 13(2)–(4)), documented,
   in their technical file, and updated through the support period.
2. **Due diligence on integrated third-party components** (Article 13(5)),
   which explicitly includes free and open-source components *"that have not
   been made available on the market in the course of a commercial activity"* —
   that is, this one. They cannot discharge that duty by citing this document;
   they have to satisfy themselves.
3. **Determine and publish a support period** of at least five years, or the
   expected use time if shorter (Article 13(8)), for their product. This
   project's [`SUPPORT_POLICY.md`](SUPPORT_POLICY.md) binds nobody but this
   project, and a volunteer commitment is not a substitute for a commercial one.
4. **Technical documentation** to Article 31 and Annex VII, drawn up before
   placing on the market, kept for at least 10 years or the support period,
   whichever is longer (Article 13(13)).
5. **Conformity assessment** under Article 32. If their product has the core
   functionality of an Annex III category and is not itself free and open-source
   software with public technical documentation, Article 32(5) is unavailable
   to them; with no harmonised standards published, that means Module B+C or
   Module H and a **notified body**.
6. **EU declaration of conformity** (Article 28, Annex V) and **CE marking**
   (Article 30). Neither may be affixed to this project's releases —
   Recital 19 is explicit that stewards may not affix CE marking, and this
   project is not even a steward.
7. **Annex II information to users**, including their own identity, contact
   point, coordinated vulnerability disclosure policy, and support end date.
8. **Article 14 reporting** for actively exploited vulnerabilities and severe
   incidents in their product — 24 hours / 72 hours / 14 days, to the CSIRT
   designated as coordinator in their Member State of main establishment and to
   ENISA, via the single reporting platform.
9. **Article 13(6)**: on finding a vulnerability in superbackup, report it to
   this project and share any fix, where appropriate in machine-readable form.
   [`SECURITY.md`](../../../SECURITY.md) is the route.

Vulnerabilities in Kopia belong to the Kopia project. That relationship, and
what superbackup inherits from it, is in [`THREAT_MODEL.md`](../THREAT_MODEL.md)
§7 and [`THIRD_PARTY.md`](../THIRD_PARTY.md).

---

## Voluntary conformance

**superbackup conforms voluntarily to the substance of the CRA's essential
cybersecurity requirements and vulnerability handling requirements, and
maintains this package as if it were in scope.**

Three reasons, none of them regulatory:

1. **The engineering is the same either way.** Every Annex I Part I requirement
   that applies to a backup tool — secure defaults, protection from
   unauthorised access, confidentiality and integrity of data, attack surface
   minimisation, data minimisation, availability, secure update delivery — was
   already a design goal before this document existed. The mapping in
   [`ANNEX_I_PART_I.md`](ANNEX_I_PART_I.md) is almost entirely a description of
   decisions already taken and already tested. Where it is not, that is a gap
   worth knowing about.
2. **It removes the obstacle if the project is ever commercialised.** Deciding
   to charge for something should not require a year of retrospective
   documentation. Every artefact the Regulation asks for is either already here
   or is a named gap with an owner.
3. **It is useful to downstream users regardless.** A company evaluating
   superbackup for internal use wants exactly these documents, whether or not
   anyone is legally obliged to produce them.

What voluntary conformance **does not** mean:

- No CE marking is affixed, and none may be. CE marking asserts conformity
  under a procedure that has not been carried out by anyone who could carry it
  out.
- The EU Declaration of Conformity in this directory is an unsigned
  **template**. It names no manufacturer, no address, and no notified body,
  because inventing any of those would be a forgery rather than a document.
- No conformity assessment under Article 32 has been performed. What exists is
  a self-assessment against Annex I, published so it can be argued with.
- The Article 14 reporting obligations do not bind this project. The process in
  [`VULNERABILITY_REPORTING.md`](VULNERABILITY_REPORTING.md) is written so that
  it *could* be followed, and states plainly which parts are conditional.

---

## Review

This package is reviewed when the applicability analysis could change — a
funding channel, a paid offering, a change of maintainership to a legal entity
— when the threat model changes, at each minor release, and against the CRA's
phased dates.

| Version | Date | Change |
|---|---|---|
| 1 | 2026-08-31 | Initial package for 0.1.0. |
