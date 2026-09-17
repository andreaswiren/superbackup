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

use std::path::{Path, PathBuf};
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
    // The same client the kopia installer uses, and for the same reason: its
    // redirect policy refuses to follow a redirect off GitHub, so no request is
    // ever issued to a foreign host — not even a connection that would leak the
    // fact that this machine is checking for an update. A plain
    // `Client::builder()` here would have followed one anywhere.
    let client = match crate::kopia::install::release_client(
        crate::kopia::install::DEFAULT_ALLOWED_HOSTS.iter().map(|s| s.to_string()).collect(),
    ) {
        Ok(c) => c,
        Err(e) => return Ok(UpdateStatus::Failed { reason: e.to_string() }),
    };

    let response = client
        .get(&url)
        .timeout(REQUEST_TIMEOUT)
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
    // Exactly the six targets `.github/workflows/release.yml` builds. Two were
    // missing here: `aarch64-pc-windows-msvc`, which the workflow has built
    // since the six-target release — so Windows on ARM was being offered the
    // emulated x64 archive when a native one was sitting on the same release
    // page — and `aarch64-unknown-linux-gnu`, which was not listed at all, so
    // an ARM Linux machine was told no build existed for it.
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => "x86_64-pc-windows-msvc",
        ("windows", "aarch64") => "aarch64-pc-windows-msvc",
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        _ => "",
    }
}

/// The triple to fall back to when the release has no native build.
///
/// Only one case exists and it is Windows on ARM, which runs x64 under
/// emulation correctly. Offering nothing at all to a machine that could run the
/// update is worse than offering a slower binary, and
/// [`AssetChoice::emulated`] is how the interface says which happened.
fn fallback_triple() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "aarch64") => "x86_64-pc-windows-msvc",
        _ => "",
    }
}

/// Pick the archive for this machine out of a release's asset list.
pub fn select_asset(assets: &[(String, String)], version: &str) -> Option<AssetChoice> {
    let ext = if cfg!(windows) { "zip" } else { "tar.gz" };
    let find = |triple: &str, emulated: bool| -> Option<AssetChoice> {
        if triple.is_empty() {
            return None;
        }
        let wanted = format!("superbackup-{version}-{triple}.{ext}");
        assets.iter().find(|(name, _)| name == &wanted).map(|(name, url)| AssetChoice {
            file_name: name.clone(),
            url: url.clone(),
            emulated,
        })
    };
    // Native first, always. The fallback exists for a release that has no build
    // for this architecture, not as a preference.
    find(target_triple(), false).or_else(|| find(fallback_triple(), true))
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

// ---------------------------------------------------------------------------
// Fetching a release
// ---------------------------------------------------------------------------

/// Refuse an archive larger than this before reading it into memory.
///
/// The release archives are single binaries of tens of megabytes. A response
/// far past that is not a superbackup build, and verification happens in memory
/// precisely so that nothing unverified ever reaches a path the resolver could
/// find — which only works if the thing being held is a size worth holding.
const MAX_ARCHIVE_BYTES: usize = 256 * 1024 * 1024;

/// Refuse a checksum manifest larger than this. It is one line per artefact.
const MAX_SUMS_BYTES: usize = 256 * 1024;

/// The files attached to one release, as `(name, download url)`.
///
/// A second request rather than a field on the release list, because
/// `/releases` returns every asset of every release and the list is fetched on
/// a timer by machines that will never install anything. This one is made once,
/// when somebody presses Install.
pub async fn release_assets(repo: &str, tag: &str) -> Result<Vec<(String, String)>> {
    #[derive(serde::Deserialize)]
    struct Asset {
        name: String,
        browser_download_url: String,
    }
    #[derive(serde::Deserialize)]
    struct Release {
        #[serde(default)]
        assets: Vec<Asset>,
    }

    if repo.is_empty() || !repo.contains('/') {
        return Err(Error::Validation(format!("`{repo}` is not an owner/name repository")));
    }
    let client = crate::kopia::install::release_client(
        crate::kopia::install::DEFAULT_ALLOWED_HOSTS.iter().map(|s| s.to_string()).collect(),
    )
    .map_err(|e| Error::Config(e.to_string()))?;

    // By tag, so the assets belong to the release the check actually saw. A
    // second call to `/latest` could answer about a newer one published in
    // between, and then the archive and the version on screen would disagree.
    let url = format!("https://api.github.com/repos/{repo}/releases/tags/{tag}");
    let body = get_capped(&client, &url, MAX_SUMS_BYTES.max(MAX_RESPONSE_BYTES)).await?;
    let release: Release = serde_json::from_slice(&body)
        .map_err(|e| Error::Config(format!("the release could not be read: {e}")))?;
    Ok(release.assets.into_iter().map(|a| (a.name, a.browser_download_url)).collect())
}

/// What a release offers this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Downloadable {
    pub version: String,
    pub asset: AssetChoice,
    /// Where `SHA256SUMS` is, for the same release.
    pub sums_url: String,
}

/// Pick the archive and the checksum manifest out of a release's assets.
///
/// Both or neither: an archive with no published digest is not something to
/// download, let alone run. That is not hypothetical caution — the digest is
/// the only thing standing between "GitHub served this" and "somebody served
/// this", and a release missing it is a release that went wrong.
pub fn downloadable(
    version: &str,
    assets: &[(String, String)],
) -> std::result::Result<Downloadable, String> {
    let asset = select_asset(assets, version).ok_or_else(|| {
        format!(
            "release {version} has no build for this machine ({}), so there is nothing to install \
             here. The release page may have one for another platform.",
            target_triple()
        )
    })?;
    let sums_url = assets
        .iter()
        .find(|(name, _)| name == "SHA256SUMS")
        .map(|(_, url)| url.clone())
        .ok_or_else(|| {
            format!(
                "release {version} publishes no SHA256SUMS, so the download cannot be verified \
                 and will not be installed."
            )
        })?;
    Ok(Downloadable { version: version.to_string(), asset, sums_url })
}

/// Download, verify, and hand back the executable, having written nothing.
///
/// Every step here is ordered so that nothing unverified is ever on disk where
/// something could run it:
///
/// 1. the checksum manifest is fetched first, because an archive with no digest
///    to check it against is not worth downloading;
/// 2. the archive is read into memory and checked against that digest;
/// 3. only then is it opened as an archive at all, by the same extractor the
///    kopia installer uses — the one that refuses `..`, absolute paths and
///    drive prefixes in member names.
///
/// The bytes come back rather than being written, because where they go is
/// [`apply`]'s business and it has rules of its own.
pub async fn fetch(target: &Downloadable) -> Result<Vec<u8>> {
    let client = crate::kopia::install::release_client(
        crate::kopia::install::DEFAULT_ALLOWED_HOSTS.iter().map(|s| s.to_string()).collect(),
    )
    .map_err(|e| Error::Config(e.to_string()))?;

    let sums = get_capped(&client, &target.sums_url, MAX_SUMS_BYTES).await?;
    let sums = String::from_utf8_lossy(&sums).into_owned();
    let published = parse_checksums(&sums);
    if published.is_empty() {
        return Err(Error::Config(
            "the release's SHA256SUMS could not be read, so the download cannot be verified and \
             will not be installed."
                .into(),
        ));
    }

    let archive = get_capped(&client, &target.asset.url, MAX_ARCHIVE_BYTES).await?;
    verify_archive(&archive, &target.asset.file_name, &published).map_err(Error::Config)?;

    let kind = if target.asset.file_name.ends_with(".zip") {
        crate::kopia::install::ArchiveKind::Zip
    } else {
        crate::kopia::install::ArchiveKind::TarGz
    };
    let member = if cfg!(windows) { "superbackup.exe" } else { "superbackup" };
    crate::kopia::install::extract_executable(&archive, kind, member, &target.asset.file_name)
        .map_err(|e| Error::Config(e.to_string()))
}

async fn get_capped(client: &reqwest::Client, url: &str, cap: usize) -> Result<Vec<u8>> {
    let mut response = client
        .get(url)
        .header("Accept", "application/octet-stream")
        .send()
        .await
        .map_err(|e| Error::Config(crate::redact::scrub(&e.to_string()).into_owned()))?;

    // A redirect the client's policy stopped arrives as a 3xx response rather
    // than an error, so where this actually ended up is checked rather than
    // assumed. Without it, "the policy refused to follow" and "the policy
    // followed somewhere else" would look the same from here.
    let host = response.url().host_str().unwrap_or_default().to_string();
    if !crate::kopia::install::host_allowed(
        &host,
        &crate::kopia::install::DEFAULT_ALLOWED_HOSTS
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>(),
    ) {
        return Err(Error::Config(format!(
            "the download was redirected to {host}, which is not where superbackup releases come              from, so nothing was fetched."
        )));
    }
    if !response.status().is_success() {
        return Err(Error::Config(format!("the download answered {}", response.status())));
    }

    // Counted as it arrives rather than trusted from `Content-Length`, which is
    // a claim by the server about a body it has not finished sending.
    let mut body = Vec::new();
    loop {
        let chunk = response
            .chunk()
            .await
            .map_err(|e| Error::Config(crate::redact::scrub(&e.to_string()).into_owned()))?;
        let Some(chunk) = chunk else { break };
        if body.len() + chunk.len() > cap {
            return Err(Error::Config(
                "the download is far larger than a superbackup release and was abandoned.".into(),
            ));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[cfg(test)]
mod fetch_tests {
    use super::*;

    fn assets(version: &str, with_sums: bool) -> Vec<(String, String)> {
        let triple = target_triple();
        let ext = if cfg!(windows) { "zip" } else { "tar.gz" };
        let mut out = vec![(
            format!("superbackup-{version}-{triple}.{ext}"),
            "https://example/archive".to_string(),
        )];
        if with_sums {
            out.push(("SHA256SUMS".to_string(), "https://example/sums".to_string()));
        }
        out
    }

    /// An archive with no published digest is not offered at all.
    ///
    /// The digest is the only thing between "GitHub served this" and "somebody
    /// served this". A release missing it is a release that went wrong, and
    /// installing from it anyway would be the one moment this code could do
    /// real harm.
    #[test]
    fn a_release_without_checksums_is_refused_rather_than_trusted() {
        if target_triple().is_empty() {
            return;
        }
        let err = downloadable("0.12.0", &assets("0.12.0", false)).unwrap_err();
        assert!(err.contains("SHA256SUMS"), "{err}");
        assert!(err.contains("will not be installed"), "{err}");
    }

    /// A release with nothing for this machine says so, naming the target.
    #[test]
    fn a_release_with_no_build_for_this_machine_says_which_machine() {
        if target_triple().is_empty() {
            return;
        }
        let other = vec![
            ("superbackup-0.12.0-some-other-target.tar.gz".to_string(), "u".to_string()),
            ("SHA256SUMS".to_string(), "s".to_string()),
        ];
        let err = downloadable("0.12.0", &other).unwrap_err();
        assert!(err.contains(target_triple()), "{err}");
    }

    /// Both halves, and the version they belong to, come back together.
    #[test]
    fn the_archive_and_its_checksums_are_chosen_from_one_release() {
        if target_triple().is_empty() {
            return;
        }
        let found = downloadable("0.12.0", &assets("0.12.0", true)).expect("both");
        assert_eq!(found.version, "0.12.0");
        assert_eq!(found.sums_url, "https://example/sums");
        assert!(found.asset.file_name.contains("0.12.0"));
        assert!(found.asset.file_name.contains(target_triple()));
    }
}

// ---------------------------------------------------------------------------
// Which copy of superbackup is this?
// ---------------------------------------------------------------------------

/// How this copy got onto the machine, and therefore how it may be replaced.
///
/// A binary under `/usr/bin` belongs to dpkg or rpm, one under `/opt/homebrew`
/// belongs to Homebrew, and one inside a `.app` in `/Applications` is replaced
/// by dragging a new bundle in. Overwriting any of those leaves the package
/// manager's database describing a file that is no longer what it says, and the
/// next upgrade through the proper channel silently puts the old version back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Installation {
    /// A binary in a folder superbackup can write to: the portable archive, a
    /// build tree, a per-user install. In-place replacement is both possible
    /// and correct here.
    SelfContained { exe: PathBuf, dir: PathBuf },
    /// Installed by something that keeps a record of it.
    PackageManaged { exe: PathBuf, manager: PackageManager },
    /// Writable only by an administrator.
    ///
    /// Separate from `PackageManaged` because the remedy differs: this one
    /// *could* be replaced, by a process with the rights.
    NeedsElevation { exe: PathBuf, dir: PathBuf },
}

/// Who owns the file, when it is not us.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageManager {
    SystemPackage,
    Homebrew,
    MacAppBundle,
}

impl PackageManager {
    /// What to tell somebody who pressed "Install".
    pub fn how_to_update(self) -> &'static str {
        match self {
            PackageManager::SystemPackage => {
                "This copy was installed by your system's package manager. Update it with `apt upgrade superbackup` or `dnf upgrade superbackup`, so the package database keeps describing what is actually on disk."
            }
            PackageManager::Homebrew => {
                "This copy was installed by Homebrew. Update it with `brew upgrade superbackup`."
            }
            PackageManager::MacAppBundle => {
                "This copy is an application bundle in /Applications. Download the new disk image and replace the bundle, which is what macOS expects and what keeps its signature intact."
            }
        }
    }
}

impl Installation {
    /// Classify the running executable.
    pub fn current() -> Result<Installation> {
        let exe = std::env::current_exe()
            .map_err(|e| Error::io("determining this program's own path", e))?;
        Ok(Installation::of(&exe))
    }

    /// As [`Installation::current`], for a named path. Separate so the
    /// classification can be tested against paths that do not exist here.
    pub fn of(exe: &Path) -> Installation {
        let exe = std::path::absolute(exe).unwrap_or_else(|_| exe.to_path_buf());
        if let Some(manager) = package_manager_for(&exe.to_string_lossy().replace('\\', "/")) {
            return Installation::PackageManaged { exe, manager };
        }
        let dir = exe.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| exe.clone());
        if writable(&dir) {
            Installation::SelfContained { exe, dir }
        } else {
            Installation::NeedsElevation { exe, dir }
        }
    }

    pub fn exe(&self) -> &Path {
        match self {
            Installation::SelfContained { exe, .. }
            | Installation::PackageManaged { exe, .. }
            | Installation::NeedsElevation { exe, .. } => exe,
        }
    }

    pub fn is_self_contained(&self) -> bool {
        matches!(self, Installation::SelfContained { .. })
    }

    /// Why an in-place update is refused, when it is. `None` means it is not.
    pub fn why_not(&self) -> Option<String> {
        match self {
            Installation::SelfContained { .. } => None,
            Installation::PackageManaged { manager, .. } => Some(manager.how_to_update().into()),
            Installation::NeedsElevation { dir, .. } => Some(format!(
                "superbackup is installed in {}, which needs administrator rights to change. Download the installer for the new version, or move superbackup somewhere you own.",
                dir.display()
            )),
        }
    }
}

/// Match the paths that belong to somebody else.
///
/// Forward slashes only — the caller normalises, so one set of patterns covers
/// every platform. Prefixes are whole path components, so `/usr/bin/` matches
/// and a home directory called `usrbin` does not.
fn package_manager_for(path: &str) -> Option<PackageManager> {
    const SYSTEM: &[&str] = &["/usr/bin/", "/usr/local/bin/", "/usr/sbin/", "/bin/", "/snap/"];
    const BREW: &[&str] = &["/opt/homebrew/", "/usr/local/cellar/", "/home/linuxbrew/"];

    let lower = path.to_ascii_lowercase();
    if lower.starts_with("/applications/") && lower.contains(".app/contents/") {
        return Some(PackageManager::MacAppBundle);
    }
    // Before the system list: Homebrew's Linux prefix is under `/home` and its
    // Intel-macOS one under `/usr/local`.
    if BREW.iter().any(|p| lower.starts_with(p)) {
        return Some(PackageManager::Homebrew);
    }
    if SYSTEM.iter().any(|p| lower.starts_with(p)) {
        return Some(PackageManager::SystemPackage);
    }
    None
}

/// Can this process create a file in `dir`?
///
/// Tried rather than inferred. Windows has no permission bit that answers it:
/// `%PROGRAMFILES%` is writable by an elevated process and not by this one, and
/// the only reliable test is to attempt it. Anything going wrong counts as
/// "no", which errs towards offering the installer rather than towards a
/// half-finished update.
fn writable(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    let probe = dir.join(format!(".superbackup-write-probe-{}", std::process::id()));
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------------
// The swap
// ---------------------------------------------------------------------------

/// The suffix given to the outgoing executable.
///
/// It is kept, not deleted. On Windows it *cannot* be deleted while the process
/// is running, and the rename is the only reason replacing a running executable
/// works there at all: an open image file may be renamed, just not removed. On
/// every platform it is also the rollback.
pub const OUTGOING_SUFFIX: &str = ".superbackup-old";

/// What a completed swap did, so the caller can say it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    /// Where the new executable now is — the same path as the old one.
    pub installed: PathBuf,
    /// The renamed previous executable, kept for rollback.
    pub previous: PathBuf,
    pub version: String,
}

/// Put a verified executable in place of the running one.
///
/// The caller is responsible for rules 1, 2 and 4 in the section above: no job
/// running, the bytes already verified against `SHA256SUMS`, and a person
/// having asked. This function is rule 3 — and it is written so that every
/// failure leaves a working superbackup on disk:
///
/// * the incoming bytes are written beside the target and probed with
///   `--version` *before* anything is moved, so a build that will not start is
///   discovered while the old one is still in place;
/// * the outgoing executable is renamed rather than deleted;
/// * if the incoming file cannot then be moved into place, the outgoing one is
///   put back before the error is returned.
///
/// The one thing it does not do is restart anything. Deciding when to stop a
/// process that may be holding vault keys belongs to the caller.
pub fn apply(target: &Path, incoming: &[u8], expected_version: &str) -> Result<Applied> {
    let dir = target
        .parent()
        .ok_or_else(|| Error::Config("the executable path has no parent directory".into()))?;

    // Beside the target, never in the temporary directory: the final move must
    // be a rename within one filesystem, and `%TEMP%` is routinely on another
    // volume from `%PROGRAMFILES%` or a portable install on a USB disk.
    let staged = dir.join(format!("superbackup-incoming-{}{}", std::process::id(), exe_suffix()));
    let _ = std::fs::remove_file(&staged);
    std::fs::write(&staged, incoming)
        .map_err(|e| Error::io(format!("writing {}", staged.display()), e))?;
    make_executable(&staged)?;

    // Does it run at all? A truncated download that still matched its checksum
    // is not possible, but a build for the wrong architecture, a missing system
    // library, or a binary a security product has quarantined all are — and all
    // of them are better discovered now than after the old one is gone.
    if let Err(e) = probe_version(&staged, expected_version) {
        let _ = std::fs::remove_file(&staged);
        return Err(e);
    }

    let previous = previous_path(target);
    let _ = std::fs::remove_file(&previous);
    std::fs::rename(target, &previous).map_err(|e| {
        let _ = std::fs::remove_file(&staged);
        Error::io(format!("moving the current executable aside to {}", previous.display()), e)
    })?;

    if let Err(e) = std::fs::rename(&staged, target) {
        // Put it back. Leaving no executable at all where superbackup is
        // expected is the one outcome worse than not updating.
        let _ = std::fs::rename(&previous, target);
        let _ = std::fs::remove_file(&staged);
        return Err(Error::io(format!("installing {}", target.display()), e));
    }

    Ok(Applied { installed: target.to_path_buf(), previous, version: expected_version.to_string() })
}

/// Where `apply` leaves the executable it replaced.
pub fn previous_path(target: &Path) -> PathBuf {
    let mut name = target.as_os_str().to_os_string();
    name.push(OUTGOING_SUFFIX);
    PathBuf::from(name)
}

/// Remove an executable left behind by a previous update.
///
/// Called at startup rather than at the end of the update: on Windows the old
/// image is still mapped while the process that was running it lives, so the
/// only moment it can actually be deleted is from the *next* process. Failure
/// is ignored — a stale file beside the executable is untidy, not harmful, and
/// the next start tries again.
pub fn clean_previous(target: &Path) {
    let previous = previous_path(target);
    if previous.exists() {
        let _ = std::fs::remove_file(&previous);
    }
}

/// Roll back to the executable `apply` moved aside.
///
/// For the person whose new version starts but does not work. Nothing calls it
/// automatically: a build that runs cannot be judged by this code.
pub fn roll_back(target: &Path) -> Result<()> {
    let previous = previous_path(target);
    if !previous.is_file() {
        return Err(Error::Config(format!(
            "there is no previous version to go back to at {}",
            previous.display()
        )));
    }
    let aside = rollback_scratch(target)?;
    let _ = std::fs::rename(target, &aside);
    std::fs::rename(&previous, target)
        .map_err(|e| Error::io(format!("restoring {}", target.display()), e))?;
    let _ = std::fs::remove_file(&aside);
    Ok(())
}

fn rollback_scratch(target: &Path) -> Result<PathBuf> {
    let dir = target
        .parent()
        .ok_or_else(|| Error::Config("the executable path has no parent directory".into()))?;
    Ok(dir.join(format!("superbackup-rollback-{}{}", std::process::id(), exe_suffix())))
}

fn exe_suffix() -> &'static str {
    if cfg!(windows) {
        ".exe"
    } else {
        ""
    }
}

/// Make a freshly written file executable where that is a thing.
fn make_executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path)
            .map_err(|e| Error::io(format!("reading {}", path.display()), e))?
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms)
            .map_err(|e| Error::io(format!("making {} executable", path.display()), e))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Run the staged binary with `--version` and check it says what it should.
///
/// `--version` is the safest thing to ask a new build: it opens no vault,
/// starts no daemon, binds no socket, and reads no configuration, so a version
/// that would misbehave against this machine's data cannot do so while being
/// asked its name.
fn probe_version(staged: &Path, expected: &str) -> Result<()> {
    let mut command = std::process::Command::new(staged);
    command.arg("--version");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No console window for a probe the user did not ask to watch.
        command.creation_flags(0x0800_0000);
    }
    let output = command
        .output()
        .map_err(|e| Error::io(format!("running the new {} to check it", staged.display()), e))?;
    if !output.status.success() {
        return Err(Error::Config(format!(
            "the downloaded superbackup did not start ({}), so it was not installed and the \
             current version is untouched",
            output.status
        )));
    }
    let said = String::from_utf8_lossy(&output.stdout);
    if !said.contains(expected) {
        return Err(Error::Config(format!(
            "the downloaded superbackup reports a different version from the one that was \
             downloaded (expected {expected}, it says {}), so it was not installed",
            said.trim().lines().next().unwrap_or("nothing").trim()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod swap_tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/update-tests")
            .join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        std::path::absolute(&dir).unwrap_or(dir)
    }

    /// A copy somebody else installed is never overwritten in place.
    #[test]
    fn a_package_managed_copy_is_left_to_its_package_manager() {
        for (path, manager) in [
            ("/usr/bin/superbackup", PackageManager::SystemPackage),
            ("/usr/local/bin/superbackup", PackageManager::SystemPackage),
            ("/snap/superbackup/current/bin/superbackup", PackageManager::SystemPackage),
            ("/opt/homebrew/bin/superbackup", PackageManager::Homebrew),
            ("/home/linuxbrew/.linuxbrew/bin/superbackup", PackageManager::Homebrew),
            (
                "/Applications/superbackup.app/Contents/MacOS/superbackup",
                PackageManager::MacAppBundle,
            ),
        ] {
            assert_eq!(package_manager_for(path), Some(manager), "{path}");
            assert!(!manager.how_to_update().is_empty());
        }
    }

    /// A path that merely resembles one of those is ours.
    #[test]
    fn a_path_that_only_resembles_a_package_path_is_still_ours() {
        for path in [
            "/home/me/usrbin/superbackup",
            "/home/me/.local/share/superbackup/superbackup",
            "C:/Users/Andreas/workspace/superbackup/dist/superbackup.exe",
            // Not under /Applications, so not the bundle case.
            "/Users/andreas/Downloads/superbackup.app/Contents/MacOS/superbackup",
        ] {
            assert_eq!(package_manager_for(path), None, "{path}");
        }
    }

    /// Every refusal says what to do instead.
    #[test]
    fn a_refusal_always_says_what_to_do_instead() {
        let managed = Installation::PackageManaged {
            exe: PathBuf::from("/usr/bin/superbackup"),
            manager: PackageManager::SystemPackage,
        };
        assert!(managed.why_not().expect("a reason").contains("apt upgrade"));
        assert!(!managed.is_self_contained());

        let elevated = Installation::NeedsElevation {
            exe: PathBuf::from("C:/Program Files/superbackup/superbackup.exe"),
            dir: PathBuf::from("C:/Program Files/superbackup"),
        };
        assert!(elevated.why_not().expect("a reason").contains("Program Files"));

        let ours = Installation::SelfContained {
            exe: PathBuf::from("/home/me/superbackup"),
            dir: PathBuf::from("/home/me"),
        };
        assert_eq!(ours.why_not(), None);
    }

    /// A build that will not start never displaces the one that does.
    ///
    /// This is the failure the whole ordering exists for: the probe runs while
    /// the current executable is still exactly where it was, so a bad download
    /// costs a temporary file and nothing else.
    #[test]
    fn an_incoming_binary_that_does_not_run_leaves_the_old_one_in_place() {
        let dir = scratch("bad-incoming");
        let target = dir.join(format!("superbackup{}", exe_suffix()));
        std::fs::write(&target, b"the working one").expect("write the current executable");

        let err = apply(&target, b"not an executable at all", "9.9.9").unwrap_err();
        let said = err.to_string();
        assert!(
            said.contains("did not start") || said.contains("running the new"),
            "the reason must name the probe: {said}"
        );

        assert_eq!(
            std::fs::read(&target).expect("the current executable must still be there"),
            b"the working one",
            "a failed update must not touch the executable that works"
        );
        assert!(
            !previous_path(&target).exists(),
            "nothing was moved aside, because nothing was replaced"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The previous executable is kept, and cleaning it up is idempotent.
    #[test]
    fn the_outgoing_executable_is_kept_and_cleaned_up_later() {
        let dir = scratch("cleanup");
        let target = dir.join("superbackup");
        let previous = previous_path(&target);
        std::fs::write(&target, b"current").expect("current");
        std::fs::write(&previous, b"previous").expect("previous");

        // Rolling back puts the kept one back where it belongs.
        roll_back(&target).expect("roll back");
        assert_eq!(std::fs::read(&target).expect("target"), b"previous");

        // And with nothing to go back to, it says so rather than doing damage.
        let err = roll_back(&target).unwrap_err().to_string();
        assert!(err.contains("no previous version"), "{err}");
        assert_eq!(std::fs::read(&target).expect("target"), b"previous");

        // Cleaning up is safe whether or not there is anything to clean.
        clean_previous(&target);
        clean_previous(&target);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
