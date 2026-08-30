#!/usr/bin/env python3
"""Generate the superbackup CycloneDX 1.5 software bill of materials.

The SBOM is generated, never edited by hand. This script is the only supported
way to produce the files in `sbom/`, and CI runs it with `--check` on every
push so a lockfile change that is not reflected in the committed SBOM fails the
build.

What it does
------------

1. Runs `cargo cyclonedx --spec-version 1.5 --all --all-features` once per
   supported target triple. cargo-cyclonedx resolves one target at a time, and
   superbackup's dependency graph genuinely differs between them: `windows` and
   `windows-service` exist only on Windows, the Ayatana/GTK stack only on
   Linux, `objc2` only on Apple. The five triples are exactly the five in
   `deny.toml`, so the SBOM and the licence/advisory policy cover the same
   ground.

2. Merges those runs into one workspace BOM. The `superbackup` binary crate's
   BOM is a strict superset of `superbackup-core`'s, so the app BOM is the one
   kept; `superbackup-core` appears inside it as a component in its own right.
   Each component records the target triples it was resolved for, in a
   `superbackup:targets` property.

3. Adds the runtime dependencies cargo cannot see. Kopia is a separate
   executable resolved at run time, not a crate, so `cargo-cyclonedx` is
   structurally incapable of finding it; the same is true of the Linux system
   libraries the GUI and tray link against dynamically. They are declared in
   `EXTERNAL_COMPONENTS` below, in one place, reviewable in diff.

4. Normalises the output so it is reproducible: absolute build-machine paths
   are rewritten to workspace-relative refs, the timestamp comes from the git
   commit date rather than the clock, and the serial number is a UUIDv5 derived
   from the BOM's own content. Regenerating from the same `Cargo.lock` with the
   same cargo-cyclonedx produces the same bytes.

5. Emits JSON and XML, and validates both against the official CycloneDX 1.5
   schemas when `jsonschema` and `lxml` are installed (`--validate`).

Usage
-----

    python sbom/generate.py                 # regenerate sbom/*.cdx.{json,xml}
    python sbom/generate.py --validate      # and validate against the schemas
    python sbom/generate.py --check         # fail if the committed SBOM is stale

`--check` compares the *content* of the BOM — the set of component PURLs, the
dependency edges, and the product version — rather than raw bytes. A different
cargo-cyclonedx build or a different rustc may reorder or re-word incidental
metadata; only a real change to what is shipped should fail CI.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import uuid
import xml.etree.ElementTree as ET
from datetime import datetime, timezone
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
OUT_DIR = REPO_ROOT / "sbom"

CDX_SPEC = "1.5"
CDX_NS = "http://cyclonedx.org/schema/bom/1.5"

# Exactly the targets policed by `deny.toml`. Keep the two lists in step: an
# SBOM that covers targets the licence policy does not is an SBOM that reports
# licences nobody has checked.
TARGETS = [
    "x86_64-pc-windows-msvc",
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
]

# The workspace member whose BOM is the superset. `superbackup` depends on
# `superbackup-core`, so its graph contains everything core's does.
ROOT_PACKAGE = "superbackup"

SUPPLIER = {
    "name": "Andreas Wiren (superbackup project)",
    "url": ["https://github.com/andreaswiren/superbackup"],
}

# --------------------------------------------------------------------------
# Runtime dependencies that are not cargo dependencies.
#
# Everything here is a real, shipped or required artefact that `cargo` has no
# knowledge of. Adding something to this list is a claim; each entry states
# where the claim is checkable in the repository.
# --------------------------------------------------------------------------

EXTERNAL_COMPONENTS = [
    {
        "type": "application",
        "bom-ref": "external:kopia",
        "publisher": "Jarek Kowalski and the Kopia contributors",
        "name": "kopia",
        "description": (
            "The backup engine. superbackup implements no deduplication, chunking or "
            "backup encryption; it drives the kopia command-line executable as a "
            "subprocess. Not linked and not a cargo dependency: kopia is resolved at "
            "run time from an explicit setting, from superbackup's managed copy, or "
            "from PATH, and its version is probed and floor-checked before any "
            "repository is touched (crates/core/src/kopia/binary.rs). No exact "
            "version is bound at build time, so this component deliberately carries "
            "no version."
        ),
        "scope": "required",
        "licenses": [{"expression": "Apache-2.0"}],
        "purl": "pkg:github/kopia/kopia",
        "externalReferences": [
            {"type": "website", "url": "https://kopia.io"},
            {"type": "vcs", "url": "https://github.com/kopia/kopia"},
            {"type": "distribution", "url": "https://github.com/kopia/kopia/releases"},
            {
                "type": "security-contact",
                "url": "https://github.com/kopia/kopia/security",
            },
        ],
        "properties": [
            {"name": "superbackup:relationship", "value": "separate executable, invoked as a subprocess"},
            {"name": "superbackup:linkage", "value": "none (no source, no static or dynamic linking)"},
            {"name": "superbackup:minimum-version", "value": "0.17.0"},
            {"name": "superbackup:version-resolution", "value": "runtime; see crates/core/src/kopia/binary.rs"},
            {"name": "superbackup:upstream-repo", "value": "kopia/kopia"},
            {"name": "superbackup:licence-obligations", "value": "docs/compliance/THIRD_PARTY.md"},
        ],
    },
    {
        "type": "library",
        "bom-ref": "external:gtk3",
        "name": "GTK 3",
        "description": (
            "Linux only. Dynamically linked at run time by the window, file-dialog "
            "and tray stack (eframe, rfd, tray-icon). Provided by the distribution, "
            "not shipped by superbackup; the -sys crates that bind it appear in the "
            "cargo graph, the library itself does not."
        ),
        "scope": "optional",
        "licenses": [{"expression": "LGPL-2.1-or-later"}],
        "purl": "pkg:generic/gtk3",
        "externalReferences": [{"type": "website", "url": "https://www.gtk.org/"}],
        "properties": [
            {"name": "superbackup:platform", "value": "linux"},
            {"name": "superbackup:linkage", "value": "dynamic, provided by the operating system"},
        ],
    },
    {
        "type": "library",
        "bom-ref": "external:libayatana-appindicator3",
        "name": "libayatana-appindicator3",
        "description": (
            "Linux only. Dynamically linked at run time by tray-icon to publish the "
            "status icon. Provided by the distribution; installed in CI by the "
            "Linux job in .github/workflows/ci.yml."
        ),
        "scope": "optional",
        "licenses": [{"expression": "LGPL-3.0-only OR LGPL-2.1-only"}],
        "purl": "pkg:generic/libayatana-appindicator3",
        "externalReferences": [
            {"type": "vcs", "url": "https://github.com/AyatanaIndicators/libayatana-appindicator"}
        ],
        "properties": [
            {"name": "superbackup:platform", "value": "linux"},
            {"name": "superbackup:linkage", "value": "dynamic, provided by the operating system"},
        ],
    },
    {
        "type": "library",
        "bom-ref": "external:libdbus-1",
        "name": "libdbus-1",
        "description": (
            "Linux only. Dynamically linked at run time for desktop notifications "
            "(notify-rust) and for Secret Service access when the optional OS "
            "keychain setting is enabled (keyring)."
        ),
        "scope": "optional",
        "licenses": [{"expression": "AFL-2.1 OR GPL-2.0-or-later"}],
        "purl": "pkg:generic/dbus",
        "externalReferences": [{"type": "website", "url": "https://www.freedesktop.org/wiki/Software/dbus/"}],
        "properties": [
            {"name": "superbackup:platform", "value": "linux"},
            {"name": "superbackup:linkage", "value": "dynamic, provided by the operating system"},
        ],
    },
    {
        "type": "library",
        "bom-ref": "external:libxdo",
        "name": "libxdo (xdotool)",
        "description": (
            "Linux only. Dynamically linked at run time through the tray/window "
            "stack for X11 input synthesis. Provided by the distribution."
        ),
        "scope": "optional",
        "licenses": [{"expression": "BSD-3-Clause"}],
        "purl": "pkg:generic/xdotool",
        "externalReferences": [{"type": "vcs", "url": "https://github.com/jordansissel/xdotool"}],
        "properties": [
            {"name": "superbackup:platform", "value": "linux"},
            {"name": "superbackup:linkage", "value": "dynamic, provided by the operating system"},
        ],
    },
]

# Dependency edges from the application to the external components above.
EXTERNAL_REFS = [c["bom-ref"] for c in EXTERNAL_COMPONENTS]


# --------------------------------------------------------------------------
# Generation
# --------------------------------------------------------------------------


def run(cmd: list[str], cwd: Path | None = None) -> str:
    proc = subprocess.run(
        cmd, cwd=cwd, capture_output=True, text=True, encoding="utf-8", errors="replace"
    )
    if proc.returncode != 0:
        sys.stderr.write(proc.stdout or "")
        sys.stderr.write(proc.stderr or "")
        raise SystemExit(f"command failed ({proc.returncode}): {' '.join(cmd)}")
    return proc.stdout


def source_date_epoch(workspace: Path) -> int:
    """A build-independent timestamp.

    SOURCE_DATE_EPOCH wins if set. Otherwise the commit date of HEAD, which
    makes regeneration on any machine at any time produce the same BOM. Falls
    back to zero rather than to the wall clock, because a wall clock defeats
    the point.
    """
    env = os.environ.get("SOURCE_DATE_EPOCH")
    if env:
        return int(env)
    try:
        out = subprocess.run(
            ["git", "-C", str(workspace), "log", "-1", "--format=%ct"],
            capture_output=True,
            text=True,
        )
        if out.returncode == 0 and out.stdout.strip():
            return int(out.stdout.strip())
    except OSError:
        pass
    return 0


def workspace_version(workspace: Path) -> str:
    text = (workspace / "Cargo.toml").read_text(encoding="utf-8")
    m = re.search(r"^\s*version\s*=\s*\"([^\"]+)\"", text, re.M)
    if not m:
        raise SystemExit("could not read [workspace.package] version from Cargo.toml")
    return m.group(1)


def rustc_identity() -> tuple[str, str]:
    """The rustc that resolved this SBOM: version string and host triple.

    This is the toolchain that ran `cargo metadata`, which is not necessarily
    the toolchain that compiled any particular release binary. It is recorded
    with that scope stated, because a consumer asking "was this built with a
    compiler carrying a known bug" deserves an answer that does not overclaim.
    """
    try:
        out = subprocess.run(["rustc", "-vV"], capture_output=True, text=True)
        if out.returncode != 0:
            return ("unknown", "unknown")
        blob = out.stdout
        version = blob.splitlines()[0].replace("rustc ", "").strip()
        host = next(
            (l.split(":", 1)[1].strip() for l in blob.splitlines() if l.startswith("host:")),
            "unknown",
        )
        return (version or "unknown", host)
    except OSError:
        return ("unknown", "unknown")


def cyclonedx_version(cargo: str) -> str:
    out = subprocess.run(
        [cargo, "cyclonedx", "--version"], capture_output=True, text=True
    )
    blob = (out.stdout or "") + (out.stderr or "")
    m = re.search(r"(\d+\.\d+\.\d+)", blob)
    return m.group(1) if m else "unknown"


def generate_for_target(cargo: str, workspace: Path, target: str, epoch: int) -> dict:
    """Run cargo-cyclonedx for one target and return the parsed app BOM.

    cargo-cyclonedx writes its output next to each workspace member's
    Cargo.toml and offers no way to redirect it. The files are read and then
    removed, so the working tree is left as it was found.
    """
    env = dict(os.environ, SOURCE_DATE_EPOCH=str(epoch))
    stem = f"sbom-tmp-{target}"

    def produced() -> list[Path]:
        # With --override-filename, cargo-cyclonedx drops the `.cdx` infix.
        return sorted(workspace.glob(f"crates/*/{stem}.json")) + sorted(
            workspace.glob(f"crates/*/{stem}.cdx.json")
        )

    try:
        subprocess.run(
            [
                cargo,
                "cyclonedx",
                "--manifest-path",
                str(workspace / "Cargo.toml"),
                "--format",
                "json",
                "--spec-version",
                CDX_SPEC,
                "--all",
                "--all-features",
                "--target",
                target,
                "--override-filename",
                stem,
                "-q",
            ],
            env=env,
            check=True,
        )
        files = produced()
        if not files:
            raise SystemExit(f"cargo-cyclonedx produced no output for {target}")
        app = next((p for p in files if p.parent.name == "app"), None)
        if app is None:
            raise SystemExit(f"no BOM for the {ROOT_PACKAGE} crate for {target}")
        return json.loads(app.read_text(encoding="utf-8"))
    finally:
        # Leave the working tree exactly as it was found, including on failure.
        for p in produced():
            p.unlink(missing_ok=True)


# --------------------------------------------------------------------------
# Normalisation
# --------------------------------------------------------------------------

_PATH_REF = re.compile(r"path\+file://[^#\s]*?/crates/([A-Za-z0-9_.-]+)#")
_DOWNLOAD_QUALIFIER = re.compile(r"\?download_url=file://[^#\s\"]*")


def normalise_ref(ref: str) -> str:
    """Rewrite build-machine absolute paths into stable workspace refs."""
    return _PATH_REF.sub(lambda m: f"path+workspace:crates/{m.group(1)}#", ref)


def normalise_purl(purl: str) -> str:
    """Drop the `download_url=file://...` qualifier from workspace crates."""
    return _DOWNLOAD_QUALIFIER.sub("", purl)


def normalise_component(c: dict) -> dict:
    if "bom-ref" in c:
        c["bom-ref"] = normalise_ref(c["bom-ref"])
    if "purl" in c:
        c["purl"] = normalise_purl(c["purl"])
    for sub in c.get("components", []):
        normalise_component(sub)
    return c


def component_key(c: dict) -> str:
    return c.get("bom-ref") or c.get("purl") or f"{c.get('name')}@{c.get('version')}"


# --------------------------------------------------------------------------
# Merge
# --------------------------------------------------------------------------


def merge(
    boms: dict[str, dict],
    version: str,
    epoch: int,
    tool_version: str,
    rustc: tuple[str, str] = ("unknown", "unknown"),
) -> dict:
    components: dict[str, dict] = {}
    comp_targets: dict[str, set[str]] = {}
    deps: dict[str, set[str]] = {}
    root_component: dict | None = None
    root_ref: str | None = None

    for target, bom in boms.items():
        meta_component = normalise_component(bom["metadata"]["component"])
        if root_component is None:
            root_component = meta_component
            root_ref = meta_component["bom-ref"]

        for c in bom.get("components", []):
            c = normalise_component(c)
            key = component_key(c)
            components.setdefault(key, c)
            comp_targets.setdefault(key, set()).add(target)

        for edge in bom.get("dependencies", []):
            ref = normalise_ref(edge["ref"])
            deps.setdefault(ref, set()).update(
                normalise_ref(d) for d in edge.get("dependsOn", [])
            )

    assert root_component is not None and root_ref is not None

    # Record which targets each dependency was resolved for. This is the
    # question a reader of a cross-platform SBOM actually has: "is this crate
    # in the binary I am running?"
    for key, c in components.items():
        targets = sorted(comp_targets.get(key, ()))
        props = [p for p in c.get("properties", []) if p["name"] != "superbackup:targets"]
        props.append(
            {
                "name": "superbackup:targets",
                "value": "all" if len(targets) == len(TARGETS) else ",".join(targets),
            }
        )
        c["properties"] = props

    for ext in EXTERNAL_COMPONENTS:
        components[ext["bom-ref"]] = json.loads(json.dumps(ext))
        deps.setdefault(ext["bom-ref"], set())
    deps.setdefault(root_ref, set()).update(EXTERNAL_REFS)

    root_component.setdefault("supplier", SUPPLIER)
    root_component["externalReferences"] = [
        {"type": "website", "url": "https://github.com/andreaswiren/superbackup"},
        {"type": "vcs", "url": "https://github.com/andreaswiren/superbackup"},
        {
            "type": "issue-tracker",
            "url": "https://github.com/andreaswiren/superbackup/issues",
        },
        {
            "type": "security-contact",
            "url": "https://github.com/andreaswiren/superbackup/blob/main/SECURITY.md",
        },
        {
            "type": "threat-model",
            "url": "https://github.com/andreaswiren/superbackup/blob/main/docs/compliance/THREAT_MODEL.md",
        },
        {
            "type": "risk-assessment",
            "url": "https://github.com/andreaswiren/superbackup/blob/main/docs/compliance/cra/RISK_ASSESSMENT.md",
        },
        {
            "type": "license",
            "url": "https://github.com/andreaswiren/superbackup/blob/main/LICENSE",
        },
        {
            "type": "documentation",
            "url": "https://github.com/andreaswiren/superbackup/blob/main/docs/compliance/SBOM.md",
        },
    ]

    ordered_components = [components[k] for k in sorted(components)]
    ordered_deps = [
        {"ref": r, "dependsOn": sorted(deps[r])} for r in sorted(deps)
    ]

    timestamp = (
        datetime.fromtimestamp(epoch, tz=timezone.utc)
        .isoformat(timespec="seconds")
        .replace("+00:00", "Z")
    )

    bom = {
        "bomFormat": "CycloneDX",
        "specVersion": CDX_SPEC,
        "version": 1,
        "metadata": {
            "timestamp": timestamp,
            "lifecycles": [{"phase": "build"}],
            "tools": {
                "components": [
                    {
                        "type": "application",
                        "author": "CycloneDX",
                        "name": "cargo-cyclonedx",
                        "version": tool_version,
                        "externalReferences": [
                            {
                                "type": "vcs",
                                "url": "https://github.com/CycloneDX/cyclonedx-rust-cargo",
                            }
                        ],
                    },
                    {
                        "type": "application",
                        "author": "The Rust Project",
                        "name": "rustc",
                        "version": rustc[0],
                        "description": (
                            "The toolchain that resolved this bill of materials. "
                            "Not necessarily the toolchain that compiled any given "
                            "release binary — see docs/compliance/SBOM.md."
                        ),
                        "properties": [{"name": "superbackup:rustc:host", "value": rustc[1]}],
                    },
                    {
                        "type": "application",
                        "author": "superbackup project",
                        "name": "sbom/generate.py",
                        "version": version,
                        "description": (
                            "Merges the per-target cargo-cyclonedx runs, adds the "
                            "runtime dependencies cargo cannot see, and normalises "
                            "the result for reproducibility."
                        ),
                    },
                ]
            },
            "authors": [{"name": "Andreas Wiren"}],
            "component": root_component,
            "supplier": SUPPLIER,
            "licenses": [{"expression": "MIT"}],
            "properties": [
                {"name": "superbackup:sbom:targets", "value": ",".join(TARGETS)},
                {
                    "name": "superbackup:sbom:scope",
                    "value": (
                        "full transitive cargo graph for every target listed above, "
                        "including build dependencies, plus non-cargo runtime "
                        "dependencies declared in sbom/generate.py"
                    ),
                },
                {
                    "name": "superbackup:sbom:generated-by",
                    "value": "sbom/generate.py — generated, never hand-edited",
                },
                {
                    "name": "superbackup:sbom:documentation",
                    "value": "docs/compliance/SBOM.md",
                },
            ],
        },
        "components": ordered_components,
        "dependencies": ordered_deps,
    }

    bom["serialNumber"] = deterministic_serial(bom)
    return bom


def deterministic_serial(bom: dict) -> str:
    """A serial number derived from the BOM's content, not from randomness.

    Two runs over the same lockfile must produce the same document. A random
    UUID would make every regeneration look like a change and would make the
    staleness check in CI useless.
    """
    material = json.dumps(
        {
            "component": bom["metadata"]["component"].get("purl"),
            "version": bom["metadata"]["component"].get("version"),
            "components": sorted(
                component_key(c) for c in bom["components"]
            ),
            "dependencies": [
                [d["ref"], d["dependsOn"]] for d in bom["dependencies"]
            ],
        },
        sort_keys=True,
        separators=(",", ":"),
    )
    return f"urn:uuid:{uuid.uuid5(uuid.NAMESPACE_URL, material)}"


# --------------------------------------------------------------------------
# XML serialisation
#
# The element order below follows the CycloneDX 1.5 XSD sequences. An SBOM that
# does not validate is worse than no SBOM, because it will be trusted, so the
# order is not left to chance and `--validate` checks it against the published
# schema.
# --------------------------------------------------------------------------

_COMPONENT_ORDER = [
    "supplier",
    "author",
    "publisher",
    "group",
    "name",
    "version",
    "description",
    "scope",
    "hashes",
    "licenses",
    "copyright",
    "cpe",
    "purl",
    "externalReferences",
    "properties",
    "components",
]


def _sub(parent: ET.Element, tag: str, text: str | None = None, **attrs) -> ET.Element:
    el = ET.SubElement(parent, tag, {k: v for k, v in attrs.items() if v is not None})
    if text is not None:
        el.text = text
    return el


def _entity_xml(parent: ET.Element, tag: str, entity: dict) -> None:
    el = _sub(parent, tag)
    if "name" in entity:
        _sub(el, "name", entity["name"])
    for url in entity.get("url", []):
        _sub(el, "url", url)


def _licenses_xml(parent: ET.Element, licenses: list[dict]) -> None:
    el = _sub(parent, "licenses")
    for lic in licenses:
        if "expression" in lic:
            _sub(el, "expression", lic["expression"])
        else:
            inner = lic.get("license", {})
            lel = _sub(el, "license")
            if "id" in inner:
                _sub(lel, "id", inner["id"])
            elif "name" in inner:
                _sub(lel, "name", inner["name"])
            if "url" in inner:
                _sub(lel, "url", inner["url"])


def _extrefs_xml(parent: ET.Element, refs: list[dict]) -> None:
    el = _sub(parent, "externalReferences")
    for r in refs:
        rel = _sub(el, "reference", type=r["type"])
        _sub(rel, "url", r["url"])


def _properties_xml(parent: ET.Element, props: list[dict]) -> None:
    el = _sub(parent, "properties")
    for p in props:
        _sub(el, "property", p.get("value", ""), name=p["name"])


def _component_xml(parent: ET.Element, tag: str, c: dict) -> ET.Element:
    attrs = {"type": c["type"]}
    if "bom-ref" in c:
        attrs["bom-ref"] = c["bom-ref"]
    el = ET.SubElement(parent, tag, attrs)
    for field in _COMPONENT_ORDER:
        if field == "supplier" and "supplier" in c:
            _entity_xml(el, "supplier", c["supplier"])
        elif field == "hashes" and c.get("hashes"):
            hel = _sub(el, "hashes")
            for h in c["hashes"]:
                _sub(hel, "hash", h["content"], alg=h["alg"])
        elif field == "licenses" and c.get("licenses"):
            _licenses_xml(el, c["licenses"])
        elif field == "externalReferences" and c.get("externalReferences"):
            _extrefs_xml(el, c["externalReferences"])
        elif field == "properties" and c.get("properties"):
            _properties_xml(el, c["properties"])
        elif field == "components" and c.get("components"):
            sub = _sub(el, "components")
            for child in c["components"]:
                _component_xml(sub, "component", child)
        elif field in (
            "supplier",
            "hashes",
            "licenses",
            "externalReferences",
            "properties",
            "components",
        ):
            continue
        elif field in c:
            _sub(el, field, str(c[field]))
    return el


def to_xml(bom: dict) -> bytes:
    ET.register_namespace("", CDX_NS)
    root = ET.Element(
        f"{{{CDX_NS}}}bom",
        {"serialNumber": bom["serialNumber"], "version": str(bom["version"])},
    )
    meta = bom["metadata"]
    m = _sub(root, "metadata")
    _sub(m, "timestamp", meta["timestamp"])
    lifecycles = _sub(m, "lifecycles")
    for lc in meta["lifecycles"]:
        _sub(_sub(lifecycles, "lifecycle"), "phase", lc["phase"])
    tools = _sub(m, "tools")
    tool_components = _sub(tools, "components")
    for t in meta["tools"]["components"]:
        _component_xml(tool_components, "component", t)
    authors = _sub(m, "authors")
    for a in meta["authors"]:
        _sub(_sub(authors, "author"), "name", a["name"])
    _component_xml(m, "component", meta["component"])
    _entity_xml(m, "supplier", meta["supplier"])
    _licenses_xml(m, meta["licenses"])
    _properties_xml(m, meta["properties"])

    comps = _sub(root, "components")
    for c in bom["components"]:
        _component_xml(comps, "component", c)

    deps = _sub(root, "dependencies")
    for d in bom["dependencies"]:
        del_ = _sub(deps, "dependency", ref=d["ref"])
        for child in d["dependsOn"]:
            _sub(del_, "dependency", ref=child)

    ET.indent(root, space="  ")
    return b'<?xml version="1.0" encoding="UTF-8"?>\n' + ET.tostring(
        root, encoding="utf-8"
    )


# --------------------------------------------------------------------------
# Validation
# --------------------------------------------------------------------------

SCHEMA_BASE = "https://raw.githubusercontent.com/CycloneDX/specification/master/schema"


def validate(json_path: Path, xml_path: Path, schema_dir: Path | None) -> int:
    """Validate both documents against the official CycloneDX 1.5 schemas."""
    failures = 0
    schema_dir = schema_dir or (Path(tempfile.gettempdir()) / "cyclonedx-schema-1.5")
    schema_dir.mkdir(parents=True, exist_ok=True)

    def fetch(name: str) -> Path:
        p = schema_dir / name
        if not p.exists():
            import urllib.request

            urllib.request.urlretrieve(f"{SCHEMA_BASE}/{name}", p)
        return p

    try:
        import jsonschema
        from referencing import Registry, Resource
        from referencing.jsonschema import DRAFT7

        bom_schema = json.loads(fetch("bom-1.5.schema.json").read_text(encoding="utf-8"))

        def retrieve(uri: str) -> Resource:
            # bom-1.5.schema.json refers to spdx.schema.json and
            # jsf-0.82.schema.json by relative name; resolve them to the copies
            # fetched alongside it rather than over the network.
            name = uri.rsplit("/", 1)[-1]
            return Resource(
                contents=json.loads(fetch(name).read_text(encoding="utf-8")),
                specification=DRAFT7,
            )

        registry = Registry(retrieve=retrieve)
        validator = jsonschema.Draft7Validator(bom_schema, registry=registry)
        errors = sorted(
            validator.iter_errors(json.loads(json_path.read_text(encoding="utf-8"))),
            key=lambda e: list(e.path),
        )
        if errors:
            failures += 1
            print(f"JSON: FAILED against bom-1.5.schema.json ({len(errors)} errors)")
            for e in errors[:10]:
                print(f"  /{'/'.join(str(p) for p in e.path)}: {e.message[:200]}")
        else:
            print("JSON: valid against the official CycloneDX 1.5 JSON schema")
    except ImportError:
        print("JSON: SKIPPED (pip install jsonschema referencing)")

    try:
        from lxml import etree

        fetch("spdx.xsd")
        # bom-1.5.xsd imports the SPDX schema by absolute URL. Point it at the
        # copy sitting next to it so validation needs no network and cannot be
        # silently skipped by an offline runner.
        patched = schema_dir / "bom-1.5.local.xsd"
        patched.write_text(
            fetch("bom-1.5.xsd")
            .read_text(encoding="utf-8")
            .replace(
                'schemaLocation="http://cyclonedx.org/schema/spdx"',
                'schemaLocation="spdx.xsd"',
            ),
            encoding="utf-8",
        )
        xsd = etree.XMLSchema(etree.parse(str(patched)))
        doc = etree.parse(str(xml_path))
        if xsd.validate(doc):
            print("XML: valid against the official CycloneDX 1.5 XSD")
        else:
            failures += 1
            print("XML: FAILED against bom-1.5.xsd")
            for e in list(xsd.error_log)[:10]:
                print(f"  line {e.line}: {e.message[:200]}")
    except ImportError:
        print("XML: SKIPPED (pip install lxml)")

    return failures


# --------------------------------------------------------------------------
# Staleness check
# --------------------------------------------------------------------------


def content_fingerprint(bom: dict) -> dict:
    return {
        "version": bom["metadata"]["component"].get("version"),
        "components": sorted(component_key(c) for c in bom["components"]),
        "dependencies": {d["ref"]: sorted(d["dependsOn"]) for d in bom["dependencies"]},
    }


def check(fresh: dict, committed_path: Path) -> int:
    if not committed_path.exists():
        print(f"STALE: {committed_path} does not exist")
        return 1
    old = json.loads(committed_path.read_text(encoding="utf-8"))
    a, b = content_fingerprint(old), content_fingerprint(fresh)
    if a == b:
        print(f"{committed_path.name} is up to date with Cargo.lock")
        return 0

    print(f"STALE: {committed_path.name} does not match the current Cargo.lock")
    if a["version"] != b["version"]:
        print(f"  product version: {a['version']} -> {b['version']}")
    added = sorted(set(b["components"]) - set(a["components"]))
    removed = sorted(set(a["components"]) - set(b["components"]))
    for r in removed[:40]:
        print(f"  - {r}")
    for r in added[:40]:
        print(f"  + {r}")
    if a["dependencies"] != b["dependencies"] and not added and not removed:
        print("  dependency edges changed")
    print("\nRun `python sbom/generate.py` and commit the result.")
    return 1


# --------------------------------------------------------------------------


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--workspace", type=Path, default=REPO_ROOT)
    ap.add_argument("--out", type=Path, default=OUT_DIR)
    ap.add_argument("--cargo", default=shutil.which("cargo") or "cargo")
    ap.add_argument("--check", action="store_true", help="fail if the committed SBOM is stale")
    ap.add_argument("--validate", action="store_true", help="validate against the CycloneDX 1.5 schemas")
    ap.add_argument("--schema-dir", type=Path, default=None)
    args = ap.parse_args()

    workspace = args.workspace.resolve()
    epoch = source_date_epoch(workspace)
    version = workspace_version(workspace)
    tool_version = cyclonedx_version(args.cargo)
    rustc = rustc_identity()

    boms = {}
    for target in TARGETS:
        print(f"resolving {target} ...", file=sys.stderr)
        boms[target] = generate_for_target(args.cargo, workspace, target, epoch)

    bom = merge(boms, version, epoch, tool_version, rustc)

    json_path = args.out / f"superbackup-{version}.cdx.json"
    xml_path = args.out / f"superbackup-{version}.cdx.xml"

    if args.check:
        return check(bom, json_path)

    args.out.mkdir(parents=True, exist_ok=True)
    json_path.write_text(
        json.dumps(bom, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
    )
    xml_path.write_bytes(to_xml(bom) + b"\n")
    print(
        f"wrote {json_path.name} and {xml_path.name}: "
        f"{len(bom['components'])} components, {len(bom['dependencies'])} dependency records"
    )

    if args.validate:
        return 1 if validate(json_path, xml_path, args.schema_dir) else 0
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
