//! Deciding what is worth trying again.
//!
//! Retrying is not free and it is not always safe. Retrying a 503 from S3 five
//! seconds later usually works; retrying a wrong repository passphrase does
//! exactly nothing except delay the failure notification the user needs, and
//! against some providers it counts towards an authentication lockout.
//!
//! So the engine retries only failures it believes are *transient*: network,
//! rate limiting, lock contention, transient storage errors. Everything else —
//! bad passphrase, missing path, invalid configuration, cancellation — fails
//! immediately.
//!
//! The driver gets the first word ([`Retryable`], set by the implementation
//! that actually saw the error), and only when it declines to classify does the
//! heuristic here look at the error code and message.

use crate::engine::executor::{ExecutorError, Retryable};
use crate::error::ErrorCode;
use chrono::Duration;

/// Bounded exponential backoff.
///
/// There is deliberately **no jitter**. Jitter exists to de-synchronise many
/// clients hammering one server; superbackup is one process on one machine
/// talking to the user's own bucket, so jitter would buy nothing and would
/// make the backoff untestable without a seeded RNG.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RetryPolicy {
    /// Total attempts including the first. `1` disables retrying.
    pub max_attempts: u32,
    pub initial_delay: Duration,
    pub max_delay: Duration,
    pub multiplier: f64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        // 5s, 20s, then give up: about half a minute of patience, which
        // covers a router reconnecting or a provider's brief 5xx, without
        // making a genuinely broken destination take minutes to report.
        RetryPolicy {
            max_attempts: 3,
            initial_delay: Duration::seconds(5),
            max_delay: Duration::seconds(120),
            multiplier: 4.0,
        }
    }
}

impl RetryPolicy {
    /// No retrying at all. Used by tests that assert a single attempt.
    pub fn none() -> RetryPolicy {
        RetryPolicy { max_attempts: 1, ..RetryPolicy::default() }
    }

    /// How long to wait before attempt `attempt + 1`, where `attempt` is
    /// 1-based and is the attempt that just failed.
    pub fn delay_after(&self, attempt: u32) -> Duration {
        let exponent = attempt.saturating_sub(1).min(16) as i32;
        let scaled = self.initial_delay.num_milliseconds() as f64
            * self.multiplier.max(1.0).powi(exponent);
        let capped = scaled.min(self.max_delay.num_milliseconds() as f64).max(0.0);
        Duration::milliseconds(capped as i64)
    }

    /// Should a failure on `attempt` be retried?
    pub fn should_retry(&self, attempt: u32, error: &ExecutorError) -> bool {
        attempt < self.max_attempts && classify(error) == Retryable::Transient
    }
}

/// Resolve a failure to a definite classification.
///
/// [`Retryable::Unknown`] is resolved by the heuristic below; the driver's own
/// verdict always wins when it has one.
pub fn classify(error: &ExecutorError) -> Retryable {
    match error.retryable {
        Retryable::Transient => Retryable::Transient,
        Retryable::Permanent => Retryable::Permanent,
        Retryable::Unknown => classify_heuristically(error),
    }
}

/// Substrings that mark a failure as worth another attempt. Matched
/// case-insensitively against the message and the captured detail.
const TRANSIENT_MARKERS: &[&str] = &[
    "timeout",
    "timed out",
    "i/o timeout",
    "context deadline exceeded",
    "connection reset",
    "connection refused",
    "connection closed",
    "broken pipe",
    "temporarily unavailable",
    "try again",
    "unexpected eof",
    "no route to host",
    "network is unreachable",
    "network error",
    "dns",
    "tls handshake",
    "handshake failure",
    "throttl",
    "slow down",
    "rate limit",
    "too many requests",
    "serviceunavailable",
    "service unavailable",
    "internal error",
    "500 internal",
    "502 ",
    "503 ",
    "504 ",
    "429 ",
    "requesttimeout",
    "operationaborted",
    "lock is held",
    "resource busy",
];

/// Substrings that mark a failure as hopeless, checked first so that a message
/// containing both (`"connection refused: invalid credentials"`) is treated as
/// the more specific of the two.
const PERMANENT_MARKERS: &[&str] = &[
    "invalid password",
    "incorrect password",
    "invalid passphrase",
    "incorrect passphrase",
    "access denied",
    "accessdenied",
    "invalidaccesskeyid",
    "signaturedoesnotmatch",
    "no such bucket",
    "nosuchbucket",
    "no such file",
    "cannot find the file",
    "cannot find the path",
    "not a directory",
    "permission denied",
    "unsupported",
    "already exists",
];

fn classify_heuristically(error: &ExecutorError) -> Retryable {
    match error.code {
        // Structurally impossible to fix by waiting.
        ErrorCode::BadPassphrase
        | ErrorCode::VaultCorrupt
        | ErrorCode::VaultVersion
        | ErrorCode::Locked
        | ErrorCode::Crypto
        | ErrorCode::KopiaMissing
        | ErrorCode::RepoExists
        | ErrorCode::Validation
        | ErrorCode::Config
        | ErrorCode::Schedule
        | ErrorCode::JobNotFound
        | ErrorCode::JobRunning
        | ErrorCode::JobCancelled
        | ErrorCode::Platform
        | ErrorCode::Internal => Retryable::Permanent,
        // Structurally worth retrying: these only exist because something
        // outside this process was momentarily unavailable.
        ErrorCode::Remote | ErrorCode::DaemonUnreachable | ErrorCode::Ipc | ErrorCode::Service => {
            Retryable::Transient
        }
        // Ambiguous: a filesystem or driver error may be either. Read the text.
        ErrorCode::Io | ErrorCode::Kopia | ErrorCode::RepoNotConnected => {
            let mut haystack = error.message.to_ascii_lowercase();
            if let Some(detail) = &error.detail {
                haystack.push(' ');
                haystack.push_str(&detail.to_ascii_lowercase());
            }
            if PERMANENT_MARKERS.iter().any(|m| haystack.contains(m)) {
                Retryable::Permanent
            } else if TRANSIENT_MARKERS.iter().any(|m| haystack.contains(m)) {
                Retryable::Transient
            } else {
                // Default to *not* retrying an error nobody recognises. An
                // unrecognised failure that repeats is noise; an unrecognised
                // failure reported once is a bug report.
                Retryable::Permanent
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(code: ErrorCode, message: &str) -> ExecutorError {
        ExecutorError::new(code, message)
    }

    #[test]
    fn driver_classification_wins() {
        let e = err(ErrorCode::BadPassphrase, "nope").transient();
        assert_eq!(classify(&e), Retryable::Transient);
        let e = err(ErrorCode::Io, "connection reset by peer").permanent();
        assert_eq!(classify(&e), Retryable::Permanent);
    }

    #[test]
    fn network_failures_are_retried() {
        for message in [
            "connection reset by peer",
            "dial tcp: i/o timeout",
            "S3 returned 503 Slow Down",
            "tls handshake failure",
            "429 Too Many Requests",
        ] {
            assert_eq!(
                classify(&err(ErrorCode::Kopia, message)),
                Retryable::Transient,
                "{message} should be retried"
            );
        }
    }

    #[test]
    fn deterministic_failures_are_not_retried() {
        assert_eq!(classify(&err(ErrorCode::BadPassphrase, "wrong")), Retryable::Permanent);
        assert_eq!(
            classify(&err(ErrorCode::Io, "The system cannot find the path specified")),
            Retryable::Permanent
        );
        assert_eq!(
            classify(&err(ErrorCode::Kopia, "invalid password for repository")),
            Retryable::Permanent
        );
        assert_eq!(classify(&ExecutorError::cancelled()), Retryable::Permanent);
    }

    #[test]
    fn a_specific_permanent_marker_beats_a_generic_transient_one() {
        let e = err(ErrorCode::Kopia, "connection refused: invalid password");
        assert_eq!(classify(&e), Retryable::Permanent);
    }

    #[test]
    fn unrecognised_failures_are_not_retried() {
        assert_eq!(classify(&err(ErrorCode::Kopia, "something odd happened")), Retryable::Permanent);
    }

    #[test]
    fn backoff_grows_and_is_capped() {
        let p = RetryPolicy::default();
        assert_eq!(p.delay_after(1), Duration::seconds(5));
        assert_eq!(p.delay_after(2), Duration::seconds(20));
        assert_eq!(p.delay_after(3), Duration::seconds(80));
        assert_eq!(p.delay_after(4), Duration::seconds(120), "capped at max_delay");
        assert_eq!(p.delay_after(50), Duration::seconds(120));
    }

    #[test]
    fn attempts_are_bounded() {
        let p = RetryPolicy::default();
        let transient = err(ErrorCode::Kopia, "connection reset");
        assert!(p.should_retry(1, &transient));
        assert!(p.should_retry(2, &transient));
        assert!(!p.should_retry(3, &transient), "max_attempts is a hard bound");
        assert!(!RetryPolicy::none().should_retry(1, &transient));
    }
}
