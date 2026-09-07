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

use egui::{Sense, Ui, Vec2};

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
    }

    pub(crate) fn show_credentials(&mut self, ui: &mut Ui) {
        let t = theme::tokens(ui.ctx());
        if !self.screens.credentials.asked {
            self.scan_credentials();
        }
        if let Some(error) = self.screens.credentials.error.clone() {
            widgets::banner(ui, widgets::BannerKind::Danger, copy::cred::FAILED, Some(&error), |_| {});
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
                    let response =
                        widgets::text(ui, fingerprint, Type::MonoSmall, t.text_muted);
                    if response.on_hover_text(copy::cred::FINGERPRINT_HINT).clicked() {
                        ui.ctx().copy_text(fingerprint.clone());
                    }
                }

                ui.add_space(space::M);
                ui.horizontal(|ui| {
                    let mut backed_up = credential.backed_up;
                    if widgets::checkbox(
                        ui,
                        &mut backed_up,
                        copy::cred::BACK_UP,
                        Some(copy::cred::BACK_UP_HINT),
                        true,
                    )
                    .clicked()
                    {
                        change =
                            Some((credential.id.clone(), backed_up, credential.synced));
                    }
                    ui.add_space(space::XL);
                    let mut synced = credential.synced;
                    if widgets::checkbox(
                        ui,
                        &mut synced,
                        copy::cred::SHARE,
                        Some(copy::cred::SHARE_HINT),
                        true,
                    )
                    .clicked()
                    {
                        change = Some((credential.id.clone(), credential.backed_up, synced));
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
