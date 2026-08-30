//! Detached Ed25519 signatures over a sealed vault.
//!
//! # What a signature adds that the AEAD does not
//!
//! Decrypting the vault already proves the file was produced by *someone
//! holding the master passphrase* — Poly1305 will not authenticate anything
//! else. So why sign at all?
//!
//! Because [`crate::model::RemoteConfigSource::trusted_signers`] pins a *set*
//! of identities, and a set is not a single passphrase. A household or a team
//! can share one Git repository between several installations, each with its
//! own vault and its own master passphrase, and pin each other's public keys.
//! Then "this vault opens with my passphrase" and "this vault came from the
//! machine I expect" become separate, independently checkable claims. The
//! signature is also checked *before* the passphrase is used, so a machine can
//! reject an unrecognised publisher without spending half a second on Argon2
//! and without the user typing anything.
//!
//! # Key derivation and identity
//!
//! The signing key is the `superbackup/v1/signing` HKDF subkey (see
//! [`super::keys::MasterKeys::signing_seed`]) used directly as an Ed25519
//! seed. It is therefore a deterministic function of the master key, which
//! means every machine that shares a vault shares one signing identity, and
//! restoring a vault from backup restores the identity with it — there is no
//! separate key file to lose.
//!
//! The consequence, which is deliberate, is that rotating the master
//! passphrase also rotates the signing identity, so a pinned fingerprint has
//! to be re-pinned afterwards. [`super::rekey::Rekey::new_signer_fingerprint`]
//! hands the caller the new value at the moment of the rotation, precisely so
//! that this is a thing the GUI can show rather than a thing the user
//! discovers when the next pull is rejected.
//!
//! # Fail closed
//!
//! Every function here returns an error rather than a boolean, and every
//! caller treats an error as "reject". [`crate::remote::verify_signature`]
//! rejects a vault when signers are pinned and the vault is unsigned, when the
//! signer is not on the list, when the embedded public key does not match its
//! own stated fingerprint, and when the signature does not verify. A security
//! control that cannot be evaluated must never degrade into one that passes.

use crate::error::{Error, Result};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// Length of an Ed25519 seed, in bytes.
pub const SEED_LEN: usize = ed25519_dalek::SECRET_KEY_LENGTH;
/// Length of an Ed25519 public key, in bytes.
pub const PUBLIC_KEY_LEN: usize = ed25519_dalek::PUBLIC_KEY_LENGTH;
/// Length of an Ed25519 signature, in bytes.
pub const SIGNATURE_LEN: usize = ed25519_dalek::SIGNATURE_LENGTH;

/// Number of hex characters in a fingerprint.
pub const FINGERPRINT_CHARS: usize = 32;

/// Domain separator, so a fingerprint can never be confused with any other
/// SHA-256 this program computes over the same bytes.
const FINGERPRINT_DOMAIN: &[u8] = b"superbackup/v1/signer-fingerprint";

/// The public identity of a signing key, as pinned in `trusted_signers`.
///
/// `SHA-256(domain || 0x00 || public_key)`, truncated to 16 bytes and rendered
/// as lowercase hex: 32 characters, short enough to compare by eye in a
/// settings screen or read down a phone line, and 128 bits — far beyond any
/// feasible collision search for a value whose only job is to name a key that
/// is itself carried in the file.
///
/// Computed over the **public key**, never over the seed. That is what lets a
/// verifier holding only the signature recompute the fingerprint and check it
/// against the pinned list; a seed-derived fingerprint would be unverifiable
/// by anyone except the signer, which would make pinning theatre.
pub fn fingerprint(public_key: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(FINGERPRINT_DOMAIN);
    h.update([0u8]);
    h.update(public_key);
    hex::encode(&h.finalize()[..16])
}

/// The Ed25519 public key for a signing seed.
pub fn public_key(seed: &[u8]) -> Result<[u8; PUBLIC_KEY_LEN]> {
    Ok(signing_key(seed)?.verifying_key().to_bytes())
}

/// The fingerprint of the identity a seed signs as.
///
/// Equal to `fingerprint(&public_key(seed)?)` by construction; provided so
/// that callers cannot accidentally fingerprint the seed instead of the key.
pub fn seed_fingerprint(seed: &[u8]) -> Result<String> {
    Ok(fingerprint(&public_key(seed)?))
}

/// Sign `payload` with the signing subkey.
///
/// Ed25519 signing is deterministic (RFC 8032): the same seed and the same
/// payload always produce the same 64 bytes, with no nonce to get wrong and no
/// dependence on the system RNG. That also means re-signing an unchanged vault
/// does not churn a synced Git repository.
pub fn sign(seed: &[u8], payload: &[u8]) -> Result<Vec<u8>> {
    Ok(signing_key(seed)?.sign(payload).to_bytes().to_vec())
}

/// Verify a detached signature, and that the key that made it is the key the
/// caller was promised.
///
/// Both halves matter, and dropping either one breaks pinning:
///
/// * the signature must verify against `public_key` — otherwise anyone can
///   attach a well-formed but meaningless 64 bytes;
/// * `public_key` must hash to `expected_fingerprint` — otherwise an attacker
///   signs with their own key while leaving a pinned fingerprint in the
///   `signer` field, and the upstream "is this signer pinned?" check and the
///   "does this signature verify?" check each pass, against different keys.
///
/// Verification uses `verify_strict`, which rejects small-order and
/// torsion-component public keys. The permissive `verify` accepts signatures
/// that validate under more than one public key, which is exactly the property
/// a pinned-signer list must not have.
pub fn verify(
    expected_fingerprint: &str,
    public_key: &[u8],
    payload: &[u8],
    signature: &[u8],
) -> Result<()> {
    let key_bytes: [u8; PUBLIC_KEY_LEN] = public_key.try_into().map_err(|_| {
        Error::Crypto(format!(
            "signing key is {} bytes; Ed25519 public keys are {PUBLIC_KEY_LEN}",
            public_key.len()
        ))
    })?;

    // Bind the key to its advertised identity before doing anything with it.
    let actual = fingerprint(&key_bytes);
    let expected = expected_fingerprint.trim().to_ascii_lowercase();
    let matches: bool = actual.as_bytes().ct_eq(expected.as_bytes()).into();
    if !matches {
        return Err(Error::Crypto(format!(
            "the signature carries key {actual} but claims to be from \
             {expected}; the file has been tampered with"
        )));
    }

    let verifying = VerifyingKey::from_bytes(&key_bytes)
        .map_err(|_| Error::Crypto("signing key is not a valid Ed25519 public key".into()))?;
    let signature_bytes: [u8; SIGNATURE_LEN] = signature.try_into().map_err(|_| {
        Error::Crypto(format!(
            "signature is {} bytes; Ed25519 signatures are {SIGNATURE_LEN}",
            signature.len()
        ))
    })?;
    verifying
        .verify_strict(payload, &Signature::from_bytes(&signature_bytes))
        .map_err(|_| Error::Crypto("the vault's signature does not verify".into()))
}

/// Whether this build can produce and check signatures.
///
/// Kept as a function now that it returns `true`: the GUI branches on it, and
/// a build stripped down for a platform without curve25519 would flip it back
/// rather than have every call site deleted and later reinstated.
pub fn is_available() -> bool {
    true
}

fn signing_key(seed: &[u8]) -> Result<SigningKey> {
    let seed: [u8; SEED_LEN] = seed.try_into().map_err(|_| {
        Error::Crypto(format!("signing seed is {} bytes; Ed25519 seeds are {SEED_LEN}", seed.len()))
    })?;
    Ok(SigningKey::from_bytes(&seed))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEED: [u8; 32] = [7u8; 32];

    #[test]
    fn signatures_round_trip() {
        let public = public_key(&SEED).expect("public key");
        let payload = b"the sealed vault";
        let signature = sign(&SEED, payload).expect("sign");
        assert_eq!(signature.len(), SIGNATURE_LEN);
        verify(&fingerprint(&public), &public, payload, &signature).expect("verify");
    }

    #[test]
    fn signing_is_deterministic_so_resigning_does_not_churn_the_file() {
        assert_eq!(sign(&SEED, b"payload").expect("a"), sign(&SEED, b"payload").expect("b"));
    }

    #[test]
    fn a_tampered_payload_does_not_verify() {
        let public = public_key(&SEED).expect("public key");
        let signature = sign(&SEED, b"the sealed vault").expect("sign");
        let err = verify(&fingerprint(&public), &public, b"the sealed vaulu", &signature)
            .expect_err("must reject");
        assert!(format!("{err}").contains("does not verify"), "{err}");
    }

    #[test]
    fn a_tampered_signature_does_not_verify() {
        let public = public_key(&SEED).expect("public key");
        let mut signature = sign(&SEED, b"payload").expect("sign");
        signature[0] ^= 1;
        assert!(verify(&fingerprint(&public), &public, b"payload", &signature).is_err());
        signature[SIGNATURE_LEN - 1] ^= 1;
        assert!(verify(&fingerprint(&public), &public, b"payload", &signature).is_err());
    }

    #[test]
    fn another_key_does_not_verify() {
        let public = public_key(&SEED).expect("public key");
        let signature = sign(&[9u8; 32], b"payload").expect("sign");
        assert!(verify(&fingerprint(&public), &public, b"payload", &signature).is_err());
    }

    /// The attack the fingerprint binding exists to stop.
    #[test]
    fn a_valid_signature_from_a_substituted_key_is_rejected() {
        // The attacker signs the payload perfectly well with their own key but
        // leaves the pinned fingerprint in place. Without the key-to-
        // fingerprint binding, the upstream pinning check and the signature
        // check would each pass, against two different keys.
        let pinned = fingerprint(&public_key(&SEED).expect("pinned key"));
        let attacker_seed = [42u8; 32];
        let attacker_public = public_key(&attacker_seed).expect("attacker key");
        let signature = sign(&attacker_seed, b"payload").expect("sign");

        let err = verify(&pinned, &attacker_public, b"payload", &signature)
            .expect_err("a key that is not the pinned one must be rejected");
        assert!(format!("{err}").contains("tampered"), "{err}");
    }

    #[test]
    fn fingerprints_are_over_the_public_key_not_the_seed() {
        let public = public_key(&SEED).expect("public key");
        assert_eq!(seed_fingerprint(&SEED).expect("seed fingerprint"), fingerprint(&public));
        assert_ne!(
            fingerprint(&public),
            fingerprint(&SEED),
            "fingerprinting the seed would make pinning unverifiable by anyone else"
        );
        assert_eq!(fingerprint(&public).len(), FINGERPRINT_CHARS);
        assert!(fingerprint(&public)
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    #[test]
    fn the_fingerprint_does_not_expose_the_seed_or_the_key() {
        let public = public_key(&SEED).expect("public key");
        let fp = fingerprint(&public);
        assert!(!hex::encode(SEED).contains(&fp));
        assert!(!hex::encode(public).contains(&fp));
    }

    #[test]
    fn malformed_inputs_are_errors_not_panics() {
        let public = public_key(&SEED).expect("public key");
        let good = sign(&SEED, b"payload").expect("sign");

        assert!(sign(&[0u8; 31], b"payload").is_err(), "short seed");
        assert!(sign(&[0u8; 33], b"payload").is_err(), "long seed");
        assert!(public_key(&[]).is_err());
        assert!(verify(&fingerprint(&public), &[], b"payload", &good).is_err(), "empty key");
        assert!(verify(&fingerprint(&public), &public, b"payload", &[]).is_err(), "no signature");
        assert!(
            verify(&fingerprint(&public), &public, b"payload", &[0u8; 64]).is_err(),
            "zero signature"
        );
        assert!(verify("", &public, b"payload", &good).is_err(), "empty fingerprint");
        assert!(verify("not hex at all", &public, b"payload", &good).is_err());
    }

    #[test]
    fn a_small_order_public_key_is_rejected() {
        // `verify_strict` refuses keys like this, which admit signatures that
        // validate under several public keys at once.
        let zero = [0u8; PUBLIC_KEY_LEN];
        assert!(verify(&fingerprint(&zero), &zero, b"payload", &[0u8; 64]).is_err());
    }

    #[test]
    fn signing_is_available() {
        assert!(is_available());
    }
}
