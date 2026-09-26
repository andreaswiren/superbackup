//! Whether this is a moment to interrupt somebody.
//!
//! # Why a backup tool cares
//!
//! Because the things it has to say are important and not urgent. A failed
//! backup matters; it does not matter in the next four seconds, and it never
//! matters more than the thing the person is doing. A toast over a fullscreen
//! game, a presentation on a projector, or a focus session is the behaviour
//! that gets an application's notifications switched off entirely — after
//! which it can no longer tell anybody that their backups have stopped, which
//! is the one message it exists to deliver.
//!
//! # Held, not dropped
//!
//! Nothing here decides to *discard* a notification. It decides whether now is
//! the moment. A caller that gets [`Attention::Busy`] is expected to keep the
//! message and raise it when the state clears — see
//! [`crate::platform::notify::Notifier`], which does exactly that. Dropping it
//! would mean a failure during a three-hour game is a failure nobody is ever
//! told about, which is worse than interrupting.
//!
//! # Platforms
//!
//! **Windows** answers properly, through `SHQueryUserNotificationState`: one
//! call that covers fullscreen Direct3D (games), presentation mode, a
//! fullscreen store app, and Focus Assist / Do Not Disturb quiet hours.
//!
//! **macOS** holds notifications itself while a Focus is on and releases them
//! afterwards, which is the same policy implemented one layer down; asking is
//! neither necessary nor reliably possible, so this reports
//! [`Attention::Unknown`] and lets the system do it.
//!
//! **Linux** has no standard for this. Some desktops expose an inhibition
//! interface, most do not, and guessing wrong in either direction is worse
//! than the honest answer.

/// Whether a notification should be raised now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attention {
    /// Notifications are welcome.
    Available,
    /// The person is doing something a toast would interrupt.
    Busy(Reason),
    /// This platform cannot say. Treated as available: a backup tool that
    /// stays silent because it could not ask is a backup tool that never
    /// reports a failure.
    Unknown,
}

impl Attention {
    /// Should a notification be raised right now?
    pub fn accepts_notifications(&self) -> bool {
        !matches!(self, Attention::Busy(_))
    }

    /// Why it is being held, for the log and for the "held" badge.
    pub fn reason(&self) -> Option<Reason> {
        match self {
            Attention::Busy(reason) => Some(*reason),
            _ => None,
        }
    }
}

/// What is going on, in terms a person would recognise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// A fullscreen Direct3D application — a game, nearly always.
    FullScreenGame,
    /// Presenting: a projector, a screen share, or presentation mode.
    Presenting,
    /// Focus Assist, Do Not Disturb, or scheduled quiet hours.
    QuietHours,
    /// Some other fullscreen application that asked not to be covered.
    FullScreenApp,
}

impl Reason {
    /// One clause, for "held because …".
    pub fn describe(self) -> &'static str {
        match self {
            Reason::FullScreenGame => "a full-screen game is running",
            Reason::Presenting => "this machine is presenting",
            Reason::QuietHours => "notifications are silenced",
            Reason::FullScreenApp => "a full-screen application is running",
        }
    }
}

/// Ask the platform whether now is a good moment.
///
/// Cheap enough to call before every notification: on Windows it is one shell
/// call that reads a cached state, and everywhere else it is a constant.
pub fn attention() -> Attention {
    platform_impl::attention()
}

#[cfg(windows)]
mod platform_impl {
    use super::{Attention, Reason};

    pub fn attention() -> Attention {
        use windows::Win32::UI::Shell::{
            SHQueryUserNotificationState, QUNS_ACCEPTS_NOTIFICATIONS, QUNS_APP, QUNS_BUSY,
            QUNS_NOT_PRESENT, QUNS_PRESENTATION_MODE, QUNS_QUIET_TIME,
            QUNS_RUNNING_D3D_FULL_SCREEN,
        };

        // SAFETY: no arguments, no buffers; it returns an enum by value.
        let state = match unsafe { SHQueryUserNotificationState() } {
            Ok(state) => state,
            // The call is documented to fail only in a session with no shell.
            // There is nobody to interrupt there, and nothing to gain from
            // guessing, so it is treated as unknown and the message goes out —
            // to be queued by Windows if it must be.
            Err(_) => return Attention::Unknown,
        };

        match state {
            QUNS_RUNNING_D3D_FULL_SCREEN => Attention::Busy(Reason::FullScreenGame),
            QUNS_PRESENTATION_MODE => Attention::Busy(Reason::Presenting),
            // "Busy" is what Windows reports for a full-screen application
            // that has asked not to be interrupted, which is the same request
            // presentation mode makes in a different way.
            QUNS_BUSY => Attention::Busy(Reason::Presenting),
            QUNS_APP => Attention::Busy(Reason::FullScreenApp),
            QUNS_QUIET_TIME => Attention::Busy(Reason::QuietHours),
            // The screen is locked or the screensaver is on. Not a reason to
            // hold anything: Windows queues the toast and shows it when they
            // come back, which is exactly the behaviour wanted.
            QUNS_NOT_PRESENT | QUNS_ACCEPTS_NOTIFICATIONS => Attention::Available,
            _ => Attention::Unknown,
        }
    }
}

#[cfg(not(windows))]
mod platform_impl {
    use super::Attention;

    /// macOS holds notifications itself while a Focus is on, and Linux has no
    /// standard to ask. See the module documentation.
    pub fn attention() -> Attention {
        Attention::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Not being able to ask must never mean staying silent.
    ///
    /// A backup tool whose one job is to say "your backups have stopped"
    /// cannot answer "I could not tell whether you were busy" by saying
    /// nothing at all.
    #[test]
    fn an_unknown_state_still_lets_the_message_through() {
        assert!(Attention::Unknown.accepts_notifications());
        assert_eq!(Attention::Unknown.reason(), None);
    }

    /// Every busy state is a hold, and every one of them can say why.
    #[test]
    fn every_reason_to_stay_quiet_can_explain_itself() {
        for reason in
            [Reason::FullScreenGame, Reason::Presenting, Reason::QuietHours, Reason::FullScreenApp]
        {
            let busy = Attention::Busy(reason);
            assert!(!busy.accepts_notifications(), "{reason:?} must hold notifications");
            assert_eq!(busy.reason(), Some(reason));
            assert!(!reason.describe().is_empty(), "{reason:?} has nothing to say");
        }
        assert!(Attention::Available.accepts_notifications());
    }

    /// Whatever this machine is doing, asking does not panic or hang.
    #[test]
    fn asking_the_real_platform_answers() {
        let answer = attention();
        // Only that it returned something coherent: the developer running this
        // may well be in Do Not Disturb, and a test asserting otherwise would
        // fail for the most ordinary reason there is.
        assert_eq!(answer.reason().is_some(), !answer.accepts_notifications());
    }
}
