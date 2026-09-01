//! Last-line-of-defence redaction for text that leaves the process.
//!
//! Everything the user can see — log lines, event messages, IPC errors,
//! desktop notifications, `--json` output — passes through [`scrub`] first.
//!
//! This is a safety net, not the primary control. The primary control is that
//! secrets live in [`crate::secret::Secret`] and are handed to child processes
//! through the environment and stdin rather than argv. `scrub` exists because
//! third-party output (kopia's stderr, a git transport error, an S3 SDK
//! message) is not under our control and has been known to echo credentials
//! back in error text.
//!
//! ## Performance is a security property here
//!
//! `scrub` runs on the daemon's own connection task, on attacker-influenced
//! input, before an error can be reported. It is therefore **strictly linear**
//! in the length of the input: one pass to mask URL userinfo, one pass to mask
//! credential-shaped assignments, no rescanning and no per-character
//! allocation.
//!
//! An earlier implementation restarted its scan after every match and called
//! `rfind` plus `to_ascii_lowercase` for every `:` or `=` it saw. That is
//! quadratic, and a 20 KB line of colons occupied a connection task for nine
//! seconds — which at the 1 MiB line cap extrapolates to hours of pinned CPU
//! from a single unauthenticated frame. Keep this linear.
//!
//! ## Over-eagerness is deliberate
//!
//! An unquoted value is masked to the end of its line (or to the next
//! structural `,`/`}`/`)`), not to the next space, because `SET PASSWORD=my
//! long passphrase` is one value and masking only `my` leaks it. A redacted
//! diagnostic is a nuisance; a leaked repository key is unrecoverable.

use std::borrow::Cow;

const MASK: &str = "[redacted]";

/// Environment variable names whose values must never be printed.
pub const SENSITIVE_ENV_VARS: &[&str] = &[
    "KOPIA_PASSWORD",
    "KOPIA_NEW_PASSWORD",
    "KOPIA_SERVER_PASSWORD",
    "KOPIA_SERVER_CONTROL_PASSWORD",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "SUPERBACKUP_PASSPHRASE",
    "GITHUB_TOKEN",
    "GIT_ASKPASS_PASSWORD",
];

/// Substrings that mark a key as naming a credential.
///
/// This is the single source of truth: [`needs_scrubbing`] is derived from it
/// rather than maintained alongside it. A parallel hand-written trigger list is
/// how `Authorization: Bearer <token>` and `credential=…` previously passed
/// through completely untouched — they were in the key list but not the
/// trigger list, so the cheap pre-check returned early and the masking pass
/// never ran. A test below asserts the two can never diverge again.
const KEY_HINTS: &[&str] = &[
    "password",
    "passwd",
    "passphrase",
    "secret",
    "token",
    "access_key",
    "access-key",
    "accesskey",
    "secret_key",
    "secret-key",
    "secretkey",
    "apikey",
    "api_key",
    "api-key",
    "authorization",
    "credential",
    "credentials",
    "auth",
];

/// Remove anything that looks like a credential from free-form text.
pub fn scrub(input: &str) -> Cow<'_, str> {
    if !needs_scrubbing(input) {
        return Cow::Borrowed(input);
    }
    let mut out = String::with_capacity(input.len() + MASK.len());
    // A key whose value is on the following line (`password:\n  hunter2`)
    // carries across the line boundary.
    let mut value_pending = false;
    for line in input.split_inclusive('\n') {
        let masked = mask_urls(line);
        mask_assignments(&masked, &mut out, &mut value_pending);
    }
    Cow::Owned(out)
}

/// Cheap pre-check so clean text allocates nothing.
///
/// Linear, single pass, no allocation: lowercase comparison is done on the fly.
fn needs_scrubbing(input: &str) -> bool {
    if input.contains("://") {
        return true;
    }
    let bytes = input.as_bytes();
    for hint in KEY_HINTS {
        if contains_ignore_ascii_case(bytes, hint.as_bytes()) {
            return true;
        }
    }
    false
}

fn contains_ignore_ascii_case(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || haystack.len() < needle.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w.eq_ignore_ascii_case(needle))
}

/// `scheme://user:password@host` -> `scheme://user:[redacted]@host`.
///
/// The single most common accidental leak: a remote-config URL with an
/// embedded personal access token, echoed verbatim in a git error.
fn mask_urls(line: &str) -> Cow<'_, str> {
    let Some(_) = line.find("://") else {
        return Cow::Borrowed(line);
    };
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(pos) = rest.find("://") {
        let after = pos + 3;
        out.push_str(&rest[..after]);
        let authority_end = rest[after..]
            .find(|c: char| c == '/' || c.is_whitespace())
            .map(|i| after + i)
            .unwrap_or(rest.len());
        let authority = &rest[after..authority_end];
        match authority.rsplit_once('@') {
            Some((userinfo, host)) => {
                match userinfo.split_once(':') {
                    // `user:password@host` — the username is not the secret.
                    Some((user, _)) => {
                        out.push_str(user);
                        out.push(':');
                        out.push_str(MASK);
                    }
                    // `token@host` — a bare PAT. Mask the whole userinfo; it
                    // is not a username even though it sits in that position.
                    None => out.push_str(MASK),
                }
                out.push('@');
                out.push_str(host);
            }
            None => out.push_str(authority),
        }
        rest = &rest[authority_end..];
    }
    out.push_str(rest);
    Cow::Owned(out)
}

/// Mask `KEY=value`, `key: value`, `--flag=value` and `"key":"value"` where the
/// key names a credential. One forward pass, tracking the current token start
/// incrementally instead of searching backwards.
fn mask_assignments(line: &str, out: &mut String, value_pending: &mut bool) {
    // The previous line was `password:` with nothing after it, so this line's
    // content is the value.
    if *value_pending {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        out.push_str(&line[..indent]);
        let content = trimmed.trim_end_matches(['\r', '\n']);
        if content.is_empty() {
            // Still waiting; a blank line does not end the value.
            out.push_str(trimmed);
            return;
        }
        out.push_str(MASK);
        out.push_str(&trimmed[content.len()..]);
        *value_pending = false;
        return;
    }

    let bytes = line.as_bytes();
    // Start of the token that would be a key if we met a separator now.
    let mut token_start = 0usize;
    // How much of `line` has already been copied to `out`.
    let mut emitted = 0usize;
    let mut i = 0usize;

    while i < bytes.len() {
        let b = bytes[i];

        if is_token_boundary(b) {
            token_start = i + 1;
            i += 1;
            continue;
        }

        if b != b'=' && b != b':' {
            i += 1;
            continue;
        }

        // `://` is a URL, already handled by mask_urls.
        if bytes[i..].starts_with(b"://") {
            i += 3;
            token_start = i;
            continue;
        }

        let key = trim_key(&line[token_start..i]);
        if !key_names_a_credential(key) {
            i += 1;
            token_start = i;
            continue;
        }

        // Copy everything up to and including the separator.
        out.push_str(&line[emitted..=i]);
        let mut v = i + 1;
        while v < bytes.len() && (bytes[v] == b' ' || bytes[v] == b'\t') {
            v += 1;
        }
        out.push_str(&line[i + 1..v]);

        let (value_end, found_value) = value_extent(bytes, v);
        if !found_value {
            // `password:` with the value on the next line.
            *value_pending = true;
            out.push_str(&line[v..]);
            return;
        }

        out.push_str(MASK);
        emitted = value_end;
        i = value_end;
        token_start = i;
    }

    out.push_str(&line[emitted..]);
}

fn is_token_boundary(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r' | b'\n' | b',' | b'{' | b'&' | b'(' | b';')
}

/// Strip the decoration around a key: quotes, leading dashes, JSON spacing.
fn trim_key(raw: &str) -> &str {
    raw.trim_matches(|c: char| c == '"' || c == '\'' || c == '-' || c.is_whitespace())
}

fn key_names_a_credential(key: &str) -> bool {
    if key.is_empty() {
        return false;
    }
    // Substring rather than suffix matching, because real credential variables
    // carry suffixes: `AWS_ACCESS_KEY_ID` ends in `_ID`, not in `access_key`.
    // This does mean an innocent key containing "token" is masked too. That is
    // the intended direction of error.
    let key_bytes = key.as_bytes();
    KEY_HINTS.iter().any(|hint| contains_ignore_ascii_case(key_bytes, hint.as_bytes()))
}

/// Find where a value ends, starting at `start`.
///
/// Returns `(end_index, found_a_value)`. A quoted value ends at its closing
/// quote; an unquoted one runs to a structural delimiter or to the end of the
/// line — **not** to the next space, because a shell assignment's value may
/// contain spaces and stopping early leaks all but the first word.
fn value_extent(bytes: &[u8], start: usize) -> (usize, bool) {
    if start >= bytes.len() {
        return (start, false);
    }
    let first = bytes[start];
    if first == b'\r' || first == b'\n' {
        return (start, false);
    }
    if first == b'"' || first == b'\'' {
        let quote = first;
        let mut j = start + 1;
        while j < bytes.len() {
            // A backslash escapes the next byte, so `"a\"b"` ends at the last
            // quote rather than the escaped one.
            if bytes[j] == b'\\' {
                j += 2;
                continue;
            }
            if bytes[j] == quote {
                return (j + 1, true);
            }
            j += 1;
        }
        return (bytes.len(), true);
    }
    // Unquoted. Stopping at the first space would leak all but the first word
    // of `SET PASSWORD=my long passphrase`; running blindly to end of line
    // would swallow the rest of a diagnostic such as
    // `PASSWORD=x rejected by https://host/bucket`, destroying the one part an
    // operator needs.
    //
    // So: consume whitespace-separated words, but stop before a word that
    // begins a new field — one containing `://` or `=`. The look-ahead scans
    // each word once and then the loop jumps past it, so every byte is still
    // visited a bounded number of times and the pass stays linear.
    let mut j = start;
    while j < bytes.len() {
        match bytes[j] {
            b',' | b'}' | b')' | b'\r' | b'\n' => return (j, true),
            b' ' | b'\t' => {
                let mut w = j;
                while w < bytes.len() && (bytes[w] == b' ' || bytes[w] == b'\t') {
                    w += 1;
                }
                let word_start = w;
                while w < bytes.len() && !bytes[w].is_ascii_whitespace() {
                    w += 1;
                }
                if word_start >= bytes.len() {
                    return (j, true);
                }
                if starts_a_new_field(&bytes[word_start..w]) {
                    return (j, true);
                }
                j = w;
            }
            _ => j += 1,
        }
    }
    (bytes.len(), true)
}

/// Does this whitespace-delimited word look like the start of a new field
/// rather than a continuation of the previous value?
fn starts_a_new_field(word: &[u8]) -> bool {
    word.windows(3).any(|w| w == b"://") || word.contains(&b'=')
}

/// Redact a path that may contain a username, for screenshots and bug reports.
pub fn scrub_home(path: &str) -> String {
    if let Some(home) = directories::BaseDirs::new() {
        let home_str = home.home_dir().display().to_string();
        if !home_str.is_empty() && path.starts_with(&home_str) {
            return path.replacen(&home_str, "~", 1);
        }
    }
    path.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_hint_also_triggers_the_precheck() {
        // The bug this prevents: `authorization` and `credential` were in
        // KEY_HINTS but absent from the hand-maintained trigger list, so the
        // cheap pre-check returned Cow::Borrowed and an Authorization header
        // carrying a GitHub token passed through completely untouched.
        for hint in KEY_HINTS {
            let sample = format!("{hint}=supersecretvalue");
            assert!(
                needs_scrubbing(&sample),
                "`{hint}` is a key hint but does not trigger scrubbing"
            );
            assert!(
                !scrub(&sample).contains("supersecretvalue"),
                "`{hint}` did not mask its value"
            );
        }
    }

    #[test]
    fn url_credentials_are_masked() {
        let s = scrub("fatal: could not read https://ghp_AbC123DeadBeef@github.com/me/cfg.git");
        assert!(!s.contains("ghp_AbC123DeadBeef"), "token survived: {s}");
        assert!(s.contains("github.com/me/cfg.git"), "host was destroyed: {s}");
    }

    #[test]
    fn user_password_urls_keep_the_username() {
        let s = scrub("connecting to https://andreas:sup3rs3cret@example.com/repo");
        assert!(!s.contains("sup3rs3cret"));
        assert!(s.contains("andreas"));
    }

    #[test]
    fn authorization_headers_are_masked() {
        let s = scrub("GET /repos/x: Authorization: Bearer ghp_LiveTokenValue1234");
        assert!(!s.contains("ghp_LiveTokenValue1234"), "{s}");
    }

    #[test]
    fn quoted_values_with_spaces_are_masked_whole() {
        let s = scrub(r#"KOPIA_PASSWORD="correct horse battery staple""#);
        assert!(!s.contains("correct"), "{s}");
        assert!(!s.contains("staple"), "{s}");
    }

    #[test]
    fn unquoted_values_with_spaces_are_masked_to_end_of_line() {
        let s = scrub("SET KOPIA_PASSWORD=my secret pass phrase");
        assert!(!s.contains("secret pass phrase"), "{s}");
        assert!(s.contains("KOPIA_PASSWORD="), "key name should survive: {s}");
    }

    #[test]
    fn a_value_on_the_following_line_is_masked() {
        let s = scrub("config:\n  password:\n hunter2-the-actual-secret\n  other: fine\n");
        assert!(!s.contains("hunter2-the-actual-secret"), "{s}");
        assert!(s.contains("other: fine"), "unrelated lines must survive: {s}");
    }

    #[test]
    fn json_values_are_masked_without_destroying_neighbours() {
        let s = scrub(r#"{"accessKey":"AKIAEXAMPLE","region":"eu-1"}"#);
        assert!(!s.contains("AKIAEXAMPLE"), "{s}");
        assert!(s.contains("eu-1"), "non-secret fields must survive: {s}");

        let s = scrub(r#"{"secret": "hunter two three", "region": "eu-1"}"#);
        assert!(!s.contains("hunter"), "{s}");
        assert!(s.contains("eu-1"), "{s}");
    }

    #[test]
    fn env_assignments_are_masked() {
        let s = scrub("running with KOPIA_PASSWORD=hunter2 and AWS_SECRET_ACCESS_KEY=abc/def+123");
        assert!(!s.contains("hunter2"), "{s}");
        assert!(!s.contains("abc/def+123"), "{s}");
        assert!(s.contains("KOPIA_PASSWORD="), "{s}");
    }

    #[test]
    fn clean_text_is_borrowed_unchanged() {
        let input = "snapshot created: 42 files, 1.2 GB";
        match scrub(input) {
            Cow::Borrowed(b) => assert_eq!(b, input),
            Cow::Owned(o) => panic!("allocated unnecessarily: {o}"),
        }
    }

    #[test]
    fn scrubbing_is_idempotent() {
        for input in ["KOPIA_PASSWORD=hunter2", "https://tok@github.com/x", r#"{"secret":"a b c"}"#]
        {
            let once = scrub(input).into_owned();
            let twice = scrub(&once).into_owned();
            assert_eq!(once, twice, "not idempotent for {input}");
        }
    }

    #[test]
    fn adversarial_input_stays_linear() {
        // The regression guard for the quadratic blow-up. 200 KB of colons is
        // ten times the payload that previously took nine seconds.
        let hostile = format!("password={}", ":".repeat(200_000));
        let start = std::time::Instant::now();
        let _ = scrub(&hostile);
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_millis(250),
            "scrub took {elapsed:?} on 200 KB — the quadratic path is back"
        );
    }

    #[test]
    fn many_separators_do_not_blow_up() {
        let hostile = "a:".repeat(100_000);
        let start = std::time::Instant::now();
        let _ = scrub(&hostile);
        assert!(start.elapsed() < std::time::Duration::from_millis(250));
    }

    #[test]
    fn multiline_output_is_scrubbed_per_line() {
        let s = scrub("line one\nKOPIA_PASSWORD=abc\nline three\n");
        assert!(!s.contains("abc"));
        assert!(s.contains("line one"));
        assert!(s.contains("line three"));
    }

    #[test]
    fn sensitive_env_var_names_are_all_covered_by_a_hint() {
        for var in SENSITIVE_ENV_VARS {
            let sample = format!("{var}=leakedvalue");
            assert!(
                !scrub(&sample).contains("leakedvalue"),
                "`{var}` is listed as sensitive but its value is not masked"
            );
        }
    }
}
