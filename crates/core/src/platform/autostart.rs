//! "Start superbackup when I log in", on three platforms with three
//! completely different opinions about what that means.
//!
//! | Platform | Mechanism | Notes |
//! |---|---|---|
//! | Windows | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` | Per-user, no elevation needed. Task Scheduler would allow a delay and "only on AC", but needs admin to create and is invisible in the Startup UI users actually look at. |
//! | Linux | `~/.config/autostart/superbackup.desktop` | XDG spec; honoured by GNOME, KDE, XFCE. Nothing runs it on a headless box — that is what the systemd user unit in [`super::service`] is for. |
//! | macOS | `~/Library/LaunchAgents/<label>.plist` | `RunAtLoad`. Modern macOS also shows it under Login Items, and the user can disable it there without us knowing. |
//!
//! # The failure that actually happens
//!
//! Nobody's autostart breaks by being switched off. It breaks because the
//! recorded command still points at the *old* executable: the user moved the
//! install, an updater wrote to a new versioned directory, or they ran it once
//! from `Downloads`. The entry is still "enabled", and silently starts nothing
//! — or worse, starts a stale build. So [`status`] does not answer
//! yes/no; it answers "enabled, and here is the path it points at", and
//! [`heal`] repairs the mismatch.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

// `IoContext` is only used by the non-Windows file-based implementations.
#[cfg_attr(windows, allow(unused_imports))]
use crate::error::{Error, IoContext, Result};
use crate::state::Event;

/// Value name under `Run`, and the stem of the `.desktop` file. Stable: it is
/// what an uninstaller looks for.
pub const ENTRY_NAME: &str = "superbackup";

/// macOS LaunchAgent label, in reverse-DNS form as launchd expects.
pub const LAUNCH_AGENT_LABEL: &str = "io.superbackup.tray";

/// The flag that makes the app come up in the tray instead of opening a window.
/// An autostart entry without it is a bad citizen: nobody wants a window in
/// their face at every login.
pub const MINIMISED_FLAG: &str = "--minimised";

#[cfg(windows)]
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

// ---------------------------------------------------------------------------
// Spec
// ---------------------------------------------------------------------------

/// What we want registered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutostartSpec {
    pub executable: PathBuf,
    pub args: Vec<String>,
    /// Shown in KDE's autostart list and in the plist's `Label` comment.
    pub display_name: String,
}

impl AutostartSpec {
    /// The spec for the currently running executable, started minimised.
    pub fn current() -> Result<AutostartSpec> {
        let exe = std::env::current_exe()
            .map_err(|e| Error::io("determining this program's own path", e))?;
        Ok(AutostartSpec::for_executable(exe))
    }

    pub fn for_executable(executable: impl Into<PathBuf>) -> AutostartSpec {
        AutostartSpec {
            executable: executable.into(),
            args: vec![MINIMISED_FLAG.to_string()],
            display_name: "superbackup".to_string(),
        }
    }

    /// The exact string to store in the Windows `Run` value.
    pub fn windows_command_line(&self) -> String {
        let mut parts = vec![quote_windows_arg(&self.executable.to_string_lossy())];
        parts.extend(self.args.iter().map(|a| quote_windows_arg(a)));
        parts.join(" ")
    }

    /// The `Exec=` line for a `.desktop` file.
    pub fn desktop_exec(&self) -> String {
        let mut parts = vec![escape_desktop_arg(&self.executable.to_string_lossy())];
        parts.extend(self.args.iter().map(|a| escape_desktop_arg(a)));
        parts.join(" ")
    }
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AutostartState {
    /// No entry at all.
    Disabled,
    /// An entry exists and points at the executable we expect.
    Enabled,
    /// An entry exists but points somewhere else — the post-upgrade,
    /// post-move failure. Self-healable.
    Stale { registered: String, expected: String },
    /// An entry exists but we could not parse it. Left alone rather than
    /// clobbered: it may belong to a different install the user wants.
    Unrecognised { registered: String },
}

impl AutostartState {
    pub fn is_enabled(&self) -> bool {
        !matches!(self, AutostartState::Disabled)
    }
    /// True when the entry exists but will not start the right program.
    pub fn needs_repair(&self) -> bool {
        matches!(self, AutostartState::Stale { .. } | AutostartState::Unrecognised { .. })
    }
    pub fn summary(&self) -> String {
        match self {
            AutostartState::Disabled => "superbackup will not start when you log in".to_string(),
            AutostartState::Enabled => "superbackup starts when you log in".to_string(),
            AutostartState::Stale { registered, .. } => {
                format!("Start-at-login points at an old location ({registered}) and will not work")
            }
            AutostartState::Unrecognised { registered } => {
                format!("Start-at-login holds an entry we did not write: {registered}")
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutostartStatus {
    pub state: AutostartState,
    /// Where the entry lives, verbatim, so support can ask the user to look.
    pub location: String,
    /// The raw registered command, when there is one.
    #[serde(default)]
    pub registered_command: Option<String>,
}

/// Where the entry lives on this platform, for the GUI's "Advanced" panel.
pub fn location() -> String {
    #[cfg(windows)]
    {
        format!(r"HKEY_CURRENT_USER\{RUN_KEY}\{ENTRY_NAME}")
    }
    #[cfg(not(windows))]
    {
        entry_file()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "(no home directory)".to_string())
    }
}

/// Is there an autostart entry at all?
pub fn is_enabled() -> Result<bool> {
    Ok(read_registered_command()?.is_some())
}

/// The full picture, including staleness.
pub fn status(spec: &AutostartSpec) -> Result<AutostartStatus> {
    let registered = read_registered_command()?;
    let state = match &registered {
        None => AutostartState::Disabled,
        Some(command) => classify(command, spec),
    };
    Ok(AutostartStatus { state, location: location(), registered_command: registered })
}

/// Pure classification of a registered command against what we want.
///
/// Split out so the interesting logic — "is this the same executable?" — is
/// testable without touching the registry or a home directory.
pub fn classify(registered: &str, spec: &AutostartSpec) -> AutostartState {
    let argv = if cfg!(windows) {
        parse_windows_command_line(registered)
    } else {
        parse_desktop_exec(registered)
    };
    let Some(exe) = argv.first() else {
        return AutostartState::Unrecognised { registered: registered.to_string() };
    };
    let expected = spec.executable.to_string_lossy().into_owned();
    if same_executable(Path::new(exe), &spec.executable) {
        AutostartState::Enabled
    } else if Path::new(exe)
        .file_stem()
        .zip(spec.executable.file_stem())
        .map(|(a, b)| eq_path_component(a.to_string_lossy().as_ref(), b.to_string_lossy().as_ref()))
        .unwrap_or(false)
    {
        // Same program, different location: this is ours and it is stale.
        AutostartState::Stale { registered: exe.clone(), expected }
    } else {
        AutostartState::Unrecognised { registered: registered.to_string() }
    }
}

/// Are these two paths the same executable?
///
/// `canonicalize` resolves the `Program Files` vs `PROGRA~1` short-name case
/// and any symlink or junction; when it fails (the old path no longer exists —
/// exactly the stale case) we fall back to a textual comparison, which is
/// case-insensitive on Windows only.
pub fn same_executable(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => paths_equal(&x, &y),
        _ => paths_equal(a, b),
    }
}

fn paths_equal(a: &Path, b: &Path) -> bool {
    eq_path_component(&a.to_string_lossy(), &b.to_string_lossy())
}

fn eq_path_component(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

// ---------------------------------------------------------------------------
// Mutation
// ---------------------------------------------------------------------------

/// Register the entry, replacing any previous one.
pub fn enable(spec: &AutostartSpec) -> Result<()> {
    #[cfg(windows)]
    {
        use super::win32::{Hive, RegKey};
        let key = RegKey::create(Hive::CurrentUser, RUN_KEY)
            .map_err(|e| Error::Platform(format!("opening the Run key for writing: {e}")))?;
        key.set_string(ENTRY_NAME, &spec.windows_command_line())
            .map_err(|e| Error::Platform(format!("writing the Run entry: {e}")))
    }
    #[cfg(not(windows))]
    {
        let path = entry_file()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ctx(format!("creating {}", parent.display()))?;
        }
        let body = if cfg!(target_os = "macos") {
            render_launch_agent(spec)
        } else {
            render_desktop_entry(spec)
        };
        crate::paths::write_atomic(&path, body.as_bytes())?;
        #[cfg(target_os = "macos")]
        {
            // Writing the plist is enough for the *next* login; bootstrapping
            // makes it take effect now. Failure here is not fatal.
            activate_launch_agent(&path);
        }
        Ok(())
    }
}

/// Remove the entry. Idempotent: removing an entry that is not there succeeds.
pub fn disable() -> Result<()> {
    #[cfg(windows)]
    {
        use super::win32::{Hive, RegKey};
        let Some(key) = RegKey::open_with(
            Hive::CurrentUser,
            RUN_KEY,
            windows::Win32::System::Registry::KEY_READ
                | windows::Win32::System::Registry::KEY_WRITE,
        ) else {
            // No Run key at all: nothing to disable.
            return Ok(());
        };
        key.delete_value(ENTRY_NAME)
            .map_err(|e| Error::Platform(format!("removing the Run entry: {e}")))
    }
    #[cfg(not(windows))]
    {
        let path = entry_file()?;
        #[cfg(target_os = "macos")]
        {
            deactivate_launch_agent(&path);
        }
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Error::io(format!("removing {}", path.display()), e)),
        }
    }
}

/// Re-point a stale entry at the current executable.
///
/// Returns an [`Event`] describing what was fixed, or `None` when nothing
/// needed fixing. Called at every start: this is how an upgrade that moved the
/// binary heals itself instead of silently never starting again.
pub fn heal(spec: &AutostartSpec) -> Result<Option<Event>> {
    let status = status(spec)?;
    match &status.state {
        AutostartState::Stale { registered, expected } => {
            enable(spec)?;
            Ok(Some(
                Event::warn(
                    "autostart.repaired",
                    format!(
                        "Start-at-login pointed at {registered}, which no longer runs \
                         superbackup. Repointed it at {expected}."
                    ),
                )
                .with_field("previous", registered.clone())
                .with_field("current", expected.clone()),
            ))
        }
        _ => Ok(None),
    }
}

fn read_registered_command() -> Result<Option<String>> {
    #[cfg(windows)]
    {
        use super::win32::{Hive, RegKey};
        let Some(key) = RegKey::open(Hive::CurrentUser, RUN_KEY) else {
            return Ok(None);
        };
        Ok(key.string(ENTRY_NAME).filter(|s| !s.trim().is_empty()))
    }
    #[cfg(not(windows))]
    {
        let path = entry_file()?;
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(Error::io(format!("reading {}", path.display()), e)),
        };
        if cfg!(target_os = "macos") {
            Ok(parse_launch_agent_command(&text))
        } else {
            Ok(parse_desktop_entry_exec(&text))
        }
    }
}

#[cfg(not(windows))]
fn entry_file() -> Result<PathBuf> {
    let base = directories::BaseDirs::new()
        .ok_or_else(|| Error::Config("no home directory for this user".into()))?;
    if cfg!(target_os = "macos") {
        Ok(base.home_dir().join("Library/LaunchAgents").join(format!("{LAUNCH_AGENT_LABEL}.plist")))
    } else {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| base.home_dir().join(".config"));
        Ok(config.join("autostart").join(format!("{ENTRY_NAME}.desktop")))
    }
}

#[cfg(target_os = "macos")]
fn activate_launch_agent(path: &Path) {
    if let Some(uid) = super::effective_uid() {
        let domain = format!("gui/{uid}");
        let _ = std::process::Command::new("launchctl")
            .args(["bootstrap", domain.as_str()])
            .arg(path)
            .output();
    }
}

#[cfg(target_os = "macos")]
fn deactivate_launch_agent(path: &Path) {
    if let Some(uid) = super::effective_uid() {
        let target = format!("gui/{uid}/{LAUNCH_AGENT_LABEL}");
        let _ = std::process::Command::new("launchctl").args(["bootout", target.as_str()]).output();
    }
    let _ = path;
}

// ---------------------------------------------------------------------------
// Rendering (pure)
// ---------------------------------------------------------------------------

/// An XDG autostart entry.
///
/// `X-GNOME-Autostart-enabled` and `Hidden` are both honoured, by different
/// desktops, for the same purpose; we write neither as `false`, because the
/// user toggling autostart *off* in their desktop's own settings UI sets one of
/// them, and we must not fight it on the next start.
pub fn render_desktop_entry(spec: &AutostartSpec) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Version=1.0\n\
         Name={name}\n\
         Comment=Keep your backups running in the background\n\
         Exec={exec}\n\
         Icon=superbackup\n\
         Terminal=false\n\
         Categories=Utility;Archiving;\n\
         StartupNotify=false\n",
        name = spec.display_name,
        exec = spec.desktop_exec(),
    )
}

/// A per-user LaunchAgent. `KeepAlive` is deliberately absent: this is the
/// tray, and a user who quits it means it.
pub fn render_launch_agent(spec: &AutostartSpec) -> String {
    let mut args = String::new();
    args.push_str(&format!(
        "\t\t<string>{}</string>\n",
        xml_escape(&spec.executable.to_string_lossy())
    ));
    for arg in &spec.args {
        args.push_str(&format!("\t\t<string>{}</string>\n", xml_escape(arg)));
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \t<key>Label</key>\n\
         \t<string>{label}</string>\n\
         \t<key>ProgramArguments</key>\n\
         \t<array>\n{args}\t</array>\n\
         \t<key>RunAtLoad</key>\n\
         \t<true/>\n\
         \t<key>ProcessType</key>\n\
         \t<string>Background</string>\n\
         </dict>\n\
         </plist>\n",
        label = LAUNCH_AGENT_LABEL,
    )
}

// ---------------------------------------------------------------------------
// Quoting and parsing (pure, and the part most likely to be wrong)
// ---------------------------------------------------------------------------

/// Quote one argument the way `CommandLineToArgvW` will un-quote it.
///
/// This matters more than it looks: `C:\Program Files\superbackup\superbackup.exe`
/// written unquoted into `Run` makes Windows try `C:\Program.exe`, then
/// `C:\Program Files\superbackup\superbackup.exe`… and the first of those is a
/// classic privilege-escalation target. Always quote.
pub fn quote_windows_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"', '\n', '\u{b}']) {
        return arg.to_string();
    }
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');
    let chars: Vec<char> = arg.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let mut backslashes = 0;
        while i < chars.len() && chars[i] == '\\' {
            backslashes += 1;
            i += 1;
        }
        if i == chars.len() {
            // Trailing backslashes must be doubled so they do not escape the
            // closing quote.
            out.extend(std::iter::repeat_n('\\', backslashes * 2));
        } else if chars[i] == '"' {
            out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
            out.push('"');
            i += 1;
        } else {
            out.extend(std::iter::repeat_n('\\', backslashes));
            out.push(chars[i]);
            i += 1;
        }
    }
    out.push('"');
    out
}

/// The inverse: split a stored command line into arguments, following the
/// documented `CommandLineToArgvW` rules. Used to read back what is registered
/// and decide whether it still points at us.
pub fn parse_windows_command_line(command: &str) -> Vec<String> {
    let mut args = Vec::new();
    let chars: Vec<char> = command.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        while i < chars.len() && (chars[i] == ' ' || chars[i] == '\t') {
            i += 1;
        }
        if i >= chars.len() {
            break;
        }
        let mut current = String::new();
        let mut in_quotes = false;
        while i < chars.len() {
            if chars[i] == '\\' {
                let mut backslashes = 0;
                while i < chars.len() && chars[i] == '\\' {
                    backslashes += 1;
                    i += 1;
                }
                if i < chars.len() && chars[i] == '"' {
                    current.extend(std::iter::repeat_n('\\', backslashes / 2));
                    if backslashes % 2 == 1 {
                        current.push('"');
                    } else {
                        in_quotes = !in_quotes;
                    }
                    i += 1;
                } else {
                    current.extend(std::iter::repeat_n('\\', backslashes));
                }
                continue;
            }
            if chars[i] == '"' {
                // `""` inside a quoted run is an escaped quote.
                if in_quotes && i + 1 < chars.len() && chars[i + 1] == '"' {
                    current.push('"');
                    i += 2;
                    continue;
                }
                in_quotes = !in_quotes;
                i += 1;
                continue;
            }
            if !in_quotes && (chars[i] == ' ' || chars[i] == '\t') {
                break;
            }
            current.push(chars[i]);
            i += 1;
        }
        args.push(current);
    }
    args
}

/// Escape one argument for a `.desktop` `Exec=` line.
///
/// The XDG spec is genuinely awkward: the value is first unescaped as a desktop
/// entry string (so `\` doubles), and *then* split into arguments with shell-ish
/// quoting where `"`, `` ` ``, `$` and `\` must be backslash-escaped inside the
/// quotes. `%` is reserved for field codes and is written `%%`.
pub fn escape_desktop_arg(arg: &str) -> String {
    let needs_quotes = arg.is_empty()
        || arg.contains([
            ' ', '\t', '"', '\'', '\\', '$', '`', '>', '<', '~', '|', '&', ';', '*', '?', '#', '(',
            ')',
        ]);
    let mut inner = String::with_capacity(arg.len());
    for ch in arg.chars() {
        match ch {
            // A literal backslash is escaped twice over: once for the
            // shell-ish Exec tokeniser (`\` -> `\\`) and then once more for
            // the desktop-entry string decoder, which turns every `\\` back
            // into one `\`. Four characters on disk for one backslash.
            '\\' => inner.push_str(r"\\\\"),
            // These only need the tokeniser's escape; the leading backslash is
            // itself doubled for the value decoder, giving `\\"`, `\\$`, ``\\` ``.
            '"' | '`' | '$' => {
                inner.push_str(r"\\");
                inner.push(ch);
            }
            '%' => inner.push_str("%%"),
            other => inner.push(other),
        }
    }
    if needs_quotes {
        format!("\"{inner}\"")
    } else {
        inner
    }
}

/// Undo the desktop-entry *string* encoding. This is the first of the two
/// decoding passes the XDG spec mandates for an `Exec=` value, and skipping it
/// is why so many launchers mangle paths containing a backslash.
pub fn decode_desktop_value(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    let mut out = String::with_capacity(value.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\' && i + 1 < chars.len() {
            match chars[i + 1] {
                's' => {
                    out.push(' ');
                    i += 2;
                    continue;
                }
                'n' => {
                    out.push('\n');
                    i += 2;
                    continue;
                }
                't' => {
                    out.push('\t');
                    i += 2;
                    continue;
                }
                'r' => {
                    out.push('\r');
                    i += 2;
                    continue;
                }
                '\\' => {
                    out.push('\\');
                    i += 2;
                    continue;
                }
                // `\"` and `\$` are not desktop-entry escapes; they belong to
                // the second (tokenising) pass and must survive this one.
                _ => {}
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Split an `Exec=` value back into arguments. Tolerant: an entry written by
/// another tool must be readable enough for us to decide it is not ours.
pub fn parse_desktop_exec(exec: &str) -> Vec<String> {
    let decoded = decode_desktop_value(exec);
    let chars: Vec<char> = decoded.chars().collect();
    let mut args = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        if i >= chars.len() {
            break;
        }
        let mut current = String::new();
        if chars[i] == '"' {
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    current.push(chars[i + 1]);
                    i += 2;
                    continue;
                }
                current.push(chars[i]);
                i += 1;
            }
            i += 1; // closing quote
        } else {
            while i < chars.len() && !chars[i].is_whitespace() {
                current.push(chars[i]);
                i += 1;
            }
        }
        args.push(current.replace("%%", "%"));
    }
    args
}

/// Pull the `Exec=` line out of a `.desktop` file.
pub fn parse_desktop_entry_exec(text: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Exec=") {
            if !rest.trim().is_empty() {
                return Some(rest.trim().to_string());
            }
        }
    }
    None
}

/// Reconstruct a command line from a LaunchAgent's `ProgramArguments`.
pub fn parse_launch_agent_command(text: &str) -> Option<String> {
    let after = text.split_once("<key>ProgramArguments</key>")?.1;
    let start = after.find("<array>")? + "<array>".len();
    let end = after[start..].find("</array>")?;
    let body = &after[start..start + end];
    let mut parts = Vec::new();
    let mut rest = body;
    while let Some(open) = rest.find("<string>") {
        let after_open = &rest[open + "<string>".len()..];
        let Some(close) = after_open.find("</string>") else {
            break;
        };
        parts.push(xml_unescape(after_open[..close].trim()));
        rest = &after_open[close..];
    }
    if parts.is_empty() {
        None
    } else {
        // Re-quote so `classify` can parse it with the same rules it uses for
        // a `.desktop` Exec line.
        Some(parts.iter().map(|p| escape_desktop_arg(p)).collect::<Vec<_>>().join(" "))
    }
}

pub fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    out
}

fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(path: &str) -> AutostartSpec {
        AutostartSpec::for_executable(path)
    }

    #[test]
    fn windows_paths_with_spaces_round_trip() {
        let s = spec(r"C:\Program Files\superbackup\superbackup.exe");
        let line = s.windows_command_line();
        assert_eq!(line, "\"C:\\Program Files\\superbackup\\superbackup.exe\" --minimised");
        let argv = parse_windows_command_line(&line);
        assert_eq!(argv[0], r"C:\Program Files\superbackup\superbackup.exe");
        assert_eq!(argv[1], "--minimised");
    }

    #[test]
    fn windows_quoting_survives_backslashes_and_quotes() {
        for arg in [
            r"C:\dir with space\a.exe",
            r"trailing\\",
            r#"has "quotes" inside"#,
            r"C:\a\\b\c",
            "",
            "plain",
        ] {
            let quoted = quote_windows_arg(arg);
            let parsed = parse_windows_command_line(&quoted);
            assert_eq!(parsed, vec![arg.to_string()], "round trip failed for {arg:?} -> {quoted}");
        }
    }

    #[test]
    fn command_line_parser_matches_win32_rules() {
        assert_eq!(parse_windows_command_line(r#""a b" c"#), vec!["a b", "c"]);
        assert_eq!(parse_windows_command_line(r#"a\\b"#), vec![r"a\\b"]);
        assert_eq!(parse_windows_command_line(r#""a\\b""#), vec![r"a\\b"]);
        assert_eq!(parse_windows_command_line(r#""a\"b""#), vec![r#"a"b"#]);
        assert_eq!(parse_windows_command_line("   "), Vec::<String>::new());
    }

    #[test]
    fn desktop_exec_escapes_and_round_trips() {
        let s = spec("/opt/super backup/bin/superbackup");
        let exec = s.desktop_exec();
        assert!(exec.starts_with('"'), "a path with a space must be quoted: {exec}");
        let argv = parse_desktop_exec(&exec);
        assert_eq!(argv[0], "/opt/super backup/bin/superbackup");
        assert_eq!(argv[1], "--minimised");
    }

    #[test]
    fn desktop_exec_escapes_shell_metacharacters() {
        let exec = escape_desktop_arg("/opt/$HOME/`x`/a\"b");
        assert!(exec.contains("\\\\$"), "$ must be escaped: {exec}");
        assert!(exec.contains("\\\\`"), "backtick must be escaped: {exec}");
        assert_eq!(parse_desktop_exec(&exec), vec!["/opt/$HOME/`x`/a\"b".to_string()]);
    }

    #[test]
    fn desktop_field_codes_are_neutralised() {
        assert_eq!(escape_desktop_arg("100%"), "100%%");
        assert_eq!(parse_desktop_exec("100%%"), vec!["100%".to_string()]);
    }

    #[test]
    fn desktop_entry_contains_the_minimised_flag() {
        let text = render_desktop_entry(&spec("/usr/bin/superbackup"));
        let exec = parse_desktop_entry_exec(&text).expect("Exec line");
        assert!(exec.contains(MINIMISED_FLAG), "{exec}");
        assert!(text.contains("Type=Application"));
        assert!(!text.contains("Hidden="), "we must not fight the desktop's own toggle");
    }

    #[test]
    fn launch_agent_round_trips_through_its_own_parser() {
        let s = AutostartSpec {
            executable: PathBuf::from("/Applications/superbackup.app/Contents/MacOS/superbackup"),
            args: vec![MINIMISED_FLAG.to_string()],
            display_name: "superbackup".into(),
        };
        let plist = render_launch_agent(&s);
        assert!(plist.contains("<key>RunAtLoad</key>"));
        let command = parse_launch_agent_command(&plist).expect("ProgramArguments");
        let argv = parse_desktop_exec(&command);
        assert_eq!(argv[0], "/Applications/superbackup.app/Contents/MacOS/superbackup");
        assert_eq!(argv[1], MINIMISED_FLAG);
    }

    #[test]
    fn a_moved_executable_is_detected_as_stale() {
        let want = spec(if cfg!(windows) {
            r"C:\Program Files\superbackup\superbackup.exe"
        } else {
            "/opt/superbackup/superbackup"
        });
        let old = if cfg!(windows) {
            "\"C:\\Users\\me\\Downloads\\superbackup.exe\" --minimised"
        } else {
            "/home/me/Downloads/superbackup --minimised"
        };
        match classify(old, &want) {
            AutostartState::Stale { registered, expected } => {
                assert!(registered.contains("Downloads"));
                assert_eq!(expected, want.executable.to_string_lossy());
            }
            other => panic!("expected Stale, got {other:?}"),
        }
    }

    #[test]
    fn a_matching_entry_is_enabled() {
        let want = spec(if cfg!(windows) { r"C:\a\superbackup.exe" } else { "/a/superbackup" });
        let line = if cfg!(windows) { want.windows_command_line() } else { want.desktop_exec() };
        assert_eq!(classify(&line, &want), AutostartState::Enabled);
    }

    #[test]
    fn someone_elses_entry_is_left_alone() {
        let want = spec(if cfg!(windows) { r"C:\a\superbackup.exe" } else { "/a/superbackup" });
        let other = if cfg!(windows) { r"C:\Windows\notepad.exe" } else { "/usr/bin/gedit" };
        assert!(matches!(classify(other, &want), AutostartState::Unrecognised { .. }));
    }

    #[test]
    fn state_summaries_are_written_for_humans() {
        assert!(AutostartState::Disabled.summary().contains("will not start"));
        assert!(AutostartState::Stale { registered: "old".into(), expected: "new".into() }
            .needs_repair());
        assert!(!AutostartState::Enabled.needs_repair());
    }

    #[test]
    fn xml_escaping_covers_the_five_entities() {
        assert_eq!(xml_escape("a&b<c>d\"e'f"), "a&amp;b&lt;c&gt;d&quot;e&apos;f");
        assert_eq!(xml_unescape("a&amp;b&lt;c&gt;d&quot;e&apos;f"), "a&b<c>d\"e'f");
    }
}
