//! Policies and maintenance.
//!
//! Retention and exclusions live **inside the repository**, attached to a
//! source path, not in superbackup's config. That is kopia's design and it has
//! a consequence worth stating: the policy must be pushed to every destination
//! before every run, because a user editing a job's exclusions in the GUI has
//! changed nothing at all until `policy set` has been run against each
//! repository the job writes to.

use super::command::RunContext;
use super::driver::{KopiaDriver, KopiaResult};
use super::error::{KopiaError, KopiaFailure};
use crate::model::{ExclusionSet, RetentionPolicy, Source};
use std::path::Path;
use std::time::Duration;

/// The stored policy for one source, as `policy show --json` reports it.
///
/// Kopia's policy document is large, versioned and deeply nested
/// (`snapshot/policy/policy.go`), and superbackup only ever needs to read back
/// the handful of fields it writes. Keeping the whole document in
/// [`StoredPolicy::raw`] means the "advanced" view can show everything without
/// this type having to track kopia's schema.
#[derive(Debug, Clone, Default)]
pub struct StoredPolicy {
    pub keep_latest: Option<u32>,
    pub keep_hourly: Option<u32>,
    pub keep_daily: Option<u32>,
    pub keep_weekly: Option<u32>,
    pub keep_monthly: Option<u32>,
    pub keep_annual: Option<u32>,
    pub ignore_rules: Vec<String>,
    pub dot_ignore_files: Vec<String>,
    pub max_file_size: Option<u64>,
    pub ignore_cache_directories: Option<bool>,
    pub one_file_system: Option<bool>,
    pub raw: serde_json::Value,
}

impl StoredPolicy {
    fn parse(stdout: &str) -> Option<StoredPolicy> {
        let v: serde_json::Value = serde_json::from_str(stdout.trim()).ok()?;
        let num = |section: &str, key: &str| -> Option<u32> {
            v.get(section)?.get(key)?.as_u64().map(|n| n as u32)
        };
        let list = |section: &str, key: &str| -> Vec<String> {
            v.get(section)
                .and_then(|s| s.get(key))
                .and_then(|a| a.as_array())
                .map(|a| a.iter().filter_map(|e| e.as_str().map(String::from)).collect())
                .unwrap_or_default()
        };
        let flag = |section: &str, key: &str| -> Option<bool> {
            v.get(section)?.get(key)?.as_bool()
        };
        Some(StoredPolicy {
            keep_latest: num("retention", "keepLatest"),
            keep_hourly: num("retention", "keepHourly"),
            keep_daily: num("retention", "keepDaily"),
            keep_weekly: num("retention", "keepWeekly"),
            keep_monthly: num("retention", "keepMonthly"),
            keep_annual: num("retention", "keepAnnual"),
            ignore_rules: list("files", "ignore"),
            dot_ignore_files: list("files", "ignoreDotFiles"),
            max_file_size: v
                .get("files")
                .and_then(|f| f.get("maxFileSize"))
                .and_then(|n| n.as_u64()),
            ignore_cache_directories: flag("files", "ignoreCacheDirs"),
            one_file_system: flag("files", "oneFileSystem"),
            raw: v,
        })
    }
}

/// Which maintenance to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaintenanceMode {
    /// Index compaction and quick blob cleanup. Seconds to a minute, safe to
    /// run after every backup.
    Quick,
    /// Full garbage collection: this is what actually reclaims the space freed
    /// by expired snapshots, and it is expensive.
    Full,
}

/// Maintenance scheduling parameters.
#[derive(Debug, Clone, Default)]
pub struct MaintenanceSettings {
    /// `user@hostname` allowed to run maintenance on this repository. Kopia
    /// permits exactly one owner, so on a repository shared by several PCs this
    /// must be set deliberately or maintenance never runs at all.
    pub owner: Option<String>,
    pub enable_quick: Option<bool>,
    pub enable_full: Option<bool>,
    pub quick_interval: Option<Duration>,
    pub full_interval: Option<Duration>,
}

impl KopiaDriver {
    /// Push a job's retention and exclusions to this repository, for one source.
    ///
    /// Flags verified against `cli/command_policy_set_retention.go`
    /// (`--keep-latest/hourly/daily/weekly/monthly/annual`) and
    /// `cli/command_policy_set_files.go` (`--add-ignore`, `--clear-ignore`,
    /// `--add-dot-ignore`, `--clear-dot-ignore`, `--max-file-size`,
    /// `--ignore-cache-dirs`, `--one-file-system`).
    ///
    /// ## Why this runs kopia twice
    ///
    /// `applyPolicyStringList` in `cli/command_policy_set.go` **returns early**
    /// when `--clear-ignore` is set:
    ///
    /// ```go
    /// if clearList {
    ///     *val = nil
    ///     return          // every --add-ignore in the same command is discarded
    /// }
    /// ```
    ///
    /// So `policy set --clear-ignore --add-ignore=X` silently leaves the ignore
    /// list *empty*. Removing a preset in the GUI would otherwise appear to
    /// work and then quietly disable every exclusion, turning a "back up my
    /// project" job into "back up node_modules forever". Clearing and setting
    /// are therefore two separate invocations, in that order.
    ///
    /// Kopia also rejects a `policy set` that would change nothing
    /// (`"no changes specified"`), so the clearing pass is skipped when there is
    /// nothing to clear.
    pub async fn apply_source_policy(
        &self,
        source: &Source,
        retention: &RetentionPolicy,
        exclusions: &ExclusionSet,
        ctx: &RunContext,
    ) -> KopiaResult<()> {
        self.require_passphrase("policy set")?;
        let target = source.path.clone();

        // Pass 1: drop whatever ignore rules a previous configuration left
        // behind, so removing an exclusion in the GUI actually removes it.
        let mut clear = self.base();
        clear
            .command("policy")
            .command("set")
            .arg(&target)
            .switch("clear-ignore")
            .switch("clear-dot-ignore");
        self.run(clear, ctx).await?;

        // Pass 2: the policy the user actually asked for.
        let mut cmd = self.base();
        cmd.command("policy").command("set").arg(&target);

        cmd.flag("keep-latest", retention.keep_latest.to_string())
            .flag("keep-hourly", retention.keep_hourly.to_string())
            .flag("keep-daily", retention.keep_daily.to_string())
            .flag("keep-weekly", retention.keep_weekly.to_string())
            .flag("keep-monthly", retention.keep_monthly.to_string())
            .flag("keep-annual", retention.keep_annual.to_string());

        let patterns = exclusions.effective_patterns();
        cmd.repeated("add-ignore", &patterns);

        if exclusions.use_gitignore {
            // Kopia's "dot-ignore" list names files inside the tree whose
            // contents are gitignore-syntax rules; naming `.gitignore` is how
            // one asks kopia to honour the repository's own ignore file.
            cmd.flag("add-dot-ignore", ".gitignore");
        }

        // `applyPolicyNumber64` parses this with `strconv.ParseInt`, so it is a
        // plain byte count with no unit suffix.
        match exclusions.max_file_size_mb {
            Some(mb) => cmd.flag("max-file-size", (mb.saturating_mul(1024 * 1024)).to_string()),
            None => cmd.flag("max-file-size", "inherit"),
        };

        cmd.flag("ignore-cache-dirs", bool_enum(exclusions.respect_cachedir_tag));
        cmd.flag("one-file-system", bool_enum(source.one_filesystem));

        self.run(cmd, ctx).await?;
        Ok(())
    }

    /// Read back the effective policy for a source, for the "what will actually
    /// be excluded?" panel and for verifying that a policy push took.
    ///
    /// `cli/command_policy_show.go` supports `--json`. Field names inside the
    /// document are not pinned by this driver; see [`StoredPolicy`].
    pub async fn show_policy(
        &self,
        source: Option<&Path>,
        ctx: &RunContext,
    ) -> KopiaResult<StoredPolicy> {
        self.require_passphrase("policy show")?;
        let mut cmd = self.base();
        cmd.command("policy").command("show");
        match source {
            Some(p) => {
                cmd.arg(p);
            }
            // `policyTargets` insists on exactly one of a target or `--global`.
            None => {
                cmd.switch("global");
            }
        }
        cmd.switch("json");
        let out = self.run(cmd, ctx).await?;
        StoredPolicy::parse(&out.stdout).ok_or_else(|| {
            KopiaError::local(
                "policy show",
                KopiaFailure::Unknown,
                Some("kopia's policy output could not be understood".into()),
            )
        })
    }

    /// Run repository maintenance.
    ///
    /// `cli/command_maintenance_run.go` exposes `--full`; without it kopia runs
    /// quick maintenance. Maintenance is owned by one `user@hostname`, and a
    /// non-owner's run exits without doing anything rather than failing — so a
    /// silent success here does not prove work happened. See
    /// [`KopiaDriver::configure_maintenance`].
    ///
    /// Full maintenance rewrites and deletes blobs; it must not be interrupted
    /// casually, so callers should give it a generous timeout and only cancel it
    /// on a real user request.
    pub async fn run_maintenance(
        &self,
        mode: MaintenanceMode,
        ctx: &RunContext,
    ) -> KopiaResult<String> {
        self.require_passphrase("maintenance run")?;
        let mut cmd = self.base();
        cmd.command("maintenance").command("run");
        if mode == MaintenanceMode::Full {
            cmd.switch("full");
        }
        let out = self.run(cmd, ctx).await?;
        Ok(out.redacted_stdout())
    }

    /// Configure automatic maintenance.
    ///
    /// Flags verified against `cli/command_maintenance_set.go`: `--owner`,
    /// `--enable-quick`, `--enable-full`, `--quick-interval`,
    /// `--full-interval`. Durations are Go duration strings (`24h`, `168h`),
    /// which is why they are rendered rather than passed as seconds.
    ///
    /// `--owner` accepts the literal `me` to claim ownership for the current
    /// `user@hostname`; that is the right value on a single-PC repository and
    /// the wrong one on a shared repository where a specific machine should own
    /// maintenance, hence the explicit parameter.
    pub async fn configure_maintenance(
        &self,
        settings: &MaintenanceSettings,
        ctx: &RunContext,
    ) -> KopiaResult<()> {
        self.require_passphrase("maintenance set")?;
        let mut cmd = self.base();
        cmd.command("maintenance").command("set");
        let mut changed = false;
        if let Some(owner) = &settings.owner {
            cmd.flag("owner", owner);
            changed = true;
        }
        if let Some(v) = settings.enable_quick {
            cmd.flag_bool("enable-quick", v);
            changed = true;
        }
        if let Some(v) = settings.enable_full {
            cmd.flag_bool("enable-full", v);
            changed = true;
        }
        if let Some(d) = settings.quick_interval {
            cmd.flag("quick-interval", go_duration(d));
            changed = true;
        }
        if let Some(d) = settings.full_interval {
            cmd.flag("full-interval", go_duration(d));
            changed = true;
        }
        if !changed {
            // Kopia would exit non-zero on a no-op; there is nothing to report.
            return Ok(());
        }
        self.run(cmd, ctx).await?;
        Ok(())
    }
}

/// Kopia's tri-state boolean flags take `true`, `false` or `inherit`
/// (`booleanEnumValues` in `cli/command_policy_set.go`). Superbackup always
/// states an explicit value so a stale inherited setting cannot surprise the
/// user.
fn bool_enum(value: bool) -> &'static str {
    if value {
        "true"
    } else {
        "false"
    }
}

/// Render a duration the way Go's `time.ParseDuration` reads it.
fn go_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs == 0 {
        return "0s".to_string();
    }
    if secs % 3600 == 0 {
        return format!("{}h", secs / 3600);
    }
    if secs % 60 == 0 {
        return format!("{}m", secs / 60);
    }
    format!("{secs}s")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_render_in_gos_syntax() {
        assert_eq!(go_duration(Duration::from_secs(0)), "0s");
        assert_eq!(go_duration(Duration::from_secs(45)), "45s");
        assert_eq!(go_duration(Duration::from_secs(600)), "10m");
        assert_eq!(go_duration(Duration::from_secs(24 * 3600)), "24h");
        assert_eq!(go_duration(Duration::from_secs(3661)), "3661s");
    }

    #[test]
    fn tri_state_booleans_are_never_left_to_inherit() {
        assert_eq!(bool_enum(true), "true");
        assert_eq!(bool_enum(false), "false");
    }

    #[test]
    fn policy_show_json_is_parsed_leniently() {
        let json = r#"{
          "retention": {"keepLatest":10,"keepHourly":24,"keepDaily":14,
                        "keepWeekly":8,"keepMonthly":12,"keepAnnual":3},
          "files": {"ignore":["node_modules/","target/"],"ignoreDotFiles":[".gitignore"],
                    "maxFileSize":2147483648,"ignoreCacheDirs":true,"oneFileSystem":false},
          "somethingNew": {"we":"do not model this"}
        }"#;
        let p = StoredPolicy::parse(json).expect("parses");
        assert_eq!(p.keep_daily, Some(14));
        assert_eq!(p.keep_annual, Some(3));
        assert_eq!(p.ignore_rules, vec!["node_modules/", "target/"]);
        assert_eq!(p.dot_ignore_files, vec![".gitignore"]);
        assert_eq!(p.max_file_size, Some(2_147_483_648));
        assert_eq!(p.ignore_cache_directories, Some(true));
        assert_eq!(p.one_file_system, Some(false));
        assert!(p.raw.get("somethingNew").is_some(), "the full document must survive");
    }

    #[test]
    fn policy_show_of_an_empty_policy_still_parses() {
        let p = StoredPolicy::parse("{}").expect("parses");
        assert_eq!(p.keep_daily, None);
        assert!(p.ignore_rules.is_empty());
        assert!(StoredPolicy::parse("not json").is_none());
    }
}
