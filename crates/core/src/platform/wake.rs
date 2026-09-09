//! Waking the machine for a scheduled backup, and letting it sleep again.
//!
//! # The problem
//!
//! A desktop that sleeps at 23:00 never runs the 02:00 backup. The scheduler
//! is correct, the job is enabled, the destination is fine, and nothing
//! happens — and the catch-up logic then runs it at 08:40 the next morning
//! while the user is trying to work. On a laptop that is closed every evening
//! the nightly backup effectively never runs at all.
//!
//! # The two halves
//!
//! Waking is not one problem but two, and doing only the first produces a
//! machine that wakes up, sits at a black screen for two minutes, and goes
//! back to sleep in the middle of the backup:
//!
//! 1. **[`WakeTimer`]** — arm the platform's wake alarm for the next run.
//! 2. **[`StayAwake`]** — hold the machine awake while a run is in progress.
//!    Windows puts a machine woken by a timer back to sleep on its ordinary
//!    idle timer, and a timer wake counts as no user activity at all.
//!
//! Going back to sleep afterwards is the absence of the second, not a third
//! thing: [`StayAwake`] is released when the last run ends and the operating
//! system's own idle timeout takes over. Superbackup does **not** force the
//! machine to suspend. It cannot tell "nobody has touched this since we woke
//! it" from "the owner sat down two minutes ago", and suspending a machine
//! somebody is using is far worse than leaving one awake that would have
//! slept in ten minutes anyway.
//!
//! # What this can and cannot do
//!
//! | State | Wakes? |
//! |---|---|
//! | Sleep / standby (S1–S3) | Yes |
//! | Modern Standby (S0 low power) | Yes |
//! | Hibernate (S4) | No |
//! | Powered off | No |
//!
//! A waitable timer lives in a running process, so hibernation and shutdown
//! are out of reach — recovering those needs an entry in the operating
//! system's own scheduler, which would mean a second scheduler disagreeing
//! with this one. [`WakeSupport`] reports the limitation rather than leaving
//! the user to discover it from a backup that did not happen.
//!
//! On Windows the power plan also has to *allow* wake timers, and on battery
//! it disallows them by default. That is a setting in the user's control and
//! not one to change behind their back, so it is read and reported.

use chrono::{DateTime, Utc};

/// Whether this machine can be woken, and what to say when it cannot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeSupport {
    /// A wake alarm can be armed here.
    pub available: bool,
    /// Why not, or a caveat worth showing when it is. Empty when there is
    /// nothing to say.
    pub note: String,
}

impl WakeSupport {
    /// Waking is possible here, with a caveat worth stating.
    ///
    /// Windows and Linux both have a mechanism and both have a condition on
    /// it. macOS has neither yet, so this is genuinely uncalled there — hence
    /// the scoped allow rather than a blanket one.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    fn yes(note: &str) -> WakeSupport {
        WakeSupport { available: true, note: note.to_string() }
    }
    #[cfg_attr(windows, allow(dead_code))]
    fn no(note: &str) -> WakeSupport {
        WakeSupport { available: false, note: note.to_string() }
    }
}

/// Can this machine be woken for a backup?
pub fn support() -> WakeSupport {
    platform_impl::support()
}

/// An armed wake alarm.
///
/// Dropping it cancels the alarm. Re-arming replaces the previous time rather
/// than adding a second one: there is one next run, so there is one alarm.
#[derive(Debug, Default)]
pub struct WakeTimer {
    at: Option<DateTime<Utc>>,
    handle: platform_impl::Handle,
}

impl WakeTimer {
    pub fn new() -> WakeTimer {
        WakeTimer::default()
    }

    /// The time currently armed, if any.
    pub fn armed_for(&self) -> Option<DateTime<Utc>> {
        self.at
    }

    /// Arm for `at`, replacing whatever was armed before.
    ///
    /// A time in the past is treated as a request to disarm rather than as an
    /// alarm that fires immediately: the scheduler is about to run that job
    /// anyway, and an alarm at a past instant on Windows fires at once and
    /// keeps the machine from sleeping.
    ///
    /// Returns whether an alarm is now set. A platform that cannot do this
    /// says so once; it is not an error, because the backup still runs
    /// whenever the machine happens to be awake.
    pub fn arm(&mut self, at: DateTime<Utc>, now: DateTime<Utc>) -> bool {
        if at <= now {
            self.disarm();
            return false;
        }
        if self.at == Some(at) {
            // Already armed for exactly this instant. Re-arming every tick
            // would be a syscall a second for no change.
            return true;
        }
        let seconds = (at - now).num_seconds().max(1);
        if platform_impl::arm(&mut self.handle, seconds) {
            self.at = Some(at);
            true
        } else {
            self.at = None;
            false
        }
    }

    /// Cancel the alarm.
    pub fn disarm(&mut self) {
        if self.at.is_some() {
            platform_impl::disarm(&mut self.handle);
            self.at = None;
        }
    }
}

impl Drop for WakeTimer {
    fn drop(&mut self) {
        self.disarm();
    }
}

/// Holds the machine awake for as long as it is alive.
///
/// # Why this is a guard rather than a pair of calls
///
/// The request must be released on every path a run can end by, including a
/// cancelled run, a driver that panicked, and a daemon shutting down mid-copy.
/// A `stay_awake()` / `allow_sleep()` pair would need every one of those to
/// remember, and the cost of forgetting is a machine that never sleeps again
/// until it is rebooted — a fault the user would blame on anything but a
/// backup tool.
#[derive(Debug)]
pub struct StayAwake {
    held: bool,
    reason: String,
}

impl StayAwake {
    /// Ask the operating system to keep the machine awake.
    ///
    /// `reason` is shown by the platform's own diagnostics — `powercfg
    /// /requests` on Windows — so it says which job, not just which program.
    pub fn hold(reason: impl Into<String>) -> StayAwake {
        let reason = reason.into();
        StayAwake { held: platform_impl::stay_awake(true), reason }
    }

    /// Whether the platform accepted the request. A machine that refused is
    /// not a failure to report: the backup still runs, it is only at risk of
    /// being interrupted by a sleep.
    pub fn is_held(&self) -> bool {
        self.held
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl Drop for StayAwake {
    fn drop(&mut self) {
        if self.held {
            platform_impl::stay_awake(false);
        }
    }
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod platform_impl {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Power::{
        SetThreadExecutionState, ES_CONTINUOUS, ES_SYSTEM_REQUIRED,
    };
    use windows::Win32::System::Threading::{
        CancelWaitableTimer, CreateWaitableTimerW, SetWaitableTimer,
    };

    /// The timer handle, kept so it can be cancelled and closed.
    ///
    /// A waitable timer is only a wake source while its handle is open: the
    /// alarm belongs to the handle, not to the system, so this outliving the
    /// scheduler is the whole point of storing it.
    #[derive(Debug, Default)]
    pub struct Handle(Option<isize>);

    impl Drop for Handle {
        fn drop(&mut self) {
            if let Some(raw) = self.0.take() {
                unsafe {
                    let _ = CloseHandle(HANDLE(raw as *mut _));
                }
            }
        }
    }

    pub fn support() -> super::WakeSupport {
        // Whether the *power plan* permits wake timers is the thing that
        // actually decides this, and it is off on battery by default. Reading
        // it needs PowerReadACValueIndex against GUID_ALLOW_RTC_WAKE; rather
        // than a COM-free but fiddly registry walk that would be wrong on some
        // machines, the caveat is stated. A wrong "yes" here costs the user a
        // backup; a stated caveat costs them a sentence.
        super::WakeSupport::yes(
            "Windows must also allow wake timers in the active power plan. It usually does on \
             mains power and usually does not on battery — check \"Allow wake timers\" under \
             Sleep in the plan's advanced settings. Waking works from sleep, not from \
             hibernation or a machine that is switched off.",
        )
    }

    pub fn arm(handle: &mut Handle, seconds: i64) -> bool {
        unsafe {
            if handle.0.is_none() {
                // Unnamed and not manual-reset. Nothing waits on it: the
                // alarm's only job is to exist, because a waitable timer with
                // `fResume` set is registered with the power manager as a wake
                // source whether or not anybody is blocked on the handle.
                match CreateWaitableTimerW(None, false, None) {
                    Ok(h) if !h.is_invalid() => handle.0 = Some(h.0 as isize),
                    _ => return false,
                }
            }
            let Some(raw) = handle.0 else { return false };
            // Negative is relative, in 100-nanosecond units. Absolute would
            // mean converting to FILETIME and would then be wrong across a
            // clock adjustment between now and the alarm.
            let due: i64 = -seconds.saturating_mul(10_000_000);
            // The `true` is `fResume`: without it this is an ordinary timer
            // that fires only if the machine happens to be awake, which is
            // exactly the situation this exists to fix.
            SetWaitableTimer(HANDLE(raw as *mut _), &due, 0, None, None, true).is_ok()
        }
    }

    pub fn disarm(handle: &mut Handle) {
        if let Some(raw) = handle.0 {
            unsafe {
                let _ = CancelWaitableTimer(HANDLE(raw as *mut _));
            }
        }
    }

    pub fn stay_awake(on: bool) -> bool {
        unsafe {
            // ES_SYSTEM_REQUIRED without ES_DISPLAY_REQUIRED: the machine
            // stays up, the screen does not come on. A backup that turns on
            // the monitor at 02:00 in a bedroom is a backup that gets turned
            // off.
            let flags = if on { ES_CONTINUOUS | ES_SYSTEM_REQUIRED } else { ES_CONTINUOUS };
            SetThreadExecutionState(flags).0 != 0
        }
    }
}

// ---------------------------------------------------------------------------
// Linux
// ---------------------------------------------------------------------------

#[cfg(all(unix, not(target_os = "macos")))]
mod platform_impl {
    use std::io::Write;
    use std::path::Path;

    /// The kernel's real-time-clock alarm.
    ///
    /// `/sys/class/rtc/rtc0/wakealarm` takes an epoch second and wakes the
    /// machine from suspend *and* from hibernation, which is more than the
    /// Windows path manages. Writing it needs root, which the system service
    /// has and a user session does not — so a per-user install reports this as
    /// unavailable rather than failing silently every night.
    const ALARM: &str = "/sys/class/rtc/rtc0/wakealarm";

    #[derive(Debug, Default)]
    pub struct Handle(());

    pub fn support() -> super::WakeSupport {
        if !Path::new(ALARM).exists() {
            return super::WakeSupport::no(
                "This machine has no real-time-clock alarm the kernel exposes \
                 (/sys/class/rtc/rtc0/wakealarm), so it cannot be woken for a backup.",
            );
        }
        if !can_write() {
            return super::WakeSupport::no(
                "Setting the wake alarm needs root. Install superbackup as a system service \
                 (superbackup service install) and it can wake this machine; a per-user \
                 instance cannot.",
            );
        }
        super::WakeSupport::yes(
            "Uses the kernel's real-time-clock alarm, which wakes this machine from suspend and \
             from hibernation. A machine that is switched off stays off.",
        )
    }

    fn can_write() -> bool {
        std::fs::OpenOptions::new().write(true).open(ALARM).is_ok()
    }

    fn write_alarm(value: &str) -> bool {
        std::fs::OpenOptions::new()
            .write(true)
            .open(ALARM)
            .and_then(|mut f| f.write_all(value.as_bytes()))
            .is_ok()
    }

    pub fn arm(_handle: &mut Handle, seconds: i64) -> bool {
        // The interface refuses a new alarm while one is set, so it is always
        // cleared first. Zero is the documented way to clear it.
        let _ = write_alarm("0");
        let at = chrono::Utc::now().timestamp().saturating_add(seconds);
        write_alarm(&at.to_string())
    }

    pub fn disarm(_handle: &mut Handle) {
        let _ = write_alarm("0");
    }

    pub fn stay_awake(_on: bool) -> bool {
        // systemd-inhibit would hold a lock for the life of a child process,
        // which is not what a guard in this process can express, and there is
        // no stable library-free way to take an inhibitor lock. Reported as
        // not held rather than pretended: on Linux the run may be interrupted
        // by a suspend, and saying so is better than a silent no-op.
        false
    }
}

// ---------------------------------------------------------------------------
// macOS
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod platform_impl {
    #[derive(Debug, Default)]
    pub struct Handle(());

    pub fn support() -> super::WakeSupport {
        super::WakeSupport::no(
            "Scheduling a wake on macOS needs `pmset schedule`, which requires an administrator \
             and registers with the system rather than with superbackup. Set it up yourself if \
             you want it: `sudo pmset repeat wake MTWRFSU 01:55:00`, a few minutes before your \
             backup runs.",
        )
    }

    pub fn arm(_handle: &mut Handle, _seconds: i64) -> bool {
        false
    }

    pub fn disarm(_handle: &mut Handle) {}

    pub fn stay_awake(_on: bool) -> bool {
        false
    }
}

#[cfg(not(any(windows, unix)))]
mod platform_impl {
    #[derive(Debug, Default)]
    pub struct Handle(());

    pub fn support() -> super::WakeSupport {
        super::WakeSupport::no("This platform has no way to wake the machine for a backup.")
    }
    pub fn arm(_handle: &mut Handle, _seconds: i64) -> bool {
        false
    }
    pub fn disarm(_handle: &mut Handle) {}
    pub fn stay_awake(_on: bool) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    /// An alarm for a moment that has already passed must not be armed.
    ///
    /// On Windows such a timer fires immediately and re-registers as a wake
    /// source, which is a machine that will not stay asleep — from a job whose
    /// run is about to start anyway.
    #[test]
    fn a_time_in_the_past_disarms_rather_than_firing_now() {
        let now = Utc::now();
        let mut timer = WakeTimer::new();
        assert!(!timer.arm(now - Duration::hours(1), now));
        assert_eq!(timer.armed_for(), None);
        assert!(!timer.arm(now, now), "and now is not the future either");
    }

    /// Re-arming for the same instant is a no-op, because the scheduler
    /// recomputes the next run on every tick and this would otherwise be a
    /// syscall a second forever.
    #[test]
    fn arming_twice_for_the_same_instant_does_not_re_arm() {
        let now = Utc::now();
        let at = now + Duration::hours(2);
        let mut timer = WakeTimer::new();
        if !timer.arm(at, now) {
            // No wake support here (a container, or a non-root Linux build).
            // The rest of this test is about the caching, which needs a first
            // arm to have succeeded.
            return;
        }
        assert_eq!(timer.armed_for(), Some(at));
        assert!(timer.arm(at, now));
        assert_eq!(timer.armed_for(), Some(at));

        // A different instant replaces it.
        let later = now + Duration::hours(3);
        assert!(timer.arm(later, now));
        assert_eq!(timer.armed_for(), Some(later));

        timer.disarm();
        assert_eq!(timer.armed_for(), None);
    }

    /// The guard must release on drop, on every platform, including the ones
    /// where holding it was never possible.
    #[test]
    fn the_stay_awake_guard_releases_itself() {
        let guard = StayAwake::hold("test");
        assert_eq!(guard.reason(), "test");
        let held = guard.is_held();
        drop(guard);
        // Taking and releasing it twice in a row must be safe: this is what a
        // run finishing and the next one starting does.
        let again = StayAwake::hold("test");
        assert_eq!(again.is_held(), held);
    }

    /// Whatever this machine reports, it says something a person can act on.
    #[test]
    fn support_always_explains_itself() {
        let support = support();
        assert!(
            !support.note.is_empty(),
            "a machine that can be woken still has caveats, and one that cannot needs a reason"
        );
        // The note is shown in the interface, so it must be a sentence rather
        // than an API name.
        assert!(support.note.ends_with('.'), "{}", support.note);
    }
}
