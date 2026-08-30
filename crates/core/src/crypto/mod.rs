//! The encrypted secret vault.
//!
//! # What this protects, and against whom
//!
//! `config.sbvault` holds every repository passphrase, every S3 access-key
//! pair and every API token this installation owns, plus (when published) the
//! configuration document that refers to them. If it is lost, every backup is
//! lost with it. If it is opened by someone else, every backup and every
//! bucket is theirs.
//!
//! The file is explicitly designed to be committed to a Git repository so
//! several machines can share one definition — see [`crate::remote`]. So the
//! threat model is not "someone stole the laptop"; it is:
//!
//! > An adversary has the file. They have unlimited time, a rack of GPUs, and
//! > full knowledge of this source code. The only thing they lack is the
//! > master passphrase.
//!
//! Everything below follows from that sentence.
//!
//! | Concern | Mechanism |
//! |---|---|
//! | Offline guessing | Argon2id, m = 256 MiB, t = 3, p = 1 ([`kdf`]) |
//! | Confidentiality + integrity | XChaCha20-Poly1305, random 192-bit nonce ([`envelope`]) |
//! | Parameter downgrade | The whole header is the AEAD's associated data |
//! | Key reuse across purposes | HKDF-SHA256 with versioned `info` strings ([`keys`]) |
//! | Losing the file | Timestamped backups + atomic replace ([`self::file`]) |
//! | Reading from a locked vault | Type-gated accessors ([`vault::OpenVault`]) |
//! | Publishing under a forged identity | Ed25519 detached signatures ([`signing`]) |
//! | Rotation silently orphaning repositories | A required acknowledgement ([`rekey`]) |
//! | Hostile file causing a crash or OOM | Bounds checked before any allocation |
//!
//! # What it does not protect against
//!
//! A weak master passphrase. Argon2id multiplies an attacker's cost by a large
//! constant; it does not turn `hunter2` into a secret. [`crate::secret::estimate_strength`]
//! exists to nag the user, and the "write this down" flow exists so that the
//! repository passphrases themselves are 256 random bits regardless of what the
//! human chose.
//!
//! It also does not protect a running process: an unlocked vault holds key
//! material in memory, and an attacker with debugger access to the process has
//! already won. Zeroing on drop narrows the window; it does not close it.
//!
//! # Quick start
//!
//! ```no_run
//! use superbackup_core::crypto::{Vault, VaultFile};
//! use superbackup_core::model::SecretRef;
//! use superbackup_core::paths::Paths;
//! use superbackup_core::secret::Secret;
//!
//! # fn main() -> superbackup_core::Result<()> {
//! let paths = Paths::discover()?;
//! let mut file = VaultFile::create(&paths, &Secret::from_str("correct horse battery staple"))?;
//! file.vault_mut().put(SecretRef("s3.access:…".into()), Secret::from_str("AKIA…"))?;
//! file.save()?;
//! # Ok(())
//! # }
//! ```

pub mod envelope;
pub mod file;
pub mod kdf;
pub mod keys;
pub mod rekey;
pub mod signing;
pub mod vault;

pub use envelope::{AeadAlgorithm, Envelope, SignatureAlgorithm, VaultHeader, VaultSignature};
pub use file::{BackupReason, VaultFile, BACKUP_KEEP};
pub use kdf::{calibrate, KdfAlgorithm, KdfParams, CALIBRATION_TARGET};
pub use keys::{encode_passphrase, generate_passphrase, MasterKeys};
pub use rekey::{
    derived_repositories, DerivedRepository, MigrationReport, MigrationState, Rekey,
    RekeyAcknowledgement, RepositoryCredentials, RepositoryMigration,
};
pub use vault::{OpenVault, RekeyPlan, SealedVault, Vault, VaultEntry};

use crate::error::{Error, Result};

// ---------------------------------------------------------------------------
// Shared primitives
// ---------------------------------------------------------------------------

/// Fill a buffer from the operating system CSPRNG.
///
/// Every random byte in this module — salts, nonces, generated passphrases —
/// comes from here and never from a userspace PRNG seeded once at startup. A
/// thread-local PRNG is fine for shuffling a list; it is not fine for a nonce
/// that must never repeat across a fork, a VM snapshot restore, or a machine
/// image cloned onto fifty desktops.
///
/// Failure is surfaced rather than swallowed: a system that cannot produce
/// randomness must not be handed a predictable nonce as a consolation prize.
pub fn fill_random(out: &mut [u8]) -> Result<()> {
    use rand::TryRngCore;
    rand::rngs::OsRng
        .try_fill_bytes(out)
        .map_err(|e| Error::Crypto(format!("the operating system CSPRNG failed: {e}")))
}

/// [`fill_random`] into a fresh `Vec`.
pub fn random_bytes(len: usize) -> Result<Vec<u8>> {
    let mut out = vec![0u8; len];
    fill_random(&mut out)?;
    Ok(out)
}

/// Standard base64 with padding, used for every binary field in the vault file.
///
/// Padded and standard rather than URL-safe: these values live inside JSON
/// string literals, never inside a URL, and the padded form is what every
/// other tool a user might reach for (`base64 -d`, Python, `jq`) expects.
fn b64_encode(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn b64_decode(text: &str) -> Result<Vec<u8>> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(text)
        .map_err(|e| Error::VaultCorrupt(format!("invalid base64: {e}")))
}

/// Base64 for a file body being uploaded to a hosting API.
///
/// Exposed so [`crate::remote`] does not have to take its own base64
/// dependency and cannot accidentally pick a different alphabet from the one
/// the vault format uses.
pub fn base64_for_upload(bytes: &[u8]) -> String {
    b64_encode(bytes)
}

/// The inverse of [`base64_for_upload`], for a body served by a hosting API.
pub fn base64_from_download(text: &str) -> Result<Vec<u8>> {
    b64_decode(text)
}

/// `serde` adaptor for `Vec<u8>` fields rendered as base64 strings.
///
/// Written by hand rather than pulled from a helper crate so that the decode
/// path returns our own [`Error::VaultCorrupt`] wording and, more importantly,
/// so that a malformed field is a parse error rather than a panic.
pub(crate) mod b64 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&super::b64_encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(d)?;
        super::b64_decode(&text).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_bytes_are_random() {
        let a = random_bytes(32).expect("a");
        let b = random_bytes(32).expect("b");
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
        assert!(a.iter().any(|&x| x != 0), "an all-zero salt would be catastrophic");
    }

    #[test]
    fn base64_round_trips_and_rejects_garbage() {
        let bytes = vec![0u8, 1, 2, 250, 251, 255];
        assert_eq!(b64_decode(&b64_encode(&bytes)).expect("decode"), bytes);
        assert!(b64_decode("not valid base64!!!").is_err());
    }
}
