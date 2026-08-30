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

use std::borrow::Cow;

const MASK: &str = "[redacted]";

/// Environment variable names whose values must never be printed.
pub const SENSITIVE_ENV_VARS: &[&str] = &[
    "KOPIA_PASSWORD",
    "KOPIA_SERVER_PASSWORD",
    "KOPIA_SERVER_CONTROL_PASSWORD",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "SUPERBACKUP_PASSPHRASE",
    "GITHUB_TOKEN",
    "GIT_ASKPASS_PASSWORD",
];

/// Remove anything that looks like a credential from free-form text.
///
/// Deliberately over-eager: a redacted diagnostic is a nuisance, a leaked
/// repository key is unrecoverable.
pub fn scrub(input: &str) -> Cow<'_, str> {
    if !needs_scrubbing(input) {
        return Cow::Borrowed(input);
    }
    let mut out = String::with_capacity(input.len());
    for line in input.split_inclusive('\n') {
        out.push_str(&scrub_line(line));
    }
    Cow::Owned(out)
}

/// Cheap pre-check so the common case allocates nothing.
fn needs_scrubbing(input: &str) -> bool {
    let lower = input.to_ascii_lowercase();
    SENSITIVE_ENV_VARS.iter().any(|k| lower.contains(&k.to_ascii_lowercase()))
        || lower.contains("password")
        || lower.contains("passphrase")
        || lower.contains("secret")
        || lower.contains("token")
        || lower.contains("access-key")
        || lower.contains("accesskey")
        || lower.contains("apikey")
        || lower.contains("api-key")
        || lower.contains("://")
}

fn scrub_line(line: &str) -> String {
    let mut result = redact_urls(line);
    result = redact_assignments(&result);
    result
}

/// `scheme://user:password@host` -> `scheme://user:[redacted]@host`.
///
/// This is the single most common accidental leak: a remote-config URL with an
/// embedded personal access token, echoed verbatim in a git error.
fn redact_urls(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let bytes: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        // Find "://"
        if bytes[i] == ':' && i + 2 < bytes.len() && bytes[i + 1] == '/' && bytes[i + 2] == '/' {
            out.push_str("://");
            i += 3;
            // Consume the authority section up to '/', whitespace, or end.
            let start = i;
            let mut authority = String::new();
            while i < bytes.len() && bytes[i] != '/' && !bytes[i].is_whitespace() {
                authority.push(bytes[i]);
                i += 1;
            }
            let _ = start;
            match authority.split_once('@') {
                Some((userinfo, host)) => {
                    let user = userinfo.split_once(':').map(|(u, _)| u).unwrap_or(userinfo);
                    // A bare token with no username (common for PATs) also gets
                    // masked entirely rather than echoed as the "username".
                    if userinfo.contains(':') {
                        out.push_str(user);
                        out.push(':');
                        out.push_str(MASK);
                    } else {
                        out.push_str(MASK);
                    }
                    out.push('@');
                    out.push_str(host);
                }
                None => out.push_str(&authority),
            }
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

/// `KEY=value`, `key: value`, `--flag=value`, `"key":"value"` where the key
/// names a credential.
fn redact_assignments(line: &str) -> String {
    const KEY_HINTS: &[&str] = &[
        "password",
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
    ];

    let mut out = String::with_capacity(line.len());
    let mut rest = line;

    'outer: loop {
        // Find the earliest separator that follows a credential-looking key.
        let mut best: Option<(usize, usize)> = None; // (key_start, value_start)
        for (idx, ch) in rest.char_indices() {
            if ch != '=' && ch != ':' {
                continue;
            }
            let before = &rest[..idx];
            let key_start = before
                .rfind(|c: char| c.is_whitespace() || c == ',' || c == '{' || c == '&')
                .map(|p| p + 1)
                .unwrap_or(0);
            let key = before[key_start..].trim_matches(|c| c == '"' || c == '\'' || c == '-');
            let key_lower = key.to_ascii_lowercase();
            // `://` is a URL, already handled above.
            if rest[idx..].starts_with("://") {
                continue;
            }
            if KEY_HINTS.iter().any(|h| key_lower.ends_with(h) || key_lower == *h) {
                best = Some((key_start, idx + ch.len_utf8()));
                break;
            }
        }

        match best {
            Some((_, value_start)) => {
                out.push_str(&rest[..value_start]);
                let value_area = &rest[value_start..];
                let leading_ws: usize =
                    value_area.len() - value_area.trim_start_matches(' ').len();
                out.push_str(&value_area[..leading_ws]);
                let value = &value_area[leading_ws..];
                // Value ends at whitespace, comma, closing brace, or quote.
                let end = value
                    .find(|c: char| c.is_whitespace() || c == ',' || c == '}' || c == ')')
                    .unwrap_or(value.len());
                if end == 0 {
                    out.push_str(value);
                    break 'outer;
                }
                out.push_str(MASK);
                rest = &value[end..];
                if rest.is_empty() {
                    break 'outer;
                }
            }
            None => {
                out.push_str(rest);
                break 'outer;
            }
        }
    }
    out
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
    fn env_assignments_are_masked() {
        let s = scrub("running with KOPIA_PASSWORD=hunter2 and AWS_SECRET_ACCESS_KEY=abc/def+123");
        assert!(!s.contains("hunter2"), "{s}");
        assert!(!s.contains("abc/def+123"), "{s}");
        assert!(s.contains("KOPIA_PASSWORD="), "key name should survive: {s}");
    }

    #[test]
    fn json_style_credentials_are_masked() {
        let s = scrub(r#"{"accessKey":"AKIAEXAMPLE","region":"eu-1"}"#);
        assert!(!s.contains("AKIAEXAMPLE"), "{s}");
        assert!(s.contains("eu-1"), "non-secret fields must survive: {s}");
    }

    #[test]
    fn clean_text_is_borrowed_unchanged() {
        let input = "snapshot created: 42 files, 1.2 GB";
        match scrub(input) {
            std::borrow::Cow::Borrowed(b) => assert_eq!(b, input),
            std::borrow::Cow::Owned(o) => panic!("allocated unnecessarily: {o}"),
        }
    }

    #[test]
    fn multiline_output_is_scrubbed_per_line() {
        let s = scrub("line one\nKOPIA_PASSWORD=abc\nline three\n");
        assert!(!s.contains("abc"));
        assert!(s.contains("line one"));
        assert!(s.contains("line three"));
    }

    #[test]
    fn scrubbing_is_idempotent() {
        let once = scrub("KOPIA_PASSWORD=hunter2").into_owned();
        let twice = scrub(&once).into_owned();
        assert_eq!(once, twice);
    }
}
