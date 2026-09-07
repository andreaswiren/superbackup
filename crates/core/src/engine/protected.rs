//! Jobs whose contents are not files the user named.
//!
//! # The two things a file backup cannot protect
//!
//! A backup of every file on a machine is worth nothing without two small
//! things that are not among those files, or are among them in a form that
//! must not be copied as-is:
//!
//! - **The vault.** It holds every repository password. Restoring it is the
//!   first step of every other restore, and it cannot be protected by an
//!   ordinary job, because an ordinary job writes into a repository whose
//!   password is in the vault. That circularity is the whole reason this
//!   module exists.
//! - **The SSH keys.** They are what get you back into the forges and the
//!   servers. They are also the one kind of file that must never be written
//!   to a destination as it sits on disk: a destination can be a folder
//!   mirror, and a folder mirror of `~/.ssh` into OneDrive is a private key
//!   handed to every device on the account.
//!
//! # How it works
//!
//! A job with a prepared [`JobContent`] carries no sources. Before the run,
//! [`ContentProvider::stage`] builds the payload into a staging folder under
//! superbackup's own data directory, and the runner backs that folder up as
//! though the user had listed it. The folder is removed when the run ends, on
//! every exit path, by [`Staging`]'s `Drop`.
//!
//! # What lands in the destination
//!
//! Ciphertext, in both cases, and never anything else:
//!
//! | Job | File | Encrypted by |
//! |---|---|---|
//! | Vault | `config.sbvault` | the vault itself, under the master passphrase |
//! | Keys | `superbackup-keys.sbkeys` | [`KeyBundle::seal`], under the master passphrase |
//!
//! Plus `RESTORE.txt`, in plain text, deliberately. The instructions for
//! getting back in are the one thing that is useless if you need the backup to
//! read them, and they give nothing away: they describe a procedure that still
//! requires the passphrase.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use uuid::Uuid;

use crate::credentials::KeyBundle;
use crate::error::{Error, IoContext, Result};
use crate::model::{JobContent, Source};
use crate::secret::Secret;

/// The plain-text instructions written beside the payload.
pub const RESTORE_FILE: &str = "RESTORE.txt";

/// A staging folder that removes itself.
///
/// Held by the runner for the length of the run. `Drop` rather than an
/// explicit cleanup call because the paths this has to survive include a
/// cancelled run, a panicking driver and a timeout — and one of those
/// forgetting to call cleanup would leave a sealed bundle of the machine's
/// private keys sitting in the data directory indefinitely.
#[derive(Debug)]
pub struct Staging {
    dir: PathBuf,
}

impl Staging {
    /// Create the folder, hardened, replacing anything already there.
    ///
    /// Replacing rather than reusing: a leftover from a previous run could
    /// hold a key that is no longer ticked for backup, and reusing the folder
    /// would keep backing it up forever.
    pub fn create(dir: PathBuf) -> Result<Staging> {
        if dir.exists() {
            std::fs::remove_dir_all(&dir)
                .ctx(format!("clearing the staging folder {}", dir.display()))?;
        }
        std::fs::create_dir_all(&dir)
            .ctx(format!("creating the staging folder {}", dir.display()))?;
        crate::paths::harden_dir(&dir)?;
        Ok(Staging { dir })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        // Nothing useful to do with a failure here, and no caller left to
        // tell. The next run's `create` clears whatever survived.
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A prepared payload, ready to be backed up.
#[derive(Debug)]
pub struct StagedContent {
    /// What the runner backs up instead of `job.sources`.
    pub sources: Vec<Source>,
    /// One line per file, for the run log. Names only, never contents.
    pub notes: Vec<String>,
    /// Kept alive for the run; deletes the folder when dropped.
    pub staging: Staging,
}

/// Supplies the payload for a job whose content is not files on disk.
///
/// A trait rather than a function because staging needs the unlocked vault and
/// the master passphrase, which live in the daemon, while the runner lives
/// here. Dyn-compatible for the same reason [`crate::engine::BackupExecutor`]
/// is: the daemon holds one `Arc<dyn ContentProvider>` chosen at startup.
pub trait ContentProvider: std::fmt::Debug + Send + Sync + 'static {
    fn stage<'a>(
        &'a self,
        content: JobContent,
        run_id: Uuid,
    ) -> crate::engine::clock::BoxFuture<'a, Result<StagedContent>>;
}

/// The provider used when none was installed.
///
/// Fails rather than backing up nothing. A keys job that quietly produced an
/// empty snapshot would report success every night and protect nothing, which
/// is the exact failure this whole feature exists to prevent.
#[derive(Debug, Default)]
pub struct UnavailableContent;

impl ContentProvider for UnavailableContent {
    fn stage<'a>(
        &'a self,
        content: JobContent,
        _run_id: Uuid,
    ) -> crate::engine::clock::BoxFuture<'a, Result<StagedContent>> {
        Box::pin(async move {
            Err(Error::Config(format!(
                "a {} backup needs the vault to be unlocked, and this instance has no way to \
                 reach it. Unlock superbackup and run the job again.",
                content.label().to_lowercase()
            )))
        })
    }
}

/// Where a run's staging folder goes.
///
/// Under the data directory, never under the system temp folder: temp is
/// swept by tools that do not ask, and is exactly the sort of place a user
/// might have pointed a sync client at.
pub fn staging_dir(data_dir: &Path, run_id: Uuid) -> PathBuf {
    data_dir.join("staging").join(run_id.simple().to_string())
}

/// Build a keys payload into `dir`.
///
/// # Why it seals before writing
///
/// This is the only arrangement under which "put my SSH keys in the backup" is
/// not a way to lose them. The destination may be an S3 bucket one policy
/// change from public, or a OneDrive folder that syncs to every device on the
/// account. What goes in is a file that needs the master passphrase, and there
/// is no setting that turns that off.
pub fn stage_keys(
    dir: &Path,
    machine: &str,
    keys: &[PathBuf],
    passphrase: &Secret,
) -> Result<Vec<String>> {
    if keys.is_empty() {
        return Err(Error::Validation(
            "no keys are ticked for backup. Tick the ones you want protected on the Credentials \
             page, then run this job again."
                .into(),
        ));
    }

    // Missing keys are named rather than skipped. A key that was deleted or
    // renamed is a key this job stopped protecting, and finding that out from
    // a failed restore is finding it out far too late.
    let missing: Vec<String> =
        keys.iter().filter(|p| !p.exists()).map(|p| p.display().to_string()).collect();
    if !missing.is_empty() {
        return Err(Error::Validation(format!(
            "these keys are ticked for backup but are not on disk any more: {}. Untick them on \
             the Credentials page, or put them back.",
            missing.join(", ")
        )));
    }

    let bundle = KeyBundle::gather(machine, keys)?;
    let names: Vec<String> = bundle.files.iter().map(|f| f.name.clone()).collect();
    let sealed = bundle.seal(passphrase)?;

    let target = dir.join(crate::credentials::BUNDLE_FILE);
    std::fs::write(&target, &sealed)
        .ctx(format!("writing the key bundle to {}", target.display()))?;
    crate::paths::harden_file(&target)?;

    write_restore_note(dir, &keys_restore_note(&names, machine))?;
    Ok(names)
}

/// Build a vault payload into `dir`.
///
/// The vault file is copied as it sits, because it is already sealed: it is
/// the output of [`crate::crypto::Vault::seal`], which is the same encryption
/// a key bundle gets. Re-encrypting it would add a second passphrase to
/// remember and protect nothing that is not already protected.
pub fn stage_vault(dir: &Path, vault_file: &Path) -> Result<Vec<String>> {
    if !vault_file.exists() {
        return Err(Error::Config(format!(
            "there is no vault at {} to back up. A vault is created the first time you set a \
             master passphrase.",
            vault_file.display()
        )));
    }

    let name = vault_file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config.sbvault".to_string());
    let target = dir.join(&name);
    std::fs::copy(vault_file, &target).ctx(format!("copying {}", vault_file.display()))?;
    crate::paths::harden_file(&target)?;

    // The vault is the secret; the configuration beside it is not, and having
    // it in the same backup is the difference between "I have my passwords"
    // and "I have my setup". Best effort: a missing config file is not a
    // reason to fail the one job that protects the passwords.
    let mut names = vec![name];
    let config = vault_file.with_file_name("config.json");
    if config.exists() && std::fs::copy(&config, dir.join("config.json")).is_ok() {
        names.push("config.json".to_string());
    }

    write_restore_note(dir, &vault_restore_note(&names))?;
    Ok(names)
}

fn write_restore_note(dir: &Path, text: &str) -> Result<()> {
    let path = dir.join(RESTORE_FILE);
    std::fs::write(&path, text.as_bytes()).ctx(format!("writing {}", path.display()))?;
    Ok(())
}

/// The instructions that travel with a vault backup.
///
/// Written out in full, in the backup, in plain text. Someone reading this is
/// having a bad day and does not have superbackup's documentation to hand, and
/// possibly does not have superbackup to hand either. It says what the file
/// is, what it needs, and the three steps; and it says the thing that is
/// easiest to get wrong, which is that a fresh install must not be given a
/// passphrase first.
pub fn vault_restore_note(files: &[String]) -> String {
    format!(
        "SUPERBACKUP - VAULT BACKUP\n\
         ==========================\n\
         \n\
         What this is\n\
         ------------\n\
         {}\n\
         \n\
         config.sbvault is superbackup's vault. It holds the password of every\n\
         backup repository you have, and any access tokens you asked superbackup\n\
         to keep. It is encrypted with your master passphrase.\n\
         \n\
         You need this file to open your other backups. Without it, the data in\n\
         them is still there and still safe, but nothing can read it.\n\
         \n\
         You also need your master passphrase. It is not in this file and it is\n\
         not anywhere else. Nobody, including us, can recover it for you.\n\
         \n\
         How to restore it\n\
         -----------------\n\
         1. Install superbackup on the new machine. Do not set a passphrase and\n\
            do not create anything yet: a fresh vault here would be a second,\n\
            empty vault, and step 2 would overwrite this one instead.\n\
         \n\
         2. Copy config.sbvault (and config.json, if it is here) into the\n\
            configuration folder, replacing anything already there:\n\
         \n\
              Windows   %APPDATA%\\superbackup\\\n\
              Linux     ~/.config/superbackup/\n\
              macOS     ~/Library/Application Support/superbackup/\n\
         \n\
            Or from a terminal, with superbackup installed:\n\
         \n\
              superbackup vault restore --from <this folder>\n\
         \n\
         3. Start superbackup and unlock it with your master passphrase. Your\n\
            destinations and jobs are back, and your other backups open again.\n\
         \n\
         Keeping it safe\n\
         ---------------\n\
         This file is encrypted, so it is safe in a bucket, in OneDrive, or on\n\
         a USB stick. It is exactly as safe as your master passphrase is, so\n\
         keep the passphrase somewhere else: a password manager, or written\n\
         down somewhere only you can get to. Not in the same folder as this.\n",
        files.join("\n")
    )
}

/// The instructions that travel with a key bundle.
pub fn keys_restore_note(names: &[String], machine: &str) -> String {
    format!(
        "SUPERBACKUP - SSH KEY BACKUP\n\
         ============================\n\
         \n\
         What this is\n\
         ------------\n\
         {bundle} holds these key files from {machine}:\n\
         \n\
         {names}\n\
         \n\
         They are encrypted together, under your superbackup master passphrase.\n\
         The private keys are NOT readable from this file without it. That is\n\
         deliberate: a private key copied somewhere in the clear is a private\n\
         key given to whoever else can reach that place.\n\
         \n\
         How to restore them\n\
         -------------------\n\
         1. Install superbackup on the machine that needs the keys, and restore\n\
            your vault first if you have not already - see the vault backup.\n\
         \n\
         2. Put {bundle} in a folder, and either:\n\
         \n\
              - open superbackup, go to Credentials, and use Bring keys in;\n\
              - or run:  superbackup cred unseal --from <that folder>\n\
         \n\
            You will be asked for your master passphrase.\n\
         \n\
         3. The keys are written into this account's key folder (~/.ssh) with\n\
            owner-only permissions. Files already there are left alone unless\n\
            you ask for them to be replaced: the likeliest mistake here is\n\
            unsealing an old bundle over the key the machine is using now.\n\
         \n\
         If a key had its own passphrase, that passphrase is in your vault and\n\
         superbackup can show it to you once the vault is unlocked.\n",
        bundle = crate::credentials::BUNDLE_FILE,
        machine = machine,
        names = names.iter().map(|n| format!("  {n}")).collect::<Vec<_>>().join("\n"),
    )
}

/// A prepared provider is installed on the runner as one of these.
pub type SharedContentProvider = Arc<dyn ContentProvider>;

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sb-stage-{}", Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).expect("create");
        dir
    }

    /// The whole point of the module: what reaches the destination is
    /// ciphertext. A test that only checked the file existed would have passed
    /// for a version that copied the key straight through.
    #[test]
    fn a_staged_key_bundle_does_not_contain_the_key() {
        let home = temp();
        let key = home.join("id_ed25519");
        let material = b"-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\n";
        std::fs::write(&key, material).expect("write");
        std::fs::write(home.join("id_ed25519.pub"), b"ssh-ed25519 AAAA test\n").expect("write");

        let staging = Staging::create(home.join("staged")).expect("staging");
        let passphrase = Secret::from_str("correct horse battery staple");
        let names = stage_keys(
            staging.dir(),
            "test-machine",
            &[key.clone(), home.join("id_ed25519.pub")],
            &passphrase,
        )
        .expect("stage");
        assert_eq!(names, vec!["id_ed25519", "id_ed25519.pub"]);

        let sealed = std::fs::read(staging.dir().join(crate::credentials::BUNDLE_FILE))
            .expect("bundle written");
        let needle = b"BEGIN OPENSSH PRIVATE KEY";
        assert!(
            !sealed.windows(needle.len()).any(|w| w == needle),
            "the key material is in the staged file in the clear"
        );

        // And it is the real thing on the way back, not merely unreadable.
        let opened = KeyBundle::unseal(&sealed, &passphrase).expect("unseal");
        let private = opened.files.iter().find(|f| f.name == "id_ed25519").expect("private");
        assert!(private.private);
        assert_eq!(KeyBundle::decode(private).expect("decode").expose(), material);

        // The wrong passphrase gets nothing.
        assert!(KeyBundle::unseal(&sealed, &Secret::from_str("wrong")).is_err());

        let _ = std::fs::remove_dir_all(&home);
    }

    /// The instructions have to be readable without superbackup, so they are
    /// the one thing in the folder that is not encrypted.
    #[test]
    fn the_restore_note_is_plain_text_and_says_what_is_needed() {
        let home = temp();
        std::fs::write(home.join("id_ed25519"), b"key").expect("write");
        let staging = Staging::create(home.join("staged")).expect("staging");
        stage_keys(staging.dir(), "m", &[home.join("id_ed25519")], &Secret::from_str("p"))
            .expect("stage");

        let note = std::fs::read_to_string(staging.dir().join(RESTORE_FILE)).expect("note");
        assert!(note.contains("master passphrase"), "{note}");
        assert!(note.contains("cred unseal"), "{note}");
        assert!(note.contains("id_ed25519"), "the files it holds are named: {note}");

        let _ = std::fs::remove_dir_all(&home);
    }

    /// A vault backup is a copy, not a re-encryption, and it carries the steps.
    #[test]
    fn a_vault_backup_is_the_sealed_file_plus_the_way_back() {
        let home = temp();
        let vault = home.join("config.sbvault");
        let sealed = b"SBVAULT\x01 not really, but opaque";
        std::fs::write(&vault, sealed).expect("write");
        std::fs::write(home.join("config.json"), b"{}").expect("write");

        let staging = Staging::create(home.join("staged")).expect("staging");
        let names = stage_vault(staging.dir(), &vault).expect("stage");
        assert_eq!(names, vec!["config.sbvault", "config.json"]);

        assert_eq!(
            std::fs::read(staging.dir().join("config.sbvault")).expect("copied"),
            sealed,
            "the vault is carried as-is"
        );

        let note = std::fs::read_to_string(staging.dir().join(RESTORE_FILE)).expect("note");
        // The three things someone in this situation has to be told.
        assert!(note.contains("%APPDATA%"), "where it goes: {note}");
        assert!(note.contains("vault restore"), "the command: {note}");
        assert!(note.contains("nothing can read it"), "why it matters: {note}");
        assert!(
            note.contains("Do not set a passphrase"),
            "the mistake that destroys the thing being restored: {note}"
        );

        let _ = std::fs::remove_dir_all(&home);
    }

    /// A job that protects nothing must say so, not succeed quietly. This is
    /// the failure that would go unnoticed for months.
    #[test]
    fn staging_no_keys_is_an_error_rather_than_an_empty_backup() {
        let home = temp();
        let staging = Staging::create(home.join("staged")).expect("staging");
        let err = stage_keys(staging.dir(), "m", &[], &Secret::from_str("p")).expect_err("refused");
        assert!(err.to_string().contains("ticked for backup"), "{err}");

        // A key that has since been deleted is named rather than skipped.
        let gone = home.join("id_gone");
        let err =
            stage_keys(staging.dir(), "m", &[gone], &Secret::from_str("p")).expect_err("refused");
        assert!(err.to_string().contains("not on disk"), "{err}");

        let _ = std::fs::remove_dir_all(&home);
    }

    /// The folder holds the machine's sealed keys, so it does not outlive the
    /// run, including the runs that end badly.
    #[test]
    fn the_staging_folder_removes_itself() {
        let home = temp();
        let dir = home.join("staged");
        {
            let staging = Staging::create(dir.clone()).expect("staging");
            std::fs::write(staging.dir().join("payload"), b"x").expect("write");
            assert!(dir.exists());
        }
        assert!(!dir.exists(), "the staging folder outlived the run");

        // And a leftover from a previous run is cleared rather than reused: a
        // key unticked since then must not keep being backed up.
        std::fs::create_dir_all(&dir).expect("create");
        std::fs::write(dir.join("stale-key.sbkeys"), b"old").expect("write");
        let staging = Staging::create(dir.clone()).expect("staging");
        assert!(!staging.dir().join("stale-key.sbkeys").exists());

        let _ = std::fs::remove_dir_all(&home);
    }

    /// Without a provider the job fails loudly. It must never look like a
    /// backup that happened.
    #[tokio::test]
    async fn a_missing_provider_fails_the_job_rather_than_emptying_it() {
        let err =
            UnavailableContent.stage(JobContent::Vault, Uuid::new_v4()).await.expect_err("refused");
        assert!(err.to_string().contains("unlocked"), "{err}");
    }
}
