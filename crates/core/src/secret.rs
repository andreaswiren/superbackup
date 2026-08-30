//! Secret material handling.
//!
//! Everything that must never be printed, logged, serialised to the plain
//! config, or sent over IPC lives inside [`Secret`]. The type deliberately
//! offers no `Display`, no `Serialize`, and a `Debug` that prints a redaction
//! marker, so the only way to observe the bytes is to ask for them explicitly
//! via [`Secret::expose`].
//!
//! The buffer is zeroed on drop. That is a best-effort mitigation, not a
//! guarantee: an OS that pages memory to disk, hibernates, or takes a crash
//! dump can still persist the plaintext. See `docs/compliance/THREAT_MODEL.md`.

use zeroize::{Zeroize, Zeroizing};

/// An owned secret byte string that zeroes itself on drop.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret {
    inner: Vec<u8>,
}

impl Secret {
    pub fn new(bytes: Vec<u8>) -> Self {
        Secret { inner: bytes }
    }

    pub fn from_string(s: String) -> Self {
        // `String` is moved in and its buffer is taken over, so no plaintext
        // copy of it survives in the caller.
        let mut s = s;
        let secret = Secret { inner: s.as_bytes().to_vec() };
        s.zeroize();
        secret
    }

    pub fn from_str(s: &str) -> Self {
        Secret { inner: s.as_bytes().to_vec() }
    }

    /// Deliberately verbose name: every call site is an audit point.
    pub fn expose(&self) -> &[u8] {
        &self.inner
    }

    /// Expose as UTF-8. Returns `None` when the secret is not valid UTF-8
    /// (which is normal for raw key material).
    pub fn expose_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.inner).ok()
    }

    /// A temporary `String` copy that zeroes itself when it goes out of scope.
    /// Use this when handing a passphrase to an API that insists on `String`.
    pub fn expose_zeroizing_string(&self) -> Option<Zeroizing<String>> {
        std::str::from_utf8(&self.inner).ok().map(|s| Zeroizing::new(s.to_string()))
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Constant-time equality. Use this rather than `==` when comparing a
    /// user-supplied value against a stored one.
    pub fn ct_eq(&self, other: &Secret) -> bool {
        if self.inner.len() != other.inner.len() {
            // Length is not secret in any of our use sites, and leaking it is
            // preferable to comparing buffers of different sizes.
            return false;
        }
        let mut diff: u8 = 0;
        for (a, b) in self.inner.iter().zip(other.inner.iter()) {
            diff |= a ^ b;
        }
        diff == 0
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        self.inner.zeroize();
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Secret([redacted; {} bytes])", self.inner.len())
    }
}

impl From<String> for Secret {
    fn from(s: String) -> Self {
        Secret::from_string(s)
    }
}

impl From<Vec<u8>> for Secret {
    fn from(v: Vec<u8>) -> Self {
        Secret::new(v)
    }
}

/// A password-strength verdict, shown live in the GUI passphrase fields.
///
/// The estimate is intentionally conservative and offline-only: it counts the
/// character classes actually used and penalises short length. It is guidance,
/// not a guarantee, and it never blocks a user who insists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Strength {
    Unusable,
    Weak,
    Fair,
    Strong,
    Excellent,
}

impl Strength {
    pub fn title(&self) -> &'static str {
        match self {
            Strength::Unusable => "Too short",
            Strength::Weak => "Weak",
            Strength::Fair => "Fair",
            Strength::Strong => "Strong",
            Strength::Excellent => "Excellent",
        }
    }
    /// Minimum we will accept for a master passphrase without an explicit
    /// "I understand the risk" confirmation.
    pub fn is_acceptable(&self) -> bool {
        *self >= Strength::Fair
    }
}

/// Passphrases so common that length and class diversity say nothing useful
/// about them. Not a complete list — the first thing an attacker tries.
const COMMON_PASSPHRASES: &[&str] = &[
    "password",
    "password1",
    "password123",
    "passw0rd",
    "12345678",
    "123456789",
    "1234567890",
    "qwertyui",
    "qwerty123",
    "iloveyou",
    "admin123",
    "letmein1",
    "welcome1",
    "abc12345",
    "changeme",
    "superbackup",
    "backup123",
];

/// Estimate passphrase strength from length, character-class diversity, and
/// how much of that length is actually distinct.
///
/// Offline and deliberately conservative. It guides the user; it never blocks
/// one who insists.
pub fn estimate_strength(passphrase: &str) -> Strength {
    let len = passphrase.chars().count();
    if len < 8 {
        return Strength::Unusable;
    }

    let normalised = passphrase.trim().to_ascii_lowercase();
    if COMMON_PASSPHRASES.contains(&normalised.as_str()) {
        return Strength::Weak;
    }

    // `aaaaaaaaaaaa` is long and useless. Distinct-character count is a cheap
    // proxy for the entropy that length alone pretends to provide.
    let distinct: std::collections::BTreeSet<char> = passphrase.chars().collect();
    if distinct.len() <= 4 {
        return Strength::Weak;
    }

    let mut classes = 0u32;
    if passphrase.chars().any(|c| c.is_ascii_lowercase()) {
        classes += 1;
    }
    if passphrase.chars().any(|c| c.is_ascii_uppercase()) {
        classes += 1;
    }
    if passphrase.chars().any(|c| c.is_ascii_digit()) {
        classes += 1;
    }
    if passphrase.chars().any(|c| !c.is_alphanumeric()) {
        classes += 1;
    }
    // A long multi-word passphrase beats a short dense one; weight length more
    // heavily than class count, which is what actually holds up. Only distinct
    // words count, so "abc abc abc abc" scores as one word.
    let words: std::collections::BTreeSet<&str> =
        passphrase.split_whitespace().filter(|w| w.chars().count() > 2).collect();
    let score = len as u32 + classes * 4 + words.len() as u32 * 6;
    match score {
        0..=17 => Strength::Weak,
        18..=29 => Strength::Fair,
        30..=45 => Strength::Strong,
        _ => Strength::Excellent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_reveals_contents() {
        let s = Secret::from_str("hunter2-super-secret");
        let rendered = format!("{s:?}");
        assert!(!rendered.contains("hunter2"), "Debug leaked the secret: {rendered}");
        assert!(rendered.contains("redacted"));
    }

    #[test]
    fn constant_time_equality_works() {
        let a = Secret::from_str("abc");
        let b = Secret::from_str("abc");
        let c = Secret::from_str("abd");
        let d = Secret::from_str("abcd");
        assert!(a.ct_eq(&b));
        assert!(!a.ct_eq(&c));
        assert!(!a.ct_eq(&d));
    }

    #[test]
    fn strength_ranking_is_sane() {
        assert_eq!(estimate_strength("short"), Strength::Unusable);
        assert!(estimate_strength("password") < Strength::Strong);
        assert!(estimate_strength("correct horse battery staple") >= Strength::Strong);
        assert!(!estimate_strength("aaaaaaaa").is_acceptable());
    }
}
