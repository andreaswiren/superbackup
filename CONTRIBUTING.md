# Contributing to superbackup

Thanks for considering it. This document is short and specific, because a
backup tool has a small number of rules that genuinely cannot bend, and a large
amount of ordinary latitude everywhere else.

## The rules that do not bend

These are not style preferences. A change that breaks one of them will be
rejected regardless of how good the rest of it is.

1. **Never lose user data.** Anything that writes must be atomic or resumable.
   Anything that deletes must be explicit, bounded, and impossible to point
   above its own root. When in doubt, do less.
2. **No secret leaves `Secret`.** No `Display`, no `Serialize`, no revealing
   `Debug`, no `String` that lingers. If you need a secret somewhere new, work
   out how to keep it inside the type rather than around it.
3. **No secret in `argv`.** Command-line arguments are readable by other users
   through `/proc` on Linux and WMI on Windows. Environment and stdin only.
4. **Third-party output is untrusted text.** Anything from kopia, git, or an
   S3 endpoint goes through `redact::scrub` before it reaches a log, an event,
   an error, an IPC response, or a notification.
5. **No `unwrap()` or `expect()` outside tests.** Malformed input is an
   `Error`. A corrupt vault must never be a panic.
6. **Never report success that did not happen.** If a destination was skipped,
   the run says so. `JobRun::derive_status()` is the only place that decides
   this — do not reimplement it.

## Getting set up

Rust 1.82 or newer (the CI pins the floor). On Linux you will also need:

```bash
sudo apt-get install libgtk-3-dev libxdo-dev libayatana-appindicator3-dev libdbus-1-dev pkg-config
```

Then:

```bash
cargo test --workspace
```

Point `SUPERBACKUP_HOME` at a scratch directory to get a completely
self-contained config, data, log and cache layout. Every integration test does
this, and so should you when trying something out — it keeps your experiments
away from a real configuration holding real repository keys.

```bash
SUPERBACKUP_HOME=/tmp/sb-scratch cargo run -- status
```

## Before you open a pull request

```bash
cargo fmt --all
cargo clippy --workspace --all-targets --all-features
cargo test --workspace
cargo deny check
```

CI runs all of this on Windows, Linux and macOS. Windows is the priority
platform: a change that works on Linux and breaks Windows is a broken change.

## Tests

Write the test that would have caught the bug. Tests that assert a function
returns what it was just told to return are worse than no tests, because they
cost review attention and catch nothing.

The interesting cases in this codebase are, reliably:

- **Time.** DST transitions in both directions, a machine that was asleep for a
  week, a bandwidth window that wraps past midnight.
- **Partial failure.** One destination out of three is unreachable.
- **Hostile paths.** Symlink loops, junctions, a destination nested inside its
  own source, paths over 260 characters on Windows.
- **Corrupt input.** A truncated vault, a flipped bit in the header, kopia
  printing something unexpected, an IPC client sending a 500 MB line.
- **Cancellation.** Stopping a job promptly, leaving no orphaned process and no
  held repository lock.

Anything touching the real registry, installing a service, or reaching the
network is `#[ignore]`d and documented as such.

## Commit messages

A subject line that says what changed, and a body that says *why* if the
reason is not obvious from the diff. The next person to read it will be trying
to work out whether they can safely change the thing you wrote.

## Platform work

Platform-specific code belongs in `platform/`, behind a trait with a working
implementation for every target. `unimplemented!()` is not a working
implementation — a stub that returns a sane default and reports the limitation
is. Every `unsafe` block needs a `// SAFETY:` comment justifying it.

Where a platform genuinely cannot do something, say so in the interface rather
than failing quietly. A user who is told "a LocalSystem service cannot reach
your OneDrive folder — install under your own account instead" can act. A user
whose backups silently do nothing cannot.

## Security issues

Do not open a public issue. See [`SECURITY.md`](SECURITY.md).

## Scope

superbackup is deliberately not a general-purpose sync tool, and not a
file-sharing product. It backs up folders to repositories and mirrors, with
enough interface to trust it. Proposals that expand that boundary are welcome
as issues for discussion before code — it is much cheaper to disagree about
scope in a paragraph than in a pull request.

## Licence

Contributions are accepted under the MIT licence, matching the project. By
opening a pull request you confirm you have the right to contribute the code
under those terms.
