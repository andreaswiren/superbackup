# Architecture

How superbackup is put together, and why it is put together that way.

## One binary, several personalities

`superbackup` is a single executable. Which personality it adopts depends on
how it is invoked:

| Invocation | Role |
|---|---|
| `superbackup` (no arguments) | Tray icon + scheduler + IPC server, in one process |
| `superbackup gui` | Opens the window (or focuses the existing instance) |
| `superbackup daemon` | Headless scheduler + IPC server, no tray |
| `superbackup service run` | Service entry point, invoked by the OS service manager |
| `superbackup <command>` | Connects to the running instance over IPC and prints the answer |

There is exactly one executable to install, sign, and update. A user's mental
model is "superbackup is running or it isn't", not "which of the four
superbackup processes is the broken one".

The consequence is that the CLI is **thin**. It does not open repositories or
touch the vault; it asks the running instance. Two processes driving the same
Kopia repository is a corruption risk, so only one process ever holds that
role, and a single-instance guard enforces it.

## Crate layout

```
crates/
  core/    superbackup-core — everything that is not a pixel
  app/     superbackup      — the binary: tray, GUI, CLI, daemon, service host
```

The split exists so that the engine can be tested without a display server, and
so that CI on a headless Linux runner exercises the parts that matter.

## Layering inside the core

```
        model ─────────────── the user's intent
        state ─────────────── what actually happened
          │
   ┌──────┴───────┐
 config         crypto        persist intent │ seal the secrets it refers to
   └──────┬───────┘
          │
   ┌──────┴───────┐
 kopia          engine        drive the backend │ decide and run
   └──────┬───────┘
          │
         ipc                  expose all of it to tray, GUI and CLI
```

Dependencies point downward only. `model` and `state` know about nothing else,
which is what makes them safe for every other module — and every subagent that
built one — to depend on simultaneously.

### `model` — intent

Providers, destinations, jobs, projects, schedules, exclusions. Serialised to
`config.json`. **Contains no secret material of any kind**: anything that would
be a password or key is a `SecretRef`, a stable handle resolved against the
unlocked vault at the moment of use. That invariant is what makes the config
file safe to read, diff, and share.

The relationship worth internalising:

```
StorageProvider   endpoint + region + credentials      "StorJ eu-1"
      │ 1..n
Destination       provider + bucket + prefix           "StorJ / backups / dev-pc/"
      │ n..n                                            or a local path
Job               sources → many destinations
      │ n..1
Project           grouping only
```

A provider is configured once. Rotating its key rotates it for every bucket and
every job that uses it — unless a destination pinned its own override, which
exists because buckets in the real world sometimes have their own key pairs.

### `state` — history

Runs, per-destination progress, events, and the five-valued `Health` behind the
tray icon. Kept rigorously separate from `model` for one reason: config can be
pulled from a shared Git repository, and a pull must never overwrite local
history.

`JobRun::derive_status()` is deliberately the only place that decides whether a
run succeeded. A run whose destinations partly failed or were skipped resolves
to `SucceededWithWarnings`, never `Succeeded` — a backup tool that says
"succeeded" when a destination was silently skipped is worse than one that says
nothing.

### `config` and `crypto` — persistence and sealing

Config is written atomically: temp file, fsync, rename. A crash leaves the old
file or the new one, never a mixture — which for the vault is the difference
between an inconvenience and losing every repository key.

The vault seals every secret under a key derived from the master passphrase.
Its format is versioned and self-describing, and its header is authenticated,
so an attacker cannot weaken the stored KDF parameters and re-present the file.
See [`compliance/THREAT_MODEL.md`](compliance/THREAT_MODEL.md) for the full
design.

### `kopia` — the backend driver

superbackup implements no deduplication, chunking, or backup encryption. Kopia
does. This module drives the `kopia` CLI as a subprocess and translates between
its world and ours.

Two rules shape the whole module:

1. **Secrets never enter `argv`.** Command-line arguments are readable by other
   users through `/proc` on Linux and WMI on Windows. Repository passphrases go
   through the child's environment; S3 keys through the AWS variables. A check
   in the command builder enforces this so a future contributor cannot
   accidentally undo it.
2. **Every byte kopia prints is untrusted text.** It goes through redaction
   before it can reach a log, an event, an error, or a notification.

Kopia's progress output is parsed incrementally as the child runs, so the GUI
shows live movement rather than a spinner. The child's pipes are drained
continuously — a blocked pipe deadlocks the child, which in a backup tool
presents as "it hangs at 40% on large folders".

### `engine` — deciding and doing

The scheduler owns a timing wheel that sleeps until the next due moment rather
than polling. Before any scheduled run starts it passes a gate: global pause,
job disabled, vault locked, metered connection, on battery, already running.
Each rejection is a recorded `Skipped` with a reason the interface can show,
because "it didn't run and I don't know why" is the failure mode that makes
people stop trusting a backup tool.

The runner executes one job across **all** its destinations, producing one
`DestinationRun` each. A destination failing does not have to take the others
down — that is `Job::continue_on_destination_error`, and the default is to keep
going, because a broken offsite link is not a reason to skip the local copy.

`LocalMirror` destinations have no Kopia. The mirror engine implements the copy
itself: incremental by size and mtime, exclusion-aware, long-path safe on
Windows, and guarded hard against writing above its own root or recursing into
a destination nested inside its own source.

The engine depends on a `BackupExecutor` trait rather than on the `kopia`
module directly. That keeps the scheduling logic testable against a mock with
no subprocess in sight, and it is why the scheduler's tests can prove DST and
catch-up behaviour deterministically.

### `ipc` — the seam

Newline-delimited JSON over a Windows named pipe or a Unix domain socket. Chosen
over a binary protocol because it can be debugged with a pipe client, spoken
from any language, and read by a human when something goes wrong.

The endpoint is a privilege boundary and is treated as one: restricted to the
owning user, remote clients rejected, line length capped, connections capped and
rate-limited. **There is no request that returns a plaintext secret.** The
protocol offers `SetSecret` and not `GetSecret`, so extracting a key from
superbackup requires the passphrase and is not something the protocol does.

The protocol also serves a machine-readable description of itself, generated
from the same definitions the dispatcher uses. That is what makes the CLI
self-documenting for an automation agent, and what stops the documentation from
drifting away from the implementation.

## The app crate

```
app/
  tray/      icon, menu, health-driven state
  gui/       egui windows: dashboard, jobs, destinations, providers, settings
  cli/       clap command surface, human and --json output
  daemon/    the Handler implementation wiring ipc → engine/config/crypto
  service/   OS service entry points
```

The GUI is **egui/eframe**: immediate-mode, pure Rust, no webview, no bundled
browser engine, no Node build step. A ~10 MB self-contained binary that starts
instantly, against a ~100 MB Electron application or a Tauri build carrying a
WebView2 runtime dependency. The cost is real — egui gives less typographic and
animation control than HTML — and the design system is built within that
constraint rather than fighting it.

## Cross-platform strategy

Windows is the priority, Linux second, macOS third. Platform-specific code is
confined to `platform/`, behind traits with a working implementation for every
target, so the rest of the codebase never branches on the operating system. CI
compiles all three on every push, and a cross-compile check catches `cfg`
mistakes before a release build does.

Where a platform genuinely cannot do something — a `LocalSystem` Windows
service cannot see the user's OneDrive folder, and Linux has no native OneDrive
client — the limitation is surfaced to the user in the interface rather than
papered over with a silent fallback.

## Concurrency

One tokio runtime. The scheduler, the IPC server, and each running job are
tasks. The GUI runs on the main thread as egui requires, and talks to the
runtime through channels.

Progress updates are coalesced before they reach subscribers: a snapshot of a
`node_modules` tree can generate tens of thousands of events per second, and
neither a 60 fps interface nor an IPC socket benefits from seeing all of them.
Coalescing never drops the final state of a run.

A slow IPC subscriber drops oldest with a "you missed N events" marker rather
than buffering without bound. An unresponsive client must not be able to stall
a backup.
