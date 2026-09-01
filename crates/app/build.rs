//! Build script: embed the Windows resources.
//!
//! Without this the executable has no icon of its own, so Explorer, the
//! taskbar, the Start menu, Alt-Tab and the shortcut a user pins all fall back
//! to the generic Rust binary icon — and the Details tab of the file's
//! properties is empty, which is what a code-signing check and a corporate
//! deployment tool both read.
//!
//! Everything here is Windows-only and a no-op elsewhere; macOS takes its icon
//! from the `.app` bundle's `Info.plist` and Linux from the `.desktop` entry,
//! neither of which is produced at compile time.

fn main() {
    // The icon is real content: rebuild if it changes, or a designer's update
    // silently fails to reach the binary.
    println!("cargo:rerun-if-changed=../../assets/icons/superbackup.ico");
    println!("cargo:rerun-if-changed=build.rs");

    emit_build_identity();

    #[cfg(windows)]
    embed_windows_resources();
}

#[cfg(windows)]
fn embed_windows_resources() {
    let icon = std::path::Path::new("../../assets/icons/superbackup.ico");
    if !icon.exists() {
        // A missing icon must not break the build for someone who has only
        // checked out the source; warn loudly instead.
        println!(
            "cargo:warning=assets/icons/superbackup.ico is missing; the executable \
             will have no icon. Run `python tools/icons/build.py` to regenerate it."
        );
        return;
    }

    let mut res = winresource::WindowsResource::new();
    res.set_icon(icon.to_str().unwrap_or_default())
        // Shown in the file's Properties -> Details, and by deployment tooling.
        .set("ProductName", "superbackup")
        .set("FileDescription", "superbackup — backup manager")
        .set("CompanyName", "Andreas Wiren")
        .set("LegalCopyright", "Copyright (c) 2026 Andreas Wiren. MIT licensed.")
        .set("OriginalFilename", "superbackup.exe")
        .set("InternalName", "superbackup");

    if let Err(e) = res.compile() {
        // Cross-compiling to Windows from a host without a resource compiler is
        // a legitimate configuration. Losing the icon is a cosmetic failure and
        // must not stop the build.
        println!("cargo:warning=could not embed Windows resources: {e}");
    }
}

/// Stamp the build with where it came from.
///
/// `CARGO_PKG_VERSION` alone cannot answer the question a user actually asks
/// when something is wrong: *is this the released 0.1.0, or a build somebody
/// made from a working tree three commits later?* Both report "0.1.0". The
/// commit, and whether the tree was dirty, are what distinguish them — and a
/// bug report against an unknown build is close to worthless.
///
/// Everything here degrades to empty rather than failing: building from a
/// source tarball with no `.git` is perfectly legitimate.
fn emit_build_identity() {
    // Re-run when the checked-out commit changes, or the stamp goes stale and
    // starts lying — which is worse than not having it.
    for path in ["../../.git/HEAD", "../../.git/refs/heads/main"] {
        if std::path::Path::new(path).exists() {
            println!("cargo:rerun-if-changed={path}");
        }
    }

    let sha = run_git(&["rev-parse", "--short=9", "HEAD"]).unwrap_or_default();
    // `--quiet --exit-code` reports tracked modifications by exit status alone.
    let dirty = std::process::Command::new("git")
        .args(["diff", "--quiet", "--ignore-submodules", "HEAD"])
        .current_dir("../..")
        .status()
        .map(|s| !s.success())
        .unwrap_or(false);
    // The nearest tag, so a build made from a release commit says so.
    let described = run_git(&["describe", "--tags", "--always", "--dirty"]).unwrap_or_default();

    println!("cargo:rustc-env=SUPERBACKUP_GIT_SHA={sha}");
    println!("cargo:rustc-env=SUPERBACKUP_GIT_DIRTY={}", if dirty { "1" } else { "0" });
    println!("cargo:rustc-env=SUPERBACKUP_GIT_DESCRIBE={described}");
}

fn run_git(args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git").args(args).current_dir("../..").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?;
    let text = text.trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}
