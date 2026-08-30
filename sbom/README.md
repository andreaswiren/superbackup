# Generated SBOM artefacts

These files are **generated**. Do not edit them by hand — edit
[`generate.py`](generate.py) and regenerate.

| File | What it is |
|---|---|
| `superbackup-0.1.0.cdx.json` | The bill of materials, CycloneDX 1.5, JSON. This is the canonical artefact: it is what CI validates, what is attached to releases, and what a vulnerability scanner should be pointed at. |
| `superbackup-0.1.0.cdx.xml` | The same document, CycloneDX 1.5, XML. Byte-for-byte equivalent in content; provided because some procurement and asset-management tooling accepts only XML. |
| `generate.py` | The generator. The only supported way to produce the two files above. |

The filename carries the workspace version, so `sbom/` accumulates one pair of
files per release rather than overwriting history.

## What is in it

- The **full transitive cargo dependency graph**, not just direct dependencies:
  every crate, including build dependencies, with name, version, licence
  expression, PURL, author, description, and the SHA-256 of the published
  `.crate` file as recorded in `Cargo.lock`.
- **Both workspace members.** `superbackup` is the BOM's
  `metadata.component`; `superbackup-core` appears as a component in its own
  right, with the binary target as a subcomponent.
- **Five target triples**, resolved separately and merged: the same five listed
  in [`deny.toml`](../deny.toml). Every component records which of them it was
  resolved for, in a `superbackup:targets` property. A crate marked
  `x86_64-pc-windows-msvc` is not in the Linux binary.
- **Runtime dependencies cargo cannot see**, declared explicitly in
  `EXTERNAL_COMPONENTS` in `generate.py`: `kopia` (a separate executable, not a
  crate) and the Linux system libraries the GUI and tray link dynamically.

What it deliberately leaves out, and why, is in
[`docs/compliance/SBOM.md`](../docs/compliance/SBOM.md).

## Regenerating

Requires `cargo-cyclonedx` and Python 3.9+:

```bash
cargo install cargo-cyclonedx --locked
python sbom/generate.py
```

The generator runs `cargo cyclonedx` once per target, merges the results, adds
the external components, and normalises the output. It writes temporary files
next to each crate's `Cargo.toml` — that is the only place cargo-cyclonedx will
write — and removes them again, including if it fails part-way.

To also check the result against the published schemas:

```bash
pip install jsonschema referencing lxml
python sbom/generate.py --validate
```

`--validate` downloads `bom-1.5.schema.json` and `bom-1.5.xsd` from the
CycloneDX specification repository and validates both outputs against them. It
exits non-zero on any schema error.

## Verifying

**That the SBOM matches the lockfile.** This is what CI enforces on every push:

```bash
python sbom/generate.py --check
```

`--check` regenerates in memory and compares the *content* — the set of
component identifiers, the dependency edges, and the product version — against
the committed file, then prints exactly which components were added or removed.
It deliberately does not compare bytes: a different cargo-cyclonedx build may
reword incidental metadata, and only a real change to what ships should fail a
build.

**That a crate in the SBOM is the crate you have.** Each crates.io component
carries the SHA-256 that `Cargo.lock` records for that package:

```bash
python - <<'EOF'
import json
bom = json.load(open("sbom/superbackup-0.1.0.cdx.json"))
for c in bom["components"]:
    for h in c.get("hashes", []):
        print(h["alg"], h["content"], c["purl"])
EOF
```

Compare against the `checksum` field for the same package in `Cargo.lock`, or
against `sha256sum ~/.cargo/registry/cache/*/<crate>-<version>.crate`.

**That the SBOM is the one the project published.** Release assets are attached
by [`.github/workflows/sbom.yml`](../.github/workflows/sbom.yml), which also
emits a build attestation. See "Verifying provenance" in
[`docs/compliance/SBOM.md`](../docs/compliance/SBOM.md).

**That the document is well formed.** Any CycloneDX-aware tool will do:

```bash
cyclonedx-cli validate --input-file sbom/superbackup-0.1.0.cdx.json
```

## Reproducibility

Two runs over the same `Cargo.lock` with the same `cargo-cyclonedx` produce the
same bytes:

- the timestamp comes from `SOURCE_DATE_EPOCH`, or failing that from the commit
  date of `HEAD`, never from the wall clock;
- the serial number is a UUIDv5 derived from the BOM's own component set and
  dependency edges, not a random UUID;
- components and dependency records are emitted in sorted order;
- absolute build-machine paths in `bom-ref` and `purl` values are rewritten to
  workspace-relative form, so the SBOM does not record where it was built.
