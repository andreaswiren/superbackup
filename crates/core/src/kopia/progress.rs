//! Incremental parsing of kopia's upload progress.
//!
//! ## Why this is a text parser and not a JSON reader
//!
//! Kopia has **no machine-readable progress stream**. `cli/cli_progress.go`
//! builds one human line and rewrites it in place on stderr:
//!
//! ```text
//! \r | 3 hashing, 1204 hashed (1.2 GB), 88 cached (410 MB), uploaded 903.1 MB, estimated 4.4 GB (39.1%) 2m10s left
//! ```
//!
//! `--json` exists on `snapshot create`, but it only prints the finished
//! manifest to stdout at the end; there is no `--progress=json`. So the GUI's
//! live numbers can only come from this line. The final, exact numbers come
//! from the manifest (see [`super::manifest`]) — this parser feeds the bar
//! while the bar is moving, and the manifest corrects it at the end.
//!
//! Two consequences the caller must know about:
//!
//! 1. **The line is `\r`-terminated, not `\n`-terminated.** A reader that
//!    splits on newlines alone sees nothing until the process exits. The
//!    stderr pump in [`super::command`] splits on both.
//! 2. **The byte counts are rounded to one decimal digit** by kopia's
//!    `units.BytesString`, so `bytes_processed` is accurate to about 0.1 of
//!    whichever unit is in play. That is fine for a progress bar and wrong for
//!    accounting, which is why the run history is filled from the manifest.
//!
//! Progress output is *off* whenever stdout is not a terminal
//! (`cli_progress.go` defaults the flag from `term.IsTerminal`), so the driver
//! always passes `--progress` explicitly. Without it this parser is fed
//! nothing at all.

use crate::state::Progress;
use std::time::{Duration, Instant};

/// The spinner characters kopia cycles through, plus `*` which it prints once
/// the upload has finished. Stripping these is the first parsing step.
const SPINNER_CHARS: &[char] = &['|', '/', '-', '\\', '*'];

/// One decoded progress line, before it is folded into [`Progress`].
///
/// Kept separate from `Progress` so the parser can be tested against recorded
/// kopia output without dragging in rate estimation or wall-clock time.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProgressLine {
    /// Files currently being hashed (kopia's parallelism in flight).
    pub hashing_in_progress: u64,
    pub hashed_files: u64,
    pub hashed_bytes: u64,
    pub cached_files: u64,
    pub cached_bytes: u64,
    /// Bytes actually written to the repository after dedup and compression.
    pub uploaded_bytes: u64,
    pub fatal_errors: u64,
    pub ignored_errors: u64,
    /// Kopia's own estimate of the total, once the estimation pass has run.
    pub estimated_total_bytes: Option<u64>,
    pub percent_complete: Option<f32>,
    pub time_remaining: Option<Duration>,
}

/// Parse one kopia progress line. Returns `None` for anything that is not one,
/// which is most of what arrives on stderr.
///
/// Deliberately allocation-free and total: malformed input yields `None`, never
/// a panic. Kopia has reworded this line before and will again.
pub fn parse_progress_line(raw: &str) -> Option<ProgressLine> {
    // The line is rewritten in place, so it arrives padded with the spaces
    // kopia used to wipe the previous, longer line.
    let mut s = raw.trim();
    if s.is_empty() {
        return None;
    }
    // Strip the leading spinner character, if present.
    if let Some(first) = s.chars().next() {
        if SPINNER_CHARS.contains(&first) {
            s = s[first.len_utf8()..].trim_start();
        }
    }

    let mut out = ProgressLine::default();

    let (hashing, s) = s.split_once(" hashing, ")?;
    out.hashing_in_progress = parse_u64(hashing)?;

    let (hashed_files, s) = s.split_once(" hashed (")?;
    out.hashed_files = parse_u64(hashed_files)?;

    let (hashed_bytes, s) = s.split_once("), ")?;
    out.hashed_bytes = parse_bytes(hashed_bytes)?;

    let (cached_files, s) = s.split_once(" cached (")?;
    out.cached_files = parse_u64(cached_files)?;

    let (cached_bytes, s) = s.split_once("), uploaded ")?;
    out.cached_bytes = parse_bytes(cached_bytes)?;

    // What remains is `<uploaded>[ (N fatal errors)][ (N errors ignored)]`
    // optionally followed by `, estimating...` or `, estimated ...`.
    let (mut head, estimate) = match s.find(", estimat") {
        Some(idx) => (s[..idx].trim(), Some(s[idx + 2..].trim())),
        None => (s.trim(), None),
    };

    // Peel the parenthesised error counts off the end, innermost last.
    while head.ends_with(')') {
        let open = match head.rfind('(') {
            Some(i) => i,
            None => break,
        };
        let group = &head[open + 1..head.len() - 1];
        if let Some(n) = group.strip_suffix(" fatal errors").and_then(parse_u64) {
            out.fatal_errors = n;
        } else if let Some(n) = group.strip_suffix(" errors ignored").and_then(parse_u64) {
            out.ignored_errors = n;
        } else {
            // Not an error group — it belongs to the byte string, stop here.
            break;
        }
        head = head[..open].trim_end();
    }
    out.uploaded_bytes = parse_bytes(head)?;

    if let Some(est) = estimate {
        // `estimating...` while the scan is still running; `estimated <total>
        // (<pct>%) <duration> left` once it has a number.
        if let Some(rest) = est.strip_prefix("estimated ") {
            if let Some((total, rest)) = rest.split_once(" (") {
                out.estimated_total_bytes = parse_bytes(total);
                if let Some((pct, rest)) = rest.split_once("%) ") {
                    out.percent_complete = pct.trim().parse::<f32>().ok();
                    out.time_remaining =
                        rest.strip_suffix(" left").and_then(|d| parse_go_duration(d.trim()));
                }
            } else {
                out.estimated_total_bytes = parse_bytes(rest);
            }
        }
    }

    Some(out)
}

fn parse_u64(s: &str) -> Option<u64> {
    s.trim().parse::<u64>().ok()
}

/// Parse `units.BytesString` output: `niceNumber(f) + " " + prefix + "B"`.
///
/// Base 10 (`1.2 GB`) is kopia's default; base 2 (`1.2 GiB`) appears when
/// `KOPIA_BYTES_STRING_BASE_2` is set. The driver pins that variable to `false`
/// for determinism, but both are accepted so a user's inherited environment
/// cannot silently break the progress bar by a factor of 1.024.
pub fn parse_bytes(s: &str) -> Option<u64> {
    let s = s.trim();
    let (number, unit) = match s.rsplit_once(' ') {
        Some((n, u)) => (n.trim(), u.trim()),
        // Tolerate a missing space, which no current kopia emits.
        None => (s.trim_end_matches(|c: char| c.is_ascii_alphabetic()), ""),
    };
    let value: f64 = number.parse().ok()?;
    if !value.is_finite() || value < 0.0 {
        return None;
    }
    let unit = unit.strip_suffix('B').unwrap_or(unit);
    let (base, exp): (f64, u32) = match unit {
        "" => (1000.0, 0),
        "K" => (1000.0, 1),
        "M" => (1000.0, 2),
        "G" => (1000.0, 3),
        "T" => (1000.0, 4),
        "P" => (1000.0, 5),
        "E" => (1000.0, 6),
        "Ki" => (1024.0, 1),
        "Mi" => (1024.0, 2),
        "Gi" => (1024.0, 3),
        "Ti" => (1024.0, 4),
        "Pi" => (1024.0, 5),
        "Ei" => (1024.0, 6),
        _ => return None,
    };
    let scaled = value * base.powi(exp as i32);
    if scaled > u64::MAX as f64 {
        return None;
    }
    Some(scaled.round() as u64)
}

/// Parse a Go `time.Duration` string such as `1m20s`, `2h3m4s`, `1.5s`, `500ms`.
///
/// Go's own formatter is what produces the "N left" field, and Rust has no
/// parser for that syntax, so this is hand-rolled. Unknown units yield `None`
/// rather than a wrong number.
pub fn parse_go_duration(s: &str) -> Option<Duration> {
    let s = s.trim();
    if s.is_empty() || s == "0" || s == "0s" {
        return Some(Duration::ZERO);
    }
    let mut total = Duration::ZERO;
    let mut rest = s.strip_prefix('+').unwrap_or(s);
    if rest.starts_with('-') {
        return None; // A negative remaining time is nonsense; refuse it.
    }
    let mut saw_component = false;
    while !rest.is_empty() {
        let digits_end = rest.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(rest.len());
        if digits_end == 0 {
            return None;
        }
        let value: f64 = rest[..digits_end].parse().ok()?;
        rest = &rest[digits_end..];
        let unit_end = rest.find(|c: char| c.is_ascii_digit()).unwrap_or(rest.len());
        let unit = &rest[..unit_end];
        rest = &rest[unit_end..];
        let seconds = match unit {
            "ns" => value / 1e9,
            "us" | "µs" | "μs" => value / 1e6,
            "ms" => value / 1e3,
            "s" => value,
            "m" => value * 60.0,
            "h" => value * 3600.0,
            _ => return None,
        };
        if !seconds.is_finite() || seconds < 0.0 {
            return None;
        }
        total += Duration::from_secs_f64(seconds);
        saw_component = true;
    }
    saw_component.then_some(total)
}

/// One decoded `kopia restore` progress line.
///
/// Restore uses a completely different renderer (`cli/restore_progress.go`):
///
/// ```text
/// \rProcessed 812 (1.9 GB) of 4021 (6.5 GB), skipped 3 (1 KB), ignored 2 errors 41.2 MB/s (29.2%) remaining 1m50s.
/// ```
///
/// It is also `\r`-terminated, it also only appears with `--progress`, and it
/// suppresses itself entirely until the first byte has been restored.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RestoreProgressLine {
    pub processed_entries: u64,
    pub processed_bytes: u64,
    pub total_entries: u64,
    pub total_bytes: u64,
    pub skipped_entries: u64,
    pub skipped_bytes: u64,
    pub ignored_errors: u64,
    pub bytes_per_second: Option<f64>,
    pub percent_complete: Option<f32>,
    pub time_remaining: Option<Duration>,
}

/// Parse a `kopia restore` progress line. `None` for anything else.
pub fn parse_restore_progress_line(raw: &str) -> Option<RestoreProgressLine> {
    let s = raw.trim().strip_prefix("Processed ")?;
    let s = s.strip_suffix('.').unwrap_or(s);
    let mut out = RestoreProgressLine::default();

    let (count, s) = s.split_once(" (")?;
    out.processed_entries = parse_u64(count)?;
    let (bytes, s) = s.split_once(") of ")?;
    out.processed_bytes = parse_bytes(bytes)?;
    let (total_count, s) = s.split_once(" (")?;
    out.total_entries = parse_u64(total_count)?;
    let (total_bytes, mut s) = s.split_once(')')?;
    out.total_bytes = parse_bytes(total_bytes)?;

    if let Some(rest) = s.strip_prefix(", skipped ") {
        let (count, rest) = rest.split_once(" (")?;
        out.skipped_entries = parse_u64(count)?;
        let (bytes, rest) = rest.split_once(')')?;
        out.skipped_bytes = parse_bytes(bytes)?;
        s = rest;
    }
    if let Some(rest) = s.strip_prefix(", ignored ") {
        let (count, rest) = rest.split_once(" errors")?;
        out.ignored_errors = parse_u64(count)?;
        s = rest;
    }
    // ` <speed> (<pct>%) remaining <duration>`, present only once kopia has an
    // estimate.
    let s = s.trim();
    if let Some((speed, rest)) = s.split_once(" (") {
        out.bytes_per_second = parse_bytes_per_second(speed);
        if let Some((pct, rest)) = rest.split_once("%) remaining ") {
            out.percent_complete = pct.trim().parse().ok();
            out.time_remaining = parse_go_duration(rest.trim());
        }
    }
    Some(out)
}

/// One decoded `kopia repository sync-to` progress line.
///
/// `sync-to` has a third progress renderer, unrelated to the upload and
/// restore ones. `cli/command_repository_sync.go` builds it with
/// `outputSyncProgress`, which — like the others — pads the line and rewrites
/// it in place with a leading `\r`, and emits nothing at all unless
/// `--progress` is on.
///
/// Two of its shapes carry numbers worth showing:
///
/// ```text
/// \r  Found 41230 BLOBs (88.1 GB) in the source repository, 512 (1.2 GB) to copy
/// \r  Copied 214 blobs (612.4 MB), Speed: 18.1 MB/s, ETA: 42s
/// ```
///
/// The first is the only place the *total* appears, so a progress bar for a
/// replication needs both: the "to copy" line sets the denominator once and
/// the "Copied" lines move the numerator. Every field is optional because a
/// given line only ever carries one of the two halves.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SyncProgressLine {
    pub blobs_copied: Option<u64>,
    pub bytes_copied: Option<u64>,
    pub bytes_per_second: Option<f64>,
    pub time_remaining: Option<Duration>,
    /// Blobs kopia decided it has to copy — the denominator.
    pub blobs_to_copy: Option<u64>,
    pub bytes_to_copy: Option<u64>,
}

/// Parse a `kopia repository sync-to` progress line. `None` for anything else.
///
/// Total, like every parser in this module: a reworded line costs a stalled
/// progress bar, never a failed replication.
pub fn parse_sync_progress_line(raw: &str) -> Option<SyncProgressLine> {
    let s = raw.trim();
    let mut out = SyncProgressLine::default();

    if let Some(rest) = s.strip_prefix("Copied ") {
        let (blobs, rest) = rest.split_once(" blobs (")?;
        out.blobs_copied = Some(parse_u64(blobs)?);
        let (bytes, rest) = rest.split_once("), Speed: ")?;
        out.bytes_copied = Some(parse_bytes(bytes)?);
        // `Speed:` and `ETA:` are formatted by kopia and may both be `-`
        // before the first sample, so neither is required.
        match rest.split_once(", ETA: ") {
            Some((speed, eta)) => {
                out.bytes_per_second = parse_bytes_per_second(speed);
                out.time_remaining = parse_go_duration(eta.trim());
            }
            None => out.bytes_per_second = parse_bytes_per_second(rest),
        }
        return Some(out);
    }

    if let Some(rest) = s.strip_prefix("Found ") {
        // `Found N BLOBs (X) in the source repository, M (Y) to copy`. The
        // other `Found …` lines (destination inventory, deletion counts) carry
        // no number this progress bar can use, so they fall through to `None`.
        let (_source_blobs, rest) = rest.split_once(" BLOBs (")?;
        let (_source_bytes, rest) = rest.split_once(") in the source repository, ")?;
        let (blobs, rest) = rest.split_once(" (")?;
        out.blobs_to_copy = Some(parse_u64(blobs)?);
        let (bytes, _) = rest.split_once(") to copy")?;
        out.bytes_to_copy = Some(parse_bytes(bytes)?);
        return Some(out);
    }

    None
}

/// `units.BytesPerSecondsString` output: the byte formatter with a `/s` suffix.
fn parse_bytes_per_second(s: &str) -> Option<f64> {
    parse_bytes(s.trim().strip_suffix("/s")?).map(|v| v as f64)
}

/// Folds a stream of progress lines into [`crate::state::Progress`], adding the
/// things kopia's line does not carry: a transfer rate, and the path currently
/// being backed up.
#[derive(Debug)]
pub struct ProgressTracker {
    progress: Progress,
    started: Instant,
    /// `(when, bytes_processed)` of the previous sample, for the rate.
    last_sample: Option<(Instant, u64)>,
    /// Smoothed rate, so the GUI number does not flicker between samples.
    smoothed_bps: f64,
}

impl Default for ProgressTracker {
    fn default() -> Self {
        ProgressTracker::new()
    }
}

impl ProgressTracker {
    pub fn new() -> Self {
        ProgressTracker {
            progress: Progress::default(),
            started: Instant::now(),
            last_sample: None,
            smoothed_bps: 0.0,
        }
    }

    /// Seed the path shown in the GUI.
    ///
    /// Kopia's progress line has no "currently reading" field — the closest it
    /// offers is an `Snapshotting <source>` info log, which we suppress to keep
    /// stderr parseable. The driver therefore seeds the source path it asked
    /// for, which is what the user actually wants to see.
    pub fn set_current_path(&mut self, path: impl Into<String>) {
        self.progress.current_path = Some(path.into());
    }

    /// Seed the totals from `snapshot estimate`, so the bar is meaningful from
    /// the first second rather than only after kopia's own estimation pass.
    pub fn seed_totals(&mut self, files: Option<u64>, bytes: Option<u64>) {
        if self.progress.files_total.is_none() {
            self.progress.files_total = files;
        }
        if self.progress.bytes_total.is_none() {
            self.progress.bytes_total = bytes;
        }
    }

    /// Feed one stderr fragment. Returns `true` when it was a progress line and
    /// [`ProgressTracker::progress`] therefore changed.
    ///
    /// Handles all three renderers — upload, restore and repository sync —
    /// because one command pipeline drives all of them and the caller should
    /// not have to know which shape to expect.
    pub fn feed(&mut self, line: &str) -> bool {
        if let Some(parsed) = parse_progress_line(line) {
            self.apply(&parsed, Instant::now());
            return true;
        }
        if let Some(parsed) = parse_restore_progress_line(line) {
            self.apply_restore(&parsed);
            return true;
        }
        if let Some(parsed) = parse_sync_progress_line(line) {
            self.apply_sync(&parsed);
            return true;
        }
        false
    }

    /// A replication moves blobs, not files, and kopia reports its own rate and
    /// ETA — so there is nothing to estimate here either.
    ///
    /// Counters are mapped onto the file/byte fields rather than given new
    /// ones: the GUI's bar, the tray tooltip and the run history all read
    /// [`Progress`], and a second set of fields that only replication filled in
    /// would leave every one of them showing an empty destination. "Files" is
    /// the wrong word for a blob, and the destination's own
    /// `replicated_from` marker is what tells the reader which it is looking at.
    fn apply_sync(&mut self, line: &SyncProgressLine) {
        // Set once, by the "…to copy" line, and deliberately not overwritten:
        // "Copied" lines carry no denominator, so recomputing it from them
        // would make the bar jump to 100% on the first sample.
        if let Some(blobs) = line.blobs_to_copy {
            self.progress.files_total = Some(blobs).filter(|n| *n > 0);
        }
        if let Some(bytes) = line.bytes_to_copy {
            self.progress.bytes_total = Some(bytes).filter(|b| *b > 0);
        }
        if let Some(blobs) = line.blobs_copied {
            self.progress.files_processed = blobs;
        }
        if let Some(bytes) = line.bytes_copied {
            self.progress.bytes_processed = bytes;
            // Every replicated byte crosses the wire: there is no local dedup
            // win to subtract, because kopia already decided this blob is
            // missing at the destination.
            self.progress.bytes_uploaded = bytes;
        }
        if let Some(bps) = line.bytes_per_second {
            self.progress.bytes_per_second = bps;
        }
        if let Some(remaining) = line.time_remaining {
            self.progress.estimated_seconds_remaining = Some(remaining.as_secs());
        }
    }

    /// Restore reports totals directly, so there is no rate to estimate and no
    /// dedup distinction to draw: every restored byte was read from the
    /// repository and written to disk.
    fn apply_restore(&mut self, line: &RestoreProgressLine) {
        self.progress.files_processed = line.processed_entries;
        self.progress.files_total = Some(line.total_entries).filter(|n| *n > 0);
        self.progress.bytes_processed = line.processed_bytes;
        self.progress.bytes_total = Some(line.total_bytes).filter(|n| *n > 0);
        self.progress.errors_ignored = line.ignored_errors;
        if let Some(bps) = line.bytes_per_second {
            self.progress.bytes_per_second = bps;
        }
        self.progress.estimated_seconds_remaining = line.time_remaining.map(|d| d.as_secs());
    }

    /// The rate/ETA maths, split out so it can be driven from a fake clock.
    fn apply(&mut self, line: &ProgressLine, now: Instant) {
        let processed = line.hashed_bytes.saturating_add(line.cached_bytes);

        // Rate is measured on processed bytes, not uploaded bytes, so that it
        // matches the denominator the percentage and ETA use. A run that is
        // 100% deduplicated still shows honest scan throughput.
        if let Some((prev_at, prev_bytes)) = self.last_sample {
            let dt = now.saturating_duration_since(prev_at).as_secs_f64();
            if dt >= 0.25 {
                let delta = processed.saturating_sub(prev_bytes) as f64;
                let instant = delta / dt;
                // Exponential smoothing; 0.3 settles in about a second at
                // kopia's 300 ms default update interval.
                self.smoothed_bps = if self.smoothed_bps == 0.0 {
                    instant
                } else {
                    0.7 * self.smoothed_bps + 0.3 * instant
                };
                self.last_sample = Some((now, processed));
            }
        } else {
            let elapsed = now.saturating_duration_since(self.started).as_secs_f64();
            if elapsed > 0.0 {
                self.smoothed_bps = processed as f64 / elapsed;
            }
            self.last_sample = Some((now, processed));
        }

        self.progress.files_processed = line.hashed_files.saturating_add(line.cached_files);
        self.progress.files_cached = line.cached_files;
        self.progress.bytes_processed = processed;
        self.progress.bytes_uploaded = line.uploaded_bytes;
        self.progress.errors_ignored = line.ignored_errors;
        self.progress.bytes_per_second = self.smoothed_bps;
        if let Some(total) = line.estimated_total_bytes {
            self.progress.bytes_total = Some(total);
        }
        self.progress.estimated_seconds_remaining =
            line.time_remaining.map(|d| d.as_secs()).or_else(|| self.derive_eta());
    }

    /// Fallback ETA for the window before kopia has an estimate of its own.
    fn derive_eta(&self) -> Option<u64> {
        let total = self.progress.bytes_total?;
        let done = self.progress.bytes_processed;
        if self.smoothed_bps <= 0.0 || done >= total {
            return None;
        }
        Some(((total - done) as f64 / self.smoothed_bps) as u64)
    }

    pub fn progress(&self) -> &Progress {
        &self.progress
    }

    /// A clone for sending over a channel.
    pub fn snapshot(&self) -> Progress {
        self.progress.clone()
    }

    /// Overwrite the counters with the exact numbers from the finished
    /// snapshot manifest, so the last thing the GUI shows is the truth rather
    /// than a rounded progress line.
    pub fn finalise(&mut self, files: u64, bytes: u64, ignored_errors: u64) {
        self.progress.files_processed = files;
        self.progress.files_total = Some(files);
        self.progress.bytes_processed = bytes;
        // The total is now known exactly, so kopia's earlier *estimate* must be
        // replaced rather than kept: leaving a too-large estimate in place would
        // freeze the finished progress bar at 58%.
        self.progress.bytes_total = Some(bytes);
        self.progress.errors_ignored = ignored_errors;
        self.progress.estimated_seconds_remaining = Some(0);
        self.progress.current_path = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Recorded from kopia 0.21 running `snapshot create --progress` with
    // stderr redirected to a file (which is why the spinner and the `\r`
    // padding survive).
    const ESTIMATING: &str =
        " | 3 hashing, 1204 hashed (1.2 GB), 88 cached (410 MB), uploaded 903.1 MB, estimating...";
    const RUNNING: &str = " / 2 hashing, 15316 hashed (4.4 GB), 1201 cached (2.1 GB), uploaded 1.9 GB, estimated 11.2 GB (58.1%) 2m10s left";
    const WITH_IGNORED: &str = " \\ 0 hashing, 900 hashed (1 GB), 0 cached (0 B), uploaded 1 GB (7 errors ignored), estimated 1 GB (100.0%) 0s left";
    const FINISHED: &str = " * 0 hashing, 15316 hashed (4.4 GB), 1201 cached (2.1 GB), uploaded 1.9 GB, estimated 6.5 GB (100.0%) 0s left";

    #[test]
    fn parses_the_estimating_phase() {
        let p = parse_progress_line(ESTIMATING).expect("should parse");
        assert_eq!(p.hashing_in_progress, 3);
        assert_eq!(p.hashed_files, 1204);
        assert_eq!(p.hashed_bytes, 1_200_000_000);
        assert_eq!(p.cached_files, 88);
        assert_eq!(p.cached_bytes, 410_000_000);
        assert_eq!(p.uploaded_bytes, 903_100_000);
        assert_eq!(p.estimated_total_bytes, None);
    }

    #[test]
    fn parses_a_running_line_with_eta() {
        let p = parse_progress_line(RUNNING).expect("should parse");
        assert_eq!(p.hashed_files, 15316);
        assert_eq!(p.cached_bytes, 2_100_000_000);
        assert_eq!(p.uploaded_bytes, 1_900_000_000);
        assert_eq!(p.estimated_total_bytes, Some(11_200_000_000));
        assert_eq!(p.percent_complete, Some(58.1));
        assert_eq!(p.time_remaining, Some(Duration::from_secs(130)));
    }

    #[test]
    fn parses_ignored_error_count() {
        let p = parse_progress_line(WITH_IGNORED).expect("should parse");
        assert_eq!(p.ignored_errors, 7);
        assert_eq!(p.fatal_errors, 0);
        assert_eq!(p.uploaded_bytes, 1_000_000_000);
        assert_eq!(p.cached_bytes, 0);
    }

    #[test]
    fn parses_both_error_groups() {
        let line = " | 0 hashing, 1 hashed (1 B), 0 cached (0 B), uploaded 1 B (2 fatal errors) (3 errors ignored), estimating...";
        let p = parse_progress_line(line).expect("should parse");
        assert_eq!(p.fatal_errors, 2);
        assert_eq!(p.ignored_errors, 3);
        assert_eq!(p.uploaded_bytes, 1);
    }

    #[test]
    fn trailing_wipe_padding_is_ignored() {
        let padded = format!("{FINISHED}          ");
        assert!(parse_progress_line(&padded).is_some());
    }

    #[test]
    fn non_progress_lines_are_rejected() {
        for line in [
            "",
            "  ",
            "kopia: error: unable to connect to repository",
            "Ignored error when processing \"C:\\\\x\": access is denied",
            "Created snapshot with root k2f1 and ID abc in 1m2s",
            " | 3 hashing, garbage",
        ] {
            assert!(parse_progress_line(line).is_none(), "wrongly parsed: {line:?}");
        }
    }

    #[test]
    fn byte_strings_round_trip_both_bases() {
        assert_eq!(parse_bytes("0 B"), Some(0));
        assert_eq!(parse_bytes("512 B"), Some(512));
        assert_eq!(parse_bytes("1.2 GB"), Some(1_200_000_000));
        assert_eq!(parse_bytes("1 KiB"), Some(1024));
        assert_eq!(parse_bytes("1.5 MiB"), Some(1_572_864));
        assert_eq!(parse_bytes("nonsense"), None);
        assert_eq!(parse_bytes("1 QB"), None);
    }

    #[test]
    fn go_durations() {
        assert_eq!(parse_go_duration("0s"), Some(Duration::ZERO));
        assert_eq!(parse_go_duration("45s"), Some(Duration::from_secs(45)));
        assert_eq!(parse_go_duration("2m10s"), Some(Duration::from_secs(130)));
        assert_eq!(parse_go_duration("1h2m3s"), Some(Duration::from_secs(3723)));
        assert_eq!(parse_go_duration("500ms"), Some(Duration::from_millis(500)));
        assert_eq!(parse_go_duration("-5s"), None);
        assert_eq!(parse_go_duration("5 fortnights"), None);
    }

    #[test]
    fn tracker_separates_uploaded_from_processed() {
        let mut t = ProgressTracker::new();
        assert!(t.feed(RUNNING));
        let p = t.progress();
        // 4.4 GB hashed + 2.1 GB cached, of which only 1.9 GB left the machine.
        assert_eq!(p.bytes_processed, 6_500_000_000);
        assert_eq!(p.bytes_uploaded, 1_900_000_000);
        assert_eq!(p.files_processed, 15316 + 1201);
        assert_eq!(p.files_cached, 1201);
        assert_eq!(p.bytes_total, Some(11_200_000_000));
        assert_eq!(p.estimated_seconds_remaining, Some(130));
    }

    #[test]
    fn tracker_computes_a_rate_between_samples() {
        let mut t = ProgressTracker::new();
        let t0 = Instant::now();
        let a = ProgressLine { hashed_bytes: 0, ..ProgressLine::default() };
        t.apply(&a, t0);
        let b = ProgressLine { hashed_bytes: 10_000_000, ..ProgressLine::default() };
        t.apply(&b, t0 + Duration::from_secs(1));
        assert!(t.progress().bytes_per_second > 0.0, "no rate computed");
    }

    #[test]
    fn finalise_overrides_rounded_progress() {
        let mut t = ProgressTracker::new();
        t.feed(RUNNING);
        t.finalise(16_517, 6_543_210_987, 4);
        let p = t.progress();
        assert_eq!(p.bytes_processed, 6_543_210_987);
        assert_eq!(p.files_processed, 16_517);
        assert_eq!(p.errors_ignored, 4);
        assert_eq!(p.fraction(), Some(1.0));
    }

    #[test]
    fn parses_restore_progress() {
        let line = "Processed 812 (1.9 GB) of 4021 (6.5 GB), skipped 3 (1 KB), ignored 2 errors 41.2 MB/s (29.2%) remaining 1m50s.";
        let p = parse_restore_progress_line(line).expect("parses");
        assert_eq!(p.processed_entries, 812);
        assert_eq!(p.processed_bytes, 1_900_000_000);
        assert_eq!(p.total_entries, 4021);
        assert_eq!(p.total_bytes, 6_500_000_000);
        assert_eq!(p.skipped_entries, 3);
        assert_eq!(p.ignored_errors, 2);
        assert_eq!(p.bytes_per_second, Some(41_200_000.0));
        assert_eq!(p.percent_complete, Some(29.2));
        assert_eq!(p.time_remaining, Some(Duration::from_secs(110)));
    }

    #[test]
    fn parses_minimal_restore_progress() {
        let p = parse_restore_progress_line("Processed 5 (1 KB) of 100 (2 MB).").expect("parses");
        assert_eq!(p.processed_entries, 5);
        assert_eq!(p.total_bytes, 2_000_000);
        assert_eq!(p.bytes_per_second, None);
        assert!(parse_restore_progress_line("Restoring to local filesystem...").is_none());
    }

    #[test]
    fn tracker_handles_both_renderers() {
        let mut t = ProgressTracker::new();
        assert!(t.feed("Processed 5 (1 KB) of 100 (2 MB)."));
        assert_eq!(t.progress().bytes_total, Some(2_000_000));
        assert!(t.feed(RUNNING));
        assert_eq!(t.progress().bytes_uploaded, 1_900_000_000);
    }

    #[test]
    fn malformed_input_never_panics() {
        for line in [
            " | 1 hashing, 1 hashed (",
            " * hashing, hashed (), cached (), uploaded ",
            " | 9999999999999999999999 hashing, 1 hashed (1 B), 0 cached (0 B), uploaded 1 B",
            " | 0 hashing, 0 hashed (1e400 GB), 0 cached (0 B), uploaded 0 B",
            "()()()()",
        ] {
            let _ = parse_progress_line(line);
        }
    }

    // -- repository sync-to -------------------------------------------------

    // The shapes `outputSyncProgress` builds in
    // `cli/command_repository_sync.go`, including the trailing padding it uses
    // to wipe a longer previous line.
    const SYNC_INVENTORY: &str =
        "  Found 41230 BLOBs (88.1 GB) in the source repository, 512 (1.2 GB) to copy";
    const SYNC_COPYING: &str = "  Copied 214 blobs (612.4 MB), Speed: 18.1 MB/s, ETA: 42s     ";

    #[test]
    fn the_sync_inventory_line_yields_the_denominator() {
        let line = parse_sync_progress_line(SYNC_INVENTORY).expect("parses");
        assert_eq!(line.blobs_to_copy, Some(512));
        assert_eq!(line.bytes_to_copy, Some(1_200_000_000));
        assert_eq!(line.blobs_copied, None);
    }

    #[test]
    fn the_sync_copying_line_yields_the_numerator_rate_and_eta() {
        let line = parse_sync_progress_line(SYNC_COPYING).expect("parses");
        assert_eq!(line.blobs_copied, Some(214));
        assert_eq!(line.bytes_copied, Some(612_400_000));
        assert_eq!(line.bytes_per_second, Some(18_100_000.0));
        assert_eq!(line.time_remaining, Some(Duration::from_secs(42)));
    }

    #[test]
    fn a_sync_copying_line_without_an_eta_still_parses() {
        // Before the first sample kopia has neither a rate nor an estimate.
        let line =
            parse_sync_progress_line("  Copied 0 blobs (0 B), Speed: -, ETA: -").expect("parses");
        assert_eq!(line.blobs_copied, Some(0));
        assert_eq!(line.bytes_copied, Some(0));
        assert_eq!(line.bytes_per_second, None);
        assert_eq!(line.time_remaining, None);
    }

    #[test]
    fn the_other_sync_lines_are_not_mistaken_for_progress() {
        for line in [
            "Synchronizing repositories:",
            "  Found 100 BLOBs in the destination repository (1.2 GB)",
            "  Found 3 BLOBs to delete (1 KB), 97 in sync (1.1 GB)",
            "NOTE: By default no BLOBs are deleted, pass --delete to allow it.",
            "Copying...",
        ] {
            assert!(parse_sync_progress_line(line).is_none(), "misparsed: {line}");
        }
    }

    #[test]
    fn the_tracker_folds_a_sync_into_progress() {
        let mut t = ProgressTracker::new();
        assert!(t.feed(SYNC_INVENTORY));
        assert_eq!(t.progress().files_total, Some(512));
        assert_eq!(t.progress().bytes_total, Some(1_200_000_000));

        assert!(t.feed(SYNC_COPYING));
        assert_eq!(t.progress().files_processed, 214);
        assert_eq!(t.progress().bytes_processed, 612_400_000);
        // Everything replicated crosses the wire; there is no local dedup win.
        assert_eq!(t.progress().bytes_uploaded, 612_400_000);
        assert_eq!(t.progress().estimated_seconds_remaining, Some(42));
        // The denominator survives: a "Copied" line carries none, and
        // recomputing it from the numerator would peg the bar at 100%.
        assert_eq!(t.progress().bytes_total, Some(1_200_000_000));
    }

    #[test]
    fn malformed_sync_input_never_panics() {
        for line in [
            "Copied ",
            "Copied x blobs (",
            "Copied 1 blobs (1 B), Speed: ",
            "Found  BLOBs (",
            "Found 1 BLOBs (1 B) in the source repository, ",
        ] {
            let _ = parse_sync_progress_line(line);
        }
    }
}
