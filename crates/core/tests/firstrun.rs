//! What the setup wizard's answers actually do.
//!
//! # The bug this file exists to hold shut
//!
//! The wizard asked four things and acted on one of them. The passphrase made
//! a vault, which worked. The OneDrive choice and the job template were stored
//! on a struct and read by nothing. The Start-menu entry, "start at login" and
//! "install the service" were sent to the daemon over IPC — during a first
//! run, when the daemon does not exist, because it will not start until a
//! vault exists and the wizard is what creates one. A request nobody is
//! listening for fails quietly, so a user who ticked every box got a vault and
//! nothing else, with no error to say so.
//!
//! Every test here therefore asserts against the *configuration on disk*
//! afterwards, not against what was asked for. Asking was never the problem.
//!
//! # What is deliberately not tested
//!
//! The Start-menu entry, autostart and the service. Carrying those out means
//! writing a shortcut into the profile of whoever runs the tests and raising
//! an elevation prompt on their machine, so the tests leave all three off and
//! the mapping from tick box to `Choices` is asserted where the wizard builds
//! it instead.

use std::path::PathBuf;

use superbackup_core::config::Store;
use superbackup_core::crypto::{KdfParams, Vault};
use superbackup_core::engine::testing::test_job;
use superbackup_core::firstrun::{self, Choices};
use superbackup_core::model::DestinationKind;
use superbackup_core::paths::Paths;
use superbackup_core::secret::Secret;

const PASSPHRASE: &str = "correct horse battery staple";

/// A temporary installation that deletes itself.
struct Home(PathBuf);

impl Home {
    fn new(tag: &str) -> Home {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let dir =
            std::env::temp_dir().join(format!("sb-first-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        Home(dir)
    }
    fn paths(&self) -> Paths {
        Paths::rooted_at(self.0.join("install"), false)
    }
    /// Somewhere to stand in for a synced OneDrive root.
    fn onedrive(&self) -> PathBuf {
        self.0.join("OneDrive")
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A vault, as the wizard's passphrase step leaves one — but with the cheap
/// KDF, because a test does not need 64 MiB of Argon2 to prove a destination
/// was written.
fn installed(home: &Home) -> Paths {
    let paths = home.paths();
    paths.ensure().expect("create the directories");
    let vault = Vault::create_unchecked(
        &Secret::from_str(PASSPHRASE),
        KdfParams::insecure_for_tests().expect("test kdf parameters"),
    )
    .expect("build the vault");
    Store::initialise_with(paths.clone(), vault).expect("initialise the store");
    paths
}

/// Nothing platform-facing: see the note at the top of the file.
fn quiet(onedrive: Option<PathBuf>, job: Option<superbackup_core::model::Job>) -> Choices {
    Choices { onedrive, job, create_shortcut: false, autostart: false, install_service: false }
}

/// The whole of what was missing: a destination and a job that exist
/// afterwards, in the configuration the daemon reads when it starts.
#[test]
fn finishing_setup_leaves_a_destination_and_a_job_behind() {
    let home = Home::new("full");
    let paths = installed(&home);

    let mut job = test_job("Development");
    // An absolute source: a scheduled run has no meaningful working
    // directory, so the configuration refuses a relative one.
    job.sources = vec![superbackup_core::model::Source::new(home.onedrive().join("src"))];

    let applied = firstrun::apply(
        &paths,
        &Secret::from_str(PASSPHRASE),
        &quiet(Some(home.onedrive()), Some(job)),
    );

    assert_eq!(applied.problems, Vec::<String>::new(), "setup reported problems");
    assert_eq!(applied.destination.as_deref(), Some("OneDrive"));
    assert_eq!(applied.job.as_deref(), Some("Development"));

    // And it is on disk, not merely reported. This is the assertion the old
    // test could not make, because it stopped at "a request was sent".
    let mut store = Store::open(paths.clone()).expect("reopen");
    store.unlock(&Secret::from_str(PASSPHRASE)).expect("unlock");
    let config = store.config().clone();

    assert_eq!(config.destinations.len(), 1, "{:?}", config.destinations);
    assert_eq!(config.jobs.len(), 1, "{:?}", config.jobs);

    let destination = &config.destinations[0];
    let DestinationKind::OneDrive { path, .. } = &destination.kind else {
        panic!("not a OneDrive destination: {:?}", destination.kind);
    };

    // `<OneDrive>/Superbackup/<machine>`. Grouped so a person opening OneDrive
    // finds one folder rather than repository blobs among their documents,
    // and per machine because two PCs writing into one kopia repository
    // directory is not a shared backup, it is a corrupted one.
    assert_eq!(path, &firstrun::onedrive_folder(&home.onedrive(), &config.machine.slug));
    assert!(path.is_dir(), "the folder was not created: {}", path.display());

    // The job backs up to the destination that was just made. A job with an
    // empty `destination_ids` runs every night and writes nowhere.
    assert_eq!(config.jobs[0].destination_ids, vec![destination.id]);

    // And the repository passphrase is in the vault, not invented at run time.
    let handle = destination.passphrase_ref.clone().expect("a passphrase handle");
    assert!(store.secret(&handle).expect("read the secret").is_some(), "no passphrase was stored");
}

/// Declining OneDrive must not create one anyway.
#[test]
fn no_onedrive_means_no_destination() {
    let home = Home::new("none");
    let paths = installed(&home);

    let applied = firstrun::apply(&paths, &Secret::from_str(PASSPHRASE), &quiet(None, None));

    assert_eq!(applied.problems, Vec::<String>::new());
    assert_eq!(applied.destination, None);
    assert_eq!(applied.job, None);
    assert!(!home.onedrive().exists(), "a folder was created for a destination nobody asked for");

    let mut store = Store::open(paths).expect("reopen");
    store.unlock(&Secret::from_str(PASSPHRASE)).expect("unlock");
    assert!(store.config().destinations.is_empty());
    assert!(store.config().jobs.is_empty());
}

/// A passphrase that does not open the vault has to say so.
///
/// Silence is what made the original bug invisible: setup finished, looked
/// exactly like a setup that had worked, and left nothing behind.
#[test]
fn a_wrong_passphrase_is_reported_rather_than_swallowed() {
    let home = Home::new("wrong");
    let paths = installed(&home);

    let applied = firstrun::apply(
        &paths,
        &Secret::from_str("not the passphrase at all"),
        &quiet(Some(home.onedrive()), None),
    );

    assert!(!applied.problems.is_empty(), "a failed setup said nothing");
    assert_eq!(applied.destination, None);
    assert_eq!(applied.job, None);
}

/// Two machines sharing one OneDrive get two folders.
#[test]
fn a_second_machine_does_not_land_in_the_first_ones_folder() {
    let root = PathBuf::from(if cfg!(windows) { r"C:\OneDrive" } else { "/OneDrive" });
    assert_ne!(
        firstrun::onedrive_folder(&root, "laptop-1111"),
        firstrun::onedrive_folder(&root, "desktop-2222")
    );
}

/// A job the wizard could not make valid must not cost the user their
/// destination as well.
///
/// The destination is the expensive half: it carries a generated repository
/// passphrase that is already in the vault and a folder that already exists.
/// A job is three fields and a folder picker.
#[test]
fn a_job_that_will_not_save_does_not_take_the_destination_with_it() {
    let home = Home::new("badjob");
    let paths = installed(&home);

    // Relative sources are refused: a scheduled run has no working directory.
    let mut job = test_job("Development");
    job.sources = vec![superbackup_core::model::Source::new("relative/path")];

    let applied = firstrun::apply(
        &paths,
        &Secret::from_str(PASSPHRASE),
        &quiet(Some(home.onedrive()), Some(job)),
    );

    assert_eq!(applied.job, None, "an invalid job must not be reported as made");
    assert!(!applied.problems.is_empty(), "it must say why there is no job");
    assert_eq!(applied.destination.as_deref(), Some("OneDrive"), "the destination was lost too");

    let mut store = Store::open(paths).expect("reopen");
    store.unlock(&Secret::from_str(PASSPHRASE)).expect("unlock");
    assert_eq!(store.config().destinations.len(), 1, "the destination is not on disk");
    assert!(store.config().jobs.is_empty());
}
