//! Talking to an authenticator without freezing the window.
//!
//! Both operations put the operating system's own prompt on screen and then
//! wait for a person to find their security key, plug it in, and type a PIN —
//! up to a minute, by design. Doing that on the frame would stop the
//! application repainting, which on Windows means the prompt appears over a
//! window that has gone white and stopped responding.
//!
//! So the call runs on a thread and the answer arrives through a channel,
//! drained once per frame. Same shape as [`crate::gui::kopia`], for the same
//! reason.
//!
//! # Why the window process and not the daemon
//!
//! `WebAuthNAuthenticatorGetAssertion` takes a window handle and shows a
//! dialog. A service has no desktop to show one on, so the daemon cannot do
//! this — the same constraint that sends an `ssh` passphrase to a terminal
//! rather than asking the daemon to type it.
//!
//! It works out neatly. The window recovers the master passphrase from the
//! sealed file itself and then calls the ordinary `vault.unlock`, so unlocking
//! with a passkey needs no new command, no new daemon state, and nothing new
//! on the wire. The daemon never learns that a passkey was involved, which is
//! correct: what it is handed is a passphrase somebody proved they were
//! entitled to.

use std::sync::mpsc::{Receiver, TryRecvError};

use superbackup_core::credentials::passkey::{self, Enrolled};
use superbackup_core::paths::Paths;
use superbackup_core::platform::webauthn::Window;
use superbackup_core::secret::Secret;

/// What the thread sends back when it is done.
pub enum Outcome {
    /// The passphrase, recovered from the sealed file. Goes straight to
    /// `vault.unlock` and is not kept.
    Unlocked(Secret),
    /// A passkey was enrolled.
    Enrolled(Box<Enrolled>),
    /// It did not work, in words for the user. A refused prompt and a missing
    /// authenticator both land here, and both are ordinary.
    Failed(String),
}

impl std::fmt::Debug for Outcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // Never the passphrase, not even in a debug line.
            Outcome::Unlocked(_) => f.write_str("Unlocked(<secret>)"),
            Outcome::Enrolled(entry) => write!(f, "Enrolled({:?})", entry.label),
            Outcome::Failed(why) => write!(f, "Failed({why:?})"),
        }
    }
}

/// An authenticator operation in flight.
#[derive(Debug)]
pub struct Pending {
    outcome: Receiver<Outcome>,
    /// What to say while the prompt is up.
    pub line: &'static str,
}

/// Shown while the operating system's prompt is waiting.
const UNLOCKING: &str = "Follow the prompt to use your passkey";
const ENROLLING: &str = "Follow the prompt to set up your passkey";

impl Pending {
    /// Recover the master passphrase using an enrolled passkey.
    pub fn unlock(paths: Paths, entry: Enrolled) -> Pending {
        Pending::spawn(UNLOCKING, move |window| {
            passkey::unlock_with(&paths, window, &entry)
                .map(Outcome::Unlocked)
                .unwrap_or_else(|e| Outcome::Failed(e.to_string()))
        })
    }

    /// Enrol one, sealing `passphrase` under the key it derives.
    pub fn enrol(paths: Paths, label: String, passphrase: Secret) -> Pending {
        Pending::spawn(ENROLLING, move |window| {
            passkey::enrol(&paths, window, &label, &passphrase)
                .map(|entry| Outcome::Enrolled(Box::new(entry)))
                .unwrap_or_else(|e| Outcome::Failed(e.to_string()))
        })
    }

    fn spawn(line: &'static str, work: impl FnOnce(Window) -> Outcome + Send + 'static) -> Pending {
        let (tx, outcome) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            // The foreground window, taken on the worker: this runs because
            // the user just clicked a button in our own window, so it is ours.
            // Asking the platform rather than plumbing a handle out of eframe
            // keeps the raw handle out of the rest of the application.
            let _ = tx.send(work(Window::foreground()));
        });
        Pending { outcome, line }
    }

    /// Whatever has happened since the last frame, if anything.
    ///
    /// A thread that dies without answering is reported as a failure rather
    /// than left pending, because a spinner that never stops is the one
    /// outcome nobody can act on.
    pub fn poll(&mut self) -> Option<Outcome> {
        match self.outcome.try_recv() {
            Ok(outcome) => Some(outcome),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                Some(Outcome::Failed("The passkey prompt stopped unexpectedly.".to_string()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A thread that dies without answering must still end the wait.
    #[test]
    fn a_prompt_that_disappears_is_reported_rather_than_hung() {
        let (tx, outcome) = std::sync::mpsc::channel();
        let mut pending = Pending { outcome, line: UNLOCKING };
        assert!(pending.poll().is_none(), "nothing has happened yet");

        drop(tx);
        let outcome = pending.poll().expect("the ending must be reported");
        assert!(matches!(outcome, Outcome::Failed(_)), "{outcome:?}");
    }

    /// The passphrase must not reach a log line, a panic message, or anywhere
    /// else `Debug` goes.
    #[test]
    fn the_recovered_passphrase_is_not_printable() {
        let outcome = Outcome::Unlocked(Secret::from_str("correct horse battery staple"));
        let rendered = format!("{outcome:?}");
        assert!(!rendered.contains("correct horse"), "{rendered}");
        assert!(rendered.contains("secret"), "{rendered}");
    }
}
