//! Turning kopia's failure modes into something a human can act on.
//!
//! Kopia's stderr is written for an operator sitting at a terminal: it is
//! wrapped Go error chains such as
//!
//! ```text
//! kopia: error: unable to connect to repository: unable to read format blob: BLOB not found
//! ```
//!
//! Dumping that into a desktop notification is how backup products get a
//! reputation for being unusable. Instead every invocation is classified into
//! a [`KopiaFailure`], which carries a sentence the user can act on and a
//! hint telling them what to do next. The raw (already redacted) stderr tail
//! is kept in [`KopiaError::detail`] for the "show details" disclosure and for
//! [`crate::state::RunError::detail`], never as the headline.
//!
//! ## Why the match strings live here and not in kopia
//!
//! Kopia has no stable machine-readable error taxonomy — no exit-code map, no
//! `--json` error envelope. Matching on message substrings is therefore the
//! only option available, and it is a *lossy* one: a kopia release may reword
//! a message and silently demote a failure to [`KopiaFailure::Unknown`]. That
//! degrades gracefully (the user still sees kopia's own text) and every
//! pattern below is annotated with where it comes from in kopia's source, so
//! a future upgrade can re-verify them.

use crate::error::{Error, ErrorCode};
use crate::redact;
use crate::state::RunError;
use chrono::Utc;

/// A classified kopia failure.
///
/// The GUI branches on this to decide what to offer the user: a passphrase
/// prompt, a "create repository" button, a credential editor, or a plain
/// retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KopiaFailure {
    /// The repository passphrase we supplied was rejected.
    WrongPassword,
    /// The storage location is reachable but holds no kopia repository.
    RepositoryNotFound,
    /// `repository create` found data where it wanted an empty location.
    RepositoryExists,
    /// The per-destination config file does not describe a connection yet.
    NotConnected,
    /// The object store rejected our credentials.
    StorageAuth,
    /// The bucket named by the destination does not exist.
    BucketNotFound,
    /// DNS, TCP, TLS or timeout failure talking to the object store.
    StorageUnreachable,
    /// Another process holds the repository, or an upgrade is in progress.
    Locked,
    /// The target volume ran out of space.
    DiskFull,
    /// The OS refused access to a source file or the repository directory.
    PermissionDenied,
    /// We killed kopia because the caller cancelled the run.
    Cancelled,
    /// We killed kopia because it exceeded its time budget.
    Timeout,
    /// The kopia binary is missing, unreadable, or too old.
    Unusable,
    /// Anything we could not classify. The user sees kopia's own words.
    Unknown,
}

impl KopiaFailure {
    /// A complete sentence, safe for a notification, with no jargon and no
    /// stderr fragments.
    pub fn message(&self) -> &'static str {
        match self {
            KopiaFailure::WrongPassword => {
                "The repository passphrase was rejected by kopia."
            }
            KopiaFailure::RepositoryNotFound => {
                "There is no backup repository at this destination yet."
            }
            KopiaFailure::RepositoryExists => {
                "That location already contains data, so a new repository cannot be created there."
            }
            KopiaFailure::NotConnected => {
                "This destination is not connected to its repository."
            }
            KopiaFailure::StorageAuth => {
                "The storage provider rejected the access key for this destination."
            }
            KopiaFailure::BucketNotFound => "The bucket does not exist on this provider.",
            KopiaFailure::StorageUnreachable => {
                "The storage provider could not be reached."
            }
            KopiaFailure::Locked => {
                "The repository is in use by another process and cannot be modified right now."
            }
            KopiaFailure::DiskFull => "The destination ran out of free space.",
            KopiaFailure::PermissionDenied => {
                "Access was denied while reading a source file or writing to the destination."
            }
            KopiaFailure::Cancelled => "The backup was cancelled.",
            KopiaFailure::Timeout => "kopia did not finish within the allowed time and was stopped.",
            KopiaFailure::Unusable => "The kopia executable could not be used.",
            KopiaFailure::Unknown => "kopia reported an error.",
        }
    }

    /// What to do about it. Shown under the message in the GUI and printed
    /// after the error in the CLI.
    pub fn hint(&self) -> Option<&'static str> {
        match self {
            KopiaFailure::WrongPassword => Some(
                "Unlock the vault and check the destination's passphrase. A repository passphrase cannot be recovered, only replaced from a machine that still has it.",
            ),
            KopiaFailure::RepositoryNotFound => {
                Some("Use \"Create repository\" on this destination, or point it at the folder or prefix that already holds one.")
            }
            KopiaFailure::RepositoryExists => Some(
                "Connect to the existing repository instead, or choose an empty folder or key prefix.",
            ),
            KopiaFailure::NotConnected => {
                Some("Run \"Test connection\" on the destination; superbackup will reconnect it.")
            }
            KopiaFailure::StorageAuth => {
                Some("Check the access key and secret on the storage provider, and that the key is allowed to write to this bucket.")
            }
            KopiaFailure::BucketNotFound => {
                Some("Create the bucket at the provider, or correct the bucket name on the destination.")
            }
            KopiaFailure::StorageUnreachable => {
                Some("Check the network connection and the provider's endpoint and region.")
            }
            KopiaFailure::Locked => {
                Some("Wait for the other backup or maintenance run to finish, then try again.")
            }
            KopiaFailure::DiskFull => {
                Some("Free space at the destination, or run maintenance to drop unreferenced data.")
            }
            KopiaFailure::PermissionDenied => {
                Some("Run superbackup as a user that can read the source, or install it as a service.")
            }
            KopiaFailure::Timeout => {
                Some("Raise the job timeout, or split a very large source into several jobs.")
            }
            KopiaFailure::Unusable => {
                Some("Run `superbackup doctor --fix` to download a supported kopia build.")
            }
            KopiaFailure::Cancelled | KopiaFailure::Unknown => None,
        }
    }

    /// The stable error code emitted in `--json` output and over IPC.
    pub fn error_code(&self) -> ErrorCode {
        match self {
            KopiaFailure::WrongPassword => ErrorCode::BadPassphrase,
            KopiaFailure::RepositoryExists => ErrorCode::RepoExists,
            KopiaFailure::NotConnected | KopiaFailure::RepositoryNotFound => {
                ErrorCode::RepoNotConnected
            }
            KopiaFailure::Cancelled => ErrorCode::JobCancelled,
            KopiaFailure::Unusable => ErrorCode::KopiaMissing,
            _ => ErrorCode::Kopia,
        }
    }

    /// Whether trying again unchanged could plausibly succeed. The scheduler
    /// uses this to decide between a retry and a hard stop: retrying a wrong
    /// passphrase forever just burns the user's battery.
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            KopiaFailure::StorageUnreachable | KopiaFailure::Locked | KopiaFailure::Timeout
        )
    }
}

/// Substring patterns, lowercased, in most-specific-first order.
///
/// Provenance of each group is noted so a kopia upgrade can re-verify it:
/// * `invalid repository password` — `repo/format/format_blob.go`.
/// * `repository already initialized` — `repo/format/format_manager.go`.
/// * `found existing data in storage location` — `cli/command_repository_create.go`.
/// * `repository is not connected` / `not connected to a repository` — `cli/config.go`.
/// * `unable to read format blob` + `blob not found` — `repo/format/format_manager.go`
///   and `repo/blob/storage.go`.
/// * `repository upgrade in progress` — `repo/open.go`.
/// * The S3, network, and OS patterns come from the AWS/minio SDK and from Go's
///   `os`/`net` packages rather than from kopia itself, so they are the most
///   likely to drift. They are matched last and failing to match is harmless.
const PATTERNS: &[(&str, KopiaFailure)] = &[
    ("invalid repository password", KopiaFailure::WrongPassword),
    ("found existing data in storage location", KopiaFailure::RepositoryExists),
    ("repository already initialized", KopiaFailure::RepositoryExists),
    ("repository is not connected", KopiaFailure::NotConnected),
    ("not connected to a repository", KopiaFailure::NotConnected),
    ("repository not initialized", KopiaFailure::RepositoryNotFound),
    ("unable to read format blob", KopiaFailure::RepositoryNotFound),
    ("repository upgrade in progress", KopiaFailure::Locked),
    // S3 / object-store credential rejection.
    ("invalidaccesskeyid", KopiaFailure::StorageAuth),
    ("signaturedoesnotmatch", KopiaFailure::StorageAuth),
    ("the request signature we calculated does not match", KopiaFailure::StorageAuth),
    ("invalid access key", KopiaFailure::StorageAuth),
    ("accessdenied", KopiaFailure::StorageAuth),
    ("access denied", KopiaFailure::StorageAuth),
    ("nosuchbucket", KopiaFailure::BucketNotFound),
    ("specified bucket does not exist", KopiaFailure::BucketNotFound),
    // Disk / filesystem.
    ("no space left on device", KopiaFailure::DiskFull),
    ("not enough space on the disk", KopiaFailure::DiskFull),
    ("insufficient disk space", KopiaFailure::DiskFull),
    ("disk quota exceeded", KopiaFailure::DiskFull),
    // Sharing violations look like permission errors but mean "locked".
    ("being used by another process", KopiaFailure::Locked),
    ("the process cannot access the file", KopiaFailure::Locked),
    ("resource temporarily unavailable", KopiaFailure::Locked),
    ("permission denied", KopiaFailure::PermissionDenied),
    ("access is denied", KopiaFailure::PermissionDenied),
    ("operation not permitted", KopiaFailure::PermissionDenied),
    // Network. Checked late: "connection refused" can appear inside a longer
    // credential error, and mis-labelling that as a network fault is worse
    // than the other way round.
    ("no such host", KopiaFailure::StorageUnreachable),
    ("connection refused", KopiaFailure::StorageUnreachable),
    ("connection reset", KopiaFailure::StorageUnreachable),
    ("network is unreachable", KopiaFailure::StorageUnreachable),
    ("i/o timeout", KopiaFailure::StorageUnreachable),
    ("context deadline exceeded", KopiaFailure::StorageUnreachable),
    ("tls handshake", KopiaFailure::StorageUnreachable),
    ("x509:", KopiaFailure::StorageUnreachable),
    ("dial tcp", KopiaFailure::StorageUnreachable),
    // `BLOB not found` on its own is ambiguous (a corrupt repository produces
    // it too), so it is the last repository-shaped pattern we try.
    ("blob not found", KopiaFailure::RepositoryNotFound),
];

/// Classify kopia's output. `text` should be the captured stderr — already
/// redacted, because this result is shown to the user.
pub fn classify(text: &str) -> KopiaFailure {
    let lower = text.to_ascii_lowercase();
    for (needle, failure) in PATTERNS {
        if lower.contains(needle) {
            return *failure;
        }
    }
    KopiaFailure::Unknown
}

/// A kopia invocation that failed, in a shape the GUI, the log, and the run
/// history can all consume without re-deriving anything.
#[derive(Debug, Clone)]
pub struct KopiaError {
    pub failure: KopiaFailure,
    /// The kopia subcommand, e.g. `repository create s3`. Never contains an
    /// argument value, so it is always safe to log.
    pub command: String,
    /// Process exit status, when kopia actually ran and exited.
    pub status: Option<i32>,
    /// The headline sentence. Defaults to [`KopiaFailure::message`] but a
    /// caller may replace it with something more specific.
    pub message: String,
    pub hint: Option<&'static str>,
    /// Redacted tail of kopia's stderr, for the "details" disclosure.
    pub detail: Option<String>,
}

impl KopiaError {
    /// Build from a failed invocation. `stderr` must already be redacted;
    /// it is scrubbed again here because double-scrubbing is idempotent and
    /// forgetting once is not recoverable.
    pub fn from_output(command: impl Into<String>, status: Option<i32>, stderr: &str) -> Self {
        let detail = redact::scrub(stderr).trim().to_string();
        let failure = classify(&detail);
        KopiaError {
            failure,
            command: command.into(),
            status,
            message: failure.message().to_string(),
            hint: failure.hint(),
            detail: if detail.is_empty() { None } else { Some(truncate_tail(&detail, 4096)) },
        }
    }

    /// A failure that did not come from kopia's own output — a spawn error, a
    /// cancellation, a timeout, unparseable output.
    pub fn local(
        command: impl Into<String>,
        failure: KopiaFailure,
        detail: impl Into<Option<String>>,
    ) -> Self {
        KopiaError {
            failure,
            command: command.into(),
            status: None,
            message: failure.message().to_string(),
            hint: failure.hint(),
            detail: detail.into().map(|d| redact::scrub(&d).into_owned()),
        }
    }

    /// Replace the headline with something more specific while keeping the
    /// classification. Used where the driver knows more than the text does —
    /// "the folder C:\backup is not a repository", say.
    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = message.into();
        self
    }

    pub fn is_transient(&self) -> bool {
        self.failure.is_transient()
    }

    /// The record written into [`crate::state::DestinationRun::error`].
    pub fn to_run_error(&self) -> RunError {
        RunError {
            code: self.failure.error_code(),
            message: self.message.clone(),
            hint: self.hint.map(|h| h.to_string()),
            detail: self.detail.clone(),
            occurred_at: Utc::now(),
        }
    }
}

impl std::fmt::Display for KopiaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)?;
        if let Some(d) = &self.detail {
            // One line only: the full detail belongs in the disclosure, not
            // in a log line or a notification body.
            if let Some(first) = d.lines().next_back() {
                write!(f, " ({first})")?;
            }
        }
        Ok(())
    }
}

impl std::error::Error for KopiaError {}

impl From<KopiaError> for Error {
    /// Map onto the crate-wide error type, preferring a specific variant so
    /// that `ErrorCode` stays meaningful for automation, and falling back to
    /// [`Error::Kopia`] with the *actionable* message rather than raw stderr.
    fn from(e: KopiaError) -> Error {
        match e.failure {
            KopiaFailure::WrongPassword => Error::BadPassphrase,
            KopiaFailure::RepositoryExists => Error::RepoExists(e.command.clone()),
            KopiaFailure::NotConnected | KopiaFailure::RepositoryNotFound => {
                Error::RepoNotConnected(e.command.clone())
            }
            KopiaFailure::Cancelled => Error::JobCancelled(e.command.clone()),
            KopiaFailure::Unusable => Error::KopiaMissing,
            _ => Error::Kopia { status: e.status.unwrap_or(-1), stderr: e.to_string() },
        }
    }
}

/// Keep the *end* of a long error: Go error chains put the root cause last.
fn truncate_tail(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let start = s.len() - max;
    // Do not split a UTF-8 sequence.
    let start = (start..s.len()).find(|i| s.is_char_boundary(*i)).unwrap_or(s.len());
    format!("… {}", &s[start..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_failure_has_a_message() {
        for f in [
            KopiaFailure::WrongPassword,
            KopiaFailure::RepositoryNotFound,
            KopiaFailure::RepositoryExists,
            KopiaFailure::NotConnected,
            KopiaFailure::StorageAuth,
            KopiaFailure::BucketNotFound,
            KopiaFailure::StorageUnreachable,
            KopiaFailure::Locked,
            KopiaFailure::DiskFull,
            KopiaFailure::PermissionDenied,
            KopiaFailure::Cancelled,
            KopiaFailure::Timeout,
            KopiaFailure::Unusable,
            KopiaFailure::Unknown,
        ] {
            assert!(!f.message().is_empty(), "{f:?} has no message");
        }
    }

    #[test]
    fn truncate_keeps_the_root_cause() {
        let long = format!("{}root cause here", "x".repeat(9000));
        let t = truncate_tail(&long, 100);
        assert!(t.ends_with("root cause here"));
        assert!(t.len() < 200);
    }
}
