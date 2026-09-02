//! Snapshots: creating them, listing them, browsing them, restoring them.
//!
//! Flag names in this module were read out of kopia's own source rather than
//! its documentation; the file each came from is named at the method that uses
//! it. Where kopia offers no flag for something superbackup's model expresses,
//! that is stated explicitly instead of being approximated.

use super::command::RunContext;
use super::driver::{KopiaDriver, KopiaResult};
use super::error::{KopiaError, KopiaFailure};
use super::manifest::{DirEntry, DirManifest, SnapshotManifest};
use super::progress::parse_bytes;
use crate::model::Source;
use crate::state::Progress;
use std::path::Path;

/// Knobs for one `snapshot create`.
#[derive(Debug, Clone, Default)]
pub struct SnapshotOptions {
    /// Free-form label stored on the snapshot, shown in the restore browser.
    pub description: Option<String>,
    /// `key:value` pairs, e.g. `job:nightly-code`. Lets a human (or a support
    /// engineer) tell superbackup's snapshots apart from hand-made ones.
    pub tags: Vec<(String, String)>,
    /// Files hashed in parallel. `None` leaves kopia's own choice, which scales
    /// with the CPU count.
    pub parallel: Option<u32>,
    /// Abort on the first unreadable file instead of skipping it. Off by
    /// default: one locked file must not cost the user their nightly backup.
    pub fail_fast: bool,
    /// Stop after this many megabytes have been uploaded, checkpointing what
    /// was done. The honest way to back up a terabyte over a metered link.
    pub upload_limit_mb: Option<u64>,
    /// Pin the snapshot so retention never expires it.
    pub pin: Option<String>,
}

/// What a finished `snapshot create` produced.
#[derive(Debug, Clone, Default)]
pub struct SnapshotOutcome {
    /// The full manifest, when kopia emitted parseable JSON.
    pub manifest: Option<SnapshotManifest>,
    /// Convenience: [`crate::state::DestinationRun::snapshot_id`].
    pub snapshot_id: Option<String>,
    /// Final counters, corrected from the manifest where it had them.
    pub progress: Progress,
    /// [`crate::state::DestinationRun::warnings`], from the manifest and from
    /// kopia's stderr.
    pub warnings: Vec<String>,
    /// Things worth saying that are not problems — what the ignore rules kept
    /// out, above all. Kept apart from `warnings` so the configuration working
    /// as written cannot colour a run amber.
    pub notes: Vec<String>,
    /// Bytes that did not have to be sent because kopia already had them.
    pub deduplicated_bytes: u64,
    /// True when kopia produced a checkpoint rather than a complete snapshot,
    /// which is what an upload limit or a mid-run interruption yields.
    pub incomplete: bool,
}

/// What `snapshot estimate` reported.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SnapshotEstimate {
    pub included_files: u64,
    pub included_bytes: u64,
    pub excluded_files: u64,
    pub excluded_bytes: u64,
    pub excluded_directories: u64,
    pub errors: u64,
}

/// Where a restore should write, and how carefully.
#[derive(Debug, Clone)]
pub struct RestoreOptions {
    /// Overwrite files that already exist in the target.
    pub overwrite_files: bool,
    /// Skip entries that already exist, for resuming a large restore.
    pub skip_existing: bool,
    /// Delete files in the target that the snapshot does not contain. Dangerous
    /// and therefore off by default.
    pub delete_extra: bool,
    /// Keep going past unreadable entries.
    pub ignore_errors: bool,
    /// Write each file to a temporary name and rename it into place, so an
    /// interrupted restore never leaves a half-written file that looks intact.
    pub write_files_atomically: bool,
    pub skip_permissions: bool,
    pub skip_owners: bool,
    pub parallel: Option<u32>,
    /// Restore to an archive instead of loose files. `None` means a plain
    /// filesystem restore.
    pub archive: Option<RestoreArchive>,
}

impl Default for RestoreOptions {
    fn default() -> Self {
        RestoreOptions {
            // Kopia's own defaults for these two are `true`; superbackup does
            // not silently overwrite a user's files, so both are inverted here
            // and the GUI asks.
            overwrite_files: false,
            skip_existing: false,
            delete_extra: false,
            ignore_errors: false,
            write_files_atomically: true,
            skip_permissions: false,
            skip_owners: false,
            parallel: None,
            archive: None,
        }
    }
}

/// `restore --mode` values, from `cli/command_restore.go`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreArchive {
    Zip,
    ZipUncompressed,
    Tar,
    TarGz,
}

impl RestoreArchive {
    fn kopia_mode(&self) -> &'static str {
        match self {
            RestoreArchive::Zip => "zip",
            RestoreArchive::ZipUncompressed => "zip-nocompress",
            RestoreArchive::Tar => "tar",
            RestoreArchive::TarGz => "tgz",
        }
    }
}

/// Outcome of a restore.
#[derive(Debug, Clone, Default)]
pub struct RestoreOutcome {
    pub progress: Progress,
    pub warnings: Vec<String>,
    /// Kopia's own closing summary line, for the results screen.
    pub summary: String,
}

impl KopiaDriver {
    /// Back up one source.
    ///
    /// Flags verified against `cli/command_snapshot_create.go`: `--json`,
    /// `--json-verbose` (hidden, in `cli/json_output.go`), `--description`,
    /// `--tags`, `--parallel`, `--fail-fast`, `--upload-limit-mb`, `--pin`.
    /// `--progress` is an *application* flag from `cli/cli_progress.go` and
    /// must precede the subcommand, which
    /// [`super::command::KopiaCommand::global`] guarantees.
    ///
    /// `--progress` is passed unconditionally and deliberately: kopia defaults
    /// it to *off* when stdout is not a terminal, and stdout is a pipe here, so
    /// omitting it would leave the GUI's progress bar frozen for the whole run.
    ///
    /// `--json-verbose` is equally load-bearing. Without it `cli/json_output.go`
    /// strips `stats` from the manifest and the run history would record a
    /// snapshot id and no sizes at all.
    ///
    /// [`Source::follow_symlinks`] has no equivalent in kopia, which never
    /// follows symlinks out of a source tree; a source that *is* a symlink is
    /// resolved by `filepath.Abs` before the walk begins.
    /// [`Source::one_filesystem`] is a policy setting rather than a snapshot
    /// flag — see [`KopiaDriver::apply_source_policy`].
    pub async fn create_snapshot(
        &self,
        source: &Source,
        options: &SnapshotOptions,
        ctx: &RunContext,
    ) -> KopiaResult<SnapshotOutcome> {
        self.require_passphrase("snapshot create")?;

        let mut cmd = self.base();
        cmd.global_bool("progress", true);
        cmd.command("snapshot").command("create").arg(&source.path);
        cmd.switch("json");
        if self.binary().supports_json_verbose() {
            cmd.switch("json-verbose");
        }
        if let Some(d) = &options.description {
            cmd.flag("description", d);
        }
        cmd.repeated("tags", options.tags.iter().map(|(k, v)| format!("{k}:{v}")));
        if let Some(p) = options.parallel {
            cmd.flag("parallel", p.to_string());
        }
        if options.fail_fast {
            cmd.switch("fail-fast");
        }
        if let Some(mb) = options.upload_limit_mb {
            cmd.flag("upload-limit-mb", mb.to_string());
        }
        if let Some(pin) = &options.pin {
            cmd.flag("pin", pin);
        }
        // A snapshot report notification is kopia's business, not ours; the
        // engine owns user-facing notifications.
        cmd.flag_bool("send-snapshot-report", false);

        // Seed the current path so the GUI has something to show during the
        // seconds before the first progress frame arrives.
        let mut ctx = ctx.clone();
        if ctx.current_path.is_none() {
            ctx.current_path = Some(source.path.display().to_string());
        }

        let out = self.run(cmd, &ctx).await?;

        let manifests: Vec<SnapshotManifest> = super::manifest::parse_json_stream(&out.stdout);
        let manifest = manifests.into_iter().next_back();

        let mut progress = out.progress;
        let mut warnings = out.warnings;
        let mut notes: Vec<String> = Vec::new();
        let mut deduplicated = 0;
        let mut incomplete = false;

        if let Some(m) = &manifest {
            if let Some((files, bytes)) = m.totals() {
                let ignored = m
                    .stats
                    .as_ref()
                    .map(|s| s.ignored_error_count)
                    .unwrap_or(progress.errors_ignored);
                // Replace the rounded progress-line numbers with the exact ones.
                progress.files_processed = files;
                progress.files_total = Some(files);
                progress.bytes_processed = bytes;
                // Replace kopia's estimate with the measured total; keeping a
                // larger estimate would leave the finished bar short of 100%.
                progress.bytes_total = Some(bytes);
                progress.errors_ignored = ignored;
                progress.estimated_seconds_remaining = Some(0);
                progress.current_path = None;
            }
            deduplicated = m.deduplicated_bytes(progress.bytes_uploaded);
            incomplete = !m.is_complete();
            warnings.extend(m.warnings());
            if let Some(excluded) = m.exclusions() {
                notes.push(format!(
                    "{} files and {} folders were left out by this job's exclusions ({}).",
                    excluded.files,
                    excluded.directories,
                    bytesize::ByteSize(excluded.bytes)
                ));
            }
        } else if !out.stdout.trim().is_empty() {
            warnings.push(
                "kopia did not report a machine-readable snapshot summary; the backup completed \
                 but its statistics are approximate."
                    .to_string(),
            );
        }

        warnings.sort();
        warnings.dedup();
        notes.sort();
        notes.dedup();

        Ok(SnapshotOutcome {
            snapshot_id: manifest.as_ref().map(|m| m.id.clone()).filter(|s| !s.is_empty()),
            notes,
            manifest,
            progress,
            warnings,
            deduplicated_bytes: deduplicated,
            incomplete,
        })
    }

    /// List snapshots.
    ///
    /// `cli/command_snapshot_list.go`: the optional positional argument filters
    /// by source, `--all` crosses the user/host boundary, `-i/--incomplete`
    /// includes checkpoints. `--json` emits a real JSON array, and
    /// `--json-verbose` is needed here too if the caller wants sizes.
    ///
    /// `all_machines` is what the restore browser wants: a repository shared by
    /// several PCs holds snapshots this machine did not make, and hiding them
    /// would make half the user's backups invisible.
    pub async fn list_snapshots(
        &self,
        source: Option<&Path>,
        all_machines: bool,
        ctx: &RunContext,
    ) -> KopiaResult<Vec<SnapshotManifest>> {
        self.require_passphrase("snapshot list")?;
        let mut cmd = self.base();
        cmd.command("snapshot").command("list");
        if let Some(s) = source {
            cmd.arg(s);
        }
        if all_machines {
            cmd.switch("all");
        }
        cmd.switch("json");
        if self.binary().supports_json_verbose() {
            cmd.switch("json-verbose");
        }
        // Checkpoints are real recovery points and the browser must offer them.
        cmd.switch("incomplete");
        let out = self.run(cmd, ctx).await?;
        Ok(super::manifest::parse_snapshot_list(&out.stdout))
    }

    /// Delete one snapshot.
    ///
    /// `cli/command_snapshot_delete.go` requires an explicit `--delete` to
    /// confirm; without it kopia performs a dry run and reports what it would
    /// have done. That is a good safety design and it is preserved here: the
    /// `confirm` parameter is the caller stating, in code, that a human agreed.
    pub async fn delete_snapshot(
        &self,
        snapshot_id: &str,
        confirm: bool,
        ctx: &RunContext,
    ) -> KopiaResult<()> {
        self.require_passphrase("snapshot delete")?;
        if snapshot_id.trim().is_empty() {
            return Err(KopiaError::local("snapshot delete", KopiaFailure::Unusable, None)
                .with_message("No snapshot was selected for deletion."));
        }
        let mut cmd = self.base();
        cmd.command("snapshot").command("delete").arg(snapshot_id);
        if confirm {
            cmd.switch("delete");
        }
        self.run(cmd, ctx).await?;
        Ok(())
    }

    /// Estimate what a snapshot of `path` would cost, before running one.
    ///
    /// `cli/command_snapshot_estimate.go` has no `--json`; it prints
    ///
    /// ```text
    /// Snapshot includes 16517 file(s), total size 6.5 GB
    /// Snapshot excludes 40311 file(s), total size 918.3 MB
    /// Snapshot excludes 812 directories. Examples:
    /// ```
    ///
    /// `--quiet` suppresses the per-directory `Analyzing …` chatter, which would
    /// otherwise be one stderr line per directory in the tree.
    ///
    /// The estimate honours the policy already stored in the repository, so
    /// calling it *after* [`KopiaDriver::apply_source_policy`] is what makes the
    /// exclusion preview in the job wizard truthful.
    pub async fn estimate_snapshot(
        &self,
        path: &Path,
        ctx: &RunContext,
    ) -> KopiaResult<SnapshotEstimate> {
        self.require_passphrase("snapshot estimate")?;
        let mut cmd = self.base();
        cmd.command("snapshot").command("estimate").arg(path).switch("quiet");
        let out = self.run(cmd, ctx).await?;
        Ok(parse_estimate(&out.stdout))
    }

    /// Restore a snapshot, a subtree of one, or a single file.
    ///
    /// `source` is anything `cli/command_restore.go` accepts: a snapshot
    /// manifest id, an object id, or either of those followed by a path inside
    /// the snapshot (`k1a2b3c4/src/main.rs`).
    ///
    /// Every flag below is from that file. Note the inversions: kopia defaults
    /// `--overwrite-files`, `--overwrite-directories` and `--overwrite-symlinks`
    /// to `true`, which is the wrong default for a desktop restore, so all
    /// three are set explicitly from [`RestoreOptions::overwrite_files`].
    pub async fn restore(
        &self,
        source: &str,
        target: &Path,
        options: &RestoreOptions,
        ctx: &RunContext,
    ) -> KopiaResult<RestoreOutcome> {
        self.require_passphrase("restore")?;
        if source.trim().is_empty() {
            return Err(KopiaError::local("restore", KopiaFailure::Unusable, None)
                .with_message("No snapshot or file was selected to restore."));
        }

        let mut cmd = self.base();
        cmd.global_bool("progress", true);
        cmd.command("restore").arg(source).arg(target);
        cmd.flag_bool("overwrite-files", options.overwrite_files)
            .flag_bool("overwrite-directories", options.overwrite_files)
            .flag_bool("overwrite-symlinks", options.overwrite_files)
            .flag_bool("write-files-atomically", options.write_files_atomically);
        if options.skip_existing {
            cmd.switch("skip-existing");
        }
        if options.delete_extra {
            cmd.switch("delete-extra");
        }
        if options.ignore_errors {
            cmd.switch("ignore-errors");
        }
        if options.skip_permissions {
            cmd.switch("skip-permissions");
        }
        if options.skip_owners {
            cmd.switch("skip-owners");
        }
        if let Some(p) = options.parallel {
            cmd.flag("parallel", p.to_string());
        }
        if let Some(a) = options.archive {
            cmd.flag("mode", a.kopia_mode());
        }

        let mut ctx = ctx.clone();
        if ctx.current_path.is_none() {
            ctx.current_path = Some(target.display().to_string());
        }
        let out = self.run(cmd, &ctx).await?;

        Ok(RestoreOutcome {
            progress: out.progress,
            warnings: out.warnings,
            summary: out
                .stderr_tail
                .lines()
                .rev()
                .find(|l| l.contains("Restored "))
                .unwrap_or("")
                .trim()
                .to_string(),
        })
    }

    /// List the entries of one directory inside a snapshot, for the restore
    /// browser.
    ///
    /// This uses `kopia show <path>` rather than `kopia ls`, and the choice
    /// matters. `cli/command_ls.go` prints a fixed-width human table
    /// (`"%v %12s %v %-34v %v%v"`) with no `--json`, so a restore browser built
    /// on it would be a column-position parser — brittle against a locale, a
    /// long object id, or a filename containing spaces. `cli/command_show.go`
    /// copies the raw object bytes to stdout, and a kopia directory object *is*
    /// a `snapshot.DirManifest` JSON document, so this yields the real
    /// structure: name, type, size, modification time and object id per entry.
    ///
    /// `object_path` accepts the same syntax as `kopia ls`: a snapshot id, an
    /// object id, or either followed by a path inside the snapshot.
    pub async fn list_directory(
        &self,
        object_path: &str,
        ctx: &RunContext,
    ) -> KopiaResult<Vec<DirEntry>> {
        self.require_passphrase("show")?;
        if object_path.trim().is_empty() {
            return Err(KopiaError::local("show", KopiaFailure::Unusable, None)
                .with_message("No snapshot directory was selected."));
        }
        let mut cmd = self.base();
        cmd.command("show").arg(object_path);
        let out = self.run(cmd, ctx).await?;

        let manifest: DirManifest = serde_json::from_str(out.stdout.trim()).map_err(|e| {
            KopiaError::local(
                "show",
                KopiaFailure::Unknown,
                Some(format!("that entry is not a directory ({e})")),
            )
            .with_message("That item cannot be browsed; only directories can be opened.")
        })?;

        let mut entries = manifest.entries;
        // Directories first, then case-insensitive by name — what a file
        // browser is expected to do, and kopia's own order is byte-wise.
        entries.sort_by(|a, b| {
            b.entry_type
                .is_dir()
                .cmp(&a.entry_type.is_dir())
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(entries)
    }

    /// Convenience for the browser's root view: every snapshot for this
    /// destination, newest first.
    pub async fn browse_roots(&self, ctx: &RunContext) -> KopiaResult<Vec<SnapshotManifest>> {
        let mut all = self.list_snapshots(None, true, ctx).await?;
        all.sort_by_key(|m| std::cmp::Reverse(m.start_time));
        Ok(all)
    }
}

/// Parse the free-form output of `snapshot estimate`.
///
/// Tolerant by construction: a missing line leaves its counter at zero rather
/// than failing, because an estimate is advisory and refusing to show one over
/// a reworded sentence would be absurd.
fn parse_estimate(stdout: &str) -> SnapshotEstimate {
    let mut est = SnapshotEstimate::default();
    for line in stdout.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Snapshot includes ") {
            if let Some((files, bytes)) = split_count_and_size(rest) {
                est.included_files = files;
                est.included_bytes = bytes;
            }
        } else if let Some(rest) = line.strip_prefix("Snapshot excludes ") {
            if rest.starts_with("no files") || rest.starts_with("no directories") {
                continue;
            }
            if let Some((files, bytes)) = split_count_and_size(rest) {
                est.excluded_files = files;
                est.excluded_bytes = bytes;
            } else if let Some(dirs) = rest.split_whitespace().next().and_then(|n| n.parse().ok()) {
                est.excluded_directories = dirs;
            }
        } else if let Some(rest) = line.strip_prefix("Encountered ") {
            est.errors = rest.split_whitespace().next().and_then(|n| n.parse().ok()).unwrap_or(0);
        }
    }
    est
}

/// `16517 file(s), total size 6.5 GB` -> `(16517, 6_500_000_000)`.
fn split_count_and_size(s: &str) -> Option<(u64, u64)> {
    let (count, rest) = s.split_once(" file(s), total size ")?;
    Some((count.trim().parse().ok()?, parse_bytes(rest)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ESTIMATE: &str = "\
Analyzing C:\\src...
Snapshot includes 16517 file(s), total size 6.5 GB
Snapshot excludes 40311 file(s), total size 918.3 MB
Snapshot excludes 812 directories. Examples:
 - C:\\src\\a\\node_modules
Encountered 3 error(s).

Estimated upload time: 1h27m6s at 10 Mbit/s
";

    #[test]
    fn estimate_output_is_parsed() {
        let e = parse_estimate(ESTIMATE);
        assert_eq!(e.included_files, 16517);
        assert_eq!(e.included_bytes, 6_500_000_000);
        assert_eq!(e.excluded_files, 40311);
        assert_eq!(e.excluded_bytes, 918_300_000);
        assert_eq!(e.excluded_directories, 812);
        assert_eq!(e.errors, 3);
    }

    #[test]
    fn estimate_with_nothing_excluded() {
        let e = parse_estimate(
            "Snapshot includes 5 file(s), total size 1 KB\nSnapshot excludes no files.\nSnapshot excludes no directories.\n",
        );
        assert_eq!(e.included_files, 5);
        assert_eq!(e.excluded_files, 0);
        assert_eq!(e.excluded_directories, 0);
    }

    #[test]
    fn estimate_of_unexpected_text_is_empty_not_a_panic() {
        assert_eq!(parse_estimate("who knows\n"), SnapshotEstimate::default());
        assert_eq!(parse_estimate(""), SnapshotEstimate::default());
    }

    #[test]
    fn archive_modes_match_kopias_enum() {
        assert_eq!(RestoreArchive::Zip.kopia_mode(), "zip");
        assert_eq!(RestoreArchive::ZipUncompressed.kopia_mode(), "zip-nocompress");
        assert_eq!(RestoreArchive::Tar.kopia_mode(), "tar");
        assert_eq!(RestoreArchive::TarGz.kopia_mode(), "tgz");
    }

    #[test]
    fn restore_defaults_do_not_overwrite() {
        let d = RestoreOptions::default();
        assert!(!d.overwrite_files, "a restore must never clobber by default");
        assert!(!d.delete_extra, "a restore must never delete by default");
        assert!(d.write_files_atomically);
    }
}
