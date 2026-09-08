//! Unlocking, locking, and changing the master passphrase.
//!
//! Everything here sends a secret to the daemon and gets nothing secret back.
//! That is the protocol's rule, not this module's: there is no request that
//! returns credential material, so the CLI could not print a passphrase even
//! if somebody asked it to.

use superbackup_core::ipc::protocol::{Request, SecretString};

use crate::cli::args::{UnlockArgs, VaultCommand, VaultRestoreArgs};
use crate::cli::client::{reply, Daemon, Start};
use crate::cli::context::Ctx;
use crate::cli::format;
use crate::cli::output::{CliError, CliResult, Outcome};
use crate::cli::prompt;

pub fn unlock(ctx: &mut Ctx, args: UnlockArgs) -> CliResult<Outcome> {
    // Unlocking is the thing a user does *in order to* let backups run, so
    // starting the instance when none is running is what they asked for.
    let daemon = Daemon::connect(ctx, Start::IfNeeded)?;

    let already = reply!(daemon, Request::VaultIsUnlocked {}, Unlocked)?;
    if already.unlocked {
        ctx.ui.line("The vault is already unlocked.");
        report_auto_lock(ctx, &already);
        return Outcome::data(already);
    }

    if args.remember {
        // `vault.unlock` has no "remember" parameter; the switch that makes
        // the key survive is the `use_os_keychain` setting, so set it first
        // and say plainly what that means.
        let mut settings = *reply!(daemon, Request::SettingsGet {}, Settings)?.settings;
        if !settings.use_os_keychain {
            settings.use_os_keychain = true;
            reply!(daemon, Request::SettingsUpdate { settings: Box::new(settings) }, Settings)?;
            ctx.ui.warn(
                "the master key will be cached in this machine's keychain so the service can \
                 run unattended. Anything that can read your keychain can now read your \
                 backups. Turn it off with `superbackup config set use_os_keychain false`.",
            );
        }
    }

    let secret = prompt::passphrase(ctx, args.passphrase_file.as_deref(), "Master passphrase: ")?;
    let unlocked =
        reply!(daemon, Request::VaultUnlock { passphrase: SecretString::new(secret) }, Unlocked)?;

    if unlocked.unlocked {
        ctx.ui.line("The vault is unlocked. Scheduled backups can run.");
        report_auto_lock(ctx, &unlocked);
    } else {
        // The daemon answered without an error but did not open the vault.
        ctx.ui.line("The vault is still locked.");
    }
    Outcome::data(unlocked)
}

fn report_auto_lock(ctx: &mut Ctx, reply: &superbackup_core::ipc::protocol::UnlockedReply) {
    if let Some(at) = reply.auto_lock_at {
        ctx.ui.line(format!(
            "It locks itself again {} ({}).",
            format::relative(at, chrono::Utc::now()),
            format::absolute_local(at)
        ));
    }
}

pub fn lock(ctx: &mut Ctx) -> CliResult<Outcome> {
    let daemon = Daemon::connect(ctx, Start::Never)?;
    let locked = reply!(daemon, Request::VaultLock {}, Unlocked)?;
    if locked.unlocked {
        ctx.ui.line("The vault is still unlocked.");
    } else {
        ctx.ui.line("The vault is locked. Scheduled backups will not run until you unlock it.");
    }
    Outcome::data(locked)
}

pub fn change_passphrase(ctx: &mut Ctx) -> CliResult<Outcome> {
    let daemon = Daemon::connect(ctx, Start::Never)?;

    // There is deliberately no `--passphrase-file` on this command: it needs
    // two different secrets, and a file cannot say which is which without
    // inventing a format nobody would remember.
    let current = prompt::from_terminal(ctx, "Current master passphrase: ")?;
    ctx.ui.note("There is no recovery if the new passphrase is lost. Write it down.");
    let replacement = prompt::new_passphrase(
        ctx,
        "New master passphrase: ",
        "Repeat the new master passphrase: ",
    )?;

    reply!(
        daemon,
        Request::VaultChangePassphrase {
            current: SecretString::new(current),
            replacement: SecretString::new(replacement),
        },
        Ack
    )?;

    ctx.ui.line("The vault was re-sealed under the new passphrase. Every stored secret is intact.");
    ctx.ui.line("Other machines sharing this vault need the new passphrase too.");
    Outcome::data(serde_json::json!({ "changed": true }))
}

// ---------------------------------------------------------------------------
// Putting a vault back
// ---------------------------------------------------------------------------

pub fn vault(ctx: &mut Ctx, command: VaultCommand) -> CliResult<Outcome> {
    match command {
        VaultCommand::Restore(args) => restore(ctx, args),
        VaultCommand::Backups => backups(ctx),
    }
}

/// Put a vault from a backup into place.
///
/// # The order of the checks
///
/// This command replaces the file that opens every backup on the machine, so
/// the order it does things in is the design:
///
/// 1. **Refuse while an instance is running.** A running daemon holds the old
///    vault in memory and writes it back on the next change, so a restore
///    underneath one is silently undone — the worst possible outcome, because
///    it looks like it worked.
/// 2. **Open the incoming file with the passphrase, before touching
///    anything.** This proves two things at once: the file really is a vault,
///    and the person doing the restore can actually open it. Finding out
///    afterwards that the passphrase was the old one, having already replaced
///    the working vault, is the failure this ordering exists to prevent.
/// 3. **Keep the existing vault.** `replace_with` writes a dated copy into the
///    backups folder first, so a restore of the wrong file is recoverable.
fn restore(ctx: &mut Ctx, args: VaultRestoreArgs) -> CliResult<Outcome> {
    use superbackup_core::crypto::file::{BackupReason, VaultFile};
    use superbackup_core::error::ErrorCode;

    let source = locate(&args.from)?;

    // 1. Nothing may be holding the old vault open.
    if Daemon::connect(ctx, Start::Never).is_ok() {
        return Err(CliError::new(
            ErrorCode::Validation,
            "superbackup is running, and it would write its own vault back over the restored \
             one.",
        )
        .with_hint(
            "Quit superbackup (and `superbackup service stop`, if the service is installed), \
             then run this again.",
        ));
    }

    let bytes = std::fs::read(&source).map_err(|e| {
        CliError::new(ErrorCode::Io, format!("{} could not be read: {e}", source.display()))
    })?;

    // 2. It has to open before anything is replaced.
    let secret = prompt::passphrase(
        ctx,
        args.passphrase_file.as_deref(),
        "Master passphrase for the backed-up vault: ",
    )?;
    let restored = superbackup_core::crypto::Vault::unlock(&bytes, &secret).map_err(|_| {
        CliError::new(
            ErrorCode::BadPassphrase,
            format!("{} did not open with that passphrase.", source.display()),
        )
        .with_hint(
            "It is the passphrase that vault was sealed under, which is not necessarily this \
             machine's current one. Nothing has been changed.",
        )
    })?;
    let handles = restored.list_refs().map(|r| r.len()).unwrap_or(0);
    drop(restored);

    let existing = VaultFile::exists(&ctx.paths);
    ctx.ui.line(format!("{} opened. It holds {handles} stored secrets.", source.display()));
    if existing {
        ctx.ui.warn(format!(
            "this replaces the vault at {}. A dated copy of it is kept in {}.",
            ctx.paths.vault_file().display(),
            ctx.paths.vault_backup_dir().display()
        ));
    }
    prompt::confirm(ctx, "Replace this machine's vault", args.yes)?;

    // 3. Replace, keeping what was there.
    let kept = if existing {
        let mut file = VaultFile::load(&ctx.paths)?;
        file.replace_with(&bytes, BackupReason::Manual)?;
        file.latest_rekey_backup().ok().flatten()
    } else {
        ctx.paths.ensure()?;
        superbackup_core::paths::write_atomic(&ctx.paths.vault_file(), &bytes)?;
        superbackup_core::paths::harden_file(&ctx.paths.vault_file())?;
        None
    };

    let mut config_restored = false;
    if args.with_config {
        let from = source.with_file_name("config.json");
        if from.is_file() {
            let to = ctx.paths.config_file();
            let text = std::fs::read(&from).map_err(|e| {
                CliError::new(ErrorCode::Io, format!("{} could not be read: {e}", from.display()))
            })?;
            superbackup_core::paths::write_atomic(&to, &text)?;
            config_restored = true;
            ctx.ui.line(format!("Configuration restored to {}.", to.display()));
        } else {
            ctx.ui.warn("there is no config.json beside that vault; only the vault was restored.");
        }
    }

    ctx.ui.line(format!("The vault is in place at {}.", ctx.paths.vault_file().display()));
    ctx.ui.line("Start superbackup and unlock it with that passphrase.");
    Outcome::data(serde_json::json!({
        "restored": true,
        "from": source.display().to_string(),
        "secrets": handles,
        "previous_vault_kept_at": kept.map(|p| p.display().to_string()),
        "config_restored": config_restored,
    }))
}

/// Accept either the folder a backup was restored into or the file itself.
///
/// A person following RESTORE.txt has a folder; a person who found the file
/// has a path to it. Guessing wrong here means an error message about a
/// directory, at the one moment nobody wants to debug an error message.
fn locate(given: &std::path::Path) -> CliResult<std::path::PathBuf> {
    use superbackup_core::error::ErrorCode;

    if given.is_file() {
        return Ok(given.to_path_buf());
    }
    if given.is_dir() {
        let candidate = given.join("config.sbvault");
        if candidate.is_file() {
            return Ok(candidate);
        }
        return Err(CliError::new(
            ErrorCode::Validation,
            format!("{} has no config.sbvault in it.", given.display()),
        )
        .with_hint(
            "Point --from at the folder you restored the vault backup into, or at the \
             config.sbvault file itself.",
        ));
    }
    Err(CliError::new(ErrorCode::Io, format!("{} does not exist.", given.display())))
}

/// List the copies kept before each change to the vault.
///
/// Worth its own verb because the copies are the answer to "I restored the
/// wrong file", and a folder of dated blobs is not obviously that until
/// something says so.
fn backups(ctx: &mut Ctx) -> CliResult<Outcome> {
    use superbackup_core::crypto::file::VaultFile;

    if !VaultFile::exists(&ctx.paths) {
        ctx.ui.line("There is no vault on this machine yet.");
        return Outcome::data(serde_json::json!({ "backups": [] }));
    }
    let file = VaultFile::load(&ctx.paths)?;
    let found = file.list_backups()?;
    if found.is_empty() {
        ctx.ui.line("No copies yet. One is written before every change to the vault.");
    } else {
        ctx.ui.line(format!(
            "{} in {}:",
            format::plural(found.len(), "copy", "copies"),
            file.backup_dir().display()
        ));
        for path in &found {
            ctx.ui.line(format!(
                "  {}",
                path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
            ));
        }
        ctx.ui.note("Put one back with `superbackup vault restore --from <that file>`.");
    }
    Outcome::data(serde_json::json!({
        "backups": found.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
    }))
}
