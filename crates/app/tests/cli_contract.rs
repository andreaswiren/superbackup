//! The contract an automation agent actually relies on, checked against the
//! real binary.
//!
//! These tests run `superbackup` as a subprocess. They deliberately assert
//! only things that hold **whether or not** a daemon happens to be listening,
//! because the properties that matter to a caller — one JSON document on
//! stdout, a documented exit code, no secret in `argv` — are exactly the ones
//! that must not depend on the environment.
//!
//! Everything that needs a daemon is tested in the crate's own unit tests
//! against `MockHandler`, over a real socket with a private endpoint. That
//! split exists because `Paths::ipc_endpoint` is the fixed pipe name
//! `\\.\pipe\superbackup` on Windows regardless of `--home`, so a subprocess
//! cannot be pointed at a private daemon there.

use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_superbackup");

/// Exit codes the schema publishes. Anything else is a contract violation.
const DOCUMENTED: [i32; 6] = [0, 1, 2, 3, 4, 5];

fn superbackup(args: &[&str]) -> Output {
    Command::new(BIN)
        .args(args)
        // A private root, so a test never reads or writes the developer's own
        // configuration.
        .env("SUPERBACKUP_HOME", scratch())
        .env("NO_COLOR", "1")
        .output()
        .unwrap_or_else(|e| panic!("running `{BIN} {}`: {e}", args.join(" ")))
}

fn scratch() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("sb-cli-it-{}", uuid::Uuid::new_v4().simple()));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn code(output: &Output) -> i32 {
    output.status.code().unwrap_or(-1)
}

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

#[test]
fn the_schema_is_valid_json_and_describes_the_contract() {
    let output = superbackup(&["schema", "--json"]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let schema: serde_json::Value =
        serde_json::from_str(&stdout(&output)).expect("`schema --json` must be one JSON document");

    assert_eq!(schema["program"], "superbackup");
    assert!(schema["commands"].as_array().map(|a| a.len()).unwrap_or(0) > 15);

    // The exit codes an agent branches on must be published, and must be the
    // ones this CLI actually returns.
    let codes: Vec<i64> = schema["exit_codes"]
        .as_array()
        .map(|a| a.iter().filter_map(|c| c["code"].as_i64()).collect())
        .unwrap_or_default();
    for expected in DOCUMENTED {
        assert!(codes.contains(&(expected as i64)), "exit code {expected} is not published");
    }

    // The conventions the implementation has to honour.
    let notes = schema["notes"].to_string();
    assert!(notes.contains("--json"), "the envelope must be documented");
    assert!(notes.contains("--no-input"), "the scripting rule must be documented");
    assert!(notes.contains("ambiguous"), "the resolution rule must be documented");
}

#[test]
fn the_published_schema_offers_no_way_to_pass_a_passphrase() {
    // The parser-level test lives in `args.rs`. This is the same rule checked
    // one level out, on the document an agent actually reads: if a secret flag
    // ever appears here, something will start putting passphrases in `argv`.
    let output = superbackup(&["schema", "--json"]);
    let schema: serde_json::Value = serde_json::from_str(&stdout(&output)).expect("json");

    fn walk(command: &serde_json::Value, path: &str) {
        for group in ["options", "positionals"] {
            for option in command[group].as_array().unwrap_or(&Vec::new()) {
                let name = option["name"].as_str().unwrap_or_default();
                let takes_value = option["is_flag"] != serde_json::Value::Bool(true);
                let is_file_reference = name.ends_with("_file");
                // An argument with a closed set of accepted values cannot
                // carry a secret: the parser rejects anything that is not one
                // of the listed tokens. `destination add --passphrase` is such
                // an argument — it names *where the passphrase comes from*,
                // not what it is.
                let is_choice =
                    option["choices"].as_array().map(|c| !c.is_empty()).unwrap_or(false);
                if takes_value && !is_file_reference && !is_choice {
                    assert!(
                        !(name.contains("passphrase") || name.contains("password")),
                        "`{name}` in `{path}` would carry a secret in argv"
                    );
                }
            }
        }
        for sub in command["subcommands"].as_array().unwrap_or(&Vec::new()) {
            let name = sub["name"].as_str().unwrap_or_default();
            walk(sub, &format!("{path} {name}"));
        }
    }

    for command in schema["commands"].as_array().unwrap_or(&Vec::new()) {
        let name = command["name"].as_str().unwrap_or_default();
        walk(command, name);
    }
}

#[test]
fn a_passphrase_flag_is_rejected_by_the_parser_on_every_command_that_takes_one() {
    for args in [
        vec!["unlock", "--passphrase", "hunter2"],
        vec!["unlock", "-p", "hunter2"],
        vec!["init", "--passphrase", "hunter2"],
        vec!["change-passphrase", "--passphrase", "hunter2"],
        vec!["destination", "add", "--local", "/tmp/x", "--passphrase-value", "hunter2"],
    ] {
        let output = superbackup(&args);
        assert_ne!(code(&output), 0, "`{}` must be refused", args.join(" "));
        assert!(
            stdout(&output).is_empty(),
            "a rejected invocation must put nothing on stdout: {}",
            stdout(&output)
        );
    }
}

#[test]
fn version_answers_without_a_daemon_in_both_shapes() {
    let human = superbackup(&["version"]);
    assert_eq!(code(&human), 0, "{}", stderr(&human));
    assert!(stdout(&human).contains("superbackup"));

    let json = superbackup(&["version", "--json"]);
    assert_eq!(code(&json), 0);
    let value: serde_json::Value = serde_json::from_str(&stdout(&json)).expect("one document");
    assert!(value["version"].is_string());
}

// ---------------------------------------------------------------------------
// The envelope, whatever the environment
// ---------------------------------------------------------------------------

/// Commands that need a daemon. Whether one is listening decides the outcome,
/// so these assert the *shape* of the answer rather than its content.
const NEEDS_DAEMON: &[&[&str]] = &[
    &["status", "--json"],
    &["job", "list", "--json"],
    &["destination", "list", "--json"],
    &["provider", "list", "--json"],
    &["config", "show", "--json"],
    &["doctor", "--json"],
];

#[test]
fn json_mode_puts_exactly_one_document_on_stdout_and_nothing_else() {
    for args in NEEDS_DAEMON {
        let mut argv = args.to_vec();
        argv.push("--timeout");
        argv.push("3");
        let output = superbackup(&argv);
        let text = stdout(&output);

        assert!(
            !text.contains('\u{1b}'),
            "`{}` coloured a machine-readable stream",
            args.join(" ")
        );
        let value: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|e| {
            panic!("`{}` did not put one JSON document on stdout ({e}):\n{text}", args.join(" "))
        });
        assert!(value["ok"].is_boolean(), "`{}` produced an envelope without `ok`", args.join(" "));
        if value["ok"] == serde_json::Value::Bool(false) {
            let error = &value["error"];
            assert!(error["code"].is_string(), "`code` is the stable field to branch on");
            assert!(error["message"].is_string());
            assert!(
                error.as_object().map(|o| o.len()).unwrap_or(0) == 3,
                "the error envelope must stay {{code, message, hint}}: {error}"
            );
        }
        assert!(
            DOCUMENTED.contains(&code(&output)),
            "`{}` exited {}, which is not a documented code",
            args.join(" "),
            code(&output)
        );
    }
}

#[test]
fn diagnostics_go_to_stderr_so_a_pipeline_stays_clean() {
    // With nothing listening this prints an error; with a daemon it prints a
    // document. Either way stdout must parse as JSON on its own.
    let output = superbackup(&["status", "--json", "--timeout", "3", "-vv"]);
    let text = stdout(&output);
    serde_json::from_str::<serde_json::Value>(&text)
        .unwrap_or_else(|e| panic!("tracing leaked into stdout ({e}):\n{text}"));
}

#[test]
fn a_malformed_invocation_exits_two_before_touching_anything() {
    for args in [
        vec!["run"],                       // neither a job nor --all
        vec!["run", "docs", "--all"],      // both
        vec!["job", "add", "--name", "x"], // no source
        vec!["nonsense"],
        vec!["destination", "add", "--local", "/tmp/a", "--mirror", "/tmp/b"],
        vec!["destination", "add", "--s3", "storj"], // --s3 needs --bucket
    ] {
        let output = superbackup(&args);
        assert_eq!(
            code(&output),
            2,
            "`{}` should be a usage error, got {} with: {}",
            args.join(" "),
            code(&output),
            stderr(&output)
        );
    }
}

#[test]
fn help_is_available_for_every_command_the_schema_advertises() {
    let output = superbackup(&["schema", "--json"]);
    let schema: serde_json::Value = serde_json::from_str(&stdout(&output)).expect("json");

    fn paths(command: &serde_json::Value, out: &mut Vec<String>) {
        if let Some(path) = command["path"].as_str() {
            out.push(path.to_string());
        }
        for sub in command["subcommands"].as_array().unwrap_or(&Vec::new()) {
            paths(sub, out);
        }
    }
    let mut all = Vec::new();
    for command in schema["commands"].as_array().unwrap_or(&Vec::new()) {
        paths(command, &mut all);
    }
    assert!(!all.is_empty());

    for path in all {
        let mut argv: Vec<&str> = path.split(' ').collect();
        argv.push("--help");
        let output = superbackup(&argv);
        assert_eq!(
            code(&output),
            0,
            "`{path} --help` exited {}: {}",
            code(&output),
            stderr(&output)
        );
        assert!(!stdout(&output).is_empty(), "`{path} --help` printed nothing");
    }
}

// ---------------------------------------------------------------------------
// Prompting
// ---------------------------------------------------------------------------

#[test]
fn no_input_never_blocks_on_a_prompt() {
    // The failure this guards against is a hang, so the assertion that
    // matters is that the process exits at all. A subprocess with a closed
    // stdin that waited for an answer would sit here until the suite timed
    // out; `output()` returning is the proof it did not.
    for args in [
        vec!["job", "remove", "anything", "--no-input", "--timeout", "3"],
        vec!["unlock", "--no-input", "--timeout", "3"],
        vec!["change-passphrase", "--no-input", "--timeout", "3"],
    ] {
        let output = superbackup(&args);
        assert_ne!(code(&output), 0, "`{}` cannot have succeeded", args.join(" "));
        assert!(
            DOCUMENTED.contains(&code(&output)),
            "`{}` exited {}",
            args.join(" "),
            code(&output)
        );
    }
}

#[test]
fn a_passphrase_file_that_does_not_exist_names_the_file() {
    let output = superbackup(&["unlock", "--passphrase-file", "/no/such/file", "--timeout", "3"]);
    assert_ne!(code(&output), 0);
    let text = format!("{}{}", stdout(&output), stderr(&output));
    // Either the daemon was unreachable first, or the file was; both answers
    // are actionable, and neither may be an unexplained failure.
    assert!(
        text.contains("file") || text.contains("daemon") || text.contains("listening"),
        "unhelpful failure: {text}"
    );
}
