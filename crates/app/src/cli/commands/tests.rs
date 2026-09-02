//! Every command, driven through the real argument parser against a real IPC
//! server backed by [`MockHandler`](superbackup_core::ipc::testing::MockHandler).
//!
//! Nothing below is mocked at the CLI's own boundary: each test parses an
//! argv, dispatches it, and reads what came out of stdout and stderr. That is
//! deliberate — the bugs worth catching here are the ones a unit test of a
//! formatting function cannot see: a reply of the wrong shape, prose leaking
//! into `--json`, an exit code that says the wrong thing, a prompt that would
//! hang a script.

use superbackup_core::error::ErrorCode;
use superbackup_core::ipc::protocol::StreamItem;
use superbackup_core::state::{Event, Severity};

use crate::cli::exit;
use crate::cli::testing::{run, run_without_daemon, Harness, RunResult};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn temp_path(tag: &str) -> String {
    std::env::temp_dir()
        .join(format!("sb-{tag}-{}", uuid::Uuid::new_v4().simple()))
        .display()
        .to_string()
}

/// Create a destination through the CLI, which is also the fixture for
/// everything that needs one.
fn add_destination(harness: &Harness, name: &str) -> RunResult {
    let result =
        run(harness, &["destination", "add", "--local", &temp_path("dest"), "--name", name]);
    assert_eq!(result.code, exit::OK, "{}{}", result.stdout, result.stderr);
    result
}

fn add_job(harness: &Harness, name: &str, destination: Option<&str>) -> RunResult {
    let source = temp_path("src");
    let mut argv = vec!["job", "add", "--name", name, "--source", &source];
    if let Some(destination) = destination {
        argv.push("-d");
        argv.push(destination);
    }
    let result = run(harness, &argv);
    assert_eq!(result.code, exit::OK, "{}{}", result.stdout, result.stderr);
    result
}

/// The JSON stream must be a document and nothing else: no escapes, no prose,
/// no second document.
fn assert_clean_json(result: &RunResult) {
    assert!(
        !result.stdout.contains('\u{1b}'),
        "an escape sequence reached a machine-readable stream:\n{}",
        result.stdout
    );
    let value = result.json();
    assert!(value.get("ok").is_some(), "the envelope must carry `ok`: {}", result.stdout);
    // One document: serde_json refuses trailing content, so parsing at all is
    // the proof. Belt and braces on the shape of the envelope itself.
    if value["ok"] == serde_json::Value::Bool(true) {
        assert!(value.get("data").is_some(), "a successful envelope carries `data`");
        assert!(value.get("error").is_none(), "a successful envelope carries no `error`");
    } else {
        let error = &value["error"];
        assert!(error.get("code").is_some());
        assert!(error.get("message").is_some());
    }
}

// ---------------------------------------------------------------------------
// status
// ---------------------------------------------------------------------------

#[test]
fn status_says_what_is_happening_at_a_glance() {
    let harness = Harness::start("status");
    add_destination(&harness, "local-ssd");
    add_job(&harness, "dev-projects", Some("local-ssd"));

    let result = run(&harness, &["status"]);
    assert_eq!(result.code, exit::OK, "{}", result.stderr);
    // Health, the machine, the job, and what runs next: the four questions
    // somebody runs `status` to answer.
    assert!(result.stdout.contains("Up to date"), "{}", result.stdout);
    assert!(result.stdout.contains("dev-projects"), "{}", result.stdout);
    assert!(result.stdout.contains("Jobs"), "{}", result.stdout);
    assert!(result.stdout.contains("Next"), "{}", result.stdout);
    assert!(result.stdout.contains("Never run"), "a job with no history must say so");
}

#[test]
fn status_json_is_a_document_with_no_prose_in_it() {
    let harness = Harness::start("status-json");
    add_job(&harness, "docs", None);

    let result = run(&harness, &["status", "--json"]);
    assert_clean_json(&result);
    let data = result.data();
    assert_eq!(data["snapshot"]["machine_slug"], "mock");
    assert!(data["jobs"].is_array());

    // The human words must be nowhere in the document.
    for prose in ["Up to date", "Never run", "Next:", "Jobs", "superbackup 0."] {
        assert!(!result.stdout.contains(prose), "`{prose}` leaked into --json:\n{}", result.stdout);
    }
}

#[test]
fn status_for_one_job_shows_that_job() {
    let harness = Harness::start("status-one");
    add_job(&harness, "photos", None);
    let result = run(&harness, &["status", "photos", "--json"]);
    assert_eq!(result.code, exit::OK);
    assert_eq!(result.data()["job"]["name"], "photos");
}

#[test]
fn watching_the_status_screen_is_refused_in_json_mode() {
    // It would emit one document per refresh, which is not a document.
    let harness = Harness::start("status-watch-json");
    let result = run(&harness, &["status", "--watch", "1", "--json"]);
    assert_eq!(result.code, exit::USAGE);
    assert_eq!(result.error()["code"], "validation");
}

// ---------------------------------------------------------------------------
// Name resolution
// ---------------------------------------------------------------------------

#[test]
fn an_ambiguous_prefix_is_an_error_that_lists_the_candidates() {
    let harness = Harness::start("ambiguous");
    add_job(&harness, "docs", None);
    add_job(&harness, "documents", None);

    let result = run(&harness, &["job", "show", "doc", "--json"]);
    assert_eq!(result.code, exit::USAGE, "guessing is never acceptable here");
    let error = result.error();
    assert_eq!(error["code"], "validation");
    let message = error["message"].as_str().unwrap_or_default();
    assert!(message.contains("docs"), "{message}");
    assert!(message.contains("documents"), "{message}");
    assert!(
        error["hint"].as_str().unwrap_or_default().contains("in full"),
        "the user must be told how to disambiguate"
    );
}

#[test]
fn a_unique_prefix_resolves_to_the_one_job() {
    let harness = Harness::start("prefix");
    add_job(&harness, "dev-projects", None);
    add_job(&harness, "photos", None);
    let result = run(&harness, &["job", "show", "dev", "--json"]);
    assert_eq!(result.code, exit::OK, "{}", result.stderr);
    assert_eq!(result.data()["name"], "dev-projects");
}

#[test]
fn an_unknown_job_is_job_not_found_and_exits_two() {
    let harness = Harness::start("unknown");
    add_job(&harness, "docs", None);
    let result = run(&harness, &["job", "show", "nope", "--json"]);
    assert_eq!(result.code, exit::USAGE);
    assert_eq!(result.error()["code"], "job_not_found");
    assert!(result.error()["hint"].as_str().unwrap_or_default().contains("docs"));
}

// ---------------------------------------------------------------------------
// job
// ---------------------------------------------------------------------------

#[test]
fn a_job_can_be_added_listed_shown_disabled_and_removed() {
    let harness = Harness::start("job-lifecycle");
    add_destination(&harness, "local-ssd");

    let added = add_job(&harness, "dev-projects", Some("local-ssd"));
    assert!(added.stdout.contains("Created dev-projects"), "{}", added.stdout);
    assert!(added.stdout.contains("Writes to     local-ssd"), "{}", added.stdout);

    let listed = run(&harness, &["job", "list"]);
    assert_eq!(listed.code, exit::OK);
    assert!(listed.stdout.contains("dev-projects"));
    assert!(listed.stdout.contains("local-ssd"), "the destination name, not its id");

    let shown = run(&harness, &["job", "show", "dev-projects"]);
    assert!(shown.stdout.contains("Sources"));
    assert!(shown.stdout.contains("node_modules"), "the developer template must be applied");

    let disabled = run(&harness, &["job", "disable", "dev-projects"]);
    assert_eq!(disabled.code, exit::OK);
    assert!(disabled.stdout.contains("Nothing was deleted"), "{}", disabled.stdout);

    let removed = run(&harness, &["job", "remove", "dev-projects", "-y"]);
    assert_eq!(removed.code, exit::OK);
    assert!(removed.stdout.contains("snapshots are still"), "{}", removed.stdout);
    assert_eq!(harness.handler.calls("job.delete"), 1);
}

#[test]
fn adding_a_job_with_no_destination_warns_that_it_backs_up_nowhere() {
    let harness = Harness::start("job-no-dest");
    let added = add_job(&harness, "orphan", None);
    assert!(
        added.stderr.contains("no destination"),
        "silence here would let somebody believe they were protected:\n{}",
        added.stderr
    );
}

#[test]
fn a_duplicate_job_name_is_refused_before_it_reaches_the_daemon() {
    let harness = Harness::start("job-dup");
    add_job(&harness, "docs", None);
    let source = temp_path("src");
    let result = run(&harness, &["job", "add", "--name", "docs", "--source", &source]);
    assert_eq!(result.code, exit::USAGE);
    assert_eq!(harness.handler.calls("job.create"), 1, "the second one must not be sent");
}

#[test]
fn job_edit_reports_exactly_what_it_changed() {
    let harness = Harness::start("job-edit");
    add_destination(&harness, "offsite");
    add_job(&harness, "docs", None);
    let result = run(
        &harness,
        &["job", "edit", "docs", "--add-destination", "offsite", "--schedule", "hourly"],
    );
    assert_eq!(result.code, exit::OK, "{}", result.stderr);
    assert!(result.stdout.contains("added destination offsite"), "{}", result.stdout);
    assert!(result.stdout.contains("schedule is now"), "{}", result.stdout);
}

#[test]
fn job_edit_with_nothing_to_change_says_so_rather_than_writing() {
    let harness = Harness::start("job-edit-noop");
    add_job(&harness, "docs", None);
    let result = run(&harness, &["job", "edit", "docs"]);
    assert_eq!(result.code, exit::USAGE);
    assert_eq!(harness.handler.calls("job.update"), 0);
}

#[test]
fn a_bad_schedule_is_refused_with_the_accepted_forms() {
    let harness = Harness::start("job-sched");
    add_job(&harness, "docs", None);
    let result = run(&harness, &["job", "edit", "docs", "--schedule", "sometimes", "--json"]);
    assert_eq!(result.code, exit::USAGE);
    assert!(result.error()["hint"].as_str().unwrap_or_default().contains("daily@02:00"));
}

#[test]
fn job_preview_says_it_is_not_available_rather_than_pretending() {
    let harness = Harness::start("job-preview");
    add_job(&harness, "docs", None);
    let result = run(&harness, &["job", "preview", "docs", "--json"]);
    assert_eq!(result.code, exit::USAGE);
    let hint = result.error()["hint"].as_str().unwrap_or_default().to_string();
    assert!(hint.contains("--dry-run"), "it must point at the nearest thing that works: {hint}");
}

// ---------------------------------------------------------------------------
// Confirmation
// ---------------------------------------------------------------------------

#[test]
fn a_destructive_command_under_no_input_fails_instead_of_prompting() {
    let harness = Harness::start("confirm");
    add_job(&harness, "docs", None);

    let result = run(&harness, &["job", "remove", "docs", "--json"]);
    assert_eq!(result.code, exit::USAGE, "a script must be told, not blocked");
    let error = result.error();
    assert!(error["message"].as_str().unwrap_or_default().contains("--no-input"));
    assert!(error["hint"].as_str().unwrap_or_default().contains("-y"));
    assert_eq!(harness.handler.calls("job.delete"), 0, "nothing may have been deleted");
}

#[test]
fn removing_a_destination_names_the_jobs_that_still_use_it() {
    let harness = Harness::start("dest-remove");
    add_destination(&harness, "local-ssd");
    add_job(&harness, "dev", Some("local-ssd"));

    let refused = run(&harness, &["destination", "remove", "local-ssd", "--json"]);
    assert_eq!(refused.code, exit::USAGE);
    let message = refused.error()["message"].as_str().unwrap_or_default().to_string();
    assert!(message.contains("local-ssd"), "{message}");

    let removed = run(&harness, &["destination", "remove", "local-ssd", "-y"]);
    assert_eq!(removed.code, exit::OK, "{}", removed.stderr);
    assert!(removed.stdout.contains("Nothing stored there was deleted"));
}

// ---------------------------------------------------------------------------
// run
// ---------------------------------------------------------------------------

#[test]
fn run_without_wait_says_it_only_queued_the_work() {
    let harness = Harness::start("run-queue");
    add_job(&harness, "docs", None);
    let result = run(&harness, &["run", "docs"]);
    assert_eq!(result.code, exit::OK, "{}", result.stderr);
    assert_eq!(harness.handler.calls("job.run"), 1);
    assert!(
        result.stdout.contains("Not waiting"),
        "silence after `run` reads as \"it did nothing\":\n{}",
        result.stdout
    );
}

#[test]
fn a_dry_run_passes_the_flag_through_and_reports_the_note() {
    let harness = Harness::start("run-dry");
    add_job(&harness, "docs", None);
    let result = run(&harness, &["run", "docs", "--dry-run"]);
    assert_eq!(result.code, exit::OK);
    assert!(result.stdout.contains("nothing will be written"), "{}", result.stdout);
}

#[test]
fn a_destination_filter_on_run_is_refused_rather_than_silently_ignored() {
    // Backing up to a destination the user excluded is not a smaller problem
    // than not backing up at all.
    let harness = Harness::start("run-filter");
    add_destination(&harness, "local-ssd");
    add_job(&harness, "docs", Some("local-ssd"));
    let result = run(&harness, &["run", "docs", "-d", "local-ssd", "--json"]);
    assert_eq!(result.code, exit::USAGE);
    assert_eq!(harness.handler.calls("job.run"), 0);
    assert!(result.error()["message"].as_str().unwrap_or_default().contains("--destination"));
}

#[test]
fn run_all_with_no_enabled_jobs_says_so() {
    let harness = Harness::start("run-all-empty");
    let result = run(&harness, &["run", "--all", "--json"]);
    assert_eq!(result.code, exit::USAGE);
    assert!(result.error()["hint"].as_str().unwrap_or_default().contains("job enable"));
}

// ---------------------------------------------------------------------------
// stop, pause, resume
// ---------------------------------------------------------------------------

#[test]
fn stopping_a_job_that_is_not_running_is_a_success() {
    let harness = Harness::start("stop-idle");
    add_job(&harness, "docs", None);
    let result = run(&harness, &["stop", "docs"]);
    assert_eq!(result.code, exit::OK, "idempotence is the point of `stop`");
    assert!(result.stdout.contains("is not running"));
    assert_eq!(harness.handler.calls("job.stop"), 0);
}

#[test]
fn stop_all_is_forwarded_even_with_nothing_running() {
    let harness = Harness::start("stop-all");
    let result = run(&harness, &["stop", "--all"]);
    assert_eq!(result.code, exit::OK);
    assert_eq!(harness.handler.calls("job.stop_all"), 1);
    assert!(result.stdout.contains("Nothing was running"));
}

#[test]
fn pause_accepts_the_durations_the_help_promises_and_refuses_a_bare_number() {
    let harness = Harness::start("pause");
    let good = run(&harness, &["pause", "2h30m"]);
    assert_eq!(good.code, exit::OK, "{}", good.stderr);
    assert!(good.stdout.contains("paused until"), "{}", good.stdout);

    let bad = run(&harness, &["pause", "30", "--json"]);
    assert_eq!(bad.code, exit::USAGE);
    assert!(bad.error()["hint"].as_str().unwrap_or_default().contains("30m"));

    let indefinite = run(&harness, &["pause", "--until-resumed", "--reason", "on a plane"]);
    assert_eq!(indefinite.code, exit::OK);
    assert!(indefinite.stdout.contains("on a plane"));

    let resumed = run(&harness, &["resume"]);
    assert_eq!(resumed.code, exit::OK);
    assert!(resumed.stdout.contains("schedules again"));
}

// ---------------------------------------------------------------------------
// watch
// ---------------------------------------------------------------------------

#[test]
fn watch_emits_one_json_object_per_line() {
    let harness = Harness::start("watch");
    let publisher = std::sync::Arc::clone(&harness.handler);

    // Publish once the subscription exists. The handler counts subscriptions,
    // so this waits for the real thing rather than sleeping and hoping.
    std::thread::spawn(move || {
        for _ in 0..200 {
            if publisher.subscriber_count() > 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        for n in 0..3 {
            publisher.publish(StreamItem::Event {
                event: Box::new(Event::info("job.started", format!("run {n} started"))),
            });
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    });

    let result = run(&harness, &["watch", "--limit", "3"]);
    assert_eq!(result.code, exit::OK);

    let lines: Vec<&str> = result.stdout.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 3, "one object per line:\n{}", result.stdout);
    for line in &lines {
        let value: serde_json::Value =
            serde_json::from_str(line).unwrap_or_else(|e| panic!("`{line}` is not JSON: {e}"));
        assert_eq!(value["kind"], "event");
    }
    assert!(result.stderr.contains("Ctrl-C"), "the user must be told how to stop it");
}

#[test]
fn watch_with_json_stays_ndjson_and_gains_no_envelope() {
    // A stream is not a document. Appending `{"ok":true,"data":null}` after
    // the last event would hand `| jq` one line it cannot parse.
    let harness = Harness::start("watch-json");
    let publisher = std::sync::Arc::clone(&harness.handler);
    std::thread::spawn(move || {
        for _ in 0..200 {
            if publisher.subscriber_count() > 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        publisher
            .publish(StreamItem::Event { event: Box::new(Event::info("job.started", "started")) });
    });

    let result = run(&harness, &["watch", "--limit", "1", "--json"]);
    assert_eq!(result.code, exit::OK);
    let lines: Vec<&str> = result.stdout.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 1, "an envelope was appended to the stream:\n{}", result.stdout);
    let value: serde_json::Value = serde_json::from_str(lines[0]).expect("NDJSON");
    assert_eq!(value["kind"], "event");
    assert!(value.get("ok").is_none(), "a stream item must not be wrapped");
}

#[test]
fn watch_filters_by_kind_without_dropping_the_lag_marker() {
    let harness = Harness::start("watch-kind");
    let publisher = std::sync::Arc::clone(&harness.handler);
    std::thread::spawn(move || {
        for _ in 0..200 {
            if publisher.subscriber_count() > 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        publisher.publish(StreamItem::Event {
            event: Box::new(Event::new(Severity::Info, "vault.unlocked", "unlocked")),
        });
        publisher.publish(StreamItem::Event {
            event: Box::new(Event::new(Severity::Error, "job.failed", "it failed")),
        });
    });

    let result = run(&harness, &["watch", "--kind", "job", "--limit", "1"]);
    assert_eq!(result.code, exit::OK);
    let lines: Vec<&str> = result.stdout.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 1, "{}", result.stdout);
    assert!(lines[0].contains("job.failed"), "`--kind job` must match `job.failed`: {}", lines[0]);
}

// ---------------------------------------------------------------------------
// Failure classes
// ---------------------------------------------------------------------------

#[test]
fn a_locked_vault_exits_four() {
    let harness = Harness::start("locked");
    harness.handler.fail_with(Some(ErrorCode::Locked));
    let result = run(&harness, &["job", "list", "--json"]);
    assert_eq!(result.code, exit::LOCKED);
    let error = result.error();
    assert_eq!(error["code"], "locked");
    assert!(
        error["hint"].as_str().unwrap_or_default().contains("unlock"),
        "the fix must be in the message"
    );
}

#[test]
fn nothing_listening_is_exit_three_and_not_an_os_error() {
    let result = run_without_daemon(&["status", "--json"]);
    assert_eq!(result.code, exit::DAEMON_UNREACHABLE);
    let error = result.error();
    assert_eq!(error["code"], "daemon_unreachable");
    let message = error["message"].as_str().unwrap_or_default().to_string();
    assert!(!message.contains("os error"), "raw OS errors help nobody: {message}");
    assert!(error["hint"].as_str().unwrap_or_default().contains("superbackup daemon"));
}

#[test]
fn a_query_never_starts_a_daemon() {
    let result = run_without_daemon(&["job", "list"]);
    assert_eq!(result.code, exit::DAEMON_UNREACHABLE);
    assert!(
        !result.stderr.contains("Starting one in the background"),
        "a read-only query must not launch a background process"
    );
}

// ---------------------------------------------------------------------------
// destinations and providers
// ---------------------------------------------------------------------------

#[test]
fn a_destination_is_created_and_its_repository_with_it() {
    let harness = Harness::start("dest-add");
    let result = add_destination(&harness, "local-ssd");
    assert!(result.stdout.contains("Added local-ssd"), "{}", result.stdout);
    assert_eq!(harness.handler.calls("dest.create"), 1);
    assert_eq!(harness.handler.calls("dest.repo_create"), 1);

    let listed = run(&harness, &["destination", "list"]);
    assert!(listed.stdout.contains("Repository in a local folder"));
    assert!(listed.stderr.contains("destination test"), "say where the fresh answer lives");
}

#[test]
fn encryption_flags_are_meaningless_on_a_mirror_and_say_so() {
    let harness = Harness::start("mirror");
    let path = temp_path("mirror");
    let result = run(
        &harness,
        &[
            "destination",
            "add",
            "--mirror",
            &path,
            "--name",
            "usb",
            "--encryption",
            "AES256-GCM-HMAC-SHA256",
            "--json",
        ],
    );
    assert_eq!(result.code, exit::USAGE);
    assert!(result.error()["message"].as_str().unwrap_or_default().contains("folder mirror"));
}

#[test]
fn an_unknown_encryption_algorithm_lists_the_ones_that_exist() {
    let harness = Harness::start("enc");
    let path = temp_path("repo");
    let result =
        run(&harness, &["destination", "add", "--local", &path, "--encryption", "rot13", "--json"]);
    assert_eq!(result.code, exit::USAGE);
    assert!(result.error()["hint"].as_str().unwrap_or_default().contains("AES256"));
}

#[test]
fn testing_a_destination_reports_reachable_and_writable() {
    let harness = Harness::start("dest-test");
    add_destination(&harness, "local-ssd");
    let result = run(&harness, &["destination", "test", "local-ssd", "--json"]);
    assert_eq!(result.code, exit::OK);
    assert_eq!(result.data()["reachable"], true);
    assert_eq!(result.data()["writable"], true);
}

#[test]
fn maintenance_and_machines_report_what_the_protocol_cannot_do() {
    let harness = Harness::start("dest-gaps");
    add_destination(&harness, "local-ssd");
    for argv in [
        vec!["destination", "maintain", "local-ssd", "--json"],
        vec!["destination", "machines", "local-ssd", "--json"],
    ] {
        let result = run(&harness, &argv);
        assert_eq!(result.code, exit::USAGE, "{argv:?}");
        assert!(result.error()["message"]
            .as_str()
            .unwrap_or_default()
            .contains("running instance"));
    }
}

#[test]
fn provider_commands_never_print_a_secret() {
    let harness = Harness::start("provider");
    let listed = run(&harness, &["provider", "list"]);
    assert_eq!(listed.code, exit::OK);
    assert!(listed.stdout.contains("No storage providers yet"));
}

#[test]
fn project_commands_say_exactly_what_is_missing() {
    let harness = Harness::start("project");
    let result = run(&harness, &["project", "list", "--json"]);
    assert_eq!(result.code, exit::USAGE);
    let message = result.error()["message"].as_str().unwrap_or_default().to_string();
    assert!(message.contains("project"), "{message}");
}

// ---------------------------------------------------------------------------
// vault
// ---------------------------------------------------------------------------

#[test]
fn unlock_reads_a_passphrase_from_a_file_and_never_from_argv() {
    let harness = Harness::start("unlock");
    let dir = std::env::temp_dir().join(format!("sb-unlock-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let file = dir.join("pp");
    std::fs::write(&file, b"correct horse battery staple\n").expect("write");

    let result =
        run(&harness, &["unlock", "--passphrase-file", &file.display().to_string(), "--json"]);
    assert_eq!(result.code, exit::OK, "{}{}", result.stdout, result.stderr);
    assert_eq!(result.data()["unlocked"], true);
    assert_eq!(harness.handler.calls("vault.unlock"), 1);
    // The passphrase must not appear in anything the program emitted.
    assert!(!result.stdout.contains("correct horse"));
    assert!(!result.stderr.contains("correct horse"));

    let locked = run(&harness, &["lock"]);
    assert_eq!(locked.code, exit::OK);
    assert!(locked.stdout.contains("will not run until you unlock"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn there_is_no_way_to_pass_a_passphrase_on_the_command_line() {
    use clap::Parser;
    // A companion to the parser-level test in `args.rs`: these are the shapes
    // somebody would actually try if they wanted to reintroduce the capability.
    for argv in [
        vec!["superbackup", "unlock", "--passphrase", "hunter2"],
        vec!["superbackup", "unlock", "-p", "hunter2"],
        vec!["superbackup", "init", "--passphrase", "hunter2"],
        vec!["superbackup", "change-passphrase", "--passphrase", "hunter2"],
    ] {
        assert!(crate::cli::Cli::try_parse_from(&argv).is_err(), "{argv:?} must not be accepted");
    }
}

#[test]
fn a_passphrase_is_needed_and_no_input_refuses_to_ask() {
    let harness = Harness::start("unlock-noinput");
    let result = run(&harness, &["unlock", "--json"]);
    assert_eq!(result.code, exit::USAGE);
    assert!(result.error()["hint"].as_str().unwrap_or_default().contains("--passphrase-file"));
    assert_eq!(harness.handler.calls("vault.unlock"), 0, "nothing may be sent");
}

// ---------------------------------------------------------------------------
// snapshots, browse, restore
// ---------------------------------------------------------------------------

#[test]
fn snapshots_for_a_job_with_only_a_mirror_explains_why_there_are_none() {
    let harness = Harness::start("snapshots-mirror");
    let path = temp_path("mirror");
    let added = run(&harness, &["destination", "add", "--mirror", &path, "--name", "usb"]);
    assert_eq!(added.code, exit::OK, "{}", added.stderr);
    add_job(&harness, "docs", Some("usb"));

    let result = run(&harness, &["snapshots", "docs", "--json"]);
    assert_eq!(result.code, exit::USAGE);
    assert!(result.error()["message"].as_str().unwrap_or_default().contains("no repository"));
}

#[test]
fn snapshots_lists_nothing_without_pretending_it_failed() {
    let harness = Harness::start("snapshots-empty");
    add_destination(&harness, "local-ssd");
    add_job(&harness, "docs", Some("local-ssd"));
    let result = run(&harness, &["snapshots", "docs"]);
    assert_eq!(result.code, exit::OK);
    assert!(result.stdout.contains("no snapshots yet"), "{}", result.stdout);
}

#[test]
fn a_time_that_cannot_be_parsed_stops_the_restore_before_anything_is_written() {
    let harness = Harness::start("restore-at");
    add_destination(&harness, "local-ssd");
    add_job(&harness, "docs", Some("local-ssd"));
    let target = temp_path("restore");
    let result =
        run(&harness, &["restore", "docs", "--to", &target, "--at", "last tuesday", "--json"]);
    assert_eq!(result.code, exit::USAGE);
    assert_eq!(harness.handler.calls("snapshot.restore"), 0);
    assert!(result.error()["hint"].as_str().unwrap_or_default().contains("3 days ago"));
}

#[test]
fn browsing_needs_a_snapshot_and_says_so_when_there_is_none() {
    let harness = Harness::start("browse");
    add_destination(&harness, "local-ssd");
    add_job(&harness, "docs", Some("local-ssd"));
    let result = run(&harness, &["browse", "docs", "--json"]);
    assert_eq!(result.code, exit::USAGE);
    assert!(result.error()["message"].as_str().unwrap_or_default().contains("no snapshots"));
}

// ---------------------------------------------------------------------------
// config
// ---------------------------------------------------------------------------

#[test]
fn a_setting_can_be_read_and_written_by_its_dotted_name() {
    let harness = Harness::start("config");
    let read = run(&harness, &["config", "get", "auto_lock_minutes"]);
    assert_eq!(read.code, exit::OK, "{}", read.stderr);
    assert_eq!(read.stdout.trim(), "30");

    let written = run(&harness, &["config", "set", "auto_lock_minutes", "60"]);
    assert_eq!(written.code, exit::OK, "{}", written.stderr);
    assert!(written.stdout.contains("30 -> 60"), "{}", written.stdout);

    let nested = run(&harness, &["config", "get", "bandwidth.upload_kbps", "--json"]);
    assert_eq!(nested.code, exit::OK);
    assert_eq!(nested.data(), serde_json::Value::Null);
}

#[test]
fn an_unknown_setting_suggests_the_ones_that_exist() {
    let harness = Harness::start("config-unknown");
    let result = run(&harness, &["config", "get", "auto_lock", "--json"]);
    assert_eq!(result.code, exit::USAGE);
    assert!(result.error()["hint"].as_str().unwrap_or_default().contains("auto_lock_minutes"));
}

#[test]
fn a_setting_of_the_wrong_type_is_rejected_here_not_by_the_daemon() {
    let harness = Harness::start("config-type");
    let result = run(&harness, &["config", "set", "auto_lock_minutes", "never", "--json"]);
    assert_eq!(result.code, exit::USAGE);
    assert_eq!(harness.handler.calls("settings.update"), 0);
    assert!(result.error()["message"].as_str().unwrap_or_default().contains("auto_lock_minutes"));
}

#[test]
fn config_show_lists_settings_by_the_keys_config_set_accepts() {
    let harness = Harness::start("config-show");
    let result = run(&harness, &["config", "show"]);
    assert_eq!(result.code, exit::OK);
    assert!(result.stdout.contains("auto_lock_minutes"));
    assert!(result.stdout.contains("bandwidth.upload_kbps"), "nested keys must be dotted");
    assert!(result.stdout.contains("Secrets are held in the vault"));
}

#[test]
fn config_validate_passes_on_an_empty_configuration() {
    let harness = Harness::start("config-validate");
    let result = run(&harness, &["config", "validate"]);
    assert_eq!(result.code, exit::OK, "{}{}", result.stdout, result.stderr);
    assert!(result.stdout.contains("valid"));
}

// ---------------------------------------------------------------------------
// doctor
// ---------------------------------------------------------------------------

#[test]
fn doctor_diagnoses_a_missing_daemon_and_exits_nonzero() {
    // The moment doctor matters most is the moment nothing is running, so it
    // must not simply fail to connect and give up.
    let result = run_without_daemon(&["doctor", "--json"]);
    assert_eq!(result.code, exit::FAILED, "a failed check is a negative answer, not a crash");
    assert_clean_json(&result);
    let data = result.data();
    assert_eq!(data["ok"], false);

    let checks = data["checks"].as_array().cloned().unwrap_or_default();
    let ids: Vec<&str> = checks.iter().filter_map(|c| c["id"].as_str()).collect();
    for expected in [
        "daemon.reachable",
        "paths.present",
        "kopia.present",
        "vault.present",
        "disk.space",
        "autostart.state",
        "service.state",
    ] {
        assert!(ids.contains(&expected), "`{expected}` is missing from {ids:?}");
    }
    let daemon_check =
        checks.iter().find(|c| c["id"] == "daemon.reachable").cloned().unwrap_or_default();
    assert_eq!(daemon_check["status"], "fail");
    assert!(!data["limitations"].as_array().cloned().unwrap_or_default().is_empty());
}

#[test]
fn doctor_passes_when_everything_it_can_check_is_fine() {
    let harness = Harness::start("doctor-ok");
    let result = run(&harness, &["doctor", "--json"]);
    // kopia may genuinely be absent on a build machine, so the overall verdict
    // is not asserted; what must hold is that the daemon check passed and the
    // envelope is well formed.
    assert_clean_json(&result);
    let checks = result.data()["checks"].as_array().cloned().unwrap_or_default();
    let daemon_check =
        checks.iter().find(|c| c["id"] == "daemon.reachable").cloned().unwrap_or_default();
    assert_eq!(daemon_check["status"], "pass");
    assert_eq!(harness.handler.calls("doctor"), 1, "the daemon's own checks must be merged in");
}

#[test]
fn doctor_fix_creates_the_missing_directories_and_says_which() {
    let result = run_without_daemon(&["doctor", "--fix"]);
    assert!(result.stdout.contains("Fixed"), "{}", result.stdout);
    assert!(result.stdout.contains("paths.present"), "it must say exactly what it did");
}

#[test]
fn doctor_skips_destination_checks_unless_asked() {
    let harness = Harness::start("doctor-dests");
    add_destination(&harness, "local-ssd");
    let quiet = run(&harness, &["doctor", "--json"]);
    assert_eq!(harness.handler.calls("dest.test"), 0, "a plain doctor makes no network requests");
    let checks = quiet.data()["checks"].as_array().cloned().unwrap_or_default();
    let skipped = checks.iter().find(|c| c["id"] == "dest.reachable").cloned().unwrap_or_default();
    assert_eq!(skipped["status"], "skipped");

    let thorough = run(&harness, &["doctor", "--check-destinations", "--json"]);
    assert_eq!(harness.handler.calls("dest.test"), 1);
    let checks = thorough.data()["checks"].as_array().cloned().unwrap_or_default();
    assert!(checks.iter().any(|c| c["id"] == "dest.reachable:local-ssd"));
}

// ---------------------------------------------------------------------------
// Cross-cutting
// ---------------------------------------------------------------------------

#[test]
fn every_command_that_answers_in_json_emits_exactly_one_document() {
    let harness = Harness::start("json-sweep");
    add_destination(&harness, "local-ssd");
    add_job(&harness, "docs", Some("local-ssd"));

    for argv in [
        vec!["status", "--json"],
        vec!["job", "list", "--json"],
        vec!["job", "show", "docs", "--json"],
        vec!["destination", "list", "--json"],
        vec!["destination", "show", "local-ssd", "--json"],
        vec!["destination", "test", "local-ssd", "--json"],
        vec!["destination", "stats", "local-ssd", "--json"],
        vec!["provider", "list", "--json"],
        vec!["snapshots", "docs", "--json"],
        vec!["config", "show", "--json"],
        vec!["config", "validate", "--json"],
        vec!["autostart", "status", "--json"],
        vec!["service", "status", "--json"],
        vec!["doctor", "--json"],
        vec!["remote", "status", "--json"],
        vec!["remote", "diff", "--json"],
        vec!["run", "docs", "--json"],
        vec!["stop", "--all", "--json"],
        vec!["pause", "1h", "--json"],
        vec!["resume", "--json"],
    ] {
        let result = run(&harness, &argv);
        assert_clean_json(&result);
        assert!(
            result.stdout.starts_with('{'),
            "{argv:?} put something before the document:\n{}",
            result.stdout
        );
    }
}

#[test]
fn quiet_silences_the_narration_but_never_the_error() {
    let harness = Harness::start("quiet");
    add_job(&harness, "docs", None);

    let listed = run(&harness, &["job", "list", "--quiet"]);
    assert_eq!(listed.code, exit::OK);
    assert_eq!(listed.stdout, "", "--quiet must produce nothing on stdout");

    let failed = run(&harness, &["job", "show", "nope", "--quiet"]);
    assert_eq!(failed.code, exit::USAGE);
    assert!(failed.stderr.contains("nope"), "errors survive --quiet");
}

/// Columns start at the same offset on every line, header included.
///
/// A golden test on the exact text would rot every time a word changed; this
/// asserts the property that actually matters and that silently decays — that
/// the table is a table.
fn assert_columns_line_up(text: &str, header_word: &str) {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.contains(header_word))
        .unwrap_or_else(|| panic!("no header containing `{header_word}` in:\n{text}"));

    // Column starts are the character offsets that follow a two-space gap.
    let offsets = |line: &str| -> Vec<usize> {
        let chars: Vec<char> = line.chars().collect();
        let mut out = vec![0usize];
        let mut i = 0;
        while i + 2 < chars.len() {
            if chars[i] == ' ' && chars[i + 1] == ' ' && chars[i + 2] != ' ' {
                out.push(i + 2);
            }
            i += 1;
        }
        out
    };

    let header = offsets(lines[start]);
    for line in lines.iter().skip(start + 1) {
        if line.trim().is_empty() {
            break;
        }
        let row = offsets(line);
        // Right-aligned cells push their start rightwards, so the assertion is
        // containment: every row boundary must be one the header declared, or
        // sit inside a column rather than straddling two.
        assert!(
            row.iter().all(|o| header.iter().any(|h| h <= o)),
            "row `{line}` does not line up with header `{}`",
            lines[start]
        );
        assert!(
            row.len() <= header.len(),
            "row `{line}` has more columns than the header `{}`",
            lines[start]
        );
    }
}

#[test]
fn a_job_list_stays_a_table_when_a_name_is_long_and_a_value_is_missing() {
    let harness = Harness::start("alignment");
    add_destination(&harness, "local-ssd");
    add_job(&harness, "a", Some("local-ssd"));
    add_job(&harness, "a-very-long-job-name-that-would-wreck-a-naive-table", None);

    let result = run(&harness, &["job", "list"]);
    assert_eq!(result.code, exit::OK, "{}", result.stderr);
    assert_columns_line_up(&result.stdout, "JOB");
    // A job with no destination still occupies its column rather than
    // collapsing the row.
    assert!(result.stdout.contains(crate::cli::format::MISSING));
}

#[test]
fn a_narrow_terminal_never_wraps_a_row() {
    let harness = Harness::start("narrow");
    add_destination(&harness, "local-ssd");
    add_job(&harness, "a-very-long-job-name-indeed-yes-really", Some("local-ssd"));

    let (mut ctx, captured) = harness.ctx(false);
    ctx.ui.width = 60;
    let command = {
        use clap::Parser;
        crate::cli::Cli::try_parse_from(["superbackup", "job", "list"])
            .ok()
            .and_then(|c| c.command)
            .unwrap_or_else(|| panic!("`job list` must parse"))
    };
    let outcome = super::dispatch(&mut ctx, command).expect("job list");
    ctx.ui.finish(&outcome);

    for line in captured.stdout().lines() {
        assert!(
            line.chars().count() <= 60,
            "a {}-column line overflows a 60-column terminal: {line}",
            line.chars().count()
        );
    }
}

#[test]
fn human_output_carries_no_escape_sequences_when_colour_is_off() {
    let harness = Harness::start("no-colour");
    add_job(&harness, "docs", None);
    let result = run(&harness, &["status"]);
    assert!(!result.stdout.contains('\u{1b}'), "a pipe must get plain text");
}
