//! Installing kopia from inside the window, while the window keeps drawing.
//!
//! # Why this is not the daemon's job
//!
//! Normally it is. `daemon::spawn_kopia_setup` calls the same installer at
//! start-up, and on every launch after the first that is what fetches kopia.
//!
//! First run is the exception, and it is the exception that mattered: there is
//! no daemon during onboarding, because the daemon will not start until a
//! vault exists and the wizard is what creates one. So the one moment a user
//! is definitely watching — the setup screen that says kopia is missing — was
//! the one moment nothing was fetching it. The button offered instead opened
//! kopia's releases page in a browser and left the user to install a program
//! by hand, in the middle of setting up the program that needs it.
//!
//! # Why a thread rather than the window's own worker
//!
//! The window's worker exists to talk to a daemon over a pipe. This has no
//! daemon to talk to. It is a download that takes tens of seconds, so it needs
//! somewhere to run that is not the frame, and a thread with a small runtime of
//! its own is the whole of what that requires.
//!
//! Progress arrives through a plain channel, drained once per frame. Nothing
//! here blocks the interface, and dropping the handle does not cancel the
//! install — a half-extracted kopia is worse than a finished one nobody is
//! watching, and the installer writes atomically at the end either way.

use std::sync::mpsc::{Receiver, TryRecvError};

use superbackup_core::kopia::install::{InstallProgressSink, KopiaInstaller};
use superbackup_core::model::Settings;
use superbackup_core::paths::Paths;

/// What the background thread sends back.
enum Update {
    /// A phase title, and how far through it is when that is known.
    Step(String, Option<f32>),
    /// The version that is now installed, or why it is not.
    Done(Result<String, String>),
}

/// An install running behind the wizard.
#[derive(Debug)]
pub struct Install {
    updates: Receiver<Update>,
    /// What it is doing, in the installer's own words.
    pub line: String,
    /// `0.0`–`1.0` during the download, `None` for the phases with no size.
    pub fraction: Option<f32>,
    /// `Some` once it has stopped: the version, or the reason it failed.
    pub finished: Option<Result<String, String>>,
}

impl std::fmt::Debug for Update {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Update::Step(title, fraction) => write!(f, "Step({title:?}, {fraction:?})"),
            Update::Done(outcome) => write!(f, "Done({outcome:?})"),
        }
    }
}

impl Install {
    /// Start fetching the latest kopia superbackup supports.
    ///
    /// Which version that is comes from the installer, not from here: it
    /// resolves the newest release the running build is known to work with,
    /// honouring a pinned version when the settings name one.
    pub fn start(paths: Paths, settings: Settings) -> Install {
        let (tx, updates) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(runtime) => runtime,
                Err(e) => {
                    let _ = tx.send(Update::Done(Err(format!("kopia could not be fetched: {e}"))));
                    return;
                }
            };

            runtime.block_on(async move {
                let installer = match KopiaInstaller::new(&paths) {
                    Ok(installer) => installer,
                    Err(e) => {
                        let _ = tx.send(Update::Done(Err(e.message())));
                        return;
                    }
                };

                let (sink, mut progress) = InstallProgressSink::channel(16);
                let steps = tx.clone();
                let pump = tokio::spawn(async move {
                    while let Some(update) = progress.recv().await {
                        let _ = steps.send(Update::Step(
                            update.phase.title().to_string(),
                            update.fraction(),
                        ));
                    }
                });

                let outcome = installer.ensure_available(&settings, &paths, Some(&sink)).await;
                // The pump ends when the last sender goes, and this is it.
                drop(sink);
                let _ = pump.await;

                let _ = tx.send(Update::Done(
                    outcome.map(|binary| binary.version().to_string()).map_err(|e| e.message()),
                ));
            });
        });

        Install {
            updates,
            line: "Looking up the latest kopia release".to_string(),
            fraction: None,
            finished: None,
        }
    }

    /// Take whatever has happened since the last frame.
    ///
    /// Returns `true` when this call is the one that saw it finish, so the
    /// caller can react once rather than every frame afterwards.
    pub fn poll(&mut self) -> bool {
        let mut just_finished = false;
        loop {
            match self.updates.try_recv() {
                Ok(Update::Step(title, fraction)) => {
                    self.line = title;
                    self.fraction = fraction;
                }
                Ok(Update::Done(outcome)) => {
                    self.finished = Some(outcome);
                    self.fraction = None;
                    just_finished = true;
                }
                Err(TryRecvError::Empty) => break,
                // The thread is gone. If it went without saying how it ended,
                // that is itself an ending: leaving the spinner turning for
                // ever is the failure mode this whole file is here to avoid.
                Err(TryRecvError::Disconnected) => {
                    if self.finished.is_none() {
                        self.finished =
                            Some(Err("The kopia download stopped unexpectedly.".to_string()));
                        just_finished = true;
                    }
                    break;
                }
            }
        }
        just_finished
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A handle whose thread has died must not spin for ever.
    ///
    /// Built by hand rather than by starting a real install: this asserts what
    /// happens when the channel closes, which is the case a download that
    /// panics or is killed produces and which no test could arrange by
    /// actually downloading something.
    #[test]
    fn a_download_that_disappears_still_ends() {
        let (tx, updates) = std::sync::mpsc::channel();
        let mut install = Install { updates, line: String::new(), fraction: None, finished: None };
        assert!(install.finished.is_none());
        assert!(!install.poll(), "nothing has happened yet");

        drop(tx);
        assert!(install.poll(), "the ending must be reported once");
        assert!(install.finished.expect("an outcome").is_err());
    }

    /// And the ending is reported once, not on every frame after it.
    #[test]
    fn a_finished_install_reports_itself_once() {
        let (tx, updates) = std::sync::mpsc::channel();
        let mut install = Install { updates, line: String::new(), fraction: None, finished: None };
        tx.send(Update::Done(Ok("0.21.1".to_string()))).expect("send");

        assert!(install.poll());
        assert!(!install.poll(), "a second frame must not fire it again");
        assert_eq!(install.finished, Some(Ok("0.21.1".to_string())));
    }

    /// Progress moves the line and the bar.
    #[test]
    fn progress_is_shown_as_it_arrives() {
        let (tx, updates) = std::sync::mpsc::channel();
        let mut install = Install { updates, line: String::new(), fraction: None, finished: None };
        tx.send(Update::Step("Downloading kopia".to_string(), Some(0.5))).expect("send");

        assert!(!install.poll(), "progress is not an ending");
        assert_eq!(install.line, "Downloading kopia");
        assert_eq!(install.fraction, Some(0.5));
        assert!(install.finished.is_none(), "progress must not look like an ending");
    }
}
