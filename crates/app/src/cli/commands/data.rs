//! Reading what has been backed up: snapshots, browse, restore.

use superbackup_core::error::ErrorCode;
use superbackup_core::ipc::protocol::{
    ConflictPolicy as WireConflict, EntryKind, Request, SnapshotInfo,
};
use superbackup_core::model::{Destination, Job};

use crate::cli::args::{BrowseArgs, ConflictPolicy, RestoreArgs, SnapshotsArgs};
use crate::cli::client::{reply, Daemon, Start};
use crate::cli::context::Ctx;
use crate::cli::format::{self, Cell, Colour, Column, Table};
use crate::cli::output::{CliError, CliResult, Outcome};
use crate::cli::{prompt, timespec};

use super::everyday::follow_run;
use super::objects::absolute;
use super::{destinations, resolve_destination, resolve_job};

// ---------------------------------------------------------------------------
// Choosing where to look
// ---------------------------------------------------------------------------

/// The repository destinations a job writes to.
///
/// Mirrors are excluded: a folder mirror is a plain copy with no snapshots in
/// it, so asking it for a snapshot list is a question it cannot answer.
fn repositories_of(daemon: &Daemon, job: &Job) -> CliResult<Vec<Destination>> {
    let all = destinations(daemon)?;
    let mine: Vec<Destination> = all
        .into_iter()
        .filter(|d| job.destination_ids.contains(&d.id) && d.kind.is_repository())
        .collect();
    if mine.is_empty() {
        return Err(CliError::new(
            ErrorCode::Validation,
            format!("{} writes to no repository, so it has no snapshots", job.name),
        )
        .with_hint(
            "Folder mirrors keep a plain copy rather than snapshots; open the folder directly.",
        ));
    }
    Ok(mine)
}

fn chosen_destinations(
    daemon: &Daemon,
    job: &Job,
    named: Option<&String>,
) -> CliResult<Vec<Destination>> {
    match named {
        Some(needle) => {
            let dest = resolve_destination(daemon, needle)?;
            if !job.destination_ids.contains(&dest.id) {
                return Err(CliError::usage(format!(
                    "{} does not write to {}",
                    job.name, dest.name
                )));
            }
            Ok(vec![dest])
        }
        None => repositories_of(daemon, job),
    }
}

fn list_snapshots(
    daemon: &Daemon,
    destination: &Destination,
    job: Option<&Job>,
    limit: u32,
) -> CliResult<Vec<SnapshotInfo>> {
    Ok(reply!(
        daemon,
        Request::SnapshotList {
            destination: destination.id.to_string(),
            job: job.map(|j| j.id.to_string()),
            limit,
        },
        Snapshots
    )?
    .snapshots)
}

// ---------------------------------------------------------------------------
// snapshots
// ---------------------------------------------------------------------------

pub fn snapshots(ctx: &mut Ctx, args: SnapshotsArgs) -> CliResult<Outcome> {
    let daemon = Daemon::connect(ctx, Start::Never)?;
    let job = resolve_job(&daemon, &args.job)?;
    let targets = chosen_destinations(&daemon, &job, args.destination.as_ref())?;

    // A `SnapshotInfo` records the job that made it, not the machine. Dropping
    // the job filter is therefore what "all machines" means here: everything
    // in the repository, including the snapshots other machines put there.
    let filter = if args.all_machines { None } else { Some(&job) };

    let mut found: Vec<(String, SnapshotInfo)> = Vec::new();
    for destination in &targets {
        let limit = args.limit.min(u32::MAX as usize) as u32;
        for snapshot in list_snapshots(&daemon, destination, filter, limit)? {
            found.push((destination.name.clone(), snapshot));
        }
    }
    // Newest first: `sort_by_key` cannot express a reversed key without
    // cloning, so reverse after sorting ascending.
    found.sort_by_key(|(_, s)| s.created_at);
    found.reverse();
    found.truncate(args.limit);

    let mut table = Table::new(vec![
        Column::new("taken"),
        Column::new("id"),
        Column::new("destination").flex(),
        Column::new("source").path(),
        Column::new("files").right(),
        Column::new("size").right(),
    ])
    .empty_note(if args.all_machines {
        format!("No snapshots at all in {}.", names_of(&targets))
    } else {
        format!("{} has taken no snapshots yet.", job.name)
    });

    for (destination, snapshot) in &found {
        let id = Cell::new(short_snapshot_id(&snapshot.id));
        table.push(vec![
            Cell::new(format::timestamp_local(snapshot.created_at)),
            if snapshot.incomplete {
                // An interrupted snapshot is restorable but is not a full
                // point-in-time copy, and a user picking one to restore from
                // needs to know that before they pick it.
                Cell::coloured(format!("{} (partial)", id.text), Colour::Yellow)
            } else {
                id
            },
            Cell::new(destination.clone()),
            Cell::new(snapshot.source_path.clone()),
            Cell::new(
                snapshot
                    .file_count
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| format::MISSING.to_string()),
            ),
            Cell::new(format::opt_bytes(snapshot.total_bytes)),
        ]);
    }
    ctx.ui.table(&table);
    Outcome::data(found.into_iter().map(|(_, s)| s).collect::<Vec<_>>())
}

fn names_of(destinations: &[Destination]) -> String {
    destinations.iter().map(|d| d.name.as_str()).collect::<Vec<_>>().join(", ")
}

/// Kopia snapshot ids are long hex strings. The first twelve characters are
/// plenty to recognise one, and the full id is in `--json` for anything that
/// needs to pass it back.
fn short_snapshot_id(id: &str) -> String {
    if id.len() <= 14 {
        id.to_string()
    } else {
        id.chars().take(12).collect()
    }
}

// ---------------------------------------------------------------------------
// Picking a snapshot
// ---------------------------------------------------------------------------

/// The snapshot a `--snapshot`/`--at` pair names, and where it lives.
struct Chosen {
    destination: Destination,
    snapshot: SnapshotInfo,
}

fn choose_snapshot(
    daemon: &Daemon,
    job: &Job,
    destination: Option<&String>,
    snapshot_id: Option<&String>,
    at: Option<&String>,
) -> CliResult<Chosen> {
    let targets = chosen_destinations(daemon, job, destination)?;

    if let Some(wanted) = snapshot_id {
        for destination in &targets {
            let all = list_snapshots(daemon, destination, None, 0)?;
            // Accept the short form printed by `snapshots`, but refuse an
            // abbreviation that matches more than one.
            let matches: Vec<&SnapshotInfo> =
                all.iter().filter(|s| s.id == *wanted || s.id.starts_with(wanted)).collect();
            match matches.len() {
                0 => {}
                1 => {
                    return Ok(Chosen {
                        destination: destination.clone(),
                        snapshot: matches[0].clone(),
                    })
                }
                _ => {
                    let ids: Vec<&str> = matches.iter().map(|s| s.id.as_str()).collect();
                    return Err(CliError::usage(format!(
                        "`{wanted}` matches {} snapshots: {}",
                        ids.len(),
                        ids.join(", ")
                    )));
                }
            }
        }
        return Err(CliError::usage(format!(
            "no snapshot with id `{wanted}` at {}",
            names_of(&targets)
        )));
    }

    let when = match at {
        Some(text) => Some(timespec::parse_at(text)?),
        None => None,
    };

    let mut best: Option<Chosen> = None;
    for destination in &targets {
        for snapshot in list_snapshots(daemon, destination, Some(job), 0)? {
            let usable = match when {
                // "at this time" means the newest snapshot that already
                // existed then. Picking the nearest in either direction would
                // happily restore a state from *after* the moment the user
                // asked to go back to.
                Some(when) => snapshot.created_at <= when,
                None => true,
            };
            if !usable {
                continue;
            }
            let better = match &best {
                Some(current) => snapshot.created_at > current.snapshot.created_at,
                None => true,
            };
            if better {
                best = Some(Chosen { destination: destination.clone(), snapshot });
            }
        }
    }

    best.ok_or_else(|| match when {
        Some(when) => CliError::new(
            ErrorCode::Validation,
            format!(
                "{} has no snapshot from {} or earlier",
                job.name,
                format::absolute_local(when)
            ),
        )
        .with_hint("Run `superbackup snapshots NAME` to see what there is."),
        None => CliError::new(
            ErrorCode::Validation,
            format!("{} has no snapshots to restore from", job.name),
        ),
    })
}

// ---------------------------------------------------------------------------
// restore
// ---------------------------------------------------------------------------

pub fn restore(ctx: &mut Ctx, args: RestoreArgs) -> CliResult<Outcome> {
    let daemon = Daemon::connect(ctx, Start::IfNeeded)?;
    let job = resolve_job(&daemon, &args.job)?;
    let chosen = choose_snapshot(
        &daemon,
        &job,
        args.destination.as_ref(),
        args.snapshot.as_ref(),
        args.at.as_ref(),
    )?;

    let target = absolute(&args.to);
    let paths: Vec<String> =
        if args.paths.is_empty() { vec![String::new()] } else { args.paths.clone() };
    let conflict = wire_conflict(args.on_conflict);

    // Say what is about to happen before it happens, every time. A restore
    // writes into a folder the user may already be using.
    ctx.ui.heading(if args.dry_run { "Would restore" } else { "About to restore" });
    let pad = 14;
    ctx.ui.field("Job", &job.name, pad);
    ctx.ui.field("From", &chosen.destination.name, pad);
    ctx.ui.field(
        "Snapshot",
        format!(
            "{} taken {}",
            short_snapshot_id(&chosen.snapshot.id),
            format::timestamp_local(chosen.snapshot.created_at)
        ),
        pad,
    );
    if chosen.snapshot.incomplete {
        ctx.ui.warn("that snapshot is marked incomplete: it was interrupted, so it is not a full point-in-time copy");
    }
    ctx.ui.field("Contents", describe_snapshot(&chosen.snapshot), pad);
    for path in &paths {
        ctx.ui.field(
            "Path",
            if path.is_empty() { "everything in the snapshot".to_string() } else { path.clone() },
            pad,
        );
    }
    ctx.ui.field("Into", target.display().to_string(), pad);
    ctx.ui.field("If a file exists", conflict_text(args.on_conflict), pad);
    ctx.ui.blank();

    if args.dry_run {
        ctx.ui.note("--dry-run: the daemon reports what it would write and writes nothing.");
    } else {
        confirm_restore(ctx, args.on_conflict)?;
    }

    let mut results = Vec::new();
    let mut all_ok = true;
    for path in &paths {
        // The protocol restores one path per request, so several `--path`
        // options become several runs. Each is reported separately rather than
        // rolled into a single number that would hide a partial failure.
        let started = reply!(
            daemon,
            Request::SnapshotRestore {
                destination: chosen.destination.id.to_string(),
                snapshot: chosen.snapshot.id.clone(),
                path: path.clone(),
                target: target.clone(),
                conflict,
                dry_run: args.dry_run,
            },
            Started
        )?;
        if let Some(note) = &started.note {
            ctx.ui.line(note);
        }
        let outcome =
            follow_run(ctx, &daemon, started.run_id, &format!("restore of {}", job.name))?;
        if !outcome.succeeded() {
            all_ok = false;
        }
        results.push(outcome.as_json());
    }

    if all_ok {
        if !args.dry_run {
            ctx.ui.line(format!("Restored into {}.", target.display()));
        }
        Outcome::data(results)
    } else {
        Outcome::negative(results)
    }
}

/// Confirmation policy for a restore.
///
/// `restore` has no `-y`, so demanding a prompt unconditionally would make it
/// impossible to script — and a scripted restore is exactly what somebody
/// rebuilding a machine needs. The rule is therefore about damage rather than
/// about the command: a restore that can overwrite existing files is confirmed
/// and fails under `--no-input`; one that cannot destroy anything prints its
/// plan and proceeds.
fn confirm_restore(ctx: &mut Ctx, policy: ConflictPolicy) -> CliResult<()> {
    let destructive = policy == ConflictPolicy::Overwrite;
    if !destructive && !ctx.can_prompt() {
        ctx.ui.note("Existing files will be left alone, so this is not being confirmed.");
        return Ok(());
    }
    if destructive {
        prompt::confirm(ctx, "This replaces files that already exist in the target folder", false)
    } else {
        prompt::confirm(ctx, "This writes files into the target folder", false)
    }
}

fn describe_snapshot(snapshot: &SnapshotInfo) -> String {
    let mut out = match snapshot.file_count {
        Some(files) => format!(
            "{} from {}",
            format::plural(files as usize, "file", "files"),
            snapshot.source_path
        ),
        None => snapshot.source_path.clone(),
    };
    if let Some(bytes) = snapshot.total_bytes {
        out.push_str(&format!(", {}", format::bytes(bytes)));
    }
    out
}

fn conflict_text(policy: ConflictPolicy) -> &'static str {
    match policy {
        ConflictPolicy::Skip => "leave the existing file alone",
        ConflictPolicy::Overwrite => "replace it",
        ConflictPolicy::KeepBoth => "write the restored one alongside it",
        ConflictPolicy::Fail => "stop the restore",
    }
}

fn wire_conflict(policy: ConflictPolicy) -> WireConflict {
    match policy {
        ConflictPolicy::Skip => WireConflict::Skip,
        ConflictPolicy::Overwrite => WireConflict::Overwrite,
        ConflictPolicy::KeepBoth => WireConflict::KeepBoth,
        ConflictPolicy::Fail => WireConflict::Fail,
    }
}

// ---------------------------------------------------------------------------
// browse
// ---------------------------------------------------------------------------

/// A ceiling on a recursive listing.
///
/// A snapshot of a developer's home directory has millions of entries. Walking
/// all of them over IPC to print them into a terminal helps nobody, so the
/// walk stops and says it stopped.
const MAX_RECURSIVE_ENTRIES: usize = 5_000;

pub fn browse(ctx: &mut Ctx, args: BrowseArgs) -> CliResult<Outcome> {
    let daemon = Daemon::connect(ctx, Start::Never)?;
    let job = resolve_job(&daemon, &args.job)?;
    let chosen = choose_snapshot(
        &daemon,
        &job,
        args.destination.as_ref(),
        args.snapshot.as_ref(),
        args.at.as_ref(),
    )?;

    let root = normalise_browse_path(&args.path);
    ctx.ui.note(format!(
        "{} taken {} at {}",
        short_snapshot_id(&chosen.snapshot.id),
        format::timestamp_local(chosen.snapshot.created_at),
        chosen.destination.name
    ));

    let mut rows: Vec<(String, superbackup_core::ipc::protocol::SnapshotEntry)> = Vec::new();
    let mut queue = vec![root.clone()];
    let mut truncated = false;

    while let Some(path) = queue.pop() {
        let listing = reply!(
            daemon,
            Request::SnapshotBrowse {
                destination: chosen.destination.id.to_string(),
                snapshot: chosen.snapshot.id.clone(),
                path: path.clone(),
            },
            Listing
        )?;
        if listing.truncated {
            truncated = true;
        }
        for entry in listing.entries {
            let full = join(&path, &entry.name);
            if args.recursive && entry.kind == EntryKind::Directory {
                queue.push(full.clone());
            }
            rows.push((full, entry));
            if rows.len() >= MAX_RECURSIVE_ENTRIES {
                truncated = true;
                queue.clear();
                break;
            }
        }
    }
    rows.sort_by(|a, b| a.0.cmp(&b.0));

    let mut table = Table::new(vec![
        Column::new("modified"),
        Column::new("size").right(),
        Column::new("name").path(),
    ])
    .empty_note(format!("{} is empty in this snapshot.", display_path(&root)));

    for (full, entry) in &rows {
        let shown = if args.recursive { full.clone() } else { entry.name.clone() };
        table.push(vec![
            Cell::new(format::opt_timestamp_local(entry.modified_at)),
            match entry.kind {
                EntryKind::Directory => Cell::coloured("<dir>", Colour::Blue),
                EntryKind::Symlink => Cell::coloured("<link>", Colour::Dim),
                EntryKind::File => Cell::new(format::bytes(entry.size_bytes)),
            },
            Cell::new(shown),
        ]);
    }
    ctx.ui.table(&table);
    if truncated {
        ctx.ui.warn(format!(
            "the listing was cut short after {} entries; browse a subdirectory to see the rest",
            rows.len()
        ));
    }

    Outcome::data(serde_json::json!({
        "snapshot": chosen.snapshot,
        "path": root,
        "truncated": truncated,
        "entries": rows.iter().map(|(_, e)| e).collect::<Vec<_>>(),
    }))
}

/// The protocol wants `/`-separated with `""` for the root; the CLI's default
/// argument is `/`.
fn normalise_browse_path(path: &str) -> String {
    path.replace('\\', "/").trim_matches('/').to_string()
}

fn display_path(path: &str) -> String {
    if path.is_empty() {
        "the snapshot root".to_string()
    } else {
        path.to_string()
    }
}

fn join(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};

    #[test]
    fn browse_paths_are_normalised_for_the_protocol() {
        assert_eq!(normalise_browse_path("/"), "");
        assert_eq!(normalise_browse_path("/src/cli/"), "src/cli");
        assert_eq!(normalise_browse_path(r"src\cli"), "src/cli");
    }

    #[test]
    fn a_short_snapshot_id_stays_recognisable() {
        assert_eq!(short_snapshot_id("abc123"), "abc123");
        assert_eq!(short_snapshot_id("k0123456789abcdef0123"), "k0123456789a");
    }

    fn info(at: DateTime<Utc>) -> SnapshotInfo {
        SnapshotInfo {
            id: "x".into(),
            destination_id: uuid::Uuid::new_v4(),
            job_id: None,
            created_at: at,
            source_path: "/src".into(),
            file_count: Some(3),
            total_bytes: Some(1_500_000),
            incomplete: false,
            tags: vec![],
        }
    }

    #[test]
    fn a_snapshot_description_survives_missing_counts() {
        let mut s = info(Utc::now());
        assert!(describe_snapshot(&s).contains("3 files"));
        s.file_count = None;
        s.total_bytes = None;
        assert_eq!(describe_snapshot(&s), "/src");
    }
}
