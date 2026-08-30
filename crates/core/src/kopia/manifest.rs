//! Kopia's JSON shapes, mirrored exactly.
//!
//! Every field name below is the Go `json:` tag from kopia's own source, so a
//! field rename in kopia shows up as a `None`/default here rather than as a
//! parse failure. Everything is `#[serde(default)]` for the same reason: a
//! backup that succeeded must never be reported as failed because a future
//! kopia added or dropped a field.
//!
//! Sources (kopia `master`, verified against the 0.21 line):
//! * `snapshot/manifest.go` — `Manifest`, `DirEntry`, `DirManifest`,
//!   `StorageStats`.
//! * `snapshot/stats.go` — `Stats`.
//! * `snapshot/source.go` — `SourceInfo`.
//! * `fs/entry.go` — `DirectorySummary`.
//!
//! ## The `--json-verbose` trap
//!
//! `cli/json_output.go` strips `stats` from the emitted manifest **unless**
//! `--json-verbose` is passed. Without it, `snapshot create --json` reports a
//! snapshot id and nothing about how big it was. The driver therefore always
//! passes both flags, and every consumer here treats `stats` as optional so
//! that a kopia which drops the (hidden) flag degrades to "we know the id"
//! rather than failing the run.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// `snapshot.SourceInfo` — which machine and path a snapshot came from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceInfo {
    #[serde(default)]
    pub host: String,
    #[serde(default, rename = "userName")]
    pub user_name: String,
    #[serde(default)]
    pub path: String,
}

impl std::fmt::Display for SourceInfo {
    /// Reproduces kopia's own `user@host:path` rendering, which is also the
    /// syntax `policy set` and `snapshot list` accept as a target.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.host.is_empty() && self.path.is_empty() && self.user_name.is_empty() {
            return f.write_str("(global)");
        }
        if self.path.is_empty() {
            return write!(f, "{}@{}", self.user_name, self.host);
        }
        write!(f, "{}@{}:{}", self.user_name, self.host, self.path)
    }
}

/// `snapshot.Stats` — the exact counters for a finished snapshot.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotStats {
    #[serde(default, rename = "totalSize")]
    pub total_size: u64,
    #[serde(default, rename = "excludedTotalSize")]
    pub excluded_total_size: u64,
    #[serde(default, rename = "fileCount")]
    pub file_count: u64,
    #[serde(default, rename = "cachedFiles")]
    pub cached_files: u64,
    #[serde(default, rename = "nonCachedFiles")]
    pub non_cached_files: u64,
    #[serde(default, rename = "dirCount")]
    pub dir_count: u64,
    #[serde(default, rename = "excludedFileCount")]
    pub excluded_file_count: u64,
    #[serde(default, rename = "excludedDirCount")]
    pub excluded_dir_count: u64,
    #[serde(default, rename = "ignoredErrorCount")]
    pub ignored_error_count: u64,
    #[serde(default, rename = "errorCount")]
    pub error_count: u64,
}

/// `fs.DirectorySummary` — the rolled-up totals kopia attaches to a directory.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DirectorySummary {
    #[serde(default, rename = "size")]
    pub total_file_size: u64,
    #[serde(default, rename = "files")]
    pub total_file_count: u64,
    #[serde(default, rename = "symlinks")]
    pub total_symlink_count: u64,
    #[serde(default, rename = "dirs")]
    pub total_dir_count: u64,
    #[serde(default, rename = "maxTime")]
    pub max_mod_time: Option<DateTime<Utc>>,
    #[serde(default)]
    pub incomplete: String,
    #[serde(default, rename = "numFailed")]
    pub fatal_error_count: u64,
    #[serde(default, rename = "numIgnoredErrors")]
    pub ignored_error_count: u64,
    #[serde(default, rename = "errors")]
    pub failed_entries: Vec<EntryError>,
}

/// One entry kopia could not read, with kopia's own message.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryError {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub error: String,
}

/// `snapshot.EntryType` — `f`ile, `d`irectory or `s`ymlink.
///
/// Deserialised through `String` rather than with `#[serde(rename)]` so that a
/// type kopia adds later becomes [`EntryType::Unknown`] instead of failing the
/// whole manifest. A restore browser that cannot show one odd row is a bug; one
/// that cannot open the snapshot at all is a disaster.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum EntryType {
    #[default]
    File,
    Directory,
    Symlink,
    Unknown,
}

impl EntryType {
    pub fn is_dir(&self) -> bool {
        matches!(self, EntryType::Directory)
    }
    pub fn as_kopia_str(&self) -> &'static str {
        match self {
            EntryType::File => "f",
            EntryType::Directory => "d",
            EntryType::Symlink => "s",
            EntryType::Unknown => "",
        }
    }
}

impl From<String> for EntryType {
    fn from(s: String) -> Self {
        match s.as_str() {
            "f" => EntryType::File,
            "d" => EntryType::Directory,
            "s" => EntryType::Symlink,
            _ => EntryType::Unknown,
        }
    }
}

impl From<EntryType> for String {
    fn from(t: EntryType) -> String {
        t.as_kopia_str().to_string()
    }
}

/// `snapshot.DirEntry` — one row in the GUI's restore browser.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DirEntry {
    #[serde(default)]
    pub name: String,
    #[serde(default, rename = "type")]
    pub entry_type: EntryType,
    /// Unix mode as an octal string (`"0644"`). Absent on Windows sources.
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default, rename = "size")]
    pub size: u64,
    #[serde(default, rename = "mtime")]
    pub modified_at: Option<DateTime<Utc>>,
    /// The kopia object id. This is what `restore` and `show` address.
    #[serde(default, rename = "obj")]
    pub object_id: String,
    #[serde(default, rename = "summ")]
    pub summary: Option<DirectorySummary>,
}

/// `snapshot.DirManifest` — the raw contents of a directory object, which is
/// what `kopia show <object-id>` prints.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DirManifest {
    #[serde(default, rename = "stream")]
    pub stream_type: String,
    #[serde(default)]
    pub entries: Vec<DirEntry>,
    #[serde(default)]
    pub summary: Option<DirectorySummary>,
}

/// `snapshot.Manifest` — one point-in-time snapshot.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SnapshotManifest {
    /// Kopia's manifest id. This is the `snapshot_id` the run history records
    /// and the handle `snapshot delete` and `restore` take.
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub source: SourceInfo,
    #[serde(default)]
    pub description: String,
    #[serde(default, rename = "startTime")]
    pub start_time: Option<DateTime<Utc>>,
    #[serde(default, rename = "endTime")]
    pub end_time: Option<DateTime<Utc>>,
    /// Absent unless `--json-verbose` was passed. See the module docs.
    #[serde(default)]
    pub stats: Option<SnapshotStats>,
    /// Non-empty when the snapshot was checkpointed rather than completed.
    #[serde(default, rename = "incomplete")]
    pub incomplete_reason: String,
    #[serde(default, rename = "rootEntry")]
    pub root_entry: Option<DirEntry>,
    #[serde(default)]
    pub tags: BTreeMap<String, String>,
    #[serde(default)]
    pub pins: Vec<String>,
}

impl SnapshotManifest {
    /// The object id of the snapshot root, which is what the restore browser
    /// walks and what `kopia restore` takes as a source.
    pub fn root_object_id(&self) -> Option<&str> {
        self.root_entry.as_ref().map(|e| e.object_id.as_str()).filter(|s| !s.is_empty())
    }

    pub fn is_complete(&self) -> bool {
        self.incomplete_reason.is_empty()
    }

    /// Files and bytes as kopia finally counted them. `None` when `stats` was
    /// stripped from the JSON, in which case the caller keeps the numbers it
    /// scraped from the progress line.
    pub fn totals(&self) -> Option<(u64, u64)> {
        self.stats.as_ref().map(|s| (s.file_count, s.total_size))
    }

    /// Bytes that never had to be sent because kopia already had them. The GUI
    /// shows this as the dedup win.
    pub fn deduplicated_bytes(&self, uploaded: u64) -> u64 {
        self.stats.as_ref().map(|s| s.total_size.saturating_sub(uploaded)).unwrap_or(0)
    }

    /// Human-readable warnings for [`crate::state::DestinationRun::warnings`].
    ///
    /// Deliberately capped and deliberately specific: "37 files could not be
    /// read" plus the first few paths is actionable; 37 000 log lines are not.
    pub fn warnings(&self) -> Vec<String> {
        const MAX_EXAMPLES: usize = 5;
        let mut out = Vec::new();

        if !self.incomplete_reason.is_empty() {
            out.push(format!(
                "Snapshot is a partial checkpoint, not a complete backup ({}).",
                self.incomplete_reason
            ));
        }

        if let Some(stats) = &self.stats {
            if stats.ignored_error_count > 0 {
                out.push(plural_files(
                    stats.ignored_error_count,
                    "could not be read and was skipped",
                    "could not be read and were skipped",
                ));
            }
            if stats.error_count > 0 {
                out.push(plural_files(
                    stats.error_count,
                    "failed with an unrecoverable error",
                    "failed with unrecoverable errors",
                ));
            }
            if stats.excluded_file_count > 0 {
                out.push(format!(
                    "{} file(s) and {} directory(ies) were excluded by the ignore rules ({} bytes).",
                    stats.excluded_file_count,
                    stats.excluded_dir_count,
                    stats.excluded_total_size
                ));
            }
        }

        if let Some(summary) = self.root_entry.as_ref().and_then(|e| e.summary.as_ref()) {
            for entry in summary.failed_entries.iter().take(MAX_EXAMPLES) {
                out.push(format!("{}: {}", entry.path, entry.error));
            }
            if summary.failed_entries.len() > MAX_EXAMPLES {
                out.push(format!(
                    "… and {} more unreadable entries.",
                    summary.failed_entries.len() - MAX_EXAMPLES
                ));
            }
        }

        out
    }
}

fn plural_files(n: u64, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 file {one}.")
    } else {
        format!("{n} files {many}.")
    }
}

/// Read a stream of concatenated JSON values.
///
/// `snapshot create --json` prints one indented object **per source** with no
/// enclosing array and no separator, so `serde_json::from_str` on the whole
/// buffer fails as soon as a second object appears. This reads them all.
/// Malformed trailing output (a warning kopia printed to stdout, say) stops the
/// stream instead of discarding the values already read.
pub fn parse_json_stream<T: serde::de::DeserializeOwned>(text: &str) -> Vec<T> {
    let mut out = Vec::new();
    let stream = serde_json::Deserializer::from_str(text).into_iter::<T>();
    for value in stream {
        match value {
            Ok(v) => out.push(v),
            Err(_) => break,
        }
    }
    out
}

/// Parse `kopia snapshot list --json`, which emits a real JSON array.
pub fn parse_snapshot_list(text: &str) -> Vec<SnapshotManifest> {
    match serde_json::from_str::<Vec<SnapshotManifest>>(text.trim()) {
        Ok(v) => v,
        // `--json` on an empty repository prints `[\n]`, which parses; a
        // version that instead printed a bare stream would land here.
        Err(_) => parse_json_stream(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Trimmed from real `kopia snapshot create --json --json-verbose` output.
    const CREATE_JSON: &str = r#"{
  "id": "k9f3a1b2c3d4e5f60718293a4b5c6d7e",
  "source": {
    "host": "workstation",
    "userName": "andreas",
    "path": "C:\\src\\superbackup"
  },
  "description": "",
  "startTime": "2026-08-30T09:15:02.331Z",
  "endTime": "2026-08-30T09:17:44.902Z",
  "stats": {
    "totalSize": 6543210987,
    "excludedTotalSize": 918273645,
    "fileCount": 16517,
    "cachedFiles": 1201,
    "nonCachedFiles": 15316,
    "dirCount": 2204,
    "excludedFileCount": 40311,
    "excludedDirCount": 812,
    "ignoredErrorCount": 4
  },
  "rootEntry": {
    "name": "superbackup",
    "type": "d",
    "mode": "0755",
    "mtime": "2026-08-30T09:15:00Z",
    "obj": "kb1f2e3d4c5b6a798",
    "summ": {
      "size": 6543210987,
      "files": 16517,
      "symlinks": 0,
      "dirs": 2204,
      "maxTime": "2026-08-30T09:14:59Z",
      "numFailed": 0,
      "numIgnoredErrors": 4,
      "errors": [
        {"path": "C:\\src\\superbackup\\target\\lock", "error": "access is denied"},
        {"path": "C:\\src\\superbackup\\.git\\index.lock", "error": "access is denied"}
      ]
    }
  }
}
"#;

    #[test]
    fn parses_a_real_create_manifest() {
        let m: SnapshotManifest = serde_json::from_str(CREATE_JSON).expect("manifest parses");
        assert_eq!(m.id, "k9f3a1b2c3d4e5f60718293a4b5c6d7e");
        assert_eq!(m.source.host, "workstation");
        assert_eq!(m.source.path, "C:\\src\\superbackup");
        assert_eq!(m.root_object_id(), Some("kb1f2e3d4c5b6a798"));
        assert!(m.is_complete());
        assert_eq!(m.totals(), Some((16517, 6_543_210_987)));
        assert_eq!(m.deduplicated_bytes(1_900_000_000), 4_643_210_987);
    }

    #[test]
    fn warnings_are_specific_and_capped() {
        let m: SnapshotManifest = serde_json::from_str(CREATE_JSON).expect("manifest parses");
        let w = m.warnings();
        assert!(w.iter().any(|s| s.contains("4 files")), "{w:?}");
        assert!(w.iter().any(|s| s.contains("40311")), "{w:?}");
        assert!(w.iter().any(|s| s.contains("index.lock")), "{w:?}");
        assert!(w.len() < 20);
    }

    #[test]
    fn manifest_without_stats_still_yields_the_id() {
        // What `--json` alone produces: json_output.go strips `stats`.
        let json = r#"{"id":"kabc","source":{"host":"h","userName":"u","path":"/p"},
            "startTime":"2026-01-01T00:00:00Z","endTime":"2026-01-01T00:01:00Z",
            "rootEntry":{"name":"p","type":"d","obj":"k1"}}"#;
        let m: SnapshotManifest = serde_json::from_str(json).expect("parses");
        assert_eq!(m.id, "kabc");
        assert_eq!(m.totals(), None);
        assert!(m.warnings().is_empty());
    }

    #[test]
    fn unknown_fields_and_types_do_not_break_parsing() {
        let json = r#"{"id":"k1","brandNewFieldFromTheFuture":42,
            "rootEntry":{"name":"x","type":"q","obj":"k2"}}"#;
        let m: SnapshotManifest = serde_json::from_str(json).expect("parses");
        assert_eq!(m.id, "k1");
        assert_eq!(m.root_entry.map(|e| e.entry_type), Some(EntryType::Unknown));
    }

    #[test]
    fn concatenated_manifests_are_all_read() {
        let two = format!("{CREATE_JSON}{CREATE_JSON}");
        let v: Vec<SnapshotManifest> = parse_json_stream(&two);
        assert_eq!(v.len(), 2);
    }

    #[test]
    fn snapshot_list_array_parses() {
        let list = format!("[\n {},\n {}\n]", CREATE_JSON.trim(), CREATE_JSON.trim());
        let v = parse_snapshot_list(&list);
        assert_eq!(v.len(), 2);
        assert_eq!(parse_snapshot_list("[\n]").len(), 0);
    }

    #[test]
    fn directory_manifest_from_kopia_show_parses() {
        let json = r#"{"stream":"kopia:directory","entries":[
            {"name":"Cargo.toml","type":"f","mode":"0644","size":774,
             "mtime":"2026-08-30T21:17:00Z","obj":"Ic1d2"},
            {"name":"crates","type":"d","mode":"0755","mtime":"2026-08-30T21:16:00Z",
             "obj":"kf00d","summ":{"size":123,"files":9,"dirs":3,"numFailed":0}}
          ],"summary":{"size":897,"files":10,"dirs":4,"numFailed":0}}"#;
        let d: DirManifest = serde_json::from_str(json).expect("dir manifest parses");
        assert_eq!(d.entries.len(), 2);
        assert!(!d.entries[0].entry_type.is_dir());
        assert!(d.entries[1].entry_type.is_dir());
        assert_eq!(d.entries[1].object_id, "kf00d");
        assert_eq!(d.summary.map(|s| s.total_file_count), Some(10));
    }

    #[test]
    fn source_info_renders_like_kopia() {
        let s = SourceInfo {
            host: "workstation".into(),
            user_name: "andreas".into(),
            path: "C:\\src".into(),
        };
        assert_eq!(s.to_string(), "andreas@workstation:C:\\src");
        assert_eq!(SourceInfo::default().to_string(), "(global)");
    }
}
