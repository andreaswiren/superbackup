//! The vault: an authenticated, passphrase-protected container for every
//! secret this installation owns, plus the configuration document that refers
//! to them.
//!
//! # Why the configuration lives inside the vault
//!
//! [`crate::model::RemoteConfigSource`] syncs exactly one file,
//! `config.sbvault`, and never the plain `config.json`. For that to be useful,
//! the shared file has to carry the jobs, destinations and providers as well as
//! the secrets they reference — otherwise a second machine would receive a bag
//! of keys with nothing to unlock. Putting the configuration in the encrypted
//! body also means bucket names, endpoints, source paths and machine labels —
//! all of which are perfectly good reconnaissance for an attacker who finds
//! the repository — are not published in clear text.
//!
//! The local `config.json` remains the working copy; the vault's embedded
//! configuration is the *published* copy, updated when the user chooses to
//! publish. See [`crate::config::Store`].
//!
//! # Locked and unlocked are different types, not a boolean
//!
//! A locked [`Vault`] holds no key material and physically cannot answer
//! [`Vault::get`]. The secret-bearing accessors live on [`OpenVault`], which
//! can only be obtained through [`Vault::opened`] — a `Result`, so a caller
//! who forgets to unlock gets a compile-time nudge and an [`Error::Locked`] at
//! worst, never a silent empty answer that looks like "there is no such
//! secret".

use super::envelope::{AeadAlgorithm, Envelope, VaultHeader, VaultSignature, FORMAT_VERSION, MAGIC};
use super::kdf::KdfParams;
use super::keys::MasterKeys;
use super::rekey::{Rekey, RekeyAcknowledgement};
use super::signing;
use crate::error::{Error, Result};
use crate::model::{Config, SecretRef};
use crate::secret::Secret;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;
use zeroize::{Zeroize, Zeroizing};

/// Version of the *plaintext body* schema, independent of the envelope format.
///
/// Split from [`FORMAT_VERSION`] on purpose: adding a field inside the
/// encrypted body does not change how the file is decrypted, so old readers
/// should be able to refuse it precisely rather than reporting the whole file
/// as unreadable.
pub const BODY_VERSION: u32 = 1;

/// One secret, with the bookkeeping the GUI shows next to it.
#[derive(Clone)]
pub struct VaultEntry {
    secret: Secret,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    /// Free-text label, e.g. "StorJ eu-1 access key". Never secret.
    label: Option<String>,
}

impl VaultEntry {
    pub fn secret(&self) -> &Secret {
        &self.secret
    }
    pub fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }
    pub fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }
    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }
}

impl std::fmt::Debug for VaultEntry {
    /// Renders metadata only. A `{:?}` on a collection of these — which is
    /// exactly what a `tracing::debug!` of the vault would do — must not print
    /// key material.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaultEntry")
            .field("bytes", &self.secret.len())
            .field("updated_at", &self.updated_at)
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------
// Plaintext body, as serialised inside the ciphertext
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct WireBody {
    version: u32,
    #[serde(default)]
    entries: BTreeMap<String, WireEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    config: Option<Config>,
}

#[derive(Serialize, Deserialize)]
struct WireEntry {
    /// Base64 of the raw secret bytes. Zeroed the moment it is converted.
    value: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    label: Option<String>,
}

// ---------------------------------------------------------------------------
// The vault
// ---------------------------------------------------------------------------

/// A sealed vault, optionally unlocked.
///
/// The struct always holds a valid sealed envelope, so `seal`/`sealed_bytes`
/// can be answered whether or not the vault is currently open. Unlocking adds
/// key material and the decrypted body; [`Vault::lock`] drops both.
pub struct Vault {
    sealed: Envelope,
    sealed_bytes: Vec<u8>,
    open: Option<OpenState>,
}

struct OpenState {
    keys: MasterKeys,
    entries: BTreeMap<SecretRef, VaultEntry>,
    config: Option<Config>,
    /// Set by every mutation; cleared by [`Vault::seal`]. When it is false,
    /// `seal` returns the cached bytes unchanged so that a no-op save does not
    /// churn a Git repository with a fresh nonce and a fresh timestamp.
    dirty: bool,
}

impl std::fmt::Debug for Vault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Vault")
            .field("vault_id", &self.sealed.header.vault_id)
            .field("kdf", &self.sealed.header.kdf.describe())
            .field("locked", &self.is_locked())
            .field("entries", &self.open.as_ref().map(|o| o.entries.len()))
            .finish_non_exhaustive()
    }
}

impl Vault {
    /// Create a brand-new, empty vault with the recommended KDF parameters and
    /// leave it unlocked.
    pub fn create(passphrase: &Secret) -> Result<Vault> {
        Vault::create_with(passphrase, KdfParams::recommended()?)
    }

    /// Create a new vault with explicit KDF parameters, for the settings
    /// screen's "calibrate for this machine" flow.
    ///
    /// Refuses parameters below the documented floor: a vault is created once
    /// and lives for years, so this is the wrong place to be lenient.
    pub fn create_with(passphrase: &Secret, kdf: KdfParams) -> Result<Vault> {
        kdf.validate_for_new_vault()?;
        Vault::create_unchecked(passphrase, kdf)
    }

    /// [`Vault::create_with`] without the "is this strong enough" check.
    ///
    /// Exists so the test suite can build hundreds of vaults per second with
    /// [`KdfParams::insecure_for_tests`]. Do not call it from application code;
    /// there is no situation in which a user is better off with a weaker vault.
    pub fn create_unchecked(passphrase: &Secret, kdf: KdfParams) -> Result<Vault> {
        if passphrase.is_empty() {
            return Err(Error::Validation("the master passphrase cannot be empty".into()));
        }
        let header = VaultHeader::new(kdf);
        let master = header.kdf.derive(passphrase)?;
        let keys = MasterKeys::derive(master.as_ref(), &header.kdf.salt)?;
        let open = OpenState {
            keys,
            entries: BTreeMap::new(),
            config: None,
            dirty: true,
        };
        let (sealed, sealed_bytes) = seal_body(&header, &open)?;
        Ok(Vault { sealed, sealed_bytes, open: Some(OpenState { dirty: false, ..open }) })
    }

    /// Parse a vault file without opening it.
    ///
    /// Lets the GUI show "created 2026-01-04, Argon2id m=256 MiB" on the
    /// unlock screen, and lets [`crate::remote`] compare vault identities
    /// before asking the user for a passphrase.
    pub fn open_locked(bytes: &[u8]) -> Result<Vault> {
        let sealed = Envelope::parse(bytes)?;
        let sealed_bytes = sealed.to_bytes()?;
        Ok(Vault { sealed, sealed_bytes, open: None })
    }

    /// Parse and unlock in one step.
    pub fn unlock(bytes: &[u8], passphrase: &Secret) -> Result<Vault> {
        let mut vault = Vault::open_locked(bytes)?;
        vault.unlock_in_place(passphrase)?;
        Ok(vault)
    }

    /// Unlock an already-parsed vault.
    ///
    /// # Errors
    ///
    /// [`Error::BadPassphrase`] when the AEAD does not authenticate. This is
    /// returned both for a wrong passphrase and for a corrupted or tampered
    /// ciphertext, because those two cases are *genuinely indistinguishable*
    /// to the decryptor: Poly1305 says "no" and declines to say why. Reporting
    /// them differently would require guessing, and a wrong guess is an oracle.
    /// The full KDF runs before the AEAD is even attempted, so both paths cost
    /// the same wall-clock time; nothing observable distinguishes them.
    ///
    /// The vault is left untouched on failure — a failed unlock never
    /// half-opens anything.
    pub fn unlock_in_place(&mut self, passphrase: &Secret) -> Result<()> {
        let (keys, entries, config) = decrypt(&self.sealed, passphrase)?;
        self.open = Some(OpenState { keys, entries, config, dirty: false });
        Ok(())
    }

    /// Drop all key material and plaintext. Cheap and idempotent.
    ///
    /// Unsaved changes are discarded: `lock` is what the auto-lock timer and
    /// the "Lock now" menu item call, and quietly writing to disk from a
    /// timer would be a worse surprise than losing an unsaved edit. Callers
    /// that care should [`Vault::seal`] first; [`Vault::is_dirty`] tells them
    /// whether they need to.
    pub fn lock(&mut self) {
        self.open = None;
    }

    pub fn is_locked(&self) -> bool {
        self.open.is_none()
    }

    /// True when there are in-memory changes that [`Vault::seal`] has not yet
    /// folded into the sealed bytes.
    pub fn is_dirty(&self) -> bool {
        self.open.as_ref().is_some_and(|o| o.dirty)
    }

    /// The header, readable without the passphrase.
    pub fn header(&self) -> &VaultHeader {
        &self.sealed.header
    }

    /// Stable identity of this vault, independent of its contents.
    pub fn id(&self) -> Uuid {
        self.sealed.header.vault_id
    }

    /// The detached signature carried by the sealed file, if any.
    pub fn signature(&self) -> Option<&VaultSignature> {
        self.sealed.signature.as_ref()
    }

    /// The last sealed bytes: exactly what is (or should be) on disk.
    pub fn sealed_bytes(&self) -> &[u8] {
        &self.sealed_bytes
    }

    /// The sealed envelope, for signature verification and remote comparison.
    pub fn envelope(&self) -> &Envelope {
        &self.sealed
    }

    // -- gated accessors ----------------------------------------------------

    /// Borrow the unlocked contents, or fail with [`Error::Locked`].
    ///
    /// This is the only door to secret material. Everything below is a
    /// convenience wrapper around it.
    pub fn opened(&self) -> Result<OpenVault<'_>> {
        match &self.open {
            Some(state) => Ok(OpenVault { state }),
            None => Err(Error::Locked),
        }
    }

    /// Fetch one secret. `Ok(None)` means "unlocked, but no such entry".
    pub fn get(&self, key: &SecretRef) -> Result<Option<Secret>> {
        Ok(self.opened()?.get(key))
    }

    /// Insert or replace a secret. Returns the previous value, if any.
    pub fn put(&mut self, key: SecretRef, value: Secret) -> Result<Option<Secret>> {
        self.put_labelled(key, value, None)
    }

    /// [`Vault::put`] with a human-readable label for the "stored secrets"
    /// screen. The label is *not* secret and is stored in clear text inside
    /// the encrypted body, so it must never be derived from the value.
    ///
    /// Passing `None` keeps whatever label the entry already had, so rotating
    /// a key through [`Vault::put`] does not quietly erase its description.
    pub fn put_labelled(
        &mut self,
        key: SecretRef,
        value: Secret,
        label: Option<String>,
    ) -> Result<Option<Secret>> {
        // Locked wins over every other complaint: it is the actionable one.
        let state = self.open.as_mut().ok_or(Error::Locked)?;
        if value.is_empty() {
            // An empty secret is almost always a caller that forgot to read a
            // field, and storing it would produce a "the passphrase is wrong"
            // failure much later, somewhere far away.
            return Err(Error::Validation(format!("refusing to store an empty secret for {key}")));
        }
        let now = Utc::now();
        let previous = state.entries.remove(&key);
        let created_at = previous.as_ref().map(|p| p.created_at).unwrap_or(now);
        let label = label.or_else(|| previous.as_ref().and_then(|p| p.label.clone()));
        state.entries.insert(
            key,
            VaultEntry { secret: value, created_at, updated_at: now, label },
        );
        state.dirty = true;
        Ok(previous.map(|p| p.secret))
    }

    /// Delete a secret, returning it if it was there.
    pub fn remove(&mut self, key: &SecretRef) -> Result<Option<Secret>> {
        let state = self.open.as_mut().ok_or(Error::Locked)?;
        let previous = state.entries.remove(key);
        // Only mark dirty when something actually changed, so that a
        // speculative delete does not force a rewrite of the file.
        state.dirty |= previous.is_some();
        Ok(previous.map(|p| p.secret))
    }

    /// Every handle currently stored, in sorted order.
    pub fn list_refs(&self) -> Result<Vec<SecretRef>> {
        Ok(self.opened()?.list_refs())
    }

    /// The configuration document published with this vault, if any.
    ///
    /// Borrowed directly from the open state rather than through
    /// [`Vault::opened`], because the guard is a temporary and a caller almost
    /// always wants to read several fields off the configuration.
    pub fn embedded_config(&self) -> Result<Option<&Config>> {
        Ok(self.open.as_ref().ok_or(Error::Locked)?.config.as_ref())
    }

    /// Replace the published configuration document.
    ///
    /// Called when the user explicitly publishes; not on every local save.
    pub fn set_embedded_config(&mut self, config: Option<Config>) -> Result<()> {
        let state = self.open.as_mut().ok_or(Error::Locked)?;
        state.config = config;
        state.dirty = true;
        Ok(())
    }

    /// The deterministic repository passphrase for a destination.
    pub fn derive_repo_passphrase(&self, destination_id: &Uuid) -> Result<Secret> {
        self.opened()?.derive_repo_passphrase(destination_id)
    }

    /// This vault's signer fingerprint, for pinning in `trusted_signers`.
    pub fn signer_fingerprint(&self) -> Result<String> {
        self.opened()?.signer_fingerprint()
    }

    // -- sealing ------------------------------------------------------------

    /// Encrypt the current contents and return the bytes to write.
    ///
    /// A fresh 192-bit nonce is drawn for every seal that has something to
    /// seal. When nothing changed since the last seal the cached bytes are
    /// returned verbatim, which keeps a synced Git repository quiet.
    pub fn seal(&mut self) -> Result<Vec<u8>> {
        let state = self.open.as_ref().ok_or(Error::Locked)?;
        if !state.dirty {
            return Ok(self.sealed_bytes.clone());
        }
        let header = VaultHeader { updated_at: Utc::now(), ..self.sealed.header.clone() };
        let (sealed, bytes) = seal_body(&header, state)?;
        self.sealed = sealed;
        self.sealed_bytes = bytes.clone();
        if let Some(state) = self.open.as_mut() {
            state.dirty = false;
        }
        Ok(bytes)
    }

    /// Attach a detached signature to the sealed bytes, for publication.
    ///
    /// # Errors
    ///
    /// [`Error::Crypto`] in this build; see [`super::signing`]. The vault is
    /// left completely unmodified when signing fails, so a caller that ignores
    /// the error still publishes a valid unsigned vault rather than a
    /// half-signed one.
    pub fn seal_signed(&mut self) -> Result<Vec<u8>> {
        self.seal()?;
        let payload = self.sealed.signing_payload()?;
        let (public_key, signature) = {
            let open = self.opened()?;
            let seed = open.state.keys.signing_seed();
            (signing::public_key(seed)?, signing::sign(seed, &payload)?)
        };
        let mut candidate = self.sealed.clone();
        candidate.signature = Some(VaultSignature {
            algorithm: super::envelope::SignatureAlgorithm::Ed25519,
            signer: signing::fingerprint(&public_key),
            public_key: public_key.to_vec(),
            signature,
        });
        let signed_bytes = candidate.to_bytes()?;
        self.sealed = candidate;
        self.sealed_bytes.clone_from(&signed_bytes);
        Ok(signed_bytes)
    }

    // -- rotation -----------------------------------------------------------

    /// Re-key the vault under a new passphrase, keeping every secret.
    ///
    /// Atomic in the strict sense: the old passphrase is verified, the new key
    /// is derived, and the new ciphertext is produced *in full* before a single
    /// field of `self` is touched. Any failure — wrong old passphrase, an
    /// empty new one, a CSPRNG that will not produce a salt — leaves the vault
    /// exactly as it was, still openable with the old passphrase.
    ///
    /// A fresh salt is minted, so the two passphrases share no precomputation,
    /// and `vault_id` and `created_at` are preserved, so a remote that has seen
    /// this vault before recognises the rotated file as the same vault rather
    /// than a stranger's.
    ///
    /// Returns the new sealed bytes. Writing them, and taking a backup first,
    /// is [`super::VaultFile::change_passphrase`]'s job.
    /// # The acknowledgement is not a formality
    ///
    /// Every destination using
    /// [`PassphraseSource::DerivedFromMaster`](crate::model::PassphraseSource::DerivedFromMaster)
    /// computes its repository password from the master key, so this call
    /// changes all of them at once — and Kopia does not find out. `ack` forces
    /// the caller to state, in the type system, which repositories it is going
    /// to migrate. See [`super::rekey`] for the whole mechanism and for why
    /// the vault is written before the repositories are moved.
    ///
    /// This method also cross-checks `ack` against the vault's *embedded*
    /// configuration when it has one, so a caller that asserts
    /// [`RekeyAcknowledgement::NoDerivedRepositories`] over a vault that
    /// plainly does have them is refused rather than believed.
    pub fn change_passphrase(
        &mut self,
        old: &Secret,
        new: &Secret,
        ack: &RekeyAcknowledgement,
    ) -> Result<Rekey> {
        self.rekey(old, new, None, ack)
    }

    /// Re-key with different KDF parameters as well as a different passphrase
    /// — the "my old vault used 64 MiB, raise it" path.
    ///
    /// The *old* parameters are used to verify `old`, and the new ones only to
    /// derive the new key. Doing it the other way round would make the
    /// verification fail every time, since the existing ciphertext was
    /// produced under the old cost.
    pub fn change_passphrase_and_params(
        &mut self,
        old: &Secret,
        new: &Secret,
        kdf: KdfParams,
        ack: &RekeyAcknowledgement,
    ) -> Result<Rekey> {
        kdf.validate_for_new_vault()?;
        self.rekey(old, new, Some(kdf), ack)
    }

    fn rekey(
        &mut self,
        old: &Secret,
        new: &Secret,
        new_kdf: Option<KdfParams>,
        ack: &RekeyAcknowledgement,
    ) -> Result<Rekey> {
        if new.is_empty() {
            return Err(Error::Validation("the new master passphrase cannot be empty".into()));
        }
        // Verify `old` against the sealed bytes rather than against any
        // in-memory state: that is the thing the user will still have to open
        // tomorrow if this goes wrong, and it is the only authority on what
        // the current passphrase actually is.
        let (old_keys, sealed_entries, sealed_config) = decrypt(&self.sealed, old)?;

        // Prefer live in-memory contents when the vault is unlocked, so an
        // unsaved edit is not silently dropped by a rotation.
        let (entries, config) = match &self.open {
            Some(state) => (state.entries.clone(), state.config.clone()),
            None => (sealed_entries, sealed_config),
        };

        // The acknowledgement is the caller's claim; the embedded
        // configuration is evidence. Check the claim against the evidence
        // where there is any, so an out-of-date or optimistic caller cannot
        // rotate away a repository password nobody is going to migrate.
        if let Some(config) = &config {
            check_acknowledgement(config, ack)?;
        }

        let kdf = new_kdf.unwrap_or_else(|| self.sealed.header.kdf.clone()).with_fresh_salt()?;
        let header = VaultHeader {
            kdf,
            aead: self.sealed.header.aead,
            created_at: self.sealed.header.created_at,
            updated_at: Utc::now(),
            vault_id: self.sealed.header.vault_id,
        };
        let master = header.kdf.derive(new)?;
        let keys = MasterKeys::derive(master.as_ref(), &header.kdf.salt)?;
        let next = OpenState { keys, entries, config, dirty: true };
        let (sealed, bytes) = seal_body(&header, &next)?;

        // Build the migration plan before touching `self`, so that a failure
        // to derive either key hierarchy still leaves the old vault intact.
        let new_keys = MasterKeys::derive(master.as_ref(), &header.kdf.salt)?;
        let plan = Rekey::new(
            header.vault_id,
            old_keys,
            new_keys,
            bytes.clone(),
            ack.repositories(),
        )?;

        // Everything succeeded; commit. Note that the signature is dropped:
        // it was computed over the old ciphertext and is now meaningless — and
        // in any case the rotation changed the signing identity.
        let was_locked = self.is_locked();
        self.sealed = sealed;
        self.sealed_bytes = bytes;
        self.open = if was_locked { None } else { Some(OpenState { dirty: false, ..next }) };
        Ok(plan)
    }

    /// Consume an unlocked vault and take its key hierarchy.
    ///
    /// Used by [`Rekey::resume`] to rebuild both halves of an interrupted
    /// rotation from the two vault files on disk.
    pub(crate) fn into_keys(self) -> Result<MasterKeys> {
        self.open.map(|state| state.keys).ok_or(Error::Locked)
    }
}

/// A borrowed view of an unlocked vault.
///
/// Obtaining one is the only way to read secret material, which is what makes
/// "read a secret from a locked vault" unrepresentable rather than merely
/// discouraged.
#[derive(Debug)]
pub struct OpenVault<'a> {
    state: &'a OpenState,
}

impl std::fmt::Debug for OpenState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenState")
            .field("entries", &self.entries.len())
            .field("has_config", &self.config.is_some())
            .field("dirty", &self.dirty)
            .finish_non_exhaustive()
    }
}

impl OpenVault<'_> {
    /// Fetch one secret by handle.
    pub fn get(&self, key: &SecretRef) -> Option<Secret> {
        self.state.entries.get(key).map(|e| e.secret.clone())
    }

    /// Fetch one entry with its metadata.
    pub fn entry(&self, key: &SecretRef) -> Option<&VaultEntry> {
        self.state.entries.get(key)
    }

    pub fn contains(&self, key: &SecretRef) -> bool {
        self.state.entries.contains_key(key)
    }

    pub fn len(&self) -> usize {
        self.state.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.state.entries.is_empty()
    }

    /// Every stored handle, sorted. Handles are not secret — they are
    /// `kind:uuid` strings that already appear in `config.json`.
    pub fn list_refs(&self) -> Vec<SecretRef> {
        self.state.entries.keys().cloned().collect()
    }

    /// Handles with their metadata, for the "stored secrets" screen.
    pub fn entries(&self) -> impl Iterator<Item = (&SecretRef, &VaultEntry)> {
        self.state.entries.iter()
    }

    pub fn embedded_config(&self) -> Option<&Config> {
        self.state.config.as_ref()
    }

    pub fn derive_repo_passphrase(&self, destination_id: &Uuid) -> Result<Secret> {
        self.state.keys.repo_passphrase(destination_id)
    }

    pub fn signer_fingerprint(&self) -> Result<String> {
        signing::seed_fingerprint(self.state.keys.signing_seed())
    }
}

// ---------------------------------------------------------------------------
// Seal / unseal
// ---------------------------------------------------------------------------

fn seal_body(header: &VaultHeader, state: &OpenState) -> Result<(Envelope, Vec<u8>)> {
    let mut wire = WireBody {
        version: BODY_VERSION,
        entries: BTreeMap::new(),
        config: state.config.clone(),
    };
    for (key, entry) in &state.entries {
        wire.entries.insert(
            key.as_str().to_string(),
            WireEntry {
                value: super::b64_encode(entry.secret.expose()),
                created_at: entry.created_at,
                updated_at: entry.updated_at,
                label: entry.label.clone(),
            },
        );
    }

    let plaintext = Zeroizing::new(
        serde_json::to_vec(&wire)
            .map_err(|e| Error::Crypto(format!("vault body could not be serialised: {e}")))?,
    );
    // The base64 copies inside `wire` are plaintext key material; wipe them
    // before the struct is dropped by the allocator's own timetable.
    for entry in wire.entries.values_mut() {
        entry.value.zeroize();
    }

    let nonce_bytes = super::random_bytes(super::envelope::NONCE_LEN)?;
    let mut envelope = Envelope {
        magic: MAGIC.to_string(),
        format_version: FORMAT_VERSION,
        header: header.clone(),
        nonce: nonce_bytes,
        // Filled in below; a placeholder long enough to pass the structural
        // check that `associated_data` does not depend on.
        ciphertext: Vec::new(),
        signature: None,
    };

    let aad = envelope.associated_data()?;
    let cipher = cipher_for(&state.keys, header)?;
    let nonce = XNonce::from_slice(&envelope.nonce);
    let ciphertext = cipher
        .encrypt(nonce, Payload { msg: plaintext.as_ref(), aad: &aad })
        .map_err(|_| Error::Crypto("vault encryption failed".into()))?;
    // Wipe the plaintext as soon as it is no longer needed rather than waiting
    // for the end of the scope.
    drop(plaintext);
    envelope.ciphertext = ciphertext;

    let bytes = envelope.to_bytes()?;
    Ok((envelope, bytes))
}

type DecryptedBody = (MasterKeys, BTreeMap<SecretRef, VaultEntry>, Option<Config>);

fn decrypt(envelope: &Envelope, passphrase: &Secret) -> Result<DecryptedBody> {
    // The KDF runs first and unconditionally, so a wrong passphrase and a
    // corrupted ciphertext take the same time to reject.
    let master = envelope.header.kdf.derive(passphrase)?;
    let keys = MasterKeys::derive(master.as_ref(), &envelope.header.kdf.salt)?;

    let aad = envelope.associated_data()?;
    let cipher = cipher_for(&keys, &envelope.header)?;
    let nonce = XNonce::from_slice(&envelope.nonce);
    let plaintext = Zeroizing::new(
        cipher
            .decrypt(nonce, Payload { msg: &envelope.ciphertext, aad: &aad })
            // Poly1305 does not distinguish "wrong key" from "modified
            // ciphertext" from "modified header", and neither do we; see
            // `Vault::unlock_in_place`.
            .map_err(|_| Error::BadPassphrase)?,
    );

    // Past this point the bytes are authenticated: they were produced by
    // something holding the vault key. A failure here is genuine corruption or
    // a version mismatch, and saying so is safe.
    let mut wire: WireBody = serde_json::from_slice(plaintext.as_ref())
        .map_err(|e| Error::VaultCorrupt(format!("vault body is not readable: {e}")))?;
    if wire.version > BODY_VERSION {
        return Err(Error::VaultVersion { found: wire.version, supported: BODY_VERSION });
    }

    let mut entries = BTreeMap::new();
    for (key, entry) in wire.entries.iter_mut() {
        let raw = super::b64_decode(&entry.value)
            .map_err(|_| Error::VaultCorrupt(format!("secret {key} is not valid base64")))?;
        entry.value.zeroize();
        entries.insert(
            SecretRef(key.clone()),
            VaultEntry {
                secret: Secret::new(raw),
                created_at: entry.created_at,
                updated_at: entry.updated_at,
                label: entry.label.clone(),
            },
        );
    }
    Ok((keys, entries, wire.config))
}

/// Check a [`RekeyAcknowledgement`] against a configuration that actually
/// exists.
///
/// The acknowledgement is the caller's promise; this is the audit. Both
/// failures matter, and the second is the subtle one:
///
/// * claiming there is nothing to migrate when there is;
/// * listing *some* of the derived repositories but not all of them, which
///   would rotate the rest into unopenability while the caller believes it has
///   handled everything.
fn check_acknowledgement(config: &Config, ack: &RekeyAcknowledgement) -> Result<()> {
    let derived = super::rekey::derived_repositories(config);
    if derived.is_empty() {
        return Ok(());
    }
    let listed: std::collections::BTreeSet<Uuid> =
        ack.repositories().iter().map(|r| r.destination_id).collect();
    let missing: Vec<&str> = derived
        .iter()
        .filter(|r| !listed.contains(&r.destination_id))
        .map(|r| r.destination_name.as_str())
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    Err(Error::Validation(format!(
        "refusing to rotate the master passphrase: {} repositor{} derive their password from \
         it and were not included in the migration plan ({}). Rotating now would leave them \
         permanently unopenable; see `superbackup_core::crypto::rekey`.",
        missing.len(),
        if missing.len() == 1 { "y" } else { "ies" },
        missing.join(", ")
    )))
}

fn cipher_for(keys: &MasterKeys, header: &VaultHeader) -> Result<XChaCha20Poly1305> {
    match header.aead {
        AeadAlgorithm::XChaCha20Poly1305 => {
            Ok(XChaCha20Poly1305::new(Key::from_slice(keys.vault_key())))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kdf() -> KdfParams {
        KdfParams::insecure_for_tests().expect("kdf")
    }

    fn vault() -> Vault {
        Vault::create_unchecked(&Secret::from_str("test-passphrase"), kdf()).expect("create")
    }

    #[test]
    fn a_new_vault_is_unlocked_and_empty() {
        let v = vault();
        assert!(!v.is_locked());
        assert!(v.list_refs().expect("refs").is_empty());
        assert!(!v.sealed_bytes().is_empty());
    }

    #[test]
    fn locked_vault_refuses_every_secret_accessor() {
        let mut v = vault();
        v.put(SecretRef("a:1".into()), Secret::from_str("x")).expect("put");
        v.lock();
        assert!(v.is_locked());
        assert!(matches!(v.get(&SecretRef("a:1".into())), Err(Error::Locked)));
        assert!(matches!(v.put(SecretRef("b:1".into()), Secret::from_str("y")), Err(Error::Locked)));
        assert!(matches!(v.remove(&SecretRef("a:1".into())), Err(Error::Locked)));
        assert!(matches!(v.list_refs(), Err(Error::Locked)));
        assert!(matches!(v.seal(), Err(Error::Locked)));
        assert!(matches!(v.opened(), Err(Error::Locked)));
        assert!(matches!(v.derive_repo_passphrase(&Uuid::nil()), Err(Error::Locked)));
        // Metadata is still readable, which is the point of keeping it
        // outside the ciphertext.
        assert!(!v.header().kdf.salt.is_empty());
    }

    #[test]
    fn put_preserves_created_at_and_updates_updated_at() {
        let mut v = vault();
        let r = SecretRef("s3.access:1".into());
        v.put(r.clone(), Secret::from_str("first")).expect("put");
        let created = v.opened().expect("open").entry(&r).expect("entry").created_at();
        let previous = v.put(r.clone(), Secret::from_str("second")).expect("replace");
        assert_eq!(previous.expect("previous").expose(), b"first");
        let entry = v.opened().expect("open");
        let entry = entry.entry(&r).expect("entry");
        assert_eq!(entry.created_at(), created, "creation time must survive a rotation");
        assert!(entry.updated_at() >= created);
    }

    #[test]
    fn empty_secrets_are_refused() {
        let mut v = vault();
        assert!(matches!(
            v.put(SecretRef("a:1".into()), Secret::new(Vec::new())),
            Err(Error::Validation(_))
        ));
    }

    #[test]
    fn sealing_an_unchanged_vault_is_byte_stable() {
        let mut v = vault();
        v.put(SecretRef("a:1".into()), Secret::from_str("x")).expect("put");
        let first = v.seal().expect("seal");
        let second = v.seal().expect("reseal");
        assert_eq!(first, second, "a no-op save must not churn the file");

        v.put(SecretRef("b:1".into()), Secret::from_str("y")).expect("put");
        let third = v.seal().expect("seal");
        assert_ne!(first, third);
    }

    #[test]
    fn a_failed_remove_does_not_dirty_the_vault() {
        let mut v = vault();
        v.seal().expect("seal");
        assert!(!v.is_dirty());
        assert!(v.remove(&SecretRef("nothing:0".into())).expect("remove").is_none());
        assert!(!v.is_dirty(), "removing a missing key must not force a rewrite");
    }

    #[test]
    fn locking_discards_unsaved_changes() {
        let mut v = vault();
        let bytes = v.seal().expect("seal");
        v.put(SecretRef("a:1".into()), Secret::from_str("x")).expect("put");
        assert!(v.is_dirty());
        v.lock();
        assert_eq!(v.sealed_bytes(), bytes.as_slice());
    }

    #[test]
    fn debug_output_never_contains_secret_material() {
        let mut v = vault();
        v.put_labelled(
            SecretRef("s3.secret:1".into()),
            Secret::from_str("SUPERSECRETVALUE"),
            Some("label".into()),
        )
        .expect("put");
        let rendered = format!("{v:?}");
        assert!(!rendered.contains("SUPERSECRET"), "{rendered}");

        let open = v.opened().expect("open");
        let entry = format!("{:?}", open.entry(&SecretRef("s3.secret:1".into())).expect("entry"));
        assert!(!entry.contains("SUPERSECRET"), "{entry}");
        assert!(entry.contains("label"), "non-secret metadata should survive: {entry}");
    }
}
