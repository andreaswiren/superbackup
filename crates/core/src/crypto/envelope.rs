//! The `config.sbvault` file format.
//!
//! # Why JSON with base64 payloads rather than a packed binary struct
//!
//! This file is designed to be committed to a Git repository — possibly a
//! public one — and pulled onto other machines. That constraint, not
//! aesthetics, chose the encoding:
//!
//! 1. **Git on Windows mangles binary files.** With the default
//!    `core.autocrlf=true` that ships in Git for Windows, a blob Git guesses
//!    to be text has its `0x0A` bytes rewritten to `0x0D 0x0A` on checkout.
//!    A packed binary envelope would be silently corrupted on the second
//!    machine, and the failure would surface as "incorrect master passphrase"
//!    — the single worst error message this application can show. A pure-ASCII
//!    file with no bare newlines inside its payloads cannot be damaged this
//!    way, with or without a `.gitattributes` the user forgot to add.
//! 2. **A human can audit it.** Someone who finds this file in a repository
//!    can read the header, see `argon2id`, `m=262144`, and satisfy themselves
//!    that the thing is actually encrypted, without a hex editor or this
//!    program.
//! 3. **Git can store it sanely.** Line-oriented text packs and transfers
//!    better than an opaque blob, and a diff shows *which* field changed.
//!
//! The cost is roughly 35 % size overhead from base64 and a parser that must
//! be written defensively. Neither matters for a file measured in kilobytes.
//! Security is not traded away for any of this: the ciphertext is the same
//! bytes either way, and the header is bound into the AEAD (see below).
//!
//! # Layout
//!
//! ```json
//! {
//!   "magic": "SBVAULT",
//!   "format_version": 1,
//!   "header": {
//!     "kdf": { "algorithm": "argon2id", "version": 19, "memory_kib": 262144,
//!              "iterations": 3, "parallelism": 1, "salt": "…", "output_len": 32 },
//!     "aead": "xchacha20-poly1305",
//!     "created_at": "2026-08-30T10:11:12Z",
//!     "updated_at": "2026-08-30T10:11:12Z",
//!     "vault_id": "8f14e45f-…"
//!   },
//!   "nonce": "…24 random bytes, base64…",
//!   "ciphertext": "…",
//!   "signature": null
//! }
//! ```
//!
//! # Header binding (the important part)
//!
//! The AEAD's associated data is
//! `"SBVAULT" || format_version || canonical_json(header)`, where
//! `canonical_json` is `serde_json`'s serialisation of the *parsed* header
//! struct — fixed field order, no insignificant whitespace. So:
//!
//! * Flipping a bit in the salt, the memory cost, the iteration count or the
//!   AEAD name changes the AAD, and `decrypt` fails. An attacker cannot
//!   downgrade `memory_kib` from 262144 to 8 to make an offline grind cheap;
//!   the file simply stops opening.
//! * Reformatting the JSON — different indentation, different key order,
//!   added whitespace — does *not* break the file, because the AAD is derived
//!   from the parsed values rather than from the bytes on disk. That is
//!   deliberate: a `prettier` run or an editor's "format on save" inside a
//!   config repository must not destroy the vault.
//!
//! The `signature` field is outside the AAD, because it is computed over the
//! finished envelope and cannot be an input to itself. It is covered by
//! [`Envelope::signing_payload`] instead.

use super::kdf::KdfParams;
use crate::error::{Error, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Magic string in every vault file. Present so that a mis-named file, or a
/// Git LFS pointer that replaced the real content, fails loudly.
pub const MAGIC: &str = "SBVAULT";

/// Current envelope format version.
pub const FORMAT_VERSION: u32 = 1;

/// Largest vault file we will even attempt to parse (16 MiB).
///
/// The vault holds a few dozen short secrets and one configuration document.
/// A remote source offering us a gigabyte is either broken or hostile, and we
/// should find that out before allocating.
pub const MAX_VAULT_BYTES: usize = 16 * 1024 * 1024;

/// Nonce length for XChaCha20-Poly1305, in bytes (192 bits).
pub const NONCE_LEN: usize = 24;

/// The AEAD used for the vault body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum AeadAlgorithm {
    /// XChaCha20-Poly1305, RFC 8439 with the HChaCha20 nonce extension.
    ///
    /// Chosen over AES-256-GCM for one reason that dominates all others here:
    /// the 192-bit nonce can be drawn at random for every seal with a
    /// collision probability that stays negligible for ever, whereas GCM's
    /// 96-bit nonce forces either a counter (which we cannot maintain across
    /// machines that all re-seal the same shared vault) or an uncomfortably
    /// close look at the birthday bound. It is also constant-time in software
    /// on machines without AES-NI, which matters for the ARM laptops and cheap
    /// VMs this will run on.
    #[serde(rename = "xchacha20-poly1305")]
    XChaCha20Poly1305,
}

/// Signature algorithms the format can carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum SignatureAlgorithm {
    Ed25519,
}

/// A detached signature over the sealed vault, used when publishing to a
/// shared Git repository.
///
/// `signer` is the fingerprint that [`crate::model::RemoteConfigSource::trusted_signers`]
/// pins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultSignature {
    pub algorithm: SignatureAlgorithm,
    /// Lowercase hex fingerprint of the signing identity.
    pub signer: String,
    #[serde(with = "crate::crypto::b64")]
    pub signature: Vec<u8>,
}

/// Everything a reader needs before it can even attempt to decrypt.
///
/// Serialised into the AEAD's associated data, so every field here is
/// tamper-evident.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultHeader {
    pub kdf: KdfParams,
    pub aead: AeadAlgorithm,
    /// When this vault was first created. Survives passphrase rotation.
    pub created_at: DateTime<Utc>,
    /// When it was last sealed. Changes on every write, which is what lets a
    /// remote sync say "theirs is newer" without trusting Git metadata.
    pub updated_at: DateTime<Utc>,
    /// Stable identity of *this vault*, independent of its contents.
    ///
    /// Lets the remote sync notice "this is a completely different vault, not
    /// a newer version of mine" and refuse to overwrite, instead of cheerfully
    /// replacing the user's keys with a stranger's.
    pub vault_id: Uuid,
}

impl VaultHeader {
    /// A header for a brand-new vault.
    pub fn new(kdf: KdfParams) -> VaultHeader {
        let now = Utc::now();
        VaultHeader {
            kdf,
            aead: AeadAlgorithm::XChaCha20Poly1305,
            created_at: now,
            updated_at: now,
            vault_id: Uuid::new_v4(),
        }
    }

    /// The associated data this header contributes to the AEAD.
    ///
    /// See the module documentation for why this is computed from the parsed
    /// struct rather than from the bytes read off disk.
    pub fn associated_data(&self, format_version: u32) -> Result<Vec<u8>> {
        let mut aad = Vec::with_capacity(256);
        aad.extend_from_slice(MAGIC.as_bytes());
        aad.push(b'\0');
        aad.extend_from_slice(&format_version.to_be_bytes());
        aad.push(b'\0');
        let body = serde_json::to_vec(self)
            .map_err(|e| Error::Crypto(format!("header could not be canonicalised: {e}")))?;
        aad.extend_from_slice(&body);
        Ok(aad)
    }
}

/// The complete on-disk file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    /// Always [`MAGIC`]. Rejected on parse if it is anything else.
    pub magic: String,
    pub format_version: u32,
    pub header: VaultHeader,
    #[serde(with = "crate::crypto::b64")]
    pub nonce: Vec<u8>,
    #[serde(with = "crate::crypto::b64")]
    pub ciphertext: Vec<u8>,
    /// Detached signature, when the vault was sealed for publication.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<VaultSignature>,
}

impl Envelope {
    /// Parse and structurally validate a vault file.
    ///
    /// This does no cryptography and touches no secret material, so it is safe
    /// to run on completely untrusted bytes — for example, whatever a remote
    /// Git repository just handed us. Everything that could make a later step
    /// allocate unboundedly or panic is checked here.
    pub fn parse(bytes: &[u8]) -> Result<Envelope> {
        if bytes.len() > MAX_VAULT_BYTES {
            return Err(Error::VaultCorrupt(format!(
                "vault file is {} bytes; the maximum is {MAX_VAULT_BYTES}",
                bytes.len()
            )));
        }
        if bytes.is_empty() {
            return Err(Error::VaultCorrupt("vault file is empty".into()));
        }

        let envelope: Envelope = serde_json::from_slice(bytes).map_err(|e| {
            // serde's message names the offending field and offset, which is
            // exactly what a user staring at a broken file needs; it cannot
            // contain plaintext because nothing has been decrypted yet.
            Error::VaultCorrupt(format!("not a readable vault file: {e}"))
        })?;
        envelope.validate()?;
        Ok(envelope)
    }

    /// Structural checks shared by parsing and sealing.
    fn validate(&self) -> Result<()> {
        if self.magic != MAGIC {
            return Err(Error::VaultCorrupt(format!(
                "wrong magic {:?}; this is not a superbackup vault",
                // Truncated so a hostile file cannot dump a megabyte into a
                // log line through the error message.
                self.magic.chars().take(16).collect::<String>()
            )));
        }
        if self.format_version > FORMAT_VERSION {
            return Err(Error::VaultVersion {
                found: self.format_version,
                supported: FORMAT_VERSION,
            });
        }
        if self.format_version == 0 {
            return Err(Error::VaultCorrupt("format version 0 does not exist".into()));
        }
        if self.nonce.len() != NONCE_LEN {
            return Err(Error::VaultCorrupt(format!(
                "nonce is {} bytes; XChaCha20-Poly1305 requires {NONCE_LEN}",
                self.nonce.len()
            )));
        }
        // 16 bytes of Poly1305 tag plus at least one byte of body.
        if self.ciphertext.len() < 17 {
            return Err(Error::VaultCorrupt(format!(
                "ciphertext is {} bytes; it has been truncated",
                self.ciphertext.len()
            )));
        }
        self.header.kdf.validate()?;
        Ok(())
    }

    /// The associated data for this envelope's AEAD operations.
    pub fn associated_data(&self) -> Result<Vec<u8>> {
        self.header.associated_data(self.format_version)
    }

    /// The exact bytes a detached signature covers.
    ///
    /// Everything except the signature itself, in a fixed order with explicit
    /// length prefixes so that no rearrangement of nonce and ciphertext can
    /// produce the same byte string (a classic length-extension confusion).
    pub fn signing_payload(&self) -> Result<Vec<u8>> {
        let mut out = self.associated_data()?;
        out.extend_from_slice(&(self.nonce.len() as u64).to_be_bytes());
        out.extend_from_slice(&self.nonce);
        out.extend_from_slice(&(self.ciphertext.len() as u64).to_be_bytes());
        out.extend_from_slice(&self.ciphertext);
        Ok(out)
    }

    /// Serialise to the bytes that go on disk or into Git.
    ///
    /// Pretty-printed with a trailing newline: Git diffs stay readable, and
    /// POSIX tools do not complain about a missing final newline. The
    /// ciphertext is one long base64 line, which changes wholesale on every
    /// seal anyway — a random nonce makes byte-level stability impossible, and
    /// that is the correct trade: nonce reuse would be catastrophic, a noisy
    /// diff is not.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let mut out = serde_json::to_vec_pretty(self)
            .map_err(|e| Error::Crypto(format!("vault could not be serialised: {e}")))?;
        out.push(b'\n');
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::kdf::KdfParams;

    fn envelope() -> Envelope {
        // Realistic cost parameters, so that "an attacker lowers memory_kib"
        // is an actual change rather than a no-op. Nothing here runs the KDF.
        let kdf = KdfParams {
            memory_kib: 256 * 1024,
            iterations: 3,
            ..KdfParams::insecure_for_tests().expect("kdf")
        };
        Envelope {
            magic: MAGIC.into(),
            format_version: FORMAT_VERSION,
            header: VaultHeader::new(kdf),
            nonce: vec![7u8; NONCE_LEN],
            ciphertext: vec![9u8; 64],
            signature: None,
        }
    }

    #[test]
    fn round_trips_through_json() {
        let e = envelope();
        let bytes = e.to_bytes().expect("serialise");
        assert!(bytes.ends_with(b"\n"));
        assert!(bytes.is_ascii(), "the file must be pure ASCII to survive Git on Windows");
        let back = Envelope::parse(&bytes).expect("parse");
        assert_eq!(e, back);
    }

    #[test]
    fn reformatting_the_json_does_not_break_the_binding() {
        let e = envelope();
        let compact = serde_json::to_vec(&e).expect("compact");
        let pretty = e.to_bytes().expect("pretty");
        let a = Envelope::parse(&compact).expect("compact parse");
        let b = Envelope::parse(&pretty).expect("pretty parse");
        assert_eq!(
            a.associated_data().expect("aad a"),
            b.associated_data().expect("aad b"),
            "whitespace must not participate in the AEAD binding"
        );
    }

    #[test]
    fn changing_any_header_field_changes_the_binding() {
        let e = envelope();
        let base = e.associated_data().expect("base");

        let mut lowered = e.clone();
        lowered.header.kdf.memory_kib = 8;
        assert_ne!(base, lowered.associated_data().expect("lowered"), "KDF downgrade");

        let mut resalted = e.clone();
        resalted.header.kdf.salt[0] ^= 1;
        assert_ne!(base, resalted.associated_data().expect("resalted"), "salt swap");

        let mut reidentified = e.clone();
        reidentified.header.vault_id = Uuid::from_u128(1234);
        assert_ne!(base, reidentified.associated_data().expect("id"), "vault identity");

        let mut retimed = e.clone();
        retimed.header.updated_at += chrono::Duration::seconds(1);
        assert_ne!(base, retimed.associated_data().expect("time"), "rollback of updated_at");

        let mut reversioned = e;
        reversioned.format_version = 1;
        assert_eq!(base, reversioned.associated_data().expect("same version"));
    }

    #[test]
    fn structural_defects_are_errors_not_panics() {
        assert!(Envelope::parse(b"").is_err());
        assert!(Envelope::parse(b"not json at all").is_err());
        assert!(Envelope::parse(b"{}").is_err());
        assert!(Envelope::parse(&[0xff, 0xfe, 0x00, 0x01]).is_err(), "invalid UTF-8");

        let mut e = envelope();
        e.magic = "NOTAVAULT".into();
        let bytes = serde_json::to_vec(&e).expect("serialise");
        assert!(matches!(Envelope::parse(&bytes), Err(Error::VaultCorrupt(_))));

        let mut e = envelope();
        e.nonce.truncate(8);
        let bytes = serde_json::to_vec(&e).expect("serialise");
        assert!(matches!(Envelope::parse(&bytes), Err(Error::VaultCorrupt(_))));

        let mut e = envelope();
        e.ciphertext.truncate(3);
        let bytes = serde_json::to_vec(&e).expect("serialise");
        assert!(matches!(Envelope::parse(&bytes), Err(Error::VaultCorrupt(_))));
    }

    #[test]
    fn a_newer_format_version_is_refused_by_version_not_by_corruption() {
        let mut e = envelope();
        e.format_version = FORMAT_VERSION + 1;
        let bytes = serde_json::to_vec(&e).expect("serialise");
        match Envelope::parse(&bytes) {
            Err(Error::VaultVersion { found, supported }) => {
                assert_eq!(found, FORMAT_VERSION + 1);
                assert_eq!(supported, FORMAT_VERSION);
            }
            other => panic!("expected a version error, got {other:?}"),
        }
    }

    #[test]
    fn oversized_input_is_rejected_before_parsing() {
        let huge = vec![b'{'; MAX_VAULT_BYTES + 1];
        assert!(matches!(Envelope::parse(&huge), Err(Error::VaultCorrupt(_))));
    }

    #[test]
    fn signing_payload_covers_nonce_and_ciphertext_unambiguously() {
        let mut a = envelope();
        a.nonce = vec![1u8; NONCE_LEN];
        a.ciphertext = vec![2u8, 2, 2, 2];
        let mut b = a.clone();
        b.ciphertext = vec![3u8, 3, 3, 3];
        assert_ne!(
            a.signing_payload().expect("a"),
            b.signing_payload().expect("b"),
            "the signature must cover the ciphertext"
        );

        // The signature field itself is excluded, since it cannot sign itself.
        let mut signed = a.clone();
        signed.signature = Some(VaultSignature {
            algorithm: SignatureAlgorithm::Ed25519,
            signer: "abc".into(),
            signature: vec![0; 64],
        });
        assert_eq!(a.signing_payload().expect("a"), signed.signing_payload().expect("signed"));
    }
}
