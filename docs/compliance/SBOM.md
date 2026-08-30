# Software Bill of Materials

Version 1, for superbackup 0.1.x. Last reviewed 2026-08-31.

superbackup publishes a CycloneDX 1.5 bill of materials for every release, in
JSON and XML, covering the full transitive dependency graph across all five
supported target triples, plus the runtime dependencies that are not cargo
crates at all.

The artefacts live in [`sbom/`](../../sbom/). The generator is
[`sbom/generate.py`](../../sbom/generate.py). Neither file is ever hand-edited:
an SBOM written by a person is a document about what someone believed was in
the product.

---

## Why there is one

Two reasons, in this order.

**The engineering reason.** superbackup holds every repository encryption key
its user owns. When a crate in its tree gets a CVE, the question "are we
affected, in which release, on which platform" has to be answerable in minutes
and without guessing. `deny.toml` already fails CI on a RustSec advisory; the
SBOM is what lets someone who is *not* running the build answer the same
question about a binary they downloaded.

**The regulatory reason.** Regulation (EU) 2024/2847 (the Cyber Resilience Act),
Annex I, Part II, point (1) requires manufacturers to *"identify and document
vulnerabilities and components contained in products with digital elements,
including by drawing up a software bill of materials in a commonly used and
machine-readable format covering at the very least the top-level dependencies
of the products"*. Top-level is the floor, and a floor is not a target. This
SBOM covers the whole graph.

Whether that obligation currently binds this project at all is a separate
question, answered in [`cra/README.md`](cra/README.md). The short version: as
it stands today it does not, and the SBOM exists anyway.

---

## What it covers

| | |
|---|---|
| **Format** | CycloneDX 1.5 (`bomFormat: CycloneDX`, `specVersion: 1.5`), JSON and XML |
| **Scope** | The `superbackup` binary and the `superbackup-core` library, and every crate either transitively depends on, including build dependencies |
| **Targets** | `x86_64-pc-windows-msvc`, `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-apple-darwin`, `aarch64-apple-darwin` — merged, with each component tagged with the targets it applies to |
| **Features** | `--all-features`, so the graph is a superset of any real build |
| **Per component** | name, version, PURL, licence expression, author or publisher, description, SHA-256 of the published `.crate` archive, and upstream VCS URL where the crate declares one |
| **Graph** | A `dependencies` record per component, so the tree can be walked — not a flat list |
| **Non-cargo runtime dependencies** | Kopia, and the Linux system libraries linked dynamically at run time |

The five target triples are exactly the five in [`deny.toml`](../../deny.toml).
That is deliberate: an SBOM that reported licences for a target the licence
policy never checks would be reporting numbers nobody has audited.

### Kopia

Kopia is a **separate executable**, not a crate. superbackup does not link it,
contains none of its source, and resolves it at run time — from an explicit
setting, from the copy superbackup manages under its own data directory, or
from `PATH`. `cargo-cyclonedx` is structurally incapable of seeing it.

It appears in the SBOM as `pkg:github/kopia/kopia`, type `application`,
licence `Apache-2.0`, scope `required`, with a dependency edge from the
application component. It carries **no version**, because no exact version is
bound at build time; instead it carries properties recording the minimum
version the driver will accept (0.17.0), where the version is resolved
(`crates/core/src/kopia/binary.rs`), and the pinned upstream repository.

A consumer who needs to know which Kopia a given installation is actually
running can ask it: `superbackup doctor --json` reports the resolved path and
version. That is a property of an installation, not of a build, and it is
correct that a build-time SBOM does not claim to know it.

### Linux system libraries

GTK 3, `libayatana-appindicator3`, `libdbus-1` and `libxdo` are linked
dynamically on Linux and supplied by the distribution. The `-sys` crates that
bind them are in the cargo graph; the libraries themselves are not. They are
declared with `scope: optional` and a `superbackup:platform=linux` property.

---

## What it deliberately does not cover

Stated so the boundary is honest rather than implied.

- **The operating system.** Windows system DLLs, macOS frameworks, and libc are
  not components superbackup ships or chooses. The `windows` crate's bindings
  are in the SBOM; `kernel32.dll` is not.
- **The Rust toolchain.** The compiler and standard library that produced a
  given binary are recorded by the release build, not by this SBOM. There is no
  `metadata.component` entry for `rustc`. This is a gap; see
  [`cra/CONFORMITY_CHECKLIST.md`](cra/CONFORMITY_CHECKLIST.md).
- **Kopia's own dependency tree.** Kopia is a Go program with its own graph.
  superbackup's SBOM identifies Kopia as a component; it does not restate
  Kopia's SBOM. A consumer who needs that should take it from the Kopia
  release. superbackup's position on inheriting Kopia's security posture is in
  [`THREAT_MODEL.md`](THREAT_MODEL.md) §7.
- **What the user backs up.** Obvious, but worth saying in a document about a
  backup tool: the SBOM describes the program, not its inputs.
- **Runtime-loaded plugins.** There are none. superbackup loads no plugins, no
  scripts other than the user's own configured hooks, and no dynamic code.
- **Feature-accurate builds.** The SBOM is generated with `--all-features`, so
  it is a superset. A release build with fewer features contains a subset of
  what the SBOM lists. It over-reports rather than under-reports, which is the
  correct direction for a security artefact.

---

## How to consume it

### Point a scanner at it

```bash
grype sbom:sbom/superbackup-0.1.0.cdx.json
trivy sbom sbom/superbackup-0.1.0.cdx.json
```

CI does this on every push to `main` and on release, in
[`.github/workflows/sbom.yml`](../../.github/workflows/sbom.yml), and fails on a
high or critical finding. That is a second opinion, not a replacement for
`cargo deny check advisories` in `ci.yml`: `cargo-deny` reads the RustSec
database from `Cargo.lock` directly, the scanner reads the SBOM. If the two
disagree, the SBOM is wrong, and that is exactly the thing worth finding out.

### Ask whether a specific crate is in your build

```bash
jq -r '.components[]
       | select(.name == "openssl")
       | "\(.purl)  \(.properties[]|select(.name=="superbackup:targets").value)"' \
   sbom/superbackup-0.1.0.cdx.json
```

The `superbackup:targets` property is the one most people want: it says whether
a component is in every build or only the Windows one.

### Get the licence inventory

```bash
jq -r '.components[] | "\(.licenses[0].expression // "UNKNOWN")\t\(.name)@\(.version)"' \
   sbom/superbackup-0.1.0.cdx.json | sort | uniq -c | sort -rn
```

This is the same information `cargo deny check licenses` enforces, in a form a
lawyer can read. The two are generated from the same lockfile.

---

## Verifying provenance

An SBOM is only worth what its provenance is worth. Three levels, in increasing
order of strength:

**1. The SBOM matches the lockfile in the repository.**

```bash
python sbom/generate.py --check
```

Regenerates in memory and reports exactly which components differ. CI runs this
on every push that touches `Cargo.lock`, `Cargo.toml`, any crate manifest, or
`sbom/`, and fails the build if it drifts.

**2. The SBOM was produced by this project's CI, from this commit.**

Every release runs `actions/attest-build-provenance`, which produces a signed
SLSA provenance statement recorded in a public transparency log:

```bash
gh attestation verify superbackup-0.1.0.cdx.json --repo andreaswiren/superbackup
```

This tells you the file was produced by the named workflow at a named commit in
the named repository, and that nobody has altered it since. It does not tell
you the source itself is trustworthy — nothing can.

**3. The binary you downloaded is the one this SBOM describes.**

Where a release carries binary assets, the same workflow runs
`actions/attest-sbom` to bind this SBOM to those exact files:

```bash
gh attestation verify superbackup.exe --repo andreaswiren/superbackup
```

**Current status: this last step does not yet do anything, because there is no
release build workflow.** `ci.yml` builds and uploads per-run artefacts but does
not publish signed release binaries, so today a release carries the SBOM and
its own provenance but no attested executable. The SBOM workflow detects this
and emits a warning rather than pretending otherwise. Closing it is the highest
priority item on the gap list in
[`cra/CONFORMITY_CHECKLIST.md`](cra/CONFORMITY_CHECKLIST.md).

### Crate-level integrity

Each crates.io component carries the SHA-256 that `Cargo.lock` records for the
published `.crate` archive. That chain is: `Cargo.lock` pins a hash → cargo
refuses a package whose hash differs → the SBOM republishes the same hash. It
lets a third party confirm the SBOM describes the same dependency versions
their own `cargo vendor` produced, without trusting this repository's copy of
`Cargo.lock`.

---

## Reproducibility

Regenerating from the same `Cargo.lock` with the same `cargo-cyclonedx` gives
byte-identical output. The timestamp comes from `SOURCE_DATE_EPOCH` or the
commit date of `HEAD`; the serial number is a UUIDv5 over the BOM's own content
rather than a random UUID; components and dependency records are sorted; and
absolute build-machine paths are rewritten out, so the document does not record
whose laptop produced it.

This matters for more than tidiness. A non-reproducible SBOM cannot be
diffed, which means a reviewer cannot tell a dependency change from a
regeneration, which means nobody reviews it.

---

## Schema validation

Both outputs are validated against the official CycloneDX specification
schemas — `bom-1.5.schema.json` and `bom-1.5.xsd`, fetched from
`github.com/CycloneDX/specification` — on every CI run, and the build fails on
any schema error. A malformed SBOM is worse than none, because it will be
trusted by the tool that fails to parse half of it.

```bash
pip install jsonschema referencing lxml
python sbom/generate.py --validate
```

The committed 0.1.0 SBOM validates clean against both.

---

## Change policy

The SBOM is regenerated whenever `Cargo.lock` changes; CI enforces it. The
generator's `EXTERNAL_COMPONENTS` list is reviewed whenever a new subprocess,
dynamic library, or downloaded artefact is introduced — that is a review item
in [`THREAT_MODEL.md`](THREAT_MODEL.md) §9 as well, because a new external
component is always also a new trust relationship.
