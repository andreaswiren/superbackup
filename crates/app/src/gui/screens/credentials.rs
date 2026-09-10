//! The keys and tokens this machine signs in with.
//!
//! # Why this page exists
//!
//! A developer machine holds two things a file backup does not obviously
//! cover. The *work* is what the Git page is about. The *credentials* are what
//! get you back to the work — and losing a laptop with an SSH key on it and no
//! copy of that key means re-keying every forge, every server and every deploy
//! target by hand, from a machine that cannot reach any of them.
//!
//! # What it will not do
//!
//! Write a private key anywhere in the clear. "Share with my other machines"
//! seals the keys under the master passphrase first, so what lands in OneDrive
//! or a bucket is bytes rather than a working key. There is no setting to turn
//! that off, because a plaintext option exists to be chosen by whoever is
//! least able to judge the consequence.

use egui::{Align, Layout, Sense, Ui, Vec2};

use superbackup_core::credentials::{Credential, CredentialKind};
use superbackup_core::ipc::protocol::Request;

use crate::gui::app::App;
use crate::gui::copy;
use crate::gui::daemon::Intent;
use crate::gui::icons::Icon;
use crate::gui::theme::{self, space, Type};
use crate::gui::widgets::{self, Button};

#[derive(Default)]
pub struct State {
    pub credentials: Vec<Credential>,
    pub sync_folder: Option<String>,
    pub loading: bool,
    pub asked: bool,
    pub error: Option<String>,
    /// What the last seal or unseal did, kept on screen rather than as a toast
    /// that scrolls away — this is the one page where "which files moved
    /// where" is worth being able to re-read.
    pub last_bundle: Option<String>,
    /// Which agent is running and what it holds.
    pub agent: Option<superbackup_core::credentials::agent::AgentStatus>,
    /// The public key of the pair just made, kept on screen because pasting it
    /// into a forge is the very next thing anybody does.
    pub last_public_key: Option<String>,
}

impl State {
    pub fn busy(&self) -> bool {
        self.loading
    }
    pub fn arrived(&mut self, credentials: Vec<Credential>, sync_folder: Option<String>) {
        self.credentials = credentials;
        self.sync_folder = sync_folder;
        self.loading = false;
        self.error = None;
    }
    pub fn failed(&mut self, why: String) {
        self.loading = false;
        self.error = Some(why);
    }
}

impl App {
    pub(crate) fn credentials_actions(&mut self, ui: &mut Ui) {
        if Button::primary(copy::cred::NEW_KEY).icon(Icon::Plus).show(ui).clicked() {
            let machine = self.data.machine_label();
            self.modal = Some(crate::gui::modals::Modal::NewKey(crate::gui::modals::NewKeyState {
                name: "id_ed25519".to_string(),
                comment: format!("{}@{machine}", account_name()),
                // Opening it is the point of making it here rather than in
                // a terminal, so it is on by default.
                load_into_agent: true,
                ..Default::default()
            }));
        }
        if Button::secondary(copy::cred::RESCAN)
            .icon(Icon::RefreshCw)
            .enabled(!self.screens.credentials.loading)
            .show(ui)
            .clicked()
        {
            self.scan_credentials();
        }
    }

    pub(crate) fn scan_credentials(&mut self) {
        self.screens.credentials.loading = true;
        self.screens.credentials.asked = true;
        self.ask(Intent::Credentials, Request::CredentialList {});
        self.ask(Intent::AgentStatus, Request::CredentialAgentStatus {});
    }

    pub(crate) fn show_credentials(&mut self, ui: &mut Ui) {
        let t = theme::tokens(ui.ctx());
        if !self.screens.credentials.asked {
            self.scan_credentials();
        }
        if let Some(error) = self.screens.credentials.error.clone() {
            widgets::banner(
                ui,
                widgets::BannerKind::Danger,
                copy::cred::FAILED,
                Some(&error),
                |_| {},
            );
            ui.add_space(space::L);
        }

        let credentials = self.screens.credentials.credentials.clone();
        if credentials.is_empty() && !self.screens.credentials.loading {
            widgets::empty_state(
                ui,
                Icon::KeyRound,
                &crate::gui::copy::Empty {
                    title: copy::cred::EMPTY,
                    body: copy::cred::EMPTY_BODY,
                    primary: None,
                    secondary: None,
                },
                None,
            );
            return;
        }

        widgets::scroll_area(ui, "credentials", |ui| {
            // The promise, stated where the decision is made rather than in
            // documentation nobody opens.
            widgets::banner(
                ui,
                widgets::BannerKind::Info,
                copy::cred::SEALED_TITLE,
                Some(copy::cred::SEALED_BODY),
                |_| {},
            );
            ui.add_space(space::XL);

            // The one thing this page could not do.
            //
            // It lists the keys, explains that they are sealed before they
            // leave the machine, and offered no way to actually copy them
            // anywhere — the job that does it had to be built by hand through
            // the wizard, choosing a template most people would never guess
            // was the right one.
            self.credential_backup(ui, &t);

            let mut role: Option<(String, bool, bool)> = None;
            for credential in &credentials {
                if let Some(change) = self.credential_card(ui, credential, &t) {
                    role = Some(change);
                }
            }
            if let Some((path, backed_up, synced)) = role {
                self.ask(
                    Intent::CredentialRole,
                    Request::CredentialSetRole { path, backed_up, synced },
                );
            }

            // The public half of a key just made, kept where it can be copied
            // rather than as a toast that has already gone.
            if let Some(public_key) = self.screens.credentials.last_public_key.clone() {
                widgets::card(ui, |ui| {
                    ui.set_width(ui.available_width());
                    widgets::text(ui, copy::cred::NEW_MADE, Type::H3, t.text_primary);
                    ui.add_space(space::S);
                    if widgets::code_block(ui, &public_key, 120.0, None) {
                        self.toasts.success(copy::toast::COPIED_CLIPBOARD);
                    }
                });
                ui.add_space(space::XL);
            }

            ui.add_space(space::XL);
            self.credentials_agent_section(ui, &credentials, &t);
            ui.add_space(space::XL);
            self.credentials_sync_section(ui, &credentials, &t);
        });
    }

    /// One key or token.
    fn credential_card(
        &mut self,
        ui: &mut Ui,
        credential: &Credential,
        t: &theme::Tokens,
    ) -> Option<(String, bool, bool)> {
        let mut change = None;
        widgets::card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                Icon::KeyRound.paint(ui.painter(), rect, t.text_secondary);
                ui.add_space(space::M);
                widgets::text(ui, &credential.name, Type::BodyStrong, t.text_primary);
                ui.add_space(space::S);
                widgets::neutral_badge(ui, credential.kind.label(), None);

                if let CredentialKind::SshKey(key) = &credential.kind {
                    // Whether a key needs a passphrase decides whether it can
                    // be loaded at login without somebody typing one, so it is
                    // said on the row rather than left to be found out.
                    match key.encrypted {
                        Some(true) => {
                            ui.add_space(space::S);
                            widgets::badge(ui, t.success, Some(Icon::Lock), copy::cred::ENCRYPTED)
                                .on_hover_text(copy::cred::ENCRYPTED_HINT);
                        }
                        Some(false) => {
                            ui.add_space(space::S);
                            widgets::badge(
                                ui,
                                t.warning,
                                Some(Icon::LockOpen),
                                copy::cred::UNENCRYPTED,
                            )
                            .on_hover_text(copy::cred::UNENCRYPTED_HINT);
                        }
                        None => {}
                    }
                    if key.world_readable {
                        ui.add_space(space::S);
                        widgets::badge(
                            ui,
                            t.danger,
                            Some(Icon::AlertTriangle),
                            copy::cred::WIDE_OPEN,
                        )
                        .on_hover_text(copy::cred::WIDE_OPEN_HINT);
                    }
                }
            });

            if let CredentialKind::SshKey(key) = &credential.kind {
                ui.add_space(space::S);
                widgets::text(
                    ui,
                    key.private_path.display().to_string(),
                    Type::MonoSmall,
                    t.text_muted,
                );
                ui.add_space(space::XS);
                ui.horizontal(|ui| {
                    if let Some(algorithm) = &key.algorithm {
                        widgets::text(ui, algorithm, Type::Small, t.text_secondary);
                        ui.add_space(space::M);
                    }
                    if let Some(comment) = &key.comment {
                        widgets::text(ui, comment, Type::Small, t.text_muted);
                    }
                });
                if let Some(fingerprint) = &key.fingerprint {
                    ui.add_space(space::XS);
                    // The form every forge shows, so a key here can be matched
                    // against a key in GitHub's settings by eye.
                    let response = widgets::text(ui, fingerprint, Type::MonoSmall, t.text_muted);
                    if response.on_hover_text(copy::cred::FINGERPRINT_HINT).clicked() {
                        ui.ctx().copy_text(fingerprint.clone());
                    }
                }

                ui.add_space(space::M);
                // Two columns of an explicit half-width each.
                //
                // A checkbox with helper text wraps that text to the width it
                // is given, and inside a plain `horizontal` the first one is
                // given everything left in the row. Its helper then filled the
                // card, and the second checkbox began past the right edge —
                // which is exactly how "Share with my other machines"
                // disappeared off the side of the page.
                let column = ((ui.available_width() - space::XL) / 2.0).max(220.0);
                ui.horizontal_top(|ui| {
                    for (index, (label, hint, value)) in [
                        (copy::cred::BACK_UP, copy::cred::BACK_UP_HINT, credential.backed_up),
                        (copy::cred::SHARE, copy::cred::SHARE_HINT, credential.synced),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        // Width fixed, height left to grow with the wrapped
                        // helper: the two hints are different lengths and
                        // pinning a height would clip the longer one.
                        ui.allocate_ui_with_layout(
                            Vec2::new(column, 0.0),
                            Layout::top_down(Align::Min),
                            |ui| {
                                ui.set_min_width(column);
                                let mut on = value;
                                if widgets::checkbox(ui, &mut on, label, Some(hint), true).clicked()
                                {
                                    change = Some(if index == 0 {
                                        (credential.id.clone(), on, credential.synced)
                                    } else {
                                        (credential.id.clone(), credential.backed_up, on)
                                    });
                                }
                            },
                        );
                        if index == 0 {
                            ui.add_space(space::XL);
                        }
                    }
                });
            }

            if let CredentialKind::GitHubCli { account } = &credential.kind {
                ui.add_space(space::S);
                widgets::paragraph(ui, copy::cred::GH_BODY, Type::Small, t.text_muted);
                if let Some(account) = account {
                    ui.add_space(space::XS);
                    widgets::kv(ui, copy::cred::ACCOUNT, account, false);
                }
            }
        });
        ui.add_space(space::M);
        change
    }

    /// Offer, in one press, a job that backs up nothing but these keys.
    ///
    /// # Why a job of its own rather than a folder in an existing one
    ///
    /// Because what it backs up is not a folder. A keys job carries no
    /// sources: the payload is built when the run starts, by sealing each key
    /// under the master passphrase into a bundle, and it is the bundle that
    /// reaches the destination. A private key is never written anywhere in the
    /// clear — not to a folder mirror, not to OneDrive, not to a bucket — and
    /// that is a property of how the payload is made, not a setting.
    ///
    /// So the job cannot be "add `~/.ssh` to my documents backup". That would
    /// copy the keys as they are.
    fn credential_backup(&mut self, ui: &mut Ui, t: &theme::Tokens) {
        use superbackup_core::model::JobContent;

        // Already done? Say so and stop offering.
        if let Some(job) = self.data.jobs.iter().find(|j| j.content == JobContent::Keys) {
            let name = job.name.clone();
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                Icon::CheckCircle.paint(ui.painter(), rect, t.success.mark);
                ui.add_space(space::M);
                widgets::text(ui, copy::cred::BACKUP_DONE, Type::BodyStrong, t.text_primary);
                ui.add_space(space::S);
                widgets::text(ui, copy::cred_backup_exists(&name), Type::Small, t.text_secondary);
            });
            ui.add_space(space::XL);
            return;
        }

        let mut make = false;
        widgets::banner(
            ui,
            widgets::BannerKind::Warning,
            copy::cred::BACKUP_TITLE,
            Some(copy::cred::BACKUP_BODY),
            |ui| {
                if Button::primary(copy::cred::BACKUP_ACTION)
                    .icon(Icon::KeyRound)
                    .compact()
                    .show(ui)
                    .clicked()
                {
                    make = true;
                }
            },
        );
        ui.add_space(space::XL);

        if make {
            self.create_keys_job();
        }
    }

    /// Build and send the keys job.
    ///
    /// Every enabled destination, because a key is small and the whole point
    /// of backing one up is that it survives losing a machine — sending it to
    /// one place and not the other five is a choice nobody would make on
    /// purpose. The job page is opened afterwards so the schedule and the
    /// destinations can be changed while the decision is still fresh, rather
    /// than a toast claiming something happened somewhere.
    fn create_keys_job(&mut self) {
        let job = match self.keys_job() {
            Some(job) => job,
            None => {
                self.toasts.warning(copy::cred::BACKUP_NO_DESTINATION);
                return;
            }
        };
        let name = job.name.clone();
        let count = job.destination_ids.len();
        self.ask(Intent::SaveJob(name.clone()), Request::JobCreate { job: Box::new(job) });
        self.toasts.success(copy::cred_backup_made(&name, count));
        self.go(crate::gui::Route::Jobs);
    }

    /// The keys job this machine would get, or `None` when there is nowhere to
    /// put it.
    ///
    /// Separate from sending it so that what it *is* can be asserted without a
    /// daemon: the two properties that matter — that it carries no folders,
    /// and that it reaches every destination that is switched on — are
    /// decided here and are invisible from the other side of an IPC call.
    pub(crate) fn keys_job(&self) -> Option<superbackup_core::model::Job> {
        let destinations: Vec<uuid::Uuid> =
            self.data.destinations.iter().filter(|d| d.enabled).map(|d| d.id).collect();
        if destinations.is_empty() {
            return None;
        }

        let mut job = crate::gui::screens::wizard::blank_job();
        job.content = superbackup_core::model::JobContent::Keys;
        job.name = crate::gui::validation::unique_name(
            copy::cred::BACKUP_JOB_NAME,
            &self.data.jobs.iter().map(|j| j.name.clone()).collect::<Vec<_>>(),
        );
        job.destination_ids = destinations;
        Some(job)
    }

    /// Which agent is running, and whether it holds this machine's keys.
    ///
    /// The heading everybody actually wants answered is "will it ask me for
    /// this key again", and the honest answer depends on which agent is
    /// running — so the agent is named rather than assumed.
    fn credentials_agent_section(
        &mut self,
        ui: &mut Ui,
        credentials: &[Credential],
        t: &theme::Tokens,
    ) {
        let Some(status) = self.screens.credentials.agent.clone() else { return };

        widgets::section_header(ui, copy::cred::AGENT_TITLE, None, |_| {});
        ui.add_space(space::M);
        widgets::banner(
            ui,
            if status.persists_across_reboot {
                widgets::BannerKind::Success
            } else if status.running {
                widgets::BannerKind::Info
            } else {
                widgets::BannerKind::Warning
            },
            if status.persists_across_reboot {
                copy::cred::AGENT_PERSISTS
            } else if status.running {
                copy::cred::AGENT_RUNNING
            } else {
                copy::cred::AGENT_NONE
            },
            Some(&status.note),
            |_| {},
        );
        ui.add_space(space::M);

        // Which of this machine's keys the agent is actually holding —
        // matched by fingerprint, which is the only thing the two lists have
        // in common.
        let mut load: Option<String> = None;
        let mut unload: Option<String> = None;
        // Path of a protected key whose passphrase the user wants to type.
        let mut in_terminal: Option<std::path::PathBuf> = None;
        for credential in credentials {
            let CredentialKind::SshKey(key) = &credential.kind else { continue };
            let Some(fingerprint) = &key.fingerprint else { continue };
            let held = status.holds(fingerprint);

            ui.horizontal(|ui| {
                ui.set_min_height(28.0);
                if held {
                    widgets::badge(ui, t.success, Some(Icon::Check), copy::cred::AGENT_LOADED)
                        .on_hover_text(if status.persists_across_reboot {
                            copy::cred::AGENT_LOADED_PERSISTS
                        } else {
                            copy::cred::AGENT_LOADED_SESSION
                        });
                } else {
                    widgets::neutral_badge(ui, copy::cred::AGENT_NOT_LOADED, None);
                }
                ui.add_space(space::M);
                widgets::text(ui, &credential.name, Type::Body, t.text_primary);

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if held {
                        // Opening a key had no undo in the interface that
                        // offered it: the only way back was restarting the
                        // agent, which on Windows means restarting a service.
                        // A choice about a credential has to be reversible
                        // where it was made.
                        if Button::ghost(copy::cred::AGENT_REMOVE)
                            .compact()
                            .show(ui)
                            .on_hover_text(copy::cred::AGENT_REMOVE_HINT)
                            .clicked()
                        {
                            unload = Some(credential.id.clone());
                        }
                        return;
                    }
                    let protected = key.encrypted == Some(true);
                    if protected {
                        // Superbackup cannot type the passphrase — that is the
                        // argv rule, and it is not going to be bent. But it
                        // can open a terminal and let ssh-add ask, which is
                        // the arrangement where the passphrase goes from the
                        // keyboard to ssh-add and touches neither a command
                        // line nor this process.
                        if Button::secondary(copy::cred::AGENT_TERMINAL)
                            .compact()
                            .enabled(status.running)
                            .show(ui)
                            .on_hover_text(copy::cred::AGENT_TERMINAL_HINT)
                            .clicked()
                        {
                            in_terminal = Some(key.private_path.clone());
                        }
                        return;
                    }
                    let mut button = Button::secondary(copy::cred::AGENT_LOAD).compact();
                    if !status.running {
                        button = button.disabled_because(copy::cred::AGENT_NONE_HINT);
                    }
                    if button.show(ui).clicked() {
                        load = Some(credential.id.clone());
                    }
                });
            });
        }

        if let Some(path) = load {
            self.ask(Intent::AgentAdd, Request::CredentialAgentAdd { path });
        }
        if let Some(path) = unload {
            self.ask(Intent::AgentRemove, Request::CredentialAgentRemove { path });
        }
        if let Some(path) = in_terminal {
            self.open_ssh_add_terminal(&path);
        }
    }

    /// Open a terminal running `ssh-add` for a key with its own passphrase.
    ///
    /// Done here in the window rather than through the daemon on purpose. The
    /// daemon may be the Windows service, which runs in session 0 and whose
    /// windows are on a desktop nobody is looking at — so a terminal opened
    /// there would be invisible and would appear to have done nothing. The
    /// window is in the user's own session, which is where a window they are
    /// meant to type into belongs.
    fn open_ssh_add_terminal(&mut self, private_key: &std::path::Path) {
        use superbackup_core::platform::terminal;

        let Some(tool) = superbackup_core::credentials::agent::ssh_add() else {
            self.toasts.warning(copy::cred::AGENT_NONE_HINT);
            return;
        };
        let args = vec![std::ffi::OsString::from(private_key)];
        match terminal::run(&tool, &args, "superbackup — open a key") {
            Ok(_) => self.toasts.info(copy::cred::AGENT_TERMINAL_OPENED),
            Err(_) => {
                // No terminal to open. The command is still the answer, so it
                // is put where the user can read and retype it rather than
                // leaving them with a button that failed.
                self.toasts.warning(format!(
                    "{} {}",
                    copy::cred::AGENT_NO_TERMINAL,
                    terminal::describe(&tool, &args)
                ));
            }
        }
    }

    /// Where the shared bundle goes, and the two buttons that move it.
    fn credentials_sync_section(
        &mut self,
        ui: &mut Ui,
        credentials: &[Credential],
        t: &theme::Tokens,
    ) {
        let shared = credentials.iter().filter(|c| c.synced).count();
        widgets::section_header(ui, copy::cred::SYNC_TITLE, Some(shared), |_| {});
        ui.add_space(space::M);

        if shared == 0 {
            widgets::paragraph(ui, copy::cred::SYNC_NONE, Type::Small, t.text_muted);
            return;
        }

        let mut folder = self.screens.credentials.sync_folder.clone().unwrap_or_default();
        widgets::Field::new()
            .label(copy::cred::SYNC_FOLDER)
            .helper(copy::cred::SYNC_FOLDER_HINT)
            .width(520.0)
            .show(ui, &mut folder);
        self.screens.credentials.sync_folder =
            (!folder.trim().is_empty()).then(|| folder.trim().to_string());

        // The destinations marked as reachable from more than one machine,
        // offered as one-click answers.
        //
        // This is the question the "shared" flag exists for. A bundle written
        // somewhere only this PC can open is a bundle that will never reach
        // the machine that needs it, and nothing about a path says which kind
        // it is — so the folders whose owner has said "my other machines see
        // this" are the ones put in front of them here.
        let candidates: Vec<(String, String)> = self
            .data
            .destinations
            .iter()
            .filter(|d| d.shared)
            .filter_map(|d| d.kind.local_path().map(|p| (d.name.clone(), p.display().to_string())))
            .collect();
        if !candidates.is_empty() {
            ui.add_space(space::S);
            widgets::text(ui, copy::cred::SYNC_SHARED_DESTS, Type::Small, t.text_muted);
            ui.add_space(space::XS);
            let mut pick: Option<String> = None;
            ui.horizontal_wrapped(|ui| {
                for (name, path) in &candidates {
                    if Button::ghost(name).compact().show(ui).on_hover_text(path.clone()).clicked()
                    {
                        pick = Some(path.clone());
                    }
                }
            });
            if let Some(path) = pick {
                self.screens.credentials.sync_folder = Some(path);
            }
        }

        ui.add_space(space::M);
        let ready = self.screens.credentials.sync_folder.is_some();
        let mut seal = false;
        let mut unseal = false;
        ui.horizontal(|ui| {
            if Button::primary(copy::cred::SEAL)
                .icon(Icon::Lock)
                .enabled(ready)
                .show(ui)
                .on_hover_text(copy::cred::SEAL_HINT)
                .clicked()
            {
                seal = true;
            }
            if Button::secondary(copy::cred::UNSEAL)
                .icon(Icon::Download)
                .enabled(ready)
                .show(ui)
                .on_hover_text(copy::cred::UNSEAL_HINT)
                .clicked()
            {
                unseal = true;
            }
        });

        if let Some(last) = &self.screens.credentials.last_bundle {
            ui.add_space(space::M);
            widgets::paragraph(ui, last.clone(), Type::Small, t.text_secondary);
        }

        if let Some(folder) = self.screens.credentials.sync_folder.clone() {
            if seal || unseal {
                // Both ask for the master passphrase again. An unlocked window
                // is not consent to gather every private key on the machine
                // into one portable file, nor to write keys out of one.
                self.modal = Some(crate::gui::modals::Modal::KeyBundle(
                    crate::gui::modals::KeyBundleState {
                        folder,
                        sealing: seal,
                        passphrase: String::new(),
                        overwrite: false,
                        busy: false,
                        error: None,
                    },
                ));
            }
        }
    }
}

/// The account name, for a key comment. Not important enough to fail over.
fn account_name() -> String {
    std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "superbackup".to_string())
}

#[cfg(test)]
mod tests {
    use crate::gui::app::App;
    use superbackup_core::model::JobContent;

    fn app() -> (App, egui::Context) {
        let ctx = egui::Context::default();
        let app = App::new_with_daemon(
            &ctx,
            std::sync::Arc::new(crate::gui::daemon::MockDaemon::new(std::sync::Arc::new(
                superbackup_core::ipc::testing::MockHandler::new(),
            ))),
        );
        (app, ctx)
    }

    /// One press makes a job that backs up the keys and nothing else.
    ///
    /// A keys job carries no sources on purpose: the payload is built when the
    /// run starts, by sealing each key under the master passphrase, and it is
    /// the sealed bundle that reaches the destination. A job with `~/.ssh` in
    /// its sources would copy the keys as they are, which is the one thing
    /// this must never do.
    #[test]
    fn the_keys_job_carries_no_folders_of_its_own() {
        let (mut app, _ctx) = app();
        crate::gui::fixtures::seed(&mut app.data);
        app.data.jobs.clear();

        let sent = app.keys_job().expect("a job to create");
        assert_eq!(sent.content, JobContent::Keys);
        assert!(
            sent.sources.is_empty(),
            "a keys job with folders would copy the keys in the clear: {:?}",
            sent.sources
        );
        assert!(!sent.destination_ids.is_empty(), "a job with nowhere to write backs up nothing");
    }

    /// Every enabled destination, none of the disabled ones.
    ///
    /// A key is small and the reason to back one up is surviving the loss of a
    /// machine, so sending it to one place and not the others is a choice
    /// nobody makes on purpose.
    #[test]
    fn the_keys_job_goes_everywhere_that_is_switched_on() {
        let (mut app, _ctx) = app();
        crate::gui::fixtures::seed(&mut app.data);
        app.data.jobs.clear();
        assert!(app.data.destinations.len() > 1, "the fixture needs more than one destination");
        app.data.destinations[0].enabled = false;

        let sent = app.keys_job().expect("a job to create");
        let expected: Vec<_> =
            app.data.destinations.iter().filter(|d| d.enabled).map(|d| d.id).collect();
        assert_eq!(sent.destination_ids, expected);
    }

    /// With nowhere to write, say so rather than making a job that cannot run.
    #[test]
    fn with_no_destinations_it_explains_instead_of_creating_one() {
        let (mut app, _ctx) = app();
        crate::gui::fixtures::seed(&mut app.data);
        app.data.jobs.clear();
        app.data.destinations.clear();

        assert!(app.keys_job().is_none(), "a job with no destination was described");

        // And the press says why rather than doing nothing.
        app.create_keys_job();
        assert_eq!(app.toasts.len(), 1, "the user must be told why nothing happened");
    }
}
