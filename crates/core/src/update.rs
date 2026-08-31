//! Checking whether a newer superbackup has been released.
//!
//! Nothing here ever happens on its own. superbackup checks, shows the user
//! what changed, and installs only when they choose to.
//!
//! That distinction matters because this is the process holding the user's
//! repository keys and quite possibly running a backup right now. Replacing its
//! own binary underneath itself has no good failure mode: a half-written
//! executable, a scheduler restarted mid-snapshot, or a new version that cannot
//! open the vault the old one wrote. So the *decision* is always the user's,
//! and the *mechanics* are then made as safe as they can be:
//!
//! - the archive is verified against the `SHA256SUMS` published with the
//!   release, in memory, before anything is written where it could be run;
//! - an update is refused outright while a job is running;
//! - the outgoing executable is renamed aside rather than deleted, and the
//!   incoming one is probed with `--version` before the swap is final, so a
//!   build that will not start can be rolled back.
//!
//! The same reasoning governs Kopia updates ([`crate::model::UpdatePolicy`]
//! defaults to `Notify`); it applies with more force to the application itself.
//!
//! ## Privacy
//!
//! An update check is a network request, and it tells GitHub this machine's IP
//! address and roughly when superbackup is running. That is a real disclosure,
//! however small, so it is listed in `docs/compliance/PRIVACY.md` alongside
//! every other connection the binary can make, and it can be switched off.
//! Nothing about the installation is sent: no machine id, no configuration, no
//! job names, no telemetry of any kind. It is an unauthenticated GET of a
//! public releases list.

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// The repository releases are published from.
pub const DEFAULT_REPO: &str = "andreaswiren/superbackup";

/// How often to look, unless the user says otherwise.
///
/// Weekly. A backup tool is not a browser: there is no security benefit to
/// asking every few hours, and a check that is too frequent is just a heartbeat
/// the user did not ask to emit.
pub const DEFAULT_INTERVAL_DAYS: u32 = 7;

/// Give up quickly. A slow or unreachable GitHub must never delay startup or
/// hold a scheduler tick.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Refuse a response larger than this. The releases list is a few kilobytes;
/// anything vastly bigger is not something to parse into memory.
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

/// How superbackup looks for its own updates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SelfUpdateSettings {
    /// Check at all. On by default: a backup tool with a known bug that the
    /// user never hears about is worse than one that mentions a release.
    pub enabled: bool,
    /// Days between automatic checks.
    pub interval_days: u32,
    /// Include pre-releases (`0.1.0-rc.1` and the like).
    pub include_prereleases: bool,
    /// `owner/name` to query. Configurable so a fork or an internal mirror can
    /// be used; changing it moves where the machine phones home, so the
    /// interface should say so.
    pub repo: String,
    /// When the last check actually completed, successfully or not.
    pub last_check_at: Option<DateTime<Utc>>,
    /// The newest version seen, so the interface can keep showing it without
    /// re-checking, and so a notification is not repeated for one the user has
    /// already been told about.
    pub last_seen_version: Option<String>,
}

impl Default for SelfUpdateSettings {
    fn default() -> Self {
        SelfUpdateSettings {
            enabled: true,
            interval_days: DEFAULT_INTERVAL_DAYS,
            include_prereleases: false,
            repo: DEFAULT_REPO.to_string(),
            last_check_at: None,
            last_seen_version: None,
        }
    }
}

impl SelfUpdateSettings {
    /// Is an automatic check due?
    ///
    /// A manual "Check for updates" ignores this entirely — the user asked.
    pub fn is_due(&self, now: DateTime<Utc>) -> bool {
        if !self.enabled {
            return false;
        }
        match self.last_check_at {
            None => true,
            Some(last) => {
                // A clock that moved backwards (a correction, a VM restore)
                // must not park the next check in the far future.
                if last > now {
                    return true;
                }
                (now - last).num_days() >= self.interval_days.max(1) as i64
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Results
// ---------------------------------------------------------------------------

/// What a check found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum UpdateStatus {
    /// This build is the newest published release.
    UpToDate { current: String },
    /// Something newer exists.
    Available(ReleaseInfo),
    /// This build is *newer* than anything published — a local or CI build.
    /// Worth saying rather than reporting "up to date", which would be a lie
    /// about a build nobody can reproduce from a tag.
    Unreleased { current: String, newest_published: Option<String> },
    /// The check could not be completed. Never an error the caller must handle:
    /// failing to reach GitHub is not a problem with the user's backups.
    Failed { reason: String },
    /// Checking is switched off.
    Disabled,
}

impl UpdateStatus {
    pub fn newer_version(&self) -> Option<&str> {
        match self {
            UpdateStatus::Available(r) => Some(r.version.as_str()),
            _ => None,
        }
    }
}

/// A published release, reduced to what the interface shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseInfo {
    /// Semantic version with any leading `v` removed.
    pub version: String,
    pub tag: String,
    pub name: Option<String>,
    pub url: String,
    pub published_at: Option<DateTime<Utc>>,
    pub prerelease: bool,
    /// Release notes, truncated — the interface shows a summary and links out.
    pub notes: Option<String>,
}

/// Release notes are shown in a panel, not a document viewer.
const MAX_NOTES: usize = 4000;

// ---------------------------------------------------------------------------
// The check
// ---------------------------------------------------------------------------

/// The shape GitHub returns from `/releases`. Only the fields we use.
#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    published_at: Option<DateTime<Utc>>,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    body: Option<String>,
}

/// Compare a published release against the running build.
///
/// Split from the network call so the decision is testable without GitHub, and
/// so a malformed tag is handled the same way whether it came from the wire or
/// from a fixture.
pub fn evaluate(
    current_version: &str,
    releases: &[GithubReleaseView],
    include_prereleases: bool,
) -> UpdateStatus {
    let current = match semver::Version::parse(current_version) {
        Ok(v) => v,
        // The running build's own version is malformed, which is a packaging
        // fault rather than something the user can act on.
        Err(e) => return UpdateStatus::Failed { reason: format!("unreadable local version: {e}") },
    };

    let mut newest: Option<(semver::Version, &GithubReleaseView)> = None;
    for release in releases {
        if release.draft {
            continue;
        }
        if release.prerelease && !include_prereleases {
            continue;
        }
        let Some(parsed) = parse_tag(&release.tag) else { continue };
        if newest.as_ref().is_none_or(|(best, _)| parsed > *best) {
            newest = Some((parsed, release));
        }
    }

    let Some((newest_version, release)) = newest else {
        return UpdateStatus::UpToDate { current: current_version.to_string() };
    };

    match newest_version.cmp(&current) {
        std::cmp::Ordering::Greater => UpdateStatus::Available(ReleaseInfo {
            version: newest_version.to_string(),
            tag: release.tag.clone(),
            name: release.name.clone(),
            url: release.url.clone(),
            published_at: release.published_at,
            prerelease: release.prerelease,
            notes: release.notes.as_ref().map(|n| truncate(n, MAX_NOTES)),
        }),
        std::cmp::Ordering::Equal => {
            UpdateStatus::UpToDate { current: current_version.to_string() }
        }
        std::cmp::Ordering::Less => UpdateStatus::Unreleased {
            current: current_version.to_string(),
            newest_published: Some(newest_version.to_string()),
        },
    }
}

/// The parts of a release the comparison needs, independent of the HTTP client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubReleaseView {
    pub tag: String,
    pub name: Option<String>,
    pub url: String,
    pub published_at: Option<DateTime<Utc>>,
    pub prerelease: bool,
    pub draft: bool,
    pub notes: Option<String>,
}

fn parse_tag(tag: &str) -> Option<semver::Version> {
    semver::Version::parse(tag.strip_prefix('v').unwrap_or(tag)).ok()
}

fn truncate(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    // Cut on a character boundary, not a byte one.
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// Ask GitHub what has been released.
///
/// Returns `UpdateStatus::Failed` rather than an error for anything the network
/// does, because a failed update check is not a failure the user must act on.
/// A genuinely malformed request (a bad repository name) is an `Err`.
pub async fn check(settings: &SelfUpdateSettings, current_version: &str) -> Result<UpdateStatus> {
    if !settings.enabled {
        return Ok(UpdateStatus::Disabled);
    }
    if settings.repo.is_empty() || !settings.repo.contains('/') {
        return Err(Error::Validation(format!(
            "`{}` is not an owner/name repository",
            settings.repo
        )));
    }

    let url = format!("https://api.github.com/repos/{}/releases?per_page=20", settings.repo);
    let client = match reqwest::Client::builder().timeout(REQUEST_TIMEOUT).build() {
        Ok(c) => c,
        Err(e) => return Ok(UpdateStatus::Failed { reason: e.to_string() }),
    };

    let response = client
        .get(&url)
        // GitHub rejects requests without one.
        .header("User-Agent", format!("superbackup/{current_version}"))
        .header("Accept", "application/vnd.github+json")
        .send()
        .await;

    let response = match response {
        Ok(r) => r,
        Err(e) => {
            return Ok(UpdateStatus::Failed {
                reason: crate::redact::scrub(&e.to_string()).into_owned(),
            })
        }
    };

    if !response.status().is_success() {
        return Ok(UpdateStatus::Failed {
            reason: format!("GitHub answered {}", response.status()),
        });
    }

    let body = match response.bytes().await {
        Ok(b) if b.len() > MAX_RESPONSE_BYTES => {
            return Ok(UpdateStatus::Failed {
                reason: "the releases list was implausibly large".into(),
            })
        }
        Ok(b) => b,
        Err(e) => {
            return Ok(UpdateStatus::Failed {
                reason: crate::redact::scrub(&e.to_string()).into_owned(),
            })
        }
    };

    let parsed: Vec<GithubRelease> = match serde_json::from_slice(&body) {
        Ok(p) => p,
        Err(e) => return Ok(UpdateStatus::Failed { reason: format!("unreadable answer: {e}") }),
    };

    let views: Vec<GithubReleaseView> = parsed
        .into_iter()
        .map(|r| GithubReleaseView {
            tag: r.tag_name,
            name: r.name,
            url: r.html_url,
            published_at: r.published_at,
            prerelease: r.prerelease,
            draft: r.draft,
            notes: r.body,
        })
        .collect();

    Ok(evaluate(current_version, &views, settings.include_prereleases))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str, prerelease: bool) -> GithubReleaseView {
        GithubReleaseView {
            tag: tag.into(),
            name: Some(tag.into()),
            url: format!("https://example.invalid/{tag}"),
            published_at: None,
            prerelease,
            draft: false,
            notes: None,
        }
    }

    #[test]
    fn a_newer_release_is_reported() {
        let out = evaluate("0.1.0", &[release("v0.2.0", false)], false);
        assert_eq!(out.newer_version(), Some("0.2.0"));
    }

    #[test]
    fn the_same_version_is_up_to_date() {
        let out = evaluate("0.1.0", &[release("v0.1.0", false)], false);
        assert!(matches!(out, UpdateStatus::UpToDate { .. }));
    }

    #[test]
    fn prereleases_are_ignored_unless_asked_for() {
        let releases = [release("v0.2.0-rc.1", true), release("v0.1.0", false)];
        assert!(matches!(evaluate("0.1.0", &releases, false), UpdateStatus::UpToDate { .. }));
        assert_eq!(evaluate("0.1.0", &releases, true).newer_version(), Some("0.2.0-rc.1"));
    }

    #[test]
    fn drafts_are_never_offered() {
        let mut draft = release("v9.9.9", false);
        draft.draft = true;
        // The release workflow opens a draft and waits for a human, so a draft
        // is explicitly not something to tell users about.
        assert!(matches!(evaluate("0.1.0", &[draft], false), UpdateStatus::UpToDate { .. }));
    }

    #[test]
    fn a_local_build_ahead_of_every_release_says_so() {
        let out = evaluate("0.2.0", &[release("v0.1.0", false)], false);
        match out {
            UpdateStatus::Unreleased { current, newest_published } => {
                assert_eq!(current, "0.2.0");
                assert_eq!(newest_published.as_deref(), Some("0.1.0"));
            }
            other => panic!("expected Unreleased, got {other:?}"),
        }
    }

    #[test]
    fn the_newest_wins_regardless_of_list_order() {
        let releases =
            [release("v0.1.0", false), release("v0.9.0", false), release("v0.3.0", false)];
        assert_eq!(evaluate("0.1.0", &releases, false).newer_version(), Some("0.9.0"));
    }

    #[test]
    fn an_unparseable_tag_is_skipped_not_fatal() {
        let releases = [release("nightly", false), release("v0.4.0", false)];
        assert_eq!(evaluate("0.1.0", &releases, false).newer_version(), Some("0.4.0"));
    }

    #[test]
    fn no_releases_at_all_is_up_to_date_not_an_error() {
        assert!(matches!(evaluate("0.1.0", &[], false), UpdateStatus::UpToDate { .. }));
    }

    #[test]
    fn a_weekly_check_is_due_after_seven_days() {
        let now = Utc::now();
        let mut s = SelfUpdateSettings::default();
        assert!(s.is_due(now), "a machine that has never checked is due");

        s.last_check_at = Some(now - chrono::Duration::days(3));
        assert!(!s.is_due(now));

        s.last_check_at = Some(now - chrono::Duration::days(8));
        assert!(s.is_due(now));
    }

    #[test]
    fn checking_can_be_switched_off() {
        let now = Utc::now();
        let s = SelfUpdateSettings { enabled: false, ..SelfUpdateSettings::default() };
        assert!(!s.is_due(now));
    }

    #[test]
    fn a_clock_that_moved_backwards_does_not_park_the_next_check() {
        // A VM restored from a snapshot, or a corrected clock, must not leave
        // the machine never checking again.
        let now = Utc::now();
        let s = SelfUpdateSettings {
            last_check_at: Some(now + chrono::Duration::days(400)),
            ..SelfUpdateSettings::default()
        };
        assert!(s.is_due(now));
    }

    #[test]
    fn notes_are_truncated_on_a_character_boundary() {
        let long = "é".repeat(5000);
        let out = truncate(&long, MAX_NOTES);
        assert!(out.len() <= MAX_NOTES + 4);
        assert!(out.ends_with('…'));
    }
}

// ---------------------------------------------------------------------------
// Downloading and applying an update
// ---------------------------------------------------------------------------

/// Applying an update replaces the executable that is *currently running* and
/// holds the user's repository keys. The rules below are not negotiable, and
/// each exists because of a specific way this goes wrong:
///
/// 1. **Never while a job is running.** A snapshot interrupted by its own
///    binary being swapped is a corrupt-looking repository and a support case
///    nobody can reconstruct.
/// 2. **Verify before anything touches disk.** The archive is checked against
///    the `SHA256SUMS` published with the release, in memory, before a single
///    byte is written where the resolver could find it.
/// 3. **Keep the old binary.** The outgoing executable is renamed aside rather
///    than deleted, so a new build that will not start can be rolled back by
///    hand — or automatically, because the new one is run with `--version`
///    before the swap is considered final.
/// 4. **Nothing is automatic.** The user reads the release notes and presses
///    the button. This module never decides on its own to replace itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetChoice {
    pub file_name: String,
    pub url: String,
    /// True when no build exists for this exact architecture and a compatible
    /// one was chosen instead (Windows on ARM running the x64 build).
    pub emulated: bool,
}

/// The archive naming the release workflow produces:
/// `superbackup-<version>-<target-triple>.<zip|tar.gz>`.
pub fn target_triple() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => "x86_64-pc-windows-msvc",
        // Windows on ARM emulates x64; a native build is not published.
        ("windows", "aarch64") => "x86_64-pc-windows-msvc",
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        _ => "",
    }
}

/// Pick the archive for this machine out of a release's asset list.
pub fn select_asset(assets: &[(String, String)], version: &str) -> Option<AssetChoice> {
    let triple = target_triple();
    if triple.is_empty() {
        return None;
    }
    let ext = if cfg!(windows) { "zip" } else { "tar.gz" };
    let wanted = format!("superbackup-{version}-{triple}.{ext}");
    assets.iter().find(|(name, _)| name == &wanted).map(|(name, url)| AssetChoice {
        file_name: name.clone(),
        url: url.clone(),
        emulated: cfg!(windows) && std::env::consts::ARCH == "aarch64",
    })
}

/// Parse a `sha256sum`-style manifest into `(file name, lowercase hex digest)`.
///
/// The release publishes one `SHA256SUMS` covering every archive, so a single
/// fetch verifies whichever one this machine needs.
pub fn parse_checksums(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let digest = parts.next()?;
            let name = parts.next()?;
            if digest.len() != 64 || !digest.chars().all(|c| c.is_ascii_hexdigit()) {
                return None;
            }
            // `sha256sum` writes "./name" for a relative path.
            let name = name.trim_start_matches('*').trim_start_matches("./");
            Some((name.to_string(), digest.to_ascii_lowercase()))
        })
        .collect()
}

/// Confirm an archive matches the digest published for it.
///
/// Returns the reason on failure rather than a bare bool: "the download did not
/// match its published checksum" is something the user must be told verbatim,
/// because the honest interpretation is either a corrupted download or a
/// tampered one, and both mean *do not install this*.
pub fn verify_archive(
    archive: &[u8],
    file_name: &str,
    checksums: &[(String, String)],
) -> std::result::Result<(), String> {
    use sha2::{Digest, Sha256};

    let Some((_, expected)) = checksums.iter().find(|(n, _)| n == file_name) else {
        return Err(format!("{file_name} is not listed in SHA256SUMS"));
    };
    let actual = hex::encode(Sha256::digest(archive));
    if actual == *expected {
        Ok(())
    } else {
        Err(format!(
            "{file_name} did not match its published checksum (expected {expected}, got {actual})"
        ))
    }
}

#[cfg(test)]
mod apply_tests {
    use super::*;

    #[test]
    fn checksums_parse_from_the_usual_formats() {
        let text = "\
abcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabca  superbackup-0.2.0-x86_64-pc-windows-msvc.zip
defdefdefdefdefdefdefdefdefdefdefdefdefdefdefdefdefdefdefdefdefd  ./superbackup-0.2.0-x86_64-unknown-linux-gnu.tar.gz
not-a-digest  ignored.zip
";
        let parsed = parse_checksums(text);
        assert_eq!(parsed.len(), 2, "a malformed line must be skipped, not fatal");
        assert_eq!(parsed[1].0, "superbackup-0.2.0-x86_64-unknown-linux-gnu.tar.gz");
    }

    #[test]
    fn an_archive_that_does_not_match_is_refused_by_name() {
        let sums = parse_checksums(
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  a.zip\n",
        );
        // The digest above is of the empty input, so empty verifies.
        assert!(verify_archive(b"", "a.zip", &sums).is_ok());
        let err = verify_archive(b"tampered", "a.zip", &sums).unwrap_err();
        assert!(err.contains("did not match its published checksum"), "{err}");
    }

    #[test]
    fn an_unlisted_archive_is_refused_rather_than_trusted() {
        let sums = parse_checksums(
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  a.zip\n",
        );
        let err = verify_archive(b"", "b.zip", &sums).unwrap_err();
        assert!(err.contains("not listed"), "{err}");
    }

    #[test]
    fn the_asset_for_this_machine_is_chosen_by_exact_name() {
        let triple = target_triple();
        if triple.is_empty() {
            return; // unsupported platform; nothing to assert
        }
        let ext = if cfg!(windows) { "zip" } else { "tar.gz" };
        let wanted = format!("superbackup-0.2.0-{triple}.{ext}");
        let assets = vec![
            ("superbackup-0.2.0-some-other-target.tar.gz".to_string(), "u1".to_string()),
            (wanted.clone(), "u2".to_string()),
        ];
        let chosen = select_asset(&assets, "0.2.0").expect("this platform must match an asset");
        assert_eq!(chosen.file_name, wanted);
        assert_eq!(chosen.url, "u2");
    }

    #[test]
    fn no_matching_asset_is_none_rather_than_a_wrong_one() {
        let assets = vec![("superbackup-0.2.0-nonsense-target.zip".to_string(), "u".to_string())];
        assert!(
            select_asset(&assets, "0.2.0").is_none(),
            "a release without a build for this machine must not install some other machine's"
        );
    }
}
