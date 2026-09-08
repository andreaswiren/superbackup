//! The git inventory: which folders being backed up are repositories, and
//! whether what is in them exists anywhere but this disk.
//!
//! # What this screen is for
//!
//! Not "which folders use git" — a list of forty repositories with a tick
//! beside each tells nobody anything. The question is which of them hold work
//! that would be gone with the disk, because those are the ones where this
//! backup is the only other copy, and they are the rows that get the colour,
//! the ordering, and the buttons.
//!
//! # Why the scan is a button
//!
//! It spawns processes — three per repository, plus one network call each when
//! remotes are checked — and a screen that did that on every frame, or on every
//! visit, would be a screen that made the machine slower for as long as it was
//! open. It runs when asked and says when it last ran.

use egui::{Align, Layout, Sense, Ui, Vec2};

use superbackup_core::git::{GitRepo, Inventory, RepoState};
use superbackup_core::ipc::protocol::Request;

use crate::gui::app::App;
use crate::gui::copy;
use crate::gui::daemon::Intent;
use crate::gui::format;
use crate::gui::icons::Icon;
use crate::gui::theme::{self, size, space, Type};
use crate::gui::viewmodel::{self, ColumnSpec};
use crate::gui::widgets::{self, Button};

#[derive(Default)]
pub struct State {
    pub inventory: Option<Inventory>,
    pub scanning: bool,
    pub check_remotes: bool,
    pub at_risk_only: bool,
    pub search: String,
    /// The repository a modal is about, by path — not by index, which changes
    /// under the modal the moment a scan finishes.
    pub acting_on: Option<std::path::PathBuf>,
    /// Set while an action is in flight, so its button cannot be pressed twice
    /// and a second commit cannot be started on top of the first.
    pub acting: bool,
    pub failed: Option<String>,
    /// Repositories still to pull in a "refresh them all" run.
    ///
    /// One at a time, because `git_act` is single-flight and twenty
    /// concurrent `git pull` processes against twenty working trees is not a
    /// refresh, it is a load test.
    pub pull_queue: std::collections::VecDeque<std::path::PathBuf>,
    /// How many the run started with, so progress can be reported as "3 of 12"
    /// rather than a spinner that says nothing.
    pub pull_total: usize,
    /// Repositories the bulk run could not pull, named at the end. Reported
    /// together rather than as a toast each, which on a laptop that has been
    /// closed for a week would be a stack of twenty.
    pub pull_failed: Vec<String>,
}

impl State {
    pub fn busy(&self) -> bool {
        self.scanning || self.acting
    }

    pub fn scan_started(&mut self) {
        self.scanning = true;
        self.failed = None;
    }

    pub fn arrived(&mut self, inventory: Inventory) {
        self.inventory = Some(inventory);
        self.scanning = false;
    }

    pub fn scan_failed(&mut self, why: String) {
        self.scanning = false;
        self.failed = Some(why);
    }

    pub fn action_finished(&mut self) {
        self.acting = false;
        self.acting_on = None;
    }

    /// True while a "refresh them all" run is in flight.
    pub fn pulling_all(&self) -> bool {
        self.pull_total > 0
    }

    /// How far through, for the button's label.
    pub fn pull_progress(&self) -> (usize, usize) {
        (self.pull_total.saturating_sub(self.pull_queue.len()), self.pull_total)
    }

    pub fn start_bulk_pull(&mut self, paths: Vec<std::path::PathBuf>) {
        self.pull_total = paths.len();
        self.pull_queue = paths.into();
        self.pull_failed.clear();
    }

    pub fn bulk_pull_finished(&mut self) {
        self.pull_total = 0;
        self.pull_queue.clear();
    }
}

/// `UX_SPEC` §9. The repository name and its state never drop: without both,
/// the row says nothing the screen exists to say.
/// The "Not in git" grid. Fixed columns, so the folder name takes the rest.
const CANDIDATE_MARK_W: f32 = 150.0;
const CANDIDATE_ITEMS_W: f32 = 90.0;
const CANDIDATE_ACTION_W: f32 = 150.0;

const GIT_COLUMNS: [ColumnSpec; 7] = [
    ColumnSpec::keep("repo", 200.0),
    ColumnSpec::keep("state", 150.0),
    ColumnSpec::droppable("branch", 150.0, 2),
    ColumnSpec::droppable("changes", 80.0, 3),
    // Wide enough for `2026-08-15 12:47`; at 110 the ISO form elided to
    // "2026-08-15 12:4…", which is worse than the format it replaced.
    ColumnSpec::droppable("last", 150.0, 1),
    ColumnSpec::droppable("host", 96.0, 4),
    ColumnSpec::keep("actions", 150.0),
];

impl App {
    pub(crate) fn git_actions(&mut self, ui: &mut Ui) {
        let scanning = self.screens.git.scanning;
        if Button::primary(copy::git::SCAN)
            .icon(Icon::RefreshCw)
            .enabled(!scanning)
            .show(ui)
            .clicked()
        {
            self.scan_git();
        }

        // Refresh every repository that says a pull is recommended.
        //
        // Only those: running `git pull` in a clean, up-to-date repository
        // spawns a network round-trip to do nothing, and doing that across
        // forty repositories is how a "refresh" becomes a two-minute freeze.
        let behind: Vec<std::path::PathBuf> = self
            .screens
            .git
            .inventory
            .as_ref()
            .map(|inventory| {
                inventory
                    .repos
                    .iter()
                    .filter(|r| matches!(r.state(), RepoState::PullRecommended))
                    .map(|r| r.path.clone())
                    .collect()
            })
            .unwrap_or_default();
        let pulling = self.screens.git.pulling_all();
        let mut pull_all = Button::secondary(copy::git::PULL_ALL)
            .icon(Icon::Download)
            .enabled(!scanning && !behind.is_empty() && !pulling);
        // Owned outside the branch: `Button` borrows its label, so a String
        // built inside the `if` would not outlive the button.
        let progress = {
            let (done, total) = self.screens.git.pull_progress();
            copy::git_pulling_all(done, total)
        };
        if pulling {
            pull_all = Button::secondary(&progress).icon(Icon::Download).enabled(false);
        } else if behind.is_empty() {
            pull_all = pull_all.disabled_because(copy::git::PULL_ALL_NONE);
        }
        if pull_all.show(ui).on_hover_text(copy::git::PULL_ALL_HINT).clicked() {
            self.screens.git.start_bulk_pull(behind);
            self.pull_next_repository();
        }

        let mut check = self.screens.git.check_remotes;
        if widgets::checkbox(ui, &mut check, copy::git::CHECK_REMOTES, None, !scanning).clicked() {
            self.screens.git.check_remotes = check;
            // Rescanning immediately, because the switch changes what the
            // numbers on screen *mean* and leaving the old ones under a new
            // label is the one thing worse than not having the switch.
            self.scan_git();
        }
        ui.add_space(space::S);

        let mut at_risk = self.screens.git.at_risk_only;
        if widgets::checkbox(ui, &mut at_risk, copy::git::AT_RISK_ONLY, None, true).clicked() {
            self.screens.git.at_risk_only = at_risk;
        }

        widgets::Field::new()
            .width(200.0)
            .placeholder(copy::git::SEARCH)
            .show(ui, &mut self.screens.git.search);
    }

    pub(crate) fn scan_git(&mut self) {
        self.screens.git.scan_started();
        self.ask(
            Intent::GitInventory,
            Request::GitInventory {
                job: None,
                check_remotes: self.screens.git.check_remotes,
                max_depth: None,
            },
        );
    }

    pub(crate) fn show_git(&mut self, ui: &mut Ui) {
        let t = theme::tokens(ui.ctx());
        let now = chrono::Utc::now();

        // Never scanned in this session. Ask once, on arrival, rather than
        // showing an empty table that looks like an answer.
        if self.screens.git.inventory.is_none() && !self.screens.git.scanning {
            match &self.screens.git.failed {
                Some(why) => {
                    let why = why.clone();
                    widgets::banner(
                        ui,
                        widgets::BannerKind::Danger,
                        copy::git::SCAN_FAILED,
                        Some(&why),
                        |_| {},
                    );
                    ui.add_space(space::L);
                    if Button::primary(copy::git::SCAN).icon(Icon::RefreshCw).show(ui).clicked() {
                        self.scan_git();
                    }
                    return;
                }
                None => {
                    self.scan_git();
                }
            }
        }

        if self.screens.git.scanning && self.screens.git.inventory.is_none() {
            ui.add_space(space::H2);
            ui.vertical_centered(|ui| {
                widgets::spinner(ui, 24.0, t.accent);
                ui.add_space(space::M);
                widgets::text(ui, copy::git::SCANNING, Type::Body, t.text_muted);
            });
            return;
        }

        let Some(inventory) = self.screens.git.inventory.clone() else { return };

        if !inventory.git_available {
            widgets::empty_state(
                ui,
                Icon::GitBranch,
                &crate::gui::copy::Empty {
                    title: copy::git::NO_GIT,
                    body: copy::git::NO_GIT_BODY,
                    primary: None,
                    secondary: None,
                },
                None,
            );
            return;
        }

        // The whole page scrolls.
        //
        // It was the only screen without one, and the repository table is as
        // long as the machine has repositories — nineteen here — so "Not in
        // git" sat below the bottom of the window and could not be reached at
        // all without maximising. A section you can only see at one window
        // size is a section most people never see.
        widgets::scroll_area(ui, "git", |ui| {
            self.show_git_body(ui, &inventory, now);
        });
    }

    fn show_git_body(
        &mut self,
        ui: &mut Ui,
        inventory: &Inventory,
        now: chrono::DateTime<chrono::Utc>,
    ) {
        let t = theme::tokens(ui.ctx());
        let inventory = inventory.clone();
        self.git_summary(ui, &inventory, now);
        ui.add_space(space::L);

        let needle = self.screens.git.search.trim().to_lowercase();
        let rows: Vec<GitRepo> = inventory
            .repos
            .iter()
            .filter(|r| !self.screens.git.at_risk_only || r.state().only_on_this_disk())
            .filter(|r| {
                needle.is_empty()
                    || r.name.to_lowercase().contains(&needle)
                    || r.path.display().to_string().to_lowercase().contains(&needle)
            })
            .cloned()
            .collect();

        if rows.is_empty() {
            let empty = if inventory.repos.is_empty() {
                crate::gui::copy::Empty {
                    title: copy::git::EMPTY,
                    body: copy::git::EMPTY_BODY,
                    primary: None,
                    secondary: None,
                }
            } else {
                crate::gui::copy::Empty {
                    title: copy::git::ALL_SAFE,
                    body: copy::git::ALL_SAFE_BODY,
                    primary: None,
                    secondary: None,
                }
            };
            widgets::empty_state(ui, Icon::GitBranch, &empty, None);
            return;
        }

        self.git_table(ui, &rows, now);

        self.git_candidates(ui, &inventory);

        for note in &inventory.notes {
            ui.add_space(space::M);
            widgets::paragraph(ui, note.clone(), Type::Small, t.warning.tint_text);
        }
        if !self.screens.git.check_remotes {
            ui.add_space(space::M);
            widgets::paragraph(ui, copy::git::STALE_NOTE, Type::Small, t.text_muted);
        }
    }

    /// The line the screen leads with: how many of these hold work that is
    /// only here.
    fn git_summary(
        &mut self,
        ui: &mut Ui,
        inventory: &Inventory,
        now: chrono::DateTime<chrono::Utc>,
    ) {
        let t = theme::tokens(ui.ctx());
        let risky = inventory.repos.iter().filter(|r| r.state().only_on_this_disk()).count();
        widgets::card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                widgets::text(
                    ui,
                    risky.to_string(),
                    Type::Display,
                    if risky > 0 { t.warning.mark } else { t.success.mark },
                );
                ui.add_space(space::S);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    widgets::text(ui, copy::git::SUMMARY_AT_RISK, Type::BodyStrong, t.text_primary);
                    widgets::text(
                        ui,
                        format!(
                            "of {} {} · scanned {}",
                            inventory.repos.len(),
                            copy::git::SUMMARY_TOTAL,
                            format::relative(inventory.scanned_at, now),
                        ),
                        Type::Small,
                        t.text_muted,
                    );
                });
            });
        });
    }

    fn git_table(&mut self, ui: &mut Ui, rows: &[GitRepo], now: chrono::DateTime<chrono::Utc>) {
        let t = theme::tokens(ui.ctx());
        let shown = viewmodel::fit_columns(
            ui.available_width() - widgets::TABLE_GUTTER,
            200.0,
            ui.spacing().item_spacing.x,
            &GIT_COLUMNS,
        );
        let has = |key: &str| shown.contains(&key);

        // Collected during the table and acted on after it: mutating `self`
        // inside the row closure would borrow it twice.
        let mut pull: Option<std::path::PathBuf> = None;
        let mut commit: Option<std::path::PathBuf> = None;
        let mut push: Option<std::path::PathBuf> = None;
        let mut trust: Option<std::path::PathBuf> = None;
        let mut open: Option<std::path::PathBuf> = None;
        // The repository page to open on its host, when the globe is clicked.
        let mut visit: Option<String> = None;
        let mut expand: Option<std::path::PathBuf> = None;
        let busy = self.screens.git.acting;

        widgets::table_frame(ui, |ui| {
            let gap = ui.spacing().item_spacing.x;
            let mut fixed = 150.0 + 150.0 + gap;
            for (key, width) in
                [("branch", 150.0), ("changes", 80.0), ("last", 110.0), ("host", 96.0)]
            {
                if has(key) {
                    fixed += width + gap;
                }
            }
            let name_width = (ui.available_width() - fixed - gap).max(180.0);

            let mut builder = egui_extras::TableBuilder::new(ui)
                .id_salt("git")
                // Without this a table senses only hover, and no row in it is
                // ever reported as clicked.
                .sense(egui::Sense::click())
                .cell_layout(Layout::left_to_right(Align::Center))
                .column(egui_extras::Column::exact(name_width));
            builder = builder.column(egui_extras::Column::exact(150.0));
            if has("branch") {
                builder = builder.column(egui_extras::Column::exact(150.0));
            }
            if has("changes") {
                builder = builder.column(egui_extras::Column::exact(80.0));
            }
            if has("last") {
                builder = builder.column(egui_extras::Column::exact(110.0));
            }
            if has("host") {
                builder = builder.column(egui_extras::Column::exact(96.0));
            }
            builder = builder.column(egui_extras::Column::exact(150.0));

            builder
                .header(size::TABLE_HEADER_H, |mut header| {
                    header.col(|ui| {
                        widgets::table_header(ui, copy::git::COL_REPO, None);
                    });
                    header.col(|ui| {
                        widgets::table_header(ui, copy::git::COL_STATE, None);
                    });
                    if has("branch") {
                        header.col(|ui| {
                            widgets::table_header(ui, copy::git::COL_BRANCH, None);
                        });
                    }
                    if has("changes") {
                        header.col(|ui| {
                            widgets::table_header(ui, copy::git::COL_CHANGES, None);
                        });
                    }
                    if has("last") {
                        header.col(|ui| {
                            widgets::table_header(ui, copy::git::COL_LAST, None);
                        });
                    }
                    if has("host") {
                        header.col(|ui| {
                            widgets::table_header(ui, copy::git::COL_HOST, None);
                        });
                    }
                    header.col(|ui| {
                        widgets::table_header(ui, copy::git::COL_ACTIONS, None);
                    });
                })
                .body(|body| {
                    body.rows(52.0, rows.len(), |mut row| {
                        let index = row.index();
                        let Some(repo) = rows.get(index) else { return };
                        let state = repo.state();
                        // Set by the buttons in the last column, so a click on
                        // one of them does not also toggle the panel.
                        let mut handled = false;

                        row.col(|ui| {
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 0.0;
                                widgets::elided(
                                    ui,
                                    &repo.name,
                                    Type::BodyStrong,
                                    t.text_primary,
                                    name_width - 12.0,
                                    false,
                                );
                                // The path from the left would elide the part
                                // that differs between two projects with the
                                // same folder name; from the right it keeps it.
                                widgets::elided(
                                    ui,
                                    &repo.path.display().to_string(),
                                    Type::MonoSmall,
                                    t.text_muted,
                                    name_width - 12.0,
                                    true,
                                );
                            });
                        });
                        row.col(|ui| {
                            let response =
                                widgets::badge(ui, git_status(state, &t), None, state.label());
                            response.on_hover_text(state.explanation());
                        });
                        if has("branch") {
                            row.col(|ui| {
                                let label = match &repo.branch {
                                    Some(b) => b.clone(),
                                    None => "—".to_string(),
                                };
                                let suffix = match (repo.ahead, repo.behind) {
                                    (0, 0) => String::new(),
                                    (a, 0) => format!("  ↑{a}"),
                                    (0, b) => format!("  ↓{b}"),
                                    (a, b) => format!("  ↑{a} ↓{b}"),
                                };
                                widgets::elided(
                                    ui,
                                    &format!("{label}{suffix}"),
                                    Type::Small,
                                    t.text_secondary,
                                    138.0,
                                    false,
                                );
                            });
                        }
                        if has("changes") {
                            row.col(|ui| {
                                let n =
                                    repo.staged + repo.unstaged + repo.untracked + repo.conflicted;
                                if n == 0 {
                                    widgets::muted_cell(ui, "—");
                                } else {
                                    let response = widgets::count_pill(ui, &n.to_string());
                                    response.on_hover_text(format!(
                                        "{} staged, {} unstaged, {} untracked, {} conflicted",
                                        repo.staged, repo.unstaged, repo.untracked, repo.conflicted
                                    ));
                                }
                            });
                        }
                        if has("last") {
                            row.col(|ui| {
                                let text = repo
                                    .last_commit_at
                                    .map(|at| format::relative(at, now))
                                    .unwrap_or_else(|| "never".into());
                                let response = widgets::text(ui, text, Type::Small, t.text_muted);
                                if let Some(summary) = &repo.last_commit_summary {
                                    let author =
                                        repo.last_commit_author.clone().unwrap_or_default();
                                    response.on_hover_text(format!("{summary}\n— {author}"));
                                }
                            });
                        }
                        if has("host") {
                            row.col(|ui| match repo.primary_remote() {
                                Some(remote) => {
                                    let response = widgets::text(
                                        ui,
                                        remote.forge.label(),
                                        Type::Small,
                                        t.text_muted,
                                    );
                                    response.on_hover_text(remote.url.clone());
                                }
                                None => widgets::muted_cell(ui, "—"),
                            });
                        }
                        row.col(|ui| {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = space::XS;
                                if state == RepoState::NotTrusted {
                                    let response =
                                        Button::secondary(copy::git::TRUST).enabled(!busy).show(ui);
                                    if response.on_hover_text(copy::git::TRUST_HINT).clicked() {
                                        trust = Some(repo.path.clone());
                                    }
                                    return;
                                }
                                // Only the action this row actually needs. A
                                // clean repository with pull, commit and push
                                // beside it is three ways to do nothing.
                                if matches!(state, RepoState::PullRecommended)
                                    && widgets::icon_button_compact(
                                        ui,
                                        Icon::Download,
                                        copy::git::PULL_HINT,
                                        !busy,
                                    )
                                    .clicked()
                                {
                                    pull = Some(repo.path.clone());
                                }
                                if state == RepoState::Uncommitted
                                    && widgets::icon_button_compact(
                                        ui,
                                        Icon::Check,
                                        copy::git::COMMIT_HINT,
                                        !busy,
                                    )
                                    .clicked()
                                {
                                    commit = Some(repo.path.clone());
                                }
                                if matches!(state, RepoState::Unpushed | RepoState::NoUpstream)
                                    && widgets::icon_button_compact(
                                        ui,
                                        Icon::ExternalLink,
                                        copy::git::PUSH_HINT,
                                        !busy,
                                    )
                                    .clicked()
                                {
                                    push = Some(repo.path.clone());
                                }
                                // The repository's page on its host, one
                                // click from the list. The address was
                                // reachable only by opening the row's dialog
                                // and reading the Remotes card, which is a
                                // long way round for "where does this live?".
                                if let Some(url) =
                                    repo.primary_remote().and_then(|remote| remote.web_url.clone())
                                {
                                    if widgets::icon_button_compact(
                                        ui,
                                        Icon::Globe,
                                        // The address itself is the tooltip,
                                        // so hovering answers the question
                                        // without leaving the application.
                                        &copy::git_open_on_web(&url),
                                        true,
                                    )
                                    .clicked()
                                    {
                                        visit = Some(url);
                                    }
                                }
                                if widgets::icon_button_compact(
                                    ui,
                                    Icon::Folder,
                                    copy::git::OPEN,
                                    true,
                                )
                                .clicked()
                                {
                                    open = Some(repo.path.clone());
                                }
                            });
                            handled = ui.rect_contains_pointer(ui.min_rect());
                        });
                        if row.response().clicked() && !handled {
                            expand = Some(repo.path.clone());
                        }
                    });
                });
        });

        // A dialog, not a panel below the table.
        //
        // Nineteen repositories make a table taller than the window, so a
        // panel underneath it was a panel nobody saw: you clicked a row near
        // the top and the answer rendered thirteen hundred pixels down.
        // Scrolling to it helped and still meant losing your place in the
        // list. A dialog appears where you are already looking, and closing it
        // leaves the table exactly as you left it.
        if let Some(path) = expand {
            if let Some(repo) = rows.iter().find(|r| r.path == path).cloned() {
                self.modal = Some(crate::gui::modals::Modal::GitRepo(Box::new(
                    crate::gui::modals::GitRepoState { repo, tab: RepoTab::default() },
                )));
            }
        }

        if let Some(path) = open {
            let _ = open::that_detached(&path);
        }
        if let Some(url) = visit {
            // Only ever a web address the inventory derived from the remote,
            // never a string typed anywhere: see `git::parse::web_url`, which
            // builds it from a recognised host rather than echoing the URL.
            let _ = open::that_detached(&url);
        }
        if let Some(path) = pull {
            self.git_act(Request::GitPull { path: path.display().to_string() }, path);
        }
        if let Some(path) = commit {
            if let Some(repo) = rows.iter().find(|r| r.path == path) {
                self.modal = Some(crate::gui::modals::Modal::GitCommit(
                    crate::gui::modals::GitCommitState {
                        path: repo.path.clone(),
                        name: repo.name.clone(),
                        // Pre-filled and fully editable. An empty box is what
                        // produces "wip" and "changes"; a suggestion that says
                        // what happened, and that superbackup made it, is
                        // something the user can accept, edit or replace.
                        message: superbackup_core::git::suggested_commit_message(
                            &repo.name,
                            repo.staged + repo.unstaged + repo.untracked + repo.conflicted,
                            now,
                        ),
                        include_untracked: true,
                        changes: repo.staged + repo.unstaged + repo.untracked + repo.conflicted,
                    },
                ));
            }
        }
        if let Some(path) = push {
            let name = rows
                .iter()
                .find(|r| r.path == path)
                .map(|r| r.name.clone())
                .unwrap_or_else(|| path.display().to_string());
            let remote = rows
                .iter()
                .find(|r| r.path == path)
                .and_then(|r| r.primary_remote().map(|x| x.url.clone()))
                .unwrap_or_default();
            self.modal = Some(crate::gui::modals::Modal::Confirm(
                crate::gui::modals::Confirm::new(
                    copy::git::PUSH_TITLE,
                    copy::git::PUSH_BODY,
                    copy::git::PUSH_CONFIRM,
                )
                .bullet(format!("{name} → {remote}"))
                .action(crate::gui::modals::ConfirmAction::GitPush(path)),
            ));
        }
        if let Some(path) = trust {
            self.modal = Some(crate::gui::modals::Modal::Confirm(
                crate::gui::modals::Confirm::new(
                    copy::git::TRUST_TITLE,
                    copy::git::TRUST_BODY,
                    copy::git::TRUST_CONFIRM,
                )
                .bullet(path.display().to_string())
                .action(crate::gui::modals::ConfirmAction::GitTrust(path)),
            ));
        }
    }

    /// Folders under the sources that are not repositories.
    ///
    /// The most exposed thing on a developer disk is a project nobody ever ran
    /// `git init` in: no history, no remote, no second copy anywhere — and
    /// invisible to a list of repositories, which is why it needs its own
    /// place on this page rather than a line in a summary.
    fn git_candidates(&mut self, ui: &mut Ui, inventory: &Inventory) {
        let t = theme::tokens(ui.ctx());
        if inventory.candidates.is_empty() {
            return;
        }
        let projects = inventory.candidates.iter().filter(|c| c.looks_like_a_project).count();

        ui.add_space(space::XL);
        widgets::section_header(
            ui,
            copy::git::NOT_TRACKED,
            Some(inventory.candidates.len()),
            |_| {},
        );
        ui.add_space(space::S);
        widgets::paragraph(
            ui,
            if projects > 0 {
                copy::git_untracked_projects(projects)
            } else {
                copy::git::NOT_TRACKED_BODY.to_string()
            },
            Type::Small,
            if projects > 0 { t.warning.tint_text } else { t.text_muted },
        );
        ui.add_space(space::M);

        let mut start: Option<superbackup_core::git::Candidate> = None;
        // The same data grid as the repository table above.
        //
        // These were cards-in-a-frame while everything else on the page was a
        // table, so one list of folders was read one way and the list directly
        // above it another. Nothing about "not in git" makes it a different
        // kind of row.
        widgets::table_frame(ui, |ui| {
            let gap = ui.spacing().item_spacing.x;
            let name_width = (ui.available_width()
                - CANDIDATE_MARK_W
                - CANDIDATE_ITEMS_W
                - CANDIDATE_ACTION_W
                - gap * 3.0)
                .max(200.0);
            egui_extras::TableBuilder::new(ui)
                .id_salt("git-candidates")
                .sense(egui::Sense::click())
                .cell_layout(Layout::left_to_right(Align::Center))
                .column(egui_extras::Column::exact(name_width))
                .column(egui_extras::Column::exact(CANDIDATE_MARK_W))
                .column(egui_extras::Column::exact(CANDIDATE_ITEMS_W))
                .column(egui_extras::Column::exact(CANDIDATE_ACTION_W))
                .header(size::TABLE_HEADER_H, |mut header| {
                    header.col(|ui| {
                        widgets::table_header(ui, copy::git::COL_FOLDER, None);
                    });
                    header.col(|ui| {
                        widgets::table_header(ui, copy::git::COL_LOOKS_LIKE, None);
                    });
                    header.col(|ui| {
                        widgets::table_header(ui, copy::git::COL_ITEMS, None);
                    });
                    header.col(|ui| {
                        widgets::table_header(ui, "", None);
                    });
                })
                .body(|body| {
                    body.rows(size::TABLE_ROW_H, inventory.candidates.len(), |mut row| {
                        let index = row.index();
                        let Some(candidate) = inventory.candidates.get(index) else {
                            return;
                        };
                        row.col(|ui| {
                            let icon_w = 16.0 + space::M;
                            let (rect, _) =
                                ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                            Icon::Folder.paint(ui.painter(), rect, t.text_muted);
                            ui.add_space(space::M);
                            // Name over path, centred in the row: the same
                            // two-line cell the providers table uses, and for
                            // the same reason a plain `vertical` would sit it
                            // against the top edge.
                            let room = (name_width - icon_w - space::M).max(120.0);
                            widgets::stacked_cell(ui, &[Type::BodyStrong, Type::MonoSmall], |ui| {
                                widgets::elided(
                                    ui,
                                    &candidate.name,
                                    Type::BodyStrong,
                                    t.text_primary,
                                    room,
                                    false,
                                );
                                widgets::elided(
                                    ui,
                                    &candidate.path.display().to_string(),
                                    Type::MonoSmall,
                                    t.text_muted,
                                    room,
                                    false,
                                );
                            });
                        });
                        row.col(|ui| {
                            if candidate.looks_like_a_project {
                                widgets::badge(
                                    ui,
                                    t.warning,
                                    None,
                                    copy::git::LOOKS_LIKE_A_PROJECT,
                                )
                                .on_hover_text(copy::git::LOOKS_LIKE_A_PROJECT_HINT);
                            } else {
                                widgets::muted_cell(ui, "—");
                            }
                        });
                        row.col(|ui| {
                            widgets::text(
                                ui,
                                copy::git_entry_count(candidate.entries),
                                Type::Small,
                                t.text_muted,
                            );
                        });
                        row.col(|ui| {
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if Button::secondary(copy::git::START_TRACKING)
                                    .compact()
                                    .show(ui)
                                    .on_hover_text(copy::git::START_TRACKING_HINT)
                                    .clicked()
                                {
                                    start = Some(candidate.clone());
                                }
                            });
                        });
                    });
                });
        });

        if let Some(candidate) = start {
            self.modal = Some(crate::gui::modals::Modal::GitInit(Box::new(
                crate::gui::modals::GitInitState {
                    installing_gh: false,
                    path: candidate.path.clone(),
                    name: candidate.name.clone(),
                    branch: "main".to_string(),
                    // A first commit by default: a repository whose first
                    // commit is empty protects nothing, and the folder already
                    // exists precisely because there is something in it.
                    commit: true,
                    message: format!("Start tracking {}", candidate.name),
                    create_remote: false,
                    remote_name: candidate.name.clone(),
                    private: true,
                    busy: false,
                    error: None,
                },
            )));
        }
    }

    /// Everything about one repository, under the row that was clicked.
    ///
    /// A separate panel rather than more columns: branches and worktrees are
    /// lists, and a list does not fit in a table cell. It opens on click, so
    /// the table stays a table for the forty repositories nobody is currently
    /// interested in.
    /// Apply whatever the details panel asked for.
    ///
    /// The panel itself is a free function that only *reports* what was
    /// clicked, so it can be rendered inside a modal — where borrowing the
    /// whole `App` mutably is not available — as well as on a page.
    pub(crate) fn git_detail_action(&mut self, repo: &GitRepo, action: GitDetailAction) {
        match action {
            GitDetailAction::OpenUrl(url) => {
                let _ = open::that_detached(&url);
            }
            GitDetailAction::ReadDocument(document) => {
                // The dialog opens empty and fills in when the daemon answers,
                // so the click is acknowledged immediately rather than after a
                // disk read that might be on a network drive.
                self.modal =
                    Some(crate::gui::modals::Modal::Document(crate::gui::modals::DocumentState {
                        title: format!("{document} — {}", repo.name),
                        path: repo.path.join(&document).display().to_string(),
                        content: String::new(),
                        truncated: false,
                        loading: true,
                        error: None,
                    }));
                self.ask(
                    Intent::GitDocument,
                    Request::GitReadDocument { path: repo.path.display().to_string(), document },
                );
            }
            GitDetailAction::SetExternal(external) => {
                self.screens.git.acting = true;
                self.ask(
                    Intent::GitAction,
                    Request::GitSetExternal { path: repo.path.display().to_string(), external },
                );
            }
        }
    }
}

/// What the details panel was asked to do.
#[derive(Debug, Clone)]
pub enum GitDetailAction {
    OpenUrl(String),
    SetExternal(bool),
    ReadDocument(String),
}

/// Which part of a repository is on screen.
///
/// Tabs rather than one long scroll: a repository with twenty branches and
/// four working trees pushed its remotes and its documents off the bottom, and
/// the reader had to know they were down there to go looking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RepoTab {
    #[default]
    Overview,
    Branches,
    Worktrees,
    Documents,
}

impl RepoTab {
    pub const ALL: [RepoTab; 4] =
        [RepoTab::Overview, RepoTab::Branches, RepoTab::Worktrees, RepoTab::Documents];
}

/// Everything about one repository, as a body that can be put anywhere.
///
/// Returns what the reader asked for; nothing is acted on here, so this can be
/// rendered inside a dialog where the whole `App` is not available to borrow.
pub fn git_details(
    ui: &mut Ui,
    repo: &GitRepo,
    tab: &mut RepoTab,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<GitDetailAction> {
    let t = theme::tokens(ui.ctx());
    let mut action = None;

    // -- header: what this is, and the one decision about it ----------------
    ui.horizontal(|ui| {
        widgets::badge(ui, git_status(repo.state(), &t), None, repo.state().label())
            .on_hover_text(repo.state().explanation());
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let label =
                if repo.external { copy::git::UNMARK_EXTERNAL } else { copy::git::MARK_EXTERNAL };
            if Button::secondary(label).show(ui).on_hover_text(copy::git::EXTERNAL_HINT).clicked() {
                action = Some(GitDetailAction::SetExternal(!repo.external));
            }
        });
    });
    ui.add_space(space::XS);
    widgets::text(ui, repo.path.display().to_string(), Type::MonoSmall, t.text_muted);
    ui.add_space(space::L);

    // -- tabs ---------------------------------------------------------------
    let labels: Vec<String> = RepoTab::ALL
        .iter()
        .map(|candidate| match candidate {
            RepoTab::Overview => copy::git::TAB_OVERVIEW.to_string(),
            RepoTab::Branches => format!("{} ({})", copy::git::TAB_BRANCHES, repo.branches.len()),
            RepoTab::Worktrees => {
                format!("{} ({})", copy::git::TAB_WORKTREES, repo.worktrees.len())
            }
            RepoTab::Documents => {
                format!("{} ({})", copy::git::TAB_DOCUMENTS, repo.documents.len())
            }
        })
        .collect();
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let mut selected = RepoTab::ALL.iter().position(|c| c == tab).unwrap_or(0);
    // Written back unconditionally rather than behind `.changed()`. The tab is
    // derived from `tab` at the top of every frame, so `selected` carries the
    // whole answer and there is nothing a change flag adds — while getting the
    // flag wrong, as this did, silently discards the click.
    widgets::segmented(ui, &mut selected, &refs);
    *tab = RepoTab::ALL[selected.min(RepoTab::ALL.len() - 1)];
    ui.add_space(space::L);

    match tab {
        RepoTab::Overview => {
            if let Some(found) = overview(ui, repo, now, &t) {
                action = Some(found);
            }
        }
        RepoTab::Branches => branches(ui, repo, now, &t),
        RepoTab::Worktrees => worktrees(ui, repo, &t),
        RepoTab::Documents => {
            if let Some(found) = documents(ui, repo, &t) {
                action = Some(found);
            }
        }
    }
    action
}

/// The answers to "what is this and where does it go".
fn overview(
    ui: &mut Ui,
    repo: &GitRepo,
    now: chrono::DateTime<chrono::Utc>,
    t: &theme::Tokens,
) -> Option<GitDetailAction> {
    let mut action = None;

    widgets::kv(ui, copy::git::OV_BRANCH, repo.branch.as_deref().unwrap_or("—"), false);
    widgets::kv(
        ui,
        copy::git::OV_LAST_COMMIT,
        &match repo.last_commit_at {
            Some(at) => format::relative(at, now),
            None => copy::git::NEVER.to_string(),
        },
        false,
    );
    if let Some(subject) = &repo.last_commit_summary {
        widgets::kv(ui, copy::git::OV_SUBJECT, subject, false);
    }
    if let Some(author) = &repo.last_commit_author {
        widgets::kv(ui, copy::git::OV_AUTHOR, author, false);
    }
    widgets::kv(
        ui,
        copy::git::OV_CHANGES,
        &copy::git_changes(repo.staged, repo.unstaged, repo.untracked, repo.conflicted),
        false,
    );

    ui.add_space(space::L);
    widgets::text(ui, copy::git::REMOTES, Type::H3, t.text_primary);
    ui.add_space(space::S);
    if repo.remotes.is_empty() {
        widgets::paragraph(ui, copy::git::NO_REMOTES, Type::Small, t.warning.tint_text);
    }
    for remote in &repo.remotes {
        widgets::card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                widgets::text(ui, &remote.name, Type::BodyStrong, t.text_primary);
                ui.add_space(space::S);
                widgets::neutral_badge(ui, remote.forge.label(), None);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if let Some(url) = &remote.web_url {
                        if widgets::link(ui, copy::git::OPEN_REMOTE).clicked() {
                            action = Some(GitDetailAction::OpenUrl(url.clone()));
                        }
                    }
                });
            });
            ui.add_space(space::XS);
            widgets::text(ui, &remote.url, Type::MonoSmall, t.text_muted);
            ui.add_space(space::XS);
            ui.horizontal(|ui| {
                widgets::text(ui, copy::git::AUTH, Type::Small, t.text_muted);
                ui.add_space(space::XS);
                widgets::text(ui, remote.auth.label(), Type::Small, t.text_secondary)
                    .on_hover_text(remote.auth.detail());
            });
        });
        ui.add_space(space::S);
    }
    if !repo.remotes.is_empty() {
        ui.add_space(space::XS);
        widgets::paragraph(ui, copy::git::AUTH_NOTE, Type::Small, t.text_muted);
    }
    action
}

/// Every branch, as a table. Unpushed work hides on a branch nobody has looked
/// at in months, and a table is what makes twenty of them readable.
fn branches(ui: &mut Ui, repo: &GitRepo, now: chrono::DateTime<chrono::Utc>, t: &theme::Tokens) {
    if repo.branches.is_empty() {
        widgets::paragraph(ui, copy::git::NO_BRANCHES, Type::Small, t.text_muted);
        return;
    }
    let never_pushed = repo.branches.iter().filter(|b| b.upstream.is_none()).count();
    if never_pushed > 0 {
        widgets::banner(
            ui,
            widgets::BannerKind::Warning,
            &copy::git_never_pushed(never_pushed),
            Some(copy::git::NEVER_PUSHED_BODY),
            |_| {},
        );
        ui.add_space(space::M);
    }

    widgets::table_frame(ui, |ui| {
        let width = ui.available_width();
        let name_w = (width * 0.42).clamp(160.0, 420.0);
        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(
                Vec2::new(name_w, 18.0),
                Layout::left_to_right(Align::Center),
                |ui| widgets::table_header(ui, copy::git::COL_BRANCH, None),
            );
            ui.allocate_ui_with_layout(
                Vec2::new(150.0, 18.0),
                Layout::left_to_right(Align::Center),
                |ui| widgets::table_header(ui, copy::git::COL_TRACKING, None),
            );
            widgets::table_header(ui, copy::git::COL_LAST, None);
        });
        widgets::divider(ui);

        for branch in &repo.branches {
            ui.horizontal(|ui| {
                ui.set_min_height(30.0);
                ui.allocate_ui_with_layout(
                    Vec2::new(name_w, 26.0),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        if branch.current {
                            widgets::text(ui, "●", Type::Small, t.success.mark)
                                .on_hover_text(copy::git::CURRENT);
                            ui.add_space(space::XS);
                        }
                        widgets::elided(
                            ui,
                            &branch.name,
                            if branch.current { Type::BodyStrong } else { Type::Body },
                            t.text_primary,
                            name_w - 40.0,
                            false,
                        );
                        if branch.checked_out_elsewhere {
                            ui.add_space(space::XS);
                            widgets::neutral_badge(ui, copy::git::ELSEWHERE, None);
                        }
                    },
                );
                ui.allocate_ui_with_layout(
                    Vec2::new(150.0, 26.0),
                    Layout::left_to_right(Align::Center),
                    |ui| match (&branch.upstream, branch.ahead, branch.behind) {
                        (None, _, _) => {
                            widgets::text(
                                ui,
                                copy::git::NO_UPSTREAM,
                                Type::Small,
                                t.warning.tint_text,
                            );
                        }
                        (Some(upstream), 0, 0) => {
                            widgets::elided(ui, upstream, Type::Small, t.text_muted, 140.0, true)
                                .on_hover_text(copy::git::IN_SYNC);
                        }
                        (Some(upstream), ahead, behind) => {
                            let label = match (ahead, behind) {
                                (a, 0) => format!("+{a}"),
                                (0, b) => format!("-{b}"),
                                (a, b) => format!("+{a} -{b}"),
                            };
                            widgets::text(ui, label, Type::MonoSmall, t.warning.tint_text)
                                .on_hover_text(upstream);
                        }
                    },
                );
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    match branch.last_commit {
                        Some(at) => {
                            widgets::text(ui, format::relative(at, now), Type::Small, t.text_muted);
                        }
                        None => widgets::muted_cell(ui, "—"),
                    }
                    if !branch.subject.is_empty() {
                        let room = ui.available_width().max(80.0);
                        widgets::elided(
                            ui,
                            &branch.subject,
                            Type::Small,
                            t.text_muted,
                            room,
                            false,
                        )
                        .on_hover_text(&branch.subject);
                    }
                });
            });
            widgets::divider(ui);
        }
    });
}

/// Linked working trees. A repository with three of them has three sets of
/// uncommitted changes, and only one of them is the folder you are looking at.
fn worktrees(ui: &mut Ui, repo: &GitRepo, t: &theme::Tokens) {
    if repo.worktrees.len() > 1 {
        widgets::banner(
            ui,
            widgets::BannerKind::Info,
            &copy::git_worktrees(repo.worktrees.len()),
            Some(copy::git::WORKTREES_BODY),
            |_| {},
        );
        ui.add_space(space::M);
    }
    if repo.worktrees.is_empty() {
        widgets::paragraph(ui, copy::git::ONE_WORKTREE, Type::Small, t.text_muted);
        return;
    }

    for tree in &repo.worktrees {
        widgets::card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                if tree.main {
                    widgets::neutral_badge(ui, copy::git::MAIN_TREE, None);
                    ui.add_space(space::S);
                }
                match &tree.branch {
                    Some(branch) => {
                        widgets::text(ui, branch, Type::BodyStrong, t.text_primary);
                    }
                    None => {
                        widgets::text(
                            ui,
                            copy::git::DETACHED,
                            Type::BodyStrong,
                            t.warning.tint_text,
                        )
                        .on_hover_text(copy::git::DETACHED_HINT);
                    }
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if tree.prunable {
                        widgets::neutral_badge(ui, copy::git::PRUNABLE, Some(Icon::AlertTriangle))
                            .on_hover_text(copy::git::PRUNABLE_HINT);
                    }
                    if tree.locked {
                        widgets::neutral_badge(ui, copy::git::LOCKED, Some(Icon::Lock));
                    }
                });
            });
            ui.add_space(space::XS);
            let room = ui.available_width().max(120.0);
            widgets::elided(ui, &tree.path, Type::MonoSmall, t.text_muted, room, true)
                .on_hover_text(&tree.path);
        });
        ui.add_space(space::S);
    }
}

/// The repository's own documents, offered to read.
fn documents(ui: &mut Ui, repo: &GitRepo, t: &theme::Tokens) -> Option<GitDetailAction> {
    if repo.documents.is_empty() {
        widgets::paragraph(ui, copy::git::NO_DOCUMENTS, Type::Small, t.text_muted);
        return None;
    }
    let mut action = None;
    for document in &repo.documents {
        let response = widgets::row_card(ui, None, |ui: &mut Ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                Icon::FileText.paint(ui.painter(), rect, t.text_secondary);
                ui.add_space(space::M);
                widgets::text(ui, &document.name, Type::BodyStrong, t.text_primary);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    widgets::text(ui, format::bytes(document.bytes), Type::Small, t.text_muted);
                });
            });
        });
        if response.response.interact(Sense::click()).clicked() {
            action = Some(GitDetailAction::ReadDocument(document.name.clone()));
        }
        ui.add_space(space::S);
    }
    ui.add_space(space::S);
    widgets::paragraph(ui, copy::git::DOCUMENTS_NOTE, Type::Small, t.text_muted);
    action
}
impl App {
    /// Send one action and mark the screen busy until it answers.
    pub(crate) fn git_act(&mut self, request: Request, path: std::path::PathBuf) {
        self.screens.git.acting = true;
        self.screens.git.acting_on = Some(path);
        self.ask(Intent::GitAction, request);
    }
}

/// Which of the four status colours a state gets.
///
/// Deliberately not one colour per variant: the screen has one message, which
/// is whether the work here exists anywhere else, and three shades say it
/// better than nine do.
///
/// Named separately from the token lookup so a test can assert the *choice*
/// without a theme, which is the part that carries the meaning.
fn git_severity(state: RepoState) -> Severity {
    match state {
        RepoState::Clean => Severity::Good,
        RepoState::PullRecommended => Severity::Note,
        RepoState::Unreadable
        | RepoState::NotTrusted
        | RepoState::Diverged
        | RepoState::Detached => Severity::Bad,
        _ => Severity::Warn,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Severity {
    Good,
    Note,
    Warn,
    Bad,
}

fn git_status(state: RepoState, t: &theme::Tokens) -> theme::Status {
    match git_severity(state) {
        Severity::Good => t.success,
        Severity::Note => t.info,
        Severity::Warn => t.warning,
        Severity::Bad => t.danger,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The colouring is the screen's whole argument. A repository whose work
    /// is only on this disk must never be green, and one that is committed and
    /// pushed must never be anything else.
    #[test]
    fn colour_follows_whether_the_work_exists_anywhere_else() {
        assert_eq!(git_severity(RepoState::Clean), Severity::Good);
        for at_risk in [
            RepoState::Uncommitted,
            RepoState::Unpushed,
            RepoState::NoRemote,
            RepoState::NoUpstream,
        ] {
            assert_ne!(
                git_severity(at_risk),
                Severity::Good,
                "{at_risk:?} is only on this disk and must not read as safe"
            );
            assert!(at_risk.only_on_this_disk());
        }
        // Being behind risks nothing, so it is information rather than a
        // warning — the one state people would otherwise learn to ignore.
        assert_eq!(git_severity(RepoState::PullRecommended), Severity::Note);
        assert!(!RepoState::PullRecommended.only_on_this_disk());
    }

    /// Every state has both a label and a sentence, because the badge is two
    /// words and the tooltip is where the meaning is.
    #[test]
    fn every_state_says_what_it_means_for_the_data() {
        for state in [
            RepoState::Unreadable,
            RepoState::NotTrusted,
            RepoState::Detached,
            RepoState::Diverged,
            RepoState::Uncommitted,
            RepoState::Unpushed,
            RepoState::NoRemote,
            RepoState::NoUpstream,
            RepoState::PullRecommended,
            RepoState::Clean,
        ] {
            assert!(!state.label().is_empty(), "{state:?}");
            let explanation = state.explanation();
            assert!(explanation.len() > 30, "{state:?}: {explanation}");
            assert!(explanation.ends_with('.'), "{state:?}: {explanation}");
        }
    }

    /// The bulk pull walks the queue once and stops, and reports at the end.
    ///
    /// The failure worth guarding is the sequencing: a run that re-armed
    /// itself, or one that stopped at the first repository git declined,
    /// would both look plausible on screen. The first never ends; the second
    /// silently refreshes three of forty.
    #[test]
    fn a_bulk_pull_visits_every_repository_and_survives_a_refusal() {
        let mut state = State::default();
        assert!(!state.pulling_all(), "idle to begin with");

        let paths: Vec<std::path::PathBuf> =
            ["/a", "/b", "/c"].iter().map(std::path::PathBuf::from).collect();
        state.start_bulk_pull(paths.clone());
        assert!(state.pulling_all());
        assert_eq!(state.pull_progress(), (0, 3));

        // Walk it the way the reply handler does.
        let mut visited = Vec::new();
        while let Some(path) = state.pull_queue.pop_front() {
            visited.push(path);
            // The middle one is refused; the run must carry on regardless.
            if visited.len() == 2 {
                state.pull_failed.push("git said no".into());
            }
        }
        assert_eq!(visited, paths, "every repository, in order, once each");
        assert_eq!(state.pull_progress(), (3, 3));
        assert_eq!(state.pull_failed.len(), 1, "the refusal is kept for the summary");

        state.bulk_pull_finished();
        assert!(!state.pulling_all(), "and the run ends rather than re-arming");
        assert_eq!(state.pull_progress(), (0, 0));
    }

    /// The summary names the count and carries git's own words, capped so a
    /// laptop closed for a fortnight cannot produce a toast taller than the
    /// window.
    #[test]
    fn the_bulk_pull_summary_is_specific_and_bounded() {
        let clean = crate::gui::copy::git_pulled_all(4);
        assert!(clean.contains('4'), "{clean}");

        let failures: Vec<String> = (0..9).map(|i| format!("repo-{i} refused")).collect();
        let message = crate::gui::copy::git_pull_all_failed(12, &failures);
        assert!(message.contains("3 of 12"), "how many worked: {message}");
        assert!(message.contains("repo-0 refused"), "and git's own words: {message}");
        assert!(message.contains("4 more"), "the rest are counted, not listed: {message}");
        assert!(!message.contains("repo-8"), "and not all nine are printed: {message}");
    }
}
