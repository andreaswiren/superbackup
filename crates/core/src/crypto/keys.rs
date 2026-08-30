//! The key hierarchy: one master key in, several purpose-separated keys out.
//!
//! ```text
//!   master passphrase
//!         │  Argon2id (kdf.rs, parameters + salt from the vault header)
//!         ▼
//!   master key (32 bytes)                     ── never used directly
//!         │  HKDF-SHA256, salt = the Argon2id salt
//!         ├── info "superbackup/v1/vault-encryption"  ─► XChaCha20-Poly1305 key
//!         ├── info "superbackup/v1/repo-passphrase"   ─► repo-passphrase root
//!         │        └── HKDF-Expand info "…/<destination-uuid>" ─► one repo passphrase
//!         └── info "superbackup/v1/signing"           ─► Ed25519 seed
//! ```
//!
//! # Why the split
//!
//! Using one key for two purposes is how protocols die. If the AEAD key and
//! the signing seed were the same 32 bytes, an attacker who recovered the
//! signing key from a faulty signature implementation would also hold the
//! vault key; if the repo-passphrase root were the AEAD key, then a repository
//! passphrase handed to Kopia — which writes it into its own config file, its
//! own logs, and a child process's environment — would be the vault key
//! itself. HKDF-Expand with distinct `info` strings makes each output
//! computationally independent of the others: learning one tells you nothing
//! about the master key or its siblings.
//!
//! The `info` strings are versioned (`v1`). Changing what a key is used for is
//! a new string, never a redefinition of an old one.

use crate::error::{Error, Result};
use crate::secret::Secret;
use hkdf::Hkdf;
use sha2::Sha256;
use uuid::Uuid;
use zeroize::{Zeroize, Zeroizing};

/// Length of every subkey in the hierarchy.
pub const SUBKEY_LEN: usize = 32;

/// HKDF `info` for the key that encrypts the vault body.
const INFO_VAULT: &[u8] = b"superbackup/v1/vault-encryption";
/// HKDF `info` for the root from which deterministic repo passphrases grow.
const INFO_REPO: &[u8] = b"superbackup/v1/repo-passphrase";
/// HKDF `info` for the Ed25519 signing seed.
const INFO_SIGNING: &[u8] = b"superbackup/v1/signing";

/// The set of keys unlocked by one successful passphrase entry.
///
/// Deliberately not `Clone`: every additional copy of these bytes is another
/// page that might be swapped out. Pass it by reference.
pub struct MasterKeys {
    /// XChaCha20-Poly1305 key for the vault body.
    vault: Zeroizing<[u8; SUBKEY_LEN]>,
    /// Root for per-destination repository passphrases.
    repo_root: Zeroizing<[u8; SUBKEY_LEN]>,
    /// Ed25519 seed used to sign sealed vaults for remote publication.
    signing: Zeroizing<[u8; SUBKEY_LEN]>,
}

impl MasterKeys {
    /// Expand a master key into the hierarchy.
    ///
    /// `salt` is the Argon2id salt from the vault header. Reusing it here is
    /// safe and deliberate: HKDF-Extract wants a salt that is unique per
    /// context but need not be secret, and the Argon2id salt is exactly that.
    /// It also means two vaults sharing a passphrase — which users do — still
    /// produce completely unrelated subkeys.
    pub fn derive(master: &[u8], salt: &[u8]) -> Result<MasterKeys> {
        if master.len() != SUBKEY_LEN {
            return Err(Error::Crypto("master key has the wrong length".into()));
        }
        let hk = Hkdf::<Sha256>::new(Some(salt), master);
        Ok(MasterKeys {
            vault: expand(&hk, INFO_VAULT)?,
            repo_root: expand(&hk, INFO_REPO)?,
            signing: expand(&hk, INFO_SIGNING)?,
        })
    }

    /// The AEAD key for the vault body.
    pub fn vault_key(&self) -> &[u8; SUBKEY_LEN] {
        &self.vault
    }

    /// The Ed25519 seed. See [`crate::crypto::signing`] for what this build
    /// can and cannot do with it.
    pub fn signing_seed(&self) -> &[u8; SUBKEY_LEN] {
        &self.signing
    }

    /// The deterministic repository passphrase for one destination.
    ///
    /// Same vault plus same destination id gives the same passphrase on every
    /// machine, for ever. That is the whole point: a second PC that pulls the
    /// shared vault can connect to the same Kopia repository without anybody
    /// transcribing a key. The destination UUID is bound into the `info`
    /// string, so two destinations never collide and moving a destination to a
    /// new id deliberately produces a new passphrase.
    ///
    /// The returned string's format is documented on
    /// [`encode_passphrase`].
    pub fn repo_passphrase(&self, destination_id: &Uuid) -> Result<Secret> {
        // A second HKDF stage rather than one big expand: the repo root can be
        // handed to a subsystem that needs to mint many passphrases without
        // that subsystem ever holding the master key.
        let hk = Hkdf::<Sha256>::from_prk(self.repo_root.as_ref())
            .map_err(|_| Error::Crypto("repo-passphrase root is not a valid PRK".into()))?;
        let mut info = Vec::with_capacity(64);
        info.extend_from_slice(INFO_REPO);
        info.push(b'/');
        info.extend_from_slice(destination_id.as_bytes());
        let mut out = Zeroizing::new([0u8; SUBKEY_LEN]);
        hk.expand(&info, out.as_mut())
            .map_err(|_| Error::Crypto("repo-passphrase expansion failed".into()))?;
        Ok(encode_passphrase(out.as_ref()))
    }
}

impl std::fmt::Debug for MasterKeys {
    /// Never renders key material, so that a `#[derive(Debug)]` anywhere up
    /// the stack cannot leak the vault key into a log line.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MasterKeys([redacted])")
    }
}

fn expand(hk: &Hkdf<Sha256>, info: &[u8]) -> Result<Zeroizing<[u8; SUBKEY_LEN]>> {
    let mut out = Zeroizing::new([0u8; SUBKEY_LEN]);
    hk.expand(info, out.as_mut()).map_err(|_| Error::Crypto("HKDF expansion failed".into()))?;
    Ok(out)
}

// ---------------------------------------------------------------------------
// Human-transcribable passphrase encoding
// ---------------------------------------------------------------------------

/// Alphabet for generated and derived passphrases: Crockford base32.
///
/// `I`, `L`, `O` and `U` are absent. The first three are absent because a
/// human copying a passphrase off a screen onto paper — which is exactly what
/// the "write this down" screen asks them to do — confuses them with `1` and
/// `0`; `U` is absent because dropping it keeps accidental profanity out of a
/// string the user has to read aloud to a colleague on the phone.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Prefix stamped on every passphrase this module produces.
pub const PASSPHRASE_PREFIX: &str = "SB1";

/// Number of base32 characters emitted for a 32-byte input.
const ENCODED_CHARS: usize = 52;
/// Characters per dash-separated group.
const GROUP: usize = 4;

/// Render 32 bytes of key material as a passphrase a human can transcribe.
///
/// # Format
///
/// ```text
/// SB1-XXXX-XXXX-XXXX-XXXX-XXXX-XXXX-XXXX-XXXX-XXXX-XXXX-XXXX-XXXX-XXXX
/// └┬┘ └───────────────────────── 13 groups of 4 ─────────────────────┘
///  └── version tag
/// ```
///
/// The 256 input bits are packed most-significant-bit first into 52 symbols of
/// the [`ALPHABET`] (52 x 5 = 260 bits; the final symbol carries four zero
/// padding bits in its low positions), then grouped in fours and joined with
/// `-`. The result is 68 characters, carries the full 256 bits of the input,
/// and contains only `A-Z`, `0-9` and `-`, so it survives every shell, every
/// `.env` file, every clipboard and every Kopia command line unquoted.
///
/// The `SB1` tag exists so that a support engineer looking at a string in a
/// screenshot can tell what it is and, more importantly, so that a future
/// change of alphabet or length is distinguishable rather than silently
/// producing a different passphrase for the same key.
pub fn encode_passphrase(key: &[u8; SUBKEY_LEN]) -> Secret {
    let mut symbols = Vec::with_capacity(ENCODED_CHARS);
    let mut acc: u16 = 0;
    let mut bits: u32 = 0;
    for &byte in key.iter() {
        acc = (acc << 8) | byte as u16;
        bits += 8;
        while bits >= 5 {
            let idx = ((acc >> (bits - 5)) & 0x1f) as usize;
            symbols.push(ALPHABET[idx]);
            bits -= 5;
        }
    }
    if bits > 0 {
        let idx = ((acc << (5 - bits)) & 0x1f) as usize;
        symbols.push(ALPHABET[idx]);
    }

    let mut out = String::with_capacity(PASSPHRASE_PREFIX.len() + ENCODED_CHARS + 16);
    out.push_str(PASSPHRASE_PREFIX);
    for (i, sym) in symbols.iter().enumerate() {
        if i % GROUP == 0 {
            out.push('-');
        }
        out.push(*sym as char);
    }
    symbols.zeroize();
    Secret::from_string(out)
}

/// A fresh 256-bit passphrase from the operating system CSPRNG.
///
/// This is what [`crate::model::PassphraseSource::Generated`] means and what
/// the "write this down" screen displays. 256 bits is far past any plausible
/// brute-force horizon; the length is dictated by wanting the passphrase to be
/// unguessable even though Kopia will stretch it only lightly, and by the fact
/// that nobody has to remember it — they only have to copy it once.
pub fn generate_passphrase() -> Result<Secret> {
    let mut key = Zeroizing::new([0u8; SUBKEY_LEN]);
    super::fill_random(key.as_mut())?;
    Ok(encode_passphrase(key.as_ref()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(master: [u8; 32], salt: &[u8]) -> MasterKeys {
        MasterKeys::derive(&master, salt).expect("derive")
    }

    #[test]
    fn subkeys_are_distinct() {
        let k = keys([7u8; 32], b"salty-salty-salty-salt");
        assert_ne!(k.vault_key(), k.signing_seed());
        assert_ne!(k.vault_key().as_slice(), k.repo_root.as_ref().as_slice());
        assert_ne!(k.signing_seed().as_slice(), k.repo_root.as_ref().as_slice());
        assert_ne!(k.vault_key(), &[7u8; 32], "the master key must not be used directly");
    }

    #[test]
    fn subkeys_depend_on_salt_and_master() {
        let a = keys([1u8; 32], b"salt-aaaaaaaaaaaa");
        let b = keys([1u8; 32], b"salt-bbbbbbbbbbbb");
        let c = keys([2u8; 32], b"salt-aaaaaaaaaaaa");
        assert_ne!(a.vault_key(), b.vault_key());
        assert_ne!(a.vault_key(), c.vault_key());
    }

    #[test]
    fn repo_passphrase_is_deterministic_and_per_destination() {
        let k = keys([9u8; 32], b"a-fixed-salt-value-here");
        let d1 = Uuid::from_u128(1);
        let d2 = Uuid::from_u128(2);

        let a = k.repo_passphrase(&d1).expect("a");
        let b = k.repo_passphrase(&d1).expect("b");
        assert!(a.ct_eq(&b), "the same destination must derive the same passphrase");

        let c = k.repo_passphrase(&d2).expect("c");
        assert!(!a.ct_eq(&c), "different destinations must not collide");

        // Same destination, different vault -> different passphrase.
        let other = keys([9u8; 32], b"a-different-salt-value!");
        let d = other.repo_passphrase(&d1).expect("d");
        assert!(!a.ct_eq(&d));
    }

    #[test]
    fn encoded_passphrase_has_the_documented_shape() {
        let p = encode_passphrase(&[0xa5; 32]);
        let s = p.expose_str().expect("utf8");
        assert!(s.starts_with("SB1-"), "{s}");
        let body: String = s.trim_start_matches("SB1-").split('-').collect();
        assert_eq!(body.len(), ENCODED_CHARS, "{s}");
        assert_eq!(s.len(), 3 + 13 + ENCODED_CHARS, "{s}");
        assert!(
            s.chars().all(|c| c == '-' || ALPHABET.contains(&(c as u8))),
            "ambiguous or unsafe character in {s}"
        );
        for bad in ['I', 'L', 'O', 'U'] {
            assert!(!s.contains(bad), "{bad} is ambiguous when transcribed: {s}");
        }
    }

    #[test]
    fn encoding_is_injective_on_the_first_and_last_bits() {
        // Flipping the most significant bit and the least significant bit of
        // the input must both change the output; a truncating or misaligned
        // packer would silently drop one of them.
        let base = [0u8; 32];
        let mut hi = base;
        hi[0] = 0x80;
        let mut lo = base;
        lo[31] = 0x01;
        let a = encode_passphrase(&base);
        let b = encode_passphrase(&hi);
        let c = encode_passphrase(&lo);
        assert!(!a.ct_eq(&b));
        assert!(!a.ct_eq(&c));
        assert!(!b.ct_eq(&c));
    }

    #[test]
    fn generated_passphrases_are_unique() {
        let a = generate_passphrase().expect("a");
        let b = generate_passphrase().expect("b");
        assert!(!a.ct_eq(&b), "the CSPRNG returned the same 256 bits twice");
        assert_eq!(a.len(), b.len());
    }

    #[test]
    fn master_keys_debug_is_redacted() {
        let k = keys([0xde; 32], b"salt-salt-salt-salt");
        let rendered = format!("{k:?}");
        assert_eq!(rendered, "MasterKeys([redacted])");
        assert!(!rendered.contains("de"));
    }
}
