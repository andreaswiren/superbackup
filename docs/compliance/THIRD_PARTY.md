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
BSD-3-Clause, ISC, Zlib, Unicode-3.0, Unicode-DFS-2016, MPL-2.0,
CDLA-Permissive-2.0, BSL-1.0.

MPL-2.0 is file-level copyleft: we may link it from an MIT binary, but a
modified MPL file must stay MPL. We do not modify any dependency, so this
imposes nothing beyond retaining notices.

**No exceptions are configured.** A dependency arriving under GPL, AGPL, LGPL,
SSPL, or a proprietary licence fails CI, because relicensing this project is a
decision to be taken deliberately and not discovered during a release.

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
| `notify` | CC0-1.0 / Artistic-2.0 | Filesystem watching |
| `keyring` | MIT / Apache-2.0 | Optional OS keychain storage |
| `croner` | MIT | Cron expression evaluation |
| `globset`, `walkdir` | MIT / Unlicense | Exclusion matching, tree walking |

---

## Assets

The tray icons, application icon, and every illustration in `assets/` and
`design/` are original work created for this project and are covered by the
project's MIT licence. No third-party icon set, font, or image is redistributed.

Any font referenced by the GUI is used from the operating system's own
installation and is not bundled.

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
