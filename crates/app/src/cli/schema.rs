//! Machine-readable description of the command-line surface.
//!
//! `superbackup schema --json` walks the *same* `clap::Command` tree the
//! parser uses and renders it as JSON. That is the entire point: a schema
//! written by hand drifts from the implementation within one release, and a
//! caller that trusted it then sends arguments the program rejects.
//!
//! Generating it means the schema is wrong only if the parser is wrong.
//!
//! ## Intended use
//!
//! An automation agent runs this once, learns every command, its arguments,
//! their types, which are required, and what each does — then drives
//! superbackup without a human transcribing documentation into a prompt.
//! Descriptions come from the doc comments in [`super::args`], which is why a
//! test there fails the build if any command lacks one.

use serde::Serialize;

/// The top-level schema document.
#[derive(Debug, Serialize)]
pub struct Schema {
    /// Schema shape version. Bumped if the *shape of this document* changes,
    /// independently of the application version, so a consumer can tell
    /// "superbackup was upgraded" from "the contract changed".
    pub schema_version: u32,
    pub program: &'static str,
    pub version: &'static str,
    pub about: Option<String>,
    /// Stable exit codes and what each means.
    pub exit_codes: Vec<ExitCode>,
    /// Options accepted by every command.
    pub global_options: Vec<Opt>,
    pub commands: Vec<CommandDoc>,
    /// Conventions a caller should know before generating invocations.
    pub notes: Vec<&'static str>,
}

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Serialize)]
pub struct ExitCode {
    pub code: i32,
    pub name: &'static str,
    pub meaning: &'static str,
}

#[derive(Debug, Serialize)]
pub struct CommandDoc {
    /// Full invocation path, e.g. `job add`. This is what a caller appends to
    /// the program name — no reassembly required.
    pub path: String,
    pub name: String,
    pub about: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub long_about: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<Opt>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub positionals: Vec<Opt>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub subcommands: Vec<CommandDoc>,
    /// True when this command has subcommands and does nothing on its own.
    pub is_group: bool,
}

#[derive(Debug, Serialize)]
pub struct Opt {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub long: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub short: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub about: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_name: Option<String>,
    pub required: bool,
    /// May be given more than once, accumulating values.
    pub repeatable: bool,
    /// Takes no value; presence is the value.
    pub is_flag: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// The complete set of accepted values, when the argument is an enum.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
    /// Environment variable that supplies this value when the flag is absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<String>,
    /// Arguments that cannot be combined with this one.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub conflicts_with: Vec<String>,
    /// Arguments that must also be supplied alongside this one.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<String>,
}

const NOTES: &[&str] = &[
    "Every command accepts --json. Output is {\"ok\":true,\"data\":…} or \
     {\"ok\":false,\"error\":{\"code\":…,\"message\":…,\"hint\":…}}. Branch on error.code, \
     which is stable; message text is not.",
    "Pass --no-input in scripts. Without it a command that wants confirmation \
     waits on stdin, which is indistinguishable from a hang.",
    "Jobs, destinations and providers can be named by id, by exact name, or by an \
     unambiguous name prefix. An ambiguous prefix is an error, never a guess.",
    "`run` returns as soon as the run is queued. Add --wait to block until it \
     finishes and to get a non-zero exit code when it fails.",
    "No command accepts a passphrase as an argument. Use --passphrase-file, or \
     `-` to read from stdin.",
    "This CLI is a thin client over the running instance. If nothing is listening, \
     commands exit 3 rather than starting a second copy, because two processes \
     driving one repository risks corrupting it.",
];

impl Schema {
    /// Build the schema by walking the live parser definition.
    pub fn generate() -> Schema {
        use clap::CommandFactory;
        let cmd = super::args::Cli::command();

        let global_options = cmd
            .get_arguments()
            .filter(|a| a.is_global_set())
            .map(describe_arg)
            .collect();

        let commands = cmd
            .get_subcommands()
            .filter(|c| !c.is_hide_set())
            .map(|c| describe_command(c, String::new()))
            .collect();

        Schema {
            schema_version: SCHEMA_VERSION,
            program: "superbackup",
            version: superbackup_core::VERSION,
            about: cmd.get_about().map(|s| s.to_string()),
            exit_codes: exit_codes(),
            global_options,
            commands,
            notes: NOTES.to_vec(),
        }
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Flattened list of every invocable command path, for tests and for
    /// shell-completion style consumers.
    pub fn command_paths(&self) -> Vec<String> {
        fn walk(c: &CommandDoc, out: &mut Vec<String>) {
            out.push(c.path.clone());
            for s in &c.subcommands {
                walk(s, out);
            }
        }
        let mut out = Vec::new();
        for c in &self.commands {
            walk(c, &mut out);
        }
        out
    }
}

fn exit_codes() -> Vec<ExitCode> {
    use super::args::exit;
    vec![
        ExitCode { code: exit::OK, name: "ok", meaning: "The command did what was asked." },
        ExitCode {
            code: exit::FAILED,
            name: "failed",
            meaning: "The command ran and the answer was negative: a job failed, or a check did not pass.",
        },
        ExitCode {
            code: exit::USAGE,
            name: "usage",
            meaning: "Bad usage: unknown job, malformed argument, or contradictory flags.",
        },
        ExitCode {
            code: exit::DAEMON_UNREACHABLE,
            name: "daemon_unreachable",
            meaning: "No superbackup instance is listening. Start the tray application or the service.",
        },
        ExitCode {
            code: exit::LOCKED,
            name: "locked",
            meaning: "The vault is locked and this command needs it open. Run `superbackup unlock`.",
        },
        ExitCode {
            code: exit::CANCELLED,
            name: "cancelled",
            meaning: "The operation was cancelled by the user or by a signal.",
        },
    ]
}

fn describe_command(cmd: &clap::Command, parent: String) -> CommandDoc {
    let name = cmd.get_name().to_string();
    let path = if parent.is_empty() { name.clone() } else { format!("{parent} {name}") };

    let mut options = Vec::new();
    let mut positionals = Vec::new();
    for arg in cmd.get_arguments() {
        // Global options are listed once at the top level rather than
        // repeated under every command, which would triple the document for
        // no information gain.
        if arg.is_global_set() {
            continue;
        }
        if matches!(arg.get_id().as_str(), "help" | "version") {
            continue;
        }
        if arg.is_positional() {
            positionals.push(describe_arg(arg));
        } else {
            options.push(describe_arg(arg));
        }
    }

    let subcommands: Vec<CommandDoc> = cmd
        .get_subcommands()
        .filter(|c| !c.is_hide_set())
        .map(|c| describe_command(c, path.clone()))
        .collect();

    CommandDoc {
        is_group: !subcommands.is_empty(),
        name,
        about: cmd.get_about().map(|s| s.to_string()),
        long_about: cmd.get_long_about().map(|s| s.to_string()).filter(|l| {
            // Only include the long form when it says more than the short one.
            cmd.get_about().map(|a| a.to_string()) != Some(l.clone())
        }),
        aliases: cmd.get_visible_aliases().map(|a| a.to_string()).collect(),
        options,
        positionals,
        subcommands,
        path,
    }
}

fn describe_arg(arg: &clap::Arg) -> Opt {
    let num_args = arg.get_num_args();
    let is_flag = num_args.map(|r| !r.takes_values()).unwrap_or(false);

    Opt {
        name: arg.get_id().to_string(),
        long: arg.get_long().map(|s| s.to_string()),
        short: arg.get_short().map(|c| c.to_string()),
        about: arg
            .get_help()
            .or_else(|| arg.get_long_help())
            .map(|s| s.to_string()),
        value_name: arg.get_value_names().and_then(|n| n.first().map(|s| s.to_string())),
        required: arg.is_required_set(),
        repeatable: matches!(arg.get_action(), clap::ArgAction::Append | clap::ArgAction::Count),
        is_flag,
        default: arg
            .get_default_values()
            .first()
            .map(|v| v.to_string_lossy().into_owned()),
        choices: arg
            .get_possible_values()
            .iter()
            .map(|v| v.get_name().to_string())
            .collect(),
        env: arg.get_env().map(|e| e.to_string_lossy().into_owned()),
        conflicts_with: arg.get_all_conflicts().iter().map(|id| id.to_string()).collect(),
        requires: arg
            .get_requires()
            .map(|(_, id)| id.to_string())
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn schema_is_valid_json_and_round_trips() {
        let s = Schema::generate();
        let json = s.to_json().expect("schema must serialise");
        let parsed: serde_json::Value =
            serde_json::from_str(&json).expect("schema must be valid JSON");
        assert_eq!(parsed["program"], "superbackup");
        assert_eq!(parsed["schema_version"], SCHEMA_VERSION);
        assert!(parsed["commands"].as_array().map(|a| a.len()).unwrap_or(0) > 15);
    }

    #[test]
    fn every_visible_command_appears_in_the_schema() {
        // This is the anti-drift test. If someone adds a subcommand and the
        // walker misses it, an agent reading the schema silently cannot use
        // the new command — a failure with no error message anywhere.
        fn collect(cmd: &clap::Command, parent: &str, out: &mut Vec<String>) {
            for sub in cmd.get_subcommands() {
                if sub.is_hide_set() {
                    continue;
                }
                let path = if parent.is_empty() {
                    sub.get_name().to_string()
                } else {
                    format!("{parent} {}", sub.get_name())
                };
                out.push(path.clone());
                collect(sub, &path, out);
            }
        }
        let mut expected = Vec::new();
        collect(&super::super::args::Cli::command(), "", &mut expected);
        expected.sort();

        let mut actual = Schema::generate().command_paths();
        actual.sort();

        assert_eq!(actual, expected, "schema does not match the live parser definition");
    }

    #[test]
    fn hidden_commands_are_not_advertised() {
        // `service run` is the OS service entry point. Advertising it invites
        // an agent to invoke it directly, which is never correct.
        let paths = Schema::generate().command_paths();
        assert!(
            !paths.iter().any(|p| p == "service run"),
            "hidden commands must not appear in the schema"
        );
        assert!(paths.iter().any(|p| p == "service install"));
    }

    #[test]
    fn global_options_are_listed_once_not_per_command() {
        let s = Schema::generate();
        assert!(s.global_options.iter().any(|o| o.long.as_deref() == Some("json")));
        let status = s.commands.iter().find(|c| c.name == "status").expect("status exists");
        assert!(
            !status.options.iter().any(|o| o.long.as_deref() == Some("json")),
            "global options must not be repeated under every command"
        );
    }

    #[test]
    fn enum_arguments_publish_their_accepted_values() {
        // Without this an agent has to guess that --template takes
        // "developer", and a guess is a failed invocation.
        let s = Schema::generate();
        let job = s.commands.iter().find(|c| c.name == "job").expect("job exists");
        let add = job.subcommands.iter().find(|c| c.name == "add").expect("job add exists");
        let template = add
            .options
            .iter()
            .find(|o| o.long.as_deref() == Some("template"))
            .expect("--template exists");
        assert!(
            template.choices.contains(&"developer".to_string()),
            "enum choices missing: {:?}",
            template.choices
        );
    }

    #[test]
    fn command_paths_are_directly_invocable() {
        // The `path` field must be appendable to the program name verbatim.
        let s = Schema::generate();
        for path in s.command_paths() {
            let mut argv: Vec<&str> = vec!["superbackup"];
            argv.extend(path.split(' '));
            let err = super::super::args::Cli::try_parse_from(&argv).err();
            if let Some(e) = err {
                // Missing required arguments are expected; an unknown
                // subcommand means the path is wrong.
                assert_ne!(
                    e.kind(),
                    clap::error::ErrorKind::InvalidSubcommand,
                    "schema path `{path}` is not a real command"
                );
            }
        }
    }

    #[test]
    fn exit_codes_are_documented_and_distinct() {
        let codes = exit_codes();
        let mut seen = std::collections::BTreeSet::new();
        for c in &codes {
            assert!(seen.insert(c.code), "duplicate exit code {}", c.code);
            assert!(!c.meaning.is_empty());
        }
    }
}
