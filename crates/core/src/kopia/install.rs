//! Fetching and updating the kopia binary from Kopia's own GitHub releases.
//!
//! Kopia is a hard prerequisite. A backup product whose first screen is "please
//! go and install this other tool" is a backup product that never runs, so
//! superbackup fetches one itself on first launch and keeps it current.
//!
//! That convenience is a supply-chain decision, and it is bounded accordingly.
//!
//! # The verification chain
//!
//! ```text
//!  api.github.com/repos/kopia/kopia/releases/latest        (TLS, rustls)
//!        │  release JSON: tag, prerelease flag, asset list
//!        ▼
//!  checksums.txt from the same release                     (TLS, host-checked)
//!        │  "<sha256>  <filename>", one line per asset
//!        ▼
//!  kopia-<version>-<platform>.<zip|tar.gz>                 (TLS, host-checked)
//!        │  SHA-256 computed over the bytes we actually received,
//!        │  compared in constant time against the line for this filename
//!        ▼
//!  extract exactly one member named kopia/kopia.exe        (path-traversal guarded)
//!        ▼
//!  write to a temp file beside the target, chmod +x, run `--version`
//!        ▼
//!  rename into place only if it reports the version the release promised
//! ```
//!
//! Every link is enforced in this file and tested in
//! `crates/core/tests/kopia_install.rs` against a local HTTP server.
//!
//! # What this chain does *not* prove
//!
//! Kopia publishes `checksums.txt.sig` alongside `checksums.txt`, but the
//! signing key is not published in a form this driver can pin, so the signature
//! is **not** verified. The checksum therefore proves integrity — that the
//! bytes were not corrupted or tampered with in transit or at a CDN edge — but
//! its authenticity rests entirely on TLS to `github.com` and on GitHub itself.
//! An attacker who can publish a release to `kopia/kopia` defeats this, exactly
//! as they would defeat a user typing `curl | tar x` by hand. This is stated
//! here, in [`InstallOutcome::signature_verified`], and in the threat model
//! rather than being left implied.
//!
//! # Failure behaviour
//!
//! This runs at startup. No network, a captive portal, a corporate proxy, a
//! GitHub outage, an API rate limit, or a read-only install directory must all
//! degrade to "carry on with the kopia we already have, and say so". Hence
//! [`KopiaInstaller::check_for_update`] returns an [`UpdateCheck`] rather than a
//! `Result`: a failed check is a warning, never something that can stop the
//! application from starting.

use super::binary::{KopiaBinary, KopiaSource, KopiaVersion, MINIMUM_KOPIA_VERSION};
use crate::error::Error;
use crate::model::{KopiaManagement, Settings, UpdatePolicy};
use crate::paths::Paths;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use semver::Version;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;
use tokio::sync::mpsc;

/// GitHub's REST endpoint. Overridable only for tests.
pub const GITHUB_API_BASE: &str = "https://api.github.com";

/// Hosts a release download is allowed to come from, before and after
/// redirects.
///
/// GitHub serves the API from `api.github.com`, the `browser_download_url` from
/// `github.com`, and then redirects to a storage host under
/// `*.githubusercontent.com` — historically `objects.githubusercontent.com`,
/// currently `release-assets.githubusercontent.com`. The suffix rule covers
/// both without needing a code change every time GitHub renames its CDN, and
/// still refuses any origin outside GitHub's own domains.
pub const DEFAULT_ALLOWED_HOSTS: &[&str] =
    &["github.com", "api.github.com", "objects.githubusercontent.com", "codeload.github.com"];

/// Suffix accepted in addition to [`DEFAULT_ALLOWED_HOSTS`].
const GITHUB_CDN_SUFFIX: &str = ".githubusercontent.com";

/// Hard ceiling on a downloaded archive. Kopia's release archives are about
/// 17 MB; anything approaching this is not a kopia release.
const MAX_ARCHIVE_BYTES: u64 = 192 * 1024 * 1024;
/// `checksums.txt` is a few kilobytes.
const MAX_CHECKSUMS_BYTES: u64 = 4 * 1024 * 1024;
/// Ceiling on the extracted executable.
const MAX_BINARY_BYTES: u64 = 192 * 1024 * 1024;
/// Ceiling on the release JSON.
const MAX_JSON_BYTES: u64 = 8 * 1024 * 1024;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const METADATA_TIMEOUT: Duration = Duration::from_secs(30);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// The `User-Agent` GitHub requires; requests without one are rejected outright.
fn user_agent() -> String {
    format!("superbackup/{} (+https://github.com/andreaswiren/superbackup)", crate::VERSION)
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Why an install did not happen.
///
/// Distinct variants because the GUI reacts differently to each: a checksum
/// mismatch is a security event that must be shown loudly, while "offline" is a
/// shrug.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallError {
    /// Could not reach GitHub at all.
    Network(String),
    /// GitHub answered, but not with a usable release.
    Api(String),
    /// GitHub is rate-limiting this IP. Distinct because retrying sooner will
    /// not help and the user has done nothing wrong.
    RateLimited,
    /// The release has no asset for this operating system and CPU.
    NoAssetForPlatform { os: &'static str, arch: &'static str, version: String },
    /// The release published no `checksums.txt`, so nothing can be verified.
    /// Installing anyway is not an option.
    NoChecksums { version: String },
    /// **Security event.** The bytes received do not match the published
    /// checksum. Nothing was written to disk.
    ChecksumMismatch { asset: String, expected: String, actual: String },
    /// A download URL resolved to a host outside GitHub.
    UntrustedHost { url_host: String },
    /// The archive contained an entry that would escape the target directory.
    /// **Security event.**
    UnsafeArchive { member: String },
    /// The archive did not contain a kopia executable.
    ExecutableNotFound { asset: String },
    /// The download exceeded its size cap.
    TooLarge { limit: u64 },
    /// The freshly installed binary did not report the version the release
    /// promised, so it was not moved into place.
    VersionMismatch { expected: String, reported: String },
    /// Could not write to the install directory.
    Io(String),
    /// The managed binary is in use — a job is running against it — so it must
    /// not be replaced right now.
    Busy,
    /// The requested version is older than what is installed, or older than the
    /// configured minimum.
    RefusedVersion { reason: String },
    /// Automatic installation is switched off and nothing is installed.
    AutoInstallDisabled,
}

impl InstallError {
    /// A complete sentence for the user.
    pub fn message(&self) -> String {
        match self {
            InstallError::Network(e) => {
                format!("Could not reach GitHub to download kopia ({e}).")
            }
            InstallError::Api(e) => format!("GitHub did not return a usable kopia release ({e})."),
            InstallError::RateLimited => {
                "GitHub is temporarily rate-limiting this computer, so kopia could not be \
                 downloaded."
                    .into()
            }
            InstallError::NoAssetForPlatform { os, arch, version } => format!(
                "Kopia {version} does not publish a build for {os} on {arch}."
            ),
            InstallError::NoChecksums { version } => format!(
                "Kopia release {version} did not publish a checksum file, so the download could \
                 not be verified and was not installed."
            ),
            InstallError::ChecksumMismatch { asset, .. } => format!(
                "The kopia download ({asset}) did not match its published checksum. It was \
                 discarded and nothing was installed."
            ),
            InstallError::UntrustedHost { url_host } => format!(
                "The kopia download was redirected to {url_host}, which is not a GitHub host. \
                 It was refused."
            ),
            InstallError::UnsafeArchive { member } => format!(
                "The kopia archive contained an entry that would write outside the install \
                 directory ({member}). It was refused."
            ),
            InstallError::ExecutableNotFound { asset } => {
                format!("The kopia archive ({asset}) did not contain a kopia executable.")
            }
            InstallError::TooLarge { limit } => {
                format!("The kopia download exceeded its {limit}-byte size limit and was stopped.")
            }
            InstallError::VersionMismatch { expected, reported } => format!(
                "The downloaded kopia reported version {reported} but the release said \
                 {expected}, so it was not installed."
            ),
            InstallError::Io(e) => format!("Could not install kopia ({e})."),
            InstallError::Busy => {
                "kopia is in use by a running backup, so it was not replaced.".into()
            }
            InstallError::RefusedVersion { reason } => format!("kopia was not replaced: {reason}"),
            InstallError::AutoInstallDisabled => {
                "kopia is not installed and automatic installation is switched off.".into()
            }
        }
    }

    /// What the user can do about it.
    pub fn hint(&self) -> Option<&'static str> {
        match self {
            InstallError::Network(_) | InstallError::RateLimited => {
                Some("Check the network connection and try again later, or install kopia yourself and point Settings at it.")
            }
            InstallError::ChecksumMismatch { .. } | InstallError::UntrustedHost { .. }
            | InstallError::UnsafeArchive { .. } => {
                Some("Do not retry over the same network. Download kopia yourself from https://kopia.io and set its path in Settings.")
            }
            InstallError::NoAssetForPlatform { .. } => {
                Some("Install kopia from your package manager and set its path in Settings.")
            }
            InstallError::Io(_) => {
                Some("Check that superbackup's data directory is writable, then try again.")
            }
            InstallError::Busy => Some("Wait for the running backup to finish."),
            InstallError::AutoInstallDisabled => {
                Some("Turn on automatic kopia installation in Settings, or set a kopia path there.")
            }
            _ => None,
        }
    }

    /// True for the two variants that mean somebody may be attacking the
    /// update channel. The GUI shows these differently and the event log keeps
    /// them forever.
    pub fn is_security_event(&self) -> bool {
        matches!(
            self,
            InstallError::ChecksumMismatch { .. }
                | InstallError::UnsafeArchive { .. }
                | InstallError::UntrustedHost { .. }
        )
    }
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for InstallError {}

impl From<InstallError> for Error {
    fn from(e: InstallError) -> Error {
        match e {
            InstallError::AutoInstallDisabled => Error::KopiaMissing,
            other => Error::Kopia { status: -1, stderr: other.message() },
        }
    }
}

type InstallResult<T> = std::result::Result<T, InstallError>;

// ---------------------------------------------------------------------------
// Progress
// ---------------------------------------------------------------------------

/// Where an install has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallPhase {
    QueryingRelease,
    DownloadingChecksums,
    DownloadingArchive,
    Verifying,
    Extracting,
    Installing,
    Done,
}

impl InstallPhase {
    /// The label under the progress bar on first run.
    pub fn title(&self) -> &'static str {
        match self {
            InstallPhase::QueryingRelease => "Looking up the latest kopia release",
            InstallPhase::DownloadingChecksums => "Fetching checksums",
            InstallPhase::DownloadingArchive => "Downloading kopia",
            InstallPhase::Verifying => "Verifying the download",
            InstallPhase::Extracting => "Extracting",
            InstallPhase::Installing => "Installing",
            InstallPhase::Done => "Done",
        }
    }
}

/// One progress sample for the first-run download bar.
#[derive(Debug, Clone)]
pub struct InstallProgress {
    pub phase: InstallPhase,
    pub downloaded_bytes: u64,
    /// `None` when the server sent no `Content-Length`.
    pub total_bytes: Option<u64>,
    pub version: Option<String>,
}

impl InstallProgress {
    pub fn fraction(&self) -> Option<f32> {
        match self.total_bytes {
            Some(t) if t > 0 => {
                Some((self.downloaded_bytes as f64 / t as f64).clamp(0.0, 1.0) as f32)
            }
            _ => None,
        }
    }
}

/// Non-blocking progress sink, for the same reason as
/// [`super::command::EventSink`]: a GUI that stops polling must slow the
/// download bar, never the download.
#[derive(Debug, Clone)]
pub struct InstallProgressSink {
    tx: mpsc::Sender<InstallProgress>,
}

impl InstallProgressSink {
    pub fn channel(capacity: usize) -> (InstallProgressSink, mpsc::Receiver<InstallProgress>) {
        let (tx, rx) = mpsc::channel(capacity.max(1));
        (InstallProgressSink { tx }, rx)
    }
    fn emit(&self, p: InstallProgress) {
        let _ = self.tx.try_send(p);
    }
}

// ---------------------------------------------------------------------------
// Release metadata
// ---------------------------------------------------------------------------

/// One downloadable file attached to a release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseAsset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

/// A GitHub release, reduced to what matters here.
#[derive(Debug, Clone)]
pub struct ReleaseInfo {
    pub tag: String,
    pub version: Version,
    pub prerelease: bool,
    pub published_at: Option<DateTime<Utc>>,
    pub assets: Vec<ReleaseAsset>,
    pub html_url: String,
}

impl ReleaseInfo {
    /// Parse the subset of GitHub's release JSON this code depends on.
    ///
    /// Field names verified against a live response from
    /// `api.github.com/repos/kopia/kopia/releases/latest`.
    pub fn from_json(v: &serde_json::Value) -> InstallResult<ReleaseInfo> {
        let tag = v
            .get("tag_name")
            .and_then(|t| t.as_str())
            .ok_or_else(|| InstallError::Api("the release has no tag_name".into()))?
            .to_string();
        let version = parse_release_version(&tag).ok_or_else(|| {
            InstallError::Api(format!("release tag {tag:?} is not a version number"))
        })?;
        let assets = v
            .get("assets")
            .and_then(|a| a.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|e| {
                        Some(ReleaseAsset {
                            name: e.get("name")?.as_str()?.to_string(),
                            url: e.get("browser_download_url")?.as_str()?.to_string(),
                            size: e.get("size").and_then(|s| s.as_u64()).unwrap_or(0),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(ReleaseInfo {
            tag,
            version,
            prerelease: v.get("prerelease").and_then(|p| p.as_bool()).unwrap_or(false),
            published_at: v
                .get("published_at")
                .and_then(|p| p.as_str())
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.with_timezone(&Utc)),
            assets,
            html_url: v
                .get("html_url")
                .and_then(|u| u.as_str())
                .unwrap_or_default()
                .to_string(),
        })
    }

    pub fn asset(&self, name: &str) -> Option<&ReleaseAsset> {
        self.assets.iter().find(|a| a.name == name)
    }
}

/// `v0.23.1` and `0.23.1` both parse; anything else does not.
pub fn parse_release_version(tag: &str) -> Option<Version> {
    let t = tag.trim().trim_start_matches(['v', 'V']);
    Version::parse(t)
        .ok()
        // Tolerate two-component tags, which kopia has not used but which cost
        // nothing to accept.
        .or_else(|| Version::parse(&format!("{t}.0")).ok())
}

/// The release asset for the running platform.
///
/// **Verified against the real `kopia/kopia` v0.23.1 release**, whose asset list
/// is exactly:
///
/// ```text
/// kopia-0.23.1-windows-x64.zip
/// kopia-0.23.1-linux-x64.tar.gz      kopia-0.23.1-linux-arm64.tar.gz     kopia-0.23.1-linux-arm.tar.gz
/// kopia-0.23.1-macOS-x64.tar.gz      kopia-0.23.1-macOS-arm64.tar.gz     kopia-0.23.1-macOS-universal.tar.gz
/// ```
///
/// Note the capital `S` in `macOS`, that the version in the file name carries
/// **no** `v` prefix while the tag does, and that Windows publishes **x64 only**
/// — there is no `windows-arm64` build. Windows on ARM64 therefore gets the x64
/// archive, which Windows 11's x64 emulation runs correctly; refusing to
/// install anything at all on those machines would be worse, and the choice is
/// visible in [`AssetChoice::emulated`].
pub fn asset_for_platform(version: &str, os: &str, arch: &str) -> Option<AssetChoice> {
    let (platform, kind, emulated) = match (os, arch) {
        ("windows", "x86_64") => ("windows-x64", ArchiveKind::Zip, false),
        ("windows", "aarch64") => ("windows-x64", ArchiveKind::Zip, true),
        ("linux", "x86_64") => ("linux-x64", ArchiveKind::TarGz, false),
        ("linux", "aarch64") => ("linux-arm64", ArchiveKind::TarGz, false),
        ("linux", "arm") => ("linux-arm", ArchiveKind::TarGz, false),
        ("macos", "x86_64") => ("macOS-x64", ArchiveKind::TarGz, false),
        ("macos", "aarch64") => ("macOS-arm64", ArchiveKind::TarGz, false),
        _ => return None,
    };
    let ext = match kind {
        ArchiveKind::Zip => "zip",
        ArchiveKind::TarGz => "tar.gz",
    };
    Some(AssetChoice {
        name: format!("kopia-{version}-{platform}.{ext}"),
        kind,
        emulated,
    })
}

/// The asset picked for this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetChoice {
    pub name: String,
    pub kind: ArchiveKind,
    /// True when no native build exists and an emulated one was chosen.
    pub emulated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveKind {
    Zip,
    TarGz,
}

/// The file name kopia publishes its checksums under. Verified against the real
/// release: a `checksums.txt` in GNU `sha256sum` format —
/// `<64 hex chars><two spaces><file name>`, one line per asset — accompanied by
/// a `checksums.txt.sig` this driver does not verify (see the module docs).
pub const CHECKSUMS_ASSET: &str = "checksums.txt";

/// Find the SHA-256 for one file in a `sha256sum`-format checksum listing.
///
/// Accepts both the `  ` (text mode) and ` *` (binary mode) separators, because
/// both are legal in that format and a release tool switching between them must
/// not break verification.
pub fn checksum_for(listing: &str, file_name: &str) -> Option<String> {
    for line in listing.lines() {
        let line = line.trim();
        let Some((hash, rest)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        let name = rest.trim_start().trim_start_matches('*').trim();
        if name == file_name {
            return Some(hash.to_ascii_lowercase());
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Update checking
// ---------------------------------------------------------------------------

/// The result of an update check. Never an error: see the module docs.
#[derive(Debug, Clone)]
pub enum UpdateCheck {
    /// Nothing was asked of GitHub.
    Skipped { reason: SkipReason },
    /// The installed kopia is current.
    UpToDate { current: Version, latest: Version },
    /// A newer release exists.
    Available { current: Option<Version>, latest: Version, release: Box<ReleaseInfo> },
    /// The check itself failed. A warning, not a failure of anything else.
    Failed { error: InstallError },
}

/// Why no check was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// `UpdatePolicy::Off`.
    PolicyOff,
    /// `check_interval_hours` has not elapsed since `last_check_at`.
    TooSoon,
    /// The user pinned an explicit kopia path, which superbackup never manages.
    UserManagedBinary,
}

impl UpdateCheck {
    /// The newer version, when there is one.
    pub fn available_version(&self) -> Option<&Version> {
        match self {
            UpdateCheck::Available { latest, .. } => Some(latest),
            _ => None,
        }
    }
    /// One line for the Settings screen.
    pub fn summary(&self) -> String {
        match self {
            UpdateCheck::Skipped { reason: SkipReason::PolicyOff } => {
                "Update checks are switched off.".into()
            }
            UpdateCheck::Skipped { reason: SkipReason::TooSoon } => {
                "Checked recently; nothing to do.".into()
            }
            UpdateCheck::Skipped { reason: SkipReason::UserManagedBinary } => {
                "kopia is managed by you, not by superbackup.".into()
            }
            UpdateCheck::UpToDate { current, .. } => format!("kopia {current} is up to date."),
            UpdateCheck::Available { latest, current: Some(c), .. } => {
                format!("kopia {latest} is available; {c} is installed.")
            }
            UpdateCheck::Available { latest, current: None, .. } => {
                format!("kopia {latest} is available.")
            }
            UpdateCheck::Failed { error } => {
                format!("Could not check for a kopia update: {error}")
            }
        }
    }
}

/// A completed installation.
#[derive(Debug, Clone)]
pub struct InstallOutcome {
    pub version: Version,
    pub path: PathBuf,
    pub asset: String,
    /// SHA-256 of the archive, recorded in the event log so an install can be
    /// audited after the fact.
    pub sha256: String,
    /// Always `false`: kopia's `checksums.txt.sig` is published but its signing
    /// key is not pinnable here, so authenticity rests on TLS to GitHub. Kept
    /// as a field so the GUI states the guarantee accurately instead of
    /// implying a stronger one.
    pub signature_verified: bool,
    /// True when no native build exists for this CPU and an emulated one was
    /// installed.
    pub emulated: bool,
    /// True when the previous managed binary was replaced rather than this
    /// being a first install.
    pub replaced_previous: bool,
}

// ---------------------------------------------------------------------------
// The installer
// ---------------------------------------------------------------------------

/// Downloads, verifies and installs the managed kopia binary.
#[derive(Debug, Clone)]
pub struct KopiaInstaller {
    client: reqwest::Client,
    api_base: String,
    allowed_hosts: Vec<String>,
    target: PathBuf,
    /// Where temp files are written. Always the target's own directory, so the
    /// final rename is atomic and never crosses a filesystem.
    target_dir: PathBuf,
}

impl KopiaInstaller {
    /// The production installer: GitHub, GitHub hosts only.
    pub fn new(paths: &Paths) -> InstallResult<KopiaInstaller> {
        KopiaInstaller::with_endpoint(
            paths,
            GITHUB_API_BASE,
            DEFAULT_ALLOWED_HOSTS.iter().map(|s| s.to_string()).collect(),
        )
    }

    /// An installer pointed at a different endpoint.
    ///
    /// Exists for the test suite, which serves a synthetic release from
    /// `127.0.0.1`, and for an organisation mirroring releases internally. Any
    /// use of it moves the trust anchor away from GitHub, which is why the
    /// production constructor does not expose it by accident.
    pub fn with_endpoint(
        paths: &Paths,
        api_base: &str,
        allowed_hosts: Vec<String>,
    ) -> InstallResult<KopiaInstaller> {
        let target = paths.bundled_kopia();
        let target_dir = target
            .parent()
            .map(|p| p.to_path_buf())
            .ok_or_else(|| InstallError::Io("the kopia install path has no parent".into()))?;

        let allowed = allowed_hosts.clone();
        let client = reqwest::Client::builder()
            .user_agent(user_agent())
            .connect_timeout(CONNECT_TIMEOUT)
            // A redirect that leaves GitHub is refused by the policy itself, so
            // no request is ever issued to the foreign host — not even a
            // connection that would leak the fact that this machine is updating.
            .redirect(reqwest::redirect::Policy::custom(move |attempt| {
                match attempt.url().host_str() {
                    Some(h) if host_allowed(h, &allowed) => {
                        if attempt.previous().len() > 8 {
                            attempt.error("too many redirects")
                        } else {
                            attempt.follow()
                        }
                    }
                    _ => attempt.stop(),
                }
            }))
            .build()
            .map_err(|e| InstallError::Network(e.to_string()))?;

        Ok(KopiaInstaller {
            client,
            api_base: api_base.trim_end_matches('/').to_string(),
            allowed_hosts,
            target,
            target_dir,
        })
    }

    /// Where the managed binary lives.
    pub fn target_path(&self) -> &Path {
        &self.target
    }

    /// The version of the managed binary, by asking it.
    ///
    /// Deliberately not read from [`KopiaManagement::managed_version`]: that
    /// field is a cache, and a binary replaced by a package manager or deleted
    /// by a disk cleaner would make it a lie.
    pub async fn installed_version(&self) -> Option<Version> {
        if !self.target.is_file() {
            return None;
        }
        KopiaBinary::probe_with_floor(&self.target, KopiaSource::Bundled, &KopiaVersion::new(0, 0, 0))
            .await
            .ok()
            .map(|b| b.version().to_semver())
    }

    /// Ask GitHub whether there is anything newer, honouring policy and the
    /// check interval.
    ///
    /// `settings.kopia.last_check_at` is *read* here; persisting a new value is
    /// the caller's job, because only the caller can save the config. The
    /// returned [`UpdateCheck`] tells it whether a check actually happened.
    pub async fn check_for_update(&self, settings: &Settings, now: DateTime<Utc>) -> UpdateCheck {
        if settings.kopia_path.is_some() {
            return UpdateCheck::Skipped { reason: SkipReason::UserManagedBinary };
        }
        let mgmt = &settings.kopia;
        if mgmt.auto_update == UpdatePolicy::Off {
            return UpdateCheck::Skipped { reason: SkipReason::PolicyOff };
        }
        if !due_for_check(mgmt, now) {
            return UpdateCheck::Skipped { reason: SkipReason::TooSoon };
        }

        let release = match self.fetch_release(mgmt).await {
            Ok(r) => r,
            Err(e) => return UpdateCheck::Failed { error: e },
        };
        let current = self.installed_version().await;
        match &current {
            Some(c) if *c >= release.version => {
                UpdateCheck::UpToDate { current: c.clone(), latest: release.version.clone() }
            }
            _ => UpdateCheck::Available {
                current,
                latest: release.version.clone(),
                release: Box::new(release),
            },
        }
    }

    /// Install the newest release the settings permit.
    pub async fn install_latest(
        &self,
        settings: &Settings,
        progress: Option<&InstallProgressSink>,
    ) -> InstallResult<InstallOutcome> {
        let release = self.fetch_release(&settings.kopia).await?;
        self.install_release(&release, settings, progress).await
    }

    /// Install one exact version.
    pub async fn install_version(
        &self,
        version: &str,
        settings: &Settings,
        progress: Option<&InstallProgressSink>,
    ) -> InstallResult<InstallOutcome> {
        let release = self.fetch_release_by_tag(&settings.kopia.source_repo, version).await?;
        self.install_release(&release, settings, progress).await
    }

    /// The startup path: use whatever kopia is already usable, and only install
    /// one when there is none.
    ///
    /// Never touches a kopia the user installed themselves — discovery decides
    /// that, and it prefers the system binary when the setting says so.
    pub async fn ensure_available(
        &self,
        settings: &Settings,
        paths: &Paths,
        progress: Option<&InstallProgressSink>,
    ) -> InstallResult<KopiaBinary> {
        if let Ok(bin) = KopiaBinary::discover(settings, paths).await {
            return Ok(bin);
        }
        if !settings.kopia.auto_install {
            return Err(InstallError::AutoInstallDisabled);
        }
        let outcome = match &settings.kopia.pinned_version {
            Some(v) => self.install_version(v, settings, progress).await?,
            None => self.install_latest(settings, progress).await?,
        };
        KopiaBinary::probe_with_floor(
            &outcome.path,
            KopiaSource::Bundled,
            &super::binary::configured_floor(settings),
        )
        .await
        .map_err(|e| InstallError::Io(e.to_string()))
    }

    /// Apply an update according to policy.
    ///
    /// `a_job_is_running` is supplied by the engine, and it is load-bearing: the
    /// binary being replaced is the one reading and writing repositories, and
    /// swapping it mid-snapshot is how a backup ends up half-written. Under
    /// [`UpdatePolicy::Automatic`] a running job defers the update rather than
    /// cancelling it.
    pub async fn apply_update_if_wanted(
        &self,
        settings: &Settings,
        check: &UpdateCheck,
        a_job_is_running: bool,
        progress: Option<&InstallProgressSink>,
    ) -> InstallResult<Option<InstallOutcome>> {
        let UpdateCheck::Available { release, .. } = check else {
            return Ok(None);
        };
        if settings.kopia.auto_update != UpdatePolicy::Automatic {
            return Ok(None);
        }
        if a_job_is_running {
            return Err(InstallError::Busy);
        }
        self.install_release(release, settings, progress).await.map(Some)
    }

    // -----------------------------------------------------------------
    // Internals
    // -----------------------------------------------------------------

    async fn fetch_release(&self, mgmt: &KopiaManagement) -> InstallResult<ReleaseInfo> {
        if let Some(pinned) = &mgmt.pinned_version {
            return self.fetch_release_by_tag(&mgmt.source_repo, pinned).await;
        }
        let latest = self
            .get_json(&format!("{}/repos/{}/releases/latest", self.api_base, mgmt.source_repo))
            .await?;
        let release = ReleaseInfo::from_json(&latest)?;
        if release.prerelease && !mgmt.allow_prerelease {
            // `/releases/latest` excludes prereleases already, so reaching here
            // means a mirror behaved differently. Refuse rather than assume.
            return Err(InstallError::Api(format!(
                "release {} is a pre-release and pre-releases are not enabled",
                release.tag
            )));
        }
        Ok(release)
    }

    async fn fetch_release_by_tag(
        &self,
        repo: &str,
        version: &str,
    ) -> InstallResult<ReleaseInfo> {
        let tag = if version.starts_with('v') { version.to_string() } else { format!("v{version}") };
        let json =
            self.get_json(&format!("{}/repos/{repo}/releases/tags/{tag}", self.api_base)).await?;
        ReleaseInfo::from_json(&json)
    }

    /// The whole verified install, from a release we have already resolved.
    async fn install_release(
        &self,
        release: &ReleaseInfo,
        settings: &Settings,
        progress: Option<&InstallProgressSink>,
    ) -> InstallResult<InstallOutcome> {
        let version_string = release.version.to_string();
        emit(progress, InstallPhase::QueryingRelease, 0, None, Some(&version_string));

        self.check_version_acceptable(release, settings).await?;

        let choice = asset_for_platform(&version_string, std::env::consts::OS, std::env::consts::ARCH)
            .ok_or(InstallError::NoAssetForPlatform {
                os: std::env::consts::OS,
                arch: std::env::consts::ARCH,
                version: version_string.clone(),
            })?;
        let asset = release.asset(&choice.name).ok_or(InstallError::NoAssetForPlatform {
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
            version: version_string.clone(),
        })?;
        let checksums_asset = release
            .asset(CHECKSUMS_ASSET)
            .ok_or(InstallError::NoChecksums { version: version_string.clone() })?;

        emit(progress, InstallPhase::DownloadingChecksums, 0, None, Some(&version_string));
        let listing = self
            .get_bytes(&checksums_asset.url, MAX_CHECKSUMS_BYTES, InstallPhase::DownloadingChecksums, None)
            .await?;
        let listing = String::from_utf8_lossy(&listing).into_owned();
        let expected = checksum_for(&listing, &choice.name)
            .ok_or(InstallError::NoChecksums { version: version_string.clone() })?;

        // Held entirely in memory: a download that fails verification is then
        // simply dropped, and no unverified bytes ever exist at a path that
        // anything could later execute.
        let archive = self
            .get_bytes(&asset.url, MAX_ARCHIVE_BYTES, InstallPhase::DownloadingArchive, progress)
            .await?;

        emit(progress, InstallPhase::Verifying, archive.len() as u64, Some(archive.len() as u64), Some(&version_string));
        let actual = hex::encode(Sha256::digest(&archive));
        if !constant_time_eq(actual.as_bytes(), expected.as_bytes()) {
            return Err(InstallError::ChecksumMismatch {
                asset: choice.name.clone(),
                expected,
                actual,
            });
        }

        emit(progress, InstallPhase::Extracting, 0, None, Some(&version_string));
        let executable = extract_kopia(&archive, choice.kind, &choice.name)?;

        emit(progress, InstallPhase::Installing, 0, None, Some(&version_string));
        let replaced = self.target.is_file();
        self.install_bytes(&executable, &release.version).await?;
        emit(progress, InstallPhase::Done, 0, None, Some(&version_string));

        Ok(InstallOutcome {
            version: release.version.clone(),
            path: self.target.clone(),
            asset: choice.name,
            sha256: actual,
            signature_verified: false,
            emulated: choice.emulated,
            replaced_previous: replaced,
        })
    }

    /// Refuse downgrades and anything below the configured minimum.
    async fn check_version_acceptable(
        &self,
        release: &ReleaseInfo,
        settings: &Settings,
    ) -> InstallResult<()> {
        let floor = super::binary::configured_floor(settings).to_semver();
        if release.version < floor {
            return Err(InstallError::RefusedVersion {
                reason: format!(
                    "kopia {} is older than the required minimum of {floor}",
                    release.version
                ),
            });
        }
        if release.version < MINIMUM_KOPIA_VERSION.to_semver() {
            return Err(InstallError::RefusedVersion {
                reason: format!(
                    "kopia {} is older than the {} this build can drive",
                    release.version, MINIMUM_KOPIA_VERSION
                ),
            });
        }
        // A pinned version is an explicit instruction, including "go back".
        if settings.kopia.pinned_version.is_some() {
            return Ok(());
        }
        if let Some(current) = self.installed_version().await {
            if release.version < current {
                return Err(InstallError::RefusedVersion {
                    reason: format!(
                        "kopia {current} is already installed and {} would be a downgrade",
                        release.version
                    ),
                });
            }
        }
        Ok(())
    }

    /// Write the executable atomically, then prove it works before it becomes
    /// the binary the rest of the application will run.
    async fn install_bytes(&self, bytes: &[u8], expected: &Version) -> InstallResult<()> {
        std::fs::create_dir_all(&self.target_dir)
            .map_err(|e| InstallError::Io(format!("creating {}: {e}", self.target_dir.display())))?;

        // Same directory as the target, so the rename is atomic and never
        // crosses a filesystem boundary. Keeps the `.exe` suffix on Windows so
        // the temporary file is still executable while it is being verified.
        let stem = format!(".kopia-install-{}-{}", std::process::id(), Utc::now().timestamp_nanos_opt().unwrap_or(0));
        let temp = self
            .target_dir
            .join(if cfg!(windows) { format!("{stem}.exe") } else { stem });

        write_executable(&temp, bytes)
            .map_err(|e| InstallError::Io(format!("writing {}: {e}", temp.display())))?;

        // A binary that will not run, or that is not the version the release
        // promised, is not installed. Verifying the temporary file means a
        // failure here leaves the previous working kopia untouched.
        let probe = KopiaBinary::probe_with_floor(
            &temp,
            KopiaSource::Bundled,
            &KopiaVersion::new(0, 0, 0),
        )
        .await;
        let reported = match probe {
            Ok(b) => b.version().clone(),
            Err(e) => {
                let _ = std::fs::remove_file(&temp);
                return Err(InstallError::VersionMismatch {
                    expected: expected.to_string(),
                    reported: format!("nothing usable ({e})"),
                });
            }
        };
        let expected_v = KopiaVersion::from_semver(expected);
        if reported != expected_v {
            let _ = std::fs::remove_file(&temp);
            return Err(InstallError::VersionMismatch {
                expected: expected.to_string(),
                reported: reported.to_string(),
            });
        }

        // Windows refuses to rename over an existing file, and refuses to
        // replace one that is currently executing — which is precisely the
        // "a backup is running" case that must not be forced.
        #[cfg(windows)]
        if self.target.exists() {
            if let Err(e) = std::fs::remove_file(&self.target) {
                let _ = std::fs::remove_file(&temp);
                return Err(if e.kind() == std::io::ErrorKind::PermissionDenied {
                    InstallError::Busy
                } else {
                    InstallError::Io(format!("replacing {}: {e}", self.target.display()))
                });
            }
        }

        std::fs::rename(&temp, &self.target).map_err(|e| {
            let _ = std::fs::remove_file(&temp);
            InstallError::Io(format!("installing {}: {e}", self.target.display()))
        })
    }

    async fn get_json(&self, url: &str) -> InstallResult<serde_json::Value> {
        let body = self.get_bytes(url, MAX_JSON_BYTES, InstallPhase::QueryingRelease, None).await?;
        serde_json::from_slice(&body)
            .map_err(|e| InstallError::Api(format!("unreadable response from GitHub: {e}")))
    }

    /// Fetch a URL, refusing anything not served by GitHub, streaming so that a
    /// size cap can be enforced before the whole body is in memory.
    async fn get_bytes(
        &self,
        url: &str,
        limit: u64,
        phase: InstallPhase,
        progress: Option<&InstallProgressSink>,
    ) -> InstallResult<Vec<u8>> {
        let host = url_host(url)
            .ok_or_else(|| InstallError::Api(format!("malformed download URL {url:?}")))?;
        if !host_allowed(&host, &self.allowed_hosts) {
            return Err(InstallError::UntrustedHost { url_host: host });
        }

        let response = self
            .client
            .get(url)
            .timeout(if limit > MAX_CHECKSUMS_BYTES { DOWNLOAD_TIMEOUT } else { METADATA_TIMEOUT })
            .header("Accept", "application/octet-stream, application/vnd.github+json, */*")
            .send()
            .await
            .map_err(|e| classify_transport_error(&e))?;

        // A redirect the policy stopped surfaces as a 3xx response rather than
        // an error, so it is checked here too.
        let final_host = response.url().host_str().unwrap_or_default().to_string();
        if !host_allowed(&final_host, &self.allowed_hosts) {
            return Err(InstallError::UntrustedHost { url_host: final_host });
        }
        let status = response.status();
        if status.as_u16() == 403 || status.as_u16() == 429 {
            return Err(InstallError::RateLimited);
        }
        if status.is_redirection() {
            return Err(InstallError::UntrustedHost {
                url_host: response
                    .headers()
                    .get("location")
                    .and_then(|h| h.to_str().ok())
                    .and_then(url_host)
                    .unwrap_or_else(|| "an unknown host".into()),
            });
        }
        if !status.is_success() {
            return Err(InstallError::Api(format!("GitHub returned HTTP {status}")));
        }

        let total = response.content_length();
        if let Some(t) = total {
            if t > limit {
                return Err(InstallError::TooLarge { limit });
            }
        }

        let mut body: Vec<u8> = Vec::with_capacity(total.unwrap_or(0).min(limit) as usize);
        let mut response = response;
        loop {
            let chunk = response.chunk().await.map_err(|e| classify_transport_error(&e))?;
            let Some(chunk) = chunk else { break };
            if body.len() as u64 + chunk.len() as u64 > limit {
                return Err(InstallError::TooLarge { limit });
            }
            body.extend_from_slice(&chunk);
            if let Some(sink) = progress {
                sink.emit(InstallProgress {
                    phase,
                    downloaded_bytes: body.len() as u64,
                    total_bytes: total,
                    version: None,
                });
            }
        }
        Ok(body)
    }
}

fn emit(
    sink: Option<&InstallProgressSink>,
    phase: InstallPhase,
    downloaded: u64,
    total: Option<u64>,
    version: Option<&str>,
) {
    if let Some(s) = sink {
        s.emit(InstallProgress {
            phase,
            downloaded_bytes: downloaded,
            total_bytes: total,
            version: version.map(|v| v.to_string()),
        });
    }
}

/// Distinguish "no network" from "GitHub said no", because the two mean very
/// different things to the user.
fn classify_transport_error(e: &reqwest::Error) -> InstallError {
    if e.is_timeout() || e.is_connect() || e.is_request() {
        InstallError::Network(e.to_string())
    } else {
        InstallError::Api(e.to_string())
    }
}

/// Whether a download may come from this host.
///
/// Exact matches from the allowlist, plus any `*.githubusercontent.com`
/// subdomain — GitHub has renamed its release CDN host at least once
/// (`objects.` → `release-assets.`) and a hard-coded list would have turned
/// that into a silent outage for every user.
pub fn host_allowed(host: &str, allowed: &[String]) -> bool {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        return false;
    }
    if allowed.iter().any(|a| a.eq_ignore_ascii_case(&host)) {
        return true;
    }
    // Only widen for GitHub's own CDN, and only when the allowlist is GitHub's
    // — a test or mirror allowlist must not inherit the wildcard.
    allowed.iter().any(|a| a.eq_ignore_ascii_case("github.com"))
        && host.ends_with(GITHUB_CDN_SUFFIX)
        && !host[..host.len() - GITHUB_CDN_SUFFIX.len()].contains('/')
}

/// Host of a URL, without pulling in a URL parser for one field.
fn url_host(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit('@').next()?;
    // Strip the port, but not an IPv6 literal's colons.
    let host = if authority.starts_with('[') {
        authority.split(']').next()?.trim_start_matches('[')
    } else {
        authority.split(':').next()?
    };
    let host = host.trim();
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// Constant-time comparison of two hex digests.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    use subtle::ConstantTimeEq;
    a.len() == b.len() && bool::from(a.ct_eq(b))
}

/// Whether enough time has passed since the last check.
pub fn due_for_check(mgmt: &KopiaManagement, now: DateTime<Utc>) -> bool {
    match mgmt.last_check_at {
        None => true,
        Some(last) => {
            let interval = ChronoDuration::hours(i64::from(mgmt.check_interval_hours.max(1)));
            // A clock that jumped backwards must not disable update checks
            // forever, so a `last_check_at` in the future counts as due.
            last > now || now - last >= interval
        }
    }
}

// ---------------------------------------------------------------------------
// Archive handling
// ---------------------------------------------------------------------------

/// The file name we are looking for inside the archive.
fn executable_member_name() -> &'static str {
    if cfg!(windows) {
        "kopia.exe"
    } else {
        "kopia"
    }
}

/// Reject any archive member that could escape the directory it is extracted
/// into.
///
/// Kopia's archives contain exactly one directory (`kopia-<version>-<platform>/`)
/// with three files in it, so nothing legitimate is affected. A backup tool that
/// unpacks a downloaded archive at startup is precisely the place a zip-slip
/// would be devastating, so every member is checked whether or not it is the one
/// being extracted.
///
/// Returns the member's final path component when it is safe, `Err` when it is
/// not.
pub fn safe_member_name(raw: &str) -> std::result::Result<String, InstallError> {
    let unsafe_member = || InstallError::UnsafeArchive { member: raw.to_string() };
    if raw.is_empty() {
        return Err(unsafe_member());
    }
    // Windows accepts both separators; normalise before judging.
    let normalised = raw.replace('\\', "/");
    if normalised.starts_with('/') {
        return Err(unsafe_member());
    }
    // A drive-letter or UNC prefix is absolute on Windows and meaningless
    // elsewhere; either way it has no business in a release archive.
    if normalised.len() >= 2 && normalised.as_bytes()[1] == b':' {
        return Err(unsafe_member());
    }
    let mut last = None;
    for component in Path::new(&normalised).components() {
        match component {
            Component::Normal(c) => last = c.to_str().map(|s| s.to_string()),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(unsafe_member());
            }
        }
    }
    last.ok_or_else(unsafe_member)
}

/// Pull the kopia executable out of a release archive.
pub fn extract_kopia(
    archive: &[u8],
    kind: ArchiveKind,
    asset_name: &str,
) -> InstallResult<Vec<u8>> {
    let found = match kind {
        ArchiveKind::Zip => extract_from_zip(archive)?,
        ArchiveKind::TarGz => extract_from_targz(archive)?,
    };
    found.ok_or_else(|| InstallError::ExecutableNotFound { asset: asset_name.to_string() })
}

fn extract_from_zip(archive: &[u8]) -> InstallResult<Option<Vec<u8>>> {
    let cursor = std::io::Cursor::new(archive);
    let mut zip = zip::ZipArchive::new(cursor)
        .map_err(|e| InstallError::Api(format!("the download is not a valid zip archive: {e}")))?;
    let mut result = None;
    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| InstallError::Api(format!("unreadable zip entry: {e}")))?;
        let raw = entry.name().to_string();
        if raw.ends_with('/') || raw.ends_with('\\') {
            // A directory entry still has to be safe, but has no contents.
            safe_member_name(raw.trim_end_matches(['/', '\\']))?;
            continue;
        }
        let name = safe_member_name(&raw)?;
        if result.is_none() && name.eq_ignore_ascii_case(executable_member_name()) {
            result = Some(read_capped(&mut entry, MAX_BINARY_BYTES)?);
        }
    }
    Ok(result)
}

fn extract_from_targz(archive: &[u8]) -> InstallResult<Option<Vec<u8>>> {
    let decoder = flate2::read::GzDecoder::new(std::io::Cursor::new(archive));
    let mut tar = tar::Archive::new(decoder);
    let entries = tar
        .entries()
        .map_err(|e| InstallError::Api(format!("the download is not a valid tar archive: {e}")))?;
    let mut result = None;
    for entry in entries {
        let mut entry =
            entry.map_err(|e| InstallError::Api(format!("unreadable tar entry: {e}")))?;
        let raw = entry
            .path()
            .map_err(|e| InstallError::Api(format!("unreadable tar entry name: {e}")))?
            .to_string_lossy()
            .into_owned();
        if raw.ends_with('/') {
            safe_member_name(raw.trim_end_matches('/'))?;
            continue;
        }
        let name = safe_member_name(&raw)?;
        if result.is_none() && name == executable_member_name() {
            result = Some(read_capped(&mut entry, MAX_BINARY_BYTES)?);
        }
    }
    Ok(result)
}

/// Read at most `limit` bytes, failing rather than truncating: a truncated
/// executable that still ran would be far worse than a refused install.
fn read_capped<R: Read>(r: &mut R, limit: u64) -> InstallResult<Vec<u8>> {
    let mut buf = Vec::new();
    let read = r
        .take(limit + 1)
        .read_to_end(&mut buf)
        .map_err(|e| InstallError::Io(format!("reading the archive: {e}")))?;
    if read as u64 > limit {
        return Err(InstallError::TooLarge { limit });
    }
    Ok(buf)
}

/// Write a file and make it executable on Unix.
fn write_executable(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::File::create(path)?;
    f.write_all(bytes)?;
    f.flush()?;
    f.sync_all()?;
    drop(f);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_names_match_the_real_release() {
        // Verbatim from kopia/kopia v0.23.1.
        assert_eq!(
            asset_for_platform("0.23.1", "windows", "x86_64").map(|a| a.name),
            Some("kopia-0.23.1-windows-x64.zip".to_string())
        );
        assert_eq!(
            asset_for_platform("0.23.1", "linux", "x86_64").map(|a| a.name),
            Some("kopia-0.23.1-linux-x64.tar.gz".to_string())
        );
        assert_eq!(
            asset_for_platform("0.23.1", "linux", "aarch64").map(|a| a.name),
            Some("kopia-0.23.1-linux-arm64.tar.gz".to_string())
        );
        assert_eq!(
            asset_for_platform("0.23.1", "macos", "aarch64").map(|a| a.name),
            Some("kopia-0.23.1-macOS-arm64.tar.gz".to_string())
        );
        assert_eq!(
            asset_for_platform("0.23.1", "macos", "x86_64").map(|a| a.name),
            Some("kopia-0.23.1-macOS-x64.tar.gz".to_string())
        );
    }

    #[test]
    fn windows_on_arm_falls_back_to_the_emulated_x64_build() {
        // Kopia publishes no windows-arm64 asset; the fallback is deliberate
        // and is reported rather than hidden.
        let a = asset_for_platform("0.23.1", "windows", "aarch64").expect("some asset");
        assert_eq!(a.name, "kopia-0.23.1-windows-x64.zip");
        assert!(a.emulated);
        assert!(!asset_for_platform("0.23.1", "linux", "x86_64").expect("linux").emulated);
    }

    #[test]
    fn an_unsupported_platform_has_no_asset() {
        assert!(asset_for_platform("0.23.1", "solaris", "sparc64").is_none());
        assert!(asset_for_platform("0.23.1", "windows", "riscv64").is_none());
    }

    #[test]
    fn checksums_are_read_in_the_real_sha256sum_format() {
        // Verbatim lines from kopia v0.23.1's checksums.txt.
        let listing = "\
416d0f84a3dbb321a8b2d8f0997b1a0a6e915babe79ee76fa6e4d2bd1e1c5178  kopia-0.23.1-linux-x64.tar.gz
19e6ed637221f4dfd46a46e978ec4c509c386b522d746db2cd6762b217478111  kopia-0.23.1-macOS-arm64.tar.gz
";
        assert_eq!(
            checksum_for(listing, "kopia-0.23.1-linux-x64.tar.gz").as_deref(),
            Some("416d0f84a3dbb321a8b2d8f0997b1a0a6e915babe79ee76fa6e4d2bd1e1c5178")
        );
        assert_eq!(checksum_for(listing, "kopia-0.23.1-windows-x64.zip"), None);
    }

    #[test]
    fn binary_mode_checksums_are_accepted_too() {
        let listing = "d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0 *kopia-1.0.0-windows-x64.zip\n";
        assert_eq!(
            checksum_for(listing, "kopia-1.0.0-windows-x64.zip").as_deref(),
            Some("d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0")
        );
    }

    #[test]
    fn junk_in_the_checksum_file_is_ignored_not_trusted() {
        let listing = "not a hash  kopia-1.0.0-linux-x64.tar.gz\n# a comment\n\n";
        assert_eq!(checksum_for(listing, "kopia-1.0.0-linux-x64.tar.gz"), None);
    }

    #[test]
    fn release_tags_parse_with_and_without_the_v() {
        assert_eq!(parse_release_version("v0.23.1"), Some(Version::new(0, 23, 1)));
        assert_eq!(parse_release_version("0.23.1"), Some(Version::new(0, 23, 1)));
        assert_eq!(parse_release_version("0.23"), Some(Version::new(0, 23, 0)));
        assert_eq!(parse_release_version("nightly"), None);
    }

    #[test]
    fn only_github_hosts_are_accepted() {
        let allowed: Vec<String> =
            DEFAULT_ALLOWED_HOSTS.iter().map(|s| s.to_string()).collect();
        for good in [
            "github.com",
            "api.github.com",
            "objects.githubusercontent.com",
            // GitHub renamed its release CDN; the suffix rule keeps working.
            "release-assets.githubusercontent.com",
            "GITHUB.COM",
        ] {
            assert!(host_allowed(good, &allowed), "{good} should be allowed");
        }
        for bad in [
            "evil.com",
            "github.com.evil.com",
            "githubusercontent.com.evil.net",
            "notgithub.com",
            "",
            "localhost",
            "127.0.0.1",
        ] {
            assert!(!host_allowed(bad, &allowed), "{bad} must be refused");
        }
    }

    #[test]
    fn a_test_allowlist_does_not_inherit_the_github_wildcard() {
        let local = vec!["127.0.0.1".to_string()];
        assert!(host_allowed("127.0.0.1", &local));
        assert!(!host_allowed("evil.githubusercontent.com", &local));
    }

    #[test]
    fn url_hosts_are_extracted_without_a_url_parser() {
        assert_eq!(url_host("https://github.com/kopia/kopia/releases"), Some("github.com".into()));
        assert_eq!(url_host("http://127.0.0.1:38211/x"), Some("127.0.0.1".into()));
        assert_eq!(url_host("https://user:pw@evil.com/a"), Some("evil.com".into()));
        assert_eq!(url_host("https://[::1]:8080/a"), Some("::1".into()));
        assert_eq!(url_host("not-a-url"), None);
    }

    #[test]
    fn archive_members_that_escape_are_refused() {
        assert_eq!(safe_member_name("kopia-0.23.1-linux-x64/kopia").as_deref(), Ok("kopia"));
        assert_eq!(safe_member_name("kopia").as_deref(), Ok("kopia"));
        for evil in [
            "../kopia",
            "../../etc/cron.d/evil",
            "a/../../b",
            "/etc/passwd",
            "/absolute",
            "C:\\Windows\\System32\\evil.exe",
            "..\\..\\kopia.exe",
            "",
        ] {
            assert!(safe_member_name(evil).is_err(), "{evil:?} must be refused");
        }
    }

    #[test]
    fn the_check_interval_is_honoured_and_survives_a_clock_jump() {
        let mut mgmt = KopiaManagement { check_interval_hours: 24, ..Default::default() };
        let now = Utc::now();
        assert!(due_for_check(&mgmt, now), "a machine that has never checked is due");

        mgmt.last_check_at = Some(now - ChronoDuration::hours(1));
        assert!(!due_for_check(&mgmt, now), "checking hourly would get us rate-limited");

        mgmt.last_check_at = Some(now - ChronoDuration::hours(25));
        assert!(due_for_check(&mgmt, now));

        // A clock that jumped backwards must not disable checks forever.
        mgmt.last_check_at = Some(now + ChronoDuration::days(400));
        assert!(due_for_check(&mgmt, now));
    }

    #[test]
    fn constant_time_comparison_still_compares() {
        assert!(constant_time_eq(b"abcd", b"abcd"));
        assert!(!constant_time_eq(b"abcd", b"abce"));
        assert!(!constant_time_eq(b"abcd", b"abc"));
    }

    #[test]
    fn install_errors_are_actionable_and_flagged() {
        let mismatch = InstallError::ChecksumMismatch {
            asset: "kopia-0.23.1-windows-x64.zip".into(),
            expected: "aa".into(),
            actual: "bb".into(),
        };
        assert!(mismatch.is_security_event());
        assert!(mismatch.message().contains("nothing was installed"));
        assert!(mismatch.hint().is_some());
        assert!(!InstallError::RateLimited.is_security_event());
        assert!(InstallError::UnsafeArchive { member: "../x".into() }.is_security_event());
    }
}
