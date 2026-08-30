//! Hostile review of the "secrets never leak" claims.
//!
//! Claims under test (THREAT_MODEL.md §4 "Design rules the code is expected to
//! hold to", secret.rs module docs):
//!
//! 1. "No secret is ever `Serialize`d, `Display`ed, or `Debug`-printed."
//! 3. "Comparisons of secret values are constant-time."

use superbackup_core::ipc::protocol::{Request, SecretString};
use superbackup_core::secret::Secret;

/// Rule 1 says no secret is ever `Serialize`d. `SecretString` implements
/// `Serialize` and emits the plaintext, and `Request` — which carries the
/// master passphrase in `vault.unlock` and `vault.change_passphrase` — derives
/// `Serialize`.
///
/// So `serde_json::to_string(&request)` produces the master passphrase in
/// clear text. That is one `tracing::debug!(%json)`, one `--json` echo of an
/// outgoing request, or one bug-report dump away from disk. The redacting
/// `Debug` does not help, because `Serialize` is the trait people reach for
/// when they want a request in a log file.
#[test]
fn a_request_carrying_the_master_passphrase_serialises_it_in_clear_text() {
    // REGRESSION GUARD (was: M3 — resolved as a documentation defect).
    //
    // This behaviour is real and intentional: a client cannot send an unlock
    // request without serialising the passphrase. What was wrong was
    // THREAT_MODEL.md §4 rule 1, which asserted without qualification that no
    // secret is ever serialised. Rule 1 now states the exception and bounds it.
    //
    // The test therefore pins the *boundary* rather than the absence:
    // serialisation reveals the passphrase (client-side, by necessity), while
    // Debug does not (any side, always).
    let request = Request::VaultUnlock {
        passphrase: SecretString::from_string("correct horse battery staple".to_string()),
    };

    let json = serde_json::to_string(&request).expect("a client must be able to send this");
    assert!(
        json.contains("correct horse battery staple"),
        "if this stops being true the wire format changed; update rule 1 with it"
    );

    let debugged = format!("{request:?}");
    assert!(
        !debugged.contains("correct horse battery staple"),
        "Debug must never reveal a passphrase, on any side of the connection: {debugged}"
    );
}

/// Rule 3 says comparisons of secret values are constant-time. `Secret` derives
/// `PartialEq`/`Eq`, so `==` is `Vec<u8>`'s early-exit `memcmp` — and `==` is
/// what any author reaches for by default. `ct_eq` exists but is opt-in, and
/// nothing prevents or flags the variable-time path.
///
/// `SecretString` derives `PartialEq`/`Eq` too, with no constant-time
/// alternative at all.
#[test]
fn secrets_expose_a_variable_time_equality_as_the_default_operator() {
    let a = Secret::from_str("a-repository-passphrase");
    let b = Secret::from_str("a-repository-passphrase");

    // REGRESSION GUARD (was: M4, fixed). The derives are gone, so the
    // variable-time path no longer exists to be reached for. The original
    // demonstration was `let _ = a == b;`, which no longer compiles — that
    // compile error IS the fix, so it is recorded rather than deleted:
    //
    //     let _ = a == b;   // must not compile
    //
    assert!(a.ct_eq(&b), "identical secrets must compare equal");
    assert!(
        !a.ct_eq(&Secret::from_str("a-different-passphrase")),
        "`Secret` derives PartialEq, so `secret_a == secret_b` is a \
         variable-time byte comparison and is the default thing to write. \
         THREAT_MODEL.md §4 rule 3 claims all secret comparisons are \
         constant-time; nothing in the type system enforces that."
    );
}

/// THREAT_MODEL.md §A1: "the UI meters strength, refuses to treat a common
/// passphrase as acceptable".
///
/// REGRESSION GUARD (was: M5, fixed).
///
/// `estimate_strength` used to compare against a 17-entry list by exact match,
/// so every trivial variant of those same passphrases scored `Fair` — which
/// `is_acceptable()` accepts — because a single ten-character word with three
/// character classes already cleared the bar.
///
/// It now de-mangles leetspeak and trailing digits before checking the list,
/// screens seasonal and month stems, and refuses to award the multi-word bonus
/// to a single token or to call any single token under twelve characters
/// acceptable.
#[test]
fn the_common_passphrase_check_is_defeated_by_appending_a_digit() {
    use superbackup_core::secret::estimate_strength;

    // On the list.
    assert!(!estimate_strength("password1").is_acceptable());

    for variant in ["Password12", "Qwerty1234", "Welcome2024", "Summer2025!"] {
        assert!(
            !estimate_strength(variant).is_acceptable(),
            "{variant:?} is rated {:?}, which `is_acceptable()` accepts; the \
             common-passphrase check is an exact match against 17 strings and \
             stops any attacker who does not add a digit",
            estimate_strength(variant)
        );
    }
}

/// Same defect on the IPC-facing wrapper, which has no `ct_eq` at all.
#[test]
fn secret_string_has_only_a_variable_time_comparison() {
    let a = SecretString::from_string("token".to_string());
    let b = SecretString::from_string("token".to_string());

    // REGRESSION GUARD (was: M4 on the IPC wrapper, fixed). `SecretString`
    // carries a master passphrase in from the socket and now mirrors `Secret`:
    // no derives, and an explicit constant-time comparison.
    //
    //     let _ = a == b;   // must not compile
    //
    assert!(a.ct_eq(&b));
    assert!(
        !a.ct_eq(&SecretString::from_string("other".to_string())),
        "`SecretString` derives PartialEq and offers no constant-time \
         alternative, so any equality check on a passphrase arriving over IPC \
         is variable-time by construction"
    );
}
