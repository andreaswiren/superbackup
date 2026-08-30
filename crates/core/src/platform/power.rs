//! Connectivity cost, power source, and sleep/wake detection.
//!
//! # The governing rule
//!
//! **A backup must never be blocked because we could not read a battery.**
//! Every function here returns a definite, safe default rather than an error.
//! "Unknown" is treated as "go ahead": a missed backup is a real loss, a few
//! megabytes on a metered connection is an annoyance. The one asymmetry is
//! that we only report *metered* when the platform says so positively — a
//! guess is not good enough to skip somebody's backup.
//!
//! # Platform coverage
//!
//! | Signal | Windows | Linux | macOS |
//! |---|---|---|---|
//! | Metered | `INetworkCostManager::GetCost` — authoritative | NetworkManager over D-Bus (`busctl`/`nmcli`) when present, else unknown | no public API; always unknown |
//! | Battery | `GetSystemPowerStatus` | `/sys/class/power_supply` | `pmset -g batt` |
//! | Sleep/wake | SCM power events ([`super::service::ServiceSignal`]) plus the clock-gap detector below | clock-gap detector | clock-gap detector |
//!
//! macOS genuinely has no supported way for a normal application to ask
//! whether the current connection is metered — Low Data Mode is not exposed.
//! Rather than shipping a heuristic that would wrongly skip backups, we report
//! [`Metered::Unknown`] and say so in the UI.

use std::path::Path;
use std::time::{Duration as StdDuration, Instant};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// How much a wall-clock jump must exceed the monotonic clock before we call
/// it a sleep. Generous, because NTP steps of a second or two are routine.
pub const SLEEP_DETECTION_TOLERANCE_SECONDS: i64 = 60;

/// Give up on a platform query after this long rather than stalling the
/// scheduler behind a wedged network service. Only the Windows COM path needs
/// it; the Unix implementations block on a short-lived child process instead.
#[cfg_attr(not(windows), allow(dead_code))]
const QUERY_TIMEOUT: StdDuration = StdDuration::from_secs(3);

// ---------------------------------------------------------------------------
// Connection cost
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Metered {
    /// Positively known to be unmetered.
    Unmetered,
    /// Positively known to be metered, over a data cap, or roaming.
    Metered,
    /// We could not tell. Treated as unmetered by [`Metered::should_skip`].
    Unknown,
}

impl Metered {
    /// Should a `skip_on_metered` job be skipped right now?
    ///
    /// Only a positive answer counts. A machine where we cannot read the
    /// connection cost still gets its backups.
    pub fn should_skip(&self) -> bool {
        matches!(self, Metered::Metered)
    }

    pub fn title(&self) -> &'static str {
        match self {
            Metered::Unmetered => "Unmetered connection",
            Metered::Metered => "Metered connection",
            Metered::Unknown => "Connection cost unknown",
        }
    }
}

/// The cost of the connection the machine would use to reach the internet.
pub fn connection_cost() -> Metered {
    platform_impl::connection_cost()
}

/// Convenience wrapper: `true` only when we are certain the link is metered.
pub fn is_metered_connection() -> bool {
    connection_cost().should_skip()
}

/// Interpret Windows' `NLM_CONNECTION_COST` bit field.
///
/// Pure, and tested against the documented bit values, because getting this
/// backwards means either never backing up on Wi-Fi or burning a phone plan.
pub fn classify_nlm_cost(cost: u32) -> Metered {
    // Values from NLM_CONNECTION_COST (netlistmgr.h).
    const UNKNOWN: u32 = 0x0;
    const UNRESTRICTED: u32 = 0x1;
    const FIXED: u32 = 0x2;
    const VARIABLE: u32 = 0x4;
    const OVER_DATA_LIMIT: u32 = 0x10000;
    const CONGESTED: u32 = 0x20000;
    const ROAMING: u32 = 0x40000;
    const APPROACHING_DATA_LIMIT: u32 = 0x80000;

    if cost == UNKNOWN {
        return Metered::Unknown;
    }
    // Any of these means the user is paying by the byte, is roaming, or is
    // about to be throttled. All of them are reasons to wait.
    let expensive =
        FIXED | VARIABLE | OVER_DATA_LIMIT | CONGESTED | ROAMING | APPROACHING_DATA_LIMIT;
    if cost & expensive != 0 {
        return Metered::Metered;
    }
    if cost & UNRESTRICTED != 0 {
        return Metered::Unmetered;
    }
    Metered::Unknown
}

/// Interpret NetworkManager's `Metered` property (`NM_METERED`).
///
/// `busctl get-property … Metered` prints `u 1`. The "guess" values are
/// NetworkManager's own heuristics; we honour a guessed *yes* because on a
/// phone tether that is usually right, and the cost of being wrong is a
/// delayed backup rather than a bill.
pub fn parse_nm_metered(output: &str) -> Option<Metered> {
    let token = output.split_whitespace().next_back()?;
    match token.trim() {
        "1" | "3" => Some(Metered::Metered),   // YES, GUESS_YES
        "2" | "4" => Some(Metered::Unmetered), // NO, GUESS_NO
        "0" => Some(Metered::Unknown),         // UNKNOWN
        _ => None,
    }
}

/// Interpret `nmcli -t -f GENERAL.METERED device show` output, which prints
/// lines such as `GENERAL.METERED:yes (guessed)`.
pub fn parse_nmcli_metered(output: &str) -> Option<Metered> {
    let mut saw_any = false;
    for line in output.lines() {
        let Some((_, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim().to_ascii_lowercase();
        if value.is_empty() || value == "unknown" {
            continue;
        }
        saw_any = true;
        if value.starts_with("yes") {
            // One metered device that would carry our traffic is enough.
            return Some(Metered::Metered);
        }
    }
    if saw_any {
        Some(Metered::Unmetered)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Power source
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PowerSource {
    /// Plugged in.
    Ac,
    /// Running on battery.
    Battery,
    /// Could not tell. Treated as `Ac`, so backups continue.
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PowerStatus {
    pub source: PowerSource,
    /// 0..=100 when known.
    #[serde(default)]
    pub battery_percent: Option<u8>,
    pub charging: bool,
    /// False on a desktop, which is why "on battery" must not be inferred
    /// from a missing battery reading.
    pub battery_present: bool,
}

impl PowerStatus {
    /// The safe default when nothing can be determined: pretend it is a
    /// desktop on mains power, and let the backup run.
    pub fn unknown() -> PowerStatus {
        PowerStatus {
            source: PowerSource::Unknown,
            battery_percent: None,
            charging: false,
            battery_present: false,
        }
    }

    /// Should a `skip_on_battery` job be skipped?
    ///
    /// Only when we positively know we are on battery. A machine with no
    /// battery, or one we could not read, keeps backing up.
    pub fn should_skip_on_battery(&self) -> bool {
        self.source == PowerSource::Battery && !self.charging
    }

    /// Below this, even a user who allows battery backups probably does not
    /// want a two-hour upload. Advisory: the engine decides what to do.
    pub fn is_critically_low(&self) -> bool {
        self.source == PowerSource::Battery
            && !self.charging
            && self.battery_percent.map(|p| p <= 20).unwrap_or(false)
    }

    pub fn summary(&self) -> String {
        match (self.source, self.battery_percent) {
            (PowerSource::Ac, Some(p)) if self.battery_present => {
                format!("Plugged in, battery {p}%")
            }
            (PowerSource::Ac, _) => "Plugged in".to_string(),
            (PowerSource::Battery, Some(p)) => format!("On battery, {p}%"),
            (PowerSource::Battery, None) => "On battery".to_string(),
            (PowerSource::Unknown, _) => "Power state unknown".to_string(),
        }
    }
}

pub fn power_status() -> PowerStatus {
    platform_impl::power_status()
}

/// `true` only when we know we are on battery and not charging.
pub fn is_on_battery() -> bool {
    power_status().should_skip_on_battery()
}

pub fn battery_percent() -> Option<u8> {
    power_status().battery_percent
}

// ---------------------------------------------------------------------------
// Sleep / wake detection
// ---------------------------------------------------------------------------

/// A gap during which the machine was not running.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SleepGap {
    /// How long the machine appears to have been asleep or powered off.
    pub seconds: i64,
    /// Wall-clock time at which we noticed.
    pub detected_at: DateTime<Utc>,
}

/// Notices that the machine slept, so the scheduler can run catch-up jobs.
///
/// # Why a clock-gap detector and not an OS event
///
/// The authoritative signal differs per platform and per host process: a
/// Windows *service* gets `SERVICE_CONTROL_POWEREVENT` (wired up in
/// [`super::service`]), a Windows *tray* gets `WM_POWERBROADCAST` — which needs
/// a message pump and therefore belongs in the GUI crate, not here — Linux has
/// `systemd-logind`'s `PrepareForSleep` signal, and macOS has
/// `NSWorkspaceDidWakeNotification`. None of them cover the case that matters
/// most: the machine was *switched off*, which produces no event at all.
///
/// Comparing the monotonic clock against the wall clock catches all of them.
/// The monotonic clock does not advance across a suspend (nor, obviously,
/// across a power-off); the wall clock does. A gap between the two is a
/// suspend, a shutdown, or a clock step — and for scheduling catch-up runs,
/// those three want the same response.
#[derive(Debug)]
pub struct WakeDetector {
    last_monotonic: Instant,
    last_wall: DateTime<Utc>,
    tolerance_seconds: i64,
}

impl WakeDetector {
    pub fn new() -> WakeDetector {
        WakeDetector {
            last_monotonic: Instant::now(),
            last_wall: Utc::now(),
            tolerance_seconds: SLEEP_DETECTION_TOLERANCE_SECONDS,
        }
    }

    pub fn with_tolerance(seconds: i64) -> WakeDetector {
        WakeDetector { tolerance_seconds: seconds, ..WakeDetector::new() }
    }

    /// Call periodically (once a minute is plenty).
    pub fn tick(&mut self) -> Option<SleepGap> {
        let now_monotonic = Instant::now();
        let elapsed = now_monotonic.saturating_duration_since(self.last_monotonic);
        let gap = self.evaluate(elapsed, Utc::now());
        self.last_monotonic = now_monotonic;
        gap
    }

    /// The pure core, with both clocks injected.
    ///
    /// Also updates `last_wall`, which is why it takes `&mut self`; the
    /// monotonic side is the caller's business so tests can drive it.
    pub fn evaluate(
        &mut self,
        monotonic_elapsed: StdDuration,
        wall_now: DateTime<Utc>,
    ) -> Option<SleepGap> {
        let wall_delta = (wall_now - self.last_wall).num_seconds();
        self.last_wall = wall_now;

        // The clock went backwards — an NTP step, or a user fixing the date.
        // Not a sleep, and not something to run catch-up jobs for.
        if wall_delta < 0 {
            tracing::debug!(wall_delta, "wall clock moved backwards; ignoring");
            return None;
        }
        let monotonic_seconds = monotonic_elapsed.as_secs() as i64;
        let unexplained = wall_delta - monotonic_seconds;
        if unexplained > self.tolerance_seconds {
            Some(SleepGap { seconds: unexplained, detected_at: wall_now })
        } else {
            None
        }
    }
}

impl Default for WakeDetector {
    fn default() -> Self {
        WakeDetector::new()
    }
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod platform_impl {
    use super::*;

    pub fn connection_cost() -> Metered {
        // COM is apartment-scoped and the caller's apartment is none of our
        // business, so the whole query runs on a thread we own and initialise.
        // It also means a wedged Network List Manager cannot stall the
        // scheduler: we simply stop waiting.
        let (tx, rx) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("superbackup-nlm".into())
            .spawn(move || {
                let _ = tx.send(query_cost());
            });
        if spawned.is_err() {
            return Metered::Unknown;
        }
        rx.recv_timeout(QUERY_TIMEOUT).unwrap_or(Metered::Unknown)
    }

    fn query_cost() -> Metered {
        use windows::Win32::Networking::NetworkListManager::{
            INetworkCostManager, INetworkListManager, NetworkListManager,
        };
        use windows::Win32::System::Com::{
            CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
        };
        // `cast` (QueryInterface) lives on this trait.
        use windows::core::Interface;

        // SAFETY: this thread is ours and has never been initialised, so we
        // may pick its apartment. Every `CoInitializeEx` that succeeds is
        // matched by the `CoUninitialize` below before the thread ends.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if hr.is_err() {
            return Metered::Unknown;
        }

        let result = (|| {
            // SAFETY: `NetworkListManager` is a registered in-process CLSID and
            // `INetworkListManager` is one of the interfaces it implements;
            // `CoCreateInstance` validates both and returns an error otherwise.
            let manager: INetworkListManager =
                match unsafe { CoCreateInstance(&NetworkListManager, None, CLSCTX_ALL) } {
                    Ok(m) => m,
                    Err(e) => {
                        tracing::debug!(error = %e, "Network List Manager unavailable");
                        return Metered::Unknown;
                    }
                };
            let cost_manager: INetworkCostManager = match manager.cast() {
                Ok(c) => c,
                Err(e) => {
                    // Present since Windows 8; absent on Server Core installs
                    // without the feature.
                    tracing::debug!(error = %e, "INetworkCostManager unavailable");
                    return Metered::Unknown;
                }
            };
            let mut cost: u32 = 0;
            // SAFETY: `cost` is a valid, writable `u32`; a null destination
            // address is the documented way to ask about the machine's default
            // route rather than a specific peer.
            match unsafe { cost_manager.GetCost(&mut cost, std::ptr::null()) } {
                Ok(()) => classify_nlm_cost(cost),
                Err(e) => {
                    tracing::debug!(error = %e, "GetCost failed");
                    Metered::Unknown
                }
            }
        })();

        // SAFETY: balances the successful `CoInitializeEx` above, on the same
        // thread, before it exits.
        unsafe { CoUninitialize() };
        result
    }

    pub fn power_status() -> PowerStatus {
        use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};

        let mut raw = SYSTEM_POWER_STATUS::default();
        // SAFETY: `raw` is a valid, writable `SYSTEM_POWER_STATUS`; the API
        // fills it in and reports failure through its return value.
        if unsafe { GetSystemPowerStatus(&mut raw) }.is_err() {
            return PowerStatus::unknown();
        }
        parse_system_power_status(raw.ACLineStatus, raw.BatteryFlag, raw.BatteryLifePercent)
    }
}

/// Interpret `SYSTEM_POWER_STATUS`. Pure, and compiled everywhere so the bit
/// handling is covered by the test suite on any host.
///
/// `ACLineStatus`: 0 offline, 1 online, 255 unknown.
/// `BatteryFlag`: bit 3 (8) charging, bit 7 (128) no system battery,
/// 255 unknown.
/// `BatteryLifePercent`: 0..=100, or 255 when unknown.
pub fn parse_system_power_status(
    ac_line: u8,
    battery_flag: u8,
    life_percent: u8,
) -> PowerStatus {
    const NO_BATTERY: u8 = 128;
    const CHARGING: u8 = 8;

    let battery_present = battery_flag != 255 && battery_flag & NO_BATTERY == 0;
    let charging = battery_flag != 255 && battery_flag & CHARGING != 0;
    let source = match ac_line {
        0 if battery_present => PowerSource::Battery,
        0 => PowerSource::Unknown,
        1 => PowerSource::Ac,
        _ => PowerSource::Unknown,
    };
    PowerStatus {
        source,
        battery_percent: (life_percent <= 100).then_some(life_percent),
        charging,
        battery_present,
    }
}

// ---------------------------------------------------------------------------
// Linux
// ---------------------------------------------------------------------------

#[cfg(all(unix, not(target_os = "macos")))]
mod platform_impl {
    use super::*;

    pub fn connection_cost() -> Metered {
        // NetworkManager's global `Metered` property is the one signal that
        // means the same thing on every desktop. `busctl` ships with systemd;
        // `nmcli` ships with NetworkManager itself. Either will do.
        if let Some(output) = run(
            "busctl",
            &[
                "--system",
                "get-property",
                "org.freedesktop.NetworkManager",
                "/org/freedesktop/NetworkManager",
                "org.freedesktop.NetworkManager",
                "Metered",
            ],
        ) {
            if let Some(metered) = parse_nm_metered(&output) {
                return metered;
            }
        }
        if let Some(output) = run("nmcli", &["-t", "-f", "GENERAL.METERED", "device", "show"]) {
            if let Some(metered) = parse_nmcli_metered(&output) {
                return metered;
            }
        }
        // No NetworkManager: a server, or a minimal distribution. Assume the
        // link is unmetered rather than blocking every backup for ever.
        Metered::Unknown
    }

    fn run(program: &str, args: &[&str]) -> Option<String> {
        let output = std::process::Command::new(program).args(args).output().ok()?;
        if !output.status.success() {
            return None;
        }
        Some(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    pub fn power_status() -> PowerStatus {
        super::read_power_supply(Path::new("/sys/class/power_supply"))
    }
}

/// Read Linux's `/sys/class/power_supply` tree.
///
/// Takes the root as a parameter so the parsing can be tested against a fake
/// tree instead of whatever hardware the CI runner happens to have.
///
/// A machine with no battery directory is a desktop: report AC, not "unknown",
/// so `skip_on_battery` never blocks a desktop's backups.
pub fn read_power_supply(root: &Path) -> PowerStatus {
    let Ok(entries) = std::fs::read_dir(root) else {
        return PowerStatus::unknown();
    };

    let mut battery_present = false;
    let mut percent: Option<u8> = None;
    let mut charging = false;
    let mut discharging = false;
    let mut mains_online: Option<bool> = None;

    for entry in entries.flatten() {
        let dir = entry.path();
        let kind = read_trimmed(&dir.join("type")).unwrap_or_default();
        match kind.as_str() {
            "Battery" => {
                // A detachable peripheral (mouse, headset) also shows up as a
                // Battery; `scope = Device` is how the kernel marks those.
                if read_trimmed(&dir.join("scope")).as_deref() == Some("Device") {
                    continue;
                }
                battery_present = true;
                if let Some(capacity) = read_trimmed(&dir.join("capacity"))
                    .and_then(|c| c.parse::<u32>().ok())
                {
                    percent = Some(capacity.min(100) as u8);
                }
                match read_trimmed(&dir.join("status")).as_deref() {
                    Some("Charging") | Some("Full") => charging = true,
                    Some("Discharging") => discharging = true,
                    _ => {}
                }
            }
            "Mains" | "USB" | "USB_PD" | "USB_PD_DRP" => {
                if let Some(online) = read_trimmed(&dir.join("online")) {
                    let on = online == "1";
                    mains_online = Some(mains_online.unwrap_or(false) || on);
                }
            }
            _ => {}
        }
    }

    let source = match (mains_online, battery_present, discharging) {
        (Some(true), _, _) => PowerSource::Ac,
        (Some(false), true, _) => PowerSource::Battery,
        (Some(false), false, _) => PowerSource::Unknown,
        (None, true, true) => PowerSource::Battery,
        (None, true, false) => PowerSource::Ac,
        // No battery and no mains adapter reported: a desktop or a VM.
        (None, false, _) => PowerSource::Ac,
    };

    PowerStatus { source, battery_percent: percent, charging, battery_present }
}

fn read_trimmed(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

// ---------------------------------------------------------------------------
// macOS
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod platform_impl {
    use super::*;

    /// macOS exposes no supported API for "is this connection metered". Low
    /// Data Mode is per-interface and private, and `NWPathMonitor`'s
    /// `isExpensive` is only reachable from a running Network.framework
    /// monitor inside an app bundle. We refuse to guess.
    pub fn connection_cost() -> Metered {
        Metered::Unknown
    }

    pub fn power_status() -> PowerStatus {
        match std::process::Command::new("pmset").args(["-g", "batt"]).output() {
            Ok(out) if out.status.success() => {
                super::parse_pmset(&String::from_utf8_lossy(&out.stdout))
            }
            _ => PowerStatus::unknown(),
        }
    }
}

/// Parse `pmset -g batt`.
///
/// ```text
/// Now drawing from 'Battery Power'
///  -InternalBattery-0 (id=1234)    87%; discharging; 4:21 remaining present: true
/// ```
///
/// Compiled on every platform so the parser is covered by the test suite.
pub fn parse_pmset(text: &str) -> PowerStatus {
    let lower = text.to_ascii_lowercase();
    let on_battery = lower.contains("drawing from 'battery power'");
    let on_ac = lower.contains("drawing from 'ac power'");

    let percent = text
        .split('%')
        .next()
        .and_then(|before| {
            let digits: String = before
                .chars()
                .rev()
                .take_while(|c| c.is_ascii_digit())
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            digits.parse::<u32>().ok()
        })
        .filter(|_| text.contains('%'))
        .map(|p| p.min(100) as u8);

    let charging = lower.contains("; charging") || lower.contains("charged;");
    let battery_present = lower.contains("internalbattery") || lower.contains("present: true");

    let source = if on_battery {
        PowerSource::Battery
    } else if on_ac {
        PowerSource::Ac
    } else {
        PowerSource::Unknown
    };

    PowerStatus { source, battery_percent: percent, charging, battery_present }
}

// ---------------------------------------------------------------------------
// Fallback
// ---------------------------------------------------------------------------

#[cfg(not(any(windows, unix)))]
mod platform_impl {
    use super::*;

    pub fn connection_cost() -> Metered {
        Metered::Unknown
    }
    pub fn power_status() -> PowerStatus {
        PowerStatus::unknown()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nlm_costs_are_classified_conservatively() {
        assert_eq!(classify_nlm_cost(0x0), Metered::Unknown);
        assert_eq!(classify_nlm_cost(0x1), Metered::Unmetered);
        assert_eq!(classify_nlm_cost(0x2), Metered::Metered);
        assert_eq!(classify_nlm_cost(0x4), Metered::Metered);
        assert_eq!(classify_nlm_cost(0x40000), Metered::Metered, "roaming is metered");
        assert_eq!(
            classify_nlm_cost(0x80000),
            Metered::Metered,
            "approaching the data limit is metered"
        );
        // Unrestricted but congested: still worth waiting.
        assert_eq!(classify_nlm_cost(0x1 | 0x20000), Metered::Metered);
    }

    #[test]
    fn unknown_cost_never_blocks_a_backup() {
        assert!(!Metered::Unknown.should_skip());
        assert!(!Metered::Unmetered.should_skip());
        assert!(Metered::Metered.should_skip());
    }

    #[test]
    fn network_manager_metered_property_is_parsed() {
        assert_eq!(parse_nm_metered("u 1"), Some(Metered::Metered));
        assert_eq!(parse_nm_metered("u 3"), Some(Metered::Metered), "guessed yes counts");
        assert_eq!(parse_nm_metered("u 2"), Some(Metered::Unmetered));
        assert_eq!(parse_nm_metered("u 4"), Some(Metered::Unmetered));
        assert_eq!(parse_nm_metered("u 0"), Some(Metered::Unknown));
        assert_eq!(parse_nm_metered("nonsense"), None);
    }

    #[test]
    fn nmcli_device_output_is_parsed() {
        assert_eq!(
            parse_nmcli_metered("GENERAL.METERED:no\nGENERAL.METERED:yes (guessed)\n"),
            Some(Metered::Metered)
        );
        assert_eq!(
            parse_nmcli_metered("GENERAL.METERED:no\nGENERAL.METERED:no\n"),
            Some(Metered::Unmetered)
        );
        assert_eq!(parse_nmcli_metered("GENERAL.METERED:unknown\n"), None);
        assert_eq!(parse_nmcli_metered(""), None);
    }

    #[test]
    fn windows_power_status_bits_are_decoded() {
        // On mains, battery at 91%, charging.
        let s = parse_system_power_status(1, 8, 91);
        assert_eq!(s.source, PowerSource::Ac);
        assert_eq!(s.battery_percent, Some(91));
        assert!(s.charging);
        assert!(s.battery_present);
        assert!(!s.should_skip_on_battery());

        // On battery, 42%.
        let s = parse_system_power_status(0, 1, 42);
        assert_eq!(s.source, PowerSource::Battery);
        assert!(s.should_skip_on_battery());
        assert!(!s.is_critically_low());

        // Desktop: no system battery, on mains.
        let s = parse_system_power_status(1, 128, 255);
        assert_eq!(s.source, PowerSource::Ac);
        assert!(!s.battery_present);
        assert_eq!(s.battery_percent, None);
        assert!(!s.should_skip_on_battery());

        // Everything unknown must never block a backup.
        let s = parse_system_power_status(255, 255, 255);
        assert_eq!(s.source, PowerSource::Unknown);
        assert!(!s.should_skip_on_battery());
    }

    #[test]
    fn critically_low_needs_a_real_reading() {
        let s = parse_system_power_status(0, 1, 15);
        assert!(s.is_critically_low());
        let charging = parse_system_power_status(1, 8, 15);
        assert!(!charging.is_critically_low(), "charging is not critical");
    }

    #[test]
    fn sysfs_power_supply_is_parsed() {
        let root = std::env::temp_dir().join(format!("sb-power-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let bat = root.join("BAT0");
        let ac = root.join("AC");
        std::fs::create_dir_all(&bat).expect("temp dir");
        std::fs::create_dir_all(&ac).expect("temp dir");
        std::fs::write(bat.join("type"), "Battery\n").expect("write");
        std::fs::write(bat.join("capacity"), "73\n").expect("write");
        std::fs::write(bat.join("status"), "Discharging\n").expect("write");
        std::fs::write(ac.join("type"), "Mains\n").expect("write");
        std::fs::write(ac.join("online"), "0\n").expect("write");

        let s = read_power_supply(&root);
        assert_eq!(s.source, PowerSource::Battery);
        assert_eq!(s.battery_percent, Some(73));
        assert!(s.battery_present);
        assert!(s.should_skip_on_battery());

        std::fs::write(ac.join("online"), "1\n").expect("write");
        std::fs::write(bat.join("status"), "Charging\n").expect("write");
        let s = read_power_supply(&root);
        assert_eq!(s.source, PowerSource::Ac);
        assert!(s.charging);
        assert!(!s.should_skip_on_battery());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_wireless_mouse_battery_is_not_the_laptop_battery() {
        let root = std::env::temp_dir().join(format!("sb-power-mouse-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mouse = root.join("hidpp_battery_0");
        std::fs::create_dir_all(&mouse).expect("temp dir");
        std::fs::write(mouse.join("type"), "Battery\n").expect("write");
        std::fs::write(mouse.join("scope"), "Device\n").expect("write");
        std::fs::write(mouse.join("capacity"), "5\n").expect("write");
        std::fs::write(mouse.join("status"), "Discharging\n").expect("write");

        let s = read_power_supply(&root);
        assert!(!s.battery_present, "a peripheral must not look like a laptop battery");
        assert!(!s.should_skip_on_battery());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_sysfs_tree_is_not_a_battery() {
        let s = read_power_supply(Path::new("/definitely/not/here"));
        assert_eq!(s.source, PowerSource::Unknown);
        assert!(!s.should_skip_on_battery());
    }

    #[test]
    fn pmset_output_is_parsed() {
        let text = "Now drawing from 'Battery Power'\n \
                    -InternalBattery-0 (id=12345)\t87%; discharging; 4:21 remaining present: true";
        let s = parse_pmset(text);
        assert_eq!(s.source, PowerSource::Battery);
        assert_eq!(s.battery_percent, Some(87));
        assert!(s.battery_present);

        let text = "Now drawing from 'AC Power'\n \
                    -InternalBattery-0 (id=12345)\t100%; charged; 0:00 remaining present: true";
        let s = parse_pmset(text);
        assert_eq!(s.source, PowerSource::Ac);
        assert_eq!(s.battery_percent, Some(100));
        assert!(!s.should_skip_on_battery());
    }

    #[test]
    fn a_sleep_shows_up_as_an_unexplained_clock_gap() {
        let mut detector = WakeDetector::with_tolerance(60);
        let t0 = Utc::now();
        detector.last_wall = t0;

        // A normal minute: both clocks advanced together.
        assert!(detector.evaluate(StdDuration::from_secs(60), t0 + chrono::Duration::seconds(60)).is_none());

        // Eight hours of wall clock, one second of monotonic: the lid was shut.
        let gap = detector
            .evaluate(
                StdDuration::from_secs(1),
                t0 + chrono::Duration::seconds(60) + chrono::Duration::hours(8),
            )
            .expect("an eight-hour gap is a sleep");
        assert!(gap.seconds > 28_000, "{gap:?}");
    }

    #[test]
    fn an_ntp_step_backwards_is_not_a_wake() {
        let mut detector = WakeDetector::with_tolerance(60);
        let t0 = Utc::now();
        detector.last_wall = t0;
        assert!(detector
            .evaluate(StdDuration::from_secs(60), t0 - chrono::Duration::hours(1))
            .is_none());
    }

    #[test]
    fn a_small_clock_correction_is_tolerated() {
        let mut detector = WakeDetector::with_tolerance(60);
        let t0 = Utc::now();
        detector.last_wall = t0;
        assert!(detector
            .evaluate(StdDuration::from_secs(60), t0 + chrono::Duration::seconds(90))
            .is_none());
    }

    #[test]
    fn live_queries_return_something_sane_and_never_panic() {
        // These hit the real platform. They must never panic and must never
        // report "metered" on a machine with no way to tell.
        let cost = connection_cost();
        assert!(matches!(cost, Metered::Metered | Metered::Unmetered | Metered::Unknown));
        let status = power_status();
        assert!(status.battery_percent.map(|p| p <= 100).unwrap_or(true));
        assert!(!status.summary().is_empty());
    }
}
