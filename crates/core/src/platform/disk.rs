//! Watching the free space on the volumes superbackup writes to.
//!
//! # Why a backup tool of all things needs this
//!
//! A full disk is the one failure that arrives without a fault. Nothing is
//! misconfigured, no credential expired, no network went down — the run simply
//! cannot write, and every night from then on says the same thing. On the
//! machine this was written for it was the *build* volume that filled, and the
//! symptom was a linker error that named a disk nobody had looked at; on a
//! backup volume the symptom is a job that stops working while the interface
//! still says the destination is fine.
//!
//! So it is checked on a schedule and before a run, and it is said in advance:
//! "getting low" is worth a sentence, "will not fit" is worth stopping for.
//!
//! # Why two thresholds and not one
//!
//! A percentage alone is wrong on a large disk: ten per cent of four
//! terabytes is four hundred gigabytes, and warning about that is noise
//! nobody will read twice. An absolute alone is wrong on a small one: ten
//! gigabytes free on a 128 GB laptop SSD is eight per cent and genuinely
//! nearly full, but on a 4 TB archive drive it is a rounding error that is
//! still worth knowing about.
//!
//! Both are configurable and either one being breached raises the warning,
//! because the two describe different kinds of trouble and a user who cares
//! about one should not have to disable the other to say so.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// How full is too full.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DiskSpaceSettings {
    /// Watch the volumes destinations are written to. On by default: a
    /// warning nobody asked for is a nuisance, and a full disk nobody was
    /// warned about is a backup that stopped.
    pub enabled: bool,
    /// Warn under this percentage of the volume free. 0 turns the percentage
    /// rule off and leaves the absolute one.
    pub warn_percent: u8,
    /// Warn under this many gigabytes free. 0 turns the absolute rule off.
    pub warn_gigabytes: u32,
    /// Below this percentage it is reported as critical rather than low.
    pub critical_percent: u8,
    /// Below this many gigabytes it is reported as critical.
    pub critical_gigabytes: u32,
}

impl Default for DiskSpaceSettings {
    fn default() -> Self {
        DiskSpaceSettings {
            enabled: true,
            // Deliberately low. This fires on volumes people actually store
            // backups on, which are often large, and a default that cried
            // wolf at 20% of a 4 TB drive would be switched off within a week
            // and then not there on the night it mattered.
            warn_percent: 5,
            warn_gigabytes: 10,
            critical_percent: 2,
            critical_gigabytes: 2,
        }
    }
}

/// How bad it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiskLevel {
    /// Above every threshold.
    Fine,
    /// Under a warning threshold.
    Low,
    /// Under a critical threshold.
    Critical,
}

impl DiskLevel {
    pub fn label(self) -> &'static str {
        match self {
            DiskLevel::Fine => "Fine",
            DiskLevel::Low => "Running low",
            DiskLevel::Critical => "Almost full",
        }
    }
}

/// One volume's answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiskReport {
    /// What was asked about — a destination's folder, or superbackup's own.
    pub path: PathBuf,
    /// What that place is called, for the sentence shown to the user.
    pub label: String,
    pub free_bytes: u64,
    pub total_bytes: u64,
    pub level: DiskLevel,
}

impl DiskReport {
    /// Free space as a percentage, or `None` for a volume reporting no size.
    pub fn free_percent(&self) -> Option<f64> {
        if self.total_bytes == 0 {
            return None;
        }
        Some(self.free_bytes as f64 * 100.0 / self.total_bytes as f64)
    }

    /// The sentence shown in a banner or a notification.
    ///
    /// Names the place, the figure, and the proportion, because "low disk
    /// space" on a machine with six volumes tells the reader nothing about
    /// which one to go and clear.
    pub fn message(&self) -> String {
        let free = bytesize::ByteSize(self.free_bytes);
        match self.free_percent() {
            Some(percent) => format!(
                "{} has {free} free ({percent:.0}% of the volume).",
                self.label
            ),
            None => format!("{} has {free} free.", self.label),
        }
    }
}

/// Judge one volume against the thresholds.
///
/// `None` when the platform could not answer, which is not a warning: a
/// volume whose size cannot be read is not a volume that is full, and
/// guessing would put a red banner on somebody's dashboard for ever.
pub fn assess(
    path: &Path,
    label: impl Into<String>,
    settings: &DiskSpaceSettings,
) -> Option<DiskReport> {
    let (free_bytes, total_bytes) = super::disk_space(path)?;
    let level = level_for(free_bytes, total_bytes, settings);
    Some(DiskReport { path: path.to_path_buf(), label: label.into(), free_bytes, total_bytes, level })
}

/// The rule, kept apart from the platform call so it can be tested exactly.
pub fn level_for(free: u64, total: u64, settings: &DiskSpaceSettings) -> DiskLevel {
    if !settings.enabled {
        return DiskLevel::Fine;
    }
    let gib = 1024_u64 * 1024 * 1024;
    let percent = |p: u8| -> Option<u64> {
        // A zero threshold is "do not apply this rule", not "warn at zero" —
        // otherwise turning one rule off would silently turn it into one that
        // never fires *and* looks configured.
        (p > 0 && total > 0).then(|| total / 100 * p as u64)
    };
    let absolute = |g: u32| -> Option<u64> { (g > 0).then(|| g as u64 * gib) };

    let breaches = |p: u8, g: u32| {
        percent(p).is_some_and(|limit| free < limit) || absolute(g).is_some_and(|limit| free < limit)
    };

    if breaches(settings.critical_percent, settings.critical_gigabytes) {
        DiskLevel::Critical
    } else if breaches(settings.warn_percent, settings.warn_gigabytes) {
        DiskLevel::Low
    } else {
        DiskLevel::Fine
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1024 * 1024 * 1024;

    fn settings() -> DiskSpaceSettings {
        DiskSpaceSettings::default()
    }

    /// The defaults on the two shapes of disk people actually have.
    #[test]
    fn the_rule_suits_a_small_ssd_and_a_large_archive_drive_alike() {
        let s = settings();

        // A 4 TB archive drive with 150 GB free: 3.7%, under the 5% warning
        // but nowhere near the 2 GB critical. Worth saying once, not an alarm.
        assert_eq!(level_for(150 * GIB, 4000 * GIB, &s), DiskLevel::Low);
        // The same drive with 300 GB free is 7.5% and 300 GB: fine on both
        // counts. A percentage-only rule would have warned here.
        assert_eq!(level_for(300 * GIB, 4000 * GIB, &s), DiskLevel::Fine);

        // A 128 GB laptop SSD with 8 GB free: 6%, so the percentage rule is
        // happy — but 8 GB is under the absolute threshold, and this is the
        // case a percentage-only rule misses entirely.
        assert_eq!(level_for(8 * GIB, 128 * GIB, &s), DiskLevel::Low);
        // 1 GB left is critical by both.
        assert_eq!(level_for(1 * GIB, 128 * GIB, &s), DiskLevel::Critical);
        // Half the disk free is fine.
        assert_eq!(level_for(64 * GIB, 128 * GIB, &s), DiskLevel::Fine);
    }

    /// A threshold of zero switches that rule off rather than becoming a rule
    /// that can never fire while still looking configured.
    #[test]
    fn a_zero_threshold_disables_that_rule_rather_than_meaning_zero() {
        let percent_only =
            DiskSpaceSettings { warn_gigabytes: 0, critical_gigabytes: 0, ..settings() };
        // 8 GB of 128 GB is 6%: above the 5% warning, and the absolute rule
        // is off, so nothing fires.
        assert_eq!(level_for(8 * GIB, 128 * GIB, &percent_only), DiskLevel::Fine);
        assert_eq!(level_for(4 * GIB, 128 * GIB, &percent_only), DiskLevel::Low);

        let absolute_only =
            DiskSpaceSettings { warn_percent: 0, critical_percent: 0, ..settings() };
        // 150 GB of 4 TB is 3.7%, but the percentage rule is off and 150 GB
        // is far above 10 GB.
        assert_eq!(level_for(150 * GIB, 4000 * GIB, &absolute_only), DiskLevel::Fine);
        assert_eq!(level_for(5 * GIB, 4000 * GIB, &absolute_only), DiskLevel::Low);

        // Everything off is always fine, however little is left. The user has
        // said they do not want to be told.
        let off = DiskSpaceSettings {
            warn_percent: 0,
            warn_gigabytes: 0,
            critical_percent: 0,
            critical_gigabytes: 0,
            ..settings()
        };
        assert_eq!(level_for(0, 4000 * GIB, &off), DiskLevel::Fine);
    }

    /// Switched off is switched off, whatever the numbers say.
    #[test]
    fn the_switch_overrides_every_threshold() {
        let s = DiskSpaceSettings { enabled: false, ..settings() };
        assert_eq!(level_for(0, 4000 * GIB, &s), DiskLevel::Fine);
    }

    /// A volume that reports no total must not be treated as a full one.
    #[test]
    fn a_volume_of_unknown_size_is_judged_on_the_absolute_rule_alone() {
        let s = settings();
        // Nothing free and no total: the absolute rule still applies, and it
        // should, because zero bytes free is zero bytes free.
        assert_eq!(level_for(0, 0, &s), DiskLevel::Critical);
        // Plenty free and no total: fine, rather than "0% of 0".
        assert_eq!(level_for(500 * GIB, 0, &s), DiskLevel::Fine);
    }

    /// The sentence names the place and the figure, because a machine with
    /// six volumes needs to be told which one to go and clear.
    #[test]
    fn the_message_names_the_volume_and_how_much_is_left() {
        let report = DiskReport {
            path: PathBuf::from("/mnt/backups"),
            label: "Offsite mirror".into(),
            free_bytes: 3 * GIB,
            total_bytes: 100 * GIB,
            level: DiskLevel::Low,
        };
        let message = report.message();
        assert!(message.contains("Offsite mirror"), "{message}");
        assert!(message.contains("3"), "{message}");
        assert!(message.contains('%'), "{message}");

        // A volume with no reported total says the figure and stops, rather
        // than dividing by zero or claiming a proportion it does not know.
        let unknown = DiskReport { total_bytes: 0, ..report };
        assert!(!unknown.message().contains('%'), "{}", unknown.message());
    }
}
