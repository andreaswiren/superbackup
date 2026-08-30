//! Finding the kopia executable and deciding whether we can drive it.
//!
//! Three things have to be true before superbackup will touch a repository:
//!
//! 1. We know **which** kopia we are running. A user with kopia on `PATH`, a
//!    bundled build, and an explicit override in Settings must get exactly the
//!    one they asked for, in that priority order, and the GUI must be able to
//!    show which one won.
//! 2. We know **what version** it is, because kopia's CLI surface has moved:
//!    error-correction coding, `repository throttle`, `validate-provider` and
//!    the hidden `--json-verbose` flag all arrived at different releases.
//!    Guessing wrong means a flag that silently does nothing.
//! 3. It is **new enough**. Refusing up front with a clear message beats
//!    failing halfway through a repository create with `unknown long flag`.

use crate::error::{Error, Result};
use crate::model::Settings;
use crate::paths::Paths;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The oldest kopia this driver will drive.
///
/// Chosen as the first release in which everything the driver depends on is
/// present and stable together: ECC at repository-create time (0.13),
/// `repository throttle set` (0.10), `repository validate-provider` (0.9), and
/// the `snapshot create --json` manifest shape used by the manifest parser.
/// 0.17 is a comfortable margin above all of them and is what the pinned
/// bundled build tracks. Development and the recorded test fixtures target the
/// 0.21 line.
pub const MINIMUM_KOPIA_VERSION: KopiaVersion = KopiaVersion { major: 0, minor: 17, patch: 0 };

/// How long to wait for `kopia --version`. It does no I/O beyond loading the
/// binary, so anything slower than this means a stalled network drive or a
/// virus scanner, and blocking the GUI on it is not acceptable.
const VERSION_PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// A parsed kopia version. Build metadata is kept for the About screen and for
/// bug reports but never participates in comparison.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct KopiaVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl KopiaVersion {
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        KopiaVersion { major, minor, patch }
    }

    pub fn at_least(&self, other: &KopiaVersion) -> bool {
        self >= other
    }

    /// Bridge to [`semver::Version`], which is what the release-tracking code
    /// in [`super::install`] compares against GitHub tags.
    pub fn to_semver(&self) -> semver::Version {
        semver::Version::new(u64::from(self.major), u64::from(self.minor), u64::from(self.patch))
    }

    /// Narrowing conversion from a release version. Pre-release and build
    /// metadata are dropped, matching [`KopiaVersion::parse`].
    pub fn from_semver(v: &semver::Version) -> KopiaVersion {
        KopiaVersion {
            major: v.major.min(u64::from(u32::MAX)) as u32,
            minor: v.minor.min(u64::from(u32::MAX)) as u32,
            patch: v.patch.min(u64::from(u32::MAX)) as u32,
        }
    }

    /// Parse the first token of `kopia --version`.
    ///
    /// The real output is built in kopia's `main.go` as
    /// `BuildVersion + " build: " + BuildInfo + " from: " + BuildGitHubRepo`,
    /// e.g. `0.21.1 build: 8f0e1c2d from: kopia/kopia`. Development builds emit
    /// things like `v20260830.0.104645`, and distributions sometimes prefix a
    /// `v`, so the parser is forgiving about everything except the digits.
    pub fn parse(output: &str) -> Option<KopiaVersion> {
        let token = output.split_whitespace().next()?;
        let token = token.trim_start_matches(['v', 'V']);
        // Drop any pre-release/build suffix: `0.21.1-rc1`, `0.21.1+dirty`.
        let core = token.split(['-', '+']).next()?;
        let mut parts = core.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next().unwrap_or("0").parse().unwrap_or(0);
        let patch = parts.next().unwrap_or("0").parse().unwrap_or(0);
        Some(KopiaVersion { major, minor, patch })
    }
}

impl std::fmt::Display for KopiaVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Where the executable we settled on came from. Surfaced verbatim in the
/// Settings screen and in `superbackup doctor`, because "kopia is too old" is
/// impossible to act on without knowing *which* kopia.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KopiaSource {
    /// `Settings::kopia_path`, set by the user.
    Configured,
    /// The build superbackup downloaded into its own data directory.
    Bundled,
    /// Found on `PATH`.
    SystemPath,
}

impl KopiaSource {
    pub fn title(&self) -> &'static str {
        match self {
            KopiaSource::Configured => "configured in Settings",
            KopiaSource::Bundled => "bundled with superbackup",
            KopiaSource::SystemPath => "found on PATH",
        }
    }
}

/// A kopia executable that has been located, probed, and accepted.
///
/// Holding this type is the proof that a usable kopia exists; every command
/// builder requires one, so no code path can accidentally shell out to an
/// unverified binary.
#[derive(Debug, Clone)]
pub struct KopiaBinary {
    path: PathBuf,
    version: KopiaVersion,
    source: KopiaSource,
    /// The raw `--version` line, for the About screen and bug reports.
    banner: String,
}

impl KopiaBinary {
    /// Locate kopia.
    ///
    /// The order is a policy decision, not an accident:
    ///
    /// 1. [`Settings::kopia_path`] — used verbatim if set, and never managed or
    ///    replaced. A user who pinned a path said something deliberate, and
    ///    quietly overwriting their build would be indefensible.
    /// 2. A kopia on `PATH`, when [`KopiaManagement::prefer_system_binary`] is
    ///    on (the default) and it meets the minimum version. If somebody has
    ///    already installed kopia, that is the one they expect to be used.
    /// 3. The managed build under [`Paths::bundled_kopia`], which superbackup
    ///    downloads and keeps current.
    ///
    /// A system kopia that is *too old* is skipped rather than rejected, so the
    /// managed build can take over without the user having to uninstall
    /// anything. Installing one when none of the three exist is
    /// [`super::install::KopiaInstaller::ensure_available`], not this function:
    /// discovery must stay usable with no network and no side effects.
    ///
    /// [`KopiaManagement::prefer_system_binary`]: crate::model::KopiaManagement::prefer_system_binary
    /// [`Paths::bundled_kopia`]: crate::paths::Paths::bundled_kopia
    pub async fn discover(settings: &Settings, paths: &Paths) -> Result<KopiaBinary> {
        // An explicit path is absolute: if it is set and broken, say so rather
        // than silently using something else the user did not ask for.
        if let Some(explicit) = &settings.kopia_path {
            return KopiaBinary::probe_with_floor(
                explicit,
                KopiaSource::Configured,
                &MINIMUM_KOPIA_VERSION,
            )
            .await;
        }

        let floor = configured_floor(settings);
        let system = which::which("kopia").ok();
        let bundled = paths.bundled_kopia();

        let mut attempts: Vec<(PathBuf, KopiaSource)> = Vec::new();
        if settings.kopia.prefer_system_binary {
            if let Some(p) = system.clone() {
                attempts.push((p, KopiaSource::SystemPath));
            }
            attempts.push((bundled, KopiaSource::Bundled));
        } else {
            attempts.push((bundled, KopiaSource::Bundled));
            if let Some(p) = system {
                attempts.push((p, KopiaSource::SystemPath));
            }
        }

        let mut last_error: Option<Error> = None;
        for (path, source) in attempts {
            if !path.is_file() && source == KopiaSource::Bundled {
                continue;
            }
            match KopiaBinary::probe_with_floor(&path, source, &floor).await {
                Ok(bin) => return Ok(bin),
                Err(e) => last_error = Some(e),
            }
        }
        Err(last_error.unwrap_or(Error::KopiaMissing))
    }

    /// Probe one specific path against the hard minimum. Used by the Settings
    /// "Browse…" button so the user gets an immediate verdict on the file they
    /// picked.
    pub async fn probe(path: &Path, source: KopiaSource) -> Result<KopiaBinary> {
        KopiaBinary::probe_with_floor(path, source, &MINIMUM_KOPIA_VERSION).await
    }

    /// Probe against a caller-supplied version floor, which is the configured
    /// minimum raised to at least the driver's own hard requirement.
    pub async fn probe_with_floor(
        path: &Path,
        source: KopiaSource,
        floor: &KopiaVersion,
    ) -> Result<KopiaBinary> {
        let banner = run_version(path).await?;
        let version = KopiaVersion::parse(&banner).ok_or_else(|| {
            Error::Validation(format!(
                "{} did not report a kopia version (it printed {:?})",
                path.display(),
                banner.chars().take(80).collect::<String>()
            ))
        })?;
        let floor = if floor.at_least(&MINIMUM_KOPIA_VERSION) { floor } else { &MINIMUM_KOPIA_VERSION };
        if !version.at_least(floor) {
            return Err(Error::Validation(format!(
                "kopia {version} at {} is too old; superbackup needs {floor} or newer",
                path.display()
            )));
        }
        Ok(KopiaBinary { path: path.to_path_buf(), version, source, banner })
    }

    /// Construct without probing.
    ///
    /// For callers that already know the version — the daemon rehydrating its
    /// state after a restart, and the test suite driving a recorded fake kopia
    /// which must not pay for a process spawn per test.
    pub fn assume(path: impl Into<PathBuf>, version: KopiaVersion, source: KopiaSource) -> Self {
        let banner = format!("{version} (not probed)");
        KopiaBinary { path: path.into(), version, source, banner }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn version(&self) -> &KopiaVersion {
        &self.version
    }
    pub fn source(&self) -> KopiaSource {
        self.source
    }
    pub fn banner(&self) -> &str {
        &self.banner
    }

    /// Whether this build accepts the hidden `--json-verbose` flag, which is
    /// the only way to get snapshot statistics out of `snapshot create --json`.
    /// Present for the whole supported range; kept as a predicate so a future
    /// removal is one edit rather than a hunt.
    pub fn supports_json_verbose(&self) -> bool {
        true
    }

    /// Whether `repository throttle set` exists. Added in kopia 0.10.
    pub fn supports_throttling(&self) -> bool {
        self.version.at_least(&KopiaVersion::new(0, 10, 0))
    }

    /// Whether `repository create --ecc` exists. Added in kopia 0.13.
    pub fn supports_ecc(&self) -> bool {
        self.version.at_least(&KopiaVersion::new(0, 13, 0))
    }
}

/// The effective version floor: the user's configured minimum, but never below
/// the driver's own hard requirement.
///
/// A user lowering `minimum_version` cannot talk superbackup into driving a
/// kopia whose output this code cannot parse; they can only raise the bar.
pub fn configured_floor(settings: &Settings) -> KopiaVersion {
    let configured = KopiaVersion::parse(&settings.kopia.minimum_version);
    match configured {
        Some(v) if v.at_least(&MINIMUM_KOPIA_VERSION) => v,
        _ => MINIMUM_KOPIA_VERSION,
    }
}

/// Run `<path> --version` and return its trimmed first line.
///
/// Kingpin writes the version to stdout and exits 0. A binary that is not
/// kopia at all typically writes to stderr and exits non-zero, so both streams
/// are captured and the failure is reported with whatever it said.
async fn run_version(path: &Path) -> Result<String> {
    let mut cmd = tokio::process::Command::new(path);
    cmd.arg("--version");
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    cmd.kill_on_drop(true);
    super::command::harden_child(&mut cmd);

    let output = match tokio::time::timeout(VERSION_PROBE_TIMEOUT, cmd.output()).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => return Err(Error::KopiaMissing),
        Ok(Err(e)) => {
            return Err(Error::io(format!("running {} --version", path.display()), e));
        }
        Err(_) => {
            return Err(Error::Validation(format!(
                "{} did not respond to --version within {} seconds",
                path.display(),
                VERSION_PROBE_TIMEOUT.as_secs()
            )));
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
    if !output.status.success() && line.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Validation(format!(
            "{} is not a working kopia executable: {}",
            path.display(),
            crate::redact::scrub(stderr.trim()).chars().take(200).collect::<String>()
        )));
    }
    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_real_version_banner() {
        let v = KopiaVersion::parse("0.21.1 build: 8f0e1c2d from: kopia/kopia").expect("parses");
        assert_eq!(v, KopiaVersion::new(0, 21, 1));
    }

    #[test]
    fn tolerates_prefixes_and_suffixes() {
        assert_eq!(KopiaVersion::parse("v0.18.2"), Some(KopiaVersion::new(0, 18, 2)));
        assert_eq!(KopiaVersion::parse("0.20.0-rc1 build: x"), Some(KopiaVersion::new(0, 20, 0)));
        assert_eq!(KopiaVersion::parse("1.0"), Some(KopiaVersion::new(1, 0, 0)));
        assert_eq!(KopiaVersion::parse("20260830.0.104645 build: dev"),
            Some(KopiaVersion::new(20260830, 0, 104645)));
    }

    #[test]
    fn rejects_output_that_is_not_a_version() {
        assert_eq!(KopiaVersion::parse(""), None);
        assert_eq!(KopiaVersion::parse("bash: kopia: command not found"), None);
        assert_eq!(KopiaVersion::parse("restic 0.16.0"), None);
    }

    #[test]
    fn ordering_is_numeric_not_lexicographic() {
        assert!(KopiaVersion::new(0, 21, 0) > KopiaVersion::new(0, 9, 9));
        assert!(KopiaVersion::new(0, 17, 0).at_least(&MINIMUM_KOPIA_VERSION));
        assert!(!KopiaVersion::new(0, 16, 9).at_least(&MINIMUM_KOPIA_VERSION));
    }

    #[test]
    fn capability_predicates_track_the_release_they_landed_in() {
        let old = KopiaBinary::assume("kopia", KopiaVersion::new(0, 9, 0), KopiaSource::SystemPath);
        assert!(!old.supports_throttling());
        assert!(!old.supports_ecc());
        let new = KopiaBinary::assume("kopia", KopiaVersion::new(0, 21, 1), KopiaSource::Bundled);
        assert!(new.supports_throttling());
        assert!(new.supports_ecc());
        assert_eq!(new.source(), KopiaSource::Bundled);
    }
}
