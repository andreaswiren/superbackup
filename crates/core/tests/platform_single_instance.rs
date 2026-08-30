//! The single-instance guard, against real lock files.
//!
//! No system state outside a per-test temporary directory is touched, and on
//! Windows the named mutex is scoped to a name derived from that directory.

use std::path::PathBuf;
use superbackup_core::paths::Paths;
use superbackup_core::platform::single_instance::{self, LockOutcome, LockRecord};
use uuid::Uuid;

struct TempPaths {
    root: PathBuf,
    paths: Paths,
}

impl TempPaths {
    fn new(tag: &str) -> TempPaths {
        let root = std::env::temp_dir().join(format!(
            "sb-si-{tag}-{}-{}",
            std::process::id(),
            Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&root).expect("create temp dir");
        let paths = Paths::rooted_at(&root, false);
        TempPaths { root, paths }
    }
}

impl Drop for TempPaths {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn foreign_record(pid: u32) -> LockRecord {
    LockRecord {
        pid,
        nonce: Uuid::new_v4(),
        acquired_at: chrono::Utc::now(),
        executable: Some(PathBuf::from("/opt/superbackup/superbackup")),
        endpoint: Some("/run/superbackup/superbackup.sock".into()),
        service_scope: false,
    }
}

fn write_lock(paths: &Paths, record: &LockRecord) {
    std::fs::create_dir_all(&paths.data_dir).expect("data dir");
    std::fs::write(paths.lock_file(), serde_json::to_vec(record).expect("json")).expect("write");
}

#[test]
fn acquiring_writes_a_record_and_releasing_removes_it() {
    let t = TempPaths::new("basic");
    let guard = match single_instance::acquire(&t.paths).expect("acquire") {
        LockOutcome::Acquired(g) => g,
        LockOutcome::AlreadyRunning(r) => panic!("nothing else can hold a fresh path: {r:?}"),
    };
    assert_eq!(guard.path(), t.paths.lock_file());
    let on_disk = single_instance::read_lock(guard.path()).expect("a record");
    assert_eq!(on_disk.pid, std::process::id());
    assert_eq!(
        on_disk.endpoint.as_deref(),
        Some(t.paths.ipc_endpoint().as_str()),
        "a second launch must be able to find the first"
    );
    let path = guard.path().to_path_buf();
    guard.release();
    assert!(!path.exists());
}

#[test]
fn a_lock_held_by_a_live_process_is_refused_with_a_useful_message() {
    let t = TempPaths::new("live");
    write_lock(&t.paths, &foreign_record(4242));

    match single_instance::acquire_with(&t.paths, |_| true).expect("acquire") {
        LockOutcome::AlreadyRunning(record) => {
            assert_eq!(record.pid, 4242);
            let message = record.describe();
            assert!(message.contains("already running"), "{message}");
            assert!(message.contains("4242"), "name the process: {message}");
            assert!(message.contains("listening on"), "and where to reach it: {message}");
        }
        LockOutcome::Acquired(_) => panic!("stole a lock from a live process"),
    }
}

#[test]
fn a_lock_left_by_a_crashed_process_is_taken_over_safely() {
    let t = TempPaths::new("crashed");
    let dead = foreign_record(999_997);
    write_lock(&t.paths, &dead);

    let guard = match single_instance::acquire_with(&t.paths, |_| false).expect("acquire") {
        LockOutcome::Acquired(g) => g,
        LockOutcome::AlreadyRunning(r) => panic!("a crash must not lock the user out: {r:?}"),
    };
    let on_disk = single_instance::read_lock(guard.path()).expect("a record");
    assert_eq!(on_disk.pid, std::process::id());
    assert_ne!(on_disk.nonce, dead.nonce, "the takeover must be a full replacement");
    assert_eq!(on_disk.nonce, guard.record().nonce, "and the nonce must be ours");
}

#[test]
fn a_lock_from_a_recycled_pid_is_not_mistaken_for_a_live_holder() {
    let t = TempPaths::new("recycled");
    // A PID that exists, but is running something else entirely. The injected
    // predicate models what `holder_is_alive` decides by comparing executables.
    let mut stale = foreign_record(1);
    stale.executable = Some(PathBuf::from("/usr/sbin/some-other-daemon"));
    write_lock(&t.paths, &stale);

    let outcome = single_instance::acquire_with(&t.paths, |record| {
        // "PID 1 exists, but it is not superbackup."
        record
            .executable
            .as_ref()
            .and_then(|p| p.file_stem())
            .map(|s| s == "superbackup")
            .unwrap_or(false)
    })
    .expect("acquire");
    assert!(
        matches!(outcome, LockOutcome::Acquired(_)),
        "a recycled PID running something else must not hold our lock"
    );
}

#[test]
fn a_truncated_lock_file_does_not_wedge_startup() {
    let t = TempPaths::new("truncated");
    std::fs::create_dir_all(&t.paths.data_dir).expect("data dir");
    // Exactly what a power failure part-way through a write leaves behind.
    std::fs::write(t.paths.lock_file(), b"{\"pid\":123,\"non").expect("write");

    let outcome = single_instance::acquire_with(&t.paths, |_| true).expect("acquire");
    assert!(
        matches!(outcome, LockOutcome::Acquired(_)),
        "an unreadable lock names no live process and must never block startup"
    );
}

#[test]
fn two_different_configurations_do_not_contend() {
    let a = TempPaths::new("multi-a");
    let b = TempPaths::new("multi-b");
    let ga = single_instance::acquire(&a.paths).expect("a").into_guard().expect("a guard");
    let gb = single_instance::acquire(&b.paths).expect("b").into_guard().expect("b guard");
    assert_ne!(ga.path(), gb.path());
    assert_ne!(
        single_instance::mutex_name(ga.path()),
        single_instance::mutex_name(gb.path()),
        "two SUPERBACKUP_HOME roots must not share a kernel object"
    );
}

#[test]
fn the_user_instance_and_the_service_instance_are_separate() {
    let root = std::env::temp_dir().join(format!("sb-si-scope-{}", Uuid::new_v4().simple()));
    let user = Paths::rooted_at(root.join("user"), false);
    let service = Paths::rooted_at(root.join("service"), true);
    assert_ne!(user.lock_file(), service.lock_file());
    assert_ne!(user.ipc_endpoint(), service.ipc_endpoint());

    let gu = single_instance::acquire(&user).expect("user").into_guard().expect("guard");
    let gs = single_instance::acquire(&service).expect("service").into_guard().expect("guard");
    assert!(!gu.record().service_scope);
    assert!(gs.record().service_scope);
    drop(gu);
    drop(gs);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_real_liveness_check_recognises_this_process() {
    let mut me = foreign_record(std::process::id());
    me.executable = std::env::current_exe().ok();
    assert!(
        single_instance::holder_is_alive(&me),
        "the test binary is definitely running"
    );

    let mut ghost = foreign_record(0xffff_fff0);
    ghost.executable = std::env::current_exe().ok();
    assert!(
        !single_instance::holder_is_alive(&ghost),
        "a PID that high is not in use"
    );
}
