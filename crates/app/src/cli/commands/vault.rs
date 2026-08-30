//! Unlocking, locking, and changing the master passphrase.
//!
//! Everything here sends a secret to the daemon and gets nothing secret back.
//! That is the protocol's rule, not this module's: there is no request that
//! returns credential material, so the CLI could not print a passphrase even
//! if somebody asked it to.

use superbackup_core::ipc::protocol::{Request, SecretString};

use crate::cli::args::UnlockArgs;
use crate::cli::client::{reply, Daemon, Start};
use crate::cli::context::Ctx;
use crate::cli::format;
use crate::cli::output::{CliResult, Outcome};
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

    let secret =
        prompt::passphrase(ctx, args.passphrase_file.as_deref(), "Master passphrase: ")?;
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

fn report_auto_lock(
    ctx: &mut Ctx,
    reply: &superbackup_core::ipc::protocol::UnlockedReply,
) {
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
