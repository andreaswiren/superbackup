//! What the setup wizard's answers are actually supposed to *do*.
//!
//! # The bug this module exists to fix
//!
//! The wizard asked four things and acted on one of them. The passphrase
//! created a vault, which worked. The OneDrive choice and the job template
//! were written into a struct field and read by nothing outside the code that
//! drew them. The Start-menu entry, "start at login" and "install the service"
//! were sent to the daemon over IPC — on a first run, when the daemon does not
//! exist yet, because it refuses to start without a vault and the window is
//! what creates one.
//!
//! So a user who ticked every box got: a vault, and nothing else. No
//! destination, no job, no shortcut, no autostart, no service, and no way to
//! tell, because a request nobody is listening for fails quietly.
//!
//! # Why this writes the configuration directly
//!
//! Everywhere else in this application the daemon owns the configuration and
//! clients ask it to change things, which is the right rule: two writers to
//! one store is how a config file gets torn in half.
//!
//! First run is the one moment that rule cannot apply, and it is safe for the
//! same reason it is necessary — the daemon provably is not running, because
//! the vault it needs was created seconds ago by this process. There is no
//! second writer to race. The daemon starts afterwards and reads what is here.
//!
//! # Why it is a function and not more code in the wizard
//!
//! Because it can be tested. The wizard's own tests render every step and
//! assert it does not panic, which is exactly the kind of test that passed
//! throughout the period when finishing setup did nothing at all.

use std::path::PathBuf;

use crate::config::Store;
use crate::model::{Config, Destination, DestinationKind, EncryptionSettings, Job, SecretRef};
use crate::secret::Secret;

/// What the user asked for on the way through the wizard.
#[derive(Debug, Clone, Default)]
pub struct Choices {
    /// The OneDrive account to back up to, if they chose one.
    pub onedrive: Option<PathBuf>,
    /// The first job: a name, the folders, and the exclusions.
    pub job: Option<Job>,
    pub create_shortcut: bool,
    pub autostart: bool,
    pub install_service: bool,
    /// Start superbackup minimised to the tray rather than with a window.
    pub start_minimised: bool,
    /// Let this machine open its own vault at login, so scheduled backups run
    /// without anybody typing anything.
    ///
    /// On by default in [`Settings`](crate::model::Settings), so this carries
    /// the user's answer rather than an opt-in: a wizard that never showed the
    /// switch leaves it as it was.
    pub unattended_unlock: bool,
}

/// What happened, so the interface can say so rather than guess.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Applied {
    pub destination: Option<String>,
    pub job: Option<String>,
    /// One line per thing that did not work. Setup continues regardless: a
    /// refused elevation prompt for the service must not cost the user their
    /// backup job.
    pub problems: Vec<String>,
}

/// The folder inside OneDrive that this machine's backups live in.
///
/// `<OneDrive>/Superbackup/<machine>`, and both halves matter. `Superbackup/`
/// keeps every machine's backups in one place a person can find, rather than
/// scattering repository blobs across the root of somebody's documents. The
/// per-machine folder beneath it is what makes a shared OneDrive usable at
/// all: two PCs writing into one kopia repository directory is not a shared
/// backup, it is a corrupted one.
///
/// The slug is fixed for the life of the install, so renaming the machine in
/// settings later does not orphan the repository.
pub fn onedrive_folder(root: &std::path::Path, machine_slug: &str) -> PathBuf {
    root.join("Superbackup").join(crate::model::machine_folder(machine_slug))
}

/// Apply the wizard's answers to a freshly created installation.
///
/// The vault must already exist: this opens it to store the repository
/// passphrase it generates. Each part is independent, and a failure in one is
/// recorded and stepped over rather than abandoning the rest.
pub fn apply(paths: &crate::paths::Paths, passphrase: &Secret, choices: &Choices) -> Applied {
    let mut applied = Applied::default();

    match Store::open(paths.clone()) {
        Ok(mut store) => {
            if let Err(e) = store.unlock(passphrase) {
                applied.problems.push(format!("the vault could not be opened: {e}"));
                // Not `return`. The Start-menu entry, starting at login and
                // installing the service have nothing to do with the vault,
                // and returning here silently skipped all three because one
                // unrelated thing had failed.
                apply_platform(paths, choices, &mut applied);
                return applied;
            }
            let mut config = store.config().clone();
            let slug = config.machine.slug.clone();
            // The settings that describe what was just done to the machine.
            //
            // These are separate from doing it — `apply_platform` writes the
            // Start-menu entry and the login entry, and this records that they
            // are wanted — and they were not being written at all. `Settings`
            // defaults `start_at_login` to true, so a user who declined it got
            // no login entry, correctly, and a Settings screen showing the
            // toggle on, which is the interface disagreeing with the machine.
            config.settings.use_os_keychain = choices.unattended_unlock;
            config.settings.start_at_login = choices.autostart;
            config.settings.start_minimised = choices.start_minimised;
            config.settings.run_as_service = choices.install_service;

            if let Some(root) = &choices.onedrive {
                match add_onedrive(&mut store, &mut config, root, &slug) {
                    Ok(name) => applied.destination = Some(name),
                    Err(e) => applied.problems.push(e),
                }
            }

            // Kept aside so the destination can be saved without it. See
            // below.
            let without_job = config.clone();

            if let Some(job) = &choices.job {
                let mut job = job.clone();
                // Every destination except one inside the job's own folders.
                //
                // "Everything" backs up the home directory, and OneDrive lives
                // inside it — so the pair the wizard offers side by side
                // produced a job that backs up the previous run's output and
                // grows without bound. The configuration refuses it, correctly
                // (`validate_no_self_nesting`), which meant choosing those two
                // perfectly reasonable options ended setup with no job at all.
                //
                // Dropping the destination rather than the job: the job is
                // what the user described, and a job with a folder and no
                // destination is still refused below, so this cannot quietly
                // produce one that backs up nowhere.
                job.destination_ids = config
                    .destinations
                    .iter()
                    .filter(|destination| !nests_inside(destination, &job))
                    .map(|d| d.id)
                    .collect();
                if job.destination_ids.is_empty() && !config.destinations.is_empty() {
                    applied.problems.push(format!(
                        "\"{}\" was not created: the only place to back it up to is inside the \
                         folders it would back up, so every run would copy the last one. Make a \
                         job with a narrower folder, or a destination outside it.",
                        job.name
                    ));
                } else {
                    applied.job = Some(job.name.clone());
                    config.jobs.push(job);
                }
            }

            if let Err(e) = store.set_config(config) {
                applied.problems.push(format!("the first backup job was not created: {e}"));
                applied.job = None;

                // A job the wizard could not make valid must not cost the user
                // their destination as well. The configuration is saved whole
                // or not at all — correctly, since half a configuration is not
                // a configuration — so the way to keep the good half is to
                // save the version without the bad one.
                //
                // A destination is the harder of the two to recreate: it holds
                // a generated repository passphrase that is already in the
                // vault, and a folder that already exists on disk. A job is
                // three fields and a folder picker.
                // Unconditionally, not only when there is a destination to
                // save. `without_job` also carries the settings — including
                // whether this machine may unlock itself, which is a security
                // decision the user made deliberately — and skipping the save
                // reverted all of them to their defaults without a word.
                if let Err(e) = store.set_config(without_job) {
                    applied.problems.push(format!("the configuration could not be saved: {e}"));
                    applied.destination = None;
                }
            }
        }
        Err(e) => applied.problems.push(format!("the vault could not be read: {e}")),
    }

    apply_platform(paths, choices, &mut applied);
    applied
}

/// Would backing up to this destination copy the job's own output?
///
/// The rule the configuration enforces, applied here so the wizard can avoid
/// producing a pair it is going to refuse. A destination inside one of the
/// job's own folders means every run backs up the previous run, and the
/// repository grows until the disk is full.
///
/// Only the destinations that live on this filesystem can nest; a bucket has
/// no path to be inside anything.
fn nests_inside(destination: &Destination, job: &Job) -> bool {
    let Some(path) = destination.kind.local_path() else {
        return false;
    };
    job.sources.iter().any(|source| path.starts_with(&source.path))
}

/// Add a OneDrive destination, with its own generated repository passphrase.
///
/// The folder is created here rather than left to the first backup. A
/// destination pointing at a folder that does not exist looks configured and
/// fails at 2am; creating it now also surfaces a read-only or full OneDrive
/// while the user is still sitting in front of the wizard.
fn add_onedrive(
    store: &mut Store,
    config: &mut Config,
    root: &std::path::Path,
    slug: &str,
) -> Result<String, String> {
    let folder = onedrive_folder(root, slug);
    std::fs::create_dir_all(&folder)
        .map_err(|e| format!("{} could not be created: {e}", folder.display()))?;

    let id = uuid::Uuid::new_v4();
    let secret_ref = SecretRef::new("repo.passphrase", &id);
    // A generated 256-bit passphrase, not one the user has to remember. It
    // lives in the vault, which is the thing their master passphrase opens.
    let repo_passphrase = crate::crypto::generate_passphrase()
        .map_err(|e| format!("a repository passphrase could not be generated: {e}"))?;
    store
        .put_secret(secret_ref.clone(), repo_passphrase)
        .map_err(|e| format!("the repository passphrase could not be stored: {e}"))?;

    let name = "OneDrive".to_string();
    config.destinations.push(Destination {
        id,
        name: name.clone(),
        kind: DestinationKind::OneDrive { path: folder, account: None },
        encryption: Some(EncryptionSettings::default()),
        passphrase_ref: Some(secret_ref),
        retention: Default::default(),
        enabled: true,
        auto_discovered: true,
        bandwidth: None,
        replicate_from: None,
        created_at: chrono::Utc::now(),
        last_verified_at: None,
        // OneDrive syncs to every device on the account, which is the whole
        // reason somebody picks it.
        shared: true,
    });
    Ok(name)
}

/// The three things that live outside the configuration.
///
/// Each is attempted separately. They are three different mechanisms that fail
/// for three different reasons, and a refused elevation prompt for the service
/// must not silently cost the user their Start-menu entry.
fn apply_platform(paths: &crate::paths::Paths, choices: &Choices, applied: &mut Applied) {
    use crate::platform;

    // One spec for both: the applications-menu entry and the login entry point
    // at the same executable, and building it twice is how they drift apart.
    let spec = if choices.create_shortcut || choices.autostart {
        match platform::autostart::AutostartSpec::current() {
            Ok(spec) => Some(spec),
            Err(e) => {
                applied.problems.push(format!("this program's own path is not readable: {e}"));
                None
            }
        }
    } else {
        None
    };

    if let Some(spec) = &spec {
        if choices.create_shortcut {
            if let Err(e) = platform::shortcut::install(spec) {
                applied.problems.push(format!("the applications-menu entry was not created: {e}"));
            }
        }
        if choices.autostart {
            if let Err(e) = platform::autostart::enable(spec) {
                applied.problems.push(format!("starting at login could not be set up: {e}"));
            }
        }
    }

    if choices.install_service {
        // Installing a system service needs administrator rights, and this
        // process is whatever the user double-clicked. Saying so plainly beats
        // a failure from deep inside the service manager: the tray still runs
        // every backup while they are signed in, which is what the service
        // improves on rather than what it enables.
        match platform::ServiceOptions::current(paths) {
            Ok(options) => {
                if options.requires_elevation() && !platform::service::is_elevated() {
                    // Ask, rather than explain. Telling somebody at the end of
                    // a wizard to close the program and start it again a
                    // different way is how a ticked box turns into nothing at
                    // all — which is exactly what used to happen here, because
                    // almost nobody sets up a backup tool as an administrator.
                    //
                    // The prompt is the operating system's. Superbackup never
                    // sees the credentials, and a user who declines is left
                    // with a working installation minus the service.
                    if let Err(e) = platform::service::request_elevated_install() {
                        applied.problems.push(e.to_string());
                    }
                } else if let Err(e) = platform::service::install(&options) {
                    applied.problems.push(format!("the background service was not installed: {e}"));
                }
            }
            Err(e) => {
                applied.problems.push(format!("the background service could not be described: {e}"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `<OneDrive>/Superbackup/<machine>`, and why each half is there.
    #[test]
    fn the_onedrive_folder_is_named_and_per_machine() {
        let root =
            PathBuf::from(if cfg!(windows) { r"C:\Users\a\OneDrive" } else { "/home/a/OneDrive" });
        let folder = onedrive_folder(&root, "awpc34-8d3608d4");

        assert!(folder.starts_with(&root), "inside OneDrive: {}", folder.display());
        let parts: Vec<String> =
            folder.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        // Grouped, so a person opening OneDrive finds one folder rather than
        // repository blobs loose among their documents.
        assert!(parts.iter().any(|p| p == "Superbackup"), "{}", folder.display());
        // And per machine: two PCs sharing one kopia repository directory is
        // not a shared backup, it is a corrupted one.
        assert_eq!(parts.last().map(String::as_str), Some("awpc34-8d3608d4"));
    }

    /// A different machine gets a different folder under the same root.
    #[test]
    fn two_machines_never_share_a_repository_folder() {
        let root = PathBuf::from(if cfg!(windows) { r"C:\OneDrive" } else { "/OneDrive" });
        assert_ne!(onedrive_folder(&root, "laptop-1111"), onedrive_folder(&root, "desktop-2222"));
    }
}
