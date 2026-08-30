//! Shared scaffolding for the daemon integration tests.
//!
//! `crates/app` has no library target, so an integration test cannot `use`
//! the binary's modules. The daemon and tray trees are therefore included by
//! path — the same arrangement the GUI workstream's `gui_app.rs` uses — with a
//! stub for the two items in `crate::cli` that `daemon::mod` names. Nothing is
//! mocked below that line: the real `Runtime`, the real `KopiaExecutor`, the
//! real engine and the real IPC server are what these tests drive.
//!
//! ## Two things the tests have to work around, and why
//!
//! **The IPC endpoint is a fixed name on Windows.**
//! `Paths::ipc_endpoint()` returns `\\.\pipe\superbackup` whatever `--home`
//! says, so two daemons under two private roots still collide — and so would a
//! test and the developer's own running tray. `daemon::Hooks::endpoint`
//! exists for that and is used here; production never sets it.
//!
//! **kopia is a real subprocess.** These tests use the scriptable fake from
//! `crates/core/tests/kopia_support`, which `rustc` compiles at test time, and
//! point `Settings::kopia_path` straight at it. That exercises the *whole*
//! path — argv construction, the environment, stderr progress parsing,
//! cancellation, child reaping — with no kopia installed.

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use superbackup_core::config::Store;
use superbackup_core::ipc::protocol::Request;
use superbackup_core::ipc::{Client, Reply};
use superbackup_core::model::{
    Config, Destination, DestinationKind, EncryptionSettings, Job, PassphraseSource, SecretRef,
    Source,
};
use superbackup_core::paths::Paths;
use superbackup_core::secret::Secret;
use uuid::Uuid;

/// The master passphrase every fixture uses. Long enough to satisfy
/// `estimate_strength`, which `vault.change_passphrase` enforces.
pub const PASSPHRASE: &str = "correct-horse-battery-staple-42";

/// A running daemon, its private home, and its fake kopia.
pub struct Harness {
    pub root: PathBuf,
    pub paths: Paths,
    pub endpoint: String,
    pub runtime: Arc<crate::daemon::runtime::Runtime>,
    /// Where the fake kopia lives, so a test can rescript it mid-flight.
    pub kopia_dir: PathBuf,
    pub kopia_exe: PathBuf,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    joined: Option<std::thread::JoinHandle<superbackup_core::Result<()>>>,
}

impl Harness {
    /// Build a home with a vault, a fake kopia, and the supplied
    /// configuration, then start the daemon on a private endpoint.
    ///
    /// `configure` is handed the private root as well as the configuration, so
    /// a fixture can put its sources, repositories and mirrors inside the home
    /// that `Drop` will clean up.
    pub async fn start(
        name: &str,
        configure: impl FnOnce(&mut Config, &std::path::Path),
    ) -> Harness {
        let (root, paths) = fresh_home(name);
        let (kopia_dir, kopia_exe) = install_fake_kopia(&root);

        // The vault is created before the daemon starts, because `Store::open`
        // deliberately refuses to create one — a first run needs a human.
        let mut store = Store::initialise(paths.clone(), &Secret::from_str(PASSPHRASE))
            .expect("initialise the store");
        let mut config = store.config().clone();
        config.settings.kopia_path = Some(kopia_exe.clone());
        // Nothing in these tests wants a scheduled run firing underneath it.
        config.settings.run_missed_on_start = false;
        config.settings.max_parallel_jobs = 2;
        configure(&mut config, &root);
        store.set_config(config).expect("save the fixture configuration");
        drop(store);

        let endpoint = private_endpoint(name);
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let hooks = crate::daemon::Hooks {
            ready: Some(ready_tx),
            external_stop: Some(stop_rx),
            endpoint: Some(endpoint.clone()),
        };
        // Run on its own thread with its own runtime, exactly as
        // `run_foreground` does — and not on this one, because
        // `platform::single_instance::InstanceGuard` holds a raw Windows
        // `HANDLE` and is therefore `!Send`, so the daemon's future cannot be
        // handed to `tokio::spawn`. Production never notices (it `block_on`s),
        // and doing the same here means the test drives the real entry path
        // rather than a rearranged one.
        let thread_paths = paths.clone();
        let joined = std::thread::Builder::new()
            .name(format!("sb-test-daemon-{name}"))
            .spawn(move || {
                // Quiet by default so a passing suite is readable; set
                // `SB_TEST_LOG=1` (with `RUST_LOG` and `--nocapture`) to watch
                // a failing daemon narrate itself.
                let quiet = std::env::var_os("SB_TEST_LOG").is_none();
                let control = crate::daemon::logging::init(&thread_paths, quiet);
                let rt = tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .expect("a runtime for the test daemon");
                rt.block_on(crate::daemon::run(
                    thread_paths,
                    crate::daemon::Surface::Headless,
                    control,
                    0,
                    Some(hooks),
                ))
            })
            .expect("spawn the test daemon thread");

        let runtime = tokio::time::timeout(Duration::from_secs(30), ready_rx)
            .await
            .expect("the daemon started within thirty seconds")
            .expect("the daemon signalled ready");

        // The kopia setup task runs in the background on purpose (a first run
        // downloads a binary), so a test that needs kopia waits for it here
        // rather than racing it.
        wait_for(Duration::from_secs(20), || runtime.kopia().is_some()).await;

        Harness {
            root,
            paths,
            endpoint,
            runtime,
            kopia_dir,
            kopia_exe,
            stop: Some(stop_tx),
            joined: Some(joined),
        }
    }

    /// A connected IPC client.
    pub async fn client(&self) -> Client {
        Client::connect(&self.endpoint).await.expect("connect to the daemon")
    }

    /// Send a request and unwrap the reply, panicking with the daemon's own
    /// message when it refuses — which is far more useful in a test failure
    /// than `Err(..)`.
    pub async fn call(&self, client: &Client, request: Request) -> Reply {
        let name = request.command();
        client
            .request(request)
            .await
            .unwrap_or_else(|e| panic!("`{name}` failed: {e}"))
    }

    /// Rewrite the fake kopia's script.
    pub fn script(&self, entries: &[(&str, &str)]) {
        script_in(&self.kopia_dir, entries);
    }

    /// Every argv the fake kopia has been invoked with.
    pub fn invocations(&self) -> Vec<Vec<String>> {
        let text = std::fs::read_to_string(self.kopia_dir.join("record.txt")).unwrap_or_default();
        text.split("--- INVOCATION ---")
            .skip(1)
            .map(|block| {
                block
                    .lines()
                    .filter_map(|l| l.strip_prefix("ARG\t").map(|a| a.to_string()))
                    .collect()
            })
            .collect()
    }

    /// Stop the daemon and wait for it to unwind.
    ///
    /// Takes `&mut self` rather than `self` so the private home survives the
    /// call: several tests assert on what shutdown *left behind* — the run
    /// history it flushed, the single-instance lock it released — and a
    /// consuming signature would delete all of it before they could look.
    /// `Drop` still cleans up at the end of the test.
    pub async fn shutdown(&mut self) -> superbackup_core::Result<()> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        let Some(handle) = self.joined.take() else { return Ok(()) };
        // Joining an OS thread blocks, so it happens off the runtime's
        // workers; a shutdown that hangs shows up as a test timeout rather
        // than as a deadlocked runtime.
        tokio::task::spawn_blocking(move || handle.join())
            .await
            .expect("the join task did not panic")
            .expect("the daemon thread did not panic")
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        // Best effort: on Windows a still-running child keeps the fake kopia
        // locked, and failing here would mask the real assertion.
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A fresh, private `SUPERBACKUP_HOME`.
pub fn fresh_home(name: &str) -> (PathBuf, Paths) {
    let root = std::env::temp_dir().join(format!(
        "sb-daemon-{name}-{}-{}",
        std::process::id(),
        Uuid::new_v4().simple()
    ));
    let paths = Paths::rooted_at(&root, false);
    paths.ensure().expect("create the private home");
    (root, paths)
}

/// An endpoint no other test — and no developer's daemon — will collide with.
pub fn private_endpoint(name: &str) -> String {
    let unique = format!("{name}-{}-{}", std::process::id(), Uuid::new_v4().simple());
    if cfg!(windows) {
        format!(r"\\.\pipe\sb-test-{unique}")
    } else {
        std::env::temp_dir().join(format!("sb-test-{unique}.sock")).display().to_string()
    }
}

// ---------------------------------------------------------------------------
// The fake kopia
// ---------------------------------------------------------------------------

/// Compile the scriptable fake kopia once per test binary.
///
/// The source is `crates/core/tests/kopia_support`'s, included by path so the
/// two suites cannot drift; only the plumbing around it is local, because the
/// daemon needs the binary inside its own home rather than in a `Scenario`.
pub fn fake_kopia_template() -> Option<PathBuf> {
    static BUILT: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    BUILT
        .get_or_init(|| {
            let dir = std::env::temp_dir().join("superbackup-fake-kopia-daemon");
            std::fs::create_dir_all(&dir).ok()?;
            let src = dir.join("fake_kopia.rs");
            std::fs::write(&src, crate::kopia_support::FAKE_KOPIA_SRC).ok()?;
            let exe = dir.join(exe_name(&format!("kopia-template-{}", std::process::id())));
            if exe.is_file() {
                return Some(exe);
            }
            let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
            let out = std::process::Command::new(&rustc)
                .args(["--edition", "2021", "-O", "-o"])
                .arg(&exe)
                .arg(&src)
                .output()
                .ok()?;
            if !out.status.success() {
                eprintln!(
                    "compiling the fake kopia failed: {}",
                    String::from_utf8_lossy(&out.stderr)
                );
                return None;
            }
            Some(exe)
        })
        .clone()
}

/// Copy the fake into a home's own `bin` directory and script it to succeed.
pub fn install_fake_kopia(root: &std::path::Path) -> (PathBuf, PathBuf) {
    let dir = root.join("kopia-bin");
    std::fs::create_dir_all(&dir).expect("kopia bin dir");
    let exe = dir.join(exe_name("kopia"));
    match fake_kopia_template() {
        Some(template) => {
            std::fs::copy(&template, &exe).expect("copy the fake kopia");
        }
        None => eprintln!("SKIPPED SETUP: rustc is unavailable, so kopia calls will fail"),
    }
    script_in(&dir, &[("mode", "ok")]);
    (dir, exe)
}

pub fn script_in(dir: &std::path::Path, entries: &[(&str, &str)]) {
    let body: String = entries.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
    std::fs::create_dir_all(dir).expect("control dir");
    std::fs::write(dir.join("control.txt"), body).expect("write control.txt");
}

pub fn exe_name(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_string()
    }
}

/// True when the fake kopia could be built. Tests that need a subprocess skip
/// loudly rather than silently when `rustc` is unavailable.
pub fn kopia_available() -> bool {
    fake_kopia_template().is_some()
}

/// A `snapshot create --json` manifest the driver will parse, with real stats
/// so the run history has numbers in it.
pub fn manifest_json(files: u64, bytes: u64) -> String {
    format!(
        r#"{{"id":"k{:016x}","source":{{"host":"pc","userName":"me","path":"C:\\src"}},
           "description":"superbackup","startTime":"2026-01-01T00:00:00Z",
           "endTime":"2026-01-01T00:01:00Z",
           "stats":{{"totalSize":{bytes},"fileCount":{files},"errorCount":0,
                     "ignoredErrorCount":0,"dirCount":3,"cachedFiles":0,
                     "nonCachedFiles":{files}}},
           "rootEntry":{{"name":"src","type":"d","obj":"kdeadbeef"}},
           "tags":{{}}}}"#,
        bytes
    )
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// A local kopia repository destination whose passphrase is stored in the
/// vault under a handle the daemon will generate on demand.
pub fn repository(name: &str, path: PathBuf) -> Destination {
    let id = Uuid::new_v4();
    Destination {
        id,
        name: name.to_string(),
        kind: DestinationKind::LocalRepository { path },
        encryption: Some(EncryptionSettings {
            passphrase_source: PassphraseSource::Generated,
            ..EncryptionSettings::default()
        }),
        passphrase_ref: Some(SecretRef::new("repo-passphrase", &id)),
        retention: Default::default(),
        enabled: true,
        auto_discovered: false,
        bandwidth: None,
        created_at: chrono::Utc::now(),
        last_verified_at: None,
    }
}

/// A folder mirror: no repository, no kopia, no secrets.
pub fn mirror(name: &str, path: PathBuf) -> Destination {
    Destination {
        id: Uuid::new_v4(),
        name: name.to_string(),
        kind: DestinationKind::LocalMirror { path },
        encryption: None,
        passphrase_ref: None,
        retention: Default::default(),
        enabled: true,
        auto_discovered: false,
        bandwidth: None,
        created_at: chrono::Utc::now(),
        last_verified_at: None,
    }
}

/// A manual job over one source, writing to the given destinations.
pub fn job(name: &str, source: PathBuf, destinations: Vec<Uuid>) -> Job {
    Job {
        id: Uuid::new_v4(),
        name: name.to_string(),
        project_id: None,
        description: String::new(),
        sources: vec![Source::new(source)],
        destination_ids: destinations,
        schedule: superbackup_core::model::Schedule::Manual,
        exclusions: Default::default(),
        bandwidth: None,
        retention: None,
        enabled: true,
        timeout_minutes: None,
        hooks: Default::default(),
        continue_on_destination_error: true,
        created_at: chrono::Utc::now(),
        tags: vec![],
    }
}

/// A small tree of real files, so the mirror engine has something to copy.
pub fn seed_tree(root: &std::path::Path, files: usize) -> PathBuf {
    let dir = root.join("sources");
    std::fs::create_dir_all(dir.join("nested")).expect("source tree");
    for i in 0..files {
        std::fs::write(dir.join(format!("file-{i}.txt")), format!("contents {i}"))
            .expect("write a source file");
    }
    std::fs::write(dir.join("nested").join("deep.txt"), b"deep").expect("write a nested file");
    dir
}

// ---------------------------------------------------------------------------
// Waiting
// ---------------------------------------------------------------------------

/// Poll until `condition` holds, or give up. Returns whether it held.
///
/// Polling rather than sleeping a fixed time: a fixed sleep is either flaky on
/// a loaded CI machine or slow on a fast one, and this is neither.
pub async fn wait_for(limit: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = tokio::time::Instant::now() + limit;
    loop {
        if condition() {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Poll an async condition until it holds.
pub async fn wait_for_async<F, Fut>(limit: Duration, mut condition: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + limit;
    loop {
        if condition().await {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
