//! One module per screen. Each owns its own ephemeral state — search text,
//! sort order, expanded rows — and reads everything else from `Data`.
//!
//! Screens are written as `impl App` blocks so that a screen can call the
//! app's own intents (`request_run`, `go`, `open_modal`) without threading a
//! second borrow through every function.

pub mod about;
pub mod activity;
pub mod dashboard;
pub mod destination_editor;
pub mod destinations;
pub mod job_editor;
pub mod jobs;
pub mod onboarding;
pub mod preview;
pub mod provider_editor;
pub mod providers;
pub mod restore;
pub mod run_detail;
pub mod settings;
pub mod wizard;

/// Every screen's state, owned by the app.
#[derive(Default)]
pub struct Screens {
    pub jobs: jobs::State,
    pub job_editor: job_editor::State,
    pub destinations: destinations::State,
    pub destination_editor: destination_editor::State,
    pub providers: providers::State,
    pub provider_editor: provider_editor::State,
    pub activity: activity::State,
    pub restore: restore::State,
    pub settings: settings::State,
    pub preview: preview::State,
    pub run_detail: run_detail::State,
}

/// The six `keep_*` values plus the maintenance interval, shared by the job
/// editor and the destination editor so one policy is never edited two ways.
pub fn retention_editor(ui: &mut egui::Ui, policy: &mut superbackup_core::model::RetentionPolicy) {
    job_editor::retention_grid(ui, policy);
}

impl Screens {
    /// True while any screen is waiting on the daemon, so the app keeps asking
    /// for frames while a spinner turns and stops the moment it does not.
    pub fn busy(&self) -> bool {
        self.destinations.busy()
            || self.destination_editor.busy()
            || self.provider_editor.busy()
            || self.restore.busy()
            || self.settings.busy()
            || self.preview.busy()
    }
}
