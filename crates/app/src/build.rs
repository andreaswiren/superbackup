//! What this binary actually is.
//!
//! The version in `Cargo.toml` answers "which release is this meant to be",
//! which is not the same question as "which build am I running". Both a tagged
//! 0.1.0 and a working-tree build three commits later report `0.1.0`, and a bug
//! report against the second one is close to useless if it is indistinguishable
//! from the first.
//!
//! So the commit, and whether the tree was clean, are stamped in at compile
//! time by `build.rs` and shown wherever the version is shown. A build from a
//! source tarball with no git metadata degrades to just the version, which is
//! honest — it says as much as is actually known.

/// The release version from `Cargo.toml`.
///
/// This is the value the release workflow checks against the tag and against
/// the changelog heading, so bumping it is what makes a release coherent.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Short commit hash, or empty when built without git metadata.
pub const GIT_SHA: &str = env!("SUPERBACKUP_GIT_SHA");

/// `git describe` against tags, e.g. `v0.1.0-rc.1-4-gabc123def-dirty`.
pub const GIT_DESCRIBE: &str = env!("SUPERBACKUP_GIT_DESCRIBE");

/// Whether tracked files were modified when this was built.
pub fn is_dirty() -> bool {
    env!("SUPERBACKUP_GIT_DIRTY") == "1"
}

/// Was this built from a clean checkout of a tagged commit?
///
/// `git describe` appends `-<n>-g<sha>` once there are commits after the tag,
/// so a bare tag name means the build sits exactly on it.
pub fn is_release_build() -> bool {
    !GIT_SHA.is_empty()
        && !is_dirty()
        && !GIT_DESCRIBE.is_empty()
        && !GIT_DESCRIBE.contains("-g")
        && !GIT_DESCRIBE.ends_with("-dirty")
}

/// One line for a status bar: `0.1.0` for a release, `0.1.0+abc123def` for a
/// build from a later commit, `0.1.0+abc123def-modified` for a dirty tree.
pub fn short() -> String {
    if GIT_SHA.is_empty() || is_release_build() {
        return VERSION.to_string();
    }
    if is_dirty() {
        format!("{VERSION}+{GIT_SHA}-modified")
    } else {
        format!("{VERSION}+{GIT_SHA}")
    }
}

/// The full identity, for About, `superbackup version` and bug reports.
pub fn long() -> String {
    let mut out = format!("superbackup {VERSION}");
    if !GIT_SHA.is_empty() {
        out.push_str(&format!(" ({GIT_SHA}"));
        if is_dirty() {
            out.push_str(", modified working tree");
        }
        out.push(')');
    }
    out.push_str(&format!(" on {} {}", std::env::consts::OS, std::env::consts::ARCH));
    out
}

/// Machine-readable form for `superbackup version --json` and diagnostics.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BuildIdentity {
    pub version: &'static str,
    pub git_sha: &'static str,
    pub git_describe: &'static str,
    pub dirty: bool,
    pub release_build: bool,
    pub target_os: &'static str,
    pub target_arch: &'static str,
}

pub fn identity() -> BuildIdentity {
    BuildIdentity {
        version: VERSION,
        git_sha: GIT_SHA,
        git_describe: GIT_DESCRIBE,
        dirty: is_dirty(),
        release_build: is_release_build(),
        target_os: std::env::consts::OS,
        target_arch: std::env::consts::ARCH,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_version_matches_the_manifest() {
        // The release workflow checks the tag and the changelog against this
        // same value, so a mismatch here would make a release incoherent.
        assert_eq!(VERSION, env!("CARGO_PKG_VERSION"));
        assert!(!VERSION.is_empty());
    }

    #[test]
    fn a_build_always_identifies_itself() {
        // Whatever the git situation, these must produce something usable —
        // they are shown in a status bar and pasted into bug reports.
        assert!(short().starts_with(VERSION));
        assert!(long().contains(VERSION));
        assert!(long().contains(std::env::consts::ARCH));
    }

    #[test]
    fn a_modified_tree_is_never_reported_as_a_release() {
        if is_dirty() {
            assert!(!is_release_build(), "a dirty tree must not claim to be a release build");
            assert!(short().ends_with("-modified"));
        }
    }

    /// The version in `Cargo.toml` and the changelog must agree.
    ///
    /// This exists because they did not: a release's worth of work — a fix for
    /// a bug that made every backup impossible among it — accumulated under
    /// `[Unreleased]` while the manifest still said 0.1.0, so every build
    /// reported a version that had shipped before any of it existed. A version
    /// number nobody moves is worse than none, because it actively misleads a
    /// bug report.
    #[test]
    fn the_changelog_has_a_heading_for_this_version() {
        let changelog = include_str!("../../../CHANGELOG.md");
        let heading = format!("## [{VERSION}]");
        assert!(
            changelog.contains(&heading),
            "Cargo.toml says {VERSION} but CHANGELOG.md has no `{heading}` section.              Either bump the version or cut the section — a build that reports a version              with no changelog entry cannot be told apart from the release that used it."
        );
    }

    /// Work sitting under `[Unreleased]` means the version is behind.
    #[test]
    fn unreleased_is_empty_whenever_a_version_has_been_cut() {
        let changelog = include_str!("../../../CHANGELOG.md");
        let Some(start) = changelog.find("## [Unreleased]") else { return };
        let rest = &changelog[start + "## [Unreleased]".len()..];
        // Everything up to the next version heading.
        let body = match rest.find(
            "
## [",
        ) {
            Some(end) => &rest[..end],
            None => rest,
        };
        let has_entries = body.lines().any(|l| l.trim_start().starts_with('-'));
        assert!(
            !has_entries,
            "CHANGELOG.md has entries under [Unreleased] while Cargo.toml says {VERSION}.              Bump the version and move them under it, so the build reports what it contains."
        );
    }

    #[test]
    fn the_identity_serialises() {
        let json = serde_json::to_string(&identity()).expect("identity must serialise");
        assert!(json.contains("\"version\""));
        assert!(json.contains("\"dirty\""));
    }
}
