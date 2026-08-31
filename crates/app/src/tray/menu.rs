//! The tray menu, per `UX_SPEC.md` §14.2–§14.5.
//!
//! ## The rule that shapes everything here
//!
//! > Disabled items stay visible and are never removed, so the menu's shape is
//! > stable and muscle memory works.
//!
//! That is why this module builds a *description* of the menu — [`MenuPlan`] —
//! and only then turns it into `muda` items. A plan is a value: it can be
//! asserted on in a test without a windowing system, which is the only way to
//! prove that "Back up now" is present-but-disabled while a job runs rather
//! than quietly absent.
//!
//! ## What changes while a job is running (§14.3), exactly
//!
//! | Item | Idle | While any run is active |
//! |---|---|---|
//! | Header line 2 | last backup / next run | `<job> — 42% · 18.2 MB/s · ~3m left`, or `2 backups running — 42%` |
//! | `Stop "<job>"` | absent | one per active run, `Stop "Dev code" (42%)` when several |
//! | `Stop all backups` | absent | present when 2+ runs are active |
//! | `Back up now` | enabled | **disabled**, suffixed `(already running)` |
//! | `Back up ›` entries | enabled | the running ones disabled and suffixed `(running)` |
//! | `Pause ›` | pauses schedules | unchanged; its header reads `Current backups finish first` |
//! | `Quit superbackup` | quits | quits, after a confirmation naming the running jobs |
//!
//! Progress is text, never a bar: no platform renders one in a menu reliably.
//! The second header line updates at most once a second (§14.3), which the
//! controller enforces by rebuilding on a one-second tick rather than on every
//! progress frame.

use superbackup_core::model::{Config, PauseState};
use superbackup_core::state::{Health, JobRun, StatusSnapshot};
use uuid::Uuid;

use crate::daemon::runtime::{health_summary, relative_past};

/// Jobs listed in `Back up ›` before it collapses to `More…` (§14.2).
pub const MAX_JOB_ITEMS: usize = 12;

/// The pause durations offered, in hours. `None` is "until I resume".
pub const PAUSE_CHOICES: [Option<u32>; 5] = [Some(1), Some(2), Some(4), Some(8), None];

/// What a menu item does when clicked.
///
/// An enum rather than a closure so the plan stays a plain value: comparable,
/// printable, and testable without a menu ever being built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Header lines and separators.
    None,
    RunAll,
    RunJob(Uuid),
    StopRun(Uuid),
    StopAll,
    /// Hours, or `None` for "until I resume".
    Pause(Option<u32>),
    Resume,
    /// Toggle every job off, or back on again.
    DisableAll(bool),
    OpenApp,
    OpenActivity,
    OpenSettings,
    OpenJob(Uuid),
    Unlock,
    FixKopia,
    Quit,
}

impl Action {
    /// The stable menu id this action is registered under.
    ///
    /// Ids carry the payload because `muda` hands back only an id string when
    /// an item is clicked; parsing it back is what
    /// [`Action::from_id`] does.
    pub fn id(&self) -> String {
        match self {
            Action::None => "none".into(),
            Action::RunAll => "run-all".into(),
            Action::RunJob(id) => format!("run-job:{id}"),
            Action::StopRun(id) => format!("stop-run:{id}"),
            Action::StopAll => "stop-all".into(),
            Action::Pause(Some(hours)) => format!("pause:{hours}"),
            Action::Pause(None) => "pause:indefinite".into(),
            Action::Resume => "resume".into(),
            Action::DisableAll(v) => format!("disable-all:{v}"),
            Action::OpenApp => "open-app".into(),
            Action::OpenActivity => "open-activity".into(),
            Action::OpenSettings => "open-settings".into(),
            Action::OpenJob(id) => format!("open-job:{id}"),
            Action::Unlock => "unlock".into(),
            Action::FixKopia => "fix-kopia".into(),
            Action::Quit => "quit".into(),
        }
    }

    /// Recover an action from a clicked item's id.
    ///
    /// Total: an id this build does not recognise yields `None` rather than
    /// panicking. Menu ids arrive from the OS, and a stale menu is a real
    /// possibility during a rebuild.
    pub fn from_id(id: &str) -> Option<Action> {
        match id {
            "run-all" => return Some(Action::RunAll),
            "stop-all" => return Some(Action::StopAll),
            "pause:indefinite" => return Some(Action::Pause(None)),
            "resume" => return Some(Action::Resume),
            "open-app" => return Some(Action::OpenApp),
            "open-activity" => return Some(Action::OpenActivity),
            "open-settings" => return Some(Action::OpenSettings),
            "unlock" => return Some(Action::Unlock),
            "fix-kopia" => return Some(Action::FixKopia),
            "quit" => return Some(Action::Quit),
            _ => {}
        }
        let (kind, value) = id.split_once(':')?;
        match kind {
            "run-job" => Uuid::parse_str(value).ok().map(Action::RunJob),
            "stop-run" => Uuid::parse_str(value).ok().map(Action::StopRun),
            "open-job" => Uuid::parse_str(value).ok().map(Action::OpenJob),
            "pause" => value.parse().ok().map(|h| Action::Pause(Some(h))),
            "disable-all" => value.parse().ok().map(Action::DisableAll),
            _ => None,
        }
    }
}

/// One line of the menu.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    /// A clickable (or deliberately disabled) item.
    Entry {
        label: String,
        action: Action,
        enabled: bool,
        /// The accessible-name suffix, `, disabled, vault locked` (§14.5).
        /// Present exactly when the item is disabled for a stated reason.
        reason: Option<String>,
    },
    /// A checkbox item, whose state is meaningful (§14.2, `Disable all jobs`).
    Check { label: String, action: Action, checked: bool, enabled: bool },
    Submenu { label: String, items: Vec<Item>, enabled: bool },
    Separator,
}

impl Item {
    fn entry(label: impl Into<String>, action: Action) -> Item {
        Item::Entry { label: label.into(), action, enabled: true, reason: None }
    }

    /// A visible item that cannot be used, and says why.
    ///
    /// `reason` becomes both the `(…)` suffix in the label and the accessible
    /// description, because a screen-reader user must hear the same
    /// explanation a sighted one reads.
    fn blocked(label: impl Into<String>, action: Action, reason: &str) -> Item {
        Item::Entry {
            label: format!("{} ({reason})", label.into()),
            action,
            enabled: false,
            reason: Some(reason.to_string()),
        }
    }

    fn header(label: impl Into<String>) -> Item {
        Item::Entry { label: label.into(), action: Action::None, enabled: false, reason: None }
    }

    /// Flatten for assertions: every label in the menu, submenus included.
    ///
    /// This and [`Item::find`] are the plan's inspection API. They exist so
    /// that `UX_SPEC.md` §14's rules — "Back up now is disabled, not removed",
    /// "Stop all appears only with two or more runs" — can be asserted without
    /// a windowing system, which is the only way those rules stay true. The
    /// daemon binary itself never calls them.
    #[allow(dead_code)]
    pub fn labels(items: &[Item]) -> Vec<String> {
        let mut out = Vec::new();
        for item in items {
            match item {
                Item::Entry { label, .. } | Item::Check { label, .. } => out.push(label.clone()),
                Item::Submenu { label, items, .. } => {
                    out.push(label.clone());
                    out.extend(Item::labels(items));
                }
                Item::Separator => {}
            }
        }
        out
    }

    /// Find an item by the action it performs, submenus included.
    #[allow(dead_code)]
    pub fn find(items: &[Item], action: &Action) -> Option<Item> {
        for item in items {
            match item {
                Item::Entry { action: a, .. } | Item::Check { action: a, .. } if a == action => {
                    return Some(item.clone())
                }
                Item::Submenu { items, .. } => {
                    if let Some(found) = Item::find(items, action) {
                        return Some(found);
                    }
                }
                _ => {}
            }
        }
        None
    }

    #[allow(dead_code)]
    pub fn is_enabled(&self) -> bool {
        match self {
            Item::Entry { enabled, .. }
            | Item::Check { enabled, .. }
            | Item::Submenu { enabled, .. } => *enabled,
            Item::Separator => false,
        }
    }
}

/// The whole menu, plus the tooltip that goes with it.
#[derive(Debug, Clone, PartialEq)]
pub struct MenuPlan {
    pub items: Vec<Item>,
    /// Line 1 of the tooltip and the icon's accessible name (§7.5, §14.5).
    pub tooltip_title: String,
    /// Line 2 of the tooltip and the icon's accessible description.
    pub tooltip_detail: String,
    pub health: Health,
}

impl MenuPlan {
    pub fn tooltip(&self) -> String {
        format!("{}\n{}", self.tooltip_title, self.tooltip_detail)
    }
}

/// Build the menu for a moment in time.
///
/// Pure: everything it needs is in the snapshot and the configuration, which
/// is what makes the state table above testable.
pub fn plan(snapshot: &StatusSnapshot, config: &Config, kopia_present: bool) -> MenuPlan {
    let (summary, _) = health_summary(snapshot, config);
    let running = !snapshot.active_runs.is_empty();
    let locked = !snapshot.unlocked;
    let paused = snapshot.paused;

    // The one reason that blocks every run item, in precedence order: a locked
    // vault first because it is the one the user can fix immediately.
    let blocker: Option<&str> = if locked {
        Some("vault locked")
    } else if !kopia_present {
        Some("kopia not found")
    } else {
        None
    };

    let mut items = Vec::new();

    // -- header (§14.2) -------------------------------------------------
    items.push(Item::header(format!("superbackup — {}", snapshot.health.title())));
    let detail = if running {
        running_line(&snapshot.active_runs)
    } else if !kopia_present {
        "Kopia was not found".to_string()
    } else {
        second_line(snapshot, &summary)
    };
    items.push(Item::header(detail.clone()));
    items.push(Item::Separator);

    // -- state-specific first action (§14.4) ----------------------------
    if locked {
        items.push(Item::entry("Unlock…", Action::Unlock));
    } else if !kopia_present {
        items.push(Item::entry("Fix in Settings…", Action::FixKopia));
    } else if snapshot.health == Health::Failed {
        if let Some(job_id) = first_failed(snapshot, config) {
            items.push(Item::entry("View the error…", Action::OpenJob(job_id)));
        }
    } else if snapshot.health == Health::Attention {
        if let Some((job_id, name)) = first_stale(snapshot, config) {
            items.push(Item::entry(format!("Back up “{name}”"), Action::RunJob(job_id)));
        }
    }

    // -- stop items, only while running (§14.3) -------------------------
    if running {
        let several = snapshot.active_runs.len() > 1;
        for run in &snapshot.active_runs {
            let label = match (several, percent(run)) {
                (true, Some(pct)) => format!("Stop “{}” ({pct}%)", run.job_name),
                _ => format!("Stop “{}”", run.job_name),
            };
            // Stopping is never blocked by a locked vault: the run already
            // holds what it needs, and refusing to stop it would be absurd.
            items.push(Item::entry(label, Action::StopRun(run.run_id)));
        }
        if several {
            items.push(Item::entry("Stop all backups", Action::StopAll));
        }
        items.push(Item::Separator);
    }

    // -- run items ------------------------------------------------------
    match (blocker, running) {
        (Some(reason), _) => items.push(Item::blocked("Back up now", Action::RunAll, reason)),
        (None, true) => {
            items.push(Item::blocked("Back up now", Action::RunAll, "already running"))
        }
        (None, false) => items.push(Item::entry("Back up now", Action::RunAll)),
    }

    let enabled_jobs: Vec<&superbackup_core::model::Job> =
        config.jobs.iter().filter(|j| j.enabled).collect();
    let mut job_items = Vec::new();
    for job in enabled_jobs.iter().take(MAX_JOB_ITEMS) {
        let is_running = snapshot.active_runs.iter().any(|r| r.job_id == job.id);
        job_items.push(match (blocker, is_running) {
            (Some(reason), _) => {
                Item::blocked(job.name.clone(), Action::RunJob(job.id), reason)
            }
            (None, true) => Item::blocked(job.name.clone(), Action::RunJob(job.id), "running"),
            (None, false) => Item::entry(job.name.clone(), Action::RunJob(job.id)),
        });
    }
    if enabled_jobs.len() > MAX_JOB_ITEMS {
        job_items.push(Item::entry("More…", Action::OpenApp));
    }
    if job_items.is_empty() {
        job_items.push(Item::header("No jobs are enabled"));
    }
    items.push(Item::Submenu {
        label: "Back up".into(),
        items: job_items,
        enabled: !enabled_jobs.is_empty(),
    });
    items.push(Item::Separator);

    // -- pause / resume (§14.2, §14.4) ----------------------------------
    if paused {
        items.push(Item::entry("Resume backups", Action::Resume));
        items.push(Item::Submenu {
            label: "Extend".into(),
            items: pause_items(running),
            enabled: true,
        });
    } else {
        items.push(Item::Submenu {
            label: "Pause".into(),
            items: pause_items(running),
            enabled: true,
        });
    }

    // `Disable all jobs` is a checkbox, not a button: it reflects the state of
    // `Job::enabled` across every job, and unticking it re-enables exactly the
    // jobs it disabled.
    let all_disabled = !config.jobs.is_empty() && config.jobs.iter().all(|j| !j.enabled);
    items.push(Item::Check {
        label: "Disable all jobs".into(),
        action: Action::DisableAll(!all_disabled),
        checked: all_disabled,
        enabled: !config.jobs.is_empty(),
    });
    items.push(Item::Separator);

    // -- navigation -----------------------------------------------------
    items.push(Item::entry("Open superbackup", Action::OpenApp));
    items.push(Item::entry("Activity…", Action::OpenActivity));
    items.push(Item::entry("Settings…", Action::OpenSettings));
    items.push(Item::Separator);
    items.push(Item::entry("Quit superbackup", Action::Quit));

    MenuPlan {
        items,
        tooltip_title: format!("superbackup — {}", snapshot.health.title()),
        tooltip_detail: detail,
        health: snapshot.health,
    }
}

/// The five pause durations, with the "current backups finish first" header
/// while something is running (§14.3).
fn pause_items(running: bool) -> Vec<Item> {
    let mut items = Vec::new();
    if running {
        items.push(Item::header("Current backups finish first"));
        items.push(Item::Separator);
    }
    for choice in PAUSE_CHOICES {
        let label = match choice {
            Some(1) => "1 hour".to_string(),
            Some(hours) => format!("{hours} hours"),
            None => "Until I resume".to_string(),
        };
        items.push(Item::entry(label, Action::Pause(choice)));
    }
    items
}

/// The second header line while backups are running (§14.3).
fn running_line(runs: &[JobRun]) -> String {
    if runs.len() > 1 {
        let best = runs.iter().filter_map(percent).max().unwrap_or(0);
        return format!("{} backups running — {best}%", runs.len());
    }
    let Some(run) = runs.first() else { return "Backing up".to_string() };
    let mut parts = vec![run.job_name.clone()];
    if let Some(pct) = percent(run) {
        parts.push(format!("{pct}%"));
    }
    let rate: f64 = run.destinations.iter().map(|d| d.progress.bytes_per_second).sum();
    if rate > 1.0 {
        parts.push(format!("{}/s", bytes(rate as u64)));
    }
    if let Some(remaining) = run
        .destinations
        .iter()
        .filter_map(|d| d.progress.estimated_seconds_remaining)
        .max()
        .filter(|s| *s > 0)
    {
        parts.push(format!("~{} left", short_duration(remaining)));
    }
    parts.join(" · ")
}

/// The second header line when nothing is running (§7.5).
fn second_line(snapshot: &StatusSnapshot, summary: &str) -> String {
    if snapshot.health == Health::Idle {
        if let Some(last) = snapshot.jobs.values().filter_map(|j| j.last_success).max() {
            return format!("Last backup {}", relative_past(last, snapshot.generated_at));
        }
    }
    summary.to_string()
}

fn percent(run: &JobRun) -> Option<u32> {
    run.overall_fraction().map(|f| (f.clamp(0.0, 1.0) * 100.0).round() as u32)
}

fn first_failed(snapshot: &StatusSnapshot, config: &Config) -> Option<Uuid> {
    config
        .jobs
        .iter()
        .find(|j| {
            matches!(
                snapshot.jobs.get(&j.id).and_then(|s| s.last_status),
                Some(superbackup_core::state::RunStatus::Failed)
            )
        })
        .map(|j| j.id)
}

fn first_stale(snapshot: &StatusSnapshot, config: &Config) -> Option<(Uuid, String)> {
    let days = config.settings.notifications.stale_after_days;
    config
        .jobs
        .iter()
        .find(|j| {
            j.enabled
                && snapshot
                    .jobs
                    .get(&j.id)
                    .map(|s| s.is_stale(days, snapshot.generated_at))
                    .unwrap_or(false)
        })
        .map(|j| (j.id, j.name.clone()))
}

fn bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn short_duration(seconds: u64) -> String {
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m", seconds / 60)
    } else {
        format!("{}h{}m", seconds / 3600, (seconds % 3600) / 60)
    }
}

/// The confirmation text for quitting with runs in flight (§14.3).
pub fn quit_confirmation(runs: &[JobRun]) -> Option<String> {
    if runs.is_empty() {
        return None;
    }
    Some(format!(
        "Quit and stop {} backup{}? {} will be discarded; the next run starts from scratch.",
        runs.len(),
        if runs.len() == 1 { "" } else { "s" },
        runs.iter().map(|r| format!("“{}”", r.job_name)).collect::<Vec<_>>().join(", ")
    ))
}

/// The pause state a menu choice produces.
pub fn pause_state_for(choice: Option<u32>, now: chrono::DateTime<chrono::Utc>) -> PauseState {
    PauseState {
        paused: true,
        until: choice.map(|hours| now + chrono::Duration::hours(hours as i64)),
        reason: Some("Paused from the tray".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use superbackup_core::engine::testing::test_job;
    use superbackup_core::state::{DestinationRun, JobSummary, Progress, RunStatus, Trigger};

    fn snapshot(health: Health) -> StatusSnapshot {
        StatusSnapshot {
            health,
            version: "0.1.0".into(),
            machine_label: "pc".into(),
            machine_slug: "pc".into(),
            unlocked: true,
            paused: health == Health::Paused,
            paused_until: None,
            service_installed: false,
            service_running: false,
            kopia_version: Some("0.21.5".into()),
            active_runs: vec![],
            jobs: Default::default(),
            next_scheduled: None,
            recent_events: vec![],
            uptime_seconds: 60,
            generated_at: Utc::now(),
        }
    }

    fn config(jobs: &[&str]) -> Config {
        let mut config = Config::default();
        for name in jobs {
            config.jobs.push(test_job(name));
        }
        config
    }

    fn run(name: &str, job_id: Uuid, fraction: f32) -> JobRun {
        let total = 1000u64;
        JobRun {
            run_id: Uuid::new_v4(),
            job_id,
            job_name: name.into(),
            trigger: Trigger::Manual,
            status: RunStatus::Running,
            started_at: Utc::now(),
            finished_at: None,
            destinations: vec![DestinationRun {
                destination_id: Uuid::new_v4(),
                destination_name: "disk".into(),
                status: RunStatus::Running,
                started_at: Some(Utc::now()),
                finished_at: None,
                progress: Progress {
                    bytes_processed: (total as f32 * fraction) as u64,
                    bytes_total: Some(total),
                    bytes_per_second: 19_000_000.0,
                    estimated_seconds_remaining: Some(185),
                    ..Default::default()
                },
                snapshot_id: None,
                error: None,
                warnings: vec![],
            }],
        }
    }

    #[test]
    fn the_idle_menu_has_the_shape_the_spec_describes() {
        let config = config(&["dev code"]);
        let plan = super::plan(&snapshot(Health::Idle), &config, true);
        let labels = Item::labels(&plan.items);
        for expected in [
            "Back up now",
            "Back up",
            "Pause",
            "Disable all jobs",
            "Open superbackup",
            "Activity…",
            "Settings…",
            "Quit superbackup",
        ] {
            assert!(labels.iter().any(|l| l == expected), "missing {expected}: {labels:?}");
        }
        assert!(Item::find(&plan.items, &Action::RunAll).is_some_and(|i| i.is_enabled()));
    }

    #[test]
    fn pausing_offers_one_two_four_eight_hours_and_indefinitely() {
        let plan = super::plan(&snapshot(Health::Idle), &config(&["a"]), true);
        for choice in PAUSE_CHOICES {
            assert!(
                Item::find(&plan.items, &Action::Pause(choice)).is_some(),
                "missing pause choice {choice:?}"
            );
        }
        let labels = Item::labels(&plan.items);
        assert!(labels.iter().any(|l| l == "1 hour"));
        assert!(labels.iter().any(|l| l == "8 hours"));
        assert!(labels.iter().any(|l| l == "Until I resume"));
    }

    #[test]
    fn while_running_back_up_now_is_disabled_but_still_present() {
        let mut config = config(&["dev code", "photos"]);
        let job_id = config.jobs[0].id;
        let mut snap = snapshot(Health::Running);
        snap.active_runs = vec![run("dev code", job_id, 0.42)];
        let plan = super::plan(&snap, &config, true);

        // Present, disabled, and it says why — the spec's central rule.
        let back_up_now =
            Item::find(&plan.items, &Action::RunAll).expect("Back up now must stay visible");
        assert!(!back_up_now.is_enabled());
        match back_up_now {
            Item::Entry { label, reason, .. } => {
                assert!(label.contains("already running"), "{label}");
                assert_eq!(reason.as_deref(), Some("already running"));
            }
            other => panic!("unexpected item {other:?}"),
        }

        // The running job is disabled in the submenu; the other one is not.
        let running_entry =
            Item::find(&plan.items, &Action::RunJob(job_id)).expect("the running job");
        assert!(!running_entry.is_enabled());
        let other = Item::find(&plan.items, &Action::RunJob(config.jobs[1].id)).expect("other");
        assert!(other.is_enabled());
        let _ = &mut config;
    }

    #[test]
    fn stop_all_appears_only_with_two_or_more_runs() {
        let config = config(&["a", "b"]);
        let mut snap = snapshot(Health::Running);
        snap.active_runs = vec![run("a", config.jobs[0].id, 0.1)];
        assert!(Item::find(&super::plan(&snap, &config, true).items, &Action::StopAll).is_none());

        snap.active_runs.push(run("b", config.jobs[1].id, 0.9));
        let two = super::plan(&snap, &config, true);
        assert!(Item::find(&two.items, &Action::StopAll).is_some());
        // With several runs the per-run items carry their own percentage.
        assert!(Item::labels(&two.items).iter().any(|l| l.contains("Stop “b” (90%)")), "{:?}", Item::labels(&two.items));
    }

    #[test]
    fn the_running_header_is_precise_text_and_never_a_bar() {
        let config = config(&["dev code"]);
        let mut snap = snapshot(Health::Running);
        snap.active_runs = vec![run("dev code", config.jobs[0].id, 0.42)];
        let plan = super::plan(&snap, &config, true);
        assert!(plan.tooltip_detail.contains("dev code"));
        assert!(plan.tooltip_detail.contains("42%"));
        assert!(plan.tooltip_detail.contains("/s"));
        assert!(plan.tooltip_detail.contains("left"));
    }

    #[test]
    fn a_locked_vault_disables_every_run_item_and_offers_unlock() {
        let config = config(&["dev code"]);
        let mut snap = snapshot(Health::Attention);
        snap.unlocked = false;
        let plan = super::plan(&snap, &config, true);
        assert!(Item::find(&plan.items, &Action::Unlock).is_some_and(|i| i.is_enabled()));
        let run_all = Item::find(&plan.items, &Action::RunAll).expect("still visible");
        assert!(!run_all.is_enabled());
        match run_all {
            Item::Entry { reason, .. } => assert_eq!(reason.as_deref(), Some("vault locked")),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn a_missing_kopia_disables_runs_and_offers_the_settings_page() {
        let plan = super::plan(&snapshot(Health::Idle), &config(&["a"]), false);
        assert!(Item::find(&plan.items, &Action::FixKopia).is_some());
        assert!(!Item::find(&plan.items, &Action::RunAll).expect("visible").is_enabled());
        assert_eq!(plan.tooltip_detail, "Kopia was not found");
    }

    #[test]
    fn pausing_replaces_the_submenu_with_resume_and_extend() {
        let config = config(&["a"]);
        let mut snap = snapshot(Health::Paused);
        snap.paused = true;
        let plan = super::plan(&snap, &config, true);
        assert!(Item::find(&plan.items, &Action::Resume).is_some());
        assert!(Item::labels(&plan.items).iter().any(|l| l == "Extend"));
        // A manual run stays enabled while paused: pause is about schedules.
        assert!(Item::find(&plan.items, &Action::RunAll).is_some_and(|i| i.is_enabled()));
    }

    #[test]
    fn disable_all_is_a_checkbox_that_reflects_every_job() {
        let mut config = config(&["a", "b"]);
        let plan = super::plan(&snapshot(Health::Idle), &config, true);
        match Item::find(&plan.items, &Action::DisableAll(true)).expect("checkbox") {
            Item::Check { checked, .. } => assert!(!checked),
            other => panic!("expected a checkbox, got {other:?}"),
        }
        for job in &mut config.jobs {
            job.enabled = false;
        }
        let plan = super::plan(&snapshot(Health::Idle), &config, true);
        match Item::find(&plan.items, &Action::DisableAll(false)).expect("checkbox") {
            Item::Check { checked, .. } => assert!(checked),
            other => panic!("expected a checkbox, got {other:?}"),
        }
    }

    #[test]
    fn more_than_twelve_jobs_collapse_to_a_more_item() {
        let names: Vec<String> = (0..20).map(|i| format!("job {i}")).collect();
        let config = config(&names.iter().map(|s| s.as_str()).collect::<Vec<_>>());
        let plan = super::plan(&snapshot(Health::Idle), &config, true);
        let labels = Item::labels(&plan.items);
        assert!(labels.iter().any(|l| l == "More…"));
        assert_eq!(labels.iter().filter(|l| l.starts_with("job ")).count(), MAX_JOB_ITEMS);
    }

    #[test]
    fn a_failed_health_offers_the_error_first() {
        let mut config = config(&["dev code"]);
        let job_id = config.jobs[0].id;
        let mut snap = snapshot(Health::Failed);
        snap.jobs.insert(
            job_id,
            JobSummary { last_status: Some(RunStatus::Failed), ..Default::default() },
        );
        let plan = super::plan(&snap, &config, true);
        assert!(Item::find(&plan.items, &Action::OpenJob(job_id)).is_some());
        let _ = &mut config;
    }

    #[test]
    fn every_action_round_trips_through_its_menu_id() {
        let id = Uuid::new_v4();
        for action in [
            Action::RunAll,
            Action::RunJob(id),
            Action::StopRun(id),
            Action::StopAll,
            Action::Pause(Some(4)),
            Action::Pause(None),
            Action::Resume,
            Action::DisableAll(true),
            Action::DisableAll(false),
            Action::OpenApp,
            Action::OpenActivity,
            Action::OpenSettings,
            Action::OpenJob(id),
            Action::Unlock,
            Action::FixKopia,
            Action::Quit,
        ] {
            assert_eq!(Action::from_id(&action.id()), Some(action.clone()), "{action:?}");
        }
    }

    #[test]
    fn an_unknown_menu_id_is_ignored_rather_than_fatal() {
        assert_eq!(Action::from_id("who-knows"), None);
        assert_eq!(Action::from_id("run-job:not-a-uuid"), None);
        assert_eq!(Action::from_id("pause:soon"), None);
        assert_eq!(Action::from_id(""), None);
    }

    #[test]
    fn quitting_mid_backup_names_the_jobs_it_would_stop() {
        assert_eq!(quit_confirmation(&[]), None);
        let text = quit_confirmation(&[run("dev code", Uuid::new_v4(), 0.5)]).expect("text");
        assert!(text.contains("dev code"));
        assert!(text.contains("Quit and stop 1 backup?"));
    }
}
