//! Shared scaffolding for the kopia test suites.
//!
//! Both `kopia_driver.rs` and `kopia_install.rs` need the same thing: a real
//! process that behaves like kopia, on a machine where kopia is not installed.
//! It is a small Rust program compiled by `rustc` at test time and scripted
//! through a `control.txt` sitting beside its own executable — deliberately
//! *not* through environment variables, because the driver builds the child
//! environment from empty and an env-driven fake would be untestable.
//!
//! Everything is `pub` and `dead_code` is allowed because each test binary uses
//! a different subset.

#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use superbackup_core::kopia::{KopiaBinary, KopiaSource, KopiaVersion};
use superbackup_core::paths::Paths;

/// Source of the stand-in binary.
///
/// Modes, selected by `mode=` in `control.txt`:
/// * `ok` (default) — exit 0, optionally printing `stdout` / `stdout_file`.
/// * `snapshot` — replay recorded `\r`-delimited progress frames and an ignored
///   -error line on stderr, then the `--json` manifest on stdout.
/// * `sync` — replay `repository sync-to`'s own progress renderer: the
///   "…to copy" inventory line followed by "Copied N blobs" frames.
/// * `synctruncated` — the same, stopping before the final frame, which is what
///   kopia's rate-limited sync output does most of the time.
/// * `hang` — run until killed, appending to `heartbeat.txt` so a test can see
///   whether the process is still alive.
/// * `flood` — write 20 000 progress frames as fast as possible, to prove a
///   stalled event consumer cannot block the pipe.
/// * `fail` — print `stderr` and exit with `exit`.
///
/// Every invocation is appended to `record.txt`, argv and environment both.
pub const FAKE_KOPIA_SRC: &str = r##"
use std::io::{Read, Write};
use std::path::PathBuf;

fn here() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn control() -> std::collections::HashMap<String, String> {
    let mut m = std::collections::HashMap::new();
    if let Ok(text) = std::fs::read_to_string(here().join("control.txt")) {
        for line in text.lines() {
            if let Some((k, v)) = line.split_once('=') {
                m.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
    }
    m
}

fn emit_stdout(ctl: &std::collections::HashMap<String, String>) {
    if let Some(p) = ctl.get("stdout_file") {
        if let Ok(mut f) = std::fs::File::open(p) {
            let mut s = Vec::new();
            let _ = f.read_to_end(&mut s);
            let _ = std::io::stdout().write_all(&s);
        }
    }
    if let Some(t) = ctl.get("stdout") {
        println!("{}", t);
    }
    let _ = std::io::stdout().flush();
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let ctl = control();
    let dir = here();

    let mut rec = String::from("--- INVOCATION ---\n");
    for a in &args {
        rec.push_str("ARG\t");
        rec.push_str(a);
        rec.push('\n');
    }
    for (k, v) in std::env::vars() {
        rec.push_str("ENVNAME\t");
        rec.push_str(&k);
        rec.push('\n');
        if k.starts_with("KOPIA_") || k.starts_with("AWS_") {
            rec.push_str("ENV\t");
            rec.push_str(&k);
            rec.push('\t');
            rec.push_str(&v);
            rec.push('\n');
        }
    }
    if let Ok(mut f) =
        std::fs::OpenOptions::new().create(true).append(true).open(dir.join("record.txt"))
    {
        let _ = f.write_all(rec.as_bytes());
    }

    if args.iter().any(|a| a == "--version") {
        let v = ctl
            .get("version")
            .cloned()
            .unwrap_or_else(|| "0.21.5 build: fake from: kopia/kopia".to_string());
        println!("{}", v);
        return;
    }

    match ctl.get("mode").map(|s| s.as_str()).unwrap_or("ok") {
        "hang" => {
            let hb = dir.join("heartbeat.txt");
            loop {
                if let Ok(mut f) =
                    std::fs::OpenOptions::new().create(true).append(true).open(&hb)
                {
                    let _ = f.write_all(b".");
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
        "flood" => {
            let e = std::io::stderr();
            let mut e = e.lock();
            for i in 1..=20000u64 {
                let _ = write!(
                    e,
                    " | 0 hashing, {} hashed ({} MB), 0 cached (0 B), uploaded {} MB, estimating...\r",
                    i, i, i
                );
            }
            let _ = e.flush();
            emit_stdout(&ctl);
        }
        "snapshot" => {
            {
                let e = std::io::stderr();
                let mut e = e.lock();
                let _ = write!(e, " | 3 hashing, 1204 hashed (1.2 GB), 88 cached (410 MB), uploaded 903.1 MB, estimating...\r");
                let _ = e.flush();
                std::thread::sleep(std::time::Duration::from_millis(20));
                let _ = write!(e, " / 2 hashing, 8100 hashed (3.1 GB), 900 cached (1.4 GB), uploaded 1.5 GB, estimated 6.5 GB (69.2%) 1m5s left\r");
                let _ = e.flush();
                std::thread::sleep(std::time::Duration::from_millis(20));
                let _ = write!(e, "\n ! Ignored error when processing \"C:\\src\\target\\lock\": access is denied\n");
                let _ = write!(e, " * 0 hashing, 15316 hashed (4.4 GB), 1201 cached (2.1 GB), uploaded 1.9 GB (1 errors ignored), estimated 6.5 GB (100.0%) 0s left\r\n");
                let _ = e.flush();
            }
            emit_stdout(&ctl);
        }
        "sync" => {
            let e = std::io::stderr();
            let mut e = e.lock();
            let _ = write!(e, "\r  Found 41230 BLOBs (88.1 GB) in the source repository, 512 (1.2 GB) to copy");
            let _ = e.flush();
            std::thread::sleep(std::time::Duration::from_millis(20));
            let _ = write!(e, "\r  Copied 214 blobs (612.4 MB), Speed: 18.1 MB/s, ETA: 42s");
            let _ = e.flush();
            std::thread::sleep(std::time::Duration::from_millis(20));
            let _ = write!(e, "\r  Copied 512 blobs (1.2 GB), Speed: 19.0 MB/s, ETA: 0s      \n");
            let _ = e.flush();
            emit_stdout(&ctl);
        }
        "synctruncated" => {
            let e = std::io::stderr();
            let mut e = e.lock();
            let _ = write!(e, "\r  Found 41230 BLOBs (88.1 GB) in the source repository, 512 (1.2 GB) to copy");
            let _ = e.flush();
            std::thread::sleep(std::time::Duration::from_millis(20));
            let _ = write!(e, "\r  Copied 214 blobs (612.4 MB), Speed: 18.1 MB/s, ETA: 42s");
            let _ = e.flush();
            emit_stdout(&ctl);
        }
        "fail" => {
            let text = ctl.get("stderr").cloned().unwrap_or_default().replace("\\n", "\n");
            eprintln!("{}", text);
            let code: i32 = ctl.get("exit").and_then(|s| s.parse().ok()).unwrap_or(1);
            std::process::exit(code);
        }
        _ => emit_stdout(&ctl),
    }
}
"##;

/// Compile the fake once per test binary. `Err` carries a human explanation.
pub fn fake_kopia_template() -> Result<&'static Path, &'static str> {
    static BUILT: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    let built = BUILT.get_or_init(|| {
        let dir = std::env::temp_dir().join("superbackup-fake-kopia");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let src = dir.join("fake_kopia.rs");
        std::fs::write(&src, FAKE_KOPIA_SRC).map_err(|e| e.to_string())?;
        let exe = dir.join(exe_name(&format!("kopia-template-{}", std::process::id())));
        let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
        let out = std::process::Command::new(&rustc)
            .arg("--edition")
            .arg("2021")
            .arg("-O")
            .arg("-o")
            .arg(&exe)
            .arg(&src)
            .output()
            .map_err(|e| format!("could not run {rustc}: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "compiling the fake kopia failed: {}",
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        Ok(exe)
    });
    match built {
        Ok(p) => Ok(p.as_path()),
        Err(_) => Err("rustc is unavailable, so the process-level kopia tests were skipped"),
    }
}

pub fn exe_name(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_string()
    }
}

/// Skip a process-level test loudly rather than silently when there is no rustc.
#[macro_export]
macro_rules! fake_or_skip {
    () => {
        match $crate::kopia_support::fake_kopia_template() {
            Ok(p) => p,
            Err(why) => {
                eprintln!("SKIPPED: {why}");
                return;
            }
        }
    };
}

/// One isolated scenario: its own superbackup home, its own copy of the fake
/// kopia, and its own script for it.
pub struct Scenario {
    pub root: PathBuf,
    pub bin_dir: PathBuf,
    pub exe: PathBuf,
}

impl Scenario {
    pub fn new(name: &str) -> Scenario {
        let template = fake_kopia_template().map(|p| p.to_path_buf()).unwrap_or_default();
        let root = std::env::temp_dir().join(format!(
            "sb-kopia-{name}-{}-{}",
            std::process::id(),
            unique()
        ));
        let bin_dir = root.join("bin");
        std::fs::create_dir_all(&bin_dir).expect("scenario bin dir");
        let exe = bin_dir.join(exe_name("kopia"));
        if !template.as_os_str().is_empty() {
            std::fs::copy(&template, &exe).expect("copy fake kopia");
        }
        let s = Scenario { root, bin_dir, exe };
        s.script(&[("mode", "ok")]);
        s
    }

    /// Write the fake's instructions.
    pub fn script(&self, entries: &[(&str, &str)]) {
        self.script_in(&self.bin_dir, entries);
    }

    /// Script a fake kopia living somewhere other than the scenario's own bin
    /// directory — used to set up a second, differently-versioned kopia.
    pub fn script_in(&self, dir: &Path, entries: &[(&str, &str)]) {
        let body: String = entries.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
        std::fs::create_dir_all(dir).expect("control dir");
        std::fs::write(dir.join("control.txt"), body).expect("write control");
    }

    /// Install a second copy of the fake kopia at `path`, with its own script.
    pub fn install_fake_at(&self, path: &Path, entries: &[(&str, &str)]) {
        let template = fake_kopia_template().map(|p| p.to_path_buf()).unwrap_or_default();
        if template.as_os_str().is_empty() {
            return;
        }
        let dir = path.parent().expect("path has a parent");
        std::fs::create_dir_all(dir).expect("target dir");
        std::fs::copy(&template, path).expect("copy fake kopia");
        self.script_in(dir, entries);
    }

    /// Point the fake at a file to print on stdout.
    pub fn stdout_file(&self, name: &str, contents: &str) -> PathBuf {
        let p = self.root.join(name);
        std::fs::write(&p, contents).expect("write stdout fixture");
        p
    }

    pub fn paths(&self) -> Paths {
        let p = Paths::rooted_at(&self.root, false);
        p.ensure().expect("create superbackup dirs");
        p
    }

    pub fn binary(&self) -> KopiaBinary {
        KopiaBinary::assume(&self.exe, KopiaVersion::new(0, 21, 5), KopiaSource::Configured)
    }

    pub fn record(&self) -> Vec<Invocation> {
        self.record_in(&self.bin_dir)
    }

    pub fn record_in(&self, dir: &Path) -> Vec<Invocation> {
        let text = std::fs::read_to_string(dir.join("record.txt")).unwrap_or_default();
        parse_record(&text)
    }

    /// The single invocation the test expects. Panics with the whole recording
    /// when there was not exactly one, which is far more useful than an index
    /// panic.
    pub fn only(&self) -> Invocation {
        let mut r = self.record();
        assert_eq!(r.len(), 1, "expected exactly one kopia invocation, got {r:#?}");
        r.remove(0)
    }

    pub fn heartbeat(&self) -> PathBuf {
        self.bin_dir.join("heartbeat.txt")
    }
}

impl Drop for Scenario {
    fn drop(&mut self) {
        // Best effort: on Windows a still-running child would keep the exe
        // locked, and a failure here would mask the real assertion failure.
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

pub fn unique() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    t ^ (N.fetch_add(1, Ordering::Relaxed) << 32)
}

/// One recorded launch of the fake kopia.
#[derive(Debug, Clone)]
pub struct Invocation {
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub env_names: HashSet<String>,
}

impl Invocation {
    pub fn joined(&self) -> String {
        self.args.join(" ")
    }
    pub fn has_flag(&self, flag: &str) -> bool {
        self.args.iter().any(|a| a == flag || a.starts_with(&format!("{flag}=")))
    }
    pub fn flag_value(&self, flag: &str) -> Option<String> {
        let prefix = format!("{flag}=");
        self.args.iter().find_map(|a| a.strip_prefix(&prefix).map(|s| s.to_string()))
    }
    pub fn flag_values(&self, flag: &str) -> Vec<String> {
        let prefix = format!("{flag}=");
        self.args.iter().filter_map(|a| a.strip_prefix(&prefix).map(|s| s.to_string())).collect()
    }
    /// The subcommand words, i.e. the arguments that are not flags.
    pub fn words(&self) -> Vec<String> {
        self.args.iter().filter(|a| !a.starts_with("--")).cloned().collect()
    }
}

pub fn parse_record(text: &str) -> Vec<Invocation> {
    let mut out: Vec<Invocation> = Vec::new();
    for block in text.split("--- INVOCATION ---").skip(1) {
        let mut inv =
            Invocation { args: Vec::new(), env: HashMap::new(), env_names: HashSet::new() };
        for line in block.lines() {
            let mut parts = line.split('\t');
            match parts.next() {
                Some("ARG") => {
                    if let Some(a) = parts.next() {
                        inv.args.push(a.to_string());
                    }
                }
                Some("ENVNAME") => {
                    if let Some(n) = parts.next() {
                        inv.env_names.insert(n.to_string());
                    }
                }
                Some("ENV") => {
                    if let (Some(k), Some(v)) = (parts.next(), parts.next()) {
                        inv.env.insert(k.to_string(), v.to_string());
                    }
                }
                _ => {}
            }
        }
        out.push(inv);
    }
    out
}

/// Serialises the tests that have to mutate `PATH`.
///
/// `which::which` reads `PATH` from the process environment, so exercising the
/// system-binary branch of discovery means changing it. Doing that in a
/// multi-threaded test binary needs a lock, and the original value is put back
/// when the guard drops even if the test panics.
static PATH_LOCK: Mutex<()> = Mutex::new(());

pub struct PathGuard {
    _lock: MutexGuard<'static, ()>,
    original: Option<OsString>,
}

impl PathGuard {
    pub fn prepend(dir: &Path) -> PathGuard {
        let lock = PATH_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let original = std::env::var_os("PATH");
        let mut entries = vec![dir.to_path_buf()];
        if let Some(p) = &original {
            entries.extend(std::env::split_paths(p));
        }
        let joined = std::env::join_paths(entries).expect("join PATH");
        std::env::set_var("PATH", joined);
        PathGuard { _lock: lock, original }
    }

    /// Remove every directory from `PATH`, so discovery cannot pick up a real
    /// kopia the developer happens to have installed.
    pub fn empty() -> PathGuard {
        let lock = PATH_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let original = std::env::var_os("PATH");
        std::env::set_var("PATH", "");
        PathGuard { _lock: lock, original }
    }
}

impl Drop for PathGuard {
    fn drop(&mut self) {
        match self.original.take() {
            Some(p) => std::env::set_var("PATH", p),
            None => std::env::remove_var("PATH"),
        }
    }
}
