//! Unified error type for superbackup.
//!
//! Every fallible operation in the core returns [`Result<T>`]. Errors are
//! designed to be safe to surface directly in the GUI and in CLI JSON output:
//! they never embed passphrases, repository keys, or S3 secrets. Redaction is
//! enforced at construction time by [`crate::secret::Secret`], which has no
//! `Display`/`Debug` implementation that reveals its contents.

use std::path::PathBuf;

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// A stable, machine-readable code for every error variant.
///
/// The CLI emits this verbatim in `--json` mode so that automation (and AI
/// agents) can branch on failures without parsing English prose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Config,
    Io,
    Locked,
    BadPassphrase,
    VaultCorrupt,
    VaultVersion,
    Crypto,
    Kopia,
    KopiaMissing,
    RepoNotConnected,
    RepoExists,
    Schedule,
    JobNotFound,
    JobRunning,
    JobCancelled,
    Ipc,
    DaemonUnreachable,
    Service,
    Platform,
    Remote,
    Validation,
    Internal,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("configuration error: {0}")]
    Config(String),

    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },

    #[error("path {path} is not usable: {reason}")]
    Path { path: PathBuf, reason: String },

    #[error("the vault is locked; unlock it with your master passphrase first")]
    Locked,

    #[error("incorrect master passphrase")]
    BadPassphrase,

    #[error("vault file is corrupt or has been tampered with: {0}")]
    VaultCorrupt(String),

    #[error(
        "vault format version {found} is not supported (this build understands up to {supported})"
    )]
    VaultVersion { found: u32, supported: u32 },

    #[error("cryptographic operation failed: {0}")]
    Crypto(String),

    #[error("kopia executable was not found; set the path in Settings or install kopia")]
    KopiaMissing,

    #[error("kopia exited with status {status}: {stderr}")]
    Kopia { status: i32, stderr: String },

    #[error("repository {0} is not connected")]
    RepoNotConnected(String),

    #[error("a repository already exists at {0}")]
    RepoExists(String),

    #[error("invalid schedule: {0}")]
    Schedule(String),

    #[error("no job with id or name {0}")]
    JobNotFound(String),

    #[error("job {0} is already running")]
    JobRunning(String),

    #[error("job {0} was cancelled")]
    JobCancelled(String),

    #[error("IPC failure: {0}")]
    Ipc(String),

    #[error("the superbackup daemon is not running (start it with `superbackup daemon`)")]
    DaemonUnreachable,

    #[error("service control failed: {0}")]
    Service(String),

    #[error("platform integration failed: {0}")]
    Platform(String),

    #[error("remote config error: {0}")]
    Remote(String),

    #[error("{0}")]
    Validation(String),

    #[error("internal error: {0}")]
    Internal(String),

    /// An error that arrived over IPC whose code has no local constructor.
    ///
    /// Several variants above carry structured fields (`Io` wraps a
    /// `std::io::Error`, `Kopia` an exit status) that cannot be rebuilt from
    /// the wire. Collapsing those into [`Error::Internal`] silently rewrote the
    /// code as `internal` — and the machine-readable `error.code` is the one
    /// thing the CLI schema tells callers to branch on, so a client saw
    /// `internal` where the daemon had said `kopia`. This variant keeps the
    /// original code and hint intact while accepting that the structured
    /// payload does not survive the trip.
    #[error("{message}")]
    Transported { code: ErrorCode, message: String, hint: Option<String> },
}

impl Error {
    pub fn code(&self) -> ErrorCode {
        match self {
            Error::Config(_) => ErrorCode::Config,
            Error::Io { .. } => ErrorCode::Io,
            Error::Path { .. } => ErrorCode::Io,
            Error::Locked => ErrorCode::Locked,
            Error::BadPassphrase => ErrorCode::BadPassphrase,
            Error::VaultCorrupt(_) => ErrorCode::VaultCorrupt,
            Error::VaultVersion { .. } => ErrorCode::VaultVersion,
            Error::Crypto(_) => ErrorCode::Crypto,
            Error::KopiaMissing => ErrorCode::KopiaMissing,
            Error::Kopia { .. } => ErrorCode::Kopia,
            Error::RepoNotConnected(_) => ErrorCode::RepoNotConnected,
            Error::RepoExists(_) => ErrorCode::RepoExists,
            Error::Schedule(_) => ErrorCode::Schedule,
            Error::JobNotFound(_) => ErrorCode::JobNotFound,
            Error::JobRunning(_) => ErrorCode::JobRunning,
            Error::JobCancelled(_) => ErrorCode::JobCancelled,
            Error::Ipc(_) => ErrorCode::Ipc,
            Error::DaemonUnreachable => ErrorCode::DaemonUnreachable,
            Error::Service(_) => ErrorCode::Service,
            Error::Platform(_) => ErrorCode::Platform,
            Error::Remote(_) => ErrorCode::Remote,
            Error::Validation(_) => ErrorCode::Validation,
            Error::Internal(_) => ErrorCode::Internal,
            Error::Transported { code, .. } => *code,
        }
    }

    /// A short, actionable hint shown under the error in the GUI and printed
    /// after the message in the CLI. `None` when the message already says
    /// everything the user needs.
    pub fn hint(&self) -> Option<&'static str> {
        match self {
            Error::Locked => Some("Open superbackup and unlock, or run `superbackup unlock`."),
            Error::BadPassphrase => {
                Some("Passphrases are case sensitive. There is no recovery if it is lost.")
            }
            Error::KopiaMissing => {
                Some("Run `superbackup doctor --fix` to download a pinned kopia build.")
            }
            Error::DaemonUnreachable => {
                Some("Run `superbackup service status`, or start the tray app.")
            }
            Error::VaultCorrupt(_) => {
                Some("Restore config.sbvault from a backup; do not overwrite it.")
            }
            // A transported hint is an owned String, so it cannot be returned
            // from this `&'static str` API; `hint_owned` below serves it.
            _ => None,
        }
    }

    /// The hint to show, including one that arrived over IPC.
    ///
    /// Prefer this over [`Error::hint`] anywhere a daemon reply might be the
    /// source, otherwise a remote hint is silently dropped on the floor.
    pub fn hint_owned(&self) -> Option<String> {
        match self {
            Error::Transported { hint, .. } => hint.clone(),
            other => other.hint().map(str::to_owned),
        }
    }

    pub fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Error::Io { context: context.into(), source }
    }
}

/// Convenience for adding context to `std::io` results without pulling in
/// `anyhow` at every call site.
pub trait IoContext<T> {
    fn ctx(self, context: impl Into<String>) -> Result<T>;
}

impl<T> IoContext<T> for std::result::Result<T, std::io::Error> {
    fn ctx(self, context: impl Into<String>) -> Result<T> {
        self.map_err(|e| Error::io(context, e))
    }
}
