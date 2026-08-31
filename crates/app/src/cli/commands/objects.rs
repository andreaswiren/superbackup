//! Jobs, destinations, providers and projects: the things a user configures.

use chrono::Utc;
use uuid::Uuid;

use superbackup_core::error::ErrorCode;
use superbackup_core::ipc::protocol::{Request, SecretString};
use superbackup_core::model::{
    default_s3_prefix, normalise_prefix, Destination, DestinationKind, EncryptionAlgorithm,
    EncryptionSettings, ExclusionSet, HashAlgorithm, Job, PassphraseSource, ProviderKind,
    RetentionPolicy, S3Credentials, S3Flavour, SecretRef, Source, Splitter, StorageProvider,
};

use crate::cli::args::{
    DestinationAddArgs, DestinationCommand, DestinationEditArgs, DestinationMaintainArgs,
    DestinationRemoveArgs, JobAddArgs, JobCommand, JobEditArgs, JobListArgs, JobPreviewArgs,
    JobRemoveArgs, JobTemplate, PassphraseMode, ProjectCommand, ProviderAddArgs, ProviderCommand,
    ProviderEditArgs, ProviderFlavour, ProviderRemoveArgs,
};
use crate::cli::client::{reply, Daemon, Start};
use crate::cli::context::Ctx;
use crate::cli::format::{self, Cell, Colour, Column, Table};
use crate::cli::output::{CliError, CliResult, Outcome};
use crate::cli::{prompt, resolve, schedule};

use super::{destinations, jobs, providers, resolve_destination, resolve_job, resolve_provider};

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Make a path absolute against *this* process's working directory.
///
/// The daemon runs somewhere else entirely — as a service, quite possibly in
/// `C:\Windows\System32`. A relative `--source .` that reached it unresolved
/// would back up the wrong folder, or nothing.
pub fn absolute(path: &std::path::Path) -> std::path::PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

fn destination_names(daemon: &Daemon) -> CliResult<std::collections::BTreeMap<Uuid, String>> {
    Ok(destinations(daemon)?.into_iter().map(|d| (d.id, d.name)).collect())
}

// ---------------------------------------------------------------------------
// job
// ---------------------------------------------------------------------------

pub fn job(ctx: &mut Ctx, command: JobCommand) -> CliResult<Outcome> {
    let daemon = Daemon::connect(ctx, Start::Never)?;
    match command {
        JobCommand::List(args) => job_list(ctx, &daemon, args),
        JobCommand::Show { job } => job_show(ctx, &daemon, &job),
        JobCommand::Add(args) => job_add(ctx, &daemon, args),
        JobCommand::Edit(args) => job_edit(ctx, &daemon, args),
        JobCommand::Remove(args) => job_remove(ctx, &daemon, args),
        JobCommand::Enable { job } => job_enabled(ctx, &daemon, &job, true),
        JobCommand::Disable { job } => job_enabled(ctx, &daemon, &job, false),
        JobCommand::Preview(args) => job_preview(ctx, &daemon, args),
    }
}

fn job_list(ctx: &mut Ctx, daemon: &Daemon, args: JobListArgs) -> CliResult<Outcome> {
    let mut all = jobs(daemon)?;
    let snapshot = *reply!(daemon, Request::Status {}, Status)?.snapshot;

    if let Some(project) = &args.project {
        // Projects exist in the configuration but the IPC surface has no
        // command that lists them, so a name cannot be turned into an id here.
        let id = Uuid::parse_str(project).map_err(|_| {
            CliError::unsupported(
                "--project by name",
                "the running instance exposes no command that lists projects",
            )
            .with_hint("Pass the project's id, which appears in `superbackup job show --json`.")
        })?;
        all.retain(|j| j.project_id == Some(id));
    }
    if args.enabled {
        all.retain(|j| j.enabled);
    }
    if args.failed {
        all.retain(|j| {
            snapshot.jobs.get(&j.id).and_then(|s| s.last_status)
                == Some(superbackup_core::state::RunStatus::Failed)
        });
    }

    let names = destination_names(daemon)?;
    let now = Utc::now();
    let mut table = Table::new(vec![
        Column::new("job").flex(),
        Column::new("schedule").flex(),
        Column::new("sources").right(),
        Column::new("destinations").flex(),
        Column::new("last run"),
        Column::new("result"),
    ])
    .empty_note(if args.failed || args.enabled || args.project.is_some() {
        "No jobs match that filter."
    } else {
        "No jobs yet. Add one with `superbackup job add --name NAME --source PATH`."
    });

    for job in &all {
        let summary = snapshot.jobs.get(&job.id).cloned().unwrap_or_default();
        let dests: Vec<String> = job
            .destination_ids
            .iter()
            .map(|id| names.get(id).cloned().unwrap_or_else(|| short_id(id)))
            .collect();
        table.push(vec![
            Cell::new(job.name.clone()),
            Cell::new(if job.enabled {
                schedule::describe(&job.schedule)
            } else {
                format!("{} (disabled)", schedule::describe(&job.schedule))
            }),
            Cell::new(job.sources.len().to_string()),
            Cell::new(if dests.is_empty() {
                format::MISSING.to_string()
            } else {
                dests.join(", ")
            }),
            Cell::new(format::opt_relative(summary.last_run, now)),
            match summary.last_status {
                Some(status) => Cell::coloured(status.title(), super::everyday::run_colour(status)),
                None => Cell::coloured("Never run", Colour::Dim),
            },
        ]);
    }
    ctx.ui.table(&table);
    Outcome::data(all)
}

fn short_id(id: &Uuid) -> String {
    id.simple().to_string().chars().take(8).collect()
}

fn job_show(ctx: &mut Ctx, daemon: &Daemon, needle: &str) -> CliResult<Outcome> {
    let job = resolve_job(daemon, needle)?;
    let names = destination_names(daemon)?;
    let pad = 20;

    ctx.ui.heading(&job.name);
    if !job.description.is_empty() {
        ctx.ui.line(format!("  {}", job.description));
    }
    ctx.ui.field("Id", job.id.to_string(), pad);
    ctx.ui.field("Enabled", if job.enabled { "yes" } else { "no" }, pad);
    ctx.ui.field("Schedule", schedule::describe(&job.schedule), pad);
    ctx.ui.field("Created", format::timestamp_local(job.created_at), pad);
    if let Some(minutes) = job.timeout_minutes {
        ctx.ui.field("Timeout", format!("{minutes} minutes"), pad);
    }
    ctx.ui.field(
        "Bandwidth",
        job.bandwidth
            .as_ref()
            .and_then(|b| b.upload_kbps)
            .map(|k| format!("{k} KB/s up"))
            .unwrap_or_else(|| "inherited".to_string()),
        pad,
    );

    ctx.ui.blank();
    ctx.ui.heading("Sources");
    for source in &job.sources {
        ctx.ui.line(format!("  {}", source.path.display()));
    }
    if job.sources.is_empty() {
        ctx.ui.line("  none - this job would back up nothing");
    }

    ctx.ui.blank();
    ctx.ui.heading("Destinations");
    if job.destination_ids.is_empty() {
        ctx.ui.line("  none - this job has nowhere to write");
    }
    for id in &job.destination_ids {
        match names.get(id) {
            Some(name) => ctx.ui.line(format!("  {name}")),
            // A dangling id is worth showing rather than hiding: it is exactly
            // the state that makes a job fail with nothing to point at.
            None => ctx.ui.line(format!("  {id} (no such destination)")),
        }
    }

    ctx.ui.blank();
    ctx.ui.heading("Exclusions");
    for preset in &job.exclusions.presets {
        ctx.ui.line(format!("  {} (preset)", preset.title()));
    }
    for pattern in &job.exclusions.patterns {
        ctx.ui.line(format!("  {pattern}"));
    }
    if job.exclusions.presets.is_empty() && job.exclusions.patterns.is_empty() {
        ctx.ui.line("  none - everything under the sources is backed up");
    }
    if let Some(mb) = job.exclusions.max_file_size_mb {
        ctx.ui.line(format!("  files larger than {mb} MB"));
    }

    Outcome::data(job)
}

fn job_add(ctx: &mut Ctx, daemon: &Daemon, args: JobAddArgs) -> CliResult<Outcome> {
    let existing = jobs(daemon)?;
    if existing.iter().any(|j| j.name == args.name) {
        return Err(CliError::usage(format!("there is already a job called {}", args.name)));
    }

    let all_destinations = destinations(daemon)?;
    let chosen = resolve::many(&args.destinations, &all_destinations, resolve::Kind::Destination)?;

    let schedule = match &args.schedule {
        Some(spec) => schedule::parse(spec)?,
        None => superbackup_core::model::Schedule::default(),
    };

    let mut exclusions = template_exclusions(args.template);
    exclusions.patterns.extend(args.excludes.iter().cloned());

    let project_id = match &args.project {
        Some(project) => Some(Uuid::parse_str(project).map_err(|_| {
            CliError::unsupported(
                "--project by name",
                "the running instance exposes no command that lists projects",
            )
        })?),
        None => None,
    };

    let job = Job {
        // Replaced by the daemon; sent so the object is complete.
        id: Uuid::new_v4(),
        name: args.name.clone(),
        project_id,
        description: args.description.clone().unwrap_or_default(),
        sources: args.sources.iter().map(|p| Source::new(absolute(p))).collect(),
        destination_ids: chosen.iter().map(|d| d.id).collect(),
        schedule,
        exclusions,
        bandwidth: None,
        retention: None,
        enabled: !args.disabled,
        timeout_minutes: None,
        hooks: Default::default(),
        continue_on_destination_error: true,
        created_at: Utc::now(),
        tags: Vec::new(),
    };

    let created = *reply!(daemon, Request::JobCreate { job: Box::new(job) }, Job)?.job;
    ctx.ui.line(format!("Created {}.", created.name));
    ctx.ui.line(format!("  Schedule      {}", schedule::describe(&created.schedule)));
    for source in &created.sources {
        ctx.ui.line(format!("  Backs up      {}", source.path.display()));
    }
    if created.destination_ids.is_empty() {
        // Silence here would let somebody believe they are protected.
        ctx.ui.warn(
            "this job has no destination, so it will not write a backup anywhere. Attach one \
             with `superbackup job edit NAME --add-destination DEST`.",
        );
    } else {
        for dest in &chosen {
            ctx.ui.line(format!("  Writes to     {}", dest.name));
        }
    }
    Outcome::data(created)
}

fn template_exclusions(template: JobTemplate) -> ExclusionSet {
    use superbackup_core::model::ExclusionPreset::*;
    match template {
        // The reason this application exists.
        JobTemplate::Developer => ExclusionSet::developer_defaults(),
        JobTemplate::Documents => ExclusionSet {
            presets: vec![OsJunk, LogsAndTemp],
            respect_cachedir_tag: true,
            ..Default::default()
        },
        JobTemplate::Everything => ExclusionSet::default(),
    }
}

fn job_edit(ctx: &mut Ctx, daemon: &Daemon, args: JobEditArgs) -> CliResult<Outcome> {
    let mut job = resolve_job(daemon, &args.job)?;
    let all_destinations = destinations(daemon)?;
    let mut changes: Vec<String> = Vec::new();

    if let Some(name) = &args.name {
        changes.push(format!("renamed to {name}"));
        job.name = name.clone();
    }
    for path in &args.add_sources {
        let path = absolute(path);
        if !job.sources.iter().any(|s| s.path == path) {
            changes.push(format!("added source {}", path.display()));
            job.sources.push(Source::new(path));
        }
    }
    for path in &args.remove_sources {
        let path = absolute(path);
        let before = job.sources.len();
        job.sources.retain(|s| s.path != path);
        if job.sources.len() != before {
            changes.push(format!("removed source {}", path.display()));
        } else {
            return Err(CliError::usage(format!(
                "{} is not a source of {}",
                path.display(),
                job.name
            )));
        }
    }
    for needle in &args.add_destinations {
        let dest = resolve::one(needle, &all_destinations, resolve::Kind::Destination)?;
        if !job.destination_ids.contains(&dest.id) {
            changes.push(format!("added destination {}", dest.name));
            job.destination_ids.push(dest.id);
        }
    }
    for needle in &args.remove_destinations {
        let dest = resolve::one(needle, &all_destinations, resolve::Kind::Destination)?;
        let before = job.destination_ids.len();
        job.destination_ids.retain(|id| *id != dest.id);
        if job.destination_ids.len() != before {
            changes.push(format!("removed destination {}", dest.name));
        }
    }
    if let Some(spec) = &args.schedule {
        let parsed = schedule::parse(spec)?;
        changes.push(format!("schedule is now {}", schedule::describe(&parsed)));
        job.schedule = parsed;
    }
    for pattern in &args.add_excludes {
        if !job.exclusions.patterns.contains(pattern) {
            changes.push(format!("excluding {pattern}"));
            job.exclusions.patterns.push(pattern.clone());
        }
    }
    for pattern in &args.remove_excludes {
        let before = job.exclusions.patterns.len();
        job.exclusions.patterns.retain(|p| p != pattern);
        if job.exclusions.patterns.len() != before {
            changes.push(format!("no longer excluding {pattern}"));
        } else {
            return Err(CliError::usage(format!(
                "{pattern} is not one of this job's own exclusion patterns"
            ))
            .with_hint("Patterns that come from a template are shown as presets, not patterns."));
        }
    }
    if let Some(kbps) = args.upload_limit {
        let mut bandwidth = job.bandwidth.clone().unwrap_or_default();
        bandwidth.upload_kbps = (kbps > 0).then_some(kbps);
        changes.push(if kbps > 0 {
            format!("upload limit {kbps} KB/s")
        } else {
            "upload limit removed".to_string()
        });
        job.bandwidth = if bandwidth.is_unlimited() { None } else { Some(bandwidth) };
    }
    if let Some(minutes) = args.timeout_minutes {
        job.timeout_minutes = (minutes > 0).then_some(minutes);
        changes.push(match job.timeout_minutes {
            Some(m) => format!("timeout {m} minutes"),
            None => "timeout removed".to_string(),
        });
    }

    if changes.is_empty() {
        return Err(CliError::usage(format!("nothing to change on {}", job.name))
            .with_hint("Pass one of --name, --add-source, --add-destination, --schedule, ..."));
    }

    let updated = *reply!(daemon, Request::JobUpdate { job: Box::new(job) }, Job)?.job;
    ctx.ui.line(format!("Updated {}:", updated.name));
    for change in &changes {
        ctx.ui.line(format!("  {change}"));
    }
    Outcome::data(updated)
}

fn job_remove(ctx: &mut Ctx, daemon: &Daemon, args: JobRemoveArgs) -> CliResult<Outcome> {
    let job = resolve_job(daemon, &args.job)?;
    prompt::confirm(
        ctx,
        &format!("Deleting the job {}. The backups it has already taken are not touched", job.name),
        args.yes,
    )?;
    reply!(daemon, Request::JobDelete { job: job.id.to_string() }, Ack)?;
    ctx.ui.line(format!("Deleted {}. Its snapshots are still where they were.", job.name));
    Outcome::data(serde_json::json!({ "deleted": job.id, "name": job.name }))
}

fn job_enabled(ctx: &mut Ctx, daemon: &Daemon, needle: &str, enabled: bool) -> CliResult<Outcome> {
    let job = resolve_job(daemon, needle)?;
    let updated =
        *reply!(daemon, Request::JobSetEnabled { job: job.id.to_string(), enabled }, Job)?.job;
    if enabled {
        ctx.ui.line(format!(
            "{} will run {}.",
            updated.name,
            schedule::describe(&updated.schedule)
        ));
    } else {
        ctx.ui.line(format!("{} will not run on its schedule. Nothing was deleted.", updated.name));
    }
    Outcome::data(updated)
}

fn job_preview(_ctx: &mut Ctx, daemon: &Daemon, args: JobPreviewArgs) -> CliResult<Outcome> {
    // Resolve first, so a typo is reported as a typo rather than as a missing
    // feature.
    let job = resolve_job(daemon, &args.job)?;
    Err(CliError::unsupported(
        &format!("previewing {}", job.name),
        "the running instance has no command that walks a job's sources without backing them up",
    )
    .with_hint("`superbackup run NAME --dry-run` reports what a real run would copy."))
}

// ---------------------------------------------------------------------------
// destination
// ---------------------------------------------------------------------------

pub fn destination(ctx: &mut Ctx, command: DestinationCommand) -> CliResult<Outcome> {
    let daemon = Daemon::connect(ctx, Start::Never)?;
    match command {
        DestinationCommand::List => destination_list(ctx, &daemon),
        DestinationCommand::Show { destination } => destination_show(ctx, &daemon, &destination),
        DestinationCommand::Add(args) => destination_add(ctx, &daemon, args),
        DestinationCommand::Edit(args) => destination_edit(ctx, &daemon, args),
        DestinationCommand::Remove(args) => destination_remove(ctx, &daemon, args),
        DestinationCommand::Test { destination } => destination_test(ctx, &daemon, &destination),
        DestinationCommand::Connect { destination } => {
            destination_connect(ctx, &daemon, &destination)
        }
        DestinationCommand::Stats { destination } => destination_stats(ctx, &daemon, &destination),
        DestinationCommand::Maintain(args) => destination_maintain(&daemon, args),
        DestinationCommand::Machines { destination } => destination_machines(&daemon, &destination),
    }
}

fn destination_list(ctx: &mut Ctx, daemon: &Daemon) -> CliResult<Outcome> {
    let all = destinations(daemon)?;
    let all_jobs = jobs(daemon)?;
    let now = Utc::now();

    let mut table = Table::new(vec![
        Column::new("destination").flex(),
        Column::new("kind"),
        Column::new("location").path(),
        Column::new("jobs").right(),
        Column::new("enabled"),
        Column::new("last checked"),
    ])
    .empty_note("No destinations yet. Add one with `superbackup destination add --local PATH`.");

    for dest in &all {
        let used = all_jobs.iter().filter(|j| j.destination_ids.contains(&dest.id)).count();
        table.push(vec![
            Cell::new(dest.name.clone()),
            Cell::new(dest.kind.label()),
            Cell::new(location_of(&dest.kind)),
            Cell::new(used.to_string()),
            if dest.enabled { Cell::new("yes") } else { Cell::coloured("no", Colour::Dim) },
            Cell::new(format::opt_relative(dest.last_verified_at, now)),
        ]);
    }
    ctx.ui.table(&table);
    if !all.is_empty() {
        // Probing every destination on a plain listing would make `ls` do
        // network I/O. Say where the answer lives instead.
        ctx.ui.note("Reachability is only as fresh as the last check. Run `superbackup destination test NAME` to check now.");
    }
    Outcome::data(all)
}

fn location_of(kind: &DestinationKind) -> String {
    match kind {
        DestinationKind::LocalRepository { path }
        | DestinationKind::OneDrive { path, .. }
        | DestinationKind::LocalMirror { path } => path.display().to_string(),
        DestinationKind::S3 { bucket, prefix, .. } => format!("s3://{bucket}/{prefix}"),
    }
}

fn destination_show(ctx: &mut Ctx, daemon: &Daemon, needle: &str) -> CliResult<Outcome> {
    let dest = resolve_destination(daemon, needle)?;
    let pad = 20;
    ctx.ui.heading(&dest.name);
    ctx.ui.field("Id", dest.id.to_string(), pad);
    ctx.ui.field("Kind", dest.kind.label(), pad);
    ctx.ui.field("Location", location_of(&dest.kind), pad);
    ctx.ui.field("Enabled", if dest.enabled { "yes" } else { "no" }, pad);
    ctx.ui.field("Created", format::timestamp_local(dest.created_at), pad);
    ctx.ui.field("Last checked", format::opt_timestamp_local(dest.last_verified_at), pad);

    if let DestinationKind::S3 { provider_id, .. } = &dest.kind {
        let provider = providers(daemon)?.into_iter().find(|p| p.id == *provider_id);
        ctx.ui.field(
            "Provider",
            provider.map(|p| p.name).unwrap_or_else(|| format!("{provider_id} (missing)")),
            pad,
        );
    }

    ctx.ui.blank();
    match &dest.encryption {
        Some(encryption) => {
            ctx.ui.heading("Encryption");
            ctx.ui.field("Algorithm", encryption.algorithm.kopia_id(), pad);
            ctx.ui.field("Hash", encryption.hash.kopia_id(), pad);
            ctx.ui.field("Splitter", encryption.splitter.kopia_id(), pad);
            ctx.ui.field("Passphrase", passphrase_source_text(encryption.passphrase_source), pad);
        }
        None => {
            ctx.ui.heading("Encryption");
            ctx.ui.line("  None. Anyone who can read the folder can read your files.");
        }
    }

    ctx.ui.blank();
    ctx.ui.heading("Retention");
    let r: &RetentionPolicy = &dest.retention;
    ctx.ui.line(format!(
        "  keep {} latest, {} hourly, {} daily, {} weekly, {} monthly, {} annual",
        r.keep_latest, r.keep_hourly, r.keep_daily, r.keep_weekly, r.keep_monthly, r.keep_annual
    ));
    Outcome::data(dest)
}

fn passphrase_source_text(source: PassphraseSource) -> &'static str {
    match source {
        PassphraseSource::Generated => "generated, held in the vault",
        PassphraseSource::UserSupplied => "typed by you, held in the vault",
        PassphraseSource::DerivedFromMaster => "derived from your master passphrase",
    }
}

fn destination_add(ctx: &mut Ctx, daemon: &Daemon, args: DestinationAddArgs) -> CliResult<Outcome> {
    let snapshot = *reply!(daemon, Request::Status {}, Status)?.snapshot;
    let kind = destination_kind(ctx, daemon, &args, &snapshot.machine_slug)?;

    let name = match &args.name {
        Some(name) => name.clone(),
        None => default_name(&kind),
    };
    let existing = destinations(daemon)?;
    if existing.iter().any(|d| d.name == name) {
        return Err(CliError::usage(format!("there is already a destination called {name}"))
            .with_hint("Give this one a name with --name."));
    }

    let is_repository = kind.is_repository();
    let encryption = if is_repository { Some(encryption_settings(&args)?) } else { None };
    if !is_repository {
        for (flag, given) in [
            ("--encryption", &args.encryption),
            ("--hash", &args.hash),
            ("--splitter", &args.splitter),
        ] {
            if given.is_some() {
                return Err(CliError::usage(format!(
                    "{flag} has no meaning for a folder mirror, which is a plain unencrypted copy"
                )));
            }
        }
    }

    let destination = Destination {
        id: Uuid::new_v4(),
        name: name.clone(),
        kind,
        encryption,
        passphrase_ref: None,
        retention: RetentionPolicy::default(),
        enabled: true,
        auto_discovered: false,
        bandwidth: None,
        replicate_from: None,
        created_at: Utc::now(),
        last_verified_at: None,
    };

    let mut created = *reply!(
        daemon,
        Request::DestinationCreate { destination: Box::new(destination) },
        Destination
    )?
    .destination;
    ctx.ui.line(format!("Added {} ({}).", created.name, created.kind.label()));

    // A user-supplied repository passphrase can only be stored once the daemon
    // has assigned the destination its id, because the vault handle is derived
    // from it.
    if is_repository && args.passphrase == PassphraseMode::Prompt {
        let secret = prompt::new_passphrase(
            ctx,
            "Repository passphrase: ",
            "Repeat the repository passphrase: ",
        )?;
        let secret_ref = SecretRef::new("repo.passphrase", &created.id);
        reply!(
            daemon,
            Request::VaultSetSecret {
                secret_ref: secret_ref.clone(),
                value: SecretString::new(secret),
            },
            Ack
        )?;
        created.passphrase_ref = Some(secret_ref);
        if let Some(encryption) = &mut created.encryption {
            encryption.passphrase_source = PassphraseSource::UserSupplied;
        }
        created = *reply!(
            daemon,
            Request::DestinationUpdate { destination: Box::new(created) },
            Destination
        )?
        .destination;
    }

    if !is_repository {
        ctx.ui.line(
            "A folder mirror is a plain copy: no repository, no deduplication, and no encryption.",
        );
        return Outcome::data(created);
    }

    let repo = if args.connect_existing {
        ctx.ui.note("Connecting to the repository that is already there.");
        reply!(
            daemon,
            Request::DestinationRepoConnect { destination: created.id.to_string() },
            Repository
        )?
    } else {
        ctx.ui.note("Creating the repository. This can take a moment.");
        reply!(
            daemon,
            Request::DestinationRepoCreate {
                destination: created.id.to_string(),
                encryption: created.encryption.clone(),
            },
            Repository
        )?
    };

    if repo.created {
        ctx.ui.line("Created the repository and connected to it.");
    } else if repo.connected {
        ctx.ui.line("Connected to the existing repository.");
    }
    Outcome::data(serde_json::json!({ "destination": created, "repository": repo }))
}

fn destination_kind(
    ctx: &mut Ctx,
    daemon: &Daemon,
    args: &DestinationAddArgs,
    machine_slug: &str,
) -> CliResult<DestinationKind> {
    if let Some(path) = &args.local {
        return Ok(DestinationKind::LocalRepository { path: absolute(path) });
    }
    if let Some(path) = &args.mirror {
        return Ok(DestinationKind::LocalMirror { path: absolute(path) });
    }
    if let Some(account) = &args.onedrive {
        return onedrive_kind(ctx, account);
    }
    if let Some(provider_needle) = &args.s3 {
        let provider = resolve_provider(daemon, provider_needle)?;
        // clap enforces `--s3` requires `--bucket`; this keeps the invariant
        // local rather than trusting it from two files away.
        let bucket = args.bucket.clone().ok_or_else(|| {
            CliError::usage("--s3 needs --bucket to say which bucket to write into")
        })?;
        let prefix = match &args.prefix {
            Some(p) => normalise_prefix(p),
            None => default_s3_prefix(machine_slug),
        };
        return Ok(DestinationKind::S3 {
            provider_id: provider.id,
            bucket,
            prefix,
            credential_override: None,
        });
    }
    Err(CliError::usage("say where the backups go").with_hint(
        "Pass one of --local PATH, --onedrive, --s3 PROVIDER --bucket NAME, or --mirror PATH.",
    ))
}

/// OneDrive discovery runs in this process rather than over IPC.
///
/// There is no `onedrive.detect` command in the protocol, and looking for a
/// folder on the local disk touches neither a repository nor the vault, so the
/// thin-client rule is not bent by doing it here. The daemon may be running as
/// a service, which cannot see the user's OneDrive at all — so this is also
/// the only process that *can* answer.
fn onedrive_kind(ctx: &mut Ctx, wanted: &str) -> CliResult<DestinationKind> {
    let accounts = superbackup_core::platform::onedrive::detect();
    if accounts.is_empty() {
        return Err(CliError::new(
            ErrorCode::Validation,
            "no OneDrive folder was found on this machine",
        )
        .with_hint("Sign in to OneDrive, or use `--local PATH` to point at the folder yourself."));
    }
    let account = if wanted.is_empty() {
        if accounts.len() > 1 {
            let names: Vec<&str> = accounts.iter().map(|a| a.display_name.as_str()).collect();
            return Err(CliError::usage(format!(
                "this machine has {} OneDrive accounts: {}",
                accounts.len(),
                names.join(", ")
            ))
            .with_hint("Name the one you want: --onedrive \"<account>\"."));
        }
        &accounts[0]
    } else {
        let lower = wanted.to_lowercase();
        let matches: Vec<_> =
            accounts.iter().filter(|a| a.display_name.to_lowercase().contains(&lower)).collect();
        match matches.len() {
            1 => matches[0],
            0 => {
                let names: Vec<&str> = accounts.iter().map(|a| a.display_name.as_str()).collect();
                return Err(CliError::usage(format!(
                    "no OneDrive account matching `{wanted}`; this machine has: {}",
                    names.join(", ")
                )));
            }
            _ => {
                let names: Vec<&str> = matches.iter().map(|a| a.display_name.as_str()).collect();
                return Err(CliError::usage(format!(
                    "`{wanted}` matches {} OneDrive accounts: {}",
                    matches.len(),
                    names.join(", ")
                )));
            }
        }
    };

    let path = account.suggested_repository_root();
    ctx.ui.line(format!("Using the OneDrive folder at {}.", path.display()));
    Ok(account.to_destination_kind(path))
}

fn default_name(kind: &DestinationKind) -> String {
    match kind {
        DestinationKind::LocalRepository { path } | DestinationKind::LocalMirror { path } => path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "local".to_string()),
        DestinationKind::OneDrive { account, .. } => {
            account.clone().unwrap_or_else(|| "onedrive".to_string())
        }
        DestinationKind::S3 { bucket, .. } => bucket.clone(),
    }
}

fn encryption_settings(args: &DestinationAddArgs) -> CliResult<EncryptionSettings> {
    let mut settings = EncryptionSettings::default();
    if let Some(name) = &args.encryption {
        settings.algorithm = *EncryptionAlgorithm::all()
            .iter()
            .find(|a| a.kopia_id().eq_ignore_ascii_case(name))
            .ok_or_else(|| {
                choice_error(
                    "--encryption",
                    name,
                    EncryptionAlgorithm::all().iter().map(|a| a.kopia_id()),
                )
            })?;
    }
    if let Some(name) = &args.hash {
        settings.hash = *HashAlgorithm::all()
            .iter()
            .find(|h| h.kopia_id().eq_ignore_ascii_case(name))
            .ok_or_else(|| {
                choice_error("--hash", name, HashAlgorithm::all().iter().map(|h| h.kopia_id()))
            })?;
    }
    if let Some(name) = &args.splitter {
        settings.splitter =
            *Splitter::all().iter().find(|s| s.kopia_id().eq_ignore_ascii_case(name)).ok_or_else(
                || choice_error("--splitter", name, Splitter::all().iter().map(|s| s.kopia_id())),
            )?;
    }
    settings.passphrase_source = match args.passphrase {
        PassphraseMode::Generated => PassphraseSource::Generated,
        PassphraseMode::Prompt => PassphraseSource::UserSupplied,
        PassphraseMode::Derived => PassphraseSource::DerivedFromMaster,
    };
    Ok(settings)
}

fn choice_error<'a>(flag: &str, given: &str, allowed: impl Iterator<Item = &'a str>) -> CliError {
    let allowed: Vec<&str> = allowed.collect();
    CliError::usage(format!("`{given}` is not a value {flag} accepts"))
        .with_hint(format!("Accepted: {}.", allowed.join(", ")))
}

fn destination_edit(
    ctx: &mut Ctx,
    daemon: &Daemon,
    args: DestinationEditArgs,
) -> CliResult<Outcome> {
    let mut dest = resolve_destination(daemon, &args.destination)?;
    let mut changes = Vec::new();

    if let Some(name) = &args.name {
        changes.push(format!("renamed to {name}"));
        dest.name = name.clone();
    }
    if let Some(prefix) = &args.prefix {
        match &mut dest.kind {
            DestinationKind::S3 { prefix: current, .. } => {
                *current = normalise_prefix(prefix);
                changes.push(format!("prefix is now {current}"));
            }
            other => {
                return Err(CliError::usage(format!(
                    "--prefix applies to a bucket, and {} is a {}",
                    dest.name,
                    other.label().to_lowercase()
                )))
            }
        }
    }
    if let Some(kbps) = args.upload_limit {
        let mut bandwidth = dest.bandwidth.clone().unwrap_or_default();
        bandwidth.upload_kbps = (kbps > 0).then_some(kbps);
        changes.push(if kbps > 0 {
            format!("upload limit {kbps} KB/s")
        } else {
            "upload limit removed".to_string()
        });
        dest.bandwidth = if bandwidth.is_unlimited() { None } else { Some(bandwidth) };
    }
    if args.enable {
        dest.enabled = true;
        changes.push("enabled".to_string());
    }
    if args.disable {
        dest.enabled = false;
        changes.push("disabled".to_string());
    }
    if changes.is_empty() {
        return Err(CliError::usage(format!("nothing to change on {}", dest.name)));
    }

    let updated =
        *reply!(daemon, Request::DestinationUpdate { destination: Box::new(dest) }, Destination)?
            .destination;
    ctx.ui.line(format!("Updated {}:", updated.name));
    for change in &changes {
        ctx.ui.line(format!("  {change}"));
    }
    Outcome::data(updated)
}

fn destination_remove(
    ctx: &mut Ctx,
    daemon: &Daemon,
    args: DestinationRemoveArgs,
) -> CliResult<Outcome> {
    let dest = resolve_destination(daemon, &args.destination)?;
    let users: Vec<String> = jobs(daemon)?
        .into_iter()
        .filter(|j| j.destination_ids.contains(&dest.id))
        .map(|j| j.name)
        .collect();

    // A mirror has no repository, and telling somebody their repository is
    // safe when they are deleting a folder copy would be an odd reassurance.
    let untouched = if dest.kind.is_repository() {
        "The repository and everything in it stay where they are"
    } else {
        "The copied files stay where they are"
    };
    let mut what = format!("Removing {} from the configuration. {untouched}", dest.name);
    if !users.is_empty() {
        what.push_str(&format!(
            ". {} here: {}",
            if users.len() == 1 {
                "One job still writes".to_string()
            } else {
                format!("{} jobs still write", users.len())
            },
            users.join(", ")
        ));
    }
    prompt::confirm(ctx, &what, args.yes)?;

    reply!(
        daemon,
        Request::DestinationDelete { destination: dest.id.to_string(), force: false },
        Ack
    )?;
    ctx.ui.line(format!("Removed {}. Nothing stored there was deleted.", dest.name));
    Outcome::data(serde_json::json!({ "removed": dest.id, "name": dest.name }))
}

fn destination_test(ctx: &mut Ctx, daemon: &Daemon, needle: &str) -> CliResult<Outcome> {
    let dest = resolve_destination(daemon, needle)?;
    ctx.ui.note(format!("Checking {}...", dest.name));
    let probe =
        reply!(daemon, Request::DestinationTest { destination: dest.id.to_string() }, Probe)?;

    let latency = probe.latency_ms.map(|ms| format!(" in {ms} ms")).unwrap_or_default();
    if probe.reachable && probe.writable {
        ctx.ui.coloured(Colour::Green, &format!("{}: reachable and writable{latency}.", dest.name));
    } else if probe.reachable {
        // Readable but not writable fails every backup, and finding that out
        // at 2am is exactly what this check exists to prevent.
        ctx.ui.coloured(
            Colour::Yellow,
            &format!("{}: reachable but NOT writable{latency}. Backups here will fail.", dest.name),
        );
    } else {
        ctx.ui.coloured(Colour::Red, &format!("{} could not be reached.", dest.name));
    }
    if let Some(detail) = &probe.detail {
        ctx.ui.line(format!("  {detail}"));
    }

    // A failed probe is a successful question with a negative answer, so the
    // envelope says ok and the exit code says the check did not pass.
    if probe.reachable && probe.writable {
        Outcome::data(probe)
    } else {
        Outcome::negative(probe)
    }
}

fn destination_connect(ctx: &mut Ctx, daemon: &Daemon, needle: &str) -> CliResult<Outcome> {
    let dest = resolve_destination(daemon, needle)?;
    let repo = reply!(
        daemon,
        Request::DestinationRepoConnect { destination: dest.id.to_string() },
        Repository
    )?;
    if repo.connected {
        ctx.ui.line(format!("Connected to the repository at {}.", dest.name));
    } else {
        ctx.ui.line(format!("{} is not connected.", dest.name));
    }
    Outcome::data(repo)
}

fn destination_stats(ctx: &mut Ctx, daemon: &Daemon, needle: &str) -> CliResult<Outcome> {
    let dest = resolve_destination(daemon, needle)?;
    let stats = reply!(
        daemon,
        Request::DestinationStats { destination: dest.id.to_string(), refresh: false },
        StorageStats
    )?;

    let pad = 18;
    ctx.ui.heading(&dest.name);
    ctx.ui.field("Snapshots", stats.snapshot_count.to_string(), pad);
    ctx.ui.field("Logical size", format::opt_bytes(stats.logical_bytes), pad);
    ctx.ui.field("Stored", format::opt_bytes(stats.stored_bytes), pad);
    if let (Some(logical), Some(stored)) = (stats.logical_bytes, stats.stored_bytes) {
        if stored > 0 {
            ctx.ui.field("Saved by dedup", format!("{:.1}x", logical as f64 / stored as f64), pad);
        }
    }
    ctx.ui.field("Newest snapshot", format::opt_timestamp_local(stats.last_snapshot_at), pad);
    ctx.ui.field("Figures from", format::relative(stats.computed_at, Utc::now()), pad);
    Outcome::data(stats)
}

fn destination_maintain(daemon: &Daemon, args: DestinationMaintainArgs) -> CliResult<Outcome> {
    let dest = resolve_destination(daemon, &args.destination)?;
    Err(CliError::unsupported(
        &format!("maintenance on {}", dest.name),
        "the running instance has no command that runs kopia maintenance on demand",
    )
    .with_hint("The daemon runs maintenance itself after the configured number of snapshots."))
}

fn destination_machines(daemon: &Daemon, needle: &str) -> CliResult<Outcome> {
    let dest = resolve_destination(daemon, needle)?;
    Err(CliError::unsupported(
        &format!("listing the machines writing to {}", dest.name),
        "the running instance has no command that reads the destination's machine manifests",
    ))
}

// ---------------------------------------------------------------------------
// provider
// ---------------------------------------------------------------------------

pub fn provider(ctx: &mut Ctx, command: ProviderCommand) -> CliResult<Outcome> {
    let daemon = Daemon::connect(ctx, Start::Never)?;
    match command {
        ProviderCommand::List => provider_list(ctx, &daemon),
        ProviderCommand::Show { provider } => provider_show(ctx, &daemon, &provider),
        ProviderCommand::Add(args) => provider_add(ctx, &daemon, args),
        ProviderCommand::Edit(args) => provider_edit(ctx, &daemon, args),
        ProviderCommand::Remove(args) => provider_remove(ctx, &daemon, args),
        ProviderCommand::Test { provider } => provider_test(ctx, &daemon, &provider),
        ProviderCommand::Rotate { provider } => provider_rotate(ctx, &daemon, &provider),
        ProviderCommand::UsedBy { provider } => provider_used_by(ctx, &daemon, &provider),
    }
}

fn provider_list(ctx: &mut Ctx, daemon: &Daemon) -> CliResult<Outcome> {
    let all = providers(daemon)?;
    let all_destinations = destinations(daemon)?;
    let mut table = Table::new(vec![
        Column::new("provider").flex(),
        Column::new("endpoint").flex(),
        Column::new("region"),
        Column::new("destinations").right(),
    ])
    .empty_note("No storage providers yet. Add one with `superbackup provider add --name NAME`.");

    for provider in &all {
        let used =
            all_destinations.iter().filter(|d| d.kind.provider_id() == Some(&provider.id)).count();
        let (endpoint, region) = match &provider.kind {
            ProviderKind::S3 { endpoint, region, .. } => (endpoint.clone(), region.clone()),
        };
        table.push(vec![
            Cell::new(provider.name.clone()),
            Cell::new(endpoint),
            Cell::new(region),
            Cell::new(used.to_string()),
        ]);
    }
    ctx.ui.table(&table);
    Outcome::data(all)
}

fn provider_show(ctx: &mut Ctx, daemon: &Daemon, needle: &str) -> CliResult<Outcome> {
    let provider = resolve_provider(daemon, needle)?;
    let pad = 18;
    ctx.ui.heading(&provider.name);
    ctx.ui.field("Id", provider.id.to_string(), pad);
    match &provider.kind {
        ProviderKind::S3 { endpoint, region, tls, flavour, credentials, .. } => {
            ctx.ui.field("Service", flavour.title(), pad);
            ctx.ui.field("Endpoint", endpoint, pad);
            ctx.ui.field("Region", region, pad);
            ctx.ui.field("TLS", if *tls { "yes" } else { "no" }, pad);
            // Handles only. There is no request that returns a secret, and
            // there will not be one.
            ctx.ui.field("Access key", credentials.access_key_ref.as_str(), pad);
            ctx.ui.field("Secret key", credentials.secret_key_ref.as_str(), pad);
        }
    }
    ctx.ui.field("Last checked", format::opt_timestamp_local(provider.last_verified_at), pad);
    ctx.ui.note("Credentials are stored in the vault and are never printed.");
    Outcome::data(provider)
}

fn provider_add(ctx: &mut Ctx, daemon: &Daemon, args: ProviderAddArgs) -> CliResult<Outcome> {
    if providers(daemon)?.iter().any(|p| p.name == args.name) {
        return Err(CliError::usage(format!("there is already a provider called {}", args.name)));
    }

    let flavour = flavour_of(args.flavour);
    let endpoint = args
        .endpoint
        .clone()
        .or_else(|| flavour.default_endpoint().map(|s| s.to_string()))
        .ok_or_else(|| {
            CliError::usage(format!("{} has no default endpoint", flavour.title()))
                .with_hint("Pass --endpoint https://...")
        })?;
    let region = args
        .region
        .clone()
        .or_else(|| flavour.default_region().map(|s| s.to_string()))
        .unwrap_or_default();

    let id = Uuid::new_v4();
    let provider = StorageProvider {
        id,
        name: args.name.clone(),
        kind: ProviderKind::S3 {
            endpoint,
            region,
            credentials: S3Credentials::for_provider(&id),
            tls: true,
            path_style: flavour.wants_path_style(),
            flavour,
        },
        notes: String::new(),
        created_at: Utc::now(),
        last_verified_at: None,
    };

    let created =
        *reply!(daemon, Request::ProviderCreate { provider: Box::new(provider) }, Provider)?
            .provider;

    // The daemon assigns the id, so the credential handles have to be rebuilt
    // around the id it chose rather than the one this process invented.
    let credentials = S3Credentials::for_provider(&created.id);
    let access_key = match &args.access_key {
        Some(key) => key.clone(),
        None => prompt::ask_line(ctx, "Access key id: ")?.trim().to_string(),
    };
    if access_key.is_empty() {
        return Err(CliError::usage("an access key id is needed"));
    }
    // The secret key is never taken from the command line, on any path.
    let secret_key = prompt::from_terminal(ctx, "Secret access key: ")?;

    reply!(
        daemon,
        Request::VaultSetSecret {
            secret_ref: credentials.access_key_ref.clone(),
            value: SecretString::from_string(access_key),
        },
        Ack
    )?;
    reply!(
        daemon,
        Request::VaultSetSecret {
            secret_ref: credentials.secret_key_ref.clone(),
            value: SecretString::new(secret_key),
        },
        Ack
    )?;

    ctx.ui.line(format!("Added {} and stored its credentials in the vault.", created.name));
    ctx.ui.line(format!("Check them with `superbackup provider test {}`.", created.name));
    Outcome::data(created)
}

fn flavour_of(flavour: ProviderFlavour) -> S3Flavour {
    match flavour {
        ProviderFlavour::Storj => S3Flavour::Storj,
        ProviderFlavour::Aws => S3Flavour::AwsS3,
        ProviderFlavour::BackblazeB2 => S3Flavour::BackblazeB2,
        ProviderFlavour::Wasabi => S3Flavour::Wasabi,
        ProviderFlavour::Minio => S3Flavour::MinIo,
        ProviderFlavour::Cloudflare => S3Flavour::Cloudflare,
        ProviderFlavour::Other => S3Flavour::Other,
    }
}

fn provider_edit(ctx: &mut Ctx, daemon: &Daemon, args: ProviderEditArgs) -> CliResult<Outcome> {
    let mut provider = resolve_provider(daemon, &args.provider)?;
    let mut changes = Vec::new();
    if let Some(name) = &args.name {
        changes.push(format!("renamed to {name}"));
        provider.name = name.clone();
    }
    match &mut provider.kind {
        ProviderKind::S3 { endpoint, region, .. } => {
            if let Some(new) = &args.endpoint {
                *endpoint = new.clone();
                changes.push(format!("endpoint is now {new}"));
            }
            if let Some(new) = &args.region {
                *region = new.clone();
                changes.push(format!("region is now {new}"));
            }
        }
    }
    if changes.is_empty() {
        return Err(CliError::usage(format!("nothing to change on {}", provider.name)));
    }
    let updated =
        *reply!(daemon, Request::ProviderUpdate { provider: Box::new(provider) }, Provider)?
            .provider;
    ctx.ui.line(format!("Updated {}:", updated.name));
    for change in &changes {
        ctx.ui.line(format!("  {change}"));
    }
    Outcome::data(updated)
}

fn provider_remove(ctx: &mut Ctx, daemon: &Daemon, args: ProviderRemoveArgs) -> CliResult<Outcome> {
    let provider = resolve_provider(daemon, &args.provider)?;
    let used =
        reply!(daemon, Request::ProviderUsedBy { provider: provider.id.to_string() }, UsedBy)?;

    let mut what =
        format!("Deleting the provider {} and forgetting its credentials", provider.name);
    if !used.destinations.is_empty() {
        let names: Vec<&str> = used.destinations.iter().map(|r| r.name.as_str()).collect();
        what.push_str(&format!(
            ". {} still use it and will stop working: {}",
            format::plural(used.destinations.len(), "destination", "destinations"),
            names.join(", ")
        ));
    }
    // `--force` makes this worse, not safer, so it is confirmed either way.
    prompt::confirm(ctx, &what, args.yes)?;

    reply!(
        daemon,
        Request::ProviderDelete { provider: provider.id.to_string(), force: args.force },
        Ack
    )?;
    ctx.ui.line(format!("Deleted {}.", provider.name));
    Outcome::data(serde_json::json!({ "removed": provider.id, "name": provider.name }))
}

fn provider_test(ctx: &mut Ctx, daemon: &Daemon, needle: &str) -> CliResult<Outcome> {
    let provider = resolve_provider(daemon, needle)?;
    ctx.ui.note(format!("Checking {}...", provider.name));
    let probe = reply!(daemon, Request::ProviderTest { provider: provider.id.to_string() }, Probe)?;
    if probe.reachable {
        ctx.ui.coloured(
            Colour::Green,
            &format!("{}: the endpoint answered and the credentials were accepted.", provider.name),
        );
    } else {
        ctx.ui.coloured(Colour::Red, &format!("{} could not be reached.", provider.name));
    }
    if let Some(detail) = &probe.detail {
        ctx.ui.line(format!("  {detail}"));
    }
    if probe.reachable {
        Outcome::data(probe)
    } else {
        Outcome::negative(probe)
    }
}

fn provider_rotate(ctx: &mut Ctx, daemon: &Daemon, needle: &str) -> CliResult<Outcome> {
    let provider = resolve_provider(daemon, needle)?;
    let used =
        reply!(daemon, Request::ProviderUsedBy { provider: provider.id.to_string() }, UsedBy)?;
    if !used.destinations.is_empty() {
        let names: Vec<&str> = used.destinations.iter().map(|r| r.name.as_str()).collect();
        ctx.ui.note(format!("These destinations inherit these credentials: {}.", names.join(", ")));
    }

    let access_key = prompt::ask_line(ctx, "New access key id: ")?.trim().to_string();
    if access_key.is_empty() {
        return Err(CliError::usage("an access key id is needed"));
    }
    let secret_key = prompt::from_terminal(ctx, "New secret access key: ")?;

    let updated = *reply!(
        daemon,
        Request::ProviderRotateCredentials {
            provider: provider.id.to_string(),
            access_key_id: SecretString::from_string(access_key),
            secret_access_key: SecretString::new(secret_key),
            session_token: None,
        },
        Provider
    )?
    .provider;
    ctx.ui.line(format!("Replaced the credentials for {}.", updated.name));
    ctx.ui.line(format!("Check them with `superbackup provider test {}`.", updated.name));
    Outcome::data(updated)
}

fn provider_used_by(ctx: &mut Ctx, daemon: &Daemon, needle: &str) -> CliResult<Outcome> {
    let provider = resolve_provider(daemon, needle)?;
    let used =
        reply!(daemon, Request::ProviderUsedBy { provider: provider.id.to_string() }, UsedBy)?;

    let mut table = Table::new(vec![Column::new("kind"), Column::new("name").flex()])
        .empty_note(format!("Nothing uses {}.", provider.name));
    for reference in &used.destinations {
        table.push(vec![Cell::new("destination"), Cell::new(reference.name.clone())]);
    }
    for reference in &used.jobs {
        table.push(vec![Cell::new("job"), Cell::new(reference.name.clone())]);
    }
    ctx.ui.table(&table);
    Outcome::data(used)
}

// ---------------------------------------------------------------------------
// project
// ---------------------------------------------------------------------------

/// Projects are part of the configuration model but the IPC surface has no
/// command for them: no list, no create, no delete. Rather than pretend, each
/// subcommand says exactly what is missing.
pub fn project(_ctx: &mut Ctx, command: ProjectCommand) -> CliResult<Outcome> {
    let what = match command {
        ProjectCommand::List => "listing projects",
        ProjectCommand::Add { .. } => "creating a project",
        ProjectCommand::Remove { .. } => "deleting a project",
    };
    Err(CliError::unsupported(what, "the running instance exposes no project commands over IPC")
        .with_hint("Group jobs with tags, or edit projects in the graphical interface."))
}
