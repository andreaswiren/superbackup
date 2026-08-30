# Third-Party Software and Licence Compliance

superbackup is released under the **MIT License** (see [`LICENSE`](../../LICENSE)).

This document records what it depends on and what each dependency obliges us to
do. Licence compliance is enforced in CI by `cargo-deny` against the policy in
[`deny.toml`](../../deny.toml): a dependency whose licence falls outside the
allowlist fails the build rather than being noticed at release time.

---

## Kopia

**The backup engine.** superbackup does not implement deduplication, chunking,
or backup encryption; Kopia does. superbackup manages repositories, schedules
work, and presents the results.

| | |
|---|---|
| Project | [kopia/kopia](https://github.com/kopia/kopia) — <https://kopia.io> |
| Licence | Apache License 2.0 |
| Copyright | Copyright 2019 Jarek Kowalski and the Kopia contributors |
| Relationship | Separate executable, invoked as a subprocess |

### What this obliges us to do

superbackup invokes `kopia` as a **separate process**. It does not link Kopia's
code, statically or dynamically, and contains no Kopia source. This is
deliberate: it keeps the licence boundary clean and lets a user run whichever
Kopia build they trust.

Apache-2.0 is permissive and compatible with distributing superbackup under
MIT. Our obligations, and how we meet them:

1. **Retain the licence and attribution.** A copy of the Apache 2.0 licence
   ships with any distribution that bundles a Kopia binary, in
   `licenses/kopia/LICENSE`. Attribution appears in the About screen, in
   `superbackup version --json`, and in the README.
2. **State changes.** We make none. We do not patch, fork, or redistribute a
   modified Kopia.
3. **Do not imply endorsement.** superbackup is not affiliated with, endorsed
   by, or supported by the Kopia project. Bugs in superbackup should be
   reported here, not to Kopia. The About screen says so.
4. **NOTICE file.** If a Kopia release includes a NOTICE file, it is reproduced
   alongside the licence in any distribution that bundles the binary.

### If a build bundles Kopia

Some installers ship a pinned Kopia binary so the application works out of the
box. Such a build must include:

```
licenses/
  kopia/
    LICENSE          # Apache 2.0, verbatim
    NOTICE           # if present in the upstream release
    VERSION          # the exact version and its published checksum
```

A build that resolves Kopia from the user's `PATH` and bundles nothing still
carries the attribution, because the About screen and README reference the
project regardless.

---

## Plakar

[PlakarKorp/plakar](https://github.com/PlakarKorp/plakar) is cited in
`design/UX_SPEC.md` as an **inspiration for interface quality only**. No Plakar
code, asset, or trademark is used, and superbackup neither links against nor
invokes Plakar. If the Kloset/ptar direction is ever adopted, this document is
updated first.

## Rclone UI

[rclone-ui/rclone-ui](https://github.com/rclone-ui/rclone-ui) is cited as a
**reference for feature scope only**. No code or assets are used.

## StorJ

StorJ is a supported storage provider reached through its S3-compatible
gateway. superbackup uses no StorJ SDK and includes no StorJ code; it speaks S3
through Kopia. "StorJ" is used nominatively to identify the service.

---

## Rust dependencies

The full transitive tree, with licences, is produced by:

```bash
cargo tree --format "{p} {l}"
```

and audited on every CI run by:

```bash
cargo deny check
```

### Allowed licences

MIT, Apache-2.0 (with or without the LLVM exception), BSD-2-Clause,
BSD-3-Clause, 0BSD, ISC, Zlib, Unicode-3.0, MPL-2.0, CC0-1.0, BSL-1.0, and the
two font licences OFL-1.1 and Ubuntu-font-1.0.

This list is kept in `deny.toml` and is the authoritative copy; this section
mirrors it. If the two disagree, `deny.toml` is right and this is stale.

Notes on the less obvious entries:

- **MPL-2.0** is file-level copyleft: we may link it from an MIT binary, but a
  modified MPL file must stay MPL. We modify no dependency, so this imposes
  nothing beyond retaining notices.
- **CC0-1.0** (`notify`, a direct dependency, and `hexf-parse`) is a
  public-domain dedication. It imposes nothing on redistribution — but it
  explicitly does *not* grant patent rights, which is why some organisations
  decline it for code. That caveat is accepted here deliberately.
- **OFL-1.1 / Ubuntu-font-1.0** cover the bundled fonts; see *Assets* above.

**No exceptions are configured.** A dependency arriving under GPL, AGPL, LGPL,
SSPL, or a proprietary licence fails CI, because relicensing this project is a
decision to be taken deliberately and not discovered during a release. The one
LGPL interaction — GTK on Linux — is a dynamically linked system library rather
than a cargo dependency, and is documented under *Assets*.

### Advisory exceptions

`cargo deny check advisories` fails the build on a known vulnerability or an
unmaintained crate. Thirteen advisories are currently accepted, each with a
written justification and a stated condition that would revoke it. They are
listed in full in [`deny.toml`](../../deny.toml); the summary:

| What | Why accepted |
|---|---|
| `quick-xml` 0.30 — two DoS advisories | Reached only through Linux accessibility (`accesskit_unix` → `atspi` → `zbus_xml`). The XML is D-Bus introspection data from the session's own accessibility bus, not untrusted input. quick-xml 0.31 is a semver break that `zbus_xml` 4 does not accept, so there is no upgrade path. |
| gtk-rs GTK3 bindings (7 crates) — unmaintained | The whole Linux tray and file-dialog ecosystem sits on GTK3 bindings, which upstream retired in favour of GTK4 that `tray-icon` has not adopted. Unmaintained is not vulnerable, and no alternative keeps a working Linux tray. |
| `proc-macro-error`, `paste` — unmaintained | Build-time macro helpers. Neither is present in the shipped binary. |
| `rustybuzz`, `ttf-parser` — unmaintained | Text shaping via `resvg`, used only to rasterise tray icons from SVG assets checked into this repository. Never processes user-supplied input. |

Two properties of this list are deliberate. **None of these is reachable on
Windows**, the priority platform — they are all in the Linux GUI stack. And
**none is reachable at all** in `superbackup daemon --no-tray` or in the service
host, which is the configuration that runs unattended and unsupervised.

The exposure that would change these answers is stated in `deny.toml` next to
each entry. The one worth repeating: if superbackup ever parses XML that came
from a network or from a user-supplied file, the `quick-xml` exceptions stop
being acceptable that day.

### Notable direct dependencies

| Crate | Licence | Used for |
|---|---|---|
| `argon2` | MIT / Apache-2.0 | Master passphrase key derivation |
| `chacha20poly1305` | MIT / Apache-2.0 | Vault AEAD |
| `hkdf`, `sha2`, `subtle` | MIT / Apache-2.0 | Subkey derivation, constant-time comparison |
| `zeroize` | MIT / Apache-2.0 | Erasing secret material on drop |
| `tokio` | MIT | Async runtime |
| `interprocess` | MIT / Apache-2.0 | Named pipes and Unix sockets for IPC |
| `reqwest` + `rustls` | MIT / Apache-2.0 / ISC | HTTPS for remote config. `rustls` is required by policy so the TLS stack is identical on every platform and no C TLS library enters the binary |
| `windows`, `windows-service` | MIT / Apache-2.0 | Win32 integration, service host |
| `notify-rust` | MIT / Apache-2.0 | Desktop notifications |
| `notify` | CC0-1.0 | Filesystem watching |
| `keyring` | MIT / Apache-2.0 | Optional OS keychain storage |
| `croner` | MIT | Cron expression evaluation |
| `globset`, `walkdir` | MIT / Unlicense | Exclusion matching, tree walking |

---

## Assets

The tray icons, application icon, and every illustration in `assets/` and
`design/` are original work created for this project and are covered by the
project's MIT licence. No third-party icon set or image is redistributed.

### Fonts — these *are* redistributed

An earlier version of this document claimed no font was bundled. That was
wrong, and generating the SBOM is what caught it. `egui` embeds its default
typefaces into the binary through the `epaint_default_fonts` crate, so every
superbackup build redistributes them:

| Font | Licence | Notes |
|---|---|---|
| Hack Regular | SIL Open Font License 1.1 | Monospace; used for paths and log output |
| Ubuntu Light | Ubuntu Font Licence 1.0 | Proportional UI text |
| Noto Emoji Regular | SIL Open Font License 1.1 | Emoji coverage |
| emoji-icon-font | MIT | egui's built-in glyph set |

Both the OFL and the Ubuntu Font Licence permit bundling a font inside an
application distributed under any licence, including MIT. Our obligations:

1. **Carry the notices.** The licence texts ship inside the `epaint_default_fonts`
   crate (`fonts/OFL.txt`, `fonts/UFL.txt`, `fonts/Hack-Regular.txt`,
   `fonts/emoji-icon-font-mit-license.txt`) and are reproduced in any
   distribution that includes a compiled binary.
2. **Do not sell the fonts by themselves.** The OFL forbids selling the font
   standalone. superbackup is free software; the question does not arise.
3. **Do not reuse Reserved Font Names.** A *modified* version of an OFL font may
   not keep its original name. We do not modify them.

A build that replaces these with system fonts — via `egui`'s font
configuration — carries none of the above. That is a supported configuration
for a distributor who would rather not redistribute fonts at all.

### Linux system libraries

On Linux the tray and window integration links against GTK 3
(LGPL-2.1-or-later) and libayatana-appindicator3 (LGPL-2.1/LGPL-3.0). These are
**dynamically linked system libraries**, not vendored or statically linked. The
LGPL permits this from an MIT-licensed application provided the user can replace
the library — which dynamic linking against the distribution's own packages
satisfies. No LGPL source is redistributed by this project.

---

## Export control

superbackup uses cryptography for the confidentiality of user data and drives
Kopia, which does the same. It implements no novel cryptography; every
primitive comes from published, widely deployed open-source implementations.

Distributors should confirm their own obligations. In the United States this
category of publicly available open-source encryption software is generally
handled under EAR 742.15(b) via a notification to BIS and the NSA at the point
of public release. **This paragraph is not legal advice.**

---

## Reproducing this audit

```bash
cargo deny check licenses     # licence policy
cargo deny check advisories   # known vulnerabilities
cargo deny check bans         # banned and duplicate crates
cargo deny check sources      # only crates.io
cargo tree --format "{p} {l}" # full tree with licences
```

Last reviewed: 2026-08-30, for version 0.1.0.
