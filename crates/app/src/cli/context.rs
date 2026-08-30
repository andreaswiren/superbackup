//! What every command is handed.

use superbackup_core::paths::Paths;

use super::args::GlobalArgs;
use super::output::Ui;

/// The command's world: the global flags, where this instance keeps its files,
/// and where its output goes.
pub struct Ctx {
    pub global: GlobalArgs,
    pub paths: Paths,
    pub ui: Ui,
    /// Talk to this endpoint instead of the one `paths` implies.
    ///
    /// There is no command-line flag for this and there will not be one: the
    /// CLI sends the master passphrase over this socket, and a knob that
    /// redirects it is a knob for stealing passphrases. It exists so the test
    /// suite can put a mock daemon behind a private endpoint, which on Windows
    /// is otherwise impossible — `Paths::ipc_endpoint` returns the fixed pipe
    /// name `\\.\pipe\superbackup` there whatever `--home` says.
    pub endpoint_override: Option<String>,
}

impl Ctx {
    pub fn new(global: GlobalArgs, paths: Paths, ui: Ui) -> Ctx {
        Ctx { global, paths, ui, endpoint_override: None }
    }

    pub fn endpoint(&self) -> String {
        match &self.endpoint_override {
            Some(e) => e.clone(),
            None => self.paths.ipc_endpoint(),
        }
    }

    /// True when a prompt would reach a human who can answer it.
    pub fn can_prompt(&self) -> bool {
        !self.global.no_input && self.ui.stdin_is_tty
    }
}
