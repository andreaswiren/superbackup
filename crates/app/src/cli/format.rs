//! Turning values into text a person can read at a glance.
//!
//! Everything here is deliberately ASCII. Human output goes to whatever
//! terminal the user happens to have, and on Windows that may still be a
//! console running code page 437, where an em dash or a box-drawing character
//! arrives as mojibake. A backup tool whose status output looks corrupted is a
//! backup tool nobody trusts, so the fallback marker is `-` and the elision
//! marker is `...`.
//!
//! Widths are counted in `char`s rather than grapheme clusters. That is exact
//! for the ASCII this module emits and for the paths and job names users
//! actually type; getting it right for combining marks would need a
//! dependency this crate does not have, and being one column out on a CJK path
//! is a cosmetic defect, not a correctness one.

use chrono::{DateTime, Datelike, Local, TimeZone, Utc};

/// What a column shows when there is no value. Not an em dash: see the module
/// note on console code pages.
pub const MISSING: &str = "-";

// ---------------------------------------------------------------------------
// Colour
// ---------------------------------------------------------------------------

/// One SGR colour, or none.
///
/// Stored as the escape body (`"31"`, `"1;33"`) rather than a full sequence so
/// that a cell's *plain* text can be measured for alignment and the escape
/// applied afterwards. Colouring before measuring is how aligned tables lose
/// their alignment the moment somebody turns colour on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Colour {
    Red,
    Green,
    Yellow,
    Blue,
    Cyan,
    Dim,
    Bold,
}

impl Colour {
    fn sgr(self) -> &'static str {
        match self {
            Colour::Red => "31",
            Colour::Green => "32",
            Colour::Yellow => "33",
            Colour::Blue => "34",
            Colour::Cyan => "36",
            Colour::Dim => "2",
            Colour::Bold => "1",
        }
    }
}

/// Whether this stream may carry escape sequences.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    pub colour: bool,
}

impl Style {
    pub const PLAIN: Style = Style { colour: false };

    pub fn paint(&self, colour: Colour, text: &str) -> String {
        if self.colour && !text.is_empty() {
            format!("\u{1b}[{}m{}\u{1b}[0m", colour.sgr(), text)
        } else {
            text.to_string()
        }
    }

    pub fn maybe(&self, colour: Option<Colour>, text: &str) -> String {
        match colour {
            Some(c) => self.paint(c, text),
            None => text.to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// Sizes, rates, durations
// ---------------------------------------------------------------------------

const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];

/// `842 MB`, `1.2 GB`, `0 B`.
///
/// Decimal (1000-based) because that is what a storage bill and a disk
/// manufacturer both use, and the destination the number describes is usually
/// somebody's S3 invoice.
pub fn bytes(n: u64) -> String {
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit + 1 < UNITS.len() {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        return format!("{n} B");
    }
    if value >= 100.0 {
        format!("{:.0} {}", value, UNITS[unit])
    } else {
        format!("{:.1} {}", value, UNITS[unit])
    }
}

pub fn opt_bytes(n: Option<u64>) -> String {
    n.map(bytes).unwrap_or_else(|| MISSING.to_string())
}

/// `18.2 MB/s`. Zero is rendered as a dash: a rate of exactly nothing is
/// almost always "not measured yet" rather than a genuine standstill.
pub fn rate(bytes_per_second: f64) -> String {
    if !bytes_per_second.is_finite() || bytes_per_second <= 0.0 {
        return MISSING.to_string();
    }
    format!("{}/s", bytes(bytes_per_second as u64))
}

/// `45s`, `4m 32s`, `2h 05m`, `3d 4h`.
pub fn duration_secs(total: i64) -> String {
    if total < 0 {
        return MISSING.to_string();
    }
    let s = total % 60;
    let m = (total / 60) % 60;
    let h = (total / 3600) % 24;
    let d = total / 86_400;
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m:02}m")
    } else if m > 0 {
        format!("{m}m {s:02}s")
    } else {
        format!("{s}s")
    }
}

pub fn opt_duration_secs(total: Option<i64>) -> String {
    total.map(duration_secs).unwrap_or_else(|| MISSING.to_string())
}

/// `62%`. Clamped, because a progress fraction that has drifted past 1.0 is a
/// daemon bug and printing `104%` helps nobody.
pub fn percent(fraction: f32) -> String {
    format!("{:.0}%", (fraction.clamp(0.0, 1.0) * 100.0))
}

// ---------------------------------------------------------------------------
// Times
// ---------------------------------------------------------------------------

/// `2 hours ago`, `in 20 minutes`, `yesterday 14:30`, `29 Aug 14:30`.
///
/// Relative for anything inside a day, absolute beyond it. "Three weeks ago"
/// reads friendly and answers nothing; a date answers "is that before or after
/// I deleted the folder?".
pub fn relative(then: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let delta = now.signed_duration_since(then);
    let secs = delta.num_seconds();

    if secs >= 0 {
        match secs {
            0..=44 => "just now".to_string(),
            45..=89 => "a minute ago".to_string(),
            90..=3599 => format!("{} minutes ago", (secs + 30) / 60),
            3600..=7199 => "an hour ago".to_string(),
            7200..=86_399 => format!("{} hours ago", secs / 3600),
            _ => absolute_local(then),
        }
    } else {
        let ahead = -secs;
        match ahead {
            0..=44 => "in a moment".to_string(),
            45..=89 => "in a minute".to_string(),
            90..=3599 => format!("in {} minutes", (ahead + 30) / 60),
            3600..=7199 => "in an hour".to_string(),
            7200..=86_399 => format!("in {} hours", ahead / 3600),
            _ => absolute_local(then),
        }
    }
}

pub fn opt_relative(then: Option<DateTime<Utc>>, now: DateTime<Utc>) -> String {
    then.map(|t| relative(t, now)).unwrap_or_else(|| MISSING.to_string())
}

/// `29 Aug 14:30`, or `29 Aug 2025 14:30` when the year is not this one.
pub fn absolute_local(t: DateTime<Utc>) -> String {
    let local = t.with_timezone(&Local);
    if local.year() == Local::now().year() {
        local.format("%-d %b %H:%M").to_string()
    } else {
        local.format("%-d %b %Y %H:%M").to_string()
    }
}

/// Full precision, for `--json`-adjacent human output where the exact instant
/// matters: snapshot listings, event logs.
pub fn timestamp_local(t: DateTime<Utc>) -> String {
    t.with_timezone(&Local).format("%Y-%m-%d %H:%M:%S").to_string()
}

pub fn opt_timestamp_local(t: Option<DateTime<Utc>>) -> String {
    t.map(timestamp_local).unwrap_or_else(|| MISSING.to_string())
}

/// Local wall-clock time from a naive local date-time, used by `--at` parsing
/// so that a user typing `2026-08-29T14:00` means their own 14:00.
pub fn local_to_utc(naive: chrono::NaiveDateTime) -> Option<DateTime<Utc>> {
    Local.from_local_datetime(&naive).single().map(|t| t.with_timezone(&Utc))
}

// ---------------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------------

pub fn width_of(s: &str) -> usize {
    s.chars().count()
}

/// Shorten to `max` columns, marking the cut. Never returns something longer
/// than `max`.
pub fn truncate(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if width_of(s) <= max {
        return s.to_string();
    }
    if max <= 3 {
        return s.chars().take(max).collect();
    }
    let keep: String = s.chars().take(max - 3).collect();
    format!("{keep}...")
}

/// Shorten a path from the *left*, keeping the tail. For a path the
/// distinguishing part is the end, so `...\src\cli\format.rs` is useful where
/// `C:\Users\andreas\wo...` is not.
pub fn truncate_path(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let n = width_of(s);
    if n <= max {
        return s.to_string();
    }
    if max <= 3 {
        return s.chars().take(max).collect();
    }
    let tail: String = s.chars().skip(n - (max - 3)).collect();
    format!("...{tail}")
}

/// `1 job` / `3 jobs`, without the "1 job(s)" that makes software look unfinished.
pub fn plural(n: usize, singular: &str, plural: &str) -> String {
    if n == 1 {
        format!("1 {singular}")
    } else {
        format!("{n} {plural}")
    }
}

// ---------------------------------------------------------------------------
// Tables
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
}

/// How a column behaves when the terminal is too narrow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shrink {
    /// Never shortened. Ids, statuses, sizes — a truncated size is a lie.
    Never,
    /// Shortened from the right, keeping the head. Names, messages.
    Tail,
    /// Shortened from the left, keeping the tail. Paths.
    Head,
}

#[derive(Debug, Clone)]
pub struct Column {
    pub title: String,
    pub align: Align,
    pub shrink: Shrink,
}

impl Column {
    pub fn new(title: &str) -> Column {
        Column { title: title.to_string(), align: Align::Left, shrink: Shrink::Never }
    }
    pub fn right(mut self) -> Column {
        self.align = Align::Right;
        self
    }
    pub fn flex(mut self) -> Column {
        self.shrink = Shrink::Tail;
        self
    }
    pub fn path(mut self) -> Column {
        self.shrink = Shrink::Head;
        self
    }
}

#[derive(Debug, Clone)]
pub struct Cell {
    pub text: String,
    pub colour: Option<Colour>,
}

impl Cell {
    pub fn new(text: impl Into<String>) -> Cell {
        Cell { text: text.into(), colour: None }
    }
    pub fn coloured(text: impl Into<String>, colour: Colour) -> Cell {
        Cell { text: text.into(), colour: Some(colour) }
    }
}

impl From<&str> for Cell {
    fn from(s: &str) -> Cell {
        Cell::new(s)
    }
}
impl From<String> for Cell {
    fn from(s: String) -> Cell {
        Cell::new(s)
    }
}

/// A fixed-column table that stays aligned when a value is missing and stays
/// readable when the terminal is narrow.
#[derive(Debug, Clone)]
pub struct Table {
    columns: Vec<Column>,
    rows: Vec<Vec<Cell>>,
    /// Printed instead of the table when there are no rows.
    empty_note: Option<String>,
}

/// Two spaces between columns. One is too tight to scan, three wastes the
/// width that the flexible column needs.
const GAP: usize = 2;
/// A column narrower than this shows nothing useful, so shrinking stops here
/// and the row is allowed to overflow rather than becoming a column of dots.
const MIN_FLEX: usize = 6;

impl Table {
    pub fn new(columns: Vec<Column>) -> Table {
        Table { columns, rows: Vec::new(), empty_note: None }
    }

    pub fn empty_note(mut self, note: impl Into<String>) -> Table {
        self.empty_note = Some(note.into());
        self
    }

    pub fn push(&mut self, cells: Vec<Cell>) {
        debug_assert_eq!(cells.len(), self.columns.len(), "row does not match the header");
        self.rows.push(cells);
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Render to lines. `width` is the usable terminal width; pass a large
    /// number for a pipe, where nothing should be truncated at all.
    pub fn render(&self, width: usize, style: Style) -> Vec<String> {
        if self.rows.is_empty() {
            return match &self.empty_note {
                Some(note) => vec![note.clone()],
                None => Vec::new(),
            };
        }

        let mut widths: Vec<usize> = self
            .columns
            .iter()
            .map(|c| if c.title.is_empty() { 0 } else { width_of(&c.title) })
            .collect();
        for row in &self.rows {
            for (i, cell) in row.iter().enumerate() {
                if let Some(w) = widths.get_mut(i) {
                    *w = (*w).max(width_of(&cell.text));
                }
            }
        }

        self.shrink_to_fit(&mut widths, width);

        let mut out = Vec::with_capacity(self.rows.len() + 1);
        let header: Vec<Cell> = self
            .columns
            .iter()
            .map(|c| Cell { text: c.title.to_uppercase(), colour: Some(Colour::Dim) })
            .collect();
        if self.columns.iter().any(|c| !c.title.is_empty()) {
            out.push(self.render_row(&header, &widths, style));
        }
        for row in &self.rows {
            out.push(self.render_row(row, &widths, style));
        }
        out
    }

    /// Take width off the shrinkable columns, widest first, until the row
    /// fits. Fixed columns are never touched, so a truncated table still shows
    /// every status and every size in full.
    fn shrink_to_fit(&self, widths: &mut [usize], width: usize) {
        let gaps = GAP * self.columns.len().saturating_sub(1);
        loop {
            let total: usize = widths.iter().sum::<usize>() + gaps;
            if total <= width {
                return;
            }
            let excess = total - width;
            let victim = self
                .columns
                .iter()
                .enumerate()
                .filter(|(i, c)| {
                    c.shrink != Shrink::Never && widths.get(*i).copied().unwrap_or(0) > MIN_FLEX
                })
                .max_by_key(|(i, _)| widths.get(*i).copied().unwrap_or(0));
            let Some((index, _)) = victim else { return };
            let current = widths[index];
            widths[index] = current.saturating_sub(excess).max(MIN_FLEX);
            if widths[index] == current {
                return;
            }
        }
    }

    fn render_row(&self, row: &[Cell], widths: &[usize], style: Style) -> String {
        let mut line = String::new();
        for (i, column) in self.columns.iter().enumerate() {
            let want = widths.get(i).copied().unwrap_or(0);
            let raw = row.get(i).map(|c| c.text.as_str()).unwrap_or("");
            let text = match column.shrink {
                Shrink::Never => raw.to_string(),
                Shrink::Tail => truncate(raw, want),
                Shrink::Head => truncate_path(raw, want),
            };
            let pad = want.saturating_sub(width_of(&text));
            let painted = style.maybe(row.get(i).and_then(|c| c.colour), &text);

            if column.align == Align::Right {
                line.push_str(&" ".repeat(pad));
                line.push_str(&painted);
            } else {
                line.push_str(&painted);
                // No trailing whitespace on the last column: it is invisible
                // on screen and noise in a diff of captured output.
                if i + 1 < self.columns.len() {
                    line.push_str(&" ".repeat(pad));
                }
            }
            if i + 1 < self.columns.len() {
                line.push_str(&" ".repeat(GAP));
            }
        }
        while line.ends_with(' ') {
            line.pop();
        }
        line
    }
}

/// A `[####----]` bar for a terminal. Never used on a pipe.
pub fn progress_bar(fraction: Option<f32>, width: usize) -> String {
    let inner = width.saturating_sub(2).max(1);
    match fraction {
        Some(f) => {
            let filled = ((f.clamp(0.0, 1.0) as f64) * inner as f64).round() as usize;
            format!("[{}{}]", "#".repeat(filled.min(inner)), "-".repeat(inner - filled.min(inner)))
        }
        // Estimating: no honest bar to draw, so draw none rather than one that
        // sits at zero and looks stuck.
        None => format!("[{}]", "?".repeat(inner)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn byte_sizes_read_the_way_a_person_says_them() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(999), "999 B");
        assert_eq!(bytes(1_000), "1.0 KB");
        assert_eq!(bytes(842_000_000), "842 MB");
        assert_eq!(bytes(1_200_000_000), "1.2 GB");
        assert_eq!(bytes(15_500_000_000_000), "15.5 TB");
    }

    #[test]
    fn durations_use_the_units_a_person_would() {
        assert_eq!(duration_secs(0), "0s");
        assert_eq!(duration_secs(45), "45s");
        assert_eq!(duration_secs(272), "4m 32s");
        assert_eq!(duration_secs(7500), "2h 05m");
        assert_eq!(duration_secs(280_000), "3d 5h");
        assert_eq!(duration_secs(-1), MISSING);
    }

    #[test]
    fn relative_times_switch_to_absolute_after_a_day() {
        let now = Utc::now();
        assert_eq!(relative(now, now), "just now");
        assert_eq!(relative(now - Duration::minutes(5), now), "5 minutes ago");
        assert_eq!(relative(now - Duration::hours(2), now), "2 hours ago");
        assert_eq!(relative(now + Duration::minutes(20), now), "in 20 minutes");
        // Two days back must not read "48 hours ago".
        let old = relative(now - Duration::days(2), now);
        assert!(!old.contains("ago"), "expected an absolute date, got {old}");
    }

    #[test]
    fn missing_values_do_not_collapse_a_column() {
        let mut t = Table::new(vec![Column::new("name").flex(), Column::new("size").right()]);
        t.push(vec![Cell::new("alpha"), Cell::new(bytes(1_200_000_000))]);
        t.push(vec![Cell::new("b"), Cell::new(MISSING)]);
        let lines = t.render(80, Style::PLAIN);
        assert_eq!(lines[0], "NAME     SIZE");
        assert_eq!(lines[1], "alpha  1.2 GB");
        assert_eq!(lines[2], "b           -");
    }

    #[test]
    fn a_narrow_terminal_shrinks_the_flexible_column_and_nothing_else() {
        let mut t = Table::new(vec![
            Column::new("job").flex(),
            Column::new("status"),
            Column::new("size").right(),
        ]);
        t.push(vec![
            Cell::new("a-very-long-job-name-indeed-yes"),
            Cell::new("Succeeded"),
            Cell::new("1.2 GB"),
        ]);
        let lines = t.render(34, Style::PLAIN);
        for line in &lines {
            assert!(width_of(line) <= 34, "line overflows: {line:?}");
        }
        assert!(lines[1].contains("Succeeded"), "status must survive: {:?}", lines[1]);
        assert!(lines[1].contains("1.2 GB"), "size must survive: {:?}", lines[1]);
        assert!(lines[1].contains("..."), "the name must be visibly cut: {:?}", lines[1]);
    }

    #[test]
    fn paths_are_cut_from_the_left_so_the_filename_survives() {
        assert_eq!(
            truncate_path("C:/Users/a/projects/app/src/main.rs", 20),
            "...s/app/src/main.rs"
        );
        assert_eq!(truncate("hello", 20), "hello");
        assert_eq!(truncate("hello world", 8), "hello...");
    }

    #[test]
    fn colour_never_changes_the_measured_width() {
        let plain = Cell::new("ok");
        let painted = Cell::coloured("ok", Colour::Green);
        let mut a = Table::new(vec![Column::new("x"), Column::new("y")]);
        a.push(vec![plain, Cell::new("z")]);
        let mut b = Table::new(vec![Column::new("x"), Column::new("y")]);
        b.push(vec![painted, Cell::new("z")]);
        let no_colour = b.render(80, Style::PLAIN);
        assert_eq!(a.render(80, Style::PLAIN), no_colour);
        let with_colour = b.render(80, Style { colour: true });
        assert!(with_colour[1].contains("\u{1b}[32m"));
        assert!(with_colour[1].ends_with('z'));
    }

    #[test]
    fn an_empty_table_says_so_instead_of_printing_a_bare_header() {
        let t = Table::new(vec![Column::new("name")]).empty_note("No jobs yet.");
        assert_eq!(t.render(80, Style::PLAIN), vec!["No jobs yet.".to_string()]);
    }
}
