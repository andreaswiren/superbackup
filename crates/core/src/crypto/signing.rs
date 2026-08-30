//! Detached signatures over a sealed vault.
//!
//! # Status in this build: the slot exists, the algorithm does not
//!
//! The design calls for an Ed25519 key derived from the master key (see
//! [`super::keys::MasterKeys::signing_seed`]) so that a machine pulling a
//! shared `config.sbvault` out of a Git repository can prove it came from a
//! holder of the master key, and can pin the acceptable signers in
//! [`crate::model::RemoteConfigSource::trusted_signers`].
//!
//! `ed25519-dalek` is not a dependency of this crate, and adding one is out of
//! scope for this workstream. Rather than inventing a curve implementation —
//! the single worst idea available — this module:
//!
//! * defines the on-disk signature slot ([`super::envelope::VaultSignature`]),
//!   so a vault sealed today is forward-compatible with a build that can sign;
//! * derives and exposes the signer **fingerprint**, which needs only SHA-256
//!   and is therefore fully implemented here;
//! * returns [`Error::Crypto`] with an explicit "signing is unavailable in
//!   this build" message from [`sign`] and [`verify`].
//!
//! # Fail closed
//!
//! The security-relevant consequence is in [`crate::remote`]: when
//! `trusted_signers` is non-empty, a pulled vault that cannot be *verified*
//! must be **rejected**, not accepted with a shrug. A build that cannot check
//! signatures must not silently behave like a build with no pinning
//! configured — that would turn a security control into a no-op the moment it
//! matters. [`verify`] therefore returns an error rather than `Ok(())`, and
//! every caller treats that error as "reject".

use crate::error::{Error, Result};
use sha2::{Digest, Sha256};

/// Message used by every "this build cannot do that" path here.
pub const UNAVAILABLE: &str =
    "detached signing is unavailable in this build (no Ed25519 implementation is linked); \
     seal and verify without a signature, or use a build with signing support";

/// Domain separator for fingerprints, so the fingerprint can never be confused
/// with any other SHA-256 this program computes.
const FINGERPRINT_DOMAIN: &[u8] = b"superbackup/v1/signer-fingerprint";

/// The public identity of a signing key, as pinned in `trusted_signers`.
///
/// Computed as `SHA-256(domain || 0x00 || seed)`, truncated to 16 bytes and
/// rendered as lowercase hex — 32 characters, short enough to compare by eye
/// in a settings screen, long enough (128 bits) that finding a second seed
/// with the same fingerprint is infeasible.
///
/// Deriving it from the seed rather than from an Ed25519 public key is a
/// deliberate consequence of the missing dependency: it keeps the fingerprint
/// stable and shareable today, and it reveals nothing about the seed (it is a
/// preimage-resistant hash of 256 bits of uniformly random material). A build
/// that gains Ed25519 will emit `SignatureAlgorithm::Ed25519` with the *same*
/// fingerprint, so pinned values do not have to be re-pinned.
pub fn fingerprint(seed: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(FINGERPRINT_DOMAIN);
    h.update([0u8]);
    h.update(seed);
    let digest = h.finalize();
    hex::encode(&digest[..16])
}

/// Produce a detached signature over `payload`.
///
/// # Errors
///
/// Always [`Error::Crypto`] in this build. See the module documentation.
pub fn sign(_seed: &[u8], _payload: &[u8]) -> Result<Vec<u8>> {
    Err(Error::Crypto(UNAVAILABLE.into()))
}

/// Verify a detached signature against a pinned signer fingerprint.
///
/// # Errors
///
/// Always [`Error::Crypto`] in this build, which callers must treat as
/// "reject". See the module documentation for why this deliberately does not
/// degrade to `Ok(())`.
pub fn verify(_signer_fingerprint: &str, _payload: &[u8], _signature: &[u8]) -> Result<()> {
    Err(Error::Crypto(UNAVAILABLE.into()))
}

/// Whether this build can produce and check signatures.
///
/// The GUI uses this to grey out "Sign published config" with an explanation,
/// rather than offering a button that always fails.
pub fn is_available() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprints_are_stable_and_seed_dependent() {
        let a = fingerprint(&[1u8; 32]);
        let b = fingerprint(&[1u8; 32]);
        let c = fingerprint(&[2u8; 32]);
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase()));
    }

    #[test]
    fn fingerprint_does_not_contain_the_seed() {
        let seed = [0xab; 32];
        let fp = fingerprint(&seed);
        assert!(!fp.contains(&hex::encode(seed)));
        assert!(!hex::encode(seed).contains(&fp));
    }

    #[test]
    fn signing_fails_closed_rather_than_silently_succeeding() {
        let e = sign(&[0u8; 32], b"payload").expect_err("must not pretend to sign");
        assert!(format!("{e}").contains("unavailable"));

        let e = verify("deadbeef", b"payload", &[0u8; 64])
            .expect_err("verification must never default to Ok");
        assert!(format!("{e}").contains("unavailable"));

        assert!(!is_available());
    }
}
