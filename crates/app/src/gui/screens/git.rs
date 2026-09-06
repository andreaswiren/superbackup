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

use egui::{Align, Layout, Ui};

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
}

/// `UX_SPEC` §9. The repository name and its state never drop: without both,
/// the row says nothing the screen exists to say.
const GIT_COLUMNS: [ColumnSpec; 7] = [
    ColumnSpec::keep("repo", 200.0),
    ColumnSpec::keep("state", 150.0),
    ColumnSpec::droppable("branch", 150.0, 2),
    ColumnSpec::droppable("changes", 80.0, 3),
    ColumnSpec::droppable("last", 110.0, 1),
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
    fn git_summary(&mut self, ui: &mut Ui, inventory: &Inventory, now: chrono::DateTime<chrono::Utc>) {
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
                                let n = repo.staged + repo.unstaged + repo.untracked
                                    + repo.conflicted;
                                if n == 0 {
                                    widgets::muted_cell(ui, "—");
                                } else {
                                    let response = widgets::count_pill(ui, &n.to_string());
                                    response.on_hover_text(format!(
                                        "{} staged, {} unstaged, {} untracked, {} conflicted",
                                        repo.staged,
                                        repo.unstaged,
                                        repo.untracked,
                                        repo.conflicted
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
                                    let response = Button::secondary(copy::git::TRUST)
                                        .enabled(!busy)
                                        .show(ui);
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
                        });
                    });
                });
        });

        if let Some(path) = open {
            let _ = open::that_detached(&path);
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
                        message: String::new(),
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
}
