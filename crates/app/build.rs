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
