//! Cooperative cancellation.
//!
//! Written by hand rather than pulled from `tokio-util` because that crate is
//! not a dependency of this workspace, and because the engine needs one extra
//! guarantee `CancellationToken` does not give for free: a *reason*, so that
//! "the user pressed Stop" and "the job blew its 6-hour timeout" produce
//! different [`crate::state::RunStatus`] values at the top of the stack.
//!
//! ## Concurrency invariants
//!
//! * A token is a cheap `Arc` handle; clones share one state.
//! * [`CancelToken::child`] creates a token that is cancelled when its parent
//!   is, but whose own cancellation does **not** propagate upwards. This is
//!   what makes "stop job A" leave job B running while "shut down the daemon"
//!   stops both.
//! * [`CancelToken::cancelled`] never misses a wakeup: the flag is published
//!   through a `watch` channel, and the waiter checks the current value before
//!   awaiting a change.
//! * Cancellation is **cooperative**. Nothing here kills a thread or a
//!   process. Every long-running operation in the engine — and every
//!   implementation of [`crate::engine::BackupExecutor`] — is required to poll
//!   the token often enough to react within roughly a second.

use std::sync::Arc;

/// Why a token fired. Carried alongside the flag so the runner can map the
/// cause onto the right terminal status and the right user-facing message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelReason {
    /// A human, the CLI, or the GUI asked for this specific run to stop.
    Requested,
    /// The job exceeded `Job::timeout_minutes`.
    Timeout,
    /// The whole engine is shutting down.
    Shutdown,
}

impl CancelReason {
    /// One line, safe to show in a notification or write to the event log.
    pub fn describe(&self) -> &'static str {
        match self {
            CancelReason::Requested => "stopped by request",
            CancelReason::Timeout => "stopped after exceeding its time limit",
            CancelReason::Shutdown => "stopped because superbackup is shutting down",
        }
    }
}

#[derive(Debug)]
struct TokenInner {
    /// `None` while running. The `watch` sender is kept alive by the `Arc`, so
    /// receivers never see a `RecvError` while any handle survives.
    state: tokio::sync::watch::Sender<Option<CancelReason>>,
    /// Kept so a child stays subscribed to its parent for its whole life; the
    /// spawned propagation task exits when either side is dropped.
    _parent: Option<Arc<TokenInner>>,
}

/// A cancellation handle. Cloning is cheap and shares state.
#[derive(Debug, Clone)]
pub struct CancelToken {
    inner: Arc<TokenInner>,
}

impl Default for CancelToken {
    fn default() -> Self {
        CancelToken::new()
    }
}

impl CancelToken {
    /// A fresh, uncancelled root token.
    pub fn new() -> CancelToken {
        let (state, _) = tokio::sync::watch::channel(None);
        CancelToken { inner: Arc::new(TokenInner { state, _parent: None }) }
    }

    /// A token that fires when `self` fires, but whose own cancellation is
    /// private to it.
    ///
    /// Propagation runs on a detached task holding only `watch` handles, which
    /// costs one idle task per in-flight run and cannot deadlock: it never
    /// takes a lock and never awaits anything but the parent's channel.
    pub fn child(&self) -> CancelToken {
        let (state, _) = tokio::sync::watch::channel(self.reason());
        let child = CancelToken {
            inner: Arc::new(TokenInner { state, _parent: Some(Arc::clone(&self.inner)) }),
        };
        if child.is_cancelled() {
            return child;
        }
        let mut parent_rx = self.inner.state.subscribe();
        let child_state = Arc::clone(&child.inner);
        tokio::spawn(async move {
            loop {
                if let Some(reason) = *parent_rx.borrow_and_update() {
                    child_state.state.send_if_modified(|slot| {
                        if slot.is_none() {
                            *slot = Some(reason);
                            true
                        } else {
                            false
                        }
                    });
                    return;
                }
                if parent_rx.changed().await.is_err() {
                    return;
                }
            }
        });
        child
    }

    /// Fire the token. Idempotent: the first reason wins, so a timeout that
    /// races a user's Stop does not rewrite the recorded cause.
    pub fn cancel(&self, reason: CancelReason) {
        self.inner.state.send_if_modified(|slot| {
            if slot.is_none() {
                *slot = Some(reason);
                true
            } else {
                false
            }
        });
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.state.borrow().is_some()
    }

    /// Why this token fired, or `None` if it has not.
    pub fn reason(&self) -> Option<CancelReason> {
        *self.inner.state.borrow()
    }

    /// Resolve once the token fires. Safe to use in a `select!` arm and safe
    /// to drop and re-create; no wakeup can be lost between the two.
    pub async fn cancelled(&self) -> CancelReason {
        let mut rx = self.inner.state.subscribe();
        loop {
            if let Some(reason) = *rx.borrow_and_update() {
                return reason;
            }
            if rx.changed().await.is_err() {
                // Unreachable while `self` holds the sender alive, but parking
                // is the only correct answer if it ever happens: reporting a
                // cancellation that did not occur would abort a healthy run.
                std::future::pending::<()>().await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn child_sees_parent_cancellation() {
        let parent = CancelToken::new();
        let child = parent.child();
        assert!(!child.is_cancelled());
        parent.cancel(CancelReason::Shutdown);
        assert_eq!(child.cancelled().await, CancelReason::Shutdown);
    }

    #[tokio::test]
    async fn sibling_cancellation_is_isolated() {
        let parent = CancelToken::new();
        let a = parent.child();
        let b = parent.child();
        a.cancel(CancelReason::Requested);
        assert!(a.is_cancelled());
        assert!(!b.is_cancelled(), "cancelling one run must not touch another");
        assert!(!parent.is_cancelled(), "a child must never cancel its parent");
    }

    #[tokio::test]
    async fn first_reason_wins() {
        let t = CancelToken::new();
        t.cancel(CancelReason::Requested);
        t.cancel(CancelReason::Timeout);
        assert_eq!(t.reason(), Some(CancelReason::Requested));
    }

    #[tokio::test]
    async fn child_of_cancelled_parent_starts_cancelled() {
        let parent = CancelToken::new();
        parent.cancel(CancelReason::Shutdown);
        let child = parent.child();
        assert_eq!(child.reason(), Some(CancelReason::Shutdown));
    }
}
