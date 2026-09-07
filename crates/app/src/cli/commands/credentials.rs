//! The keys and tokens this machine signs in with, from the command line.
//!
//! Nothing here prints key material, and nothing here reads a private key: the
//! daemon does the reading, under the rules in
//! [`superbackup_core::credentials`], and the protocol has no reply that can
//! carry a secret back.
//!
//! `unseal` is the half of the key backup that matters on the bad day. It is
//! the command written into every key backup's RESTORE.txt, so it has to exist
//! and has to work with nothing but the bundle and the passphrase.

use superbackup_core::credentials::{Credential, CredentialKind};
use superbackup_core::ipc::protocol::{Request, SecretString};

use crate::cli::args::{CredCommand, CredFolderArgs, CredRoleArgs, CredUnsealArgs};
use crate::cli::client::{reply, Daemon, Start};
use crate::cli::context::Ctx;
use crate::cli::output::{CliResult, Outcome};
use crate::cli::prompt;
use crate::cli::format::{Cell, Column, Table};

pub fn cred(ctx: &mut Ctx, command: CredCommand) -> CliResult<Outcome> {
    let daemon = Daemon::connect(ctx, Start::Never)?;
    match command {
        CredCommand::List => list(ctx, &daemon),
        CredCommand::Role(args) => role(ctx, &daemon, args),
        CredCommand::Seal(args) => seal(ctx, &daemon, args),
        CredCommand::Unseal(args) => unseal(ctx, &daemon, args),
    }
}

fn list(ctx: &mut Ctx, daemon: &Daemon) -> CliResult<Outcome> {
    let found = reply!(daemon, Request::CredentialList {}, Credentials)?.credentials;

    let mut table = Table::new(vec![
        Column::new("name").flex(),
        Column::new("kind"),
        Column::new("protected"),
        Column::new("roles").flex(),
        Column::new("where").flex(),
    ])
    .empty_note("No SSH keys or tokens were found on this machine.");

    for credential in &found {
        table.push(vec![
            Cell::new(credential.name.clone()),
            Cell::new(credential.kind.label()),
            Cell::new(protection(credential)),
            Cell::new(roles(credential)),
            Cell::new(location(credential)),
        ]);
    }
    ctx.ui.table(&table);
    Outcome::data(found)
}

/// Whether the key on disk has a passphrase of its own.
///
/// Reported as "unknown" rather than guessed when the header could not be
/// read. A key wrongly shown as protected is a key someone leaves lying about.
fn protection(credential: &Credential) -> String {
    match &credential.kind {
        CredentialKind::SshKey(key) => match key.encrypted {
            Some(true) => "yes".to_string(),
            Some(false) => "no".to_string(),
            None => "unknown".to_string(),
        },
        _ => "-".to_string(),
    }
}

fn roles(credential: &Credential) -> String {
    match (credential.backed_up, credential.synced) {
        (true, true) => "backed up, shared".to_string(),
        (true, false) => "backed up".to_string(),
        (false, true) => "shared".to_string(),
        (false, false) => "-".to_string(),
    }
}

fn location(credential: &Credential) -> String {
    match &credential.kind {
        CredentialKind::SshKey(key) => key.private_path.display().to_string(),
        CredentialKind::ForgeToken { base_url, .. } => base_url.clone(),
        CredentialKind::GitHubCli { .. } => "the GitHub CLI".to_string(),
    }
}

fn role(ctx: &mut Ctx, daemon: &Daemon, args: CredRoleArgs) -> CliResult<Outcome> {
    reply!(
        daemon,
        Request::CredentialSetRole {
            path: args.path.clone(),
            backed_up: args.backup,
            synced: args.sync,
        },
        Ack
    )?;
    ctx.ui.line(match (args.backup, args.sync) {
        (true, true) => format!("{} is backed up and shared with your other machines.", args.path),
        (true, false) => format!("{} is included in the key backup.", args.path),
        (false, true) => format!("{} is shared with your other machines.", args.path),
        (false, false) => format!("{} is neither backed up nor shared.", args.path),
    });
    if args.backup {
        ctx.ui.note("The key backup job seals these under your master passphrase before writing.");
    }
    Outcome::data(serde_json::json!({
        "path": args.path,
        "backed_up": args.backup,
        "synced": args.sync,
    }))
}

fn seal(ctx: &mut Ctx, daemon: &Daemon, args: CredFolderArgs) -> CliResult<Outcome> {
    let secret =
        prompt::passphrase(ctx, args.passphrase_file.as_deref(), "Master passphrase: ")?;
    let sealed = reply!(
        daemon,
        Request::CredentialSealKeys {
            folder: args.to.display().to_string(),
            passphrase: SecretString::new(secret),
        },
        KeyBundle
    )?;

    ctx.ui.line(format!("{} keys sealed into {}.", sealed.files.len(), sealed.path));
    for name in &sealed.files {
        ctx.ui.line(format!("  {name}"));
    }
    ctx.ui.note("Opening it on another machine needs the same master passphrase.");
    Outcome::data(sealed)
}

fn unseal(ctx: &mut Ctx, daemon: &Daemon, args: CredUnsealArgs) -> CliResult<Outcome> {
    let secret = prompt::passphrase(
        ctx,
        args.passphrase_file.as_deref(),
        "Master passphrase the bundle was sealed under: ",
    )?;
    let opened = reply!(
        daemon,
        Request::CredentialUnsealKeys {
            folder: args.from.display().to_string(),
            overwrite: args.overwrite,
            passphrase: SecretString::new(secret),
        },
        KeyBundle
    )?;

    match &opened.from_machine {
        Some(machine) => ctx.ui.line(format!("Opened a bundle sealed on {machine}.")),
        None => ctx.ui.line("Opened the bundle."),
    }
    if opened.files.is_empty() {
        ctx.ui.line("Nothing new was written.");
    } else {
        ctx.ui.line(format!("{} key files written:", opened.files.len()));
        for name in &opened.files {
            ctx.ui.line(format!("  {name}"));
        }
    }
    if !opened.skipped.is_empty() {
        // Named rather than counted: "3 skipped" leaves the user unsure
        // whether the one they came for is among them.
        ctx.ui.warn(format!(
            "left alone because a file of that name is already here: {}. Use --overwrite to \
             replace them, having checked they are not the keys this machine is using.",
            opened.skipped.join(", ")
        ));
    }
    Outcome::data(opened)
}
