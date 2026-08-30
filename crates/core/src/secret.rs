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
///
/// Deliberately **not** `PartialEq`/`Eq`. Deriving them makes `a == b` the
/// path of least resistance, and that is `Vec<u8>`'s early-exit comparison —
/// variable-time, and therefore a timing oracle on the very values this type
/// exists to protect. [`Secret::ct_eq`] is the only way to compare two
/// secrets, so a caller cannot reach for the unsafe one by habit.
#[derive(Clone)]
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
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
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
    "qwerty",
    "qwertyuiop",
    "monkey123",
    "dragon123",
    "football",
    "baseball",
    "sunshine",
    "princess",
    "trustno1",
    "starwars",
    "whatever",
    "computer",
    "internet",
];

/// Stems that, with a year or a digit appended, make up an enormous share of
/// real-world passwords. Matched as a prefix of the de-mangled stem.
const SEASONAL_STEMS: &[&str] = &[
    "summer",
    "winter",
    "spring",
    "autumn",
    "january",
    "february",
    "march",
    "april",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
    "welcome",
    "password",
    "letmein",
    "changeme",
    "company",
    "admin",
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

    // `Password12` is `password` with the decoration every guessing tool tries
    // first. Checking only the literal string rated it `Fair` and therefore
    // acceptable — for a vault whose entire security reduces to this value.
    // Strip the common mangling and re-check the stem.
    let stem: String = normalised
        .trim_end_matches(|c: char| c.is_ascii_digit() || c.is_ascii_punctuation())
        .chars()
        .map(|c| match c {
            '0' => 'o',
            '1' => 'l',
            '3' => 'e',
            '4' => 'a',
            '5' => 's',
            '@' => 'a',
            '$' => 's',
            '!' => 'i',
            other => other,
        })
        .collect();
    if COMMON_PASSPHRASES.contains(&stem.as_str()) {
        return Strength::Weak;
    }
    // A season or a month plus a year is the other universal pattern.
    for prefix in SEASONAL_STEMS {
        if stem.starts_with(prefix) {
            return Strength::Weak;
        }
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
    //
    // A *single* word earns no word bonus at all, however many character
    // classes it decorates itself with. The bonus is meant to reward the
    // entropy of choosing several independent words; awarding it to one word
    // is what let `Password12` clear the acceptable threshold.
    let words: std::collections::BTreeSet<&str> =
        passphrase.split_whitespace().filter(|w| w.chars().count() > 2).collect();
    let word_bonus = if words.len() >= 2 { words.len() as u32 * 6 } else { 0 };

    // A single token, however decorated, has to be long before it is worth
    // anything: character-class diversity across eight characters is a mask
    // attack, not entropy. This vault has no recovery path, so the bar for
    // "acceptable" is a real passphrase rather than a compliant-looking
    // password.
    if words.len() < 2 && len < 12 {
        return Strength::Weak;
    }

    let score = len as u32 + classes * 4 + word_bonus;
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

    #[test]
    fn common_passwords_survive_the_usual_decoration() {
        // Every one of these is what a guessing tool tries in its first few
        // thousand candidates. Rating any of them acceptable would be worse
        // than showing no meter at all, because it actively reassures.
        for candidate in [
            "Password12",
            "Qwerty1234",
            "Welcome2024",
            "Summer2025!",
            "P@ssw0rd",
            "Letmein123",
            "Admin2025",
            "Changeme1",
        ] {
            assert!(
                !estimate_strength(candidate).is_acceptable(),
                "`{candidate}` was rated {:?}, which is acceptable",
                estimate_strength(candidate)
            );
        }
    }

    #[test]
    fn a_single_decorated_word_is_not_acceptable() {
        // One word plus character-class decoration is not a passphrase, and
        // the word bonus must not rescue it.
        assert!(!estimate_strength("Zx9!qWer").is_acceptable());
    }

    #[test]
    fn genuine_passphrases_still_pass() {
        for candidate in [
            "correct horse battery staple",
            "trombone reactor plum ledger",
            "my kingdom for a properly sized horse",
        ] {
            assert!(
                estimate_strength(candidate).is_acceptable(),
                "`{candidate}` was rejected at {:?}",
                estimate_strength(candidate)
            );
        }
    }
}
