//! The SSH agent: which one, what it is holding, and whether that survives a
//! reboot.
//!
//! # The trap this module exists to avoid
//!
//! On Windows there are usually **two** SSH agents and they are not the same
//! agent. Git for Windows ships its own `ssh-add` in `C:\Program Files\Git`,
//! which talks to an MSYS agent started per shell; Windows ships one in
//! `System32\OpenSSH`, which talks to the `ssh-agent` *service*. Both are on
//! `PATH`, Git's is usually first, and they can give opposite answers about
//! the same key on the same machine:
//!
//! ```text
//! $ ssh-add -l                                    # Git's
//! Could not open a connection to your authentication agent.
//! $ /c/Windows/System32/OpenSSH/ssh-add.exe -l    # Windows'
//! 256 SHA256:XziF6… andreas@AWPC34 (ED25519)
//! ```
//!
//! Only the service agent persists: it keeps keys encrypted in the registry
//! and reloads them at every boot, which is exactly what "open my key
//! automatically and never ask me" means on Windows. So this module resolves
//! the tool by path rather than by `PATH`, and says which agent it asked.
//!
//! # What it will not do
//!
//! Type a passphrase for you. `ssh-add` takes one from a terminal or an
//! askpass helper, never from an argument — and putting one in an argument is
//! exactly the leak the whole codebase avoids. A key with a passphrase is
//! therefore reported as needing one, and adding it is left to the user in
//! their own terminal, once.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Which agent answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    /// The Windows `ssh-agent` service. Keys survive a reboot.
    WindowsService,
    /// An agent reached through `SSH_AUTH_SOCK` — the usual arrangement on
    /// Linux and macOS, and what Git for Windows starts per shell.
    Socket,
    /// Nothing answered.
    None,
}

/// What the agent is holding, and whether it will still be holding it
/// tomorrow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentStatus {
    pub kind: AgentKind,
    pub running: bool,
    /// `SHA256:…` for every key loaded, in the same form the Credentials page
    /// shows, so the two can be compared directly.
    pub loaded: Vec<String>,
    /// Whether a key added now is still there after a restart.
    pub persists_across_reboot: bool,
    /// One sentence for the user about the arrangement they actually have.
    pub note: String,
}

impl AgentStatus {
    pub fn holds(&self, fingerprint: &str) -> bool {
        self.loaded.iter().any(|f| f == fingerprint)
    }
}

/// `ssh-add`, resolved to the agent that matters rather than to `PATH`.
fn ssh_add() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        // The service agent first, deliberately: it is the one whose keys
        // survive a reboot, and it is *not* the one `PATH` usually finds.
        let system = std::env::var_os("SystemRoot")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
            .join(r"System32\OpenSSH\ssh-add.exe");
        if system.is_file() {
            return Some(system);
        }
    }
    locate("ssh-add")
}

/// `ssh-keygen`, resolved the same way for the same reason: a key made by one
/// OpenSSH and used by another is fine, but keeping to one avoids surprises
/// about default formats.
pub fn ssh_keygen() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let system = std::env::var_os("SystemRoot")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
            .join(r"System32\OpenSSH\ssh-keygen.exe");
        if system.is_file() {
            return Some(system);
        }
    }
    locate("ssh-keygen")
}

fn locate(tool: &str) -> Option<PathBuf> {
    let exe = if cfg!(windows) { format!("{tool}.exe") } else { tool.to_string() };
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|dir| dir.join(&exe)).find(|c| c.is_file())
}

/// Ask the agent what it is holding.
pub async fn status() -> AgentStatus {
    let Some(tool) = ssh_add() else {
        return AgentStatus {
            kind: AgentKind::None,
            running: false,
            loaded: Vec::new(),
            persists_across_reboot: false,
            note: "OpenSSH is not installed, so there is no agent to load keys into.".into(),
        };
    };
    let windows_service = cfg!(windows) && tool.to_string_lossy().contains("System32");

    let mut command = tokio::process::Command::new(&tool);
    command.arg("-l");
    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    crate::kopia::harden_child(&mut command);

    let output = match tokio::time::timeout(
        std::time::Duration::from_secs(10),
        command.output(),
    )
    .await
    {
        Ok(Ok(output)) => output,
        _ => {
            return AgentStatus {
                kind: AgentKind::None,
                running: false,
                loaded: Vec::new(),
                persists_across_reboot: false,
                note: "The agent did not answer.".into(),
            }
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    // `ssh-add -l` exits 1 for "no identities" and 2 for "cannot reach the
    // agent". The difference is the whole question: an empty agent is running.
    let running = output.status.code() != Some(2)
        && !stderr.contains("Could not open a connection")
        && !stderr.contains("Error connecting");

    let loaded = parse_loaded(&stdout);
    let kind = match (running, windows_service) {
        (false, _) => AgentKind::None,
        (true, true) => AgentKind::WindowsService,
        (true, false) => AgentKind::Socket,
    };
    AgentStatus {
        note: note_for(kind, loaded.len()),
        persists_across_reboot: kind == AgentKind::WindowsService,
        kind,
        running,
        loaded,
    }
}

/// Pull the fingerprints out of `ssh-add -l`.
///
/// Each line is `<bits> SHA256:<base64> <comment> (<TYPE>)`.
fn parse_loaded(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| {
            line.split_whitespace().find(|field| field.starts_with("SHA256:")).map(str::to_string)
        })
        .collect()
}

fn note_for(kind: AgentKind, loaded: usize) -> String {
    match kind {
        AgentKind::WindowsService => format!(
            "The Windows ssh-agent service is running and holding {loaded} key{}. It keeps them \
             encrypted in the registry and loads them again at every boot, so a key added here \
             is not asked for again.",
            if loaded == 1 { "" } else { "s" }
        ),
        AgentKind::Socket => format!(
            "An agent is reachable and holding {loaded} key{}. Agents reached this way usually \
             end with the session, so a key added now is likely to be gone after a restart \
             unless something loads it at login.",
            if loaded == 1 { "" } else { "s" }
        ),
        AgentKind::None => "No agent is running, so every use of a key asks for it.".to_string(),
    }
}

/// Load a key into the agent.
///
/// Refuses a key with a passphrase. `ssh-add` takes one from a terminal or an
/// askpass helper and never from an argument, and putting one in an argument
/// is exactly the leak this codebase does not permit — so a protected key is
/// added by the user, in their own terminal, once.
pub async fn add(private_key: &std::path::Path, encrypted: Option<bool>) -> Result<String> {
    if encrypted == Some(true) {
        return Err(Error::Validation(format!(
            "{} is protected by its own passphrase. Superbackup cannot type that for you — a \
             passphrase passed on a command line is readable by every process on the machine. \
             Run `ssh-add \"{}\"` once in a terminal instead; on Windows the agent then keeps it \
             across reboots.",
            private_key.display(),
            private_key.display()
        )));
    }
    let tool = ssh_add().ok_or_else(|| {
        Error::Config("OpenSSH is not installed, so there is no agent to load keys into.".into())
    })?;

    let mut command = tokio::process::Command::new(&tool);
    command.arg(private_key);
    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    // Never let it raise a prompt: with no terminal it would block for ever,
    // and in a tray application that is an application that has stopped.
    command.env("SSH_ASKPASS_REQUIRE", "never");
    command.env("DISPLAY", "");
    crate::kopia::harden_child(&mut command);

    let output = tokio::time::timeout(std::time::Duration::from_secs(20), command.output())
        .await
        .map_err(|_| Error::Config("the agent did not answer within twenty seconds".into()))?
        .map_err(|e| Error::io("running ssh-add", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let reason = stderr
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("ssh-add failed without saying why");
        return Err(Error::Config(reason.to_string()));
    }
    Ok(format!("{} was loaded into the agent.", private_key.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real `ssh-add -l` output, including the case that matters: an agent
    /// that is running and holding nothing looks nothing like one that is not
    /// running, and confusing the two would have the page telling people to
    /// start an agent they already have.
    #[test]
    fn fingerprints_are_read_in_the_form_the_credentials_page_shows() {
        let output = "256 SHA256:XziF6PsU6xsajzIBXuqdfI8pTgJgbBCl7oY/3dDiXwY andreas@AWPC34 (ED25519)\n\
                      4096 SHA256:abcdefghijklmnopqrstuvwxyz0123456789ABCDEFG work@laptop (RSA)\n";
        let loaded = parse_loaded(output);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0], "SHA256:XziF6PsU6xsajzIBXuqdfI8pTgJgbBCl7oY/3dDiXwY");
        assert!(loaded[1].starts_with("SHA256:"));

        // An empty agent says so in words, and holds nothing.
        assert!(parse_loaded("The agent has no identities.\n").is_empty());
        assert!(parse_loaded("").is_empty());
    }

    /// Only the Windows service agent survives a reboot, and saying otherwise
    /// would be promising something the arrangement does not do.
    #[test]
    fn only_the_service_agent_is_claimed_to_survive_a_restart() {
        let service = AgentStatus {
            kind: AgentKind::WindowsService,
            running: true,
            loaded: vec!["SHA256:x".into()],
            persists_across_reboot: true,
            note: note_for(AgentKind::WindowsService, 1),
        };
        assert!(service.persists_across_reboot);
        assert!(service.note.contains("every boot"));
        assert!(service.holds("SHA256:x"));
        assert!(!service.holds("SHA256:y"));

        // A socket agent is not promised to.
        assert!(note_for(AgentKind::Socket, 1).contains("likely to be gone"));
        assert!(note_for(AgentKind::None, 0).contains("No agent"));
    }

    /// A key with a passphrase is refused with the command to run, not with a
    /// shrug — and above all without superbackup putting a passphrase on a
    /// command line.
    #[tokio::test]
    async fn a_protected_key_is_refused_with_the_command_to_run() {
        let err = add(std::path::Path::new("/home/a/.ssh/id_rsa"), Some(true))
            .await
            .expect_err("must be refused");
        let text = err.to_string();
        assert!(text.contains("ssh-add"), "{text}");
        assert!(text.contains("readable by every process"), "{text}");
    }
}
