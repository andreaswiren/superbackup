//! Optional caching of the master passphrase in the OS keychain.
//!
//! Gated by [`Settings::use_os_keychain`](superbackup_core::model::Settings),
//! which is **off by default and must stay that way**. Caching the master
//! passphrase moves the security boundary from "something the user knows" to
//! "something the machine holds", and that is a trade only the machine's owner
//! may make. The setting exists for the real case it serves: an unattended
//! workstation that must back up without anyone typing a passphrase at 02:00.
//!
//! ## Status on this build: not available
//!
//! The `keyring` crate is a dependency of **`superbackup-core`**, not of
//! `superbackup`, and core exposes no wrapper around it. This crate therefore
//! has no way to reach a platform keychain, and no `Cargo.toml` in this
//! workspace may be edited by the workstream that owns this file.
//!
//! Two one-line fixes exist, either of which makes the rest of this module
//! work unchanged:
//!
//! 1. add `keyring` to `crates/app/Cargo.toml` and fill in the three bodies
//!    below, or — better —
//! 2. add a `core::platform::keychain` module with `store`/`load`/`forget`,
//!    so that the *only* code holding a plaintext passphrase stays inside the
//!    crate whose whole job is holding secrets safely.
//!
//! Until then this module is deliberately loud rather than quietly useless:
//! [`explain_unavailable`] returns a sentence the daemon logs and records as an
//! activity event whenever the setting is switched on, so a user who ticks the
//! box is told it did not take effect instead of discovering at 02:00 that it
//! did not.
//!
//! ## The contract the eventual implementation must honour
//!
//! * **Service name** `superbackup`, **account** the vault's identity — see
//!   [`entry_name`]. Keying on the configuration root rather than a constant
//!   is what lets a per-user tray and a machine-wide service coexist without
//!   handing each other the other's passphrase.
//! * `store` overwrites; `forget` must not fail when there is nothing to
//!   forget, because it is called on the failure path of `load`.
//! * `load` returning the wrong passphrase must be survivable: the caller
//!   ([`super::lifecycle::try_keychain_unlock`]) treats a failed unlock as
//!   "the passphrase was rotated elsewhere", discards the entry, and falls
//!   back to asking.
//! * Nothing here may log, display, or return the passphrase in an error.

use superbackup_core::paths::Paths;
use superbackup_core::secret::Secret;
use superbackup_core::{Error, Result};

/// The keychain service name. Stable: a user looking through Credential
/// Manager or Keychain Access should see one recognisable entry.
pub const SERVICE: &str = "superbackup";

/// The account name for one installation's entry.
///
/// Derived from the configuration root so that the per-user instance and the
/// machine-wide service — which have different vaults and different
/// passphrases — cannot read each other's entry.
pub fn entry_name(paths: &Paths) -> String {
    let scope = if paths.service_scope { "service" } else { "user" };
    // The config directory is not a secret, and including it keeps portable
    // installs under `SUPERBACKUP_HOME` distinct from the default one.
    format!("{scope}:{}", paths.config_dir.display())
}

/// Whether this build can reach a platform keychain at all.
pub fn available() -> bool {
    false
}

/// Why not, in a sentence fit for the activity log and the Settings page.
pub fn explain_unavailable() -> &'static str {
    "This build cannot use the operating system's keychain, so \"remember my passphrase\" has no \
     effect and superbackup will keep asking. Unlock it manually, or install the service under an \
     account that stays signed in."
}

/// Cache the passphrase.
pub fn store(paths: &Paths, _passphrase: &Secret) -> Result<()> {
    let _ = entry_name(paths);
    Err(Error::Platform(explain_unavailable().to_string()))
}

/// Read the cached passphrase, or `None` when there is not one.
///
/// Never an error: every caller treats "no cached passphrase" and "the
/// keychain would not answer" identically, and turning the second into a
/// failure would break unlocking on a machine whose keyring daemon is not
/// running.
pub fn load(paths: &Paths) -> Option<Secret> {
    let _ = entry_name(paths);
    None
}

/// Remove the cached passphrase. Succeeds when there was nothing to remove.
pub fn forget(paths: &Paths) -> Result<()> {
    let _ = entry_name(paths);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_entry_name_separates_the_service_from_the_user_instance() {
        let user = Paths::rooted_at("/tmp/sb", false);
        let service = Paths::rooted_at("/tmp/sb", true);
        assert_ne!(entry_name(&user), entry_name(&service));
        assert!(entry_name(&user).starts_with("user:"));
        assert!(entry_name(&service).starts_with("service:"));
    }

    #[test]
    fn forgetting_a_passphrase_that_was_never_cached_succeeds() {
        assert!(forget(&Paths::rooted_at("/tmp/sb", false)).is_ok());
    }

    #[test]
    fn storing_fails_loudly_rather_than_pretending() {
        let err = store(&Paths::rooted_at("/tmp/sb", false), &Secret::from_str("x"))
            .expect_err("this build has no keychain");
        assert!(err.to_string().contains("keychain"), "{err}");
    }
}
