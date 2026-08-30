//! Setting the machine up: init, the service, autostart, settings, and the
//! shared configuration repository.

use superbackup_core::error::ErrorCode;
use superbackup_core::ipc::protocol::{Request, SecretString};
use superbackup_core::model::{Destination, DestinationKind, RetentionPolicy, Settings};
use superbackup_core::platform;

use crate::cli::args::{
    AutostartCommand, ConfigCommand, InitArgs, RemoteCommand, RemotePullArgs, RemotePushArgs,
    RemoteSetArgs, ServiceCommand, ServiceInstallArgs,
};
use crate::cli::client::{reply, Daemon, Start};
use crate::cli::context::Ctx;
use crate::cli::format::{self, Cell, Colour, Column, Table};
use crate::cli::output::{CliError, CliResult, Outcome};
use crate::cli::{prompt, schedule};

use super::{destinations, jobs, providers};

// ---------------------------------------------------------------------------
// init
// ---------------------------------------------------------------------------

pub fn init(ctx: &mut Ctx, args: InitArgs) -> CliResult<Outcome> {
    if args.non_interactive && args.passphrase_file.is_none() {
        return Err(CliError::usage(
            "--non-interactive needs --passphrase-file: there is no way to invent a master \
             passphrase without asking",
        ));
    }
    if args.machine_name.is_some() {
        // The machine identity lives in the configuration file and the IPC
        // surface has no command that edits it.
        ctx.ui.warn(
            "--machine-name cannot be applied from here: the running instance exposes no \
             command that renames the machine. Set it in the interface instead.",
        );
    }
    let interactive = !args.non_interactive && ctx.can_prompt();

    // Creating the directories is a local filesystem act, not a vault or
    // repository one, and doing it here means the daemon that is about to be
    // started finds a home it can write to.
    ctx.paths.ensure().map_err(CliError::from)?;
    ctx.ui.line(format!("Configuration lives in {}.", ctx.paths.config_dir.display()));

    let daemon = Daemon::connect(ctx, Start::IfNeeded)?;
    let version = reply!(daemon, Request::Version {}, Version)?;
    ctx.ui.line(format!(
        "Talking to superbackup {} (protocol {}).",
        version.version, version.protocol
    ));
    match &version.kopia_version {
        Some(v) => ctx.ui.line(format!("Kopia {v} is available.")),
        None => ctx.ui.warn(
            "kopia was not found. Repository destinations will not work until it is. Run \
             `superbackup doctor --fix`.",
        ),
    }

    // -- the vault -------------------------------------------------------
    let state = reply!(daemon, Request::VaultIsUnlocked {}, Unlocked)?;
    let vault_exists = ctx.paths.vault_file().exists();
    if state.unlocked {
        ctx.ui.line("The vault is already unlocked.");
    } else {
        let secret = match args.passphrase_file.as_deref() {
            Some(path) => prompt::from_file(path)?,
            None if !vault_exists => {
                ctx.ui.blank();
                ctx.ui.heading("Choose a master passphrase");
                ctx.ui.line("It protects every repository key superbackup holds.");
                ctx.ui.line("There is no recovery if it is lost. Write it down somewhere safe.");
                prompt::new_passphrase(
                    ctx,
                    "Master passphrase: ",
                    "Repeat the master passphrase: ",
                )?
            }
            None => prompt::from_terminal(ctx, "Master passphrase: ")?,
        };
        let unlocked = reply!(
            daemon,
            Request::VaultUnlock { passphrase: SecretString::new(secret) },
            Unlocked
        )?;
        ctx.ui.line(if unlocked.unlocked {
            "The vault is unlocked."
        } else {
            "The vault is still locked."
        });
    }

    // -- start at login --------------------------------------------------
    if args.skip_autostart {
        ctx.ui.line("Leaving start-at-login alone, as asked.");
    } else {
        match reply!(daemon, Request::ServiceSetAutostart { enabled: true }, Service) {
            Ok(service) if service.autostart => {
                ctx.ui.line("superbackup will start when you log in.")
            }
            Ok(_) => ctx.ui.warn("start-at-login could not be registered."),
            Err(e) => ctx.ui.warn(format!("start-at-login could not be registered: {}", e.message)),
        }
    }

    // -- OneDrive --------------------------------------------------------
    if args.skip_onedrive {
        ctx.ui.line("Not looking for OneDrive, as asked.");
    } else {
        let accounts = platform::onedrive::detect();
        if accounts.is_empty() {
            ctx.ui.line("No OneDrive folder was found on this machine.");
        } else {
            for account in &accounts {
                ctx.ui.line(format!(
                    "Found {} with {} free.",
                    account.display_name,
                    format::bytes(account.available_bytes)
                ));
                for warning in &account.warnings {
                    ctx.ui.warn(warning);
                }
            }
        }
    }

    // -- a first destination and job -------------------------------------
    let existing_destinations = destinations(&daemon)?;
    let existing_jobs = jobs(&daemon)?;

    if interactive && existing_destinations.is_empty() {
        offer_first_destination(ctx, &daemon)?;
    }
    let have_destinations = !destinations(&daemon)?.is_empty();
    if interactive && have_destinations && existing_jobs.is_empty() {
        offer_first_job(ctx, &daemon)?;
    }

    ctx.ui.blank();
    ctx.ui.heading("Next");
    if !have_destinations {
        ctx.ui.line("  superbackup destination add --local PATH   choose where backups go");
    }
    ctx.ui.line("  superbackup job add --name NAME --source PATH   say what to back up");
    ctx.ui.line("  superbackup status                             see how it is going");

    Outcome::data(serde_json::json!({
        "config_dir": ctx.paths.config_dir,
        "version": version,
        "destinations": destinations(&daemon)?.len(),
        "jobs": jobs(&daemon)?.len(),
    }))
}

fn offer_first_destination(ctx: &mut Ctx, daemon: &Daemon) -> CliResult<()> {
    ctx.ui.blank();
    if !prompt::ask_yes_no(ctx, "Set up somewhere for backups to go now?", true)? {
        return Ok(());
    }
    let suggestion = platform::onedrive::detect()
        .first()
        .map(|a| a.suggested_repository_root())
        .unwrap_or_else(|| ctx.paths.data_dir.join("repository"));
    let answer =
        prompt::ask_with_default(ctx, "Folder for the repository", &suggestion.display().to_string())?;
    let path = super::objects::absolute(std::path::Path::new(&answer));

    let destination = Destination {
        id: uuid::Uuid::new_v4(),
        name: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "local".to_string()),
        kind: DestinationKind::LocalRepository { path: path.clone() },
        encryption: Some(Default::default()),
        passphrase_ref: None,
        retention: RetentionPolicy::default(),
        enabled: true,
        auto_discovered: false,
        bandwidth: None,
        created_at: chrono::Utc::now(),
        last_verified_at: None,
    };
    let created = *reply!(
        daemon,
        Request::DestinationCreate { destination: Box::new(destination) },
        Destination
    )?
    .destination;
    ctx.ui.line(format!("Added {}.", created.name));

    ctx.ui.note("Creating the repository...");
    let repo = reply!(
        daemon,
        Request::DestinationRepoCreate {
            destination: created.id.to_string(),
            encryption: created.encryption.clone(),
        },
        Repository
    )?;
    if repo.created || repo.connected {
        ctx.ui.line(format!("The repository at {} is ready.", path.display()));
    }
    Ok(())
}

fn offer_first_job(ctx: &mut Ctx, daemon: &Daemon) -> CliResult<()> {
    ctx.ui.blank();
    if !prompt::ask_yes_no(ctx, "Create a first backup job now?", true)? {
        return Ok(());
    }
    let folder = prompt::ask_line(ctx, "Folder to back up: ")?.trim().to_string();
    if folder.is_empty() {
        ctx.ui.line("No folder given; skipping.");
        return Ok(());
    }
    let path = super::objects::absolute(std::path::Path::new(&folder));
    if !path.exists() {
        return Err(CliError::usage(format!("{} could not be found", path.display())));
    }
    let name = prompt::ask_with_default(
        ctx,
        "Name for this job",
        &path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "backup".into()),
    )?;
    let spec = prompt::ask_with_default(ctx, "When should it run", "daily@02:00")?;
    let parsed = schedule::parse(&spec)?;

    let all_destinations = destinations(daemon)?;
    let job = superbackup_core::model::Job {
        id: uuid::Uuid::new_v4(),
        name: name.clone(),
        project_id: None,
        description: String::new(),
        sources: vec![superbackup_core::model::Source::new(path)],
        destination_ids: all_destinations.iter().map(|d| d.id).collect(),
        schedule: parsed,
        // The template that is the reason this program exists.
        exclusions: superbackup_core::model::ExclusionSet::developer_defaults(),
        bandwidth: None,
        retention: None,
        enabled: true,
        timeout_minutes: None,
        hooks: Default::default(),
        continue_on_destination_error: true,
        created_at: chrono::Utc::now(),
        tags: Vec::new(),
    };
    let created = *reply!(daemon, Request::JobCreate { job: Box::new(job) }, Job)?.job;
    ctx.ui.line(format!(
        "Created {}, running {}, skipping build output and caches.",
        created.name,
        schedule::describe(&created.schedule)
    ));
    ctx.ui.line(format!("Try it now with `superbackup run {} --wait`.", created.name));
    Ok(())
}

// ---------------------------------------------------------------------------
// service
// ---------------------------------------------------------------------------

/// Service control has to work when the daemon is *not* running — that is
/// most of the point of it — so the local platform layer answers where it can
/// and the daemon is consulted only for the things it owns.
pub fn service(ctx: &mut Ctx, command: ServiceCommand) -> CliResult<Outcome> {
    match command {
        ServiceCommand::Status => service_status(ctx),
        ServiceCommand::Install(args) => service_install(ctx, args),
        ServiceCommand::Uninstall { yes } => service_uninstall(ctx, yes),
        ServiceCommand::Start => service_control(ctx, true),
        ServiceCommand::Stop => service_control(ctx, false),
        // `main.rs` intercepts this; the arm keeps the match exhaustive.
        ServiceCommand::Run => Err(CliError::new(
            ErrorCode::Internal,
            "`service run` is the operating system's entry point and is handled elsewhere",
        )),
    }
}

fn local_service_status() -> superbackup_core::error::Result<platform::service::ServiceStatus> {
    platform::service::status(
        platform::service::DEFAULT_SERVICE_NAME,
        platform::service::ServiceScope::System,
    )
}

fn service_status(ctx: &mut Ctx) -> CliResult<Outcome> {
    let status = local_service_status().map_err(CliError::from)?;
    let autostart = autostart_status();

    let pad = 16;
    ctx.ui.heading("Background service");
    ctx.ui.field("State", status.state.title(), pad);
    if let Some(account) = &status.account {
        ctx.ui.field("Runs as", account, pad);
    }
    if let Some(exe) = &status.executable {
        ctx.ui.field("Executable", exe.display().to_string(), pad);
    }
    if let Some(pid) = status.pid {
        ctx.ui.field("Process", pid.to_string(), pad);
    }
    if let Some(detail) = &status.detail {
        ctx.ui.field("Detail", detail, pad);
    }
    if let Ok(exe) = std::env::current_exe() {
        if status.installed && status.is_stale(&exe) {
            ctx.ui.warn(
                "the installed service points at a different copy of superbackup than this one; \
                 reinstall it to repair that",
            );
        }
    }

    ctx.ui.blank();
    ctx.ui.heading("Start at login");
    match &autostart {
        Ok(state) => ctx.ui.line(format!("  {}", state.state.summary())),
        Err(e) => ctx.ui.line(format!("  could not be read: {e}")),
    }

    let healthy = !status.installed || status.state == platform::service::ServiceState::Running;
    let value = serde_json::json!({
        "service": status,
        "autostart": autostart.ok(),
    });
    if healthy {
        Outcome::data(value)
    } else {
        Outcome::negative(value)
    }
}

fn autostart_status() -> superbackup_core::error::Result<platform::autostart::AutostartStatus> {
    let spec = platform::autostart::AutostartSpec::current()?;
    platform::autostart::status(&spec)
}

fn service_install(ctx: &mut Ctx, args: ServiceInstallArgs) -> CliResult<Outcome> {
    if args.user.is_some() {
        return Err(CliError::unsupported(
            "--user",
            "the running instance installs the service with its own defaults and takes no \
             account parameter",
        )
        .with_hint(
            "A system account cannot see OneDrive or mapped drives; leave those destinations \
             to the tray application.",
        ));
    }
    if args.user_scope {
        return Err(CliError::unsupported(
            "--user-scope",
            "the running instance takes no scope parameter on `service.install`",
        ));
    }
    if !platform::service::is_elevated() {
        ctx.ui.warn(
            "installing a service needs administrator rights. If this fails, run it from an \
             elevated terminal.",
        );
    }
    let daemon = Daemon::connect(ctx, Start::Never)?;
    let service = reply!(daemon, Request::ServiceInstall {}, Service)?;
    if service.installed {
        ctx.ui.line("The service is installed. Backups can now run with nobody logged in.");
    } else {
        ctx.ui.line("The service was not installed.");
    }
    if let Some(detail) = &service.detail {
        ctx.ui.line(format!("  {detail}"));
    }
    if service.installed {
        Outcome::data(service)
    } else {
        Outcome::negative(service)
    }
}

fn service_uninstall(ctx: &mut Ctx, yes: bool) -> CliResult<Outcome> {
    prompt::confirm(
        ctx,
        "Removing the background service. Scheduled backups will only run while you are logged \
         in with superbackup open",
        yes,
    )?;
    let daemon = Daemon::connect(ctx, Start::Never)?;
    let service = reply!(daemon, Request::ServiceUninstall {}, Service)?;
    ctx.ui.line("The service was removed. Your configuration and your backups are untouched.");
    Outcome::data(service)
}

/// `service start` and `service stop` have no IPC command behind them: the
/// service is an operating-system object, and when it is stopped there is no
/// daemon to ask. Both are therefore performed locally through the same
/// platform layer the daemon itself uses.
fn service_control(ctx: &mut Ctx, start: bool) -> CliResult<Outcome> {
    use platform::service::{ServiceScope, DEFAULT_SERVICE_NAME};

    if !platform::service::is_elevated() {
        ctx.ui.warn("controlling a service usually needs administrator rights.");
    }
    let outcome = if start {
        platform::service::start(DEFAULT_SERVICE_NAME, ServiceScope::System)
    } else {
        platform::service::stop(DEFAULT_SERVICE_NAME, ServiceScope::System)
    };
    outcome.map_err(CliError::from)?;

    let status = local_service_status().map_err(CliError::from)?;
    ctx.ui.line(format!("The service is {}.", status.state.title().to_lowercase()));
    if !start {
        ctx.ui.line("Scheduled backups will not run until it is started again.");
    }
    Outcome::data(status)
}

// ---------------------------------------------------------------------------
// autostart
// ---------------------------------------------------------------------------

pub fn autostart(ctx: &mut Ctx, command: AutostartCommand) -> CliResult<Outcome> {
    match command {
        AutostartCommand::Status => {
            let status = autostart_status().map_err(CliError::from)?;
            ctx.ui.line(status.state.summary());
            ctx.ui.field("Entry", &status.location, 10);
            if let Some(command) = &status.registered_command {
                ctx.ui.field("Command", command, 10);
            }
            if status.state.needs_repair() {
                ctx.ui.warn("repair it with `superbackup doctor --fix`.");
                return Outcome::negative(status);
            }
            Outcome::data(status)
        }
        AutostartCommand::Enable => set_autostart(ctx, true),
        AutostartCommand::Disable => set_autostart(ctx, false),
    }
}

/// The daemon owns this setting, so it is told first. When nothing is
/// listening the registration is still made locally — otherwise "start at
/// login" would be impossible to turn on until something was already running,
/// which is precisely backwards.
fn set_autostart(ctx: &mut Ctx, enabled: bool) -> CliResult<Outcome> {
    match Daemon::connect(ctx, Start::Never) {
        Ok(daemon) => {
            let service = reply!(daemon, Request::ServiceSetAutostart { enabled }, Service)?;
            report_autostart(ctx, service.autostart);
            Outcome::data(service)
        }
        Err(e) if e.code == ErrorCode::DaemonUnreachable => {
            ctx.ui.note("Nothing is running, so this is being registered directly.");
            let spec = platform::autostart::AutostartSpec::current().map_err(CliError::from)?;
            if enabled {
                platform::autostart::enable(&spec).map_err(CliError::from)?;
            } else {
                platform::autostart::disable().map_err(CliError::from)?;
            }
            report_autostart(ctx, enabled);
            Outcome::data(autostart_status().map_err(CliError::from)?)
        }
        Err(e) => Err(e),
    }
}

fn report_autostart(ctx: &mut Ctx, enabled: bool) {
    if enabled {
        ctx.ui.line("superbackup will start when you log in.");
    } else {
        ctx.ui.line(
            "superbackup will not start when you log in. Scheduled backups will not run unless \
             the service is installed.",
        );
    }
}

// ---------------------------------------------------------------------------
// config
// ---------------------------------------------------------------------------

pub fn config(ctx: &mut Ctx, command: ConfigCommand) -> CliResult<Outcome> {
    let daemon = Daemon::connect(ctx, Start::Never)?;
    match command {
        ConfigCommand::Show => config_show(ctx, &daemon),
        ConfigCommand::Get { key } => config_get(ctx, &daemon, &key),
        ConfigCommand::Set { key, value } => config_set(ctx, &daemon, &key, &value),
        ConfigCommand::Validate => config_validate(ctx, &daemon),
        ConfigCommand::Export { to } => config_export(ctx, &daemon, &to),
        ConfigCommand::Reload => {
            reply!(daemon, Request::ControlReloadConfig {}, Ack)?;
            ctx.ui.line("The configuration was re-read from disk.");
            Outcome::data(serde_json::json!({ "reloaded": true }))
        }
    }
}

/// The whole configuration, assembled from the four commands that expose it.
///
/// There is no `config.get` in the protocol — settings, jobs, destinations and
/// providers are separate commands — so this is the join. No secret is in any
/// of them: providers carry vault handles, never values.
fn whole_config(daemon: &Daemon) -> CliResult<serde_json::Value> {
    let settings = *reply!(daemon, Request::SettingsGet {}, Settings)?.settings;
    Ok(serde_json::json!({
        "settings": settings,
        "providers": providers(daemon)?,
        "destinations": destinations(daemon)?,
        "jobs": jobs(daemon)?,
    }))
}

fn config_show(ctx: &mut Ctx, daemon: &Daemon) -> CliResult<Outcome> {
    let document = whole_config(daemon)?;
    if !ctx.ui.json {
        // Human mode prints the settings as a flat key/value list, because
        // `config get`/`config set` speak in exactly those keys.
        ctx.ui.heading("Settings");
        let mut table = Table::new(vec![Column::new("key").flex(), Column::new("value").flex()]);
        for (key, value) in flatten(&document["settings"], "") {
            table.push(vec![Cell::new(key), Cell::new(render_scalar(&value))]);
        }
        ctx.ui.table(&table);
        ctx.ui.blank();
        ctx.ui.line(format!(
            "{}, {}, {}. Secrets are held in the vault and are never shown.",
            format::plural(
                document["jobs"].as_array().map(|a| a.len()).unwrap_or(0),
                "job",
                "jobs"
            ),
            format::plural(
                document["destinations"].as_array().map(|a| a.len()).unwrap_or(0),
                "destination",
                "destinations"
            ),
            format::plural(
                document["providers"].as_array().map(|a| a.len()).unwrap_or(0),
                "provider",
                "providers"
            ),
        ));
    }
    Outcome::data(document)
}

fn config_get(ctx: &mut Ctx, daemon: &Daemon, key: &str) -> CliResult<Outcome> {
    let settings = serde_json::to_value(
        *reply!(daemon, Request::SettingsGet {}, Settings)?.settings,
    )
    .map_err(|e| CliError::new(ErrorCode::Internal, e.to_string()))?;
    let value = lookup(&settings, key).ok_or_else(|| unknown_key(key, &settings))?;
    ctx.ui.line(render_scalar(value));
    Outcome::data(value.clone())
}

fn config_set(ctx: &mut Ctx, daemon: &Daemon, key: &str, value: &str) -> CliResult<Outcome> {
    let settings = *reply!(daemon, Request::SettingsGet {}, Settings)?.settings;
    let mut document = serde_json::to_value(&settings)
        .map_err(|e| CliError::new(ErrorCode::Internal, e.to_string()))?;

    let current = lookup(&document, key).cloned().ok_or_else(|| unknown_key(key, &document))?;
    // A bare word is a string; `true`, `42` and `null` are themselves. Typing
    // the JSON form is always available for the ambiguous cases.
    let parsed: serde_json::Value = serde_json::from_str(value)
        .unwrap_or_else(|_| serde_json::Value::String(value.to_string()));

    replace(&mut document, key, parsed.clone())?;

    // Round-trip through `Settings` before sending: a value of the wrong type
    // is rejected here, with the key named, rather than by the daemon with a
    // serde error that points at nothing.
    let updated: Settings = serde_json::from_value(document).map_err(|e| {
        CliError::usage(format!("{key} cannot be set to {}: {e}", render_scalar(&parsed)))
            .with_hint(format!("It is currently {}.", render_scalar(&current)))
    })?;

    let stored =
        *reply!(daemon, Request::SettingsUpdate { settings: Box::new(updated) }, Settings)?
            .settings;
    let stored_value = serde_json::to_value(&stored)
        .map_err(|e| CliError::new(ErrorCode::Internal, e.to_string()))?;
    let now = lookup(&stored_value, key).cloned().unwrap_or(serde_json::Value::Null);

    ctx.ui.line(format!("{key}: {} -> {}", render_scalar(&current), render_scalar(&now)));
    Outcome::data(serde_json::json!({ "key": key, "previous": current, "value": now }))
}

fn config_validate(ctx: &mut Ctx, daemon: &Daemon) -> CliResult<Outcome> {
    let snapshot = *reply!(daemon, Request::Status {}, Status)?.snapshot;
    let settings = *reply!(daemon, Request::SettingsGet {}, Settings)?.settings;

    // The protocol has no `config.validate`, but it does expose every part the
    // validator reads, so the check runs here against the same code the daemon
    // uses when it saves. The machine identity is not exposed over IPC and is
    // reconstructed from the status snapshot; it is not what this validates.
    let mut config = superbackup_core::model::Config {
        settings,
        providers: providers(daemon)?,
        destinations: destinations(daemon)?,
        jobs: jobs(daemon)?,
        ..Default::default()
    };
    config.machine.label = snapshot.machine_label.clone();
    config.machine.slug = snapshot.machine_slug.clone();

    let report = superbackup_core::config::validate(&config);
    let mut table = Table::new(vec![
        Column::new("severity"),
        Column::new("where").flex(),
        Column::new("problem").flex(),
    ])
    .empty_note("The configuration is valid.");
    for issue in &report.errors {
        table.push(vec![
            Cell::coloured("error", Colour::Red),
            Cell::new(issue.location.clone()),
            Cell::new(issue.message.clone()),
        ]);
    }
    for issue in &report.warnings {
        table.push(vec![
            Cell::coloured("warning", Colour::Yellow),
            Cell::new(issue.location.clone()),
            Cell::new(issue.message.clone()),
        ]);
    }
    ctx.ui.table(&table);

    let value = serde_json::json!({ "errors": report.errors, "warnings": report.warnings });
    if report.errors.is_empty() {
        Outcome::data(value)
    } else {
        Outcome::negative(value)
    }
}

fn config_export(ctx: &mut Ctx, daemon: &Daemon, to: &std::path::Path) -> CliResult<Outcome> {
    let document = whole_config(daemon)?;
    let text = serde_json::to_string_pretty(&document)
        .map_err(|e| CliError::new(ErrorCode::Internal, e.to_string()))?;
    std::fs::write(to, text.as_bytes()).map_err(|e| {
        CliError::new(ErrorCode::Io, format!("writing {}: {e}", to.display()))
    })?;
    ctx.ui.line(format!("Wrote the configuration to {}. It holds no secrets.", to.display()));
    Outcome::data(serde_json::json!({ "written": to }))
}

/// `bandwidth.upload_kbps` into a JSON document.
fn lookup<'a>(document: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
    let mut current = document;
    for segment in key.split('.') {
        current = current.get(segment)?;
    }
    Some(current)
}

fn replace(
    document: &mut serde_json::Value,
    key: &str,
    value: serde_json::Value,
) -> CliResult<()> {
    let mut current = document;
    let segments: Vec<&str> = key.split('.').collect();
    let (last, parents) = segments.split_last().unwrap_or((&"", &[]));
    for segment in parents {
        current = current
            .get_mut(*segment)
            .ok_or_else(|| CliError::usage(format!("there is no setting called {key}")))?;
    }
    match current.get_mut(*last) {
        Some(slot) => {
            *slot = value;
            Ok(())
        }
        None => Err(CliError::usage(format!("there is no setting called {key}"))),
    }
}

fn unknown_key(key: &str, document: &serde_json::Value) -> CliError {
    let mut known: Vec<String> = flatten(document, "").into_iter().map(|(k, _)| k).collect();
    known.sort();
    // Suggest by shared prefix rather than dumping forty keys at somebody who
    // mistyped one.
    let head = key.split('.').next().unwrap_or(key);
    let near: Vec<&String> = known.iter().filter(|k| k.starts_with(head)).take(8).collect();
    let error = CliError::usage(format!("there is no setting called {key}"));
    if near.is_empty() {
        error.with_hint("Run `superbackup config show` to see every setting.")
    } else {
        error.with_hint(format!(
            "Did you mean one of: {}?",
            near.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
        ))
    }
}

/// Flatten nested objects into dotted keys. Arrays are left whole: there is no
/// `config set` syntax for an element, so pretending otherwise would be a lie.
fn flatten(value: &serde_json::Value, prefix: &str) -> Vec<(String, serde_json::Value)> {
    let mut out = Vec::new();
    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                let path =
                    if prefix.is_empty() { key.clone() } else { format!("{prefix}.{key}") };
                if child.is_object() {
                    out.extend(flatten(child, &path));
                } else {
                    out.push((path, child.clone()));
                }
            }
        }
        other => out.push((prefix.to_string(), other.clone())),
    }
    out
}

fn render_scalar(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => format::MISSING.to_string(),
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------------------
// remote
// ---------------------------------------------------------------------------

pub fn remote(ctx: &mut Ctx, command: RemoteCommand) -> CliResult<Outcome> {
    let daemon = Daemon::connect(ctx, Start::Never)?;
    match command {
        RemoteCommand::Set(args) => remote_set(args),
        RemoteCommand::Status => remote_status(ctx, &daemon),
        RemoteCommand::Diff => remote_diff(ctx, &daemon),
        RemoteCommand::Pull(args) => remote_pull(ctx, &daemon, args),
        RemoteCommand::Push(args) => remote_push(ctx, &daemon, args),
    }
}

fn remote_set(args: RemoteSetArgs) -> CliResult<Outcome> {
    Err(CliError::unsupported(
        &format!("pointing at {}", args.url),
        "the remote configuration source lives in the configuration file and no IPC command \
         writes it",
    )
    .with_hint("Set the shared repository in the interface, under Remote configuration."))
}

fn remote_status(ctx: &mut Ctx, daemon: &Daemon) -> CliResult<Outcome> {
    // `remote.pull` returns the state but also fetches, and `remote.status`
    // does not exist, so the read-only answer is what `remote.diff` can say.
    let diff = reply!(daemon, Request::RemoteDiff {}, RemoteDiff)?;
    match &diff.remote_commit {
        Some(commit) => ctx.ui.line(format!("Last pulled commit: {commit}")),
        None => ctx.ui.line("No shared configuration has been pulled."),
    }
    if diff.changes.is_empty() {
        ctx.ui.line("The local configuration matches what was pulled.");
    } else {
        ctx.ui.line(format!(
            "{} would change if the pulled configuration were applied.",
            format::plural(diff.changes.len(), "thing", "things")
        ));
    }
    ctx.ui.note(
        "The repository URL and the time of the last pull are not available over IPC; \
         `superbackup config show` in the interface shows them.",
    );
    Outcome::data(diff)
}

fn remote_diff(ctx: &mut Ctx, daemon: &Daemon) -> CliResult<Outcome> {
    let diff = reply!(daemon, Request::RemoteDiff {}, RemoteDiff)?;
    let mut table = Table::new(vec![
        Column::new("change"),
        Column::new("what"),
        Column::new("name").flex(),
        Column::new("detail").flex(),
    ])
    .empty_note("The local configuration matches the shared one.");
    for change in &diff.changes {
        let (label, colour) = match change.kind {
            superbackup_core::ipc::protocol::ChangeKind::Added => ("added", Colour::Green),
            superbackup_core::ipc::protocol::ChangeKind::Removed => ("removed", Colour::Red),
            superbackup_core::ipc::protocol::ChangeKind::Modified => ("changed", Colour::Yellow),
        };
        table.push(vec![
            Cell::coloured(label, colour),
            Cell::new(change.entity.clone()),
            Cell::new(change.name.clone()),
            Cell::new(change.summary.clone()),
        ]);
    }
    ctx.ui.table(&table);
    Outcome::data(diff)
}

fn remote_pull(ctx: &mut Ctx, daemon: &Daemon, args: RemotePullArgs) -> CliResult<Outcome> {
    ctx.ui.note("Fetching the shared configuration...");
    let status = reply!(daemon, Request::RemotePull {}, RemoteStatus)?;
    let diff = reply!(daemon, Request::RemoteDiff {}, RemoteDiff)?;

    if diff.changes.is_empty() {
        ctx.ui.line("Nothing to apply: the local configuration already matches.");
        return Outcome::data(serde_json::json!({ "status": status, "changes": diff.changes }));
    }

    remote_diff(ctx, daemon)?;
    if !args.apply {
        ctx.ui.line("\nNothing was changed. Run again with --apply to accept these.");
        return Outcome::data(serde_json::json!({ "status": status, "changes": diff.changes }));
    }

    prompt::confirm(
        ctx,
        &format!(
            "Applying {} from the shared configuration. Local run history is kept",
            format::plural(diff.changes.len(), "change", "changes")
        ),
        args.yes,
    )?;
    let applied = reply!(daemon, Request::RemoteApply {}, RemoteStatus)?;
    ctx.ui.line("The shared configuration was applied.");
    Outcome::data(applied)
}

fn remote_push(ctx: &mut Ctx, daemon: &Daemon, args: RemotePushArgs) -> CliResult<Outcome> {
    prompt::confirm(
        ctx,
        "Publishing this machine's sealed configuration to the shared repository",
        args.yes,
    )?;
    let status =
        reply!(daemon, Request::RemotePush { message: args.message.clone() }, RemoteStatus)?;
    ctx.ui.line("The sealed vault was published.");
    if let Some(commit) = &status.last_known_commit {
        ctx.ui.line(format!("  commit {commit}"));
    }
    Outcome::data(status)
}
