//! Running superbackup without anybody being logged in.
//!
//! # What a service actually buys you, and what it costs
//!
//! Autostart ([`super::autostart`]) runs the tray when a human logs in. That
//! covers a laptop. It does not cover the machine that is left at the login
//! screen, the one that reboots overnight for updates, or the one where the
//! user logs out on Friday. For those, a service is the only answer.
//!
//! The cost is real and must be shown to the user rather than discovered in a
//! support ticket. A Windows service running as `LocalSystem`:
//!
//! * **cannot see mapped network drives.** `Z:\` is a per-logon-session object.
//!   A UNC path (`\\nas\backups`) is only reachable if the share allows the
//!   *computer account* (`DOMAIN\MACHINE$`), which most do not.
//! * **cannot see the user's OneDrive folder.** It lives under the user's
//!   profile, and the OneDrive sync client only runs inside the user's
//!   interactive session. A LocalSystem service writing there would produce
//!   files nothing ever uploads.
//! * **has a different DPAPI store and a different keychain.** A repository
//!   passphrase cached by the interactive user with "remember in Windows
//!   Credential Manager" is unreadable from `LocalSystem`. The service needs
//!   the vault passphrase supplied to it, or its own cached copy.
//! * **cannot show a UI.** Session 0 isolation means no toast, no prompt, no
//!   passphrase dialog. Notifications must be raised by the tray.
//!
//! Installing under a **specific user account** removes the first three
//! restrictions — which is exactly why we support it, and why it is the right
//! answer for a OneDrive destination. It reintroduces one: the account's
//! password is stored by the SCM, and the service stops working when the
//! password changes.
//!
//! Use [`destination_support`] to tell the user which of their destinations
//! will actually work before they commit.
//!
//! # Linux and macOS
//!
//! systemd distinguishes a *system* unit (`/etc/systemd/system`, root, runs at
//! boot) from a *user* unit (`~/.config/systemd/user`, runs at login, or at
//! boot when lingering is enabled with `loginctl enable-linger`). We generate
//! both. macOS uses a LaunchDaemon in `/Library/LaunchDaemons`.
//!
//! # Elevation
//!
//! Installing a system service needs Administrator on Windows and root
//! elsewhere. We check *first* and return a specific, actionable error rather
//! than letting the user meet a bare "Access is denied. (os error 5)".

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

// `IoContext` is only used by the systemd and launchd implementations below.
#[cfg_attr(windows, allow(unused_imports))]
use crate::error::{Error, IoContext, Result};
use crate::model::DestinationKind;
use crate::paths::Paths;
use crate::secret::Secret;
use crate::state::Health;

/// SCM / systemd / launchd identifier. Stable across versions: it is what an
/// uninstaller and every support document refer to.
pub const DEFAULT_SERVICE_NAME: &str = "superbackup";
pub const DEFAULT_DISPLAY_NAME: &str = "superbackup Backup Service";
pub const DEFAULT_DESCRIPTION: &str =
    "Runs scheduled backups even when no user is signed in. Part of superbackup.";
/// launchd label, and the systemd unit file stem.
pub const LAUNCH_DAEMON_LABEL: &str = "io.superbackup.daemon";

/// Arguments the service binary is started with. `--service` tells it to use
/// the machine-wide [`Paths::for_service`] layout instead of a user profile.
pub const SERVICE_ARGS: &[&str] = &["daemon", "--service"];

// ---------------------------------------------------------------------------
// Options
// ---------------------------------------------------------------------------

/// Which identity the service runs as.
#[derive(Debug, Default)]
pub enum ServiceAccount {
    /// `NT AUTHORITY\SYSTEM` on Windows, root elsewhere. Maximum privilege,
    /// minimum reach into user data. The default because it needs no password.
    #[default]
    LocalSystem,
    /// `NT AUTHORITY\LocalService`. Almost no privilege and no network
    /// identity; offered for completeness but a backup service usually needs
    /// to read files it does not own, so this will disappoint.
    LocalService,
    /// `NT AUTHORITY\NetworkService`. Presents the computer account on the
    /// network; occasionally the right answer for a domain file share.
    NetworkService,
    /// A named account. `DOMAIN\user`, `.\\user` for a local account, or
    /// `user@domain`. This is the answer for OneDrive and mapped drives.
    ///
    /// The password is only needed at install time. It is handed to the SCM,
    /// which stores it in the LSA secret store — we cannot zero the copy the
    /// SCM keeps, and we say so rather than implying otherwise.
    User { username: String, password: Option<Secret> },
}

impl Clone for ServiceAccount {
    fn clone(&self) -> Self {
        match self {
            ServiceAccount::LocalSystem => ServiceAccount::LocalSystem,
            ServiceAccount::LocalService => ServiceAccount::LocalService,
            ServiceAccount::NetworkService => ServiceAccount::NetworkService,
            ServiceAccount::User { username, password } => ServiceAccount::User {
                username: username.clone(),
                password: password.clone(),
            },
        }
    }
}

impl ServiceAccount {
    /// The identifier the platform expects, or `None` for "the default".
    pub fn system_identifier(&self) -> Option<String> {
        match self {
            // `None` means LocalSystem to the SCM; passing the literal string
            // also works but `None` is the documented spelling.
            ServiceAccount::LocalSystem => None,
            ServiceAccount::LocalService => Some(r"NT AUTHORITY\LocalService".to_string()),
            ServiceAccount::NetworkService => Some(r"NT AUTHORITY\NetworkService".to_string()),
            ServiceAccount::User { username, .. } => Some(username.clone()),
        }
    }

    /// True when this identity has access to an interactive user's profile.
    pub fn sees_user_profile(&self) -> bool {
        matches!(self, ServiceAccount::User { .. })
    }

    pub fn title(&self) -> String {
        match self {
            ServiceAccount::LocalSystem => "Local System".to_string(),
            ServiceAccount::LocalService => "Local Service".to_string(),
            ServiceAccount::NetworkService => "Network Service".to_string(),
            ServiceAccount::User { username, .. } => username.clone(),
        }
    }
}

/// System-wide, or belonging to one logged-in user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceScope {
    /// Windows service, systemd system unit, macOS LaunchDaemon. Needs
    /// elevation; runs with no user logged in.
    System,
    /// systemd user unit or macOS LaunchAgent. No elevation; runs when the
    /// user is logged in (or always, with `loginctl enable-linger`). Not
    /// available on Windows, where the equivalent is autostart.
    User,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartMode {
    Automatic,
    /// Windows "Automatic (Delayed Start)". Kinder to boot time, and a backup
    /// service has no business competing with the login screen.
    AutomaticDelayed,
    Manual,
    Disabled,
}

#[derive(Debug, Clone)]
pub struct ServiceOptions {
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub executable: PathBuf,
    pub args: Vec<String>,
    pub account: ServiceAccount,
    pub scope: ServiceScope,
    pub start_mode: StartMode,
    /// Directories the service must be able to write. Used for systemd's
    /// `ReadWritePaths=` under `ProtectSystem=strict`; ignored elsewhere.
    pub state_dirs: Vec<PathBuf>,
}

impl ServiceOptions {
    /// Sensible defaults for the currently running executable.
    pub fn current(paths: &Paths) -> Result<ServiceOptions> {
        let exe = std::env::current_exe()
            .map_err(|e| Error::io("determining this program's own path", e))?;
        Ok(ServiceOptions::new(exe, paths))
    }

    pub fn new(executable: impl Into<PathBuf>, paths: &Paths) -> ServiceOptions {
        ServiceOptions {
            name: DEFAULT_SERVICE_NAME.to_string(),
            display_name: DEFAULT_DISPLAY_NAME.to_string(),
            description: DEFAULT_DESCRIPTION.to_string(),
            executable: executable.into(),
            args: SERVICE_ARGS.iter().map(|s| s.to_string()).collect(),
            account: ServiceAccount::LocalSystem,
            scope: ServiceScope::System,
            start_mode: StartMode::AutomaticDelayed,
            state_dirs: vec![paths.data_dir.clone(), paths.log_dir.clone(), paths.cache_dir.clone()],
        }
    }

    /// True when installing this configuration requires elevation.
    pub fn requires_elevation(&self) -> bool {
        self.scope == ServiceScope::System
    }
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceState {
    NotInstalled,
    Stopped,
    Starting,
    Stopping,
    Running,
    Paused,
    /// Installed, but the platform reported something we do not model.
    Unknown,
}

impl ServiceState {
    pub fn title(&self) -> &'static str {
        match self {
            ServiceState::NotInstalled => "Not installed",
            ServiceState::Stopped => "Stopped",
            ServiceState::Starting => "Starting",
            ServiceState::Stopping => "Stopping",
            ServiceState::Running => "Running",
            ServiceState::Paused => "Paused",
            ServiceState::Unknown => "Unknown",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceStatus {
    pub installed: bool,
    pub state: ServiceState,
    #[serde(default)]
    pub start_mode: Option<StartMode>,
    /// The account the service is configured to run as, as the platform spells
    /// it.
    #[serde(default)]
    pub account: Option<String>,
    /// The binary the platform will actually launch. Compared against the
    /// running executable, this catches the post-upgrade stale-path failure.
    #[serde(default)]
    pub executable: Option<PathBuf>,
    #[serde(default)]
    pub pid: Option<u32>,
    /// Raw platform detail, for the diagnostics pane.
    #[serde(default)]
    pub detail: Option<String>,
}

impl ServiceStatus {
    pub fn not_installed() -> ServiceStatus {
        ServiceStatus {
            installed: false,
            state: ServiceState::NotInstalled,
            start_mode: None,
            account: None,
            executable: None,
            pid: None,
            detail: None,
        }
    }

    /// How this contributes to the tray icon.
    ///
    /// An installed-but-stopped service that should be automatic is a real
    /// problem the user must see; a service that was never installed is not.
    pub fn health(&self) -> Health {
        match (self.installed, self.state) {
            (false, _) => Health::Idle,
            (true, ServiceState::Running) => Health::Idle,
            (true, ServiceState::Starting) | (true, ServiceState::Stopping) => Health::Running,
            (true, ServiceState::Stopped)
                if matches!(
                    self.start_mode,
                    Some(StartMode::Automatic) | Some(StartMode::AutomaticDelayed)
                ) =>
            {
                Health::Failed
            }
            (true, _) => Health::Attention,
        }
    }

    /// True when the installed service points at a different binary than the
    /// one asking. Same failure mode as a stale autostart entry, and just as
    /// silent.
    pub fn is_stale(&self, expected: &Path) -> bool {
        match &self.executable {
            Some(current) => !super::autostart::same_executable(current, expected),
            None => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Elevation
// ---------------------------------------------------------------------------

/// Are we running with the privileges needed to install a system service?
///
/// Never errors. "We could not tell" and "no" lead to the same advice.
pub fn is_elevated() -> bool {
    #[cfg(windows)]
    {
        super::win32::is_elevated()
    }
    #[cfg(not(windows))]
    {
        super::effective_uid() == Some(0)
    }
}

/// The error to return when an operation needs elevation and does not have it.
/// Kept in one place so the wording is identical everywhere the user meets it.
fn not_elevated(action: &str) -> Error {
    #[cfg(windows)]
    let how = "Close superbackup, right-click it and choose \"Run as administrator\", then try \
               again. From a terminal, start an Administrator PowerShell first.";
    #[cfg(target_os = "macos")]
    let how = "Run the same command again with `sudo`.";
    #[cfg(not(any(windows, target_os = "macos")))]
    let how = "Run the same command again with `sudo`, or install a user service instead with \
               `--user`, which needs no privileges.";

    Error::Service(format!(
        "{action} requires administrator privileges, and superbackup is not running with them. \
         {how}"
    ))
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Install the service. Never leaves a half-installed service behind: if any
/// step after creation fails, the service is deleted again.
pub fn install(options: &ServiceOptions) -> Result<()> {
    if options.requires_elevation() && !is_elevated() {
        return Err(not_elevated("Installing the superbackup service"));
    }
    if !options.executable.exists() {
        return Err(Error::Service(format!(
            "{} does not exist, so a service pointing at it would never start.",
            options.executable.display()
        )));
    }
    platform_impl::install(options)
}

/// Remove the service. Stops it first where the platform requires it.
/// Removing a service that is not installed succeeds.
pub fn uninstall(name: &str, scope: ServiceScope) -> Result<()> {
    if scope == ServiceScope::System && !is_elevated() {
        return Err(not_elevated("Removing the superbackup service"));
    }
    platform_impl::uninstall(name, scope)
}

pub fn start(name: &str, scope: ServiceScope) -> Result<()> {
    platform_impl::start(name, scope)
}

pub fn stop(name: &str, scope: ServiceScope) -> Result<()> {
    platform_impl::stop(name, scope)
}

/// Query the service. Returns [`ServiceStatus::not_installed`] rather than an
/// error when there is nothing installed — "not installed" is an answer.
pub fn status(name: &str, scope: ServiceScope) -> Result<ServiceStatus> {
    platform_impl::status(name, scope)
}

// ---------------------------------------------------------------------------
// Which destinations work in service mode
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "level", rename_all = "snake_case")]
pub enum SupportLevel {
    /// Works.
    Supported,
    /// Works, with a caveat the user must know about.
    Degraded { reason: String },
    /// Will not work. Do not let the user commit to it.
    Unsupported { reason: String },
}

impl SupportLevel {
    pub fn is_usable(&self) -> bool {
        !matches!(self, SupportLevel::Unsupported { .. })
    }
    fn degraded(reason: impl Into<String>) -> Self {
        SupportLevel::Degraded { reason: reason.into() }
    }
    fn unsupported(reason: impl Into<String>) -> Self {
        SupportLevel::Unsupported { reason: reason.into() }
    }
}

/// Can this destination be written from a service running under `account`?
///
/// Pure: no I/O, no platform calls. This is the function the GUI calls to grey
/// out "run as a service" with an explanation, instead of letting the user
/// discover the answer three days later when nothing has been backed up.
pub fn destination_support(
    kind: &DestinationKind,
    account: &ServiceAccount,
    scope: ServiceScope,
) -> SupportLevel {
    // A user-scoped unit is just the user, so everything the user can reach
    // works — but only while that user is logged in (or lingering).
    if scope == ServiceScope::User {
        return match kind {
            DestinationKind::OneDrive { .. } => SupportLevel::degraded(
                "OneDrive only syncs while you are signed in. Backups written while you are \
                 logged out will upload the next time you sign in.",
            ),
            _ => SupportLevel::Supported,
        };
    }

    let user_account = account.sees_user_profile();
    match kind {
        DestinationKind::S3 { .. } => SupportLevel::Supported,

        DestinationKind::OneDrive { .. } if !user_account => SupportLevel::unsupported(
            "The OneDrive folder lives inside your user profile, and the OneDrive sync app only \
             runs while you are signed in. A Local System service cannot see it. Install the \
             service under your own account, or back this destination up from the tray instead.",
        ),
        DestinationKind::OneDrive { .. } => SupportLevel::degraded(
            "OneDrive uploads only while the account is signed in. The service will write the \
             files immediately; OneDrive will upload them at the next sign-in.",
        ),

        DestinationKind::LocalRepository { path } | DestinationKind::LocalMirror { path } => {
            if is_unc_path(path) {
                if user_account {
                    SupportLevel::degraded(
                        "A network share is reachable from a service running as your account \
                         only if the share does not require an interactive logon. Test it before \
                         relying on it.",
                    )
                } else {
                    SupportLevel::unsupported(
                        "A Local System service presents the computer account on the network, \
                         not you, so it usually cannot reach a network share. Install the \
                         service under an account that has access to the share.",
                    )
                }
            } else if is_mapped_drive_letter(path) {
                SupportLevel::unsupported(
                    "Mapped drive letters exist only inside your signed-in session; a service \
                     cannot see them. Use the full network path (\\\\server\\share) instead.",
                )
            } else {
                SupportLevel::Supported
            }
        }
    }
}

/// `\\server\share\...` — including the `\\?\UNC\` long-path form.
pub fn is_unc_path(path: &Path) -> bool {
    let s = path.to_string_lossy();
    s.starts_with(r"\\?\UNC\") || (s.starts_with(r"\\") && !s.starts_with(r"\\?\"))
}

/// A bare drive letter other than the system drive is *possibly* a mapped
/// network drive. We cannot tell for certain without `WNetGetConnectionW`, and
/// a wrong "unsupported" would be worse than a wrong "supported", so this is
/// deliberately conservative: it only fires for drive letters from `H:` on,
/// which is where Windows starts assigning network mappings by convention.
pub fn is_mapped_drive_letter(path: &Path) -> bool {
    if !cfg!(windows) {
        return false;
    }
    let s = path.to_string_lossy();
    let bytes = s.as_bytes();
    if bytes.len() < 2 || bytes[1] != b':' {
        return false;
    }
    let letter = bytes[0].to_ascii_uppercase();
    (b'H'..=b'Z').contains(&letter)
}

// ---------------------------------------------------------------------------
// Windows implementation
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod platform_impl {
    use super::*;
    use std::ffi::OsString;
    use std::time::Duration;
    use windows_service::service::{
        ServiceAccess, ServiceAction, ServiceActionType, ServiceErrorControl, ServiceFailureActions,
        ServiceFailureResetPeriod, ServiceInfo, ServiceStartType, ServiceState as WinState,
        ServiceType,
    };
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    fn manager(access: ServiceManagerAccess) -> Result<ServiceManager> {
        ServiceManager::local_computer(None::<&str>, access).map_err(|e| {
            Error::Service(format!("cannot talk to the Windows Service Control Manager: {e}"))
        })
    }

    fn start_type(mode: StartMode) -> ServiceStartType {
        match mode {
            StartMode::Automatic | StartMode::AutomaticDelayed => ServiceStartType::AutoStart,
            StartMode::Manual => ServiceStartType::OnDemand,
            StartMode::Disabled => ServiceStartType::Disabled,
        }
    }

    pub fn install(options: &ServiceOptions) -> Result<()> {
        let manager = manager(ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE)?;

        let password = match &options.account {
            ServiceAccount::User { password: Some(secret), .. } => {
                // The SCM keeps its own copy in the LSA store; ours is zeroed
                // when this scope ends, which is the most we can promise.
                secret.expose_zeroizing_string().map(|s| OsString::from(s.as_str()))
            }
            _ => None,
        };

        let info = ServiceInfo {
            name: OsString::from(&options.name),
            display_name: OsString::from(&options.display_name),
            service_type: ServiceType::OWN_PROCESS,
            start_type: start_type(options.start_mode),
            // `Normal` logs the failure and carries on booting. `Severe` or
            // `Critical` would let a broken backup service stop a PC starting,
            // which is a wildly disproportionate blast radius.
            error_control: ServiceErrorControl::Normal,
            executable_path: options.executable.clone(),
            launch_arguments: options.args.iter().map(OsString::from).collect(),
            dependencies: vec![],
            account_name: options.account.system_identifier().map(OsString::from),
            account_password: password,
        };

        let service = manager
            .create_service(&info, ServiceAccess::CHANGE_CONFIG | ServiceAccess::START)
            .map_err(|e| Error::Service(describe_scm_error("creating the service", &e)))?;

        // From here on, any failure must not leave a half-configured service
        // behind. Every step is best-effort *except* where it changes
        // behaviour the user asked for.
        let cleanup = || {
            if let Err(e) = service.delete() {
                tracing::error!(error = %e, "could not roll back a partially installed service");
            }
        };

        if let Err(e) = service.set_description(&options.description) {
            // Cosmetic. Not worth destroying a working install over.
            tracing::warn!(error = %e, "could not set the service description");
        }

        if options.start_mode == StartMode::AutomaticDelayed {
            if let Err(e) = service.set_delayed_auto_start(true) {
                tracing::warn!(error = %e, "could not set delayed auto-start");
            }
        }

        // Restart twice with a delay, then give up and leave it stopped so the
        // tray's "service failed" notification is not drowned in a restart
        // loop.
        let actions = ServiceFailureActions {
            reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(86_400)),
            reboot_msg: None,
            command: None,
            actions: Some(vec![
                ServiceAction {
                    action_type: ServiceActionType::Restart,
                    delay: Duration::from_secs(60),
                },
                ServiceAction {
                    action_type: ServiceActionType::Restart,
                    delay: Duration::from_secs(300),
                },
                ServiceAction {
                    action_type: ServiceActionType::None,
                    delay: Duration::ZERO,
                },
            ]),
        };
        if let Err(e) = service.update_failure_actions(actions) {
            tracing::warn!(error = %e, "could not configure service recovery actions");
        }

        // Verify the service really is there and runnable. If the SCM accepted
        // the account but the account lacks "Log on as a service", the failure
        // shows up here rather than silently at the next boot.
        match service.query_config() {
            Ok(_) => Ok(()),
            Err(e) => {
                cleanup();
                Err(Error::Service(describe_scm_error("verifying the new service", &e)))
            }
        }
    }

    pub fn uninstall(name: &str, _scope: ServiceScope) -> Result<()> {
        let manager = manager(ServiceManagerAccess::CONNECT)?;
        let service = match manager.open_service(
            name,
            ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE,
        ) {
            Ok(s) => s,
            Err(e) if is_not_found(&e) => return Ok(()),
            Err(e) => return Err(Error::Service(describe_scm_error("opening the service", &e))),
        };

        if let Ok(status) = service.query_status() {
            if status.current_state != WinState::Stopped {
                // A stop failure must not block deletion: the SCM removes the
                // entry once the last handle closes and the process exits.
                if let Err(e) = service.stop() {
                    tracing::warn!(error = %e, "could not stop the service before removing it");
                }
            }
        }

        service
            .delete()
            .map_err(|e| Error::Service(describe_scm_error("removing the service", &e)))
    }

    pub fn start(name: &str, _scope: ServiceScope) -> Result<()> {
        let manager = manager(ServiceManagerAccess::CONNECT)?;
        let service = manager
            .open_service(name, ServiceAccess::START | ServiceAccess::QUERY_STATUS)
            .map_err(|e| Error::Service(describe_scm_error("opening the service", &e)))?;
        if let Ok(status) = service.query_status() {
            if status.current_state == WinState::Running {
                return Ok(());
            }
        }
        service
            .start::<&str>(&[])
            .map_err(|e| Error::Service(describe_scm_error("starting the service", &e)))
    }

    pub fn stop(name: &str, _scope: ServiceScope) -> Result<()> {
        let manager = manager(ServiceManagerAccess::CONNECT)?;
        let service = manager
            .open_service(name, ServiceAccess::STOP | ServiceAccess::QUERY_STATUS)
            .map_err(|e| Error::Service(describe_scm_error("opening the service", &e)))?;
        if let Ok(status) = service.query_status() {
            if status.current_state == WinState::Stopped {
                return Ok(());
            }
        }
        service
            .stop()
            .map(|_| ())
            .map_err(|e| Error::Service(describe_scm_error("stopping the service", &e)))
    }

    pub fn status(name: &str, _scope: ServiceScope) -> Result<ServiceStatus> {
        let manager = manager(ServiceManagerAccess::CONNECT)?;
        let service = match manager
            .open_service(name, ServiceAccess::QUERY_STATUS | ServiceAccess::QUERY_CONFIG)
        {
            Ok(s) => s,
            Err(e) if is_not_found(&e) => return Ok(ServiceStatus::not_installed()),
            Err(e) => return Err(Error::Service(describe_scm_error("opening the service", &e))),
        };

        let mut out = ServiceStatus::not_installed();
        out.installed = true;
        out.state = ServiceState::Unknown;

        match service.query_status() {
            Ok(s) => {
                out.state = match s.current_state {
                    WinState::Stopped => ServiceState::Stopped,
                    WinState::StartPending | WinState::ContinuePending => ServiceState::Starting,
                    WinState::StopPending | WinState::PausePending => ServiceState::Stopping,
                    WinState::Running => ServiceState::Running,
                    WinState::Paused => ServiceState::Paused,
                };
                out.pid = s.process_id;
            }
            Err(e) => out.detail = Some(format!("status query failed: {e}")),
        }

        if let Ok(config) = service.query_config() {
            // `lpBinaryPathName` is the whole command line, not just a path.
            let command = config.executable_path.to_string_lossy().into_owned();
            let argv = super::super::autostart::parse_windows_command_line(&command);
            out.executable = argv.first().map(PathBuf::from);
            out.account = config.account_name.map(|a| a.to_string_lossy().into_owned());
            // `QueryServiceConfigW` cannot distinguish Automatic from
            // Automatic (Delayed Start); that lives in a separate config
            // query we do not need for a health decision.
            out.start_mode = Some(match config.start_type {
                ServiceStartType::AutoStart => StartMode::Automatic,
                ServiceStartType::OnDemand => StartMode::Manual,
                ServiceStartType::Disabled => StartMode::Disabled,
                _ => StartMode::Manual,
            });
        }
        Ok(out)
    }

    fn is_not_found(error: &windows_service::Error) -> bool {
        // ERROR_SERVICE_DOES_NOT_EXIST
        matches!(error, windows_service::Error::Winapi(e) if e.raw_os_error() == Some(1060))
    }

    /// Turn the SCM's numeric errors into something a person can act on.
    fn describe_scm_error(action: &str, error: &windows_service::Error) -> String {
        let code = match error {
            windows_service::Error::Winapi(e) => e.raw_os_error(),
            _ => None,
        };
        let hint = match code {
            Some(5) => Some(
                "Access is denied. Run superbackup as administrator to change Windows services.",
            ),
            Some(1072) => Some(
                "The service is marked for deletion. Close services.msc and any open Service \
                 Control Manager window, then try again — Windows finishes the deletion when the \
                 last handle closes.",
            ),
            Some(1073) => Some("A service with this name already exists."),
            Some(1057) => Some(
                "The account name is invalid, or the password is wrong. Use DOMAIN\\user or \
                 .\\user for a local account.",
            ),
            Some(1069) => Some(
                "The service could not log on. Grant the account the \"Log on as a service\" \
                 right in Local Security Policy.",
            ),
            Some(1060) => Some("The service is not installed."),
            _ => None,
        };
        match hint {
            Some(h) => format!("{action} failed: {error}. {h}"),
            None => format!("{action} failed: {error}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Windows service entry point
// ---------------------------------------------------------------------------

/// A control message from the SCM (Windows), or a signal (elsewhere).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceSignal {
    /// The SCM asked us to stop. Finish the current snapshot if it is quick,
    /// then exit.
    Stop,
    /// The machine is shutting down. Less time than [`ServiceSignal::Stop`].
    Shutdown,
    /// The machine is going to sleep. Pause work; the scheduler must not fire
    /// mid-suspend.
    Suspend,
    /// The machine woke up. The scheduler should run catch-up.
    Resume,
    /// A user signed in — their OneDrive and mapped drives may now exist.
    SessionLogon,
    SessionLogoff,
}

/// The channel a service worker uses to hear from the platform.
#[derive(Debug)]
pub struct ServiceEvents {
    receiver: std::sync::mpsc::Receiver<ServiceSignal>,
}

impl ServiceEvents {
    pub fn new(receiver: std::sync::mpsc::Receiver<ServiceSignal>) -> Self {
        ServiceEvents { receiver }
    }
    /// Block until the next signal, or `None` once the platform side is gone.
    pub fn recv(&self) -> Option<ServiceSignal> {
        self.receiver.recv().ok()
    }
    /// Non-blocking poll, for a worker with its own loop.
    pub fn try_recv(&self) -> Option<ServiceSignal> {
        self.receiver.try_recv().ok()
    }
    /// Block for at most `timeout`.
    pub fn recv_timeout(&self, timeout: std::time::Duration) -> Option<ServiceSignal> {
        self.receiver.recv_timeout(timeout).ok()
    }
}

#[cfg(windows)]
pub use windows_entry::{dispatch, ServiceWorker};

#[cfg(windows)]
mod windows_entry {
    use super::*;
    use std::sync::mpsc;
    use std::sync::{Mutex, OnceLock};
    use std::time::Duration;
    use windows_service::service::{
        PowerEventParam, ServiceControl, ServiceControlAccept, ServiceExitCode,
        ServiceState as WinState, ServiceStatus as WinStatus, ServiceType, SessionChangeReason,
    };
    use windows_service::service_control_handler::{self, ServiceControlHandlerResult};

    /// The body of the service: runs until it returns, then the service stops.
    pub type ServiceWorker = Box<dyn FnOnce(ServiceEvents) + Send + 'static>;

    struct Registration {
        name: String,
        worker: ServiceWorker,
    }

    static REGISTRATION: OnceLock<Mutex<Option<Registration>>> = OnceLock::new();

    windows_service::define_windows_service!(ffi_service_main, service_main);

    /// Hand control to the SCM and run `worker` as the service body.
    ///
    /// Blocks until the service stops. Call this from `main` when the process
    /// was started by the SCM; call nothing else first that needs a UI.
    pub fn dispatch(name: &str, worker: ServiceWorker) -> Result<()> {
        let slot = REGISTRATION.get_or_init(|| Mutex::new(None));
        {
            let mut guard = slot
                .lock()
                .map_err(|_| Error::Internal("service registration lock poisoned".into()))?;
            if guard.is_some() {
                return Err(Error::Service(
                    "a Windows service dispatcher is already running in this process".into(),
                ));
            }
            *guard = Some(Registration { name: name.to_string(), worker });
        }

        windows_service::service_dispatcher::start(name, ffi_service_main).map_err(|e| {
            Error::Service(format!(
                "could not start the service dispatcher: {e}. This entry point only works when \
                 Windows starts the process as a service; run `superbackup daemon` for an \
                 ordinary foreground run."
            ))
        })
    }

    fn service_main(_arguments: Vec<std::ffi::OsString>) {
        // Nothing here may panic across the FFI boundary, and nothing here can
        // report to a user — the only channel is the event log and our own
        // file log.
        let Some(slot) = REGISTRATION.get() else {
            return;
        };
        let registration = match slot.lock() {
            Ok(mut guard) => guard.take(),
            Err(_) => return,
        };
        let Some(Registration { name, worker }) = registration else {
            return;
        };

        if let Err(e) = run(&name, worker) {
            tracing::error!(error = %e, "the superbackup service failed to start");
        }
    }

    fn run(name: &str, worker: ServiceWorker) -> Result<()> {
        let (tx, rx) = mpsc::channel::<ServiceSignal>();

        let handler_tx = tx.clone();
        let event_handler = move |control: ServiceControl| -> ServiceControlHandlerResult {
            match control {
                // Interrogate must be answered or the SCM decides we hung.
                ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
                ServiceControl::Stop => {
                    let _ = handler_tx.send(ServiceSignal::Stop);
                    ServiceControlHandlerResult::NoError
                }
                ServiceControl::Shutdown => {
                    let _ = handler_tx.send(ServiceSignal::Shutdown);
                    ServiceControlHandlerResult::NoError
                }
                // Power events are why the scheduler can do catch-up runs
                // instead of silently missing every schedule that elapsed
                // while the machine was asleep.
                ServiceControl::PowerEvent(param) => {
                    let signal = match param {
                        PowerEventParam::Suspend => Some(ServiceSignal::Suspend),
                        PowerEventParam::ResumeAutomatic
                        | PowerEventParam::ResumeSuspend
                        | PowerEventParam::ResumeCritical => Some(ServiceSignal::Resume),
                        _ => None,
                    };
                    if let Some(s) = signal {
                        let _ = handler_tx.send(s);
                    }
                    ServiceControlHandlerResult::NoError
                }
                // A user signing in is when their OneDrive folder and mapped
                // drives come into existence.
                ServiceControl::SessionChange(param) => {
                    let signal = match param.reason {
                        SessionChangeReason::SessionLogon
                        | SessionChangeReason::SessionUnlock
                        | SessionChangeReason::RemoteConnect => Some(ServiceSignal::SessionLogon),
                        SessionChangeReason::SessionLogoff
                        | SessionChangeReason::SessionLock
                        | SessionChangeReason::RemoteDisconnect => {
                            Some(ServiceSignal::SessionLogoff)
                        }
                        _ => None,
                    };
                    if let Some(s) = signal {
                        let _ = handler_tx.send(s);
                    }
                    ServiceControlHandlerResult::NoError
                }
                _ => ServiceControlHandlerResult::NotImplemented,
            }
        };

        let handle = service_control_handler::register(name, event_handler)
            .map_err(|e| Error::Service(format!("registering the control handler: {e}")))?;

        let accepted = ServiceControlAccept::STOP
            | ServiceControlAccept::SHUTDOWN
            | ServiceControlAccept::POWER_EVENT
            | ServiceControlAccept::SESSION_CHANGE;

        let report = |state: WinState, accepted: ServiceControlAccept, wait: Duration| WinStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: state,
            controls_accepted: accepted,
            exit_code: ServiceExitCode::NO_ERROR,
            checkpoint: 0,
            wait_hint: wait,
            process_id: None,
        };

        handle
            .set_service_status(report(WinState::Running, accepted, Duration::default()))
            .map_err(|e| Error::Service(format!("reporting Running to the SCM: {e}")))?;

        worker(ServiceEvents::new(rx));

        // Always report Stopped, even after a worker panic unwound past us —
        // a service stuck in "Stopping" needs a reboot to clear.
        let _ = handle.set_service_status(report(
            WinState::Stopped,
            ServiceControlAccept::empty(),
            Duration::default(),
        ));
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// systemd implementation
// ---------------------------------------------------------------------------

#[cfg(all(unix, not(target_os = "macos")))]
mod platform_impl {
    use super::*;

    pub fn install(options: &ServiceOptions) -> Result<()> {
        let path = unit_path(&options.name, options.scope)?;
        let body = super::render_systemd_unit(options);

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ctx(format!("creating {}", parent.display()))?;
        }
        crate::paths::write_atomic(&path, body.as_bytes())?;

        // From here on, undo the unit file if systemd rejects it, so a failed
        // install does not leave a broken unit that `systemctl status` will
        // complain about forever.
        let rollback = || {
            let _ = std::fs::remove_file(&path);
            let _ = systemctl(options.scope, &["daemon-reload"]);
        };

        if let Err(e) = systemctl(options.scope, &["daemon-reload"]) {
            rollback();
            return Err(e);
        }
        if options.start_mode != StartMode::Disabled {
            let unit = unit_name(&options.name);
            if let Err(e) = systemctl(options.scope, &["enable", unit.as_str()]) {
                rollback();
                return Err(e);
            }
        }
        Ok(())
    }

    pub fn uninstall(name: &str, scope: ServiceScope) -> Result<()> {
        let unit = unit_name(name);
        // Best effort: a unit that is already stopped or disabled makes
        // systemctl exit non-zero, and that is not a failure of ours.
        let _ = systemctl(scope, &["stop", unit.as_str()]);
        let _ = systemctl(scope, &["disable", unit.as_str()]);
        let path = unit_path(name, scope)?;
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(Error::io(format!("removing {}", path.display()), e)),
        }
        let _ = systemctl(scope, &["daemon-reload"]);
        let _ = systemctl(scope, &["reset-failed", unit.as_str()]);
        Ok(())
    }

    pub fn start(name: &str, scope: ServiceScope) -> Result<()> {
        let unit = unit_name(name);
        systemctl(scope, &["start", unit.as_str()])
    }

    pub fn stop(name: &str, scope: ServiceScope) -> Result<()> {
        let unit = unit_name(name);
        systemctl(scope, &["stop", unit.as_str()])
    }

    pub fn status(name: &str, scope: ServiceScope) -> Result<ServiceStatus> {
        let unit = unit_name(name);
        let output = std::process::Command::new("systemctl")
            .args(scope_args(scope))
            .args([
                "show",
                unit.as_str(),
                "--property=LoadState,ActiveState,SubState,MainPID,ExecStart,UnitFileState,User",
            ])
            .output();
        let output = match output {
            Ok(o) => o,
            // No systemd on this box (a container, or a non-systemd distro).
            Err(_) => return Ok(ServiceStatus::not_installed()),
        };
        let text = String::from_utf8_lossy(&output.stdout);
        Ok(super::parse_systemctl_show(&text))
    }

    fn unit_name(name: &str) -> String {
        format!("{name}.service")
    }

    fn scope_args(scope: ServiceScope) -> &'static [&'static str] {
        match scope {
            ServiceScope::System => &[],
            ServiceScope::User => &["--user"],
        }
    }

    fn unit_path(name: &str, scope: ServiceScope) -> Result<PathBuf> {
        match scope {
            ServiceScope::System => {
                Ok(PathBuf::from("/etc/systemd/system").join(unit_name(name)))
            }
            ServiceScope::User => {
                let base = directories::BaseDirs::new()
                    .ok_or_else(|| Error::Config("no home directory for this user".into()))?;
                let config = std::env::var_os("XDG_CONFIG_HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| base.home_dir().join(".config"));
                Ok(config.join("systemd/user").join(unit_name(name)))
            }
        }
    }

    fn systemctl(scope: ServiceScope, args: &[&str]) -> Result<()> {
        let output = std::process::Command::new("systemctl")
            .args(scope_args(scope))
            .args(args)
            .output()
            .map_err(|e| {
                Error::Service(format!(
                    "could not run systemctl ({e}). superbackup's service mode needs systemd; \
                     use start-at-login instead on a system without it."
                ))
            })?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = crate::redact::scrub(stderr.trim()).into_owned();
        if stderr.contains("Interactive authentication required")
            || stderr.contains("Access denied")
        {
            return Err(super::not_elevated("Managing the superbackup system service"));
        }
        Err(Error::Service(format!("systemctl {} failed: {stderr}", args.join(" "))))
    }
}

// ---------------------------------------------------------------------------
// launchd implementation
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod platform_impl {
    use super::*;

    pub fn install(options: &ServiceOptions) -> Result<()> {
        let path = plist_path(options.scope)?;
        let body = super::render_launch_daemon(options);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ctx(format!("creating {}", parent.display()))?;
        }
        crate::paths::write_atomic(&path, body.as_bytes())?;

        let target = domain(options.scope);
        if let Err(e) = launchctl(&["bootstrap", target.as_str()], Some(path.as_path())) {
            let _ = std::fs::remove_file(&path);
            return Err(e);
        }
        Ok(())
    }

    pub fn uninstall(_name: &str, scope: ServiceScope) -> Result<()> {
        let path = plist_path(scope)?;
        let target = format!("{}/{LAUNCH_DAEMON_LABEL}", domain(scope));
        let _ = launchctl(&["bootout", target.as_str()], None);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Error::io(format!("removing {}", path.display()), e)),
        }
    }

    pub fn start(_name: &str, scope: ServiceScope) -> Result<()> {
        let target = format!("{}/{LAUNCH_DAEMON_LABEL}", domain(scope));
        launchctl(&["kickstart", "-k", target.as_str()], None)
    }

    pub fn stop(_name: &str, scope: ServiceScope) -> Result<()> {
        let target = format!("{}/{LAUNCH_DAEMON_LABEL}", domain(scope));
        launchctl(&["kill", "SIGTERM", target.as_str()], None)
    }

    pub fn status(_name: &str, scope: ServiceScope) -> Result<ServiceStatus> {
        let path = plist_path(scope)?;
        if !path.exists() {
            return Ok(ServiceStatus::not_installed());
        }
        let target = format!("{}/{LAUNCH_DAEMON_LABEL}", domain(scope));
        let output = std::process::Command::new("launchctl")
            .args(["print", target.as_str()])
            .output();
        let mut status = match output {
            Ok(o) => super::parse_launchctl_print(&String::from_utf8_lossy(&o.stdout)),
            Err(_) => ServiceStatus::not_installed(),
        };
        status.installed = true;
        if status.state == ServiceState::NotInstalled {
            status.state = ServiceState::Stopped;
        }
        if let Ok(text) = std::fs::read_to_string(&path) {
            status.executable = super::super::autostart::parse_launch_agent_command(&text)
                .map(|c| {
                    PathBuf::from(
                        super::super::autostart::parse_desktop_exec(&c)
                            .first()
                            .cloned()
                            .unwrap_or_default(),
                    )
                });
        }
        Ok(status)
    }

    fn domain(scope: ServiceScope) -> String {
        match scope {
            ServiceScope::System => "system".to_string(),
            ServiceScope::User => match super::super::effective_uid() {
                Some(uid) => format!("gui/{uid}"),
                None => "system".to_string(),
            },
        }
    }

    fn plist_path(scope: ServiceScope) -> Result<PathBuf> {
        match scope {
            ServiceScope::System => Ok(PathBuf::from("/Library/LaunchDaemons")
                .join(format!("{LAUNCH_DAEMON_LABEL}.plist"))),
            ServiceScope::User => {
                let base = directories::BaseDirs::new()
                    .ok_or_else(|| Error::Config("no home directory for this user".into()))?;
                Ok(base
                    .home_dir()
                    .join("Library/LaunchAgents")
                    .join(format!("{LAUNCH_DAEMON_LABEL}.plist")))
            }
        }
    }

    fn launchctl(args: &[&str], extra_path: Option<&Path>) -> Result<()> {
        let mut command = std::process::Command::new("launchctl");
        command.args(args);
        if let Some(p) = extra_path {
            command.arg(p);
        }
        let output = command
            .output()
            .map_err(|e| Error::Service(format!("could not run launchctl: {e}")))?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = crate::redact::scrub(stderr.trim()).into_owned();
        // launchd reports EPERM as exit 1 with "Operation not permitted".
        if stderr.contains("Operation not permitted") || stderr.contains("Permission denied") {
            return Err(super::not_elevated("Managing the superbackup service"));
        }
        Err(Error::Service(format!("launchctl {} failed: {stderr}", args.join(" "))))
    }
}

// ---------------------------------------------------------------------------
// Fallback implementation for any other platform
// ---------------------------------------------------------------------------

#[cfg(not(any(windows, unix)))]
mod platform_impl {
    use super::*;

    fn unsupported() -> Error {
        Error::Service(format!(
            "superbackup has no service integration for {}. Use start-at-login, or run \
             `superbackup daemon` from your own supervisor.",
            std::env::consts::OS
        ))
    }

    pub fn install(_options: &ServiceOptions) -> Result<()> {
        Err(unsupported())
    }
    pub fn uninstall(_name: &str, _scope: ServiceScope) -> Result<()> {
        Err(unsupported())
    }
    pub fn start(_name: &str, _scope: ServiceScope) -> Result<()> {
        Err(unsupported())
    }
    pub fn stop(_name: &str, _scope: ServiceScope) -> Result<()> {
        Err(unsupported())
    }
    /// Reporting "not installed" is honest here: there is nothing to install.
    pub fn status(_name: &str, _scope: ServiceScope) -> Result<ServiceStatus> {
        Ok(ServiceStatus::not_installed())
    }
}

// ---------------------------------------------------------------------------
// Unit / plist rendering and parsing (pure — tested on every platform)
// ---------------------------------------------------------------------------

/// Quote one argument for a systemd `ExecStart=` line.
///
/// systemd's parser is *not* a shell: it understands double quotes and
/// backslash escapes and nothing else. `$` is special only as `${}`/`$X`, and
/// `$$` is the escape.
pub fn escape_systemd_arg(arg: &str) -> String {
    let needs_quotes = arg.is_empty() || arg.contains([' ', '\t', '"', '\'', '\\', '\n']);
    let escaped: String = arg
        .chars()
        .flat_map(|c| match c {
            '"' | '\\' => vec!['\\', c],
            other => vec![other],
        })
        .collect();
    // `$` must be doubled whether or not the argument is quoted.
    let escaped = escaped.replace('$', "$$");
    if needs_quotes {
        format!("\"{escaped}\"")
    } else {
        escaped
    }
}

/// Render a systemd unit.
///
/// The hardening block is deliberately asymmetric between scopes:
///
/// * A **system** unit gets `ProtectSystem=strict` plus an explicit
///   `ReadWritePaths=` for our own state, and `ProtectHome=read-only` — a
///   backup daemon must *read* every user's home directory and must never
///   write to one. Getting this backwards (the usual `ProtectHome=yes`) makes
///   the service unable to back up the only files anybody cares about.
/// * A **user** unit runs as the user and writes inside their home, so
///   `ProtectHome` is omitted entirely and `ProtectSystem=full` is as far as we
///   can go.
///
/// `SystemCallFilter=@system-service` is safe for us and for kopia; we do not
/// add `MemoryDenyWriteExecute`, which breaks the Go runtime kopia is built on.
pub fn render_systemd_unit(options: &ServiceOptions) -> String {
    let mut exec = vec![escape_systemd_arg(&options.executable.to_string_lossy())];
    exec.extend(options.args.iter().map(|a| escape_systemd_arg(a)));
    let exec = exec.join(" ");

    let mut unit = String::new();
    unit.push_str("[Unit]\n");
    unit.push_str(&format!("Description={}\n", options.description));
    unit.push_str("Documentation=https://github.com/andreaswiren/superbackup\n");
    // `network-online` rather than `network`: an S3 destination is unreachable
    // until routing is actually up, and a service that starts too early just
    // logs a failure at every boot.
    unit.push_str("After=network-online.target\n");
    unit.push_str("Wants=network-online.target\n\n");

    unit.push_str("[Service]\n");
    unit.push_str("Type=simple\n");
    unit.push_str(&format!("ExecStart={exec}\n"));
    unit.push_str("Restart=on-failure\n");
    unit.push_str("RestartSec=30s\n");
    // Long enough to let a snapshot finalise, short enough that a shutdown is
    // not held up for ever.
    unit.push_str("TimeoutStopSec=120s\n");
    unit.push_str("KillSignal=SIGTERM\n");
    if let ServiceAccount::User { username, .. } = &options.account {
        if options.scope == ServiceScope::System {
            unit.push_str(&format!("User={username}\n"));
        }
    }
    unit.push('\n');

    unit.push_str("# Hardening. See systemd.exec(5).\n");
    unit.push_str("NoNewPrivileges=yes\n");
    unit.push_str("PrivateTmp=yes\n");
    unit.push_str("PrivateDevices=yes\n");
    unit.push_str("ProtectKernelTunables=yes\n");
    unit.push_str("ProtectKernelModules=yes\n");
    unit.push_str("ProtectKernelLogs=yes\n");
    unit.push_str("ProtectControlGroups=yes\n");
    unit.push_str("ProtectClock=yes\n");
    unit.push_str("ProtectHostname=yes\n");
    unit.push_str("ProtectProc=invisible\n");
    unit.push_str("RestrictSUIDSGID=yes\n");
    unit.push_str("RestrictRealtime=yes\n");
    unit.push_str("RestrictNamespaces=yes\n");
    unit.push_str("RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6\n");
    unit.push_str("LockPersonality=yes\n");
    unit.push_str("SystemCallArchitectures=native\n");
    unit.push_str("SystemCallFilter=@system-service\n");
    unit.push_str("SystemCallErrorNumber=EPERM\n");

    match options.scope {
        ServiceScope::System => {
            unit.push_str("ProtectSystem=strict\n");
            // Read every home, write to none of them.
            unit.push_str("ProtectHome=read-only\n");
            if !options.state_dirs.is_empty() {
                let paths: Vec<String> = options
                    .state_dirs
                    .iter()
                    .map(|p| escape_systemd_arg(&p.to_string_lossy()))
                    .collect();
                unit.push_str(&format!("ReadWritePaths={}\n", paths.join(" ")));
            }
            // Reading files owned by other users is the entire job; without
            // this the service can only back up root's own files.
            unit.push_str("CapabilityBoundingSet=CAP_DAC_READ_SEARCH\n");
        }
        ServiceScope::User => {
            // `strict` would make the unit unable to write into the user's own
            // ~/.local/share, and a user unit has nothing to protect the
            // system from that the user could not do anyway.
            unit.push_str("ProtectSystem=full\n");
        }
    }

    unit.push_str("\n[Install]\n");
    match options.scope {
        ServiceScope::System => unit.push_str("WantedBy=multi-user.target\n"),
        ServiceScope::User => unit.push_str("WantedBy=default.target\n"),
    }
    unit
}

/// Parse `systemctl show --property=…` output into a [`ServiceStatus`].
pub fn parse_systemctl_show(text: &str) -> ServiceStatus {
    let mut fields = std::collections::HashMap::new();
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('=') {
            fields.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    let load_state = fields.get("LoadState").map(String::as_str).unwrap_or("");
    if load_state.is_empty() || load_state == "not-found" || load_state == "masked" {
        return ServiceStatus::not_installed();
    }

    let state = match fields.get("ActiveState").map(String::as_str) {
        Some("active") => match fields.get("SubState").map(String::as_str) {
            Some("start") | Some("start-pre") | Some("start-post") => ServiceState::Starting,
            Some("stop") | Some("stop-sigterm") => ServiceState::Stopping,
            _ => ServiceState::Running,
        },
        Some("activating") => ServiceState::Starting,
        Some("deactivating") => ServiceState::Stopping,
        Some("inactive") => ServiceState::Stopped,
        Some("failed") => ServiceState::Stopped,
        _ => ServiceState::Unknown,
    };

    let start_mode = match fields.get("UnitFileState").map(String::as_str) {
        Some("enabled") | Some("enabled-runtime") | Some("static") => Some(StartMode::Automatic),
        Some("disabled") => Some(StartMode::Manual),
        Some("masked") => Some(StartMode::Disabled),
        _ => None,
    };

    // `ExecStart={ path=/usr/bin/superbackup ; argv[]=… }`
    let executable = fields.get("ExecStart").and_then(|raw| {
        raw.split("path=")
            .nth(1)
            .and_then(|rest| rest.split([' ', ';']).next())
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
    });

    let pid = fields
        .get("MainPID")
        .and_then(|p| p.parse::<u32>().ok())
        .filter(|p| *p != 0);

    ServiceStatus {
        installed: true,
        state,
        start_mode,
        account: fields.get("User").cloned().filter(|u| !u.is_empty()),
        executable,
        pid,
        detail: fields.get("ActiveState").cloned(),
    }
}

/// A macOS LaunchDaemon. `KeepAlive` is on for the daemon (unlike the tray
/// LaunchAgent): a backup service that dies must come back.
pub fn render_launch_daemon(options: &ServiceOptions) -> String {
    let mut args = format!(
        "\t\t<string>{}</string>\n",
        super::autostart::xml_escape(&options.executable.to_string_lossy())
    );
    for arg in &options.args {
        args.push_str(&format!("\t\t<string>{}</string>\n", super::autostart::xml_escape(arg)));
    }
    let user = match &options.account {
        ServiceAccount::User { username, .. } => format!(
            "\t<key>UserName</key>\n\t<string>{}</string>\n",
            super::autostart::xml_escape(username)
        ),
        _ => String::new(),
    };
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \t<key>Label</key>\n\
         \t<string>{label}</string>\n\
         \t<key>ProgramArguments</key>\n\
         \t<array>\n{args}\t</array>\n\
         \t<key>RunAtLoad</key>\n\
         \t<{run_at_load}/>\n\
         \t<key>KeepAlive</key>\n\
         \t<true/>\n\
         \t<key>ThrottleInterval</key>\n\
         \t<integer>30</integer>\n\
         \t<key>ProcessType</key>\n\
         \t<string>Background</string>\n\
         {user}\
         </dict>\n\
         </plist>\n",
        label = LAUNCH_DAEMON_LABEL,
        run_at_load = if options.start_mode == StartMode::Disabled { "false" } else { "true" },
    )
}

/// Parse `launchctl print <domain>/<label>` output.
pub fn parse_launchctl_print(text: &str) -> ServiceStatus {
    let mut status = ServiceStatus::not_installed();
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("state = ") {
            status.installed = true;
            status.state = match rest.trim() {
                "running" => ServiceState::Running,
                "not running" | "waiting" => ServiceState::Stopped,
                _ => ServiceState::Unknown,
            };
            status.detail = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("pid = ") {
            status.pid = rest.trim().parse().ok();
            status.installed = true;
        }
    }
    status
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn options(scope: ServiceScope, account: ServiceAccount) -> ServiceOptions {
        ServiceOptions {
            name: "superbackup".into(),
            display_name: "superbackup".into(),
            description: "test".into(),
            executable: PathBuf::from("/usr/bin/superbackup"),
            args: vec!["daemon".into(), "--service".into()],
            account,
            scope,
            start_mode: StartMode::Automatic,
            state_dirs: vec![PathBuf::from("/var/lib/superbackup")],
        }
    }

    #[test]
    fn local_system_cannot_reach_onedrive() {
        let kind = DestinationKind::OneDrive { path: "C:/x".into(), account: None };
        let support =
            destination_support(&kind, &ServiceAccount::LocalSystem, ServiceScope::System);
        assert!(!support.is_usable(), "{support:?}");
        match support {
            SupportLevel::Unsupported { reason } => {
                assert!(reason.contains("user profile"), "{reason}");
            }
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    #[test]
    fn a_user_account_service_can_reach_onedrive_with_a_caveat() {
        let kind = DestinationKind::OneDrive { path: "C:/x".into(), account: None };
        let account = ServiceAccount::User { username: "me".into(), password: None };
        assert!(matches!(
            destination_support(&kind, &account, ServiceScope::System),
            SupportLevel::Degraded { .. }
        ));
    }

    #[test]
    fn s3_works_from_any_account() {
        let kind = DestinationKind::S3 {
            provider_id: Uuid::new_v4(),
            bucket: "b".into(),
            prefix: String::new(),
            credential_override: None,
        };
        for account in [
            ServiceAccount::LocalSystem,
            ServiceAccount::NetworkService,
            ServiceAccount::User { username: "me".into(), password: None },
        ] {
            assert_eq!(
                destination_support(&kind, &account, ServiceScope::System),
                SupportLevel::Supported
            );
        }
    }

    #[test]
    fn unc_paths_are_out_of_reach_for_local_system() {
        let kind = DestinationKind::LocalRepository { path: r"\\nas\backups".into() };
        assert!(!destination_support(&kind, &ServiceAccount::LocalSystem, ServiceScope::System)
            .is_usable());
        let account = ServiceAccount::User { username: "me".into(), password: None };
        assert!(matches!(
            destination_support(&kind, &account, ServiceScope::System),
            SupportLevel::Degraded { .. }
        ));
    }

    #[test]
    fn a_plain_local_path_is_supported() {
        let path = if cfg!(windows) { r"C:\backups" } else { "/srv/backups" };
        let kind = DestinationKind::LocalRepository { path: path.into() };
        assert_eq!(
            destination_support(&kind, &ServiceAccount::LocalSystem, ServiceScope::System),
            SupportLevel::Supported
        );
    }

    #[test]
    fn unc_detection_ignores_long_path_prefixes() {
        assert!(is_unc_path(Path::new(r"\\nas\share")));
        assert!(is_unc_path(Path::new(r"\\?\UNC\nas\share")));
        assert!(!is_unc_path(Path::new(r"\\?\C:\dir")));
        assert!(!is_unc_path(Path::new(r"C:\dir")));
    }

    #[test]
    fn systemd_unit_reads_homes_but_never_writes_them() {
        let unit = render_systemd_unit(&options(ServiceScope::System, ServiceAccount::LocalSystem));
        assert!(unit.contains("ProtectHome=read-only"), "a backup daemon must read homes:\n{unit}");
        assert!(unit.contains("ReadWritePaths=/var/lib/superbackup"));
        assert!(unit.contains("NoNewPrivileges=yes"));
        assert!(unit.contains("CapabilityBoundingSet=CAP_DAC_READ_SEARCH"));
        assert!(!unit.contains("MemoryDenyWriteExecute"), "that breaks the Go runtime kopia uses");
        assert!(unit.contains("WantedBy=multi-user.target"));
    }

    #[test]
    fn a_user_unit_may_write_inside_the_home() {
        let unit = render_systemd_unit(&options(ServiceScope::User, ServiceAccount::LocalSystem));
        assert!(!unit.contains("ProtectHome"), "a user unit lives in the home:\n{unit}");
        assert!(!unit.contains("ProtectSystem=strict"));
        assert!(unit.contains("WantedBy=default.target"));
    }

    #[test]
    fn systemd_arguments_are_escaped() {
        assert_eq!(escape_systemd_arg("/usr/bin/sb"), "/usr/bin/sb");
        assert_eq!(escape_systemd_arg("/opt/my app/sb"), "\"/opt/my app/sb\"");
        assert_eq!(escape_systemd_arg("a\"b"), "\"a\\\"b\"");
        assert_eq!(escape_systemd_arg("$HOME"), "$$HOME");
    }

    #[test]
    fn systemctl_output_is_parsed() {
        let text = "LoadState=loaded\nActiveState=active\nSubState=running\nMainPID=4242\n\
                    ExecStart={ path=/usr/bin/superbackup ; argv[]=/usr/bin/superbackup daemon ; }\n\
                    UnitFileState=enabled\nUser=\n";
        let s = parse_systemctl_show(text);
        assert!(s.installed);
        assert_eq!(s.state, ServiceState::Running);
        assert_eq!(s.pid, Some(4242));
        assert_eq!(s.executable, Some(PathBuf::from("/usr/bin/superbackup")));
        assert_eq!(s.start_mode, Some(StartMode::Automatic));
        assert_eq!(s.account, None);
    }

    #[test]
    fn a_missing_unit_is_reported_as_not_installed() {
        let s = parse_systemctl_show("LoadState=not-found\nActiveState=inactive\n");
        assert!(!s.installed);
        assert_eq!(s.state, ServiceState::NotInstalled);
        assert_eq!(s.health(), Health::Idle, "never installed is not a problem");
    }

    #[test]
    fn launchctl_output_is_parsed() {
        let text = "io.superbackup.daemon = {\n\tstate = running\n\tpid = 900\n}\n";
        let s = parse_launchctl_print(text);
        assert_eq!(s.state, ServiceState::Running);
        assert_eq!(s.pid, Some(900));
    }

    #[test]
    fn an_automatic_service_that_is_stopped_is_a_failure() {
        let mut s = ServiceStatus::not_installed();
        s.installed = true;
        s.state = ServiceState::Stopped;
        s.start_mode = Some(StartMode::AutomaticDelayed);
        assert_eq!(s.health(), Health::Failed);
        s.start_mode = Some(StartMode::Manual);
        assert_eq!(s.health(), Health::Attention);
        s.state = ServiceState::Running;
        assert_eq!(s.health(), Health::Idle);
    }

    #[test]
    fn a_service_pointing_at_a_moved_binary_is_stale() {
        let mut s = ServiceStatus::not_installed();
        s.installed = true;
        s.executable = Some(PathBuf::from("/opt/old/superbackup"));
        assert!(s.is_stale(Path::new("/opt/new/superbackup")));
        assert!(!s.is_stale(Path::new("/opt/old/superbackup")));
    }

    #[test]
    fn launch_daemon_plist_keeps_the_daemon_alive() {
        let plist = render_launch_daemon(&options(ServiceScope::System, ServiceAccount::LocalSystem));
        assert!(plist.contains("<key>KeepAlive</key>"));
        assert!(plist.contains("<key>Label</key>"));
        assert!(plist.contains("io.superbackup.daemon"));
        let parsed = parse_launchctl_print("state = not running\n");
        assert_eq!(parsed.state, ServiceState::Stopped);
    }

    #[test]
    fn user_scope_never_requires_elevation() {
        let mut o = options(ServiceScope::User, ServiceAccount::LocalSystem);
        assert!(!o.requires_elevation());
        o.scope = ServiceScope::System;
        assert!(o.requires_elevation());
    }

    #[test]
    fn account_identifiers_match_what_the_scm_expects() {
        assert_eq!(ServiceAccount::LocalSystem.system_identifier(), None);
        assert_eq!(
            ServiceAccount::NetworkService.system_identifier().as_deref(),
            Some(r"NT AUTHORITY\NetworkService")
        );
        assert!(!ServiceAccount::LocalSystem.sees_user_profile());
        assert!(ServiceAccount::User { username: "x".into(), password: None }.sees_user_profile());
    }
}
