//! Hostile review of `redact::scrub`.
//!
//! Claim under test (THREAT_MODEL.md §A6, redact.rs module docs):
//! "`redact::scrub` runs over everything before it can reach a log, an event,
//! an IPC response, or a notification. ... It is deliberately over-eager."
//!
//! Every test below is a credential that survives `scrub` in clear text, or an
//! input that makes `scrub` pathologically slow.

use std::time::Instant;
use superbackup_core::redact::scrub;

/// `Authorization` is in `redact_assignments::KEY_HINTS` but *not* in
/// `needs_scrubbing`, so `scrub` returns the line `Cow::Borrowed` and the
/// assignment redactor is never reached.
///
/// This is the single most common way a credential appears in third-party
/// HTTP output — and `remote::RemoteClient::fetch` sends exactly this header
/// with the user's GitHub PAT in it.
#[test]
fn authorization_bearer_headers_are_not_redacted_at_all() {
    let leaked = "ghp_AbCdEf0123456789DeadBeefCafe";
    let line = format!("request failed\nAuthorization: Bearer {leaked}\n");
    let out = scrub(&line);
    assert!(!out.contains(leaked), "the bearer token survived scrub() verbatim:\n{out}");
}

/// Same root cause: `credential` is a KEY_HINT but not a `needs_scrubbing`
/// trigger.
#[test]
fn a_credential_assignment_is_not_redacted_at_all() {
    let out = scrub("aws: credential=AKIAIOSFODNN7EXAMPLE");
    assert!(!out.contains("AKIAIOSFODNN7EXAMPLE"), "credential= survived: {out}");
}

/// `redact_assignments` terminates a value at the first whitespace, so a
/// quoted passphrase containing spaces — which is exactly what a good
/// passphrase looks like — leaks everything after the first word.
#[test]
fn quoted_values_containing_spaces_leak_all_but_the_first_word() {
    let out = scrub(r#"kopia: KOPIA_PASSWORD="correct horse battery staple""#);
    assert!(
        !out.contains("horse battery staple"),
        "all but the first word of the passphrase survived: {out}"
    );
}

/// The same defect in JSON with conventional spacing after the colon and a
/// value containing a space.
#[test]
fn json_values_with_spaces_leak() {
    let out = scrub(r#"{"secret": "hunter two three", "region": "eu-1"}"#);
    assert!(!out.contains("two three"), "part of the secret survived: {out}");
}

/// `scrub` is line-oriented, so a credential printed on the line after its
/// label — the shape of most YAML, TOML and `kopia --help`-style output — is
/// never even looked at.
#[test]
fn a_value_on_the_line_after_its_key_is_not_redacted() {
    let out = scrub("password:\n  hunter2-the-actual-secret\n");
    assert!(!out.contains("hunter2-the-actual-secret"), "multi-line value survived: {out}");
}

/// Windows batch/PowerShell echo style. `SET` output is `SET NAME=value`, and
/// the key is found correctly — but a value containing a space still leaks its
/// tail, and this is how `cmd /c set` prints an environment.
#[test]
fn windows_set_syntax_with_a_spaced_value_leaks() {
    let out = scrub("SET KOPIA_PASSWORD=my secret pass phrase");
    assert!(!out.contains("secret pass phrase"), "tail of the value survived: {out}");
}

/// `redact_assignments` rescans `rest` from the beginning on every iteration
/// and, for every `:` or `=` it meets, does an O(n) `rfind` plus an O(n)
/// `to_ascii_lowercase` allocation. A line of colons is therefore quadratic in
/// both time and allocation.
///
/// `Limits::max_line_bytes` is 1 MiB and IPC error text is scrubbed on the way
/// out, so a single request is enough to burn a core inside the daemon.
#[test]
fn adversarial_input_is_not_quadratic() {
    // Enough to make the quadratic term obvious, far below the 1 MiB line cap.
    let hostile = format!("://{}", ":".repeat(40_000));

    let start = Instant::now();
    let _ = scrub(&hostile);
    let elapsed = start.elapsed();

    assert!(
        elapsed.as_millis() < 500,
        "scrub() took {elapsed:?} on {} bytes of colons; it is quadratic and a \
         1 MiB line (the IPC cap) would take minutes",
        hostile.len()
    );
}
