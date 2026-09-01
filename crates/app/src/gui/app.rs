//! The window: chrome, routing, keyboard, and the frame loop.
//!
//! `App::frame` is the whole interface. It drains the daemon bridge, installs
//! the palette for this frame, draws the rail, the header, any pinned banners,
//! the current screen, the status strip, the toasts and at most one modal — in
//! that order — and then decides whether to ask for another frame at all.

// The interface is a library-shaped tree inside a binary crate. Its screens
// and intents are also compiled by `crates/app/tests/gui_app.rs` as a separate
// crate, so items the harness drives look unused from the binary's side.
#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use egui::{Align, Context, Layout, Rect, Sense, Stroke, Vec2};
use uuid::Uuid;

use superbackup_core::ipc::protocol::{ConflictPolicy, Reply, Request, SecretString};
use superbackup_core::model::{Job, Theme};

use super::copy;
use super::daemon::{self, Bridge, Daemon, Incoming, Intent};
use super::data::{Action, Data, Gate};
use super::format;
use super::icons::Icon;
use super::modals::{self, Modal};
use super::nav::{Nav, Route, Section, SettingsSection};
use super::screens;
use super::theme::{self, radius, size, space, Tokens, Type};
use super::toasts::{Toast, ToastKind, Toasts};
use super::validation::OnboardingStep;
use super::widgets::{self, Button};

/// What the user was trying to do when the vault stopped them.
///
/// Kept so that unlocking *performs* the action rather than discarding it —
/// the single most important detail of the locked-vault flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pending {
    RunJob(Uuid),
    PreviewJob(Uuid),
    CheckKey(Uuid),
    RunAll,
    Verify(Uuid),
    CreateRepository(Uuid),
    TestProvider(Uuid),
    RotateKeys(Uuid),
    BrowseSnapshots(Uuid),
    Restore,
}

pub struct App {
    pub data: Data,
    pub nav: Nav,
    pub toasts: Toasts,
    pub bridge: Bridge,
    pub screens: screens::Screens,
    pub modal: Option<Modal>,
    /// The onboarding flow, when `config.json` did not exist.
    pub onboarding: Option<screens::onboarding::Onboarding>,
    /// The action to perform once the vault opens.
    pub pending: Option<Pending>,
    /// Where this instance keeps its configuration, when the window was opened
    /// by the application rather than constructed for a test.
    ///
    /// The window needs this for exactly one thing: creating the vault on a
    /// first run. That cannot be asked of the daemon, because the daemon
    /// refuses to start until the vault exists — so the process that would
    /// answer the request is the process that will not start. It is the same
    /// reason `superbackup init` writes the vault itself.
    pub paths: Option<superbackup_core::paths::Paths>,
    tokens: Tokens,
    style_installed: bool,
    system_dark: bool,
    last_theme: Option<(Theme, bool)>,
    /// Set once the first frame has issued the opening requests.
    opened: bool,
    /// Ticks up whenever the status snapshot should be refetched.
    last_refresh: Option<std::time::Instant>,
}

impl App {
    /// A window talking to a real daemon.
    pub fn new(ctx: &Context, endpoint: String, timeout: Duration) -> App {
        let daemon: Arc<dyn Daemon> = Arc::new(daemon::IpcDaemon::new(endpoint, timeout));
        App::new_with_daemon(ctx, daemon)
    }

    /// The same, told where its configuration lives.
    ///
    /// A window that knows its `Paths` can complete a first run; one that does
    /// not simply never offers to, which is the right behaviour for the mock
    /// windows used by tests and the screenshot harness.
    pub fn with_paths(mut self, paths: superbackup_core::paths::Paths) -> App {
        // A missing vault *is* the first run. Starting the flow here rather
        // than waiting for a daemon reply matters because on a first run there
        // is no daemon to reply: it will not start until this flow has done
        // its work. Before this, `begin_onboarding` was reachable only from
        // the screenshot harness, so a fresh install opened a window with
        // nothing in it and no way forward.
        if !superbackup_core::config::is_initialised(&paths) {
            self.begin_onboarding();
        }
        self.paths = Some(paths);
        self
    }

    /// A window talking to whatever transport is handed in — a real daemon, or
    /// `MockHandler` for development and tests.
    pub fn new_with_daemon(ctx: &Context, daemon: Arc<dyn Daemon>) -> App {
        // Fonts must be installed before the first pass: `set_fonts` takes
        // effect when the next pass begins, and the named weight families are
        // referenced by the type scale from the very first widget.
        ctx.set_fonts(theme::fonts());
        let tokens = Tokens::for_theme(Theme::System, ctx.style().visuals.dark_mode);
        ctx.set_style(theme::style(&tokens));

        App {
            data: Data::new(),
            nav: Nav::new(),
            toasts: Toasts::default(),
            bridge: Bridge::spawn(daemon, ctx.clone()),
            screens: screens::Screens::default(),
            modal: None,
            onboarding: None,
            pending: None,
            paths: None,
            tokens,
            style_installed: true,
            system_dark: ctx.style().visuals.dark_mode,
            last_theme: None,
            opened: false,
            last_refresh: None,
        }
    }

    // -- routing ------------------------------------------------------------

    pub fn go(&mut self, route: Route) {
        self.nav.go(route);
    }

    pub fn route(&self) -> Route {
        self.nav.current().clone()
    }

    pub fn open_modal(&mut self, modal: Modal) {
        self.modal = Some(modal);
    }

    pub fn close_modal(&mut self) {
        self.modal = None;
    }

    pub fn modal_is_unlock(&self) -> bool {
        matches!(self.modal, Some(Modal::Unlock(_)))
    }

    /// For screenshots and render tests: keep whatever has been put into
    /// `data` rather than replacing it with the daemon's first replies.
    pub fn preview_mode(&mut self) {
        self.opened = true;
        self.data.loading = false;
        self.data.link_up = true;
    }

    pub fn begin_onboarding(&mut self) {
        self.onboarding = Some(screens::onboarding::Onboarding::default());
    }

    pub fn onboarding_goto(&mut self, step: OnboardingStep) {
        if self.onboarding.is_none() {
            self.begin_onboarding();
        }
        if let Some(o) = &mut self.onboarding {
            o.step = step;
        }
    }

    // -- talking to the daemon ---------------------------------------------

    pub fn ask(&self, intent: Intent, request: Request) {
        self.bridge.send(intent, request);
    }

    pub fn refresh(&mut self) {
        for (intent, request) in daemon::initial_requests() {
            self.ask(intent, request);
        }
        self.last_refresh = Some(std::time::Instant::now());
    }

    /// Drain everything the worker has posted since the last frame.
    pub fn pump(&mut self) {
        let messages = self.bridge.drain();
        for message in messages {
            match &message {
                Incoming::Failed(intent, payload) => self.report(intent.clone(), payload.clone()),
                Incoming::Reply(intent, reply) => self.on_reply(intent.clone(), reply),
                _ => {}
            }
            self.data.apply(message);
        }
        // A lagged stream means items were dropped, so the picture on screen is
        // stale: resynchronise from a fresh snapshot rather than guessing.
        if self.data.lagged {
            self.data.lagged = false;
            self.ask(Intent::Status, Request::Status {});
        }
    }

    fn on_reply(&mut self, intent: Intent, reply: &Reply) {
        match (&intent, reply) {
            (Intent::Unlock, Reply::Unlocked(r)) if r.unlocked => {
                self.toasts.success(copy::vault::UNLOCKED_TOAST);
                self.modal = None;
                self.perform_pending();
                self.ask(Intent::Status, Request::Status {});
            }
            (Intent::Lock, Reply::Unlocked(_)) => {
                self.toasts.info(copy::vault::LOCKED_TOAST);
                self.ask(Intent::Status, Request::Status {});
            }
            (Intent::RunJob(name), Reply::Started(started)) => {
                if started.started {
                    self.toasts.success(copy::toast_run_started(name));
                } else if let Some(note) = &started.note {
                    self.toasts.info(note.clone());
                }
                self.ask(Intent::Status, Request::Status {});
            }
            (Intent::SaveJob(name), Reply::Job(_)) => {
                self.toasts.success(copy::toast_saved(name));
                self.ask(Intent::Jobs, Request::JobList { include_disabled: true });
            }
            (Intent::DeleteJob(name), Reply::Ack(_)) => {
                self.toasts.success(copy::toast_deleted(name));
                self.ask(Intent::Jobs, Request::JobList { include_disabled: true });
            }
            (Intent::SaveDestination(name), Reply::Destination(_)) => {
                self.toasts.success(copy::toast_saved(name));
                self.ask(Intent::Destinations, Request::DestinationList {});
            }
            (Intent::DeleteDestination(name), Reply::Ack(_)) => {
                self.toasts.success(copy::toast_removed(name));
                self.ask(Intent::Destinations, Request::DestinationList {});
            }
            (Intent::SaveProvider(name), Reply::Provider(_)) => {
                self.toasts.success(copy::toast_saved(name));
                self.ask(Intent::Providers, Request::ProviderList {});
            }
            (Intent::DeleteProvider(name), Reply::Ack(_)) => {
                self.toasts.success(copy::toast_removed(name));
                self.ask(Intent::Providers, Request::ProviderList {});
            }
            (Intent::TestDestination(id), Reply::Probe(probe)) => {
                let name = self.data.destination_name(id);
                self.screens.destinations.probe(*id, probe.clone());
                // The destinations list renders this result itself, as a
                // banner that stays put and can be read at leisure. Toasting
                // the same sentence on top of it said everything twice —
                // twice over, once per destination, in a stack that covered
                // the list it was describing. The toast is for when the
                // result is *not* already on screen: pressed from an editor,
                // from the dashboard, or from the tray.
                let shown_in_place = matches!(self.nav.current(), Route::Destinations);
                if !shown_in_place {
                    if probe.reachable && probe.writable {
                        // Reachable and writable is a pass even when there is
                        // no repository in it yet: those are separate
                        // questions, and reporting the second as a failure of
                        // the first is what made a working bucket read as
                        // unreachable.
                        match probe.detail.as_deref().filter(|d| !d.is_empty()) {
                            Some(note) => self.toasts.info(format!("{name}: {note}")),
                            None => self.toasts.success(copy::dest_verify_ok_toast(&name)),
                        }
                    } else {
                        self.toasts.danger(
                            name,
                            probe
                                .detail
                                .clone()
                                .unwrap_or_else(|| copy::dest::STATUS_UNREACHABLE.into()),
                        );
                    }
                }
                self.ask(Intent::Destinations, Request::DestinationList {});
            }
            (Intent::TestProvider(id), Reply::Buckets(reply)) => {
                // The outcome is kept on the editor screen, but Test is also on
                // every row of the providers *list* — where nothing rendered it,
                // so pressing it there looked like the button did nothing at
                // all. A toast reaches the user wherever they pressed it.
                //
                // `credentials_ok` without `listed` is a success with a
                // caveat: the key is right and simply may not enumerate
                // buckets. It gets a warning rather than a danger toast,
                // because telling someone their working key is broken is the
                // one answer that is actually wrong.
                let name = self.data.provider_name(id);
                let detail = reply.detail.clone().unwrap_or_default();
                if reply.listed {
                    self.toasts.success(copy::toast_provider_reachable(
                        &name,
                        &copy::prov_test_ok(reply.buckets.len()),
                    ));
                } else if reply.credentials_ok {
                    self.toasts.warning(copy::toast_provider_reachable(&name, &detail));
                } else {
                    self.toasts.warning(copy::toast_provider_unreachable(&name, &detail));
                }
                self.screens.provider_editor.probe(*id, reply);
                self.ask(Intent::Providers, Request::ProviderList {});
            }
            (Intent::ListBuckets(id), Reply::Buckets(reply)) => {
                self.screens.destination_editor.buckets_arrived(*id, reply);
            }
            (Intent::ListObjects(id), Reply::Objects(reply)) => {
                self.screens.destination_editor.objects_arrived(*id, reply);
            }
            (Intent::CreateRepository(id), Reply::Repository(repo)) => {
                let name = self.data.destination_name(id);
                if repo.created || repo.connected {
                    self.toasts.success(copy::toast_repo_created(&name));
                }
                self.screens.destination_editor.repository_done(repo.clone());
                self.ask(Intent::Destinations, Request::DestinationList {});
            }
            (Intent::Snapshots(id), Reply::Snapshots(list)) => {
                self.screens.restore.snapshots_arrived(*id, list.snapshots.clone());
            }
            (Intent::Browse(id, path), Reply::Listing(listing)) => {
                self.screens.restore.listing_arrived(*id, path.clone(), listing.clone());
            }
            (Intent::Restore, Reply::Started(_)) => {
                self.screens.restore.restore_started();
            }
            (Intent::Pause, Reply::Pause(pause)) => {
                if pause.pause.paused {
                    match pause.pause.until {
                        Some(until) => self.toasts.info(copy::toast_paused(&format::clock(until))),
                        None => self.toasts.info(copy::set::PAUSE_ACTIVE_FOREVER),
                    }
                } else {
                    self.toasts.info(copy::toast::RESUMED);
                }
                self.ask(Intent::Status, Request::Status {});
            }
            (Intent::Doctor, Reply::Doctor(report)) => {
                self.screens.settings.doctor_arrived(report.clone());
            }
            (Intent::PreviewJob(job_id, name), Reply::Started(started)) => {
                self.screens.preview.accepted(*job_id, started.run_id);
                self.toasts.info(copy::preview_started(name));
                self.ask(Intent::Status, Request::Status {});
            }
            (Intent::KopiaProbe, Reply::KopiaProbe(probe)) => {
                self.screens.settings.kopia_probe_arrived(probe.clone());
            }
            (Intent::CheckKey(id), Reply::KeyCheck(check)) => {
                self.screens.destination_editor.key_check_arrived(*id, check.clone());
                let name = self.data.destination_name(id);
                if check.valid {
                    self.toasts.success(copy::keys::CHECK_OK);
                } else if check.no_repository {
                    self.toasts.info(copy::keys::CHECK_NONE);
                } else {
                    self.toasts.danger(name, copy::keys::CHECK_BAD);
                }
            }
            (Intent::ExportKeys, Reply::KeyExport(export)) => {
                // The document never lands in `Data`: the modal owns it, and
                // closing the modal is what forgets it.
                if let Some(Modal::ExportKeys(state)) = &mut self.modal {
                    state.document_arrived(export.clone());
                }
            }
            (Intent::Service, Reply::Service(_)) => {}
            (Intent::SecretRefs, Reply::SecretRefs(refs)) => {
                self.screens.provider_editor.secret_refs(refs.refs.clone());
            }
            _ => {}
        }
    }

    /// One error, at the place the user can act on it. A failure never becomes
    /// a toast *and* a banner (`UX_SPEC.md` §16.2).
    fn report(&mut self, intent: Intent, payload: superbackup_core::ipc::protocol::ErrorPayload) {
        use superbackup_core::error::ErrorCode as E;
        match payload.code {
            // The banner and the disabled controls already say this.
            E::Locked | E::DaemonUnreachable | E::Ipc | E::KopiaMissing => {}
            E::BadPassphrase => {
                if let Some(Modal::Unlock(state)) = &mut self.modal {
                    state.fail();
                } else if let Some(Modal::ExportKeys(state)) = &mut self.modal {
                    state.fail(copy::err::BAD_PASSPHRASE.to_string());
                } else {
                    self.toasts.danger(copy::err::BAD_PASSPHRASE, payload.message);
                }
            }
            E::JobNotFound => {
                self.toasts.warning(copy::err::JOB_NOT_FOUND);
                self.ask(Intent::Jobs, Request::JobList { include_disabled: true });
            }
            E::JobRunning => self.toasts.warning(copy::err::JOB_RUNNING),
            E::JobCancelled => {}
            _ => match intent {
                // Errors that belong to a screen are rendered by that screen.
                Intent::TestProvider(id) => {
                    let name = self.data.provider_name(&id);
                    self.toasts.warning(copy::toast_provider_unreachable(&name, &payload.message));
                    self.screens.provider_editor.probe_failed(id, payload)
                }
                // A picker that could not be filled is not an error the user
                // has to deal with: the manual field is right there. The
                // reason is shown beside it and nothing is blocked.
                Intent::ListBuckets(id) => {
                    self.screens.destination_editor.buckets_unavailable(id, payload.message)
                }
                Intent::ListObjects(id) => {
                    self.screens.destination_editor.objects_unavailable(id, payload.message)
                }
                Intent::TestDestination(id) => self.screens.destinations.probe_failed(id, payload),
                Intent::CreateRepository(_) => {
                    self.screens.destination_editor.repository_failed(payload)
                }
                Intent::Snapshots(_) | Intent::Browse(_, _) => self.screens.restore.failed(payload),
                Intent::PreviewJob(job_id, _) => self.screens.preview.failed(job_id, payload),
                Intent::KopiaProbe => self.screens.settings.kopia_probe_failed(payload),
                Intent::CheckKey(id) => {
                    self.screens.destination_editor.key_check_failed(id, payload)
                }
                Intent::ExportKeys => {
                    if let Some(Modal::ExportKeys(state)) = &mut self.modal {
                        state.fail(payload.message);
                    }
                }
                _ => {
                    let title = payload.message.clone();
                    let body = payload.hint.clone().unwrap_or_default();
                    self.toasts.push(Toast::new(ToastKind::Danger, title).body(body));
                }
            },
        }
    }

    // -- intents that a locked vault can interrupt --------------------------

    fn guard(&mut self, action: Action, pending: Pending) -> bool {
        match self.data.gate(action) {
            Gate::Allowed => true,
            Gate::NeedsUnlock => {
                self.pending = Some(pending);
                self.modal = Some(Modal::Unlock(modals::UnlockState::blocking()));
                false
            }
            Gate::NeedsDaemon => {
                self.toasts
                    .danger(copy::err::DAEMON_UNREACHABLE, copy::err::DAEMON_UNREACHABLE_ACTION);
                false
            }
        }
    }

    pub fn request_run(&mut self, job: &Job) {
        if !self.guard(Action::RunJob, Pending::RunJob(job.id)) {
            return;
        }
        self.ask(
            Intent::RunJob(job.name.clone()),
            Request::JobRun { job: job.id.to_string(), dry_run: false },
        );
    }

    /// Rehearse a job: report what it would copy, write nothing.
    ///
    /// Goes to the same `job.run` the real thing does, with `dry_run` set —
    /// the engine is the authority on what a rehearsal means, and a second
    /// code path would eventually disagree with it.
    pub fn request_preview(&mut self, job: &Job) {
        if !self.guard(Action::RunJob, Pending::PreviewJob(job.id)) {
            return;
        }
        self.screens.preview.started(job.id, job.name.clone());
        self.go(Route::Preview(job.id));
        self.ask(
            Intent::PreviewJob(job.id, job.name.clone()),
            Request::JobRun { job: job.id.to_string(), dry_run: true },
        );
    }

    /// Ask the daemon where kopia is and to run it for real.
    pub fn request_kopia_probe(&mut self, destination: Option<Uuid>, check_for_update: bool) {
        self.screens.settings.kopia_probe_started();
        self.ask(
            Intent::KopiaProbe,
            Request::KopiaProbe {
                destination: destination.map(|d| d.to_string()),
                check_for_update,
            },
        );
    }

    /// Check that a repository encryption key opens its repository.
    ///
    /// `key` is `None` to check the one already in the vault, which is the
    /// question "is my backup still openable?" rather than "did I type this
    /// correctly?".
    pub fn request_check_key(&mut self, destination: Uuid, key: Option<String>) {
        if !self.guard(Action::VerifyDestination, Pending::CheckKey(destination)) {
            return;
        }
        self.screens.destination_editor.key_check_started(destination);
        self.ask(
            Intent::CheckKey(destination),
            Request::DestinationCheckKey {
                destination: destination.to_string(),
                key: key.map(SecretString::from_string),
            },
        );
    }

    /// Ask for the encryption key export document.
    ///
    /// The reply carries every repository key in the clear. It is handed
    /// straight to the export modal and is never stored in [`Data`], never
    /// logged, and never held after the modal closes.
    pub fn request_key_export(&mut self, passphrase: String) {
        self.ask(
            Intent::ExportKeys,
            Request::VaultExportKeys { passphrase: SecretString::from_string(passphrase) },
        );
    }

    pub fn request_run_all(&mut self) {
        if !self.guard(Action::RunJob, Pending::RunAll) {
            return;
        }
        let jobs: Vec<Job> = self.data.jobs.iter().filter(|j| j.enabled).cloned().collect();
        for job in jobs {
            self.ask(
                Intent::RunJob(job.name.clone()),
                Request::JobRun { job: job.id.to_string(), dry_run: false },
            );
        }
    }

    pub fn request_verify(&mut self, destination: Uuid) {
        if !self.guard(Action::VerifyDestination, Pending::Verify(destination)) {
            return;
        }
        self.screens.destinations.probe_started(destination);
        self.ask(
            Intent::TestDestination(destination),
            Request::DestinationTest { destination: destination.to_string() },
        );
    }

    pub fn request_test_provider(&mut self, provider: Uuid) {
        if !self.guard(Action::TestProvider, Pending::TestProvider(provider)) {
            return;
        }
        self.screens.provider_editor.probe_started(provider);
        // `provider.list_buckets` rather than `provider.test`: it is the same
        // authenticated call, and it comes back with the names, so the editor
        // can show what the credentials can actually see instead of only that
        // they worked. `provider.test` stays in the protocol for the CLI and
        // for agents that want a plain reachable/not answer.
        self.ask(
            Intent::TestProvider(provider),
            Request::ProviderListBuckets { provider: provider.to_string() },
        );
    }

    /// Fetch the bucket list for the destination editor's picker.
    ///
    /// Never gates the editor: if the vault is locked or the request fails,
    /// the picker degrades to the manual field, which is always present.
    pub fn request_bucket_list(&mut self, provider: Uuid) {
        if !self.data.gate(Action::TestProvider).allowed() {
            self.screens
                .destination_editor
                .buckets_unavailable(provider, copy::dest::S3_BUCKET_LOCKED.to_string());
            return;
        }
        self.screens.destination_editor.buckets_requested(provider);
        self.ask(
            Intent::ListBuckets(provider),
            Request::ProviderListBuckets { provider: provider.to_string() },
        );
    }

    /// Ask what is already stored under a destination's prefix.
    pub fn request_prefix_check(&mut self, provider: Uuid, bucket: String, prefix: String) {
        if !self.data.gate(Action::TestProvider).allowed() || bucket.trim().is_empty() {
            return;
        }
        self.screens.destination_editor.objects_requested(provider);
        self.ask(
            Intent::ListObjects(provider),
            Request::ProviderListObjects {
                provider: provider.to_string(),
                bucket,
                prefix,
                max_keys: 16,
            },
        );
    }

    pub fn request_create_repository(&mut self, destination: Uuid) {
        if !self.guard(Action::CreateRepository, Pending::CreateRepository(destination)) {
            return;
        }
        self.screens.destination_editor.repository_started();
        let encryption = self.data.destination(&destination).and_then(|d| d.encryption.clone());
        self.ask(
            Intent::CreateRepository(destination),
            Request::DestinationRepoCreate { destination: destination.to_string(), encryption },
        );
    }

    pub fn request_snapshots(&mut self, destination: Uuid) {
        if !self.guard(Action::BrowseSnapshots, Pending::BrowseSnapshots(destination)) {
            return;
        }
        self.screens.restore.snapshots_requested(destination);
        self.ask(
            Intent::Snapshots(destination),
            Request::SnapshotList { destination: destination.to_string(), job: None, limit: 0 },
        );
    }

    pub fn request_browse(&mut self, destination: Uuid, snapshot: String, path: String) {
        if !self.guard(Action::BrowseSnapshots, Pending::BrowseSnapshots(destination)) {
            return;
        }
        self.screens.restore.listing_requested(path.clone());
        self.ask(
            Intent::Browse(destination, path.clone()),
            Request::SnapshotBrowse { destination: destination.to_string(), snapshot, path },
        );
    }

    pub fn request_restore(
        &mut self,
        destination: Uuid,
        snapshot: String,
        path: String,
        target: std::path::PathBuf,
        conflict: ConflictPolicy,
    ) {
        if !self.guard(Action::Restore, Pending::Restore) {
            return;
        }
        self.ask(
            Intent::Restore,
            Request::SnapshotRestore {
                destination: destination.to_string(),
                snapshot,
                path,
                target,
                conflict,
                dry_run: false,
            },
        );
    }

    pub fn request_stop(&mut self, run_id: Uuid) {
        self.ask(Intent::StopRun, Request::JobStop { run_id });
    }

    pub fn request_stop_all(&mut self) {
        self.ask(Intent::StopRun, Request::JobStopAll {});
    }

    pub fn unlock(&mut self, passphrase: String) {
        self.ask(
            Intent::Unlock,
            Request::VaultUnlock { passphrase: SecretString::from_string(passphrase) },
        );
    }

    pub fn lock(&mut self) {
        self.ask(Intent::Lock, Request::VaultLock {});
    }

    pub fn pause(&mut self, seconds: Option<u64>, reason: Option<String>) {
        self.ask(Intent::Pause, Request::ControlPause { seconds, reason });
    }

    pub fn resume(&mut self) {
        self.ask(Intent::Pause, Request::ControlResume {});
    }

    pub fn save_settings(&mut self) {
        self.ask(
            Intent::Settings,
            Request::SettingsUpdate { settings: Box::new(self.data.settings.clone()) },
        );
    }

    /// Perform whatever the user was blocked from doing.
    fn perform_pending(&mut self) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        match pending {
            Pending::RunJob(id) => {
                if let Some(job) = self.data.job(&id).cloned() {
                    self.ask(
                        Intent::RunJob(job.name.clone()),
                        Request::JobRun { job: job.id.to_string(), dry_run: false },
                    );
                }
            }
            Pending::PreviewJob(id) => {
                if let Some(job) = self.data.job(&id).cloned() {
                    self.request_preview(&job);
                }
            }
            // Deliberately does not re-run the check with a key the user typed
            // before the vault locked: that string is gone by now, and
            // silently checking the *stored* key instead would answer a
            // different question than the one asked.
            Pending::CheckKey(id) => self.go(Route::DestinationEditor(id)),
            Pending::RunAll => self.request_run_all(),
            Pending::Verify(id) => self.request_verify(id),
            Pending::CreateRepository(id) => self.request_create_repository(id),
            Pending::TestProvider(id) => self.request_test_provider(id),
            Pending::RotateKeys(id) => {
                self.modal = Some(Modal::Rotate(modals::RotateState::new(id)));
            }
            Pending::BrowseSnapshots(id) => self.request_snapshots(id),
            Pending::Restore => self.go(Route::Restore),
        }
    }

    /// Test hook: pretend the daemon accepted a passphrase.
    pub fn complete_unlock_for_test(&mut self) {
        if let Some(s) = &mut self.data.snapshot {
            s.unlocked = true;
        }
        self.modal = None;
        self.perform_pending();
    }

    // -- the frame ----------------------------------------------------------

    pub fn frame(&mut self, ctx: &Context) {
        self.pump();

        if !self.opened {
            self.opened = true;
            self.refresh();
        }

        let wanted = (self.data.settings.theme, self.system_dark);
        if self.last_theme != Some(wanted) {
            self.tokens = Tokens::for_theme(wanted.0, wanted.1);
            // `set_style` takes effect immediately; `set_fonts` would not, so
            // it is never called from inside a pass.
            ctx.set_style(theme::style(&self.tokens));
            self.last_theme = Some(wanted);
        }
        theme::install(ctx, self.tokens);

        if self.onboarding.is_some() {
            self.show_onboarding(ctx);
            self.schedule_repaint(ctx);
            return;
        }

        self.handle_shortcuts(ctx);

        let t = self.tokens;
        egui::TopBottomPanel::bottom("sb-status")
            .exact_height(size::STATUS_STRIP)
            .frame(
                egui::Frame::new()
                    .fill(t.bg_rail)
                    .stroke(Stroke::NONE)
                    .inner_margin(egui::Margin::symmetric(12, 0)),
            )
            .show(ctx, |ui| {
                ui.painter().rect_filled(
                    Rect::from_min_size(
                        ui.max_rect().left_top() - Vec2::new(12.0, 0.0),
                        Vec2::new(ui.max_rect().width() + 24.0, 1.0),
                    ),
                    0,
                    t.border_subtle,
                );
                self.status_strip(ui);
            });

        let narrow = ctx.screen_rect().width() < size::BREAKPOINT;
        let rail_width = if narrow { size::RAIL_COLLAPSED } else { size::RAIL };
        egui::SidePanel::left("sb-rail")
            .exact_width(rail_width)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(t.bg_rail)
                    .inner_margin(egui::Margin::symmetric(if narrow { 8 } else { 12 }, 16)),
            )
            .show(ctx, |ui| self.rail(ui, narrow));

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(t.bg_canvas).inner_margin(egui::Margin::ZERO))
            .show(ctx, |ui| {
                let full = ui.max_rect();
                self.header(ui, narrow);
                let pad = if narrow { size::CONTENT_PAD_NARROW } else { size::CONTENT_PAD };
                let content = Rect::from_min_max(
                    egui::Pos2::new(full.left() + pad, full.top() + size::HEADER + pad),
                    egui::Pos2::new(full.right() - pad, full.bottom() - pad),
                );
                let mut content_ui = ui.new_child(
                    egui::UiBuilder::new().max_rect(content).layout(Layout::top_down(Align::Min)),
                );
                // Clip a little wider than the layout rect. Clipping at exactly
                // the content edge shaves the outer pixel off a border or a
                // focus ring on the leftmost and rightmost widgets, which is
                // what made them look cut off rather than merely tight.
                content_ui.set_clip_rect(Rect::from_min_max(
                    egui::Pos2::new(content.left() - 4.0, content.top()),
                    egui::Pos2::new(content.right() + 4.0, content.bottom() + 4.0),
                ));
                self.banners(&mut content_ui);
                self.screen(&mut content_ui);
                self.toasts.show(ui, full);
            });

        self.show_modal(ctx);
        self.schedule_repaint(ctx);
    }

    // -- chrome -------------------------------------------------------------

    fn rail(&mut self, ui: &mut egui::Ui, narrow: bool) {
        let t = self.tokens;
        ui.spacing_mut().item_spacing.y = 0.0;

        // Machine identity.
        if narrow {
            let initials: String = self
                .data
                .machine_label()
                .split(|c: char| !c.is_alphanumeric())
                .filter(|s| !s.is_empty())
                .take(2)
                .filter_map(|s| s.chars().next())
                .collect();
            let (rect, response) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::click());
            ui.painter().rect_filled(rect, radius::CONTROL, t.bg_raised);
            let g =
                widgets::galley(ui, initials.to_uppercase(), Type::SmallStrong, t.text_secondary);
            let size = g.size();
            ui.painter().galley(rect.center() - size / 2.0, g, t.text_secondary);
            let label = format!("{} ({})", self.data.machine_label(), self.data.machine_slug());
            if response.on_hover_text(label).clicked() {
                self.go(Route::Settings(SettingsSection::General));
            }
        } else {
            let (rect, response) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 36.0), Sense::click());
            let mut child = ui.new_child(
                egui::UiBuilder::new().max_rect(rect).layout(Layout::top_down(Align::Min)),
            );
            child.spacing_mut().item_spacing.y = 0.0;
            // The hostname, not the editable label: this answers "which
            // computer am I looking at", and a renamed label would otherwise
            // hide it. The label is still shown when the two differ, since
            // that is exactly when the distinction matters.
            let hostname = self.data.machine_hostname();
            let label = self.data.machine_label();
            let primary =
                if hostname.is_empty() { label.to_string() } else { hostname.to_string() };
            widgets::elided(
                &mut child,
                &primary,
                Type::BodyStrong,
                t.text_primary,
                rect.width(),
                false,
            );
            // The slug repeats the machine name before its identifier, which is
            // redundant under a line already showing the name. Show the
            // identifier itself, and the label alongside when it has been
            // changed.
            let id = self.data.machine_slug();
            let compact = id.rsplit('-').next().unwrap_or(id);
            let secondary = if !label.is_empty() && !label.eq_ignore_ascii_case(&primary) {
                format!("{label} · {compact}")
            } else {
                compact.to_string()
            };
            widgets::elided(&mut child, &secondary, Type::Small, t.text_muted, rect.width(), false);
            let response = response.on_hover_text(format!("{primary} · {id}"));
            if response.clicked() {
                self.go(Route::Settings(SettingsSection::General));
            }
        }
        ui.add_space(space::L);

        let current = self.nav.current().section();
        let mut go: Option<Route> = None;
        for (index, section) in Section::ALL.iter().enumerate() {
            if section.gap_before() {
                // The remainder spacer, so Settings and About sit at the foot
                // of the navigation block rather than floating.
                ui.add_space(space::L);
            }
            let selected = *section == current;
            let attention = self.section_needs_attention(*section);
            if self.rail_item(ui, *section, selected, attention, narrow, index) {
                go = Some(section.route());
            }
        }

        // Push the vault control to the bottom, with the *same* gap above and
        // below it. The reserve used to be a bare 44px, which was less than a
        // divider plus two gaps plus the control once the control learned to
        // size itself to its own text — so the badge sat hard against the
        // status bar with a visible gap only above it.
        let gap = space::L;
        let reserve = widgets::DIVIDER_H + gap + size::RAIL_ITEM_H + gap;
        let remaining = ui.available_height() - reserve;
        if remaining > 0.0 {
            ui.add_space(remaining);
        }
        widgets::divider(ui);
        ui.add_space(gap);
        self.vault_control(ui, narrow);
        ui.add_space(gap);

        if let Some(route) = go {
            self.go(route);
        }
    }

    fn section_needs_attention(&self, section: Section) -> bool {
        match section {
            Section::Destinations => {
                self.data.destinations.iter().any(|d| self.screens.destinations.failed_probe(d.id))
            }
            Section::Activity => self
                .data
                .events
                .iter()
                .any(|e| e.severity == superbackup_core::state::Severity::Error),
            _ => false,
        }
    }

    fn rail_item(
        &self,
        ui: &mut egui::Ui,
        section: Section,
        selected: bool,
        attention: bool,
        narrow: bool,
        index: usize,
    ) -> bool {
        let t = self.tokens;
        let (rect, response) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), size::RAIL_ITEM_H),
            Sense::click(),
        );
        if ui.is_rect_visible(rect) {
            if selected {
                ui.painter().rect_filled(rect, radius::CONTROL, t.rail_selected_bg);
                ui.painter().rect_filled(
                    Rect::from_min_size(rect.left_top(), Vec2::new(3.0, rect.height())),
                    egui::CornerRadius::same(2),
                    t.rail_selected_marker,
                );
            } else if response.hovered() {
                ui.painter().rect_filled(rect, radius::CONTROL, t.bg_raised);
            }
            if response.has_focus() {
                widgets::focus_ring(ui, rect.shrink(2.0), radius::CONTROL);
            }
            let colour = if selected { t.text_primary } else { t.text_secondary };
            let icon_x = if narrow { rect.center().x - 8.0 } else { rect.left() + 12.0 };
            section.icon().paint(
                ui.painter(),
                Rect::from_min_size(
                    egui::Pos2::new(icon_x, rect.center().y - 8.0),
                    Vec2::splat(16.0),
                ),
                colour,
            );
            if !narrow {
                let style = if selected { Type::BodyStrong } else { Type::Body };
                let g = widgets::galley(ui, section.title(), style, colour);
                let h = g.size().y;
                ui.painter().galley(
                    egui::Pos2::new(rect.left() + 12.0 + 16.0 + 12.0, rect.center().y - h / 2.0),
                    g,
                    colour,
                );
            }
            if attention {
                ui.painter().circle_filled(
                    egui::Pos2::new(rect.right() - 12.0, rect.center().y),
                    3.0,
                    t.danger.mark,
                );
            }
        }
        let label = if attention {
            copy::a11y_rail_attention(section.title())
        } else if selected {
            copy::a11y_rail_selected(section.title(), index + 1, Section::ALL.len())
        } else {
            copy::a11y_rail_item(section.title(), index + 1, Section::ALL.len())
        };
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, &label)
        });
        let response = if narrow { response.on_hover_text(section.title()) } else { response };
        response.clicked()
    }

    /// The rail's lock badge, sized to whatever it has to say.
    ///
    /// It used to be a fixed 32px tall with two lines of text painted inside
    /// it, so "Locked / Schedules are blocked" filled the box edge to edge and
    /// read as clipped. The height and the text measure are now derived from
    /// the strings themselves, which is what keeps it honest for the longer
    /// unlocked variant and for a translation.
    fn vault_control(&mut self, ui: &mut egui::Ui, narrow: bool) {
        let t = self.tokens;
        let unlocked = self.data.unlocked();

        // Laid out before the rect is allocated, because the rect's height
        // depends on how tall the wrapped text turns out to be.
        const PAD_X: f32 = 10.0;
        const PAD_Y: f32 = 7.0;
        const ICON: f32 = 16.0;
        const GAP: f32 = 8.0;
        let text_x_offset = PAD_X + ICON + GAP;
        let full_width = ui.available_width();
        let text_width = (full_width - text_x_offset - PAD_X).max(40.0);

        let label = if unlocked { copy::vault::UNLOCKED } else { copy::vault::LOCKED };
        let sub = if unlocked {
            let minutes = self.data.settings.auto_lock_minutes;
            (minutes != 0).then(|| copy::vault_locks_in(&format::minutes(minutes)))
        } else {
            Some(copy::vault::LOCKED_SUB.to_string())
        };
        let text_colour = if unlocked { t.text_primary } else { t.danger.tint_text };
        let title = widgets::galley_wrapped(ui, label, Type::BodyStrong, text_colour, text_width);
        let subtitle = sub
            .as_ref()
            .map(|s| widgets::galley_wrapped(ui, s.clone(), Type::Small, t.text_muted, text_width));

        let text_height = title.size().y + subtitle.as_ref().map(|g| g.size().y).unwrap_or(0.0);
        // Never shorter than a rail item, so the two states and the navigation
        // above them keep a common rhythm.
        let height = (text_height + PAD_Y * 2.0).max(size::RAIL_ITEM_H);

        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(full_width, height), Sense::click());
        if ui.is_rect_visible(rect) {
            if !unlocked {
                ui.painter().rect_filled(rect, radius::CONTROL, t.danger.tint_bg);
            } else if response.hovered() {
                ui.painter().rect_filled(rect, radius::CONTROL, t.bg_raised);
            }
            if response.has_focus() {
                widgets::focus_ring(ui, rect.shrink(2.0), radius::CONTROL);
            }
            let (icon, colour) = if unlocked {
                (Icon::LockOpen, t.success.mark)
            } else {
                (Icon::Lock, t.danger.mark)
            };
            let icon_x = if narrow { rect.center().x - ICON / 2.0 } else { rect.left() + PAD_X };
            icon.paint(
                ui.painter(),
                Rect::from_min_size(
                    egui::Pos2::new(icon_x, rect.center().y - ICON / 2.0),
                    Vec2::splat(ICON),
                ),
                colour,
            );
            if !narrow {
                let x = rect.left() + text_x_offset;
                let top = rect.center().y - text_height / 2.0;
                let title_height = title.size().y;
                ui.painter().galley(egui::Pos2::new(x, top), title, text_colour);
                if let Some(subtitle) = subtitle {
                    ui.painter().galley(
                        egui::Pos2::new(x, top + title_height),
                        subtitle,
                        t.text_muted,
                    );
                }
            }
        }
        let announce = if unlocked {
            copy::a11y_vault_unlocked(&format::minutes(self.data.settings.auto_lock_minutes))
        } else {
            copy::A11Y_VAULT_LOCKED.to_string()
        };
        response
            .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &announce));
        if response.clicked() {
            if unlocked {
                self.lock();
            } else {
                self.modal = Some(Modal::Unlock(modals::UnlockState::voluntary()));
            }
        }
    }

    fn header(&mut self, ui: &mut egui::Ui, narrow: bool) {
        let t = self.tokens;
        let full = ui.max_rect();
        let rect = Rect::from_min_size(full.left_top(), Vec2::new(full.width(), size::HEADER));
        ui.painter().rect_filled(
            Rect::from_min_size(
                egui::Pos2::new(rect.left(), rect.bottom() - 1.0),
                Vec2::new(rect.width(), 1.0),
            ),
            0,
            t.border_subtle,
        );
        let pad = if narrow { size::CONTENT_PAD_NARROW } else { size::CONTENT_PAD };
        let mut ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect.shrink2(Vec2::new(pad, 12.0)))
                .layout(Layout::left_to_right(Align::Center)),
        );
        ui.spacing_mut().item_spacing.x = space::M;

        let route = self.nav.current().clone();
        if route.is_sub_screen() {
            if widgets::icon_button(&mut ui, Icon::ArrowLeft, copy::action::BACK, true).clicked() {
                self.nav.back();
            }
            let parent = route.parent();
            if widgets::link(&mut ui, parent.section().title()).clicked() {
                self.go(parent);
            }
            widgets::text(&mut ui, "/", Type::Body, t.text_muted);
            let title = self.screen_title();
            widgets::elided(&mut ui, &title, Type::H1, t.text_primary, 360.0, false);
        } else {
            widgets::text(&mut ui, self.screen_title(), Type::H1, t.text_primary);
        }

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            self.header_actions(ui);
        });
    }

    fn screen_title(&self) -> String {
        match self.nav.current() {
            Route::JobEditor(id) => self.data.job_name(id),
            Route::DestinationEditor(id) => self.data.destination_name(id),
            Route::NewDestination => copy::dest::NEW.to_string(),
            Route::ProviderEditor(id) => self
                .data
                .provider(id)
                .map(|p| p.name.clone())
                .unwrap_or_else(|| copy::state::UNKNOWN.to_string()),
            Route::NewProvider => copy::prov::NEW.to_string(),
            Route::RunDetail(id) => self
                .data
                .history
                .iter()
                .find(|r| &r.run_id == id)
                .map(|r| copy::run_detail_title(&r.job_name, &format::absolute(r.started_at)))
                .unwrap_or_else(|| copy::state::UNKNOWN.to_string()),
            Route::Preview(id) => copy::preview_title(&self.data.job_name(id)),
            other => other.section().title().to_string(),
        }
    }

    /// Global banners, in the documented precedence, at most two.
    fn banners(&mut self, ui: &mut egui::Ui) {
        let mut shown = 0;
        let mut extra = 0;

        if !self.data.unlocked() && !self.data.loading {
            if shown < 2 {
                shown += 1;
                let mut unlock = false;
                widgets::banner(
                    ui,
                    widgets::BannerKind::Danger,
                    copy::vault::BANNER_TITLE,
                    Some(copy::vault::BANNER_BODY),
                    |ui| {
                        unlock = Button::primary(copy::vault::BANNER_ACTION)
                            .compact()
                            .show(ui)
                            .clicked();
                    },
                );
                ui.add_space(space::XL);
                if unlock {
                    self.modal = Some(Modal::Unlock(modals::UnlockState::voluntary()));
                }
            } else {
                extra += 1;
            }
        }

        if self.data.paused() {
            if shown < 2 {
                shown += 1;
                let body = match self.data.paused_until() {
                    Some(until) => copy::set_pause_active(&format::clock(until)),
                    None => copy::set::PAUSE_ACTIVE_FOREVER.to_string(),
                };
                let mut resume = false;
                widgets::banner(
                    ui,
                    widgets::BannerKind::Warning,
                    &body,
                    Some(copy::set::PAUSE_BODY),
                    |ui| {
                        resume =
                            Button::secondary(copy::set::PAUSE_RESUME).compact().show(ui).clicked();
                    },
                );
                ui.add_space(space::XL);
                if resume {
                    self.resume();
                }
            } else {
                extra += 1;
            }
        }

        if self.data.kopia_missing() {
            if shown < 2 {
                shown += 1;
                let mut fix = false;
                widgets::banner(
                    ui,
                    widgets::BannerKind::Danger,
                    copy::err::KOPIA_MISSING,
                    None,
                    |ui| {
                        fix = Button::secondary(copy::err::KOPIA_MISSING_ACTION)
                            .compact()
                            .show(ui)
                            .clicked();
                    },
                );
                ui.add_space(space::XL);
                if fix {
                    self.go(Route::Settings(SettingsSection::Kopia));
                }
            } else {
                extra += 1;
            }
        }

        if !self.data.link_up && !self.data.loading {
            if shown < 2 {
                let mut retry = false;
                widgets::banner(
                    ui,
                    widgets::BannerKind::Danger,
                    copy::err::DAEMON_UNREACHABLE,
                    None,
                    |ui| {
                        retry = Button::secondary(copy::action::RETRY).compact().show(ui).clicked();
                    },
                );
                ui.add_space(space::XL);
                if retry {
                    self.refresh();
                }
            } else {
                extra += 1;
            }
        }

        if extra > 0 {
            let label = format!("+{extra} more issues");
            if widgets::link(ui, &label).clicked() {
                self.go(Route::Settings(SettingsSection::Advanced));
            }
            ui.add_space(space::L);
        }
    }

    fn status_strip(&mut self, ui: &mut egui::Ui) {
        let t = self.tokens;
        let mut go: Option<Route> = None;
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = space::L;

            let running = self.data.link_up;
            let colour = if running { t.success.mark } else { t.danger.mark };
            let label = if running { "Daemon running" } else { "Daemon not running" };
            widgets::status_dot(ui, colour, label, 6.0);
            let uptime = self
                .data
                .snapshot
                .as_ref()
                .map(|s| format!("Running for {}", format::duration(s.uptime_seconds as i64)))
                .unwrap_or_else(|| copy::state::UNKNOWN.to_string());
            if widgets::text(ui, label, Type::Small, t.text_muted)
                .on_hover_text(uptime)
                .interact(Sense::click())
                .clicked()
            {
                go = Some(Route::Settings(SettingsSection::Advanced));
            }

            widgets::vertical_rule(ui, 14.0);
            let service = match &self.data.service {
                Some(s) if s.installed && s.running => copy::set::SERVICE_INSTALLED_RUNNING,
                Some(s) if s.installed => copy::set::SERVICE_INSTALLED_STOPPED,
                _ => copy::set::SERVICE_NOT_INSTALLED,
            };
            widgets::text(ui, service, Type::Small, t.text_muted);

            // superbackup's own version, next to Kopia's. Showing only Kopia's
            // is what made "0.23.1" look like this application's version.
            widgets::vertical_rule(ui, 14.0);
            let build = crate::build::short();
            let colour = if crate::build::is_dirty() { t.warning.mark } else { t.text_muted };
            if widgets::text(ui, format!("superbackup {build}"), Type::Small, colour)
                .on_hover_text(crate::build::long())
                .clicked()
            {
                go = Some(Route::About);
            }

            widgets::vertical_rule(ui, 14.0);
            match self.data.snapshot.as_ref().and_then(|s| s.kopia_version.clone()) {
                Some(version) => {
                    widgets::text(ui, format!("Kopia {version}"), Type::Small, t.text_muted);
                }
                None => {
                    if widgets::text(ui, "Kopia not found", Type::Small, t.warning.mark)
                        .interact(Sense::click())
                        .clicked()
                    {
                        go = Some(Route::Settings(SettingsSection::Kopia));
                    }
                }
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let Some(event) = self.data.events.first().cloned() {
                    let status = t.severity(event.severity);
                    let line = format!("{} {}", format::clock(event.at), event.message);
                    let width = (ui.available_width() - 20.0).max(80.0);
                    if widgets::elided(ui, &line, Type::Small, t.text_muted, width, false)
                        .interact(Sense::click())
                        .clicked()
                    {
                        go = Some(Route::Activity);
                    }
                    widgets::status_dot(ui, status.mark, event.severity_label(), 6.0);
                }
            });
        });
        if let Some(route) = go {
            self.go(route);
        }
    }

    fn show_modal(&mut self, ctx: &Context) {
        let Some(modal) = self.modal.take() else {
            return;
        };
        match modals::show(self, ctx, modal) {
            Some(still_open) => self.modal = Some(still_open),
            None => self.modal = None,
        }
    }

    fn show_onboarding(&mut self, ctx: &Context) {
        let t = self.tokens;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(t.bg_canvas).inner_margin(egui::Margin::ZERO))
            .show(ctx, |ui| {
                screens::onboarding::show(self, ui);
            });
    }

    fn screen(&mut self, ui: &mut egui::Ui) {
        match self.nav.current().clone() {
            Route::Dashboard => self.show_dashboard(ui),
            Route::Jobs => self.show_jobs(ui),
            Route::JobEditor(id) => self.show_job_editor(ui, id),
            Route::Destinations => self.show_destinations(ui),
            Route::DestinationEditor(id) => self.show_destination_editor(ui, Some(id)),
            Route::NewDestination => self.show_destination_editor(ui, None),
            Route::Providers => self.show_providers(ui),
            Route::ProviderEditor(id) => self.show_provider_editor(ui, Some(id)),
            Route::NewProvider => self.show_provider_editor(ui, None),
            Route::Restore => self.show_restore(ui),
            Route::Activity => self.show_activity(ui),
            Route::RunDetail(id) => self.show_run_detail(ui, id),
            Route::Preview(id) => self.show_preview(ui, id),
            Route::Settings(section) => self.show_settings(ui, section),
            Route::About => self.show_about(ui),
        }
    }

    fn header_actions(&mut self, ui: &mut egui::Ui) {
        match self.nav.current().clone() {
            Route::Dashboard => self.dashboard_actions(ui),
            Route::Jobs => self.jobs_actions(ui),
            Route::JobEditor(id) => self.job_editor_actions(ui, id),
            Route::Destinations => self.destinations_actions(ui),
            Route::Providers => self.providers_actions(ui),
            Route::Activity => self.activity_actions(ui),
            Route::Restore => self.restore_actions(ui),
            _ => {}
        }
    }

    // -- keyboard (DESIGN_SYSTEM §9.1) --------------------------------------

    fn handle_shortcuts(&mut self, ctx: &Context) {
        if self.modal.is_some() {
            return;
        }
        let mut go: Option<Route> = None;
        let mut refresh = false;
        let mut lock = false;
        let mut new_job = false;
        let mut run = false;
        ctx.input(|i| {
            let cmd = i.modifiers.command;
            if cmd {
                for section in Section::ALL {
                    if let Some(key) = section.shortcut() {
                        if i.key_pressed(key) {
                            go = Some(section.route());
                        }
                    }
                }
                if i.key_pressed(egui::Key::N) {
                    new_job = true;
                }
                if i.key_pressed(egui::Key::R) {
                    run = true;
                }
                if i.key_pressed(egui::Key::L) {
                    lock = true;
                }
                if i.key_pressed(egui::Key::Comma) {
                    go = Some(Route::Settings(SettingsSection::General));
                }
            }
            if i.key_pressed(egui::Key::F5) {
                refresh = true;
            }
        });
        if let Some(route) = go {
            self.go(route);
        }
        if refresh {
            self.refresh();
        }
        if lock && self.data.unlocked() {
            self.lock();
        }
        if new_job {
            self.modal = Some(Modal::Wizard(Box::new(modals::WizardState::new(&self.data))));
        }
        if run {
            // `Ctrl+R` runs the job the user is looking at, or every job from
            // the dashboard, which is what the visible controls do too.
            match self.nav.current().clone() {
                Route::JobEditor(id) => {
                    if let Some(job) = self.data.job(&id).cloned() {
                        self.request_run(&job);
                    }
                }
                _ => self.request_run_all(),
            }
        }
    }

    // -- repaint policy (L14) ----------------------------------------------

    /// egui repaints on demand. A window with nothing happening must reach
    /// zero frames per second, or a laptop loses an hour of battery to an idle
    /// backup tool.
    fn schedule_repaint(&self, ctx: &Context) {
        if let Some(delay) = self.repaint_delay() {
            ctx.request_repaint_after(delay);
        }
    }

    pub fn repaint_delay(&self) -> Option<Duration> {
        let running = !self.data.active_runs().is_empty();
        if running {
            // 30fps while something is genuinely moving.
            return Some(Duration::from_millis(33));
        }
        if self.toasts.animating() {
            return Some(Duration::from_millis(100));
        }
        if self.screens.busy() || self.modal.as_ref().map(Modal::busy).unwrap_or(false) {
            return Some(Duration::from_millis(100));
        }
        // A locked, idle window still wants its auto-lock countdown to tick,
        // but once a second is plenty for a minutes-resolution label.
        if self.data.unlocked() && self.data.settings.auto_lock_minutes > 0 {
            return Some(Duration::from_secs(30));
        }
        None
    }
}

/// A tiny extension so the status strip can name a severity without matching
/// on it in three places.
trait SeverityLabel {
    fn severity_label(&self) -> &'static str;
}

impl SeverityLabel for superbackup_core::state::Event {
    fn severity_label(&self) -> &'static str {
        use superbackup_core::state::Severity as S;
        match self.severity {
            S::Debug => "Debug",
            S::Info => "Information",
            S::Warning => "Warning",
            S::Error => "Error",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use superbackup_core::ipc::testing::MockHandler;

    fn app() -> (App, Context) {
        let ctx = Context::default();
        let app = App::new_with_daemon(
            &ctx,
            Arc::new(daemon::MockDaemon::new(Arc::new(MockHandler::new()))),
        );
        (app, ctx)
    }

    #[test]
    fn an_idle_window_schedules_no_repaint() {
        let (mut app, _ctx) = app();
        app.data.loading = false;
        app.data.link_up = true;
        app.data.settings.auto_lock_minutes = 0;
        assert_eq!(app.repaint_delay(), None);
    }

    #[test]
    fn a_running_job_schedules_a_repaint() {
        let (mut app, _ctx) = app();
        super::super::fixtures::seed(&mut app.data);
        assert!(app.repaint_delay().unwrap_or(Duration::MAX) <= Duration::from_millis(33));
    }

    #[test]
    fn a_blocked_run_opens_the_unlock_modal_and_remembers_the_intent() {
        let (mut app, _ctx) = app();
        super::super::fixtures::seed(&mut app.data);
        if let Some(s) = &mut app.data.snapshot {
            s.unlocked = false;
        }
        let job = app.data.jobs[0].clone();
        app.request_run(&job);
        assert!(app.modal_is_unlock());
        assert_eq!(app.pending, Some(Pending::RunJob(job.id)));
    }

    #[test]
    fn unlocking_performs_the_remembered_action() {
        let (mut app, _ctx) = app();
        super::super::fixtures::seed(&mut app.data);
        if let Some(s) = &mut app.data.snapshot {
            s.unlocked = false;
        }
        let job = app.data.jobs[0].clone();
        app.request_run(&job);
        app.complete_unlock_for_test();
        assert!(app.pending.is_none(), "the intent must be consumed, not dropped");
        assert!(app.modal.is_none());
    }

    #[test]
    fn a_missing_daemon_reports_once_rather_than_opening_the_unlock_modal() {
        let (mut app, _ctx) = app();
        super::super::fixtures::seed(&mut app.data);
        app.data.link_up = false;
        let job = app.data.jobs[0].clone();
        app.request_run(&job);
        assert!(!app.modal_is_unlock());
        assert_eq!(app.toasts.len(), 1);
    }
}
