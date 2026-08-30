//! `doctor`: find out what is actually wrong.
//!
//! The important design point is that this must work **when the daemon is
//! not running**, because that is one of the things most likely to be wrong.
//! So the checks that can be answered from this process are answered here —
//! kopia, the directories, disk space, autostart, the service — and the
//! daemon's own diagnostics are merged in when there is a daemon to ask.
//!
//! `--fix` repairs only what is unambiguous and reversible, and prints exactly
//! what it changed. It never creates a repository, never touches the vault,
//! and never installs anything the user did not ask for beyond the kopia the
//! daemon manages on its own.

use superbackup_core::error::ErrorCode;
use superbackup_core::ipc::protocol::{CheckStatus, DoctorCheck, Request};
use superbackup_core::model::Settings;
use superbackup_core::platform;

use crate::cli::args::DoctorArgs;
use crate::cli::client::{reply, Daemon, Start};
use crate::cli::context::Ctx;
use crate::cli::format::{self, Cell, Colour, Column, Table};
use crate::cli::output::{CliResult, Outcome};

/// Below this, a backup that would have worked yesterday starts failing
/// halfway through and leaves a partial snapshot behind.
const LOW_DISK_BYTES: u64 = 5 * 1024 * 1024 * 1024;

fn check(id: &str, title: &str, status: CheckStatus) -> DoctorCheck {
    DoctorCheck {
        id: id.to_string(),
        title: title.to_string(),
        status,
        detail: None,
        hint: None,
        fixable: false,
    }
}

fn with_detail(mut c: DoctorCheck, detail: impl Into<String>) -> DoctorCheck {
    c.detail = Some(detail.into());
    c
}

fn with_hint(mut c: DoctorCheck, hint: impl Into<String>) -> DoctorCheck {
    c.hint = Some(hint.into());
    c
}

fn fixable(mut c: DoctorCheck) -> DoctorCheck {
    c.fixable = true;
    c
}

pub fn doctor(ctx: &mut Ctx, args: DoctorArgs) -> CliResult<Outcome> {
    let mut checks: Vec<DoctorCheck> = Vec::new();
    let mut fixed: Vec<String> = Vec::new();

    // Connect first, but treat failure as a finding rather than as an error:
    // "the daemon is not running" is a diagnosis, and refusing to run the rest
    // of the checks because of it would withhold the diagnosis.
    let daemon = match Daemon::connect(ctx, Start::Never) {
        Ok(daemon) => {
            checks.push(check("daemon.reachable", "superbackup is running", CheckStatus::Pass));
            Some(daemon)
        }
        Err(e) if e.code == ErrorCode::DaemonUnreachable => {
            checks.push(with_hint(
                check("daemon.reachable", "superbackup is running", CheckStatus::Fail),
                "Start it with `superbackup daemon`, or install the service with \
                 `superbackup service install`.",
            ));
            None
        }
        Err(e) => {
            checks.push(with_detail(
                check("daemon.reachable", "superbackup is running", CheckStatus::Fail),
                e.message.clone(),
            ));
            None
        }
    };

    let settings = match &daemon {
        Some(daemon) => reply!(daemon, Request::SettingsGet {}, Settings)
            .map(|s| *s.settings)
            .unwrap_or_default(),
        None => Settings::default(),
    };

    checks.extend(directory_checks(ctx, args.fix, &mut fixed));
    checks.push(kopia_check(ctx, &settings));
    checks.extend(vault_checks(ctx, daemon.as_ref()));
    checks.push(disk_check(ctx));
    checks.extend(autostart_checks(args.fix, &mut fixed));
    checks.push(service_check());

    if let Some(daemon) = &daemon {
        checks.push(config_check(daemon));
        if args.check_destinations {
            checks.extend(destination_checks(ctx, daemon));
        } else {
            checks.push(with_hint(
                check(
                    "dest.reachable",
                    "destinations are reachable",
                    CheckStatus::Skipped,
                ),
                "Pass --check-destinations to try each one. It makes network requests.",
            ));
        }

        // The daemon knows things this process cannot see: repository
        // connection state, the engine, its own history.
        match reply!(daemon, Request::Doctor { fix: args.fix }, Doctor) {
            Ok(remote) => {
                fixed.extend(remote.fixed.iter().cloned());
                for remote_check in remote.checks {
                    // The local answer wins for anything both can see: this
                    // process is the one the user is actually running.
                    if !checks.iter().any(|c| c.id == remote_check.id) {
                        checks.push(remote_check);
                    }
                }
            }
            Err(e) => checks.push(with_detail(
                check("daemon.doctor", "the running instance self-checks", CheckStatus::Warn),
                e.message,
            )),
        }
    } else if args.check_destinations {
        checks.push(with_hint(
            check("dest.reachable", "destinations are reachable", CheckStatus::Skipped),
            "Only the running instance can reach a destination; start it and try again.",
        ));
    }

    render(ctx, &checks, &fixed, args.fix);

    let failed = checks.iter().any(|c| c.status == CheckStatus::Fail);
    let value = serde_json::json!({
        "ok": !failed,
        "checks": checks,
        "fixed": fixed,
        "limitations": platform::limitations(),
    });
    if failed {
        Outcome::negative(value)
    } else {
        Outcome::data(value)
    }
}

// ---------------------------------------------------------------------------
// Individual checks
// ---------------------------------------------------------------------------

fn directory_checks(ctx: &mut Ctx, fix: bool, fixed: &mut Vec<String>) -> Vec<DoctorCheck> {
    let missing: Vec<String> = [
        (&ctx.paths.config_dir, "configuration"),
        (&ctx.paths.data_dir, "data"),
        (&ctx.paths.log_dir, "logs"),
        (&ctx.paths.cache_dir, "cache"),
    ]
    .iter()
    .filter(|(dir, _)| !dir.is_dir())
    .map(|(dir, what)| format!("{what} ({})", dir.display()))
    .collect();

    if missing.is_empty() {
        return vec![with_detail(
            check("paths.present", "the directories exist", CheckStatus::Pass),
            ctx.paths.config_dir.display().to_string(),
        )];
    }

    let mut c = fixable(with_detail(
        check("paths.present", "the directories exist", CheckStatus::Fail),
        format!("missing: {}", missing.join(", ")),
    ));
    if fix {
        // Creating a directory is unambiguous and reversible; this is the
        // safest possible repair.
        match ctx.paths.ensure() {
            Ok(()) => {
                fixed.push("paths.present".to_string());
                c = with_detail(
                    check("paths.present", "the directories exist", CheckStatus::Pass),
                    format!("created: {}", missing.join(", ")),
                );
            }
            Err(e) => c = with_detail(c, format!("could not create them: {e}")),
        }
    } else {
        c = with_hint(c, "Run `superbackup doctor --fix` to create them.");
    }
    vec![c]
}

fn kopia_check(ctx: &mut Ctx, settings: &Settings) -> DoctorCheck {
    // `discover` is async and does not need the daemon, so it gets its own
    // throwaway runtime rather than a connection.
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(e) => {
            return with_detail(
                check("kopia.present", "kopia is installed", CheckStatus::Warn),
                format!("could not be checked: {e}"),
            )
        }
    };
    let found = runtime.block_on(superbackup_core::kopia::KopiaBinary::discover(
        settings,
        &ctx.paths,
    ));
    match found {
        Ok(binary) => with_detail(
            check("kopia.present", "kopia is installed", CheckStatus::Pass),
            format!("{} at {} ({})", binary.version(), binary.path().display(), binary.source().title()),
        ),
        Err(e) => with_hint(
            fixable(with_detail(
                check("kopia.present", "kopia is installed", CheckStatus::Fail),
                e.to_string(),
            )),
            "Run `superbackup doctor --fix` to download the pinned build, or set the path in \
             Settings.",
        ),
    }
}

fn vault_checks(ctx: &mut Ctx, daemon: Option<&Daemon>) -> Vec<DoctorCheck> {
    let mut out = Vec::new();
    let vault = ctx.paths.vault_file();
    if vault.is_file() {
        let size = std::fs::metadata(&vault).map(|m| m.len()).unwrap_or(0);
        out.push(with_detail(
            check("vault.present", "the vault file is there", CheckStatus::Pass),
            format!("{} ({})", vault.display(), format::bytes(size)),
        ));
    } else {
        out.push(with_hint(
            with_detail(
                check("vault.present", "the vault file is there", CheckStatus::Warn),
                format!("{} does not exist yet", vault.display()),
            ),
            "Run `superbackup init` to set a master passphrase and create it.",
        ));
    }

    match daemon {
        Some(daemon) => match reply!(daemon, Request::VaultIsUnlocked {}, Unlocked) {
            Ok(state) if state.unlocked => {
                out.push(check("vault.unlocked", "the vault is unlocked", CheckStatus::Pass))
            }
            Ok(_) => out.push(with_hint(
                check("vault.unlocked", "the vault is unlocked", CheckStatus::Warn),
                "Scheduled backups cannot run while it is locked. Run `superbackup unlock`.",
            )),
            Err(e) => out.push(with_detail(
                check("vault.unlocked", "the vault is unlocked", CheckStatus::Warn),
                e.message,
            )),
        },
        None => out.push(check("vault.unlocked", "the vault is unlocked", CheckStatus::Skipped)),
    }
    out
}

fn disk_check(ctx: &mut Ctx) -> DoctorCheck {
    match platform::disk_space(&ctx.paths.data_dir) {
        Some((free, total)) => {
            let detail = format!("{} free of {}", format::bytes(free), format::bytes(total));
            if free < LOW_DISK_BYTES {
                with_hint(
                    with_detail(
                        check("disk.space", "there is room to work", CheckStatus::Warn),
                        detail,
                    ),
                    "Backups write a cache and a temporary index here. Free some space.",
                )
            } else {
                with_detail(
                    check("disk.space", "there is room to work", CheckStatus::Pass),
                    detail,
                )
            }
        }
        None => with_detail(
            check("disk.space", "there is room to work", CheckStatus::Skipped),
            "this platform would not report the free space",
        ),
    }
}

fn autostart_checks(fix: bool, fixed: &mut Vec<String>) -> Vec<DoctorCheck> {
    let spec = match platform::autostart::AutostartSpec::current() {
        Ok(spec) => spec,
        Err(e) => {
            return vec![with_detail(
                check("autostart.state", "start at login", CheckStatus::Warn),
                e.to_string(),
            )]
        }
    };
    let status = match platform::autostart::status(&spec) {
        Ok(status) => status,
        Err(e) => {
            return vec![with_detail(
                check("autostart.state", "start at login", CheckStatus::Warn),
                e.to_string(),
            )]
        }
    };

    let summary = status.state.summary();
    if status.state.needs_repair() {
        let mut c = fixable(with_detail(
            check("autostart.state", "start at login", CheckStatus::Warn),
            summary.clone(),
        ));
        if fix {
            // Repointing a stale entry at this executable is the one repair
            // here that is obviously right: the entry already exists and
            // already says the user wants it.
            match platform::autostart::heal(&spec) {
                Ok(Some(_)) => {
                    fixed.push("autostart.state".to_string());
                    c = with_detail(
                        check("autostart.state", "start at login", CheckStatus::Pass),
                        "the entry was repointed at this copy of superbackup",
                    );
                }
                Ok(None) => {}
                Err(e) => c = with_detail(c, format!("could not be repaired: {e}")),
            }
        } else {
            c = with_hint(c, "Run `superbackup doctor --fix` to repoint it.");
        }
        return vec![c];
    }

    vec![with_detail(
        check("autostart.state", "start at login", CheckStatus::Pass),
        summary,
    )]
}

fn service_check() -> DoctorCheck {
    let status = platform::service::status(
        platform::service::DEFAULT_SERVICE_NAME,
        platform::service::ServiceScope::System,
    );
    match status {
        Ok(status) if !status.installed => with_detail(
            check("service.state", "the background service", CheckStatus::Skipped),
            "not installed; backups run while you are logged in",
        ),
        Ok(status) if status.state == platform::service::ServiceState::Running => with_detail(
            check("service.state", "the background service", CheckStatus::Pass),
            status.state.title(),
        ),
        Ok(status) => with_hint(
            with_detail(
                check("service.state", "the background service", CheckStatus::Fail),
                format!("installed but {}", status.state.title().to_lowercase()),
            ),
            "Start it with `superbackup service start`.",
        ),
        Err(e) => with_detail(
            check("service.state", "the background service", CheckStatus::Warn),
            e.to_string(),
        ),
    }
}

fn config_check(daemon: &Daemon) -> DoctorCheck {
    let parts = (
        reply!(daemon, Request::SettingsGet {}, Settings),
        super::providers(daemon),
        super::destinations(daemon),
        super::jobs(daemon),
    );
    let (settings, providers, destinations, jobs) = match parts {
        (Ok(s), Ok(p), Ok(d), Ok(j)) => (*s.settings, p, d, j),
        _ => {
            return with_detail(
                check("config.valid", "the configuration is valid", CheckStatus::Warn),
                "the configuration could not be read",
            )
        }
    };
    let config =
        superbackup_core::model::Config { settings, providers, destinations, jobs, ..Default::default() };
    let report = superbackup_core::config::validate(&config);

    if !report.errors.is_empty() {
        let detail =
            report.errors.iter().map(|i| i.to_string()).collect::<Vec<_>>().join("; ");
        with_hint(
            with_detail(
                check("config.valid", "the configuration is valid", CheckStatus::Fail),
                detail,
            ),
            "Run `superbackup config validate` for the full list.",
        )
    } else if !report.warnings.is_empty() {
        let detail =
            report.warnings.iter().map(|i| i.to_string()).collect::<Vec<_>>().join("; ");
        with_detail(
            check("config.valid", "the configuration is valid", CheckStatus::Warn),
            detail,
        )
    } else {
        check("config.valid", "the configuration is valid", CheckStatus::Pass)
    }
}

fn destination_checks(ctx: &mut Ctx, daemon: &Daemon) -> Vec<DoctorCheck> {
    let all = match super::destinations(daemon) {
        Ok(all) => all,
        Err(e) => {
            return vec![with_detail(
                check("dest.reachable", "destinations are reachable", CheckStatus::Warn),
                e.message,
            )]
        }
    };
    if all.is_empty() {
        return vec![with_detail(
            check("dest.reachable", "destinations are reachable", CheckStatus::Skipped),
            "none are configured",
        )];
    }

    let mut out = Vec::new();
    for destination in &all {
        ctx.ui.note(format!("Checking {}...", destination.name));
        let id = format!("dest.reachable:{}", destination.name);
        let title = format!("{} is reachable", destination.name);
        let probe = reply!(
            daemon,
            Request::DestinationTest { destination: destination.id.to_string() },
            Probe
        );
        out.push(match probe {
            Ok(probe) if probe.reachable && probe.writable => with_detail(
                check(&id, &title, CheckStatus::Pass),
                probe
                    .latency_ms
                    .map(|ms| format!("answered in {ms} ms"))
                    .unwrap_or_else(|| "answered".to_string()),
            ),
            Ok(probe) if probe.reachable => with_hint(
                with_detail(
                    check(&id, &title, CheckStatus::Fail),
                    probe.detail.unwrap_or_else(|| "reachable but not writable".to_string()),
                ),
                "Every backup to this destination will fail until it can be written to.",
            ),
            Ok(probe) => with_detail(
                check(&id, &title, CheckStatus::Fail),
                probe.detail.unwrap_or_else(|| "could not be reached".to_string()),
            ),
            Err(e) => with_detail(check(&id, &title, CheckStatus::Fail), e.message),
        });
    }
    out
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

fn render(ctx: &mut Ctx, checks: &[DoctorCheck], fixed: &[String], fixing: bool) {
    let mut table = Table::new(vec![
        Column::new(""),
        Column::new("check").flex(),
        Column::new("detail").flex(),
    ]);
    for c in checks {
        table.push(vec![
            Cell::coloured(status_mark(c.status), status_colour(c.status)),
            Cell::new(c.title.clone()),
            Cell::new(c.detail.clone().unwrap_or_default()),
        ]);
    }
    ctx.ui.table(&table);

    let hints: Vec<&DoctorCheck> = checks
        .iter()
        .filter(|c| c.hint.is_some() && matches!(c.status, CheckStatus::Fail | CheckStatus::Warn))
        .collect();
    if !hints.is_empty() {
        ctx.ui.blank();
        ctx.ui.heading("What to do");
        for c in hints {
            ctx.ui.line(format!("  {}: {}", c.title, c.hint.clone().unwrap_or_default()));
        }
    }

    if fixing {
        ctx.ui.blank();
        if fixed.is_empty() {
            ctx.ui.line("--fix: nothing needed repairing that could be repaired safely.");
        } else {
            ctx.ui.heading("Fixed");
            for id in fixed {
                ctx.ui.line(format!("  {id}"));
            }
        }
    }

    // Platform limitations are facts about the operating system, not faults,
    // so they are listed rather than counted as failures. They are here
    // because the alternative is a user meeting each one as a bug report.
    let limitations = platform::limitations();
    if !limitations.is_empty() {
        ctx.ui.blank();
        ctx.ui.heading("Worth knowing about this platform");
        for limitation in &limitations {
            ctx.ui.line(format!("  [{}] {}", limitation.area, squash(&limitation.message)));
            if let Some(remedy) = &limitation.remedy {
                ctx.ui.line(format!("      {}", squash(remedy)));
            }
        }
    }

    let failed = checks.iter().filter(|c| c.status == CheckStatus::Fail).count();
    let warned = checks.iter().filter(|c| c.status == CheckStatus::Warn).count();
    ctx.ui.blank();
    if failed > 0 {
        ctx.ui.coloured(
            Colour::Red,
            &format!(
                "{} and {}.",
                format::plural(failed, "check failed", "checks failed"),
                format::plural(warned, "warning", "warnings")
            ),
        );
    } else if warned > 0 {
        ctx.ui.coloured(
            Colour::Yellow,
            &format!("Nothing is broken, but there {}.", if warned == 1 {
                "is 1 warning".to_string()
            } else {
                format!("are {warned} warnings")
            }),
        );
    } else {
        ctx.ui.coloured(Colour::Green, "Everything checks out.");
    }
}

/// The limitation strings are written for a GUI and carry the source file's
/// line-continuation whitespace; collapse it so a terminal line reads well.
fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn status_mark(status: CheckStatus) -> &'static str {
    match status {
        CheckStatus::Pass => "ok",
        CheckStatus::Warn => "!!",
        CheckStatus::Fail => "XX",
        CheckStatus::Skipped => "--",
    }
}

fn status_colour(status: CheckStatus) -> Colour {
    match status {
        CheckStatus::Pass => Colour::Green,
        CheckStatus::Warn => Colour::Yellow,
        CheckStatus::Fail => Colour::Red,
        CheckStatus::Skipped => Colour::Dim,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_limitation_text_is_readable_on_one_line() {
        for limitation in platform::limitations() {
            let squashed = squash(&limitation.message);
            assert!(!squashed.contains("  "), "double spaces survived: {squashed}");
        }
    }
}
