//! Password-based key derivation for the vault.
//!
//! # Why Argon2id, and why these numbers
//!
//! The vault holds every repository passphrase, every S3 key pair and every
//! API token this installation owns. Its ciphertext is *designed* to be
//! committed to a Git repository, which means the threat model is not "someone
//! stole my laptop" but "the file is public and an adversary grinds it
//! offline on rented GPUs for as long as they like". The only thing standing
//! between a mediocre human-chosen passphrase and every backup the user has is
//! the cost of one KDF evaluation.
//!
//! Argon2id is the current [RFC 9106] recommendation and the only
//! memory-hard KDF in wide production use with a side-channel-resistant first
//! pass (Argon2i) followed by a data-dependent second pass (Argon2d). PBKDF2
//! and bcrypt are not memory hard; scrypt is, but has a weaker analysis and no
//! side-channel story.
//!
//! [RFC 9106]: https://www.rfc-editor.org/rfc/rfc9106
//!
//! ## Defaults: m = 256 MiB, t = 3, p = 1
//!
//! * **m = 262144 KiB (256 MiB).** Memory is the parameter that actually hurts
//!   an attacker: an Argon2id core on an FPGA or GPU needs the full 256 MiB of
//!   fast, low-latency memory *per parallel guess*. A 24 GB GPU therefore fits
//!   fewer than a hundred concurrent guesses instead of the tens of thousands
//!   it manages against a 19 MiB configuration. RFC 9106's "first recommended"
//!   option is 2 GiB and its "second recommended" is 64 MiB; OWASP's 2024
//!   floor is 19 MiB. 256 MiB sits deliberately far above the floor while
//!   remaining trivially affordable on a 2026 desktop, where 16 GiB of RAM is
//!   entry level. It is also small enough that the allocation cannot itself be
//!   turned into a denial of service (see [`KdfParams::validate`]).
//! * **t = 3.** With p = 1, RFC 9106 requires t >= 3 for the "second
//!   recommended" profile; below that, the tradeoff-resilience analysis of
//!   Argon2id does not hold. Raising t is a linear cost for defender and
//!   attacker alike, so memory is the better knob and t stays at the
//!   analytically justified minimum.
//! * **p = 1.** Parallelism changes the derived key, so it is a compatibility
//!   parameter as much as a cost parameter. Pinning it to 1 means a vault
//!   created on a 32-core workstation opens at the same speed on a 4-core
//!   laptop, which matters because this file is explicitly meant to be shared
//!   between machines. It also maximises the memory an attacker must commit
//!   per guess for a given wall-clock budget on our side, since we spend our
//!   whole budget in one lane rather than splitting it.
//! * **32-byte output.** One 256-bit master key, immediately split by HKDF
//!   into purpose-separated subkeys (see [`super::keys`]).
//!
//! Every parameter is stored in the vault header so that a future release can
//! raise them without orphaning existing vaults; the header is bound into the
//! AEAD as associated data, so an attacker cannot *lower* them in transit
//! without destroying the ciphertext's authenticity.

use crate::error::{Error, Result};
use crate::secret::Secret;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

/// Length of the Argon2id salt, in bytes.
///
/// 128 bits is the RFC 9106 recommendation; we use 256 to leave no doubt, and
/// because the salt doubles as the HKDF salt for subkey separation.
pub const SALT_LEN: usize = 32;

/// Length of the master key produced by the KDF.
pub const MASTER_KEY_LEN: usize = 32;

/// Default memory cost, in kibibytes (256 MiB). See the module docs.
pub const DEFAULT_MEMORY_KIB: u32 = 256 * 1024;

/// Default number of passes. See the module docs.
pub const DEFAULT_ITERATIONS: u32 = 3;

/// Default lane count. See the module docs.
pub const DEFAULT_PARALLELISM: u32 = 1;

/// Hard floor enforced on *newly created* vaults (64 MiB), matching the
/// brief's minimum. Existing vaults with lower parameters still open — the
/// header is authoritative — because refusing to open them would lose data
/// without making anyone safer.
pub const MIN_NEW_MEMORY_KIB: u32 = 64 * 1024;

/// Upper bound accepted when *reading* a header, in kibibytes (2 GiB).
///
/// This is a denial-of-service control, not a cryptographic one. A hostile
/// `config.sbvault` pulled from a Git repository could otherwise claim
/// `memory_kib = 4294967295` and make us try to allocate 4 TiB before we ever
/// get to authenticate anything. Argon2 allocates before it verifies, so the
/// bound has to be applied at parse time.
pub const MAX_MEMORY_KIB: u32 = 2 * 1024 * 1024;

/// Upper bound on passes accepted when reading a header, for the same reason.
pub const MAX_ITERATIONS: u32 = 64;

/// Upper bound on lanes accepted when reading a header.
pub const MAX_PARALLELISM: u32 = 64;

/// Ceiling applied by [`KdfParams::calibrate`] (1 GiB).
///
/// Calibration measures one machine; the vault may be opened on a weaker one,
/// possibly a low-memory VM or a service account with a job-object memory cap.
/// Letting a 128 GiB workstation calibrate itself to 8 GiB would produce a
/// vault its owner's laptop cannot open at all.
pub const CALIBRATION_MAX_MEMORY_KIB: u32 = 1024 * 1024;

/// The KDF used to turn a passphrase into the master key.
///
/// Modelled as an enum rather than assumed, so that a future migration to a
/// different KDF is a new variant with an unambiguous on-disk tag instead of a
/// silent reinterpretation of the same numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum KdfAlgorithm {
    Argon2id,
}

impl KdfAlgorithm {
    /// Human-readable name for the "vault details" screen.
    pub fn title(&self) -> &'static str {
        match self {
            KdfAlgorithm::Argon2id => "Argon2id",
        }
    }
}

/// Everything needed to reproduce the master key from a passphrase.
///
/// Serialised verbatim into the vault header. Because the header is the AEAD's
/// associated data, tampering with any field here — in particular lowering
/// `memory_kib` to make an offline grind cheaper — makes the ciphertext fail
/// to authenticate instead of silently weakening the vault.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KdfParams {
    pub algorithm: KdfAlgorithm,
    /// Argon2 version number; 0x13 (19) is the only one anyone should use.
    pub version: u32,
    /// Memory cost in kibibytes.
    pub memory_kib: u32,
    /// Number of passes over memory.
    pub iterations: u32,
    /// Number of lanes.
    pub parallelism: u32,
    /// Per-vault random salt, base64 in the file.
    #[serde(with = "crate::crypto::b64")]
    pub salt: Vec<u8>,
    /// Length of the derived master key, in bytes.
    pub output_len: u32,
}

impl KdfParams {
    /// Fresh parameters for a brand-new vault, with a fresh random salt.
    pub fn recommended() -> Result<KdfParams> {
        Ok(KdfParams {
            algorithm: KdfAlgorithm::Argon2id,
            version: 0x13,
            memory_kib: DEFAULT_MEMORY_KIB,
            iterations: DEFAULT_ITERATIONS,
            parallelism: DEFAULT_PARALLELISM,
            salt: super::random_bytes(SALT_LEN)?,
            output_len: MASTER_KEY_LEN as u32,
        })
    }

    /// The same cost parameters as `self`, but with a fresh salt.
    ///
    /// Used by passphrase rotation: a new passphrase must never reuse the old
    /// salt, or the two derivations share a precomputation.
    pub fn with_fresh_salt(&self) -> Result<KdfParams> {
        Ok(KdfParams { salt: super::random_bytes(SALT_LEN)?, ..self.clone() })
    }

    /// Deliberately weak parameters, for tests only.
    ///
    /// # Warning
    ///
    /// A vault created with these is worth roughly nothing against an offline
    /// attacker. It exists so that the test suite can exercise the format,
    /// the state machine and the error paths hundreds of times per second
    /// instead of once every second and a half. Never call this from
    /// production code paths; [`KdfParams::validate_for_new_vault`] rejects
    /// the result.
    pub fn insecure_for_tests() -> Result<KdfParams> {
        Ok(KdfParams {
            algorithm: KdfAlgorithm::Argon2id,
            version: 0x13,
            memory_kib: 8,
            iterations: 1,
            parallelism: 1,
            salt: super::random_bytes(SALT_LEN)?,
            output_len: MASTER_KEY_LEN as u32,
        })
    }

    /// Reject parameters that are structurally impossible or that would let a
    /// hostile vault file exhaust this machine's memory.
    ///
    /// Called on every parse, *before* any allocation.
    pub fn validate(&self) -> Result<()> {
        if self.version != 0x13 {
            return Err(Error::VaultCorrupt(format!(
                "unsupported Argon2 version 0x{:x}",
                self.version
            )));
        }
        if self.salt.len() < 8 {
            return Err(Error::VaultCorrupt(format!(
                "Argon2 salt is {} bytes; at least 8 are required",
                self.salt.len()
            )));
        }
        if self.salt.len() > 64 {
            return Err(Error::VaultCorrupt("Argon2 salt is implausibly long".into()));
        }
        if self.output_len != MASTER_KEY_LEN as u32 {
            return Err(Error::VaultCorrupt(format!(
                "master key length {} is not supported (expected {MASTER_KEY_LEN})",
                self.output_len
            )));
        }
        if self.parallelism == 0 || self.parallelism > MAX_PARALLELISM {
            return Err(Error::VaultCorrupt(format!(
                "Argon2 parallelism {} is out of range 1..={MAX_PARALLELISM}",
                self.parallelism
            )));
        }
        if self.iterations == 0 || self.iterations > MAX_ITERATIONS {
            return Err(Error::VaultCorrupt(format!(
                "Argon2 iteration count {} is out of range 1..={MAX_ITERATIONS}",
                self.iterations
            )));
        }
        // Argon2 requires m >= 8 * p.
        let floor = 8u32.saturating_mul(self.parallelism);
        if self.memory_kib < floor || self.memory_kib > MAX_MEMORY_KIB {
            return Err(Error::VaultCorrupt(format!(
                "Argon2 memory cost {} KiB is out of range {floor}..={MAX_MEMORY_KIB}",
                self.memory_kib
            )));
        }
        Ok(())
    }

    /// Stricter check applied when creating or re-keying a vault: refuses to
    /// *write* anything below the documented floor, so a test helper or a
    /// misguided setting cannot quietly downgrade a real user's vault.
    pub fn validate_for_new_vault(&self) -> Result<()> {
        self.validate()?;
        if self.memory_kib < MIN_NEW_MEMORY_KIB || self.iterations < 3 {
            return Err(Error::Crypto(format!(
                "refusing to create a vault with Argon2id m={} KiB t={}; \
                 the minimum for new vaults is m={MIN_NEW_MEMORY_KIB} KiB t=3",
                self.memory_kib, self.iterations
            )));
        }
        Ok(())
    }

    /// Derive the 32-byte master key from a passphrase.
    ///
    /// The output lives in a [`Zeroizing`] buffer so that it is wiped even if
    /// the caller drops it on an error path.
    pub fn derive(&self, passphrase: &Secret) -> Result<Zeroizing<[u8; MASTER_KEY_LEN]>> {
        self.validate()?;
        let params = argon2::Params::new(
            self.memory_kib,
            self.iterations,
            self.parallelism,
            Some(MASTER_KEY_LEN),
        )
        .map_err(|e| Error::Crypto(format!("invalid Argon2 parameters: {e}")))?;

        let argon = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
        let mut out = Zeroizing::new([0u8; MASTER_KEY_LEN]);
        argon
            .hash_password_into(passphrase.expose(), &self.salt, out.as_mut())
            .map_err(|e| Error::Crypto(format!("key derivation failed: {e}")))?;
        Ok(out)
    }

    /// Roughly how long one derivation takes on this machine, measured now.
    ///
    /// Used by the settings screen ("unlocking takes about 0.6 s on this PC")
    /// and by [`calibrate`].
    pub fn measure(&self) -> Result<Duration> {
        let probe = Secret::from_str("superbackup-calibration-probe");
        let start = Instant::now();
        self.derive(&probe)?;
        Ok(start.elapsed())
    }

    /// Human-readable cost summary for the GUI.
    pub fn describe(&self) -> String {
        format!(
            "{} m={} MiB t={} p={}",
            self.algorithm.title(),
            self.memory_kib / 1024,
            self.iterations,
            self.parallelism
        )
    }
}

/// The target unlock time [`calibrate`] aims for.
///
/// 500 ms is the usual interactive-latency compromise: long enough that it
/// multiplies an attacker's grind by a large constant, short enough that a
/// user who unlocks several times a day does not start resenting it — and
/// crucially, short enough that they do not choose a shorter passphrase to
/// compensate.
pub const CALIBRATION_TARGET: Duration = Duration::from_millis(500);

/// Pick Argon2id parameters that take about `target` on *this* machine.
///
/// Iterations and parallelism stay at the documented defaults; only memory
/// moves, because memory is the parameter that costs an attacker asymmetrically
/// more than it costs us. The result is clamped to
/// `[MIN_NEW_MEMORY_KIB, CALIBRATION_MAX_MEMORY_KIB]` so that a very fast
/// machine cannot produce a vault that a slower one — or a memory-capped
/// service account — cannot open.
///
/// The probe runs at [`MIN_NEW_MEMORY_KIB`] rather than at the full default,
/// so that calibrating never costs more than the cheapest vault we would ever
/// write. Argon2's runtime is very close to linear in `m` for fixed `t` and
/// `p`, so one probe plus a linear extrapolation is enough: being 30 % off a
/// 500 ms target is irrelevant, and a second probe would double a cost the
/// user is already waiting on.
pub fn calibrate(target: Duration) -> Result<KdfParams> {
    let mut probe = KdfParams::recommended()?;
    probe.memory_kib = MIN_NEW_MEMORY_KIB;
    calibrate_with_probe(target, &probe)
}

/// [`calibrate`] with an explicit probe configuration.
///
/// Exposed so that tests, and a future "recalibrate" button that wants to
/// probe at the vault's *current* cost, can share exactly the code path that
/// production uses instead of an approximation of it.
pub fn calibrate_with_probe(target: Duration, probe: &KdfParams) -> Result<KdfParams> {
    let measured = probe.measure()?;
    let memory_kib = scaled_memory_kib(probe.memory_kib, measured, target);
    let tuned = KdfParams {
        memory_kib,
        iterations: DEFAULT_ITERATIONS.max(probe.iterations),
        parallelism: DEFAULT_PARALLELISM,
        salt: super::random_bytes(SALT_LEN)?,
        ..probe.clone()
    };
    tuned.validate_for_new_vault()?;
    Ok(tuned)
}

/// Linear extrapolation from one measurement to a memory cost, clamped.
///
/// Split out from [`calibrate`] because it is the only part with interesting
/// behaviour (saturation, rounding, clamping) and the only part that can be
/// tested without spending half a second on Argon2.
fn scaled_memory_kib(probe_kib: u32, measured: Duration, target: Duration) -> u32 {
    // A measurement of zero means the clock is too coarse to see the probe,
    // which can only happen with absurdly cheap parameters; `max` keeps the
    // ratio finite rather than producing an infinity.
    let measured_ms = (measured.as_secs_f64() * 1000.0).max(0.001);
    let target_ms = (target.as_secs_f64() * 1000.0).max(0.0);
    let scale = (target_ms / measured_ms).clamp(0.0, 4096.0);

    let scaled = (probe_kib as f64) * scale;
    // Round to a whole mebibyte: the extra precision is noise, and round
    // numbers are far easier for a human auditing the header to sanity-check.
    let mib = (scaled / 1024.0).round();
    let mib = if mib.is_finite() { mib.clamp(0.0, u32::MAX as f64 / 1024.0) as u32 } else { 0 };
    mib.saturating_mul(1024).clamp(MIN_NEW_MEMORY_KIB, CALIBRATION_MAX_MEMORY_KIB)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_meet_the_documented_floor() {
        let p = KdfParams::recommended().expect("params");
        assert!(p.memory_kib >= MIN_NEW_MEMORY_KIB);
        assert_eq!(p.iterations, 3);
        assert_eq!(p.parallelism, 1);
        assert_eq!(p.salt.len(), SALT_LEN);
        p.validate_for_new_vault().expect("recommended params must be acceptable");
    }

    #[test]
    fn salts_are_unique_per_vault() {
        let a = KdfParams::recommended().expect("a");
        let b = KdfParams::recommended().expect("b");
        assert_ne!(a.salt, b.salt, "salt reuse across vaults would be a serious defect");
    }

    #[test]
    fn hostile_headers_are_rejected_before_allocation() {
        let base = KdfParams::insecure_for_tests().expect("base");

        let huge = KdfParams { memory_kib: u32::MAX, ..base.clone() };
        assert!(huge.validate().is_err(), "4 TiB allocation must be refused");

        let zero_t = KdfParams { iterations: 0, ..base.clone() };
        assert!(zero_t.validate().is_err());

        let zero_p = KdfParams { parallelism: 0, ..base.clone() };
        assert!(zero_p.validate().is_err());

        let short_salt = KdfParams { salt: vec![1, 2, 3], ..base.clone() };
        assert!(short_salt.validate().is_err());

        let bad_version = KdfParams { version: 0x10, ..base.clone() };
        assert!(bad_version.validate().is_err());

        let bad_len = KdfParams { output_len: 16, ..base };
        assert!(bad_len.validate().is_err());
    }

    #[test]
    fn weak_test_params_cannot_create_a_real_vault() {
        let weak = KdfParams::insecure_for_tests().expect("weak");
        assert!(weak.validate().is_ok(), "they must still parse");
        assert!(
            weak.validate_for_new_vault().is_err(),
            "but they must never be written to a real vault"
        );
    }

    #[test]
    fn derivation_is_deterministic_and_salt_dependent() {
        let p = KdfParams::insecure_for_tests().expect("params");
        let pass = Secret::from_str("correct horse battery staple");
        let a = p.derive(&pass).expect("a");
        let b = p.derive(&pass).expect("b");
        assert_eq!(a.as_ref(), b.as_ref(), "same salt + passphrase must give the same key");

        let p2 = p.with_fresh_salt().expect("fresh salt");
        let c = p2.derive(&pass).expect("c");
        assert_ne!(a.as_ref(), c.as_ref(), "a new salt must give a new key");

        let other = p.derive(&Secret::from_str("wrong")).expect("other");
        assert_ne!(a.as_ref(), other.as_ref());
    }

    #[test]
    fn scaling_saturates_instead_of_overflowing() {
        // A machine so fast the probe is instant must not produce a vault
        // nobody can open.
        assert_eq!(
            scaled_memory_kib(64 * 1024, Duration::from_nanos(1), Duration::from_secs(3600)),
            CALIBRATION_MAX_MEMORY_KIB
        );
        // A machine so slow the probe already overshoots must not drop below
        // the floor.
        assert_eq!(
            scaled_memory_kib(64 * 1024, Duration::from_secs(60), Duration::from_millis(1)),
            MIN_NEW_MEMORY_KIB
        );
        // A zero target is nonsense, but it must clamp rather than divide by
        // zero or produce a NaN cast.
        assert_eq!(
            scaled_memory_kib(64 * 1024, Duration::from_millis(50), Duration::ZERO),
            MIN_NEW_MEMORY_KIB
        );
        // The ordinary case: 64 MiB took 100 ms, we want 500 ms -> 320 MiB.
        assert_eq!(
            scaled_memory_kib(64 * 1024, Duration::from_millis(100), Duration::from_millis(500)),
            320 * 1024
        );
        // Every result lands on a whole mebibyte.
        for ms in [7u64, 13, 29, 101, 997] {
            let got =
                scaled_memory_kib(64 * 1024, Duration::from_millis(ms), CALIBRATION_TARGET);
            assert_eq!(got % 1024, 0, "{got} KiB is not a whole MiB");
        }
    }

    #[test]
    fn calibration_produces_usable_parameters() {
        // Probe with the cheap test parameters so the test costs nothing; the
        // code path is exactly the production one.
        let probe = KdfParams::insecure_for_tests().expect("probe");
        let tuned = calibrate_with_probe(CALIBRATION_TARGET, &probe).expect("calibrate");
        tuned
            .validate_for_new_vault()
            .expect("calibration must never emit parameters we refuse to write");
        assert!(tuned.memory_kib >= MIN_NEW_MEMORY_KIB);
        assert!(tuned.memory_kib <= CALIBRATION_MAX_MEMORY_KIB);
        assert_ne!(tuned.salt, probe.salt, "calibration must mint a fresh salt");
    }
}
