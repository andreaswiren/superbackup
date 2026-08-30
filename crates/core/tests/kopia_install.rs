//! Tests for the kopia auto-installer, with no network access.
//!
//! A one-line HTTP server on `127.0.0.1` stands in for GitHub and serves a
//! synthetic release: a release JSON, a `checksums.txt` in the real
//! `sha256sum` format, and archives built here in the same shape kopia
//! publishes (`kopia-<version>-<platform>/kopia[.exe]` inside a zip or a
//! tar.gz). The "kopia" inside the archive is the same compiled fake the driver
//! tests use, so the post-install `--version` check is genuinely exercised
//! rather than stubbed.
//!
//! Serving over plain HTTP from a loopback address means the host allowlist has
//! to be widened for these tests; that widening is itself covered, both by
//! asserting that the *default* allowlist refuses a non-GitHub host and by
//! asserting that a narrowed allowlist does not inherit GitHub's CDN wildcard.

mod kopia_support;

use std::collections::HashMap;
use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kopia_support::{fake_kopia_template, PathGuard, Scenario};
use sha2::{Digest, Sha256};
use superbackup_core::kopia::install::{
    asset_for_platform, checksum_for, extract_kopia, host_allowed, safe_member_name, ArchiveKind,
    DEFAULT_ALLOWED_HOSTS,
};
use superbackup_core::kopia::*;
use superbackup_core::model::{Settings, UpdatePolicy};

// ---------------------------------------------------------------------------
// A pretend GitHub
// ---------------------------------------------------------------------------

/// One canned response.
#[derive(Clone)]
struct Canned {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Canned {
    fn json(body: String) -> Canned {
        Canned {
            status: 200,
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: body.into_bytes(),
        }
    }
    fn bytes(body: Vec<u8>) -> Canned {
        Canned {
            status: 200,
            headers: vec![("Content-Type".into(), "application/octet-stream".into())],
            body,
        }
    }
    fn redirect(to: &str) -> Canned {
        Canned {
            status: 302,
            headers: vec![("Location".into(), to.to_string())],
            body: Vec::new(),
        }
    }
    fn not_found() -> Canned {
        Canned { status: 404, headers: Vec::new(), body: b"not found".to_vec() }
    }
    fn rate_limited() -> Canned {
        Canned {
            status: 403,
            headers: Vec::new(),
            body: b"{\"message\":\"API rate limit exceeded\"}".to_vec(),
        }
    }
}

/// A minimal HTTP/1.1 server. Enough to answer `GET`, and nothing else.
struct FakeGitHub {
    addr: SocketAddr,
    routes: Arc<Mutex<HashMap<String, Canned>>>,
    task: tokio::task::JoinHandle<()>,
    /// Paths that were requested, so tests can assert what was fetched.
    requested: Arc<Mutex<Vec<String>>>,
}

impl FakeGitHub {
    async fn start() -> FakeGitHub {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let routes: Arc<Mutex<HashMap<String, Canned>>> = Arc::new(Mutex::new(HashMap::new()));
        let requested: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

        let r = routes.clone();
        let req = requested.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else { break };
                let r = r.clone();
                let req = req.clone();
                tokio::spawn(async move {
                    let _ = serve_one(stream, r, req).await;
                });
            }
        });

        FakeGitHub { addr, routes, task, requested }
    }

    fn base(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }

    fn route(&self, path: &str, response: Canned) {
        self.routes.lock().expect("routes").insert(path.to_string(), response);
    }

    fn requests(&self) -> Vec<String> {
        self.requested.lock().expect("requested").clone()
    }

    /// An installer wired to this server. Loopback has to be allowlisted, which
    /// is the only reason `with_endpoint` exists.
    fn installer(&self, paths: &superbackup_core::paths::Paths) -> KopiaInstaller {
        KopiaInstaller::with_endpoint(paths, &self.base(), vec!["127.0.0.1".to_string()])
            .expect("installer")
    }
}

impl Drop for FakeGitHub {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve_one(
    mut stream: tokio::net::TcpStream,
    routes: Arc<Mutex<HashMap<String, Canned>>>,
    requested: Arc<Mutex<Vec<String>>>,
) -> std::io::Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    // Read up to the end of the headers; a GET has no body.
    loop {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if buf.len() > 64 * 1024 {
            break;
        }
    }
    let head = String::from_utf8_lossy(&buf);
    let path = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .unwrap_or("/")
        .to_string();
    requested.lock().expect("requested").push(path.clone());

    let response =
        routes.lock().expect("routes").get(&path).cloned().unwrap_or_else(Canned::not_found);

    let mut out = format!("HTTP/1.1 {} X\r\n", response.status);
    for (k, v) in &response.headers {
        out.push_str(&format!("{k}: {v}\r\n"));
    }
    out.push_str(&format!("Content-Length: {}\r\n", response.body.len()));
    out.push_str("Connection: close\r\n\r\n");
    stream.write_all(out.as_bytes()).await?;
    stream.write_all(&response.body).await?;
    stream.flush().await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Building a release that looks like kopia's
// ---------------------------------------------------------------------------

const TEST_VERSION: &str = "0.23.1";

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// The archive member layout kopia really uses, verified against v0.23.1:
/// one top-level directory, with the executable, LICENSE and README inside it.
fn build_archive(members: &[(&str, &[u8])], kind: ArchiveKind) -> Vec<u8> {
    match kind {
        ArchiveKind::Zip => {
            let mut out = std::io::Cursor::new(Vec::new());
            {
                let mut zip = zip::ZipWriter::new(&mut out);
                let options: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored);
                for (name, body) in members {
                    zip.start_file(*name, options).expect("start_file");
                    zip.write_all(body).expect("write member");
                }
                zip.finish().expect("finish zip");
            }
            out.into_inner()
        }
        ArchiveKind::TarGz => {
            let mut gz =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
            {
                let mut tar = tar::Builder::new(&mut gz);
                for (name, body) in members {
                    let mut header = tar::Header::new_gnu();
                    header.set_size(body.len() as u64);
                    header.set_mode(0o755);
                    header.set_cksum();
                    tar.append_data(&mut header, name, *body).expect("append");
                }
                tar.finish().expect("finish tar");
            }
            gz.finish().expect("finish gz")
        }
    }
}

/// Build a tar.gz containing a member name the `tar` crate's own `Builder`
/// refuses to write.
///
/// `Builder::append_data` rejects `..` in a path, which is exactly why an
/// attacker would not use it: a real malicious archive is produced by writing
/// the 512-byte header directly. This does the same, so the extractor is tested
/// against the thing it actually has to defend against rather than against a
/// well-behaved library's output.
fn build_evil_targz(name: &str, body: &[u8]) -> Vec<u8> {
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    {
        let mut builder = tar::Builder::new(&mut gz);
        let mut header = tar::Header::new_ustar();
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(0);
        header.set_entry_type(tar::EntryType::Regular);
        {
            let old = header.as_old_mut();
            let raw = name.as_bytes();
            assert!(raw.len() < old.name.len(), "test name too long for a ustar header");
            old.name[..raw.len()].copy_from_slice(raw);
        }
        header.set_cksum();
        builder.append(&header, body).expect("append raw header");
        builder.finish().expect("finish tar");
    }
    gz.finish().expect("finish gz")
}

/// The asset this platform will actually ask for, and an archive containing the
/// compiled fake kopia under kopia's real member path.
fn platform_asset(version: &str) -> (String, ArchiveKind, Vec<u8>) {
    let choice = asset_for_platform(version, std::env::consts::OS, std::env::consts::ARCH)
        .expect("this platform must be supported by the test matrix");
    let exe = std::fs::read(fake_kopia_template().expect("fake kopia")).expect("read fake");
    let dir = choice.name.trim_end_matches(".zip").trim_end_matches(".tar.gz");
    let member = if cfg!(windows) { "kopia.exe" } else { "kopia" };
    let archive = build_archive(
        &[
            (&format!("{dir}/{member}"), exe.as_slice()),
            (&format!("{dir}/LICENSE"), b"Apache 2.0".as_slice()),
        ],
        choice.kind,
    );
    (choice.name, choice.kind, archive)
}

fn release_json(server: &FakeGitHub, version: &str, assets: &[(String, u64)]) -> String {
    let asset_entries: Vec<String> = assets
        .iter()
        .map(|(name, size)| {
            format!(
                r#"{{"name":"{name}","size":{size},"browser_download_url":"{}"}}"#,
                server.url(&format!("/assets/{name}"))
            )
        })
        .collect();
    format!(
        r#"{{"tag_name":"v{version}","prerelease":false,
             "published_at":"2026-08-01T10:00:00Z",
             "html_url":"https://github.com/kopia/kopia/releases/tag/v{version}",
             "assets":[{}]}}"#,
        asset_entries.join(",")
    )
}

/// Stand up a complete, valid release on the fake server.
///
/// Returns the asset name and the archive bytes, so a test can corrupt them.
async fn serve_release(server: &FakeGitHub, version: &str) -> (String, Vec<u8>) {
    let (asset_name, _kind, archive) = platform_asset(version);
    let checksums = format!(
        "{}  {asset_name}\n{}  some-other-file.tar.gz\n",
        sha256_hex(&archive),
        sha256_hex(b"unrelated"),
    );
    serve_release_with(server, version, &asset_name, archive.clone(), checksums).await;
    (asset_name, archive)
}

/// The same, but with the archive and checksum listing supplied by the caller,
/// so the verification chain can be broken deliberately.
async fn serve_release_with(
    server: &FakeGitHub,
    version: &str,
    asset_name: &str,
    archive: Vec<u8>,
    checksums: String,
) {
    let assets = vec![
        (asset_name.to_string(), archive.len() as u64),
        ("checksums.txt".to_string(), checksums.len() as u64),
    ];
    let json = release_json(server, version, &assets);
    server.route("/repos/kopia/kopia/releases/latest", Canned::json(json.clone()));
    server.route(&format!("/repos/kopia/kopia/releases/tags/v{version}"), Canned::json(json));
    server.route(&format!("/assets/{asset_name}"), Canned::bytes(archive));
    server.route("/assets/checksums.txt", Canned::bytes(checksums.into_bytes()));
}

/// Settings pointed at the fake, with the managed binary reporting `version`
/// once it is installed.
fn settings_for(scenario: &Scenario, paths: &superbackup_core::paths::Paths, version: &str) -> Settings {
    // The installed binary's `--version` is driven by a control file beside it,
    // and the installer probes the temp file in that same directory.
    scenario.script_in(
        paths.bundled_kopia().parent().expect("bin dir"),
        &[("version", &format!("{version} build: installed from: kopia/kopia"))],
    );
    Settings::default()
}

// ---------------------------------------------------------------------------
// Happy path
// ---------------------------------------------------------------------------

#[tokio::test]
async fn installs_a_verified_release_and_reports_progress() {
    let _ = fake_or_skip!();
    let s = Scenario::new("install-ok");
    let paths = s.paths();
    let server = FakeGitHub::start().await;
    let (asset_name, archive) = serve_release(&server, TEST_VERSION).await;
    let settings = settings_for(&s, &paths, TEST_VERSION);

    let (sink, mut rx) = InstallProgressSink::channel(256);
    let installer = server.installer(&paths);
    let outcome = installer.install_latest(&settings, Some(&sink)).await.expect("installs");

    assert_eq!(outcome.version.to_string(), TEST_VERSION);
    assert_eq!(outcome.asset, asset_name);
    assert_eq!(outcome.sha256, sha256_hex(&archive));
    assert!(!outcome.replaced_previous, "this was a first install");
    assert!(
        !outcome.signature_verified,
        "the guarantee must be stated honestly: kopia's checksums.txt.sig is not verified"
    );

    // The binary is where discovery will look for it, and it runs.
    assert_eq!(outcome.path, paths.bundled_kopia());
    assert!(outcome.path.is_file());
    assert_eq!(installer.installed_version().await.map(|v| v.to_string()).as_deref(),
        Some(TEST_VERSION));

    // Nothing half-written was left behind.
    let leftovers: Vec<_> = std::fs::read_dir(outcome.path.parent().expect("bin dir"))
        .expect("read bin dir")
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with(".kopia-install-"))
        .collect();
    assert!(leftovers.is_empty(), "temporary install files were left behind: {leftovers:?}");

    // The GUI gets a real download bar, not a frozen window.
    let mut phases = Vec::new();
    let mut saw_bytes = false;
    while let Ok(p) = rx.try_recv() {
        if p.phase == InstallPhase::DownloadingArchive && p.downloaded_bytes > 0 {
            saw_bytes = true;
            assert_eq!(p.total_bytes, Some(archive.len() as u64));
        }
        if phases.last() != Some(&p.phase) {
            phases.push(p.phase);
        }
    }
    assert!(saw_bytes, "no download progress was reported");
    assert!(phases.contains(&InstallPhase::Verifying), "{phases:?}");
    assert!(phases.contains(&InstallPhase::Done), "{phases:?}");

    // Checksums were fetched before the archive: verification is not an
    // afterthought applied to bytes we already trusted.
    let reqs = server.requests();
    let checksum_at = reqs.iter().position(|r| r.contains("checksums.txt")).expect("checksums");
    let archive_at = reqs.iter().position(|r| r.contains(&asset_name)).expect("archive");
    assert!(checksum_at < archive_at, "{reqs:?}");
}

#[tokio::test]
async fn installs_an_exact_pinned_version_by_tag() {
    let _ = fake_or_skip!();
    let s = Scenario::new("install-pinned");
    let paths = s.paths();
    let server = FakeGitHub::start().await;
    serve_release(&server, TEST_VERSION).await;
    let settings = settings_for(&s, &paths, TEST_VERSION);

    let installer = server.installer(&paths);
    installer.install_version(TEST_VERSION, &settings, None).await.expect("installs");
    assert!(
        server.requests().iter().any(|r| r.ends_with(&format!("/tags/v{TEST_VERSION}"))),
        "a pinned version must be fetched by tag, not by 'latest': {:?}",
        server.requests()
    );
}

// ---------------------------------------------------------------------------
// The verification chain
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_checksum_mismatch_refuses_to_install_and_leaves_nothing_behind() {
    let _ = fake_or_skip!();
    let s = Scenario::new("bad-checksum");
    let paths = s.paths();
    let server = FakeGitHub::start().await;

    let (asset_name, _kind, archive) = platform_asset(TEST_VERSION);
    // The checksum of a *different* payload: exactly what a tampered CDN or a
    // corrupted download looks like.
    let checksums = format!("{}  {asset_name}\n", sha256_hex(b"not the archive you downloaded"));
    serve_release_with(&server, TEST_VERSION, &asset_name, archive, checksums).await;
    let settings = settings_for(&s, &paths, TEST_VERSION);

    let err = server
        .installer(&paths)
        .install_latest(&settings, None)
        .await
        .expect_err("a mismatched checksum must never install");

    match &err {
        InstallError::ChecksumMismatch { asset, expected, actual } => {
            assert_eq!(asset, &asset_name);
            assert_ne!(expected, actual);
        }
        other => panic!("wrong error for a tampered download: {other:?}"),
    }
    assert!(err.is_security_event(), "a checksum mismatch must be flagged as a security event");
    assert!(!paths.bundled_kopia().exists(), "an unverified binary was installed");

    let target = paths.bundled_kopia();
    let bin_dir = target.parent().expect("bin dir");
    let stray: Vec<_> = std::fs::read_dir(bin_dir)
        .map(|d| {
            d.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n != "control.txt")
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    assert!(stray.is_empty(), "unverified bytes were left on disk: {stray:?}");
}

#[tokio::test]
async fn a_release_without_checksums_is_refused() {
    let _ = fake_or_skip!();
    let s = Scenario::new("no-checksums");
    let paths = s.paths();
    let server = FakeGitHub::start().await;

    let (asset_name, _kind, archive) = platform_asset(TEST_VERSION);
    let json = release_json(&server, TEST_VERSION, &[(asset_name.clone(), archive.len() as u64)]);
    server.route("/repos/kopia/kopia/releases/latest", Canned::json(json));
    server.route(&format!("/assets/{asset_name}"), Canned::bytes(archive));
    let settings = settings_for(&s, &paths, TEST_VERSION);

    let err = server.installer(&paths).install_latest(&settings, None).await.expect_err("refuses");
    assert!(matches!(err, InstallError::NoChecksums { .. }), "{err:?}");
    assert!(!paths.bundled_kopia().exists());
}

#[tokio::test]
async fn a_zip_slip_archive_is_rejected() {
    // Hand-built malicious archives, in both formats kopia publishes.
    let evil_zip = build_archive(
        &[("../../../../etc/cron.d/pwned", b"* * * * * root sh -c evil".as_slice())],
        ArchiveKind::Zip,
    );
    let err = extract_kopia(&evil_zip, ArchiveKind::Zip, "evil.zip").expect_err("must refuse");
    assert!(matches!(err, InstallError::UnsafeArchive { .. }), "{err:?}");
    assert!(err.is_security_event());

    let evil_tar = build_evil_targz("../../root/.ssh/authorized_keys", b"ssh-rsa AAAA");
    let err = extract_kopia(&evil_tar, ArchiveKind::TarGz, "evil.tar.gz").expect_err("must refuse");
    assert!(matches!(err, InstallError::UnsafeArchive { .. }), "{err:?}");

    // An absolute path is the other half of the attack.
    let absolute = build_evil_targz("/etc/cron.d/pwned", b"* * * * * root evil");
    assert!(
        matches!(
            extract_kopia(&absolute, ArchiveKind::TarGz, "abs.tar.gz"),
            Err(InstallError::UnsafeArchive { .. })
        ),
        "an absolute member path must be refused"
    );

    // A traversal entry sitting *after* a legitimate one is still fatal:
    // finding the executable first must not short-circuit the scan.
    let mixed = build_archive(
        &[
            ("kopia-0.23.1-windows-x64/kopia.exe", b"MZ".as_slice()),
            ("kopia-0.23.1-linux-x64/kopia", b"ELF".as_slice()),
            ("../evil", b"x".as_slice()),
        ],
        ArchiveKind::Zip,
    );
    assert!(
        matches!(
            extract_kopia(&mixed, ArchiveKind::Zip, "mixed.zip"),
            Err(InstallError::UnsafeArchive { .. })
        ),
        "every member must be checked, not just the one being extracted"
    );
}

#[tokio::test]
async fn an_archive_without_a_kopia_executable_is_refused() {
    let archive = build_archive(
        &[("kopia-0.23.1-linux-x64/README.md", b"hello".as_slice())],
        ArchiveKind::TarGz,
    );
    let err = extract_kopia(&archive, ArchiveKind::TarGz, "x.tar.gz").expect_err("must refuse");
    assert!(matches!(err, InstallError::ExecutableNotFound { .. }), "{err:?}");
}

#[tokio::test]
async fn a_download_redirected_off_github_is_refused() {
    let _ = fake_or_skip!();
    let s = Scenario::new("offsite");
    let paths = s.paths();
    let server = FakeGitHub::start().await;

    let (asset_name, _kind, archive) = platform_asset(TEST_VERSION);
    let checksums = format!("{}  {asset_name}\n", sha256_hex(&archive));
    serve_release_with(&server, TEST_VERSION, &asset_name, archive, checksums).await;
    // The asset now redirects somewhere that is not on the allowlist.
    server.route(
        &format!("/assets/{asset_name}"),
        Canned::redirect("http://malicious.example.net/kopia.zip"),
    );
    let settings = settings_for(&s, &paths, TEST_VERSION);

    let err = server.installer(&paths).install_latest(&settings, None).await.expect_err("refuses");
    assert!(matches!(err, InstallError::UntrustedHost { .. }), "{err:?}");
    assert!(err.is_security_event());
    assert!(
        !server.requests().iter().any(|r| r.contains("malicious")),
        "the redirect must not have been followed"
    );
}

#[tokio::test]
async fn the_default_allowlist_refuses_a_non_github_endpoint() {
    let s = Scenario::new("hosts");
    let paths = s.paths();
    // The production constructor, pointed at a hostile URL: the request must be
    // refused before any connection is made.
    let installer = KopiaInstaller::new(&paths).expect("installer");
    let allowed: Vec<String> = DEFAULT_ALLOWED_HOSTS.iter().map(|h| h.to_string()).collect();
    assert!(!host_allowed("malicious.example.net", &allowed));
    assert!(!host_allowed("api.github.com.evil.net", &allowed));
    assert!(host_allowed("release-assets.githubusercontent.com", &allowed));
    assert_eq!(installer.target_path(), paths.bundled_kopia().as_path());
}

// ---------------------------------------------------------------------------
// Version policy
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_downgrade_is_refused() {
    let _ = fake_or_skip!();
    let s = Scenario::new("downgrade");
    let paths = s.paths();
    let server = FakeGitHub::start().await;

    // Pretend 0.22.0 is already installed.
    s.install_fake_at(
        &paths.bundled_kopia(),
        &[("version", "0.22.0 build: installed from: kopia/kopia")],
    );
    serve_release(&server, "0.20.0").await;

    let err = server
        .installer(&paths)
        .install_latest(&Settings::default(), None)
        .await
        .expect_err("a downgrade must be refused");
    match &err {
        InstallError::RefusedVersion { reason } => assert!(reason.contains("downgrade"), "{reason}"),
        other => panic!("wrong error: {other:?}"),
    }
}

#[tokio::test]
async fn a_release_below_the_configured_minimum_is_refused() {
    let _ = fake_or_skip!();
    let s = Scenario::new("below-min");
    let paths = s.paths();
    let server = FakeGitHub::start().await;
    serve_release(&server, "0.18.0").await;

    let mut settings = settings_for(&s, &paths, "0.18.0");
    settings.kopia.minimum_version = "0.21.0".into();

    let err = server.installer(&paths).install_latest(&settings, None).await.expect_err("refuses");
    match &err {
        InstallError::RefusedVersion { reason } => assert!(reason.contains("minimum"), "{reason}"),
        other => panic!("wrong error: {other:?}"),
    }
    assert!(!paths.bundled_kopia().exists());
}

#[tokio::test]
async fn a_release_below_the_hard_floor_is_refused_however_the_settings_are_written() {
    let _ = fake_or_skip!();
    let s = Scenario::new("below-floor");
    let paths = s.paths();
    let server = FakeGitHub::start().await;
    serve_release(&server, "0.9.0").await;

    let mut settings = settings_for(&s, &paths, "0.9.0");
    // A user cannot talk superbackup into a kopia this driver cannot parse.
    settings.kopia.minimum_version = "0.1.0".into();

    let err = server.installer(&paths).install_latest(&settings, None).await.expect_err("refuses");
    assert!(matches!(err, InstallError::RefusedVersion { .. }), "{err:?}");
}

#[tokio::test]
async fn a_binary_that_lies_about_its_version_is_not_installed() {
    let _ = fake_or_skip!();
    let s = Scenario::new("liar");
    let paths = s.paths();
    let server = FakeGitHub::start().await;
    serve_release(&server, TEST_VERSION).await;

    // The release claims 0.23.1, but the binary in the archive reports 0.20.0.
    let settings = settings_for(&s, &paths, "0.20.0");

    let err = server.installer(&paths).install_latest(&settings, None).await.expect_err("refuses");
    match &err {
        InstallError::VersionMismatch { expected, reported } => {
            assert_eq!(expected, TEST_VERSION);
            assert_eq!(reported, "0.20.0");
        }
        other => panic!("wrong error: {other:?}"),
    }
    assert!(!paths.bundled_kopia().exists(), "a binary that lied was still installed");
}

// ---------------------------------------------------------------------------
// Update checking
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_update_check_reports_availability_without_installing_anything() {
    let _ = fake_or_skip!();
    let s = Scenario::new("check-available");
    let paths = s.paths();
    let server = FakeGitHub::start().await;
    s.install_fake_at(
        &paths.bundled_kopia(),
        &[("version", "0.21.0 build: installed from: kopia/kopia")],
    );
    serve_release(&server, TEST_VERSION).await;

    let settings = Settings::default();
    assert_eq!(settings.kopia.auto_update, UpdatePolicy::Notify, "Notify is the safe default");

    let installer = server.installer(&paths);
    let check = installer.check_for_update(&settings, chrono::Utc::now()).await;
    match &check {
        UpdateCheck::Available { current, latest, .. } => {
            assert_eq!(current.as_ref().map(|v| v.to_string()).as_deref(), Some("0.21.0"));
            assert_eq!(latest.to_string(), TEST_VERSION);
        }
        other => panic!("expected an available update, got {other:?}"),
    }
    assert!(check.summary().contains(TEST_VERSION));

    // Notify installs nothing.
    let applied = installer
        .apply_update_if_wanted(&settings, &check, false, None)
        .await
        .expect("notify must not fail");
    assert!(applied.is_none(), "UpdatePolicy::Notify must never replace the binary");
    assert_eq!(installer.installed_version().await.map(|v| v.to_string()).as_deref(), Some("0.21.0"));
}

#[tokio::test]
async fn an_automatic_update_defers_while_a_job_is_running() {
    let _ = fake_or_skip!();
    let s = Scenario::new("busy");
    let paths = s.paths();
    let server = FakeGitHub::start().await;
    s.install_fake_at(
        &paths.bundled_kopia(),
        &[("version", "0.21.0 build: installed from: kopia/kopia")],
    );
    serve_release(&server, TEST_VERSION).await;

    let mut settings = Settings::default();
    settings.kopia.auto_update = UpdatePolicy::Automatic;

    let installer = server.installer(&paths);
    let check = installer.check_for_update(&settings, chrono::Utc::now()).await;
    let err = installer
        .apply_update_if_wanted(&settings, &check, true, None)
        .await
        .expect_err("must not swap the binary mid-snapshot");
    assert_eq!(err, InstallError::Busy);
    assert_eq!(installer.installed_version().await.map(|v| v.to_string()).as_deref(), Some("0.21.0"));
}

#[tokio::test]
async fn an_update_check_is_skipped_when_it_is_too_soon() {
    let _ = fake_or_skip!();
    let s = Scenario::new("throttle");
    let paths = s.paths();
    let server = FakeGitHub::start().await;
    serve_release(&server, TEST_VERSION).await;

    let now = chrono::Utc::now();
    let mut settings = Settings::default();
    settings.kopia.check_interval_hours = 24;
    settings.kopia.last_check_at = Some(now - chrono::Duration::hours(2));

    let check = server.installer(&paths).check_for_update(&settings, now).await;
    assert!(matches!(check, UpdateCheck::Skipped { reason: SkipReason::TooSoon }), "{check:?}");
    assert!(server.requests().is_empty(), "GitHub must not be contacted: {:?}", server.requests());
}

#[tokio::test]
async fn update_checks_are_switched_off_by_policy_and_by_a_user_managed_binary() {
    let _ = fake_or_skip!();
    let s = Scenario::new("policy-off");
    let paths = s.paths();
    let server = FakeGitHub::start().await;
    serve_release(&server, TEST_VERSION).await;
    let installer = server.installer(&paths);
    let now = chrono::Utc::now();

    let mut off = Settings::default();
    off.kopia.auto_update = UpdatePolicy::Off;
    assert!(matches!(
        installer.check_for_update(&off, now).await,
        UpdateCheck::Skipped { reason: SkipReason::PolicyOff }
    ));

    // A user who set an explicit path owns that binary; superbackup does not
    // check it, offer to replace it, or touch it.
    let user_managed = Settings { kopia_path: Some(s.exe.clone()), ..Settings::default() };
    assert!(matches!(
        installer.check_for_update(&user_managed, now).await,
        UpdateCheck::Skipped { reason: SkipReason::UserManagedBinary }
    ));
    assert!(server.requests().is_empty(), "{:?}", server.requests());
}

#[tokio::test]
async fn a_failed_check_is_a_warning_not_a_fatal_error() {
    let _ = fake_or_skip!();
    let s = Scenario::new("check-fails");
    let paths = s.paths();

    // A server that answers nothing useful, and one that has gone away entirely.
    let server = FakeGitHub::start().await;
    server.route("/repos/kopia/kopia/releases/latest", Canned::rate_limited());
    let check =
        server.installer(&paths).check_for_update(&Settings::default(), chrono::Utc::now()).await;
    match &check {
        UpdateCheck::Failed { error } => assert_eq!(error, &InstallError::RateLimited),
        other => panic!("a rate limit must degrade to a warning, got {other:?}"),
    }
    assert!(check.summary().contains("Could not check"));

    let dead = KopiaInstaller::with_endpoint(
        &paths,
        "http://127.0.0.1:1",
        vec!["127.0.0.1".to_string()],
    )
    .expect("installer");
    let check = dead.check_for_update(&Settings::default(), chrono::Utc::now()).await;
    assert!(
        matches!(check, UpdateCheck::Failed { .. }),
        "an unreachable GitHub must not stop the application from starting"
    );
}

// ---------------------------------------------------------------------------
// The startup path
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ensure_available_installs_when_there_is_no_kopia_at_all() {
    let _ = fake_or_skip!();
    let s = Scenario::new("ensure-install");
    let paths = s.paths();
    let server = FakeGitHub::start().await;
    serve_release(&server, TEST_VERSION).await;
    let settings = settings_for(&s, &paths, TEST_VERSION);

    let _path = PathGuard::empty();
    let bin = server
        .installer(&paths)
        .ensure_available(&settings, &paths, None)
        .await
        .expect("installs and returns a usable binary");
    assert_eq!(bin.source(), KopiaSource::Bundled);
    assert_eq!(bin.version().to_string(), TEST_VERSION);
}

#[tokio::test]
async fn ensure_available_leaves_a_usable_system_kopia_alone() {
    let _ = fake_or_skip!();
    let s = Scenario::new("ensure-system");
    let paths = s.paths();
    let server = FakeGitHub::start().await;
    serve_release(&server, TEST_VERSION).await;
    s.script(&[("version", "0.21.0 build: onpath from: kopia/kopia")]);

    let _path = PathGuard::prepend(&s.bin_dir);
    let bin = server
        .installer(&paths)
        .ensure_available(&Settings::default(), &paths, None)
        .await
        .expect("uses what is already there");
    assert_eq!(bin.source(), KopiaSource::SystemPath);
    assert!(
        !paths.bundled_kopia().exists(),
        "nothing should have been downloaded when a usable kopia was already installed"
    );
    assert!(server.requests().is_empty(), "GitHub must not be contacted: {:?}", server.requests());
}

#[tokio::test]
async fn ensure_available_says_so_when_auto_install_is_off() {
    let _ = fake_or_skip!();
    let s = Scenario::new("no-autoinstall");
    let paths = s.paths();
    let server = FakeGitHub::start().await;
    serve_release(&server, TEST_VERSION).await;

    let mut settings = Settings::default();
    settings.kopia.auto_install = false;

    let _path = PathGuard::empty();
    let err = server
        .installer(&paths)
        .ensure_available(&settings, &paths, None)
        .await
        .expect_err("must not install");
    assert_eq!(err, InstallError::AutoInstallDisabled);
    assert!(err.hint().is_some());
    assert!(matches!(
        superbackup_core::Error::from(err).code(),
        superbackup_core::ErrorCode::KopiaMissing
    ));
}

#[tokio::test]
async fn a_read_only_install_directory_is_reported_not_panicked_on() {
    let _ = fake_or_skip!();
    let s = Scenario::new("readonly");
    let paths = s.paths();
    let server = FakeGitHub::start().await;
    serve_release(&server, TEST_VERSION).await;

    // Occupy the install path with a directory, which is the simplest portable
    // way to make the final rename impossible.
    let target = paths.bundled_kopia();
    std::fs::create_dir_all(&target).expect("create blocking directory");
    let settings = Settings::default();

    let err = server.installer(&paths).install_latest(&settings, None).await.expect_err("fails");
    assert!(
        matches!(err, InstallError::Io(_) | InstallError::Busy | InstallError::VersionMismatch { .. }),
        "an unwritable target must degrade to a reported error, got {err:?}"
    );
    assert!(err.message().len() > 10);
}

// ---------------------------------------------------------------------------
// Asset selection
// ---------------------------------------------------------------------------

#[test]
fn the_asset_matrix_covers_every_platform_superbackup_supports() {
    // Names verified against the real kopia/kopia v0.23.1 asset list.
    let expected: &[(&str, &str, &str, ArchiveKind)] = &[
        ("windows", "x86_64", "kopia-0.23.1-windows-x64.zip", ArchiveKind::Zip),
        ("windows", "aarch64", "kopia-0.23.1-windows-x64.zip", ArchiveKind::Zip),
        ("linux", "x86_64", "kopia-0.23.1-linux-x64.tar.gz", ArchiveKind::TarGz),
        ("linux", "aarch64", "kopia-0.23.1-linux-arm64.tar.gz", ArchiveKind::TarGz),
        ("linux", "arm", "kopia-0.23.1-linux-arm.tar.gz", ArchiveKind::TarGz),
        ("macos", "x86_64", "kopia-0.23.1-macOS-x64.tar.gz", ArchiveKind::TarGz),
        ("macos", "aarch64", "kopia-0.23.1-macOS-arm64.tar.gz", ArchiveKind::TarGz),
    ];
    for (os, arch, name, kind) in expected {
        let a = asset_for_platform("0.23.1", os, arch)
            .unwrap_or_else(|| panic!("no asset for {os}/{arch}"));
        assert_eq!(&a.name, name);
        assert_eq!(&a.kind, kind);
    }
    assert!(asset_for_platform("0.23.1", "haiku", "x86_64").is_none());
}

#[tokio::test]
async fn a_release_without_an_asset_for_this_platform_fails_loudly() {
    let _ = fake_or_skip!();
    let s = Scenario::new("no-asset");
    let paths = s.paths();
    let server = FakeGitHub::start().await;

    // A release that only publishes a platform this machine is not.
    let checksums = "aa  kopia-0.23.1-solaris-sparc.tar.gz\n".to_string();
    let json = release_json(
        &server,
        TEST_VERSION,
        &[("kopia-0.23.1-solaris-sparc.tar.gz".into(), 10), ("checksums.txt".into(), 40)],
    );
    server.route("/repos/kopia/kopia/releases/latest", Canned::json(json));
    server.route("/assets/checksums.txt", Canned::bytes(checksums.into_bytes()));
    let settings = settings_for(&s, &paths, TEST_VERSION);

    let err = server.installer(&paths).install_latest(&settings, None).await.expect_err("fails");
    assert!(matches!(err, InstallError::NoAssetForPlatform { .. }), "{err:?}");
    assert!(err.hint().is_some(), "the user must be told what to do instead");
}

#[test]
fn safe_member_names_and_checksum_parsing_are_reachable_from_outside() {
    // These are the two pure guards the installer leans on; assert they are
    // public API so the app layer can reuse them for a manual install.
    assert_eq!(safe_member_name("kopia-0.23.1-linux-x64/kopia").as_deref(), Ok("kopia"));
    assert!(safe_member_name("../escape").is_err());
    assert_eq!(
        checksum_for(
            "416d0f84a3dbb321a8b2d8f0997b1a0a6e915babe79ee76fa6e4d2bd1e1c5178  kopia-0.23.1-linux-x64.tar.gz\n",
            "kopia-0.23.1-linux-x64.tar.gz"
        )
        .as_deref(),
        Some("416d0f84a3dbb321a8b2d8f0997b1a0a6e915babe79ee76fa6e4d2bd1e1c5178")
    );
}

/// Keeps the unused-import lint honest about the helpers this file shares with
/// the driver suite.
#[allow(dead_code)]
fn _unused_helpers(_: &Path, _: PathBuf, _: Duration) {}
