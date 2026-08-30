# Support policy

Version 1, for superbackup 0.1.x. Last reviewed 2026-08-31.

This document states what "support" means for superbackup, for how long, and
what happens when it ends. It is written to be kept, not to sound reassuring.
An unkeepable promise is worse than a modest one, particularly for a tool people
rely on to still be working in five years.

Article 13(8) of Regulation (EU) 2024/2847 requires manufacturers to determine a
support period reflecting how long the product is expected to be in use, and
sets a floor: *"the support period shall be at least five years. Where the
product with digital elements is expected to be in use for less than five years,
the support period shall correspond to the expected use time."* Article 13(19)
requires the end date, at least the month and the year, to be clearly specified
at the time of purchase. Article 13(9) requires each security update issued
during the period to remain available for at least 10 years after it was
issued, or for the remainder of the support period, whichever is longer.

Those obligations do not currently bind this project — superbackup is not made
available on the market, see [`README.md`](README.md) — and the policy below is
adopted voluntarily anyway.

---

## The declared support period

**Five years from the release date of each minor version.**

That is the Article 13(8) floor, and it is chosen rather than exceeded for a
reason given below.

| Release line | Released | Security support until |
|---|---|---|
| 0.1.x | *not yet released* | *release date + 5 years* |

The table is filled in at release. The end date must also appear, as a month and
year, in `README.md`, in `CHANGELOG.md` against the release heading, and in the
About screen — see
[`ANNEX_II_USER_INFORMATION.md`](ANNEX_II_USER_INFORMATION.md), items A4 and A5.

### Why five years and not longer

Article 13(8) asks for the period to reflect *"the length of time during which
the product is expected to be in use, taking into account, in particular,
reasonable user expectations, the nature of the product, including its intended
purpose"*, and permits weighing the support periods of similar products and of
integrated third-party components.

Weighed here:

- **Reasonable user expectation.** A backup tool is installed and left alone. A
  user who set it up in 2026 expects it to still be working in 2031 without
  having thought about it since. Five years is the low end of that expectation,
  not the high end.
- **The nature of the product.** It holds the keys that make the user's backups
  recoverable. A tool in that position should not be dropped quickly.
- **Similar products.** Commercial backup software commonly supports a major
  version for three to five years.
- **Third-party components.** Kopia's own support horizon is not published as a
  fixed period, and superbackup depends on it. This is one of the Article 13(8)
  factors and it argues for caution rather than generosity.
- **The maintainer.** One person, unfunded. This is the factor that stops the
  number being ten.

Five years is what can honestly be committed to. Anything longer would be a
number chosen to look good in a document.

### During 0.x

While the version is below 1.0, **only the latest release receives security
fixes** — that is what [`SECURITY.md`](../../../SECURITY.md) says, and it is not
softened here. A 0.x release is superseded rather than maintained in parallel.
The five-year period becomes meaningful at 1.0, when the release line stabilises
and back-porting to a maintained branch becomes possible.

---

## What support means

**In scope**, and what is actually promised:

| | |
|---|---|
| **Security fixes** | Vulnerabilities in scope per `SECURITY.md` are triaged and fixed. Triage classes and target times are in [`ANNEX_I_PART_II.md`](ANNEX_I_PART_II.md) point (2) |
| **Coordinated disclosure handling** | Acknowledgement within 5 working days, assessment within 10, per `SECURITY.md` |
| **Dependency vulnerability remediation** | `cargo deny` fails CI on a RustSec advisory; the remediation policy is in `ANNEX_I_PART_II.md` point (2) |
| **Kopia floor maintenance** | Raising `MINIMUM_KOPIA_VERSION` in response to a Kopia security issue |
| **Advisories** | A GitHub Security Advisory for every fixed vulnerability, with description, affected versions, impact, severity and remediation |
| **Compatibility with supported platforms** | Keeping the product working on the operating systems it targets, within reason |
| **Public documentation** | Kept accurate; that is a defect class in this project, not a nicety |

**Out of scope**, stated so nobody is surprised:

- **No service level agreement.** Response targets are targets. `SECURITY.md`
  already says timelines depend on maintainer availability, and that sentence is
  the honest version of an SLA.
- **No helpdesk, no email support, no ticket queue.** Questions go to GitHub
  issues and are answered when they are answered.
- **No bug bounty.**
- **No guaranteed feature work, and no guaranteed non-security bug fixes.** A
  non-security bug is fixed when someone fixes it, and "someone" may be you.
- **No support for modified builds**, forks, or a superbackup driving a Kopia
  below the documented minimum version.
- **No support for the destinations themselves.** A StorJ outage, an S3
  permissions problem or a full disk is between the user and their provider.
- **No support for Kopia.** Kopia bugs go to
  [the Kopia project](https://github.com/kopia/kopia/security);
  `SECURITY.md` says so.
- **Nothing is promised about data recovery.** There is no passphrase recovery,
  by design, and no amount of support changes that.

---

## Security updates

**Free of charge, to everyone, always.** superbackup is MIT-licensed with no
paid tier and no support contract. Annex I Part II point (8) requires security
updates to be free of charge; here it is not a compliance measure but the only
possible arrangement.

**Delivery.** Currently: build from source, or take a release from GitHub. There
is no signed release pipeline, no package-manager presence and no update
mechanism — that is the largest open gap in this package, recorded as R-17 in
[`RISK_ASSESSMENT.md`](RISK_ASSESSMENT.md) and at the top of
[`CONFORMITY_CHECKLIST.md`](CONFORMITY_CHECKLIST.md). Until it is closed, "how
do I get a security update" has an unsatisfying answer, and this document says
so rather than describing an intended pipeline in the present tense.

**Separation from feature updates.** From 1.0, security fixes ship as
semantic-versioned patch releases cut from the release branch containing nothing
but the fix, so a user can take a security update without taking a feature.
During 0.x that is not possible and a security fix ships in whatever release
comes next. Annex I Part II point (2) requires separation only *"where
technically feasible"*, and this states plainly when it becomes feasible.

**Availability of past updates.** Article 13(9) requires each security update to
remain available for at least 10 years after it was issued, or the remainder of
the support period, whichever is longer. Git history and GitHub release assets
are the mechanism. If the repository ever moves, the tags and release assets
move with it; if the project is archived, GitHub retains both.

---

## End of life

**Notice period: at least 6 months.** When a release line is going out of
support, or the project as a whole is, the announcement goes out at least six
months before the end date, through:

1. A `CHANGELOG.md` entry.
2. A note at the top of `README.md`.
3. A GitHub release or repository announcement.
4. An in-product notice, once the About screen carries the support end date —
   [`ANNEX_II_USER_INFORMATION.md`](ANNEX_II_USER_INFORMATION.md) item A5. This
   needs no network: the end date is a build-time constant.

**What "end of support" means.** No further security fixes for that line. The
software keeps working — it is a local tool with no licence server and nothing
that expires — and the source remains available under the MIT licence. Backups
already taken remain readable by Kopia independently of superbackup, which is
a deliberate property of driving a separate, standard tool rather than
implementing a proprietary format.

**If the project is abandoned.** The realistic failure mode for a
single-maintainer project, so it is planned for rather than left implicit
(R-19 in [`RISK_ASSESSMENT.md`](RISK_ASSESSMENT.md)):

- The repository is **archived, not deleted**, so the source, the history, the
  releases, the SBOMs and this documentation remain available.
- The README is updated to say the project is unmaintained, prominently, before
  archiving.
- Any known unfixed security issue is published as an advisory at that point,
  regardless of whether a fix exists. Users are owed the information more than
  the project is owed the silence.
- The MIT licence permits anyone to fork and continue. A fork is the succession
  plan; there is no other.

**What users should do on end of life.** Migrate to maintained software, or fork
and maintain. Backups in a Kopia repository stay accessible with the `kopia` CLI
directly — `kopia repository connect` with the repository passphrase — so
leaving superbackup does not mean leaving the data. Folder mirrors are plain
files and need no tool at all. This is worth stating: the exit path is
deliberate, not incidental.

---

## Article 13(23) — cessation of operations

Article 13(23) requires a manufacturer ceasing operations to inform the relevant
market surveillance authorities and, by any means available and to the extent
possible, the users of the affected products, before the cessation takes effect.

This project is not a manufacturer and has no such duty. The end-of-life
procedure above is written to satisfy the substance of it anyway: users are told
in advance, through every channel the project has.

---

## Review

Reviewed at each minor release, whenever the maintainer's capacity to support
materially changes, and whenever the applicability analysis in
[`README.md`](README.md) changes — a commercial offering would require the
support period to be reconsidered as a legal commitment rather than a voluntary
one.

| Version | Date | Change |
|---|---|---|
| 1 | 2026-08-31 | Initial policy for 0.1.0. |
