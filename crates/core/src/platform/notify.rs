//! Desktop notifications, with superbackup's rules applied.
//!
//! Three things make this more than a thin wrapper around `notify-rust`:
//!
//! 1. **A notification must never fail a backup.** Every entry point returns a
//!    [`NotifyOutcome`] and swallows platform errors into a log line. A missing
//!    D-Bus session on a headless Linux box, a Windows toast subsystem that
//!    refuses because the app is unregistered, a macOS user who denied
//!    permission — none of these are backup failures.
//! 2. **Everything is redacted.** Notification text is assembled from kopia's
//!    stderr and from remote-storage SDK messages, both of which have been
//!    known to echo credentials. [`crate::redact::scrub`] runs on the way out,
//!    with no exception and no fast path around it.
//! 3. **Repeats are suppressed.** A job that fails every fifteen minutes must
//!    not produce ninety-six toasts a day, or the user turns notifications off
//!    and then misses the one that mattered. The dedupe cache is keyed by job
//!    *and* error kind, so a job that starts failing for a new reason still
//!    gets through.
//!
//! # Windows toasts and the AppUserModelID
//!
//! A desktop (unpackaged) Win32 application cannot raise a toast under its own
//! name unless an AppUserModelID has been registered for it. Windows silently
//! drops the toast otherwise — no error, no notification, nothing. Registration
//! is an *installer* job, not something an application can reliably do for
//! itself; see [`installer_requirements`] for the exact list. When the AUMID is
//! missing we fall back to the shell's own identity so the user still sees the
//! notification (attributed to PowerShell, which is ugly but visible) and
//! record a warning the GUI can show once.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::ErrorCode;
use crate::model::NotificationSettings;
use crate::redact;
use crate::state::{Event, Severity};

/// The AppUserModelID the installer must register. Reverse-DNS-ish, stable
/// across versions — changing it orphans every toast setting the user has.
pub const APP_USER_MODEL_ID: &str = "Superbackup.Superbackup";

/// `appname` on XDG, and the display name in notification settings.
pub const APP_NAME: &str = "superbackup";

/// How many dedupe entries to keep. A user with more distinct failing jobs
/// than this has bigger problems, and an unbounded map in a long-lived daemon
/// is a slow leak.
const DEDUPE_CAPACITY: usize = 256;

/// Give up waiting for the platform to accept a notification after this long.
/// A hung notification daemon must not hold up the scheduler.
const SHOW_TIMEOUT_SECONDS: u64 = 10;

// ---------------------------------------------------------------------------
// What we notify about
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationKind {
    /// A job failed. The one everybody actually wants.
    Failure,
    /// A job succeeded. Off by default — a backup tool that is working should
    /// be silent.
    Success,
    /// The service could not start, crashed, or lost its configuration.
    ServiceError,
    /// A job has not succeeded in `stale_after_days`. Not an error yet, but
    /// the failure mode where backups quietly stopped is the one that hurts.
    Stale,
    /// Anything else worth a toast (an update, a repository that filled up).
    Info,
}

impl NotificationKind {
    /// Does the user's configuration allow this kind through?
    ///
    /// Pure, so the policy is testable without a notification daemon.
    pub fn is_permitted(&self, settings: &NotificationSettings) -> bool {
        if !settings.enabled {
            return false;
        }
        match self {
            NotificationKind::Failure => settings.on_failure,
            NotificationKind::Success => settings.on_success,
            NotificationKind::ServiceError => settings.on_service_error,
            // Staleness is the quiet half of failure and rides the same switch:
            // a user who wants to hear about failures wants to hear that
            // nothing has run for a week.
            NotificationKind::Stale => settings.on_failure,
            NotificationKind::Info => true,
        }
    }
}

/// What the tray should do when the user clicks the notification or one of its
/// buttons. Carried as a payload rather than a closure so it can cross the IPC
/// boundary to whichever process owns the window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ActionTarget {
    /// Bring the main window to the front.
    OpenApp,
    /// Open the job's detail page — the run that failed, with its log.
    OpenJob { job_id: Uuid },
    /// Open the destination's settings, e.g. after a "repository full".
    OpenDestination { destination_id: Uuid },
    /// Open the log folder.
    OpenLogs,
    /// Run the job again now.
    RetryJob { job_id: Uuid },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationAction {
    /// Identifier handed to the platform and echoed back on activation.
    pub id: String,
    /// Button label. Kept short: Windows truncates hard.
    pub label: String,
    pub target: ActionTarget,
}

impl NotificationAction {
    pub fn open_job(job_id: Uuid) -> Self {
        NotificationAction {
            id: "open-job".into(),
            label: "Show details".into(),
            target: ActionTarget::OpenJob { job_id },
        }
    }
    pub fn retry_job(job_id: Uuid) -> Self {
        NotificationAction {
            id: "retry-job".into(),
            label: "Try again".into(),
            target: ActionTarget::RetryJob { job_id },
        }
    }
}

/// One notification, before policy and redaction are applied.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub kind: NotificationKind,
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub job_id: Option<Uuid>,
    #[serde(default)]
    pub destination_id: Option<Uuid>,
    /// Distinguishes "failed because the network is down" from "failed because
    /// the passphrase is wrong", so a job that starts failing differently
    /// escapes the dedupe window.
    #[serde(default)]
    pub error_code: Option<ErrorCode>,
    #[serde(default)]
    pub actions: Vec<NotificationAction>,
}

impl Notification {
    pub fn new(
        kind: NotificationKind,
        title: impl Into<String>,
        body: impl Into<String>,
    ) -> Notification {
        Notification {
            kind,
            title: title.into(),
            body: body.into(),
            job_id: None,
            destination_id: None,
            error_code: None,
            actions: Vec::new(),
        }
    }

    pub fn with_job(mut self, job_id: Uuid) -> Self {
        self.job_id = Some(job_id);
        self.actions.push(NotificationAction::open_job(job_id));
        self
    }

    pub fn with_destination(mut self, destination_id: Uuid) -> Self {
        self.destination_id = Some(destination_id);
        self
    }

    pub fn with_error_code(mut self, code: ErrorCode) -> Self {
        self.error_code = Some(code);
        self
    }

    pub fn with_action(mut self, action: NotificationAction) -> Self {
        self.actions.push(action);
        self
    }

    /// The dedupe identity: same job, same kind, same error means "the same
    /// problem, again". Deliberately *not* the message text, which contains
    /// changing byte counts and timestamps and would defeat deduplication.
    pub fn dedupe_key(&self) -> String {
        let subject = self
            .job_id
            .map(|j| j.to_string())
            .or_else(|| self.destination_id.map(|d| d.to_string()))
            .unwrap_or_else(|| "global".to_string());
        let code = self.error_code.map(|c| format!("{c:?}")).unwrap_or_else(|| "none".to_string());
        format!("{:?}|{subject}|{code}", self.kind)
    }

    /// Build a notification from an activity-log [`Event`], so the daemon can
    /// have one code path that both logs and notifies.
    pub fn from_event(event: &Event) -> Option<Notification> {
        // Severity gates first: `service.started` is an Info event and must
        // not be promoted to a "service problem" toast just because of its
        // prefix.
        let kind = match (event.severity, event.kind.as_str()) {
            (Severity::Error, k) | (Severity::Warning, k) if k.starts_with("service.") => {
                NotificationKind::ServiceError
            }
            (Severity::Error, _) => NotificationKind::Failure,
            (Severity::Warning, k) if k.contains("stale") => NotificationKind::Stale,
            (Severity::Warning, _) => NotificationKind::Info,
            // Info and Debug events are activity-log material, not toasts.
            _ => return None,
        };
        let title = match kind {
            NotificationKind::Failure => "Backup failed",
            NotificationKind::ServiceError => "superbackup service problem",
            NotificationKind::Stale => "Backups are out of date",
            _ => "superbackup",
        };
        let mut notification = Notification::new(kind, title, event.message.clone());
        if let Some(job_id) = event.job_id {
            notification = notification.with_job(job_id);
        }
        if let Some(destination_id) = event.destination_id {
            notification = notification.with_destination(destination_id);
        }
        Some(notification)
    }
}

// ---------------------------------------------------------------------------
// Outcome
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotifyOutcome {
    /// Handed to the platform successfully.
    Shown,
    /// Notifications are switched off entirely.
    SuppressedDisabled,
    /// This kind is switched off (e.g. "notify on success" is unchecked).
    SuppressedByKind,
    /// The same problem was reported recently.
    Deduped { seconds_remaining: i64 },
    /// The platform has no notification service (headless Linux, a Windows
    /// service in session 0). Logged instead.
    Unavailable { reason: String },
    /// The platform rejected it. Logged instead.
    Failed { reason: String },
}

impl NotifyOutcome {
    pub fn was_shown(&self) -> bool {
        matches!(self, NotifyOutcome::Shown)
    }
}

// ---------------------------------------------------------------------------
// Dedupe cache
// ---------------------------------------------------------------------------

/// Remembers when each distinct problem was last announced.
///
/// Separated from [`Notifier`] and driven by an injected clock so the window
/// logic can be tested in microseconds rather than in minutes.
#[derive(Debug, Default)]
pub struct DedupeCache {
    entries: HashMap<String, DateTime<Utc>>,
}

impl DedupeCache {
    pub fn new() -> DedupeCache {
        DedupeCache { entries: HashMap::new() }
    }

    /// Should this key be announced now? Records the announcement when yes.
    ///
    /// `window_minutes == 0` disables deduplication entirely, which is what a
    /// user who sets the setting to zero is asking for.
    pub fn admit(&mut self, key: &str, now: DateTime<Utc>, window_minutes: u32) -> Result<(), i64> {
        if window_minutes == 0 {
            return Ok(());
        }
        let window = Duration::minutes(window_minutes as i64);
        if let Some(last) = self.entries.get(key) {
            let elapsed = now - *last;
            if elapsed < window && elapsed >= Duration::zero() {
                return Err((window - elapsed).num_seconds().max(0));
            }
        }
        self.prune(now, window_minutes);
        self.entries.insert(key.to_string(), now);
        Ok(())
    }

    /// Drop entries that can no longer suppress anything, and enforce the
    /// capacity bound by evicting the oldest.
    pub fn prune(&mut self, now: DateTime<Utc>, window_minutes: u32) {
        let window = Duration::minutes(window_minutes as i64);
        self.entries.retain(|_, at| now - *at < window && now >= *at);
        if self.entries.len() >= DEDUPE_CAPACITY {
            let mut by_age: Vec<(String, DateTime<Utc>)> =
                self.entries.iter().map(|(k, v)| (k.clone(), *v)).collect();
            by_age.sort_by_key(|(_, at)| *at);
            for (key, _) in by_age.into_iter().take(self.entries.len() - DEDUPE_CAPACITY / 2) {
                self.entries.remove(&key);
            }
        }
    }

    /// Forget a key, so the next occurrence is announced immediately. Called
    /// when a job succeeds: the next failure is news again.
    pub fn clear_subject(&mut self, subject: &Uuid) {
        let needle = subject.to_string();
        self.entries.retain(|k, _| !k.contains(&needle));
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Windows toast registration
// ---------------------------------------------------------------------------

/// What we know about this machine's ability to show a branded toast.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToastRegistration {
    /// True when an AppUserModelID we can use is registered.
    pub registered: bool,
    /// The AUMID to hand the toast API, or `None` to accept the fallback.
    pub app_id: Option<String>,
    /// A single sentence for the GUI when the toast will be misattributed.
    pub warning: Option<String>,
}

/// Check whether the installer did its job.
///
/// Windows accepts a toast from a desktop application only when its AUMID is
/// known, which happens either through a Start-menu shortcut carrying the
/// `System.AppUserModel.ID` property or through an
/// `HKCU\Software\Classes\AppUserModelId\<AUMID>` key. We look for both.
pub fn toast_registration() -> ToastRegistration {
    #[cfg(windows)]
    {
        use super::win32::{Hive, RegKey};
        let key_path = format!(r"Software\Classes\AppUserModelId\{APP_USER_MODEL_ID}");
        let by_registry = RegKey::open(Hive::CurrentUser, &key_path).is_some();
        let by_shortcut = std::env::var_os("APPDATA")
            .map(std::path::PathBuf::from)
            .map(|appdata| {
                appdata
                    .join(r"Microsoft\Windows\Start Menu\Programs")
                    .join(format!("{APP_NAME}.lnk"))
            })
            .map(|p| p.exists())
            .unwrap_or(false);

        if by_registry || by_shortcut {
            ToastRegistration {
                registered: true,
                app_id: Some(APP_USER_MODEL_ID.to_string()),
                warning: None,
            }
        } else {
            ToastRegistration {
                registered: false,
                app_id: None,
                warning: Some(
                    "Windows has no Start-menu entry for superbackup, so notifications will be \
                     labelled as coming from Windows PowerShell. Reinstalling superbackup fixes \
                     this."
                        .to_string(),
                ),
            }
        }
    }
    #[cfg(not(windows))]
    {
        ToastRegistration { registered: true, app_id: None, warning: None }
    }
}

/// Exactly what the Windows installer must do for toasts to work. Kept in code
/// rather than only in a document so it stays in step with the constant above.
pub fn installer_requirements() -> Vec<String> {
    vec![
        format!(
            "Create a Start-menu shortcut at %APPDATA%\\Microsoft\\Windows\\Start Menu\\\
             Programs\\{APP_NAME}.lnk pointing at superbackup.exe."
        ),
        format!(
            "Set the shortcut's System.AppUserModel.ID property (PKEY_AppUserModel_ID) to \
             exactly \"{APP_USER_MODEL_ID}\". Without this Windows silently discards every \
             toast — no error is reported to the application."
        ),
        format!(
            "Optionally also create HKCU\\Software\\Classes\\AppUserModelId\\\
             {APP_USER_MODEL_ID} with DisplayName and IconUri values, which is what Windows \
             Settings shows in the per-app notification list."
        ),
        "Do not change the AppUserModelID between versions: it is the key under which Windows \
         stores the user's per-app notification preferences."
            .to_string(),
    ]
}

// ---------------------------------------------------------------------------
// Notifier
// ---------------------------------------------------------------------------

type ActionSink = Arc<dyn Fn(ActionTarget) + Send + Sync>;

/// The notification front door. Cheap to clone via `Arc`; safe to share.
pub struct Notifier {
    settings: Mutex<NotificationSettings>,
    dedupe: Mutex<DedupeCache>,
    registration: ToastRegistration,
    sink: Mutex<Option<ActionSink>>,
    /// When true, nothing is handed to the platform — everything is logged.
    /// Used by the service (session 0 has no desktop) and by tests.
    log_only: bool,
}

impl std::fmt::Debug for Notifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Notifier")
            .field("log_only", &self.log_only)
            .field("registration", &self.registration)
            .field("dedupe_entries", &self.dedupe.lock().map(|d| d.len()).unwrap_or(0))
            .finish()
    }
}

impl Notifier {
    pub fn new(settings: NotificationSettings) -> Notifier {
        Notifier {
            settings: Mutex::new(settings),
            dedupe: Mutex::new(DedupeCache::new()),
            registration: toast_registration(),
            sink: Mutex::new(None),
            log_only: false,
        }
    }

    /// A notifier that only writes to the log.
    ///
    /// This is the correct notifier for the Windows service: session 0 is
    /// isolated from every interactive desktop, so a toast raised there is
    /// invisible by design. The tray raises the toast instead, over IPC.
    pub fn log_only(settings: NotificationSettings) -> Notifier {
        Notifier { log_only: true, ..Notifier::new(settings) }
    }

    /// Replace the settings, e.g. after the user saves the Settings page.
    pub fn update_settings(&self, settings: NotificationSettings) {
        if let Ok(mut guard) = self.settings.lock() {
            *guard = settings;
        }
    }

    /// Register the callback the tray uses to act on a click.
    pub fn on_action(&self, sink: impl Fn(ActionTarget) + Send + Sync + 'static) {
        if let Ok(mut guard) = self.sink.lock() {
            *guard = Some(Arc::new(sink));
        }
    }

    /// The warning the GUI should show once, if any.
    pub fn platform_warning(&self) -> Option<&str> {
        self.registration.warning.as_deref()
    }

    /// A job succeeded — forget its failure history so the next failure is
    /// announced immediately rather than being swallowed by the window.
    pub fn subject_recovered(&self, subject: &Uuid) {
        if let Ok(mut cache) = self.dedupe.lock() {
            cache.clear_subject(subject);
        }
    }

    /// Show a notification, honouring settings, dedupe and redaction.
    ///
    /// Never returns an error and never panics; the worst case is a log line.
    pub fn notify(&self, notification: &Notification) -> NotifyOutcome {
        self.notify_at(notification, Utc::now())
    }

    /// [`Notifier::notify`] with an injected clock, for tests.
    pub fn notify_at(&self, notification: &Notification, now: DateTime<Utc>) -> NotifyOutcome {
        let settings = match self.settings.lock() {
            Ok(g) => g.clone(),
            // A poisoned lock means another thread panicked while holding it.
            // Losing a notification is strictly better than propagating that
            // panic into a backup.
            Err(poisoned) => poisoned.into_inner().clone(),
        };

        if !settings.enabled {
            return NotifyOutcome::SuppressedDisabled;
        }
        if !notification.kind.is_permitted(&settings) {
            return NotifyOutcome::SuppressedByKind;
        }

        let key = notification.dedupe_key();
        {
            let mut cache = match self.dedupe.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            if let Err(seconds_remaining) = cache.admit(&key, now, settings.dedupe_minutes) {
                tracing::debug!(key = %key, seconds_remaining, "notification deduplicated");
                return NotifyOutcome::Deduped { seconds_remaining };
            }
        }

        // Redaction is the last thing before the text leaves the process, and
        // there is no path to `deliver` that skips it.
        let title = redact::scrub(&notification.title).into_owned();
        let body = redact::scrub(&notification.body).into_owned();

        if self.log_only {
            tracing::info!(title = %title, body = %body, "notification (log only)");
            return NotifyOutcome::Shown;
        }

        self.deliver(notification, title, body)
    }

    fn deliver(&self, notification: &Notification, title: String, body: String) -> NotifyOutcome {
        let app_id = self.registration.app_id.clone();
        let actions: Vec<(String, String)> = notification
            .actions
            .iter()
            .map(|a| (a.id.clone(), redact::scrub(&a.label).into_owned()))
            .collect();
        let targets: HashMap<String, ActionTarget> =
            notification.actions.iter().map(|a| (a.id.clone(), a.target.clone())).collect();
        let sink = match self.sink.lock() {
            Ok(g) => g.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        };
        // Clicking the notification body (rather than a button) reports the
        // platform's default action id, which we map to the first target.
        let default_target =
            notification.actions.first().map(|a| a.target.clone()).or(Some(ActionTarget::OpenApp));

        let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let log_title = title.clone();

        // The whole platform interaction happens on its own thread: `show()`
        // can block on a D-Bus round trip, and `wait_for_action` blocks until
        // the user clicks or the toast expires. Neither may hold up a backup.
        let spawned =
            std::thread::Builder::new().name("superbackup-notify".into()).spawn(move || {
                let mut builder = notify_rust::Notification::new();
                builder.summary(&title).body(&body).appname(APP_NAME);
                #[cfg(windows)]
                if let Some(id) = &app_id {
                    builder.app_id(id);
                }
                // No other platform has an AppUserModelID; XDG and macOS
                // identify the sender by `appname` and by the bundle id.
                #[cfg(not(windows))]
                {
                    let _ = &app_id;
                }
                for (id, label) in &actions {
                    builder.action(id, label);
                }

                match builder.show() {
                    Ok(handle) => {
                        let _ = tx.send(Ok(()));
                        // macOS is deliberately excluded: on the
                        // NSUserNotificationCenter path `wait_for_action`
                        // re-sends the notification and needs the *main* run
                        // loop to be pumping, so calling it from a worker
                        // thread would show a duplicate toast and then block
                        // that thread for ever. Click handling on macOS
                        // belongs to the app's own notification delegate.
                        #[cfg(not(target_os = "macos"))]
                        if let Some(sink) = sink {
                            handle.wait_for_action(move |id| {
                                let target = targets.get(id).cloned().or(if id == "__closed" {
                                    None
                                } else {
                                    default_target
                                });
                                if let Some(target) = target {
                                    sink(target);
                                }
                            });
                        }
                        #[cfg(target_os = "macos")]
                        {
                            let _ = (&sink, &targets, &default_target, handle);
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Err(e.to_string()));
                    }
                }
            });

        if let Err(e) = spawned {
            tracing::warn!(error = %e, "could not spawn the notification thread");
            return NotifyOutcome::Failed { reason: e.to_string() };
        }

        match rx.recv_timeout(std::time::Duration::from_secs(SHOW_TIMEOUT_SECONDS)) {
            Ok(Ok(())) => NotifyOutcome::Shown,
            Ok(Err(reason)) => {
                let reason = redact::scrub(&reason).into_owned();
                tracing::warn!(
                    title = %log_title,
                    error = %reason,
                    "the desktop refused a notification; logging it instead"
                );
                if looks_unavailable(&reason) {
                    NotifyOutcome::Unavailable { reason }
                } else {
                    NotifyOutcome::Failed { reason }
                }
            }
            Err(_) => {
                // The platform never answered. Assume it worked rather than
                // reporting a failure we cannot substantiate; the thread is
                // detached and will finish or die on its own.
                tracing::debug!(title = %log_title, "notification did not confirm in time");
                NotifyOutcome::Shown
            }
        }
    }
}

/// Distinguish "there is no notification service here" from "it said no".
/// The first is normal on a server; the second deserves a warning.
fn looks_unavailable(reason: &str) -> bool {
    let lower = reason.to_ascii_lowercase();
    lower.contains("no such file or directory")
        || lower.contains("dbus")
        || lower.contains("d-bus")
        || lower.contains("connection refused")
        || lower.contains("not provided by any .service")
        || lower.contains("display")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> NotificationSettings {
        NotificationSettings {
            enabled: true,
            on_failure: true,
            on_success: false,
            stale_after_days: 3,
            on_service_error: true,
            dedupe_minutes: 60,
        }
    }

    fn failure(job: Uuid) -> Notification {
        Notification::new(NotificationKind::Failure, "Backup failed", "disk full")
            .with_job(job)
            .with_error_code(ErrorCode::Io)
    }

    #[test]
    fn dedupe_suppresses_the_same_problem_inside_the_window() {
        let mut cache = DedupeCache::new();
        let t0 = Utc::now();
        assert!(cache.admit("job|io", t0, 60).is_ok());
        assert!(cache.admit("job|io", t0 + Duration::minutes(30), 60).is_err());
        assert!(cache.admit("job|io", t0 + Duration::minutes(61), 60).is_ok());
    }

    #[test]
    fn dedupe_reports_how_long_is_left() {
        let mut cache = DedupeCache::new();
        let t0 = Utc::now();
        assert!(cache.admit("k", t0, 60).is_ok());
        let remaining = cache.admit("k", t0 + Duration::minutes(20), 60).unwrap_err();
        assert!((2350..=2400).contains(&remaining), "got {remaining}");
    }

    #[test]
    fn a_zero_window_disables_dedupe() {
        let mut cache = DedupeCache::new();
        let t0 = Utc::now();
        assert!(cache.admit("k", t0, 0).is_ok());
        assert!(cache.admit("k", t0, 0).is_ok());
        assert!(cache.is_empty(), "a disabled cache must not accumulate state");
    }

    #[test]
    fn dedupe_distinguishes_error_kinds() {
        let job = Uuid::new_v4();
        let io = failure(job);
        let mut auth = failure(job);
        auth.error_code = Some(ErrorCode::BadPassphrase);
        assert_ne!(
            io.dedupe_key(),
            auth.dedupe_key(),
            "a job failing for a new reason must get through"
        );
    }

    #[test]
    fn dedupe_distinguishes_jobs() {
        assert_ne!(failure(Uuid::new_v4()).dedupe_key(), failure(Uuid::new_v4()).dedupe_key());
    }

    #[test]
    fn a_success_clears_the_suppression_for_that_job() {
        let job = Uuid::new_v4();
        let mut cache = DedupeCache::new();
        let key = failure(job).dedupe_key();
        let t0 = Utc::now();
        assert!(cache.admit(&key, t0, 60).is_ok());
        assert!(cache.admit(&key, t0, 60).is_err());
        cache.clear_subject(&job);
        assert!(cache.admit(&key, t0, 60).is_ok(), "the next failure is news again");
    }

    #[test]
    fn the_cache_stays_bounded() {
        let mut cache = DedupeCache::new();
        let t0 = Utc::now();
        for i in 0..(DEDUPE_CAPACITY * 3) {
            let _ = cache.admit(&format!("key-{i}"), t0, 60);
        }
        assert!(cache.len() <= DEDUPE_CAPACITY, "cache grew to {}", cache.len());
    }

    #[test]
    fn expired_entries_are_pruned() {
        let mut cache = DedupeCache::new();
        let t0 = Utc::now();
        let _ = cache.admit("old", t0, 10);
        cache.prune(t0 + Duration::minutes(30), 10);
        assert!(cache.is_empty());
    }

    #[test]
    fn settings_gate_each_kind() {
        let mut s = settings();
        assert!(NotificationKind::Failure.is_permitted(&s));
        assert!(!NotificationKind::Success.is_permitted(&s));
        assert!(NotificationKind::ServiceError.is_permitted(&s));
        assert!(NotificationKind::Stale.is_permitted(&s));
        s.enabled = false;
        for kind in [
            NotificationKind::Failure,
            NotificationKind::Success,
            NotificationKind::ServiceError,
            NotificationKind::Stale,
            NotificationKind::Info,
        ] {
            assert!(!kind.is_permitted(&s), "{kind:?} escaped the master switch");
        }
    }

    #[test]
    fn a_disabled_notifier_reports_why_and_shows_nothing() {
        let mut s = settings();
        s.enabled = false;
        let notifier = Notifier::log_only(s);
        assert_eq!(notifier.notify(&failure(Uuid::new_v4())), NotifyOutcome::SuppressedDisabled);
    }

    #[test]
    fn success_notifications_are_off_by_default() {
        let notifier = Notifier::log_only(settings());
        let n = Notification::new(NotificationKind::Success, "Done", "ok");
        assert_eq!(notifier.notify(&n), NotifyOutcome::SuppressedByKind);
    }

    #[test]
    fn the_notifier_dedupes_end_to_end() {
        let notifier = Notifier::log_only(settings());
        let job = Uuid::new_v4();
        let t0 = Utc::now();
        assert_eq!(notifier.notify_at(&failure(job), t0), NotifyOutcome::Shown);
        assert!(matches!(
            notifier.notify_at(&failure(job), t0 + Duration::minutes(5)),
            NotifyOutcome::Deduped { .. }
        ));
        assert_eq!(
            notifier.notify_at(&failure(job), t0 + Duration::minutes(120)),
            NotifyOutcome::Shown
        );
    }

    #[test]
    fn updating_settings_takes_effect_immediately() {
        let notifier = Notifier::log_only(settings());
        let n = Notification::new(NotificationKind::Success, "Done", "ok");
        assert_eq!(notifier.notify(&n), NotifyOutcome::SuppressedByKind);
        let mut s = settings();
        s.on_success = true;
        notifier.update_settings(s);
        assert_eq!(notifier.notify(&n), NotifyOutcome::Shown);
    }

    #[test]
    fn secrets_never_reach_the_notification_text() {
        // The notifier is log-only here, so assert on the redaction itself:
        // this is the exact transformation `notify_at` applies.
        let raw = "kopia failed: KOPIA_PASSWORD=hunter2 rejected by \
                   https://key:s3cr3t@gateway.storjshare.io/bucket";
        let scrubbed = redact::scrub(raw);
        assert!(!scrubbed.contains("hunter2"), "{scrubbed}");
        assert!(!scrubbed.contains("s3cr3t"), "{scrubbed}");
        assert!(scrubbed.contains("gateway.storjshare.io"), "{scrubbed}");
    }

    #[test]
    fn events_map_to_the_right_notification_kind() {
        let job = Uuid::new_v4();
        let failed = Event::error("job.failed", "disk full").with_job(job);
        let n = Notification::from_event(&failed).expect("errors notify");
        assert_eq!(n.kind, NotificationKind::Failure);
        assert_eq!(n.job_id, Some(job));
        assert!(n.actions.iter().any(|a| a.target == ActionTarget::OpenJob { job_id: job }));

        let svc = Event::error("service.crashed", "exited 1");
        assert_eq!(
            Notification::from_event(&svc).map(|n| n.kind),
            Some(NotificationKind::ServiceError)
        );

        let stale = Event::warn("job.stale", "no successful run in 5 days");
        assert_eq!(Notification::from_event(&stale).map(|n| n.kind), Some(NotificationKind::Stale));

        let chatter = Event::info("job.started", "starting");
        assert!(Notification::from_event(&chatter).is_none(), "info events are not toasts");
    }

    #[test]
    fn actions_carry_a_target_the_tray_can_act_on() {
        let job = Uuid::new_v4();
        let n = failure(job).with_action(NotificationAction::retry_job(job));
        let targets: Vec<&ActionTarget> = n.actions.iter().map(|a| &a.target).collect();
        assert!(targets.contains(&&ActionTarget::OpenJob { job_id: job }));
        assert!(targets.contains(&&ActionTarget::RetryJob { job_id: job }));
        // Round trip through JSON: the tray lives in another process.
        let json = serde_json::to_string(&n.actions).expect("serialisable");
        let back: Vec<NotificationAction> = serde_json::from_str(&json).expect("deserialisable");
        assert_eq!(back, n.actions);
    }

    #[test]
    fn installer_requirements_name_the_app_user_model_id() {
        let reqs = installer_requirements();
        assert!(reqs.iter().any(|r| r.contains(APP_USER_MODEL_ID)));
        assert!(reqs.iter().any(|r| r.contains("System.AppUserModel.ID")));
    }

    #[test]
    fn unavailable_is_distinguished_from_refused() {
        assert!(looks_unavailable("Failed to connect to D-Bus: No such file or directory"));
        assert!(!looks_unavailable("The notification was rejected by policy"));
    }
}
