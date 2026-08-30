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
    let request = Request::VaultUnlock {
        passphrase: SecretString::from_string("correct horse battery staple".to_string()),
    };

    // Debug is genuinely safe — the control that is documented and tested.
    let debugged = format!("{request:?}");
    assert!(!debugged.contains("correct horse"), "Debug leaked: {debugged}");

    // Serialize is not.
    let json = serde_json::to_string(&request).expect("Request derives Serialize");
    assert!(
        !json.contains("correct horse battery staple"),
        "THREAT_MODEL.md §4 rule 1 says no secret is ever Serialized, but \
         `serde_json::to_string(&Request::VaultUnlock {{ .. }})` yields:\n{json}"
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

    // If this compiles, a variable-time comparison of secret material is the
    // path of least resistance in this codebase.
    let variable_time = a == b;
    assert!(
        !variable_time,
        "`Secret` derives PartialEq, so `secret_a == secret_b` is a \
         variable-time byte comparison and is the default thing to write. \
         THREAT_MODEL.md §4 rule 3 claims all secret comparisons are \
         constant-time; nothing in the type system enforces that."
    );
}

/// Same defect on the IPC-facing wrapper, which has no `ct_eq` at all.
#[test]
fn secret_string_has_only_a_variable_time_comparison() {
    let a = SecretString::from_string("token".to_string());
    let b = SecretString::from_string("token".to_string());
    assert!(
        a != b,
        "`SecretString` derives PartialEq and offers no constant-time \
         alternative, so any equality check on a passphrase arriving over IPC \
         is variable-time by construction"
    );
}
