# superbackup — Wireframes

Version 1.0 · Companion to `UX_SPEC.md` and `DESIGN_SYSTEM.md`

## Reading these frames

**Grid**: 1 character column = **10 px**, 1 character row = **20 px**.
A 110 × 36 frame is therefore **1100 × 720** — the default window size.
The 900 × 600 frames are 90 × 30.

**Regions**: the vertical rule at column 22 is the 1 px rail divider
(rail = 208 px ≈ 21 columns). `═` rows are 1 px dividers. Every frame includes
the 40 px OS title bar as the first row, the 56 px header bar, the content area,
and the 28 px status strip.

**Glyphs**

| Glyph | Meaning | Glyph | Meaning |
|---|---|---|---|
| `✓` | success mark | `▓▓░░` | progress bar (filled / track) |
| `!` | warning mark | `▌` | 3 px status spine on a card |
| `✕` | failure mark | `●` `○` | radio selected / unselected |
| `▸` `▾` | collapsed / expanded | `[x]` `[ ]` | checkbox on / off |
| `(o )` `( o)` | toggle off / on | `[ Label ]` | button (primary in **bold** in the notes) |
| `▤ ☁ ▣ ⇄` | local repo / OneDrive / S3 / mirror icons | `…` | middle-elided text |
| `◉` | reveal-secret toggle | `⋮` `⋯` | row / card overflow menu |

Measurements below each frame are normative and take precedence over the
character grid, which can only approximate to 10 px.


---

## 1. Dashboard


### D-1 · Dashboard — one job running, 1100 × 720

**Regions**: title bar 40 px · header bar 56 px · content 24 px padding ·
status strip 28 px. Content column 819 px.

**Health strip** 819 × 88: three cards at 275 / 264 / 264 px, 16 px gutters,
16 px internal padding, radius 10, 1 px `border.subtle`. The health ring is
40 px, drawn with the same five states as the tray.

**Run panel** 819 × 212: radius 10, 1 px `border.control`, 16 px padding.
Aggregate bar 8 px tall; per-destination bars 6 px with a 90 px name column,
a 40 px right-aligned percentage, a 120 px byte column and a badge.

**Job cards** 401 × 96, 16 px gutter, 16 px row gap, 3 px status spine on the
left edge, `⋯` menu 26 × 26 at the top right, `Run now` 26 px compact ghost
bottom-right.

```
┌────────────────────────────────────────────────────────────────────────────────────────────────────────────┐
│  superbackup                                                                        _  □  ✕                │
│────────────────────────────────────────────────────────────────────────────────────────────────────────────│
│                     │ Dashboard                                                    [ Back up now ]   [ ⋯ ] │
│────────────────────────────────────────────────────────────────────────────────────────────────────────────│
│ ANDREAS-PC          │                                                                                      │
│ andreas-pc-a3f9c2   │ ┌──────────────────────────┐┌─────────────────────────┐┌──────────────────────────┐  │
│                     │ │ ((●)) Backing up         ││ NEXT SCHEDULED RUN      ││ LAST 7 DAYS              │  │
│ ▤  Dashboard        │ │       1 job running      ││ in 4 hours              ││  ▂▅█▃█▁▆    3.1 GB       │  │
│ ⟳  Jobs             │ │                          ││ Dev code · today 02:00  ││  M T W T F S S           │  │
│ ▣  Destinations     │ │                          ││                         ││  14 runs, 1 failed       │  │
│ ⚿  Storage prov.    │ └──────────────────────────┘└─────────────────────────┘└──────────────────────────┘  │
│ ↺  Restore          │                                                                                      │
│ ≡  Activity         │  Running now  (1)                                        [ Stop all ]                │
│                     │ ┌──────────────────────────────────────────────────────────────────────────────┐     │
│                     │ │ ⟳ Dev code                                        [Running]     [ Stop ]     │     │
│                     │ │ Started 4m 12s ago · triggered by Schedule                                   │     │
│                     │ │ ▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░  42%  │     │
│                     │ │ 84,214 of 120,000 files · 6.2 GB of 9.1 GB · 18.2 MB/s · approx 3m left      │     │
│                     │ │ Scanning C:\Users\andreas\…\web\src\components                               │     │
│                     │ │ ──────────────────────────────────────────────────────────────────────────── │     │
│                     │ │ ▤ Local repo    ▓▓▓▓▓▓▓▓▓▓▓▓▓▓░░░░░░  71%   4.1 GB up      [Running]         │     │
│                     │ │ ☁ OneDrive      ▓▓▓▓▓▓▓▓░░░░░░░░░░░░  38%   2.2 GB up      [Running]         │     │
│                     │ │ ▣ StorJ offsite ▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓ 100%   842 MB up      [✓ Succeeded]     │     │
│                     │ └──────────────────────────────────────────────────────────────────────────────┘     │
│ ⚙  Settings         │  Jobs  (4)                                                                           │
│ i  About            │ ┌───────────────────────────────────────┐┌───────────────────────────────────────┐   │
│                     │ │▌ Dev code                  [Running] ⋯││▌ Documents              [✓ Succeeded]⋯│   │
│                     │ │▌ Started 4m ago · Schedule            ││▌ Last run 6 h ago · 1m 02s · 44 MB up │   │
│                     │ │▌ ▓▓▓▓▓▓▓▓▓░░░░░░░░ 42% · 18.2 MB/s    ││▌ [▤ Local repo] [☁ OneDrive] [ Run ]  │   │
│                     │ └───────────────────────────────────────┘└───────────────────────────────────────┘   │
│                     │ ┌───────────────────────────────────────┐┌───────────────────────────────────────┐   │
│                     │ │▌ Photos                    [! Stale] ⋯││▌ Scratch VM             [  Disabled ]⋯│   │
│                     │ │▌ Last success 5 days ago              ││▌ Disabled · Succeeded 12 Mar          │   │
│                     │ │▌ [▣ StorJ offsite]           [ Run ]  ││▌ [▤ Local repo]          [ Enable ]   │   │
│                     │ └───────────────────────────────────────┘└───────────────────────────────────────┘   │
│────────────────────────────────────────────────────────────────────────────────────────────────────────────│
│ ● Daemon running │ Service installed │ Kopia 0.17.0 │ 02:04 Dev code started (Schedule)                    │
│◈ Unlocked  27 min   │                                                                                      │
└────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

### D-1 · Dashboard at 900 × 600

**What changed** (per `UX_SPEC.md` §4.4): the rail collapses to 64 px
icon-only with tooltips; content padding drops 24 → 20 px; the job grid becomes
one column; the run panel drops the per-destination byte column; the
`Last 7 days` tile drops its byte total and keeps the bar strip and the run
count; the status strip drops the Service segment.

```
┌────────────────────────────────────────────────────────────────────────────────────────┐
│  superbackup                                                    _  □  ✕                │
│────────────────────────────────────────────────────────────────────────────────────────│
│      │ Dashboard                                  [ Back up now ] [⋯]                  │
│────────────────────────────────────────────────────────────────────────────────────────│
│ AP   │ ┌───────────────────┐┌────────────────┐┌────────────────────┐                   │
│      │ │ ((●)) Backing up  ││ NEXT RUN       ││ LAST 7 DAYS        │                   │
│ ▤    │ │   1 job running   ││ in 4 hours     ││ ▂▅█▃█▁▆  14 runs   │                   │
│ ⟳    │ │                   ││ Dev code 02:00 ││ M T W T F S S      │                   │
│ ▣    │ └───────────────────┘└────────────────┘└────────────────────┘                   │
│ ⚿    │  Running now (1)                              [ Stop all ]                      │
│ ↺    │ ┌─────────────────────────────────────────────────────────┐                     │
│ ≡    │ │ ⟳ Dev code                       [Running]    [ Stop ]  │                     │
│      │ │ Started 4m 12s ago · Schedule                           │                     │
│      │ │ ▓▓▓▓▓▓▓▓▓▓▓▓▓░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░  42%   │                     │
│      │ │ 84,214 of 120,000 files · 18.2 MB/s · approx 3m left    │                     │
│      │ │ ─────────────────────────────────────────────────────── │                     │
│      │ │ ▤ Local repo    ▓▓▓▓▓▓▓▓░░░░  71%      [Running]        │                     │
│      │ │ ☁ OneDrive      ▓▓▓▓░░░░░░░░  38%      [Running]        │                     │
│      │ │ ▣ StorJ offsite ▓▓▓▓▓▓▓▓▓▓▓▓ 100%      [✓ Succeeded]    │                     │
│      │ └─────────────────────────────────────────────────────────┘                     │
│      │  Jobs (4)                                                                       │
│      │ ┌─────────────────────────────────────────────────────────┐                     │
│      │ │▌ Dev code                            [Running]        ⋯ │                     │
│      │ │▌ Started 4m ago · Schedule                              │                     │
│      │ │▌ ▓▓▓▓▓▓▓▓▓░░░░░░░░░░ 42% · 18.2 MB/s                    │                     │
│      │ └─────────────────────────────────────────────────────────┘                     │
│ ⚙    │ ┌─────────────────────────────────────────────────────────┐                     │
│ i    │ │▌ Documents                        [✓ Succeeded]       ⋯ │                     │
│      │ │▌ Last run 6 h ago · 1m 02s · 44 MB uploaded             │                     │
│      │ │▌ [▤ Local repo] [☁ OneDrive]              [ Run now ]   │                     │
│      │ └─────────────────────────────────────────────────────────┘                     │
│────────────────────────────────────────────────────────────────────────────────────────│
│ ● Daemon │ Kopia 0.17.0 │ 02:04 Dev code started                                       │
│ ◈    │                                                                                 │
└────────────────────────────────────────────────────────────────────────────────────────┘
```

### D-1L · Dashboard with the vault locked

The locked banner is pinned under the header bar on **every** screen, pushes
content down, and is not dismissible. The rail's vault control turns
`danger.tint.bg`. `Back up now` and every card's `Run now` are disabled but keep
their position and size. The next-run value is struck through with the reason
underneath. Nothing is hidden — only actions that need a resolved `SecretRef`
are blocked (`UX_SPEC.md` §3.2).

```
┌────────────────────────────────────────────────────────────────────────────────────────────────────────────┐
│  superbackup                                                                        _  □  ✕                │
│────────────────────────────────────────────────────────────────────────────────────────────────────────────│
│                     │ Dashboard                                                    [ Back up now ]   [ ⋯ ] │
│────────────────────────────────────────────────────────────────────────────────────────────────────────────│
│ ANDREAS-PC          │ ┌─────────────────────────────────────────────────────────────────────────────────┐  │
│ andreas-pc-a3f9c2   │ │ ⚿  The vault is locked                                        [ Unlock ]        │  │
│                     │ │    Scheduled backups will not start, and destinations cannot be reached,        │  │
│ ▤  Dashboard        │ │    until it is unlocked.                                                        │  │
│ ⟳  Jobs             │ └─────────────────────────────────────────────────────────────────────────────────┘  │
│ ▣  Destinations     │ ┌──────────────────────────┐┌─────────────────────────┐┌──────────────────────────┐  │
│ ⚿  Storage prov.    │ │ ((!)) Needs attention    ││ NEXT SCHEDULED RUN      ││ LAST 7 DAYS              │  │
│ ↺  Restore          │ │       The vault is locked││ in 4 hours  (struck)   ││  ▂▅█▃█▁▆    3.1 GB        │  │
│ ≡  Activity         │ │              [ Unlock ]  ││ blocked while locked    ││  M T W T F S S           │  │
│                     │ └──────────────────────────┘└─────────────────────────┘└──────────────────────────┘  │
│                     │  Jobs  (4)                                                                           │
│                     │ ┌───────────────────────────────────────┐┌───────────────────────────────────────┐   │
│                     │ │▌ Dev code               [✓ Succeeded]⋯││▌ Documents              [✓ Succeeded]⋯│   │
│                     │ │▌ Last run 2 h ago · 4m 12s · 842 MB up││▌ Last run 6 h ago · 1m 02s · 44 MB up │   │
│                     │ │▌ [▤][☁][▣]              [ Run now ]   ││▌ [▤][☁]                 [ Run now ]   │   │
│                     │ └───────────────────────────────────────┘└───────────────────────────────────────┘   │
│                     │                                     ↑ disabled, tooltip: Unlock the vault            │
│                     │                                       to use this.                                   │
│                     │                                                                                      │
│                     │                                                                                      │
│                     │                                                                                      │
│                     │                                                                                      │
│                     │                                                                                      │
│                     │                                                                                      │
│                     │                                                                                      │
│                     │                                                                                      │
│                     │                                                                                      │
│                     │                                                                                      │
│                     │                                                                                      │
│────────────────────────────────────────────────────────────────────────────────────────────────────────────│
│ ● Daemon running │ Service installed │ Kopia 0.17.0 │ 21:40 Vault locked (auto-lock)                       │
│⚿ Locked             │                                                                                      │
└────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

---

## 2. Jobs


### J-1 · Jobs list

Row actions (`Run now` / `Stop` compact ghost + `⋯`) appear on hover **and on
keyboard focus** in the Actions column. Multi-select replaces the header actions
with a 44 px selection bar.

```
┌────────────────────────────────────────────────────────────────────────────────────────────────────────────┐
│  superbackup                                                                        _  □  ✕                │
│────────────────────────────────────────────────────────────────────────────────────────────────────────────│
│                     │ Jobs           [ Search jobs   ] [Group: None ▾] [Filter: All ▾] [ New job ]         │
│────────────────────────────────────────────────────────────────────────────────────────────────────────────│
│ ANDREAS-PC          │ ┌─────────────────────────────────────────────────────────────────────────────┐      │
│ andreas-pc-a3f9c2   │ │ St Name          Fldr Destinations   Schedule    Last run   Next   Up   ⋮   │      │
│                     │ │─────────────────────────────────────────────────────────────────────────────│      │
│ ▤  Dashboard        │ │ ✕  Photos          1  [▣]           Daily 03:00 5 d ago     in 5h  0 B  ⋮   │      │
│ ⟳  Jobs             │ │    Family archive                                                           │      │
│ ▣  Destinations     │ │─────────────────────────────────────────────────────────────────────────────│      │
│ ⚿  Storage prov.    │ │ !  Website         2  [▤][☁]        On change   2 d ago     —    112M  ⋮    │      │
│ ↺  Restore          │ │─────────────────────────────────────────────────────────────────────────────│      │
│ ≡  Activity         │ │ ⟳  Dev code        2  [▤][☁][▣]     Daily 02:00 running     —    842M  ⋮    │      │
│                     │ │─────────────────────────────────────────────────────────────────────────────│      │
│                     │ │ ✓  Documents       2  [▤][☁]        Daily 02:00 6 h ago     in 4h   44M  ⋮  │      │
│                     │ │─────────────────────────────────────────────────────────────────────────────│      │
│                     │ │ ✓  Scratch VM      1  [▤]           Manual only 12 Mar      —    2.1G  ⋮    │      │
│                     │ │                                                          [Disabled]         │      │
│                     │ └─────────────────────────────────────────────────────────────────────────────┘      │
│                     │                                                                                      │
│                     │   Column widths at 1100: Status 32 · Name 220 · Folders 60 ·                         │
│                     │   Destinations 150 · Schedule 130 · Last run 120 · Next run 120 ·                    │
│                     │   Uploaded 90 · Actions remainder (min 76).                                          │
│                     │   Row height 36 px, or 48 px when any row carries a description.                     │
│                     │   Header row 32 px, sticky, bg.raised. Row dividers 1 px, inset 12 px.               │
│                     │   Default sort: Status descending (problems first), then Name.                       │
│                     │                                                                                      │
│                     │   Drop order at 900 px: Uploaded → Next run → Folders.                               │
│                     │                                                                                      │
│                     │                                                                                      │
│                     │                                                                                      │
│                     │                                                                                      │
│                     │                                                                                      │
│                     │                                                                                      │
│────────────────────────────────────────────────────────────────────────────────────────────────────────────│
│ ● Daemon running │ Service installed │ Kopia 0.17.0 │ 02:04 Dev code started (Schedule)                    │
│◈ Unlocked  27 min   │                                                                                      │
└────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

### J-2 · Job editor — Exclusions tab

**Preset rows** 56 px, radius 6, 8 px apart. The rationale line is
`ExclusionPreset::rationale()` **verbatim** — the GUI never paraphrases it, so
the CLI and the GUI can never disagree. Risky presets (`GitObjects`,
`VirtualMachineImages`) carry a `!` mark and a 30 % `warning.tint.bg` row.
Expanding `n patterns` grows the row by `n × 20 px` and lists them in
`mono.small`.

**Impact strip** is pinned to the bottom of the tab, 44 px, `bg.raised`,
recomputed 600 ms after the last edit with a 4-second walk budget.

The dirty dot `●` on the tab label is announced as `, has unsaved changes`.

```
┌────────────────────────────────────────────────────────────────────────────────────────────────────────────┐
│  superbackup                                                                        _  □  ✕                │
│────────────────────────────────────────────────────────────────────────────────────────────────────────────│
│                     │ ← Jobs / Dev code                        [ Run now ] [⋯]  [ Cancel ] [ Save changes ]│
│────────────────────────────────────────────────────────────────────────────────────────────────────────────│
│ ANDREAS-PC          │ ┌───────┬──────────────┬──────────┬─────────────┬──────────┐                         │
│ andreas-pc-a3f9c2   │ │Folders│ Destinations │ Schedule │ Exclusions ●│ Advanced │                         │
│                     │ └───────┴──────────────┴──────────┴─────────────┴──────────┘                         │
│ ▤  Dashboard        │  Exclusions                                                                          │
│ ⟳  Jobs             │  Leaving out files you can rebuild is what keeps a developer backup                  │
│ ▣  Destinations     │  small enough to finish every night.                                                 │
│ ⚿  Storage prov.    │  [ Select developer defaults ]  [ Clear all ]                                        │
│ ↺  Restore          │ ┌─────────────────────────────────────────────────────────────────────┐              │
│ ≡  Activity         │ │ [x] node_modules                                       3 patterns ▸ │              │
│                     │ │     Reinstallable from your lockfile. Usually the single largest win│              │
│                     │ │─────────────────────────────────────────────────────────────────────│              │
│                     │ │ [x] Next.js / bundler caches                           9 patterns ▸ │              │
│                     │ │     Regenerated on the next build. Pure churn otherwise.            │              │
│                     │ │─────────────────────────────────────────────────────────────────────│              │
│                     │ │ [x] Rust target directories                            3 patterns ▾ │              │
│                     │ │     Rebuildable. Keeps repository maintenance fast.                 │              │
│                     │ │       /**/target/debug/   /**/target/release/  /**/target/tmp/      │              │
│                     │ │─────────────────────────────────────────────────────────────────────│              │
│                     │ │ [ ] ! Git object stores                                2 patterns ▸ │              │
│                     │ │     Packfiles are recoverable from your remote — if you have one.   │              │
│                     │ └─────────────────────────────────────────────────────────────────────┘              │
│                     │  Additional options                                                                  │
│                     │   (o ) Use .gitignore files found in the folders                                     │
│                     │   ( o) Skip folders tagged with CACHEDIR.TAG                                         │
│                     │   [ ] Skip files larger than [    500 ] MB                                           │
│                     │  Your own patterns                                                                   │
│                     │ ┌────────────────────────────────────────────────────────────────────┐               │
│                     │ │ /**/*.psd                                                          │               │
│                     │ │ /**/coverage/                                                      │               │
│                     │ └────────────────────────────────────────────────────────────────────┘               │
│                     │  ▸ Show all effective patterns (41)                                                  │
│                     │ ┌────────────────────────────────────────────────────────────────────┐               │
│                     │ │ These rules leave out about 8.2 GB in 412,000 files.               │               │
│                     │ └────────────────────────────────────────────────────────────────────┘               │
│────────────────────────────────────────────────────────────────────────────────────────────────────────────│
│ ● Daemon running │ Service installed │ Kopia 0.17.0 │ 14:22 Dev code edited                                │
│◈ Unlocked  30 min   │                                                                                      │
└────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

### J-2 · Job editor — Destinations tab (the fan-out)

Rows 64 px, radius 6, 8 px apart. Ordering (ticked first, then by kind, then by
name) is applied **on save only**, so ticking a box never makes the list jump.
Disabled destinations render at 60 % opacity and cannot be ticked.

`New destination…` pushes the destination editor as a full screen and returns
here with the new destination ticked, preserving the unsaved job edits.

```
┌────────────────────────────────────────────────────────────────────────────────────────────────────────────┐
│  superbackup                                                                        _  □  ✕                │
│────────────────────────────────────────────────────────────────────────────────────────────────────────────│
│                     │ ← Jobs / Dev code                        [ Run now ] [⋯]  [ Cancel ] [ Save changes ]│
│────────────────────────────────────────────────────────────────────────────────────────────────────────────│
│ ANDREAS-PC          │ ┌───────┬──────────────┬──────────┬────────────┬──────────┐                          │
│ andreas-pc-a3f9c2   │ │Folders│ Destinations │ Schedule │ Exclusions │ Advanced │                          │
│                     │ └───────┴──────────────┴──────────┴────────────┴──────────┘                          │
│ ▤  Dashboard        │  Send this backup to                                                                 │
│ ⟳  Jobs             │  Every destination you tick receives a complete copy. A failure at one               │
│ ▣  Destinations     │  does not stop the others.                                                           │
│ ⚿  Storage prov.    │ ┌─────────────────────────────────────────────────────────────────────┐              │
│ ↺  Restore          │ │ [x] ▤ Local repo                       [✓ Verified 2 h ago]      ⋮  │              │
│ ≡  Activity         │ │        Local repository · D:\superbackup\andreas-pc\repository      │              │
│                     │ └─────────────────────────────────────────────────────────────────────┘              │
│                     │ ┌─────────────────────────────────────────────────────────────────────┐              │
│                     │ │ [x] ☁ OneDrive                         [✓ Verified 2 h ago]      ⋮  │              │
│                     │ │        OneDrive repository · C:\Users\…\OneDrive\superbackup        │              │
│                     │ └─────────────────────────────────────────────────────────────────────┘              │
│                     │ ┌─────────────────────────────────────────────────────────────────────┐              │
│                     │ │ [x] ▣ StorJ offsite                    [! Never verified]        ⋮  │              │
│                     │ │        S3 bucket · storj-backups / superbackup/andreas-pc-a3f9…/    │              │
│                     │ └─────────────────────────────────────────────────────────────────────┘              │
│                     │ ┌─────────────────────────────────────────────────────────────────────┐              │
│                     │ │ [ ] ⇄ Plain copy for Ana               [  Disabled ]              ⋮ │              │
│                     │ │        Folder mirror · E:\share\dev-mirror                          │              │
│                     │ └─────────────────────────────────────────────────────────────────────┘              │
│                     │  [ New destination… ]                                                                │
│                     │                                                                                      │
│                     │  When a destination fails                                                            │
│                     │   ( o) Keep going to the other destinations                                          │
│                     │   With this off, the first destination that fails stops the run and                  │
│                     │   the rest are recorded as cancelled.                                                │
│                     │                                                                                      │
│                     │                                                                                      │
│                     │                                                                                      │
│────────────────────────────────────────────────────────────────────────────────────────────────────────────│
│ ● Daemon running │ Service installed │ Kopia 0.17.0 │ 14:22 Dev code edited                                │
│◈ Unlocked  30 min   │                                                                                      │
└────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

---

## 3. New job wizard (760 px modal)


### W-1 · Template chooser

Modal 760 × 620 (capped at `window_height − 96`). Header 56 px, footer 60 px.
Template cards 340 × 104 in a 2 × 2 grid with a 16 px gutter. The
`Development folder` card is preselected and carries a 1 px `accent` border and
the `accent` eyebrow. Choosing it calls `ExclusionSet::developer_defaults()`
directly — the GUI never assembles its own preset list.

```
┌────────────────────────────────────────────────────────────────────────┐
│ New job                                                              ✕ │
│ Step 1 of 6 · Template                                                 │
│════════════════════════════════════════════════════════════════════════│
│                                                                        │
│ ┌────────────────────────────────┐ ┌────────────────────────────────┐  │
│ │ ⟳  RECOMMENDED FOR DEVELOPERS  │ │ ▤                              │  │
│ │    Development folder          │ │    Documents and desktop       │  │
│ │    Your code, without the      │ │    The folders most people     │  │
│ │    parts that rebuild          │ │    lose first.                 │  │
│ │    themselves.                 │ │                                │  │
│ │    C:\Users\andreas\dev        │ │    C:\Users\andreas\Documents  │  │
│ │    Applies 10 exclusion presets│ │    Skips OS junk and temp files│  │
│ └────────────────────────────────┘ └────────────────────────────────┘  │
│                                                                        │
│ ┌────────────────────────────────┐ ┌────────────────────────────────┐  │
│ │ ⌂                              │ │ +                              │  │
│ │    Whole user folder           │ │    Start from scratch          │  │
│ │    Everything under your       │ │    Choose the folders          │  │
│ │    home folder.                │ │    yourself.                   │  │
│ │                                │ │                                │  │
│ │    C:\Users\andreas            │ │    No exclusions and no        │  │
│ │    Expect a large first run.   │ │    schedule until you add them.│  │
│ └────────────────────────────────┘ └────────────────────────────────┘  │
│                                                                        │
│════════════════════════════════════════════════════════════════════════│
│                                        [ Cancel ]        [ Continue ]  │
└────────────────────────────────────────────────────────────────────────┘
```

### W-6 · Review

Key/value list: 160 px label column, `remainder` value column, 28 px rows.
Sizes come from the background walk started at W-2 with a 2-second budget.
When the vault is locked, `Create job` still creates the job and a warning toast
explains the run did not start, with an `Unlock and run` action.

```
┌────────────────────────────────────────────────────────────────────────┐
│ New job                                                              ✕ │
│ Step 6 of 6 · Review                                                   │
│════════════════════════════════════════════════════════════════════════│
│                                                                        │
│ Name             Development                                           │
│ Project          — none —                                              │
│ Folders          C:\Users\andreas\dev          12.4 GB · 84,102 files  │
│                  C:\Users\andreas\source\repos  3.1 GB · 21,880 files  │
│                  after exclusions                                      │
│ Destinations     Local repo · OneDrive · StorJ offsite (to create)     │
│ Schedule         Daily at 02:00 — next run tonight at 02:00            │
│ Exclusions       10 presets, 41 patterns                               │
│                  leaves out about 8.2 GB in 412,000 files              │
│ Retention        Each destination's own policy                         │
│ Bandwidth        Global limit (2,000 kB/s up)                          │
│                                                                        │
│ ────────────────────────────────────────────────────────────────────── │
│                                                                        │
│ [x] Run this job now                                                   │
│     The first run copies everything. Later runs copy only what changed.│
│                                                                        │
│                                                                        │
│                                                                        │
│                                                                        │
│                                                                        │
│════════════════════════════════════════════════════════════════════════│
│                        [ Back ]  [ Cancel ]      [ Create job ]        │
└────────────────────────────────────────────────────────────────────────┘
```

---

## 4. Destinations


### T-1 · Destinations list

```
┌────────────────────────────────────────────────────────────────────────────────────────────────────────────┐
│  superbackup                                                                        _  □  ✕                │
│────────────────────────────────────────────────────────────────────────────────────────────────────────────│
│                     │ Destinations                       [ Search    ] [Kind: All ▾]   [ New destination ] │
│────────────────────────────────────────────────────────────────────────────────────────────────────────────│
│ ANDREAS-PC          │ ┌────────────────────────────────────────────────────────────────────────────┐       │
│ andreas-pc-a3f9c2   │ │ K Name        Location                     Used  Size  Verified  Status ⋮  │       │
│                     │ │────────────────────────────────────────────────────────────────────────────│       │
│ ▤  Dashboard        │ │ ▤ Local repo  D:\superbackup\andreas-pc\…  4 jobs 61 GB 2 h ago  [Ready]   │       │
│ ⟳  Jobs             │ │────────────────────────────────────────────────────────────────────────────│       │
│ ▣  Destinations     │ │ ☁ OneDrive ✦  C:\Users\…\OneDrive\superb…  3 jobs 58 GB 2 h ago  [Ready]   │       │
│ ⚿  Storage prov.    │ │────────────────────────────────────────────────────────────────────────────│       │
│ ↺  Restore          │ │ ▣ StorJ offs. StorJ eu-1 · storj-backups/… 2 jobs 44 GB Never    [!  Not   │       │
│ ≡  Activity         │ │                                                                  connected]│       │
│                     │ │────────────────────────────────────────────────────────────────────────────│       │
│                     │ │ ⇄ Plain copy  E:\share\dev-mirror          0 jobs  —    12 d ago [Disabled]│       │
│                     │ └────────────────────────────────────────────────────────────────────────────┘       │
│                     │                                                                                      │
│                     │   ✦ = found automatically (auto_discovered)                                          │
│                     │                                                                                      │
│                     │   Row height 44 px. Columns: Kind 36 · Name 200 · Location remainder                 │
│                     │   (min 240) · Used by 90 · Size 100 · Last verified 120 · Status 100 ·               │
│                     │   Actions 76.  Drop order at 900 px: Size → Last verified → Used by.                 │
│                     │                                                                                      │
│                     │   ⋮ menu: Verify now · Browse snapshots… · Edit… · Duplicate ·                       │
│                     │            Run maintenance now · Disable · ─── · Remove… (danger)                    │
│                     │                                                                                      │
│                     │                                                                                      │
│                     │                                                                                      │
│                     │                                                                                      │
│                     │                                                                                      │
│                     │                                                                                      │
│                     │                                                                                      │
│                     │                                                                                      │
│────────────────────────────────────────────────────────────────────────────────────────────────────────────│
│ ● Daemon running │ Service installed │ Kopia 0.17.0 │ 12:02 OneDrive verified                              │
│◈ Unlocked  22 min   │                                                                                      │
└────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

### T-2c · Destination editor — S3 bucket

Fields are 400 px unless stated (Bucket 280, Region 200). Groups are separated
by a 1 px `border.subtle` with 20 px above and 16 px below. The provider strip
is 44 px `bg.raised`, read-only — it is what stops the user re-entering
credentials per bucket.

The key prefix normalises through `normalise_prefix()` **on blur** and shows the
normalised value back immediately, so surprises happen at edit time.

```
┌────────────────────────────────────────────────────────────────────────────────────────────────────────────┐
│  superbackup                                                                        _  □  ✕                │
│────────────────────────────────────────────────────────────────────────────────────────────────────────────│
│                     │ ← Destinations / StorJ offsite                        [ Verify ]  [ Save changes ]   │
│────────────────────────────────────────────────────────────────────────────────────────────────────────────│
│ ANDREAS-PC          │  Name        [ StorJ offsite                        ]                                │
│ andreas-pc-a3f9c2   │  Kind        [ S3 bucket ]  fixed once a destination exists                          │
│                     │  ( o) Enabled                                                                        │
│ ▤  Dashboard        │  ──────────────────────────────────────────────────────────────────────              │
│ ⟳  Jobs             │  Storage provider                                                                    │
│ ▣  Destinations     │  [ StorJ eu-1 (personal)                          ▾ ]                                │
│ ⚿  Storage prov.    │ ┌─────────────────────────────────────────────────────────────────────┐              │
│ ↺  Restore          │ │ https://gateway.storjshare.io · eu-1 · StorJ      [ Edit provider ] │              │
│ ≡  Activity         │ └─────────────────────────────────────────────────────────────────────┘              │
│                     │  Bucket      [ storj-backups            ]  [ List buckets ]                          │
│                     │  Key prefix  [ superbackup/andreas-pc-a3f9c2d1/    ]                                 │
│                     │    Full path: s3://storj-backups/superbackup/andreas-pc-a3f9c2d1/                    │
│                     │    The default contains this machine's folder name, which is what                    │
│                     │    keeps several computers and several jobs apart inside one bucket.                 │
│                     │  Credentials for this bucket                                                         │
│                     │    (●) Use the provider's credentials                                                │
│                     │        Uses the keys stored on StorJ eu-1 (personal).                                │
│                     │    ( ) Use a separate key pair for this bucket                                       │
│                     │  ──────────────────────────────────────────────────────────────────────              │
│                     │  Encryption                                                                          │
│                     │ ┌────────────────────────────────────────────────────────────────────┐               │
│                     │ │ Recommended settings — AES-256-GCM, BLAKE2B-256, dynamic 4 MB      │               │
│                     │ │ blocks, no error correction.                          [ Change… ]  │               │
│                     │ └────────────────────────────────────────────────────────────────────┘               │
│                     │  These settings are fixed when the repository is created and cannot be               │
│                     │  changed afterwards.                     [ Create repository ]                       │
│                     │  ──────────────────────────────────────────────────────────────────────              │
│                     │  ▸ Retention        Keeps 10 latest, 24 hourly, 14 daily, 8 weekly…                  │
│                     │  ▸ Bandwidth        Using the global limit                                           │
│────────────────────────────────────────────────────────────────────────────────────────────────────────────│
│ ● Daemon running │ Service installed │ Kopia 0.17.0 │ 12:02 OneDrive verified                              │
│◈ Unlocked  22 min   │                                                                                      │
└────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

### T-4 · Encryption panel (expanded)

Every control maps 1:1 onto `EncryptionSettings`. Option labels are the exact
`kopia_id()` tokens; helper lines for the algorithm are
`EncryptionAlgorithm::describe()` **verbatim**. The splitter suggestion strip
appears only when the job's sources have a high small-file ratio and sets
`Splitter::recommended_for_many_small_files()`; it never changes the value on
its own.

```
┌──────────────────────────────────────────────────────────────────────────────────────┐
│ Encryption                                                                           │
│══════════════════════════════════════════════════════════════════════════════════════│
│ Encryption algorithm                                                                 │
│  (●) AES256-GCM-HMAC-SHA256                                        RECOMMENDED       │
│      Hardware-accelerated on virtually every modern x86 and ARM CPU. Recommended.    │
│  ( ) CHACHA20-POLY1305-HMAC-SHA256                                                   │
│      Faster on CPUs without AES instructions. Equally strong.                        │
│                                                                                      │
│ Hash        [ BLAKE2B-256                                       ▾ ]                  │
│             Default. Fast and well studied.                                          │
│                                                                                      │
│ Splitter    [ DYNAMIC-4M-BUZHASH                                ▾ ]                  │
│             How files are cut into blocks before they are stored.                    │
│ ┌──────────────────────────────────────────────────────────────────────────────────┐ │
│ │ Your folders hold a lot of small files. DYNAMIC-2M-BUZHASH deduplicates them     │ │
│ │ better.                                                            [ Use it ]    │ │
│ └──────────────────────────────────────────────────────────────────────────────────┘ │
│                                                                                      │
│ ( o) Add error-correcting data                                                       │
│      Overhead [   5 ] %      Reed-Solomon with CRC32                                 │
│      Stores extra data so a repository survives a limited amount of corruption.      │
│      It does nothing about a whole disk failing.                                     │
│                                                                                      │
│ Repository passphrase                                                                │
│  (●) Generate one for me                                                             │
│      superbackup generates 256 random bits and keeps them in your vault. You are     │
│      shown the passphrase once and asked to save it.                                 │
│  ( ) I will choose it                                                                │
│      Use this if you also open this repository with the kopia command line.          │
│  ( ) Work it out from my master passphrase                                           │
│      Nothing extra to store. If you lose your master passphrase, this repository is  │
│      lost with it.                                                                   │
│                                                                                      │
│ These settings are fixed when the repository is created and cannot be changed        │
│ afterwards.                                                                          │
│══════════════════════════════════════════════════════════════════════════════════════│
│                                                              [ Create repository ]   │
└──────────────────────────────────────────────────────────────────────────────────────┘
```

### T-4b · Repository creation checklist

Rows 20 px. Icons move `○` → `⟳` → `✓`, or `✕` on failure with the
`RunError.message` inline and a `Retry` / `Change settings` pair beneath.

```
┌──────────────────────────────────────────────────────────────────────────────────────┐
│ Creating the repository                                                              │
│══════════════════════════════════════════════════════════════════════════════════════│
│  ✓  Checking the location                                                            │
│  ✓  Creating the repository                                                          │
│  ⟳  Storing the passphrase in your vault                                             │
│  ○  Applying the retention policy                                                    │
│  ○  Writing the machine record                                                       │
│     A small folder called _superbackup is written alongside the data, so anyone      │
│     browsing this drive later can tell which computer each backup belongs to.        │
│══════════════════════════════════════════════════════════════════════════════════════│
└──────────────────────────────────────────────────────────────────────────────────────┘
```

### T-5 · Write this down (blocking modal, 760 px)

No `✕`, Escape does nothing. `Done` is disabled until the checkbox is ticked.
The passphrase block is `bg.code`, 16 px padding, `mono.strong` 13 px, four
8-character groups per line with a wide gap. `Copy` clears the clipboard after
60 seconds and says so. The screen-reader affordance is specified in
`UX_SPEC.md` §18 — the block is focusable and read character by character only
on explicit focus.

```
┌────────────────────────────────────────────────────────────────────────────┐
│ ⚿  Write this down now                                                     │
│════════════════════════════════════════════════════════════════════════════│
│                                                                            │
│ This passphrase opens the repository at                                    │
│ s3://storj-backups/superbackup/andreas-pc-a3f9c2d1/. It is stored in your  │
│ vault, so you will not normally be asked for it.                           │
│                                                                            │
│ You will need it if you ever restore on a different computer, or if your   │
│ vault is lost.                                                             │
│                                                                            │
│ ┌────────────────────────────────────────────────────────────────────────┐ │
│ │  7QK3M9XA   PL42VDTR   6HNS8BWZ   E1CY5FJG                             │ │
│ │  RD9UT2MK   4XVBQ7HL   S3NAZ6PE   W8KFY1DC                             │ │
│ └────────────────────────────────────────────────────────────────────────┘ │
│ The passphrase is shown in groups only to make it easier to copy.          │
│ The spaces are not part of it.                                             │
│                                                                            │
│ [ Copy ]   [ Save to a file… ]   [ Print… ]                                │
│                                                                            │
│ [ ] I have saved this passphrase somewhere safe.                           │
│                                                                            │
│ If you skip this, the passphrase can still be exported later from          │
│ Settings › Security, using your master passphrase.                         │
│                                                                            │
│════════════════════════════════════════════════════════════════════════════│
│                                                            [ Done ]        │
└────────────────────────────────────────────────────────────────────────────┘
```

---

## 5. Storage providers


### P-1 / P-2 · Providers list and editor

The `Used by N destinations` strip is 44 px `bg.raised` and is computed from
`Config::destinations_using()`. Destinations with a
`credential_override` are listed separately under
`Not affected — these use their own key pair`; hiding that distinction would
make key rotation dangerous.

The access key ID is **not** masked — it is an identifier, and masking it makes
verification harder for no benefit. The secret is masked with a reveal toggle
that re-masks on blur, on window focus loss, and after 15 seconds.

```
┌────────────────────────────────────────────────────────────────────────────────────────────────────────────┐
│  superbackup                                                                        _  □  ✕                │
│────────────────────────────────────────────────────────────────────────────────────────────────────────────│
│                     │ Storage providers                       [ Search           ]      [ New provider ]   │
│────────────────────────────────────────────────────────────────────────────────────────────────────────────│
│ ANDREAS-PC          │ ┌────────────────────────────────────────────────────────────────────────────┐       │
│ andreas-pc-a3f9c2   │ │ F Name              Endpoint                  Used by      Verified    ⋮   │       │
│                     │ │────────────────────────────────────────────────────────────────────────────│       │
│ ▤  Dashboard        │ │ ▣ StorJ eu-1        gateway.storjshare.io     2 dests      2 h ago     ⋮   │       │
│ ⟳  Jobs             │ │   (personal)        · eu-1                                                 │       │
│ ▣  Destinations     │ │────────────────────────────────────────────────────────────────────────────│       │
│ ⚿  Storage prov.    │ │ ▣ MinIO (lab)       10.0.0.14:9000 · us-east-1  Not used   Never       ⋮   │       │
│ ↺  Restore          │ │   ! plain HTTP                                                             │       │
│ ≡  Activity         │ └────────────────────────────────────────────────────────────────────────────┘       │
│                     │                                                                                      │
│                     │  ── Provider editor (pushed screen) ──────────────────────────────────               │
│                     │ ┌────────────────────────────────────────────────────────────────────┐               │
│                     │ │ Used by 2 destinations across 5 jobs.               [ Show them ]  │               │
│                     │ └────────────────────────────────────────────────────────────────────┘               │
│                     │  Name          [ StorJ eu-1 (personal)                ]                              │
│                     │  Notes         [ Personal StorJ account, paid monthly ]                              │
│                     │  Provider type [ StorJ                              ▾ ]                              │
│                     │    Endpoint and region filled in for StorJ. Change them if your                      │
│                     │    account differs.                                                                  │
│                     │  Endpoint      [ https://gateway.storjshare.io        ]                              │
│                     │    https://gateway.storjshare.io — TLS on, port 443                                  │
│                     │  Region        [ eu-1       ]                                                        │
│                     │  ( o) Use TLS          ( o) Path-style addressing                                    │
│                     │  Credentials                                                                         │
│                     │  Access key ID     [ jvabc7xyz2q4m8n1p5r9              ]                             │
│                     │  Secret access key [ ••••••••••••••••••••    ] [◉] [ Replace… ]                      │
│                     │    Stored in your vault. Leave blank to keep it.                                     │
│                     │    Handed to kopia through the environment, never on a command line.                 │
│                     │                        [ Test connection ]   [ Save provider ]                       │
│────────────────────────────────────────────────────────────────────────────────────────────────────────────│
│ ● Daemon running │ Service installed │ Kopia 0.17.0 │ 12:02 StorJ eu-1 verified                            │
│◈ Unlocked  22 min   │                                                                                      │
└────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

### P-4 · Key rotation, step 2

Medium modal, three internal steps — never a modal on a modal
(`DESIGN_SYSTEM.md` L13). The vault write is atomic: either every affected
`SecretRef` is updated or none is, and the failure copy says so explicitly.

```
┌───────────────────────────────────────────────────────────────────────────────┐
│ Rotate the keys on StorJ eu-1 (personal)                                    ✕ │
│ Step 2 of 3 · New credentials                                                 │
│═══════════════════════════════════════════════════════════════════════════════│
│                                                                               │
│ superbackup cannot create keys for you. Create a new key pair in your         │
│ provider's console, enter it here, and it will be checked against every       │
│ destination before anything is replaced.                                      │
│                                                                               │
│ ! Your old key keeps working until you revoke it yourself.                    │
│                                                                               │
│ Access key ID     [ ka9m2v7t4x1p8s3z                          ]               │
│ Secret access key [ ••••••••••••••••••••••••                  ] [◉]           │
│ [ ] Use a session token                                                       │
│                                                                               │
│                             [ Verify against all destinations ]               │
│                                                                               │
│ ┌───────────────────────────────────────────────────────────────────────────┐ │
│ │ ✓  StorJ offsite            Reachable with the new key                    │ │
│ │ ✓  StorJ archive            Reachable with the new key                    │ │
│ │ ✕  StorJ media              Not reachable with the new key: access denied │ │
│ └───────────────────────────────────────────────────────────────────────────┘ │
│                                                                               │
│ Fix the failures above, or continue and accept that these destinations will   │
│ fail on their next run.                                                       │
│                                                                               │
│ Not affected — these use their own key pair:  Backblaze weekly                │
│                                                                               │
│═══════════════════════════════════════════════════════════════════════════════│
│          [ Back ]  [ Cancel ]  [ Continue anyway ]      [ Continue ]          │
└───────────────────────────────────────────────────────────────────────────────┘
```

---

## 6. Activity


### A-1 · Activity — Runs and Events

The Destinations column shows one 14 px status dot per `DestinationRun` in
order, plus `succeeded / total`. This is the one column that makes a fan-out
outcome legible at a glance, and it is why a partial success is never rendered
as a plain tick.

Events rows are 28 px compact; clicking one expands it in place to 200 px with
`Event.fields` as a `mono.small` key/value block.

```
┌────────────────────────────────────────────────────────────────────────────────────────────────────────────┐
│  superbackup                                                                        _  □  ✕                │
│────────────────────────────────────────────────────────────────────────────────────────────────────────────│
│                     │ Activity     [ Runs | Events ] [ Search ] [Last 7 days ▾]           [ Export… ]      │
│────────────────────────────────────────────────────────────────────────────────────────────────────────────│
│ ANDREAS-PC          │  Job: Dev code ✕   Status: Failed ✕                     [ Clear all ]                │
│ andreas-pc-a3f9c2   │ ┌────────────────────────────────────────────────────────────────────────────┐       │
│                     │ │ St Started       Job        Started by  Destinations  Duration   Up     ⋮  │       │
│ ▤  Dashboard        │ │────────────────────────────────────────────────────────────────────────────│       │
│ ⟳  Jobs             │ │ ⟳  02:00 today  Dev code   Schedule    ●●●  1/3      running    —      ›   │       │
│ ▣  Destinations     │ │────────────────────────────────────────────────────────────────────────────│       │
│ ⚿  Storage prov.    │ │ !  12 Mar 02:00 Dev code   Schedule    ●●●  2/3      4m 12s     842 M  ›   │       │
│ ↺  Restore          │ │────────────────────────────────────────────────────────────────────────────│       │
│ ≡  Activity         │ │ ✕  11 Mar 02:00 Dev code   Schedule    ●●●  0/3      0m 08s     0 B    ›   │       │
│                     │ │────────────────────────────────────────────────────────────────────────────│       │
│                     │ │ ✓  10 Mar 02:00 Dev code   Schedule    ●●●  3/3      3m 51s     1.1 G  ›   │       │
│                     │ │────────────────────────────────────────────────────────────────────────────│       │
│                     │ │ ✓  09 Mar 21:14 Dev code   Manual      ●●●  3/3      0m 44s     92 M   ›   │       │
│                     │ │────────────────────────────────────────────────────────────────────────────│       │
│                     │ │ –  09 Mar 02:00 Dev code   Schedule    –––  skipped  —          —      ›   │       │
│                     │ │                                        metered connection                  │       │
│                     │ └────────────────────────────────────────────────────────────────────────────┘       │
│                     │  superbackup keeps the last 200 runs. Older activity is in the event log.            │
│                     │                                                                                      │
│                     │  ── Events view ───────────────────────────────────────────────────────              │
│                     │ ┌────────────────────────────────────────────────────────────────────────────┐       │
│                     │ │ S Time              Event              Message                   Job       │       │
│                     │ │────────────────────────────────────────────────────────────────────────────│       │
│                     │ │ ● 12 Mar 02:04:16   job.finished       Completed with warnings   Dev code  │       │
│                     │ │ ! 12 Mar 02:03:58   destination.failed StorJ offsite unreachable Dev code  │       │
│                     │ │ ● 12 Mar 02:00:04   job.started        Triggered by schedule     Dev code  │       │
│                     │ │ ● 11 Mar 21:40:02   vault.locked       Auto-locked after 30 min  —         │       │
│                     │ └────────────────────────────────────────────────────────────────────────────┘       │
│                     │                                                                                      │
│                     │                                                                                      │
│────────────────────────────────────────────────────────────────────────────────────────────────────────────│
│ ● Daemon running │ Service installed │ Kopia 0.17.0 │ 02:04 Dev code completed with warnings               │
│◈ Unlocked  27 min   │                                                                                      │
└────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

### A-2 · Run detail — a partial failure

This screen is the reason `JobRun::derive_status()` exists. The summary card
states `Completed with warnings` and immediately explains that a destination did
not complete; the per-destination cards then carry the specifics.
`RunError.detail` is shown exactly as stored (the driver has already redacted
it) and the UI says so, so nobody wonders where the output went.

```
┌────────────────────────────────────────────────────────────────────────────────────────────────────────────┐
│  superbackup                                                                        _  □  ✕                │
│────────────────────────────────────────────────────────────────────────────────────────────────────────────│
│                     │ ← Activity / Dev code — 12 Mar 02:00              [ Retry this job ] [ Copy summary ]│
│────────────────────────────────────────────────────────────────────────────────────────────────────────────│
│ ANDREAS-PC          │ ┌────────────────────────────────────────────────────────────────────────────┐       │
│ andreas-pc-a3f9c2   │ │ [! Completed with warnings]   Dev code                                     │       │
│                     │ │ Status        Completed with warnings   Started    12 Mar 02:00:04         │       │
│ ▤  Dashboard        │ │  Some destinations did not          Finished   12 Mar 02:04:16             │       │
│ ⟳  Jobs             │ │  complete. See below.               Run id     3f9c2d1a…            [copy] │       │
│ ▣  Destinations     │ │ Started by    Schedule              Job id     a12b8e44…            [copy] │       │
│ ⚿  Storage prov.    │ │ Duration      4m 12s                                                       │       │
│ ↺  Restore          │ │ Destinations  2 succeeded, 1 failed                                        │       │
│ ≡  Activity         │ └────────────────────────────────────────────────────────────────────────────┘       │
│                     │ ┌────────────────────────────────────────────────────────────────────────────┐       │
│                     │ │ ▤ Local repo                          [✓ Succeeded]            2m 04s      │       │
│                     │ │ Snapshot   k9f2ab7c31de…  [copy]           [ Browse this snapshot ]        │       │
│                     │ │ Files      84,214 processed · 71,090 unchanged · 12 skipped                │       │
│                     │ │ Data       9.1 GB read · 842 MB uploaded                                   │       │
│                     │ │ Throughput 18.2 MB/s average                                               │       │
│                     │ │ ▸ 12 warnings                                                              │       │
│                     │ └────────────────────────────────────────────────────────────────────────────┘       │
│                     │ ┌────────────────────────────────────────────────────────────────────────────┐       │
│                     │ │ ☁ OneDrive                            [✓ Succeeded]            3m 12s      │       │
│                     │ │ Snapshot   b4c81e07aa92…  [copy]           [ Browse this snapshot ]        │       │
│                     │ └────────────────────────────────────────────────────────────────────────────┘       │
│                     │ ┌────────────────────────────────────────────────────────────────────────────┐       │
│                     │ │ ▣ StorJ offsite                       [✕ Failed]                0m 12s     │       │
│                     │ │ ✕ The endpoint answered, but rejected these credentials.                   │       │
│                     │ │   Error code: kopia · 12 Mar 02:03:58                                      │       │
│                     │ │ ┌──────────────────────────────────────────────────────────────────────┐   │       │
│                     │ │ │ i Check the keys on StorJ eu-1 (personal), then verify again.        │   │       │
│                     │ │ └──────────────────────────────────────────────────────────────────────┘   │       │
│                     │ │ ▸ Show technical details                                                   │       │
│                     │ │   Anything that looked like a credential has been removed.                 │       │
│                     │ │ [ Retry this job ] [ Copy details ] [ Open the destination ]               │       │
│                     │ └────────────────────────────────────────────────────────────────────────────┘       │
│────────────────────────────────────────────────────────────────────────────────────────────────────────────│
│ ● Daemon running │ Service installed │ Kopia 0.17.0 │ 02:04 Dev code completed with warnings               │
│◈ Unlocked  27 min   │                                                                                      │
└────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

---

## 7. Restore


### R-1 / R-2 · Restore source picker and snapshot list

Restore opens on a source picker, not a file tree, because "restore" starts
with "which copy". The retention line under the table is rendered from the
effective `RetentionPolicy`, so the absence of older snapshots is explained
rather than mysterious.

```
┌────────────────────────────────────────────────────────────────────────────────────────────────────────────┐
│  superbackup                                                                        _  □  ✕                │
│────────────────────────────────────────────────────────────────────────────────────────────────────────────│
│                     │ Restore                                                                              │
│────────────────────────────────────────────────────────────────────────────────────────────────────────────│
│ ANDREAS-PC          │ ┌───────────────────────┬────────────────────────────────────────────────────┐       │
│ andreas-pc-a3f9c2   │ │ RESTORE FROM          │ All | Today | This week | This month | [Date ▾]    │       │
│                     │ │                       │────────────────────────────────────────────────────│       │
│ ▤  Dashboard        │ │ Dev code              │ When            Folder          Files  Size   Id   │       │
│ ⟳  Jobs             │ │  ▤ Local repo         │────────────────────────────────────────────────────│       │
│ ▣  Destinations     │ │    18 snapshots       │ 12 Mar 02:00    C:\Users\…\dev  84,214 12 GB k9f2… │       │
│ ⚿  Storage prov.    │ │    newest 2 h ago     │  2 hours ago                                       │       │
│ ↺  Restore          │ │  ☁ OneDrive           │────────────────────────────────────────────────────│       │
│ ≡  Activity         │ │    18 snapshots       │ 11 Mar 02:00    C:\Users\…\dev  84,010 12 GB a71b… │       │
│                     │ │    newest 2 h ago     │  yesterday                    [ Compare with prev ]│       │
│                     │ │  ▣ StorJ offsite      │────────────────────────────────────────────────────│       │
│                     │ │    17 snapshots       │ 10 Mar 02:00    C:\Users\…\dev  83,884 12 GB 55cd… │       │
│                     │ │    newest 1 d ago     │────────────────────────────────────────────────────│       │
│                     │ │                       │ 09 Mar 02:00    C:\Users\…\dev  83,701 11 GB 2e90… │       │
│                     │ │ Documents             │────────────────────────────────────────────────────│       │
│                     │ │  ▤ Local repo         │ 08 Mar 02:00    C:\Users\…\dev  83,540 11 GB 7ab4… │       │
│                     │ │     9 snapshots       │────────────────────────────────────────────────────│       │
│                     │ │    newest yesterday   │ 01 Mar 02:00    C:\Users\…\dev  80,112 11 GB c3f1… │       │
│                     │ │                       │                                                    │       │
│                     │ │ Folder mirrors        │ Retention keeps 10 latest, 24 hourly, 14 daily,    │       │
│                     │ │  ⇄ Plain copy for Ana │ 8 weekly, 12 monthly and 3 annual snapshots.       │       │
│                     │ │    Open these in your │                                                    │       │
│                     │ │    file manager.      │                                                    │       │
│                     │ │      [ Open folder ]  │                                                    │       │
│                     │ │                       │                                                    │       │
│                     │ └───────────────────────┴────────────────────────────────────────────────────┘       │
│                     │                                                                                      │
│                     │   Left pane 280 px (220 px at 900). Rows 48 px. Right pane remainder.                │
│────────────────────────────────────────────────────────────────────────────────────────────────────────────│
│ ● Daemon running │ Service installed │ Kopia 0.17.0 │ 02:04 Dev code completed with warnings               │
│◈ Unlocked  27 min   │                                                                                      │
└────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

### R-3 · Snapshot browser

```
┌────────────────────────────────────────────────────────────────────────────────────────────────────────────┐
│  superbackup                                                                        _  □  ✕                │
│────────────────────────────────────────────────────────────────────────────────────────────────────────────│
│                     │ ← Restore / Local repo / 12 Mar 02:00  [ Filter ] (o ) Hidden [ Restore 3 items ]    │
│────────────────────────────────────────────────────────────────────────────────────────────────────────────│
│ ANDREAS-PC          │  ⌂ / Users / andreas / dev / web / src        Snapshot [ 12 Mar 02:00 ▾ ]            │
│ andreas-pc-a3f9c2   │ ┌────────────────────────────────────────────────────────────────────────────┐       │
│                     │ │ 3 items selected · about 1.2 GB       [ ▸ Show selection ]     [ Clear ]   │       │
│ ▤  Dashboard        │ └────────────────────────────────────────────────────────────────────────────┘       │
│ ⟳  Jobs             │ ┌────────────────────────────────────────────────────────────────────────────┐       │
│ ▣  Destinations     │ │ [-] Name                                       Size       Modified      ⋮  │       │
│ ⚿  Storage prov.    │ │────────────────────────────────────────────────────────────────────────────│       │
│ ↺  Restore          │ │ [■] ▸ components                               412 MB     11 Mar 18:22  ⋮  │       │
│ ≡  Activity         │ │ [ ] ▸ lib                                       88 MB     10 Mar 09:04  ⋮  │       │
│                     │ │ [ ] ▸ pages                                     12 MB     11 Mar 17:58  ⋮  │       │
│                     │ │ [ ] ▸ styles                                   1.2 MB     04 Mar 11:31  ⋮  │       │
│                     │ │ [x]   app.tsx                                  18.4 kB    11 Mar 18:22  ⋮  │       │
│                     │ │ [x]   index.tsx                                 2.1 kB    09 Mar 14:07  ⋮  │       │
│                     │ │ [ ]   vite.config.ts                             904 B    02 Mar 08:19  ⋮  │       │
│                     │ │ [ ]   tsconfig.json                              1.4 kB   02 Mar 08:19  ⋮  │       │
│                     │ │ [ ]   .env.local                                  212 B   11 Mar 18:20  ⋮  │       │
│                     │ │                                                                            │       │
│                     │ │                                                                            │       │
│                     │ │                                                                            │       │
│                     │ │                                                                            │       │
│                     │ │                                                                            │       │
│                     │ │                                                                            │       │
│                     │ │                                                                            │       │
│                     │ └────────────────────────────────────────────────────────────────────────────┘       │
│                     │                                                                                      │
│                     │   Flat, virtualised, single-level list with breadcrumb navigation —                  │
│                     │   NOT an expanding tree (DESIGN_SYSTEM.md L4). Fixed 28 px rows via                  │
│                     │   ScrollArea::show_rows, so a 400,000-entry folder costs the same as                 │
│                     │   a 10-entry one. [■] = whole subtree selected. [-] = mixed.                         │
│                     │   Alt+← / Alt+→ move through breadcrumb history.                                     │
│────────────────────────────────────────────────────────────────────────────────────────────────────────────│
│ ● Daemon running │ Service installed │ Kopia 0.17.0 │ 14:02 Browsing snapshot k9f2ab7c31de                 │
│◈ Unlocked  27 min   │                                                                                      │
└────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

### R-4 · Restore options

The only path in the product that writes over live user data, so it is the only
one besides vault destruction that requires typing a word. The primary button
becomes the Danger variant and changes its label to name the consequence.
No conflict option is preselected when restoring to the original location.

```
┌────────────────────────────────────────────────────────────────────────────────┐
│ Restore 3 items                                                             ✕  │
│═══════════════════════════════════════════════════════════════════════════════ │
│                                                                                │
│ 3 items · about 1.2 GB · from 12 Mar 02:00            [ ▸ Show the list ]      │
│                                                                                │
│ Where should these go?                                                         │
│  (●) Back to the original location                                             │
│      C:\Users\andreas\dev\web\src                                              │
│  ( ) To another folder                                                         │
│      [ C:\Users\andreas\Downloads\superbackup-restore-20260312-1402 ] [Browse] │
│      [x] Recreate the full folder structure                                    │
│                                                                                │
│ If a file already exists there                                                 │
│  ( ) Skip it            Leaves what is on disk untouched.                      │
│  (●) Overwrite it       Replaces the file on disk. This cannot be undone.      │
│  ( ) Keep both          Restores as “name (restored 12 Mar 14:02).ext”.        │
│                                                                                │
│ Also restore                                                                   │
│  [x] File timestamps          [x] Permissions and ownership                    │
│                                                                                │
│ 184 GB free at the destination.                                                │
│                                                                                │
│ Type overwrite to confirm  [ overwrite            ]                            │
│                                                                                │
│═══════════════════════════════════════════════════════════════════════════════ │
│                                  [ Cancel ]      [ Overwrite and restore ]     │
└────────────────────────────────────────────────────────────────────────────────┘
```

---

## 8. Settings


### S-5 · Settings — Security

```
┌────────────────────────────────────────────────────────────────────────────────────────────────────────────┐
│  superbackup                                                                        _  □  ✕                │
│────────────────────────────────────────────────────────────────────────────────────────────────────────────│
│                     │ Settings                                                                             │
│────────────────────────────────────────────────────────────────────────────────────────────────────────────│
│ ANDREAS-PC          │ ┌────────────────┬─────────────────────────────────────────────────────────┐         │
│ andreas-pc-a3f9c2   │ │ General        │ Vault                                                   │         │
│                     │ │ Scheduling     │ ┌─────────────────────────────────────────────────────┐ │         │
│ ▤  Dashboard        │ │ Bandwidth      │ │ ◈ Unlocked · locks in 27 minutes    [ Lock now ]    │ │         │
│ ⟳  Jobs             │ │ Notifications  │ └─────────────────────────────────────────────────────┘ │         │
│ ▣  Destinations     │ │ Security     ▸ │ Lock automatically after [  30 ] minutes                │         │
│ ⚿  Storage prov.    │ │ Kopia binary   │   Set to 0 to lock as soon as the window is closed.     │         │
│ ↺  Restore          │ │ Remote config  │   Auto-lock never happens while a backup is running.    │         │
│ ≡  Activity         │ │ Advanced       │                                                         │         │
│                     │ │ Reset          │ (o ) Store the vault key in Windows Credential Manager  │         │
│                     │ │                │      Unattended backups stop needing a person to type   │         │
│                     │ │                │      the passphrase. In exchange, anything that can run │         │
│                     │ │                │      programs as you can ask for the key.               │         │
│                     │ │                │ ─────────────────────────────────────────────────────── │         │
│                     │ │                │ [ Change master passphrase… ]                           │         │
│                     │ │                │ [ Export repository passphrases… ]                      │         │
│                     │ │                │   Writes every repository passphrase to a plain text    │         │
│                     │ │                │   file. The file is not encrypted.                      │         │
│                     │ │                │ ─────────────────────────────────────────────────────── │         │
│                     │ │                │ Vault backups                                           │         │
│                     │ │                │  config.sbvault.20260312-1402   4.1 kB   12 Mar 14:02   │         │
│                     │ │                │  config.sbvault.20260308-0911   4.0 kB   08 Mar 09:11   │         │
│                     │ │                │  [ Open folder ]  [ Restore a backup… ]                 │         │
│                     │ │                │ ─────────────────────────────────────────────────────── │         │
│                     │ │                │ ┌─────────────────────────────────────────────────────┐ │         │
│                     │ │                │ │ ! Danger zone                                       │ │         │
│                     │ │                │ │   [ Reset the vault and start over ]                │ │         │
│                     │ │                │ └─────────────────────────────────────────────────────┘ │         │
│                     │ └────────────────┴─────────────────────────────────────────────────────────┘         │
│                     │   Section list 200 px. Settings apply immediately; a 1.5 s “Saved”                   │
│                     │   confirmation appears beside the label. No global Save button.                      │
│────────────────────────────────────────────────────────────────────────────────────────────────────────────│
│ ● Daemon running │ Service installed │ Kopia 0.17.0 │ 14:02 Auto-lock timer reset                          │
│◈ Unlocked  27 min   │                                                                                      │
└────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

### S-3 · Settings — Bandwidth, with the daily window

```
┌────────────────────────────────────────────────────────────────────────────────────────────────────────────┐
│  superbackup                                                                        _  □  ✕                │
│────────────────────────────────────────────────────────────────────────────────────────────────────────────│
│                     │ Settings                                                                             │
│────────────────────────────────────────────────────────────────────────────────────────────────────────────│
│ ANDREAS-PC          │ ┌────────────────┬─────────────────────────────────────────────────────────┐         │
│ andreas-pc-a3f9c2   │ │ General        │ Upload limit    [x] [   2000 ] kB/s   approx 16 Mbit/s  │         │
│                     │ │ Scheduling     │ Download limit  [ ] [        ] kB/s   unlimited         │         │
│ ▤  Dashboard        │ │ Bandwidth    ▸ │   Downloads happen during restores and maintenance.     │         │
│ ⟳  Jobs             │ │ Notifications  │ ─────────────────────────────────────────────────────── │         │
│ ▣  Destinations     │ │ Security       │ ( o) Use a different limit during part of the day       │         │
│ ⚿  Storage prov.    │ │ Kopia binary   │   From [ 09 ]:[ 00 ]   To [ 18 ]:[ 00 ]                 │         │
│ ↺  Restore          │ │ Remote config  │   Days [Mo][Tu][We][Th][Fr][  ][  ]                     │         │
│ ≡  Activity         │ │ Advanced       │   Upload   [x] [    500 ] kB/s                          │         │
│                     │ │ Reset          │   Download [ ] [        ] kB/s                          │         │
│                     │ │                │                                                         │         │
│                     │ │                │  0    3    6    9    12   15   18   21   24             │         │
│                     │ │                │ ┌─────────────────────────────────────────────────────┐ │         │
│                     │ │                │ │░░░░░░░░░░░░░░░░░░│▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓│░░░░░░░░░░░░░░░│ │         │
│                     │ │                │ │  2,000 kB/s      │   500 kB/s       │  2,000 kB/s   │ │         │
│                     │ │                │ └─────────────────────────────────────────────────────┘ │         │
│                     │ │                │                                                         │         │
│                     │ │                │ Between 09:00 and 18:00 on weekdays, uploads are        │         │
│                     │ │                │ limited to 500 kB/s. Outside that window, uploads are   │         │
│                     │ │                │ limited to 2,000 kB/s.                                  │         │
│                     │ │                │                                                         │         │
│                     │ │                │ Limits are applied per destination, so two destinations │         │
│                     │ │                │ running at once can each use the full limit.            │         │
│                     │ │                │                                                         │         │
│                     │ │                │                                                         │         │
│                     │ │                │                                                         │         │
│                     │ └────────────────┴─────────────────────────────────────────────────────────┘         │
│                     │   24-hour strip: 819 × 40 px, hour ticks, window drawn as accent @ 30 %.             │
│                     │   A window crossing midnight renders as two blocks and says it wraps.                │
│────────────────────────────────────────────────────────────────────────────────────────────────────────────│
│ ● Daemon running │ Service installed │ Kopia 0.17.0 │ 14:05 Bandwidth window saved                         │
│◈ Unlocked  27 min   │                                                                                      │
└────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

### AB-1 · About

The Kopia attribution is mandatory and appears in three places: here, the
onboarding welcome footnote, and the diagnostic bundle's README. The third-party
list is generated at build time by `cargo-about`.

```
┌────────────────────────────────────────────────────────────────────────────────────────────────────────────┐
│  superbackup                                                                        _  □  ✕                │
│────────────────────────────────────────────────────────────────────────────────────────────────────────────│
│                     │ About                                                                                │
│────────────────────────────────────────────────────────────────────────────────────────────────────────────│
│ ANDREAS-PC          │                          ┌────────┐                                                  │
│ andreas-pc-a3f9c2   │                          │ ((o))  │                                                  │
│                     │                          └────────┘                                                  │
│ ▤  Dashboard        │                          superbackup                                                 │
│ ⟳  Jobs             │                       superbackup 0.1.0                                              │
│ ▣  Destinations     │                  windows-x86_64 · built 12 Mar 2026                                  │
│ ⚿  Storage prov.    │                Backups for machines full of code.                                    │
│ ↺  Restore          │                                                                                      │
│ ≡  Activity         │   Kopia                  0.17.0 · D:\tools\kopia\kopia.exe                           │
│                     │   Machine                ANDREAS-PC · andreas-pc-a3f9c2d1                            │
│                     │   Configuration format   1                                                           │
│                     │   Data folder            C:\Users\andreas\AppData\Local\superbackup                  │
│                     │                                              [ Open folder ]                         │
│                     │ ┌────────────────────────────────────────────────────────────────────┐               │
│                     │ │ Licences                                                           │               │
│                     │ │ superbackup is released under the MIT licence.    [ View licence ] │               │
│                     │ │                                                                    │               │
│                     │ │ superbackup uses Kopia, which is released under the Apache Licence │               │
│                     │ │ 2.0. Kopia is a separate program: superbackup runs it and does not │               │
│                     │ │ modify it.        [ View the Apache 2.0 licence ]   [ kopia.io ]   │               │
│                     │ │                                                                    │               │
│                     │ │ Inter and JetBrains Mono are used under the SIL Open Font Licence  │               │
│                     │ │ 1.1. Icons are from Lucide, under the ISC licence.                 │               │
│                     │ │                                                                    │               │
│                     │ │ ▸ Third-party licences            [ Copy all licence text ]        │               │
│                     │ └────────────────────────────────────────────────────────────────────┘               │
│                     │  [ Website ] [ Documentation ] [ Report an issue ] [ Release notes ]                 │
│                     │                                                                                      │
│                     │                     © 2026 Andreas Wiren                                             │
│────────────────────────────────────────────────────────────────────────────────────────────────────────────│
│ ● Daemon running │ Service installed │ Kopia 0.17.0 │ 14:05 Bandwidth window saved                         │
│◈ Unlocked  27 min   │                                                                                      │
└────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

---

## 9. Onboarding (880 × 640, no rail, no status strip)


### O-2 · Create your master passphrase

Left column 360 px, right column 400 px. Strength meter: four segments, 6 px
tall, 2 px gaps, full field width, with the band label to its right. The
advisory requirements use `○`, never a green tick — they cannot be verified, and
a tick would be a lie.

```
┌──────────────────────────────────────────────────────────────────────────────────────┐
│                                                                                      │
│                        ▬▬▬  ●  ●  ●  ●  ●  ●        Step 2 of 7                      │
│                                                                                      │
│  Create your master passphrase                                                       │
│                                                                                      │
│  This passphrase unlocks the vault      Master passphrase                            │
│  that holds your repository             [ ••••••••••••••••••••     ] [◉]             │
│  passphrases and storage keys. You                                                   │
│  will type it when you start            ▓▓▓▓▓▓▓▓ ▓▓▓▓▓▓▓▓ ▓▓▓▓▓▓▓▓ ░░░░░░░░  Good    │
│  superbackup, and when a backup                                                      │
│  needs to run unattended.               ✓ At least 12 characters                     │
│                                         ○ Not a password you use anywhere else       │
│  This is not the passphrase of any      ○ Four or more words is stronger than a      │
│  single backup repository. It is          short mix of symbols                       │
│  the one that protects all of them.                                                  │
│                                         Confirm passphrase                           │
│                                         [ ••••••••••••••••••••     ] [◉]             │
│                                                                                      │
│                                         [ Suggest a passphrase ]                     │
│                                                                                      │
│                                                                                      │
│                                                                                      │
│══════════════════════════════════════════════════════════════════════════════════════│
│  [ Back ]                                                          [ Continue ]      │
└──────────────────────────────────────────────────────────────────────────────────────┘
```

### O-3 · There is no recovery

A full step, not a modal and not a checkbox buried in O-2. `Continue` is
disabled until the checkbox is ticked, and a second mandatory checkbox appears
above it when the strength score was 0–1. **The vault is created when
`Continue` is pressed here**, not on O-2, so backing out never leaves a
half-initialised vault behind.

```
┌──────────────────────────────────────────────────────────────────────────────────────┐
│                                                                                      │
│                        ●  ▬▬▬  ●  ●  ●  ●  ●        Step 3 of 7                      │
│                                                                                      │
│  !  There is no way to recover this                                                  │
│                                                                                      │
│  Your master passphrase encrypts the vault on this machine. It is never sent         │
│  anywhere, and it is not stored in a form anyone can read.                           │
│                                                                                      │
│  That means there is no reset link, no backdoor and no support address that can      │
│  open your vault for you. If the passphrase is lost, the repository keys inside      │
│  are lost with it, and the backups they protect cannot be read again.                │
│                                                                                      │
│  Put it in a password manager now, or write it down and keep the paper somewhere     │
│  you would keep a spare key.                                                         │
│                                                                                      │
│  ┌────────────────────────────────────────────────────────────────────────────────┐  │
│  │ [ Copy passphrase to clipboard ]      [ Save a recovery sheet… ]               │  │
│  │ The clipboard is cleared after 60 seconds.                                     │  │
│  │ The recovery sheet is a plain text file. Anyone who can read the file can read │  │
│  │ the passphrase.                                                                │  │
│  └────────────────────────────────────────────────────────────────────────────────┘  │
│                                                                                      │
│  [ ] I have stored my master passphrase somewhere I can get to it. If I lose it,     │
│      my backups cannot be recovered.                                                 │
│                                                                                      │
│══════════════════════════════════════════════════════════════════════════════════════│
│  [ Back ]                                                          [ Continue ]      │
└──────────────────────────────────────────────────────────────────────────────────────┘
```

### O-4 · Scanning this machine

Three probes with a 12-second overall budget, results appearing as they arrive.
Rows are 64 px. A missing Kopia does **not** block onboarding — jobs simply
cannot run until it is resolved, and the dashboard says so.

```
┌──────────────────────────────────────────────────────────────────────────────────────┐
│                                                                                      │
│                        ●  ●  ▬▬▬  ●  ●  ●  ●        Step 4 of 7                      │
│                                                                                      │
│  Checking this machine                                                               │
│  Looking for the pieces superbackup can use.                                         │
│                                                                                      │
│  ┌────────────────────────────────────────────────────────────────────────────────┐  │
│  │ ! Kopia was not found                                                          │  │
│  │   superbackup uses Kopia to write and read backups. You can download a tested  │  │
│  │   build now, or point superbackup at a copy you already have.                  │  │
│  │                              [ Download Kopia ]   [ Choose a file… ]           │  │
│  │──────────────────────────────────────────────────────────────────────────────  │  │
│  │ ✓ OneDrive — andreas@example.com                                               │  │
│  │   C:\Users\andreas\OneDrive                                                    │  │
│  │   [x] Create a OneDrive destination here                                       │  │
│  │   A repository is a small number of large files, not the millions of small     │  │
│  │   ones that make OneDrive struggle. That is the whole point of putting it      │  │
│  │   here.                                                                        │  │
│  │──────────────────────────────────────────────────────────────────────────────  │  │
│  │ ✓ 184 GB free on D:                                                            │  │
│  └────────────────────────────────────────────────────────────────────────────────┘  │
│                                                                                      │
│  You can set this up later. Backups will not run until Kopia is available.           │
│                                                                                      │
│══════════════════════════════════════════════════════════════════════════════════════│
│  [ Back ]                                             [ Skip setup ]  [ Continue ]   │
└──────────────────────────────────────────────────────────────────────────────────────┘
```

### V-1 · Unlock modal (small, 420 px)

Blocking when reached by attempting a locked action: no `✕`, Escape does
nothing. Dismissible when opened from the rail. The error area is a reserved
20 px so the modal never jumps. The field is **not** cleared on a wrong
passphrase — it is selected, so retyping replaces it.

On success the modal closes, focus returns to the control that triggered the
unlock, and **if that control was an action it is performed automatically**.
This is the most important detail of the flow: the user's intent survives the
interruption.

```
┌───────────────────────────────────────────┐
│ ⚿  Unlock superbackup                     │
│═══════════════════════════════════════════│
│                                           │
│ Your master passphrase decrypts the       │
│ repository passphrases and storage keys   │
│ needed to run backups.                    │
│                                           │
│ Master passphrase                         │
│ [ ••••••••••••••••          ] [◉]         │
│                                           │
│ ✕ That passphrase did not open the vault. │
│   Passphrases are case sensitive.         │
│                                           │
│═══════════════════════════════════════════│
│                            [ Unlock ]     │
└───────────────────────────────────────────┘
```

---

## 10. Tray menus

Rendered by the OS. Widths are native; the frames below show structure and
ordering only. Disabled items stay visible so the menu's shape is stable.


### Tray menu — Idle

```
┌───────────────────────────────────────────────────┐
│ superbackup — Up to date              (disabled)  │
│ Last backup 2 hours ago               (disabled)  │
│───────────────────────────────────────────────────│
│ Back up now                                       │
│ Back up                                         ▸ │
│───────────────────────────────────────────────────│
│ Pause                                           ▸ │
│ Disable all jobs                              [ ] │
│───────────────────────────────────────────────────│
│ Open superbackup                                  │
│ Activity…                                         │
│ Settings…                                         │
│───────────────────────────────────────────────────│
│ Quit superbackup                                  │
└───────────────────────────────────────────────────┘
```

### Tray menu — while a job is running

The second header line updates at most once per second. With two or more active
runs it reads `2 backups running — 42%` and the per-run detail moves into the
`Stop “…” (42%)` item labels. `Stop all backups` is inserted when 2+ runs are
active.

`Stop` from the tray acts immediately and raises a notification saying what was
stopped — a modal the user cannot see is worse than no confirmation. Progress is
never a bar in the menu; no platform supports it reliably.

```
┌───────────────────────────────────────────────────────────┐
│ superbackup — Backing up                      (disabled)  │
│ Dev code — 42% · 18.2 MB/s · approx 3m left   (disabled)  │
│───────────────────────────────────────────────────────────│
│ Stop “Dev code”                                           │
│───────────────────────────────────────────────────────────│
│ Back up now                    (already running, disabled)│
│ Back up                                                 ▸ │
│    Dev code                          (running, disabled)  │
│    Documents                                              │
│    Photos                                                 │
│───────────────────────────────────────────────────────────│
│ Pause                                                   ▸ │
│    Current backups finish first         (header, disabled)│
│    1 hour / 2 hours / 4 hours / 8 hours / Until I resume  │
│ Disable all jobs                                      [ ] │
│───────────────────────────────────────────────────────────│
│ Open superbackup                                          │
│ Activity…                                                 │
│ Settings…                                                 │
│───────────────────────────────────────────────────────────│
│ Quit superbackup            (confirms: Quit and stop 1?)  │
└───────────────────────────────────────────────────────────┘
```

### Tray menu — vault locked

```
┌─────────────────────────────────────────────────────┐
│ superbackup — Needs attention          (disabled)   │
│ The vault is locked                    (disabled)   │
│──────────────────────────────────────────────────── │
│ Unlock…                                             │
│──────────────────────────────────────────────────── │
│ Back up now                 (vault locked, disabled)│
│ Back up                  ▸  (vault locked, disabled)│
│──────────────────────────────────────────────────── │
│ Pause                                            ▸  │
│ Disable all jobs                               [ ]  │
│──────────────────────────────────────────────────── │
│ Open superbackup                                    │
│ Activity…                                           │
│ Settings…                                           │
│──────────────────────────────────────────────────── │
│ Quit superbackup                                    │
└─────────────────────────────────────────────────────┘
```

### Tray menu — paused

`Back up now` stays **enabled**: a manual run is an explicit act, and pause is
about schedules. `Pause ›` is replaced in place by `Resume backups` plus
`Extend ›`, so the menu keeps its shape.

```
┌────────────────────────────────────────────────────┐
│ superbackup — Paused                   (disabled)  │
│ Paused until 18:20 — On the road       (disabled)  │
│────────────────────────────────────────────────────│
│ Back up now                                        │
│ Back up                                          ▸ │
│────────────────────────────────────────────────────│
│ Resume backups                                     │
│ Extend                                           ▸ │
│ Disable all jobs                               [ ] │
│────────────────────────────────────────────────────│
│ Open superbackup                                   │
│ Activity…                                          │
│ Settings…                                          │
│────────────────────────────────────────────────────│
│ Quit superbackup                                   │
└────────────────────────────────────────────────────┘
```

---

## 11. Tray icon states (32 × 32 design grid)

Full geometry is in `DESIGN_SYSTEM.md` §7. Each state is shape-distinct so it
survives the macOS monochrome template treatment.


### Tray icons

The 80° gap in the ring at the down-right position is where the pip sits, so the
pip never overlaps the ring stroke. `Idle` and `Paused` close the ring
(nothing pending); `Running`, `Attention` and `Failed` open it.
Running is a 12-frame animation at 80 ms per frame (≈1 revolution per second),
reduced to 3 frames at 500 ms when the OS reports reduced motion.

```
┌──────────────────────────────────────────────────────────────────────────────────────────┐
│   Idle              Running           Attention         Paused            Failed         │
│                                                                                          │
│   ▄▄▄▄▄▄▄           ▄▄▄▄▄▄▄           ▄▄▄▄▄▄▄           ▄▄▄▄▄▄▄           ▄▄▄▄▄▄▄        │
│  █       █         █       █         █       █         █ ██ ██ █         █       █       │
│  █   ●   █         █       █         █       █         █ ██ ██ █         █       █       │
│  █       █         █      ▟▛         █      ▄▄▖        █ ██ ██ █         █      ▄▄▖      │
│   ▀▀▀▀▀▀▀           ▀▀▀▀▀ ▟▛          ▀▀▀▀▀ ▐!▌         ▀▀▀▀▀▀▀           ▀▀▀▀▀ ▐✕▌      │
│                                             ▀▀▘                                ▀▀▘       │
│  closed ring       open ring,        open ring +       closed ring +      open ring +    │
│  + centre dot      rotating arc      amber pip “!”     two bars           red pip “✕”    │
│  neutral           accent blue       warning           neutral            danger         │
└──────────────────────────────────────────────────────────────────────────────────────────┘
```

---

## 12. Component measurements

The frames above approximate to 10 px per column. These are the normative
values; they repeat `DESIGN_SYSTEM.md` §8 so an implementer working from the
wireframes alone does not have to guess.


### Component measurements

```
┌────────────────────────────────────────────────────────────────────────────────────────┐
│ Button        30 h · radius 6 · padding 12 x, 8 icon gap · label 14/500                │
│               compact 26 h (table rows, toolbars) · onboarding primary 36 h            │
│ Icon button   30 × 30 · radius 6 · ghost · tooltip + AccessKit label mandatory         │
│ Text input    30 h · radius 6 · padding 10 x · focus ring 2 px INSET (inputs only)     │
│ Combo box     30 h · popup radius 14 · items 28 h · max popup 320 h                    │
│ Checkbox      16 × 16 · radius 4 · label 8 px right, part of the hit target            │
│ Toggle        36 × 20 · radius 10 · knob 16                                            │
│ Segmented     30 h · radius 6 · 3 px inner pad · segment padding 12 x                  │
│ Card          radius 10 · 1 px border.subtle · padding 16 · gutter 16                  │
│ Job card      401 × 96 (1100) / 795 × 96 (900) · 3 px status spine                     │
│ Table header  32 h · sticky · micro 11/500 in text.muted                               │
│ Table row     36 h standard · 28 h compact (snapshot browser, event log)               │
│ Row divider   1 px border.subtle, inset 12 px from both ends · NO zebra striping       │
│ Progress bar  6 h in cards · 8 h in run detail · radius = height / 2                   │
│               value lerped over 220 ms · indeterminate band 30 % over 1600 ms          │
│ Badge         20 h · radius 5 · padding 8 x · 14 px icon · label 12/500                │
│ Destination   24 h chip · radius 5 · bg.raised · 14 px kind icon                       │
│ Banner        min 44 h · radius 10 · padding 16 · 20 px icon                           │
│ Toast         360 w · min 52 h · radius 10 · bottom-right, 16 px inset, max 3 stacked  │
│               success 4 s · info 5 s · warning 8 s · danger never auto-dismisses       │
│ Modal         small 420 / medium 560 / large 760 · header 56 · footer 60 · radius 14   │
│ Empty state   max 420 w · 32 px icon · centred at 45 % of container height             │
│ Tooltip       radius 6 · padding 8 · max 280 w · 500 ms delay (0 for truncation)       │
│ Rail item     36 h · 12 px pad · 16 px icon · 12 px gap · 3 px selected marker         │
│ Key/value     label column 160 · row 28 h · 4 px vertical gap · no dividers            │
│ Focus ring    2 px border.focus, 2 px OUTSIDE the control, radius + 2, never animated  │
│ Hit target    minimum 30 × 30 · table rows are full-width targets                      │
└────────────────────────────────────────────────────────────────────────────────────────┘
```

### Toast stack (bottom-right of the content area)

360 px wide, 8 px gaps, stacked upward, maximum 3 — a fourth replaces the
oldest. Hovering pauses the dismiss timer. Identical toasts within 60 s are
suppressed and the existing toast's timer resets. Danger toasts never
auto-dismiss.

```
┌─────────────────────────────────────────┐
│ ┌──────────────────────────────────────┐│
│ │ ✕  Dev code failed          Retry  ✕ ││
│ │    The endpoint answered, but        ││
│ │    rejected these credentials.       ││
│ └──────────────────────────────────────┘│
│ ┌──────────────────────────────────────┐│
│ │ ✓  Documents finished              ✕ ││
│ │    44 MB uploaded in 1m 02s          ││
│ └──────────────────────────────────────┘│
└─────────────────────────────────────────┘
```

---

## 13. Focus order reference

egui's tab order is widget instantiation order, not visual order
(`DESIGN_SYSTEM.md` L6), so it must be written down per screen.

| Screen | Focus order |
|---|---|
| Dashboard | Rail items 1–8 → vault control → header `Back up now` → header `⋯` → each run panel (`Stop`) → jobs `Back up now` → jobs `⋯` → each job card (card → `⋯` → `Run now`) in grid reading order |
| Jobs list | Search → Group by → Filter → `New job` → table header cells (sortable) → each row (row → `Run now` → `⋯`) |
| Job editor | Back → tab segments → tab body in visual order → `Run now` → `⋯` → `Cancel` → `Save changes` |
| Job editor · Folders | Name → Description → Project → Tags → per source row (path, symlinks, one-fs, delete) → `Add folder…` |
| Job editor · Exclusions | `Select developer defaults` → `Clear all` → each preset (checkbox → pattern-count disclosure) → gitignore → cachedir → max-size checkbox → max-size value → custom patterns → effective-patterns disclosure |
| Wizard | Header `✕` → step body in visual order → `Back` → `Cancel` → `Continue` |
| Destination editor | Name → Kind → Enabled → kind-specific fields top to bottom → encryption disclosure → encryption controls → `Create repository` → retention → bandwidth → `Verify` → `Save changes` |
| Provider editor | Name → Notes → Type → Endpoint → Region → TLS → Path-style → Access key → Secret key → session-token checkbox → `Test connection` → `Save provider` |
| Restore browser | Breadcrumb segments → filter → hidden toggle → `Restore selected` → select-all → each row (checkbox → name → `⋯`) |
| Modals | First interactive element → … → footer secondary → footer primary. Focus is trapped and returns to the invoking control on close. |
| Settings | Section list → section body top to bottom |

