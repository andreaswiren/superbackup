//! The per-destination driver: one [`KopiaDriver`] owns one kopia repository.
//!
//! Everything a repository operation needs — which binary, which config file,
//! which cache directory, which credentials — is bound once here, so no call
//! site can accidentally run a command against the wrong repository or against
//! the user's own hand-rolled kopia configuration.
//!
//! ## Isolation
//!
//! A stock `kopia` run by the user reads `~/.config/kopia/repository.config`
//! and caches under `~/.cache/kopia`. Superbackup never goes near either: every
//! invocation carries `--config-file <data>/kopia/<destination-id>.config` and
//! its own `--cache-directory`. Two destinations therefore cannot fight over a
//! single "currently connected repository", which is the single biggest
//! footgun in kopia's CLI design for a multi-destination product.

use super::binary::KopiaBinary;
use super::command::{CommandOutput, KopiaCommand, RunContext};
use super::error::{KopiaError, KopiaFailure};
use crate::model::{
    Destination, DestinationKind, EncryptionSettings, ProviderKind, StorageProvider,
};
use crate::paths::Paths;
use crate::secret::Secret;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Shorthand for the driver's fallible operations.
pub type KopiaResult<T> = std::result::Result<T, KopiaError>;

/// Every secret one destination might need, already resolved against the
/// unlocked vault.
///
/// The driver never touches the vault itself: resolving a [`crate::model::SecretRef`]
/// is the engine's job, and keeping that boundary means the kopia layer can be
/// unit-tested without a crypto stack.
#[derive(Debug, Default, Clone)]
pub struct DestinationSecrets {
    /// The repository passphrase. Required for every repository operation.
    pub passphrase: Option<Secret>,
    pub access_key: Option<Secret>,
    pub secret_key: Option<Secret>,
    pub session_token: Option<Secret>,
}

impl DestinationSecrets {
    pub fn with_passphrase(passphrase: Secret) -> Self {
        DestinationSecrets { passphrase: Some(passphrase), ..Default::default() }
    }
    pub fn with_s3(mut self, access_key: Secret, secret_key: Secret) -> Self {
        self.access_key = Some(access_key);
        self.secret_key = Some(secret_key);
        self
    }
}

/// Options for a destination that kopia's CLI has no flag for.
///
/// Surfaced rather than silently ignored so the GUI can tell the user their
/// setting will not take effect, instead of leaving them to discover it when
/// the backup fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedOption {
    pub setting: &'static str,
    pub reason: &'static str,
}

/// One kopia repository, bound to one superbackup destination.
#[derive(Debug, Clone)]
pub struct KopiaDriver {
    binary: KopiaBinary,
    destination_id: Uuid,
    destination_name: String,
    kind: DestinationKind,
    provider: Option<StorageProvider>,
    encryption: EncryptionSettings,
    secrets: DestinationSecrets,
    config_file: PathBuf,
    cache_dir: PathBuf,
    log_dir: PathBuf,
}

impl KopiaDriver {
    /// Bind a driver to a destination.
    ///
    /// Fails fast on configurations kopia could never satisfy — an S3
    /// destination with no provider, a mirror destination that owns no
    /// repository — rather than producing a confusing kopia error later.
    pub fn new(
        binary: KopiaBinary,
        paths: &Paths,
        destination: &Destination,
        provider: Option<&StorageProvider>,
        secrets: DestinationSecrets,
    ) -> KopiaResult<KopiaDriver> {
        let label = format!("destination {}", destination.name);
        if !destination.kind.is_repository() {
            return Err(KopiaError::local(label, KopiaFailure::Unusable, None).with_message(
                format!(
                    "\"{}\" is a folder mirror, which does not use a kopia repository.",
                    destination.name
                ),
            ));
        }
        if matches!(destination.kind, DestinationKind::S3 { .. }) && provider.is_none() {
            return Err(KopiaError::local(label, KopiaFailure::Unusable, None)
                .with_message(format!(
                "\"{}\" points at a storage provider that no longer exists in the configuration.",
                destination.name
            )));
        }

        Ok(KopiaDriver {
            binary,
            destination_id: destination.id,
            destination_name: destination.name.clone(),
            kind: destination.kind.clone(),
            provider: provider.cloned(),
            encryption: destination.encryption.clone().unwrap_or_default(),
            secrets,
            config_file: paths.kopia_config_for(&destination.id),
            // Per destination, because kopia's cache is keyed to one
            // repository's content ids and sharing it across repositories is
            // not something kopia supports.
            cache_dir: paths.kopia_cache_dir().join(destination.id.to_string()),
            log_dir: paths.log_dir.join("kopia"),
        })
    }

    pub fn binary(&self) -> &KopiaBinary {
        &self.binary
    }
    pub fn destination_id(&self) -> &Uuid {
        &self.destination_id
    }
    pub fn destination_name(&self) -> &str {
        &self.destination_name
    }
    pub fn config_file(&self) -> &Path {
        &self.config_file
    }
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// Settings this destination carries that kopia's CLI cannot express.
    ///
    /// Currently one: **S3 path-style addressing**. `cli/storage_s3.go` exposes
    /// `--bucket`, `--endpoint`, `--region`, `--prefix`, `--disable-tls`,
    /// `--disable-tls-verification`, `--session-token` and the root-CA flags,
    /// and nothing else — there is no path-style toggle. Kopia's S3 backend is
    /// minio-go, which selects path-style automatically for endpoints that
    /// require it, so a MinIO destination still works; the checkbox simply has
    /// no effect and the user deserves to be told.
    pub fn unsupported_options(&self) -> Vec<UnsupportedOption> {
        let mut out = Vec::new();
        if let Some(StorageProvider { kind: ProviderKind::S3 { path_style: true, .. }, .. }) =
            &self.provider
        {
            out.push(UnsupportedOption {
                setting: "Force path-style addressing",
                reason: "kopia's S3 backend chooses path-style automatically and exposes no flag \
                         for it; this setting is ignored.",
            });
        }
        if !self.binary.supports_ecc() && self.encryption.ecc.is_some() {
            out.push(UnsupportedOption {
                setting: "Error-correction coding",
                reason: "this kopia build predates --ecc; the repository will be created without \
                         error correction.",
            });
        }
        out
    }

    // -----------------------------------------------------------------
    // Command construction
    // -----------------------------------------------------------------

    /// A command pinned to this destination's config, cache and credentials.
    ///
    /// Every driver method starts here, which is what makes the isolation and
    /// secret-handling guarantees structural rather than a convention.
    pub(super) fn base(&self) -> KopiaCommand {
        let mut cmd = KopiaCommand::new(self.binary.path());
        cmd.global("config-file", &self.config_file)
            .global("log-dir", &self.log_dir)
            // Console output is parsed, so keep it to real problems. Progress
            // is a separate channel and is unaffected by the log level.
            .global("log-level", "warning")
            // Kopia's own on-disk log is the post-mortem record when a backup
            // fails at 3am; info is the useful level for that without the
            // volume of debug.
            .global("file-log-level", "info")
            // The vault is the only place a repository passphrase is allowed to
            // live. Kopia would otherwise stash it in the OS keychain or beside
            // its config on connect, creating a second copy we do not control.
            .global_bool("persist-credentials", false);

        // Deterministic, machine-friendly output.
        cmd.env("KOPIA_CHECK_FOR_UPDATES", "false")
            .env("KOPIA_DISABLE_COLOR", "true")
            // Pins `units.BytesString` to base 10 so the progress parser cannot
            // be thrown off by a factor of 1.024 by an inherited variable.
            .env("KOPIA_BYTES_STRING_BASE_2", "false");

        if let Some(p) = &self.secrets.passphrase {
            cmd.secret_env("KOPIA_PASSWORD", p);
        }
        self.credential_env(&mut cmd);
        cmd
    }

    /// Object-store credentials, always through the environment.
    ///
    /// Kopia binds `--access-key`, `--secret-access-key` and `--session-token`
    /// to exactly these variables (`cli/storage_s3.go`), and kingpin treats an
    /// environment-supplied value as satisfying the flag's `Required()`, so the
    /// flags are never passed at all.
    fn credential_env(&self, cmd: &mut KopiaCommand) {
        if let Some(k) = &self.secrets.access_key {
            cmd.secret_env("AWS_ACCESS_KEY_ID", k);
        }
        if let Some(k) = &self.secrets.secret_key {
            cmd.secret_env("AWS_SECRET_ACCESS_KEY", k);
        }
        if let Some(k) = &self.secrets.session_token {
            cmd.secret_env("AWS_SESSION_TOKEN", k);
        }
    }

    /// Append the storage subcommand and its flags for `repository create` and
    /// `repository connect`, which share an identical storage flag set.
    fn storage_args(&self, cmd: &mut KopiaCommand) -> KopiaResult<()> {
        match &self.kind {
            DestinationKind::LocalRepository { path } | DestinationKind::OneDrive { path, .. } => {
                cmd.command("filesystem").flag("path", path);
            }
            DestinationKind::S3 { bucket, prefix, .. } => {
                let Some(StorageProvider {
                    kind: ProviderKind::S3 { endpoint, region, tls, .. },
                    ..
                }) = &self.provider
                else {
                    return Err(KopiaError::local(
                        "repository",
                        KopiaFailure::Unusable,
                        Some("the destination's storage provider is missing".into()),
                    ));
                };
                let (host, scheme_disables_tls) = s3_endpoint_host(endpoint);
                if host.is_empty() {
                    return Err(KopiaError::local("repository", KopiaFailure::Unusable, None)
                        .with_message(
                            "The storage provider has no endpoint. StorJ's is \
                         https://gateway.storjshare.io.",
                        ));
                }
                cmd.command("s3").flag("bucket", bucket).flag("endpoint", &host);
                if !region.is_empty() {
                    cmd.flag("region", region);
                }
                if !prefix.is_empty() {
                    cmd.flag("prefix", prefix);
                }
                // `--disable-tls` means plain HTTP. Only ever set when the user
                // asked for it or wrote an http:// endpoint: silently
                // downgrading a backup to cleartext would be indefensible.
                if !*tls || scheme_disables_tls {
                    cmd.switch("disable-tls");
                }
            }
            DestinationKind::LocalMirror { .. } => {
                return Err(KopiaError::local("repository", KopiaFailure::Unusable, None)
                    .with_message("A folder mirror has no kopia repository."));
            }
        }
        Ok(())
    }

    /// Cache and client flags shared by `repository create` and
    /// `repository connect`. Both accept the identical set — kopia defines them
    /// once in `connectOptions` (`cli/command_repository_connect.go`) and mixes
    /// them into both commands.
    fn connect_options(&self, cmd: &mut KopiaCommand) {
        cmd.flag("cache-directory", &self.cache_dir)
            .flag_bool("check-for-updates", false)
            .flag("description", format!("superbackup: {}", self.destination_name));
    }

    // -----------------------------------------------------------------
    // Repository lifecycle
    // -----------------------------------------------------------------

    /// Create the repository, applying the destination's encryption settings.
    ///
    /// Flags verified against `cli/command_repository_create.go`:
    /// `--block-hash`, `--encryption`, `--ecc`, `--ecc-overhead-percent`,
    /// `--object-splitter`, `--create-only`, `--format-version`.
    ///
    /// Note that kopia enables error correction only when the overhead is
    /// greater than zero — `--ecc` alone is inert — so both are set together or
    /// neither is set at all.
    pub async fn create_repository(&self, ctx: &RunContext) -> KopiaResult<()> {
        self.require_passphrase("repository create")?;
        let mut cmd = self.base();
        cmd.command("repository").command("create");
        cmd.flag("encryption", self.encryption.algorithm.kopia_id())
            .flag("block-hash", self.encryption.hash.kopia_id())
            .flag("object-splitter", self.encryption.splitter.kopia_id());

        match (&self.encryption.ecc, self.encryption.ecc_overhead_percent) {
            (Some(ecc), overhead) if overhead > 0 && self.binary.supports_ecc() => {
                cmd.flag("ecc", ecc.kopia_id()).flag("ecc-overhead-percent", overhead.to_string());
            }
            _ => {}
        }

        self.connect_options(&mut cmd);
        self.storage_args(&mut cmd)?;
        self.prepare_local_directories()?;
        cmd.run(ctx).await?;
        Ok(())
    }

    /// Connect this destination's config file to an existing repository.
    ///
    /// Idempotent from the caller's point of view: connecting an
    /// already-connected destination simply rewrites the config file.
    pub async fn connect_repository(&self, ctx: &RunContext) -> KopiaResult<()> {
        self.require_passphrase("repository connect")?;
        let mut cmd = self.base();
        cmd.command("repository").command("connect");
        self.connect_options(&mut cmd);
        self.storage_args(&mut cmd)?;
        self.prepare_local_directories()?;
        cmd.run(ctx).await?;
        Ok(())
    }

    /// Disconnect, leaving the repository itself untouched.
    pub async fn disconnect_repository(&self, ctx: &RunContext) -> KopiaResult<()> {
        let mut cmd = self.base();
        cmd.command("repository").command("disconnect");
        cmd.run(ctx).await?;
        Ok(())
    }

    /// Full repository status, as `repository status --json`.
    pub async fn repository_status(&self, ctx: &RunContext) -> KopiaResult<RepositoryStatus> {
        let mut cmd = self.base();
        cmd.command("repository").command("status").switch("json");
        let out = cmd.run(ctx).await?;
        RepositoryStatus::parse(&out.stdout).ok_or_else(|| {
            KopiaError::local(
                "repository status",
                KopiaFailure::Unknown,
                Some("kopia's repository status output could not be understood".into()),
            )
        })
    }

    /// Cheap "is this destination usable right now?" check.
    ///
    /// Returns `false` rather than an error for the ordinary not-connected and
    /// no-repository-here cases, because the GUI asks this on every refresh and
    /// an exception per idle destination is not a design.
    pub async fn is_connected(&self, ctx: &RunContext) -> bool {
        if !self.config_file.is_file() {
            return false;
        }
        self.repository_status(ctx).await.is_ok()
    }

    /// The "Test connection" button.
    ///
    /// Connecting is the honest test: it exercises DNS, TLS, the endpoint, the
    /// credentials, the bucket and the passphrase in one shot, and writes only
    /// to superbackup's own config file. A storage location that is reachable
    /// but holds no repository yet is reported as success — that is exactly the
    /// state a user is in just before they press "Create".
    pub async fn test_connection(&self, ctx: &RunContext) -> KopiaResult<ConnectionTest> {
        match self.connect_repository(ctx).await {
            Ok(()) => {
                let status = self.repository_status(ctx).await.ok();
                Ok(ConnectionTest::Connected { status: status.map(Box::new) })
            }
            Err(e) if e.failure == KopiaFailure::RepositoryNotFound => {
                Ok(ConnectionTest::ReachableButEmpty)
            }
            Err(e) => Err(e),
        }
    }

    /// Kopia's own provider-compatibility suite.
    ///
    /// `repository validate-provider` writes and re-reads blobs to prove the
    /// storage backend has the consistency guarantees kopia relies on. It
    /// requires an already-connected repository (it is registered as a
    /// `directRepositoryWriteAction`), so it is a deeper, slower follow-up to
    /// [`KopiaDriver::test_connection`] rather than a replacement for it — the
    /// right thing to run once, when a user adds an unusual S3 gateway.
    pub async fn validate_provider(&self, ctx: &RunContext) -> KopiaResult<String> {
        let mut cmd = self.base();
        cmd.command("repository").command("validate-provider");
        let out = cmd.run(ctx).await?;
        Ok(out.redacted_stdout())
    }

    /// Rotate the repository passphrase.
    ///
    /// The new passphrase travels in `KOPIA_NEW_PASSWORD`, which is what
    /// `cli/command_repository_change_password.go` binds `--new-password` to;
    /// the flag itself is never used, for the same reason `--password` is not.
    ///
    /// The caller must write the new passphrase to the vault **only after** this
    /// returns `Ok`: a vault holding a passphrase the repository does not
    /// accept is unrecoverable.
    pub async fn change_password(
        &self,
        new_passphrase: &Secret,
        ctx: &RunContext,
    ) -> KopiaResult<()> {
        self.require_passphrase("repository change-password")?;
        if new_passphrase.is_empty() {
            return Err(KopiaError::local(
                "repository change-password",
                KopiaFailure::Unusable,
                None,
            )
            .with_message("A repository passphrase cannot be empty."));
        }
        let mut cmd = self.base();
        cmd.command("repository")
            .command("change-password")
            .secret_env("KOPIA_NEW_PASSWORD", new_passphrase);
        cmd.run(ctx).await?;
        Ok(())
    }

    /// Set the repository's bandwidth ceilings.
    ///
    /// Verified against `cli/throttle_set.go`: kopia expresses limits as
    /// **bytes per second** via `--upload-bytes-per-second` and
    /// `--download-bytes-per-second`, and accepts the literal `unlimited` to
    /// clear one. There is no kilobit or percentage form, so
    /// [`crate::model::BandwidthSettings`], which is in kilobytes per second,
    /// is multiplied by 1024 here.
    ///
    /// The limits are stored **in the repository**, not in the client config,
    /// so they apply to every machine that connects to it. A per-job or
    /// time-of-day ceiling therefore has to be applied by re-running this
    /// before the job starts; kopia has no per-invocation throttle flag.
    pub async fn set_throttle(
        &self,
        upload_kbps: Option<u32>,
        download_kbps: Option<u32>,
        ctx: &RunContext,
    ) -> KopiaResult<()> {
        if !self.binary.supports_throttling() {
            return Err(KopiaError::local("repository throttle set", KopiaFailure::Unusable, None)
                .with_message(format!(
                    "kopia {} does not support bandwidth limits; 0.10 or newer is required.",
                    self.binary.version()
                )));
        }
        let mut cmd = self.base();
        cmd.command("repository").command("throttle").command("set");
        cmd.flag("upload-bytes-per-second", throttle_value(upload_kbps));
        cmd.flag("download-bytes-per-second", throttle_value(download_kbps));
        cmd.run(ctx).await?;
        Ok(())
    }

    /// Total size of the destination, from `blob stats --raw`.
    ///
    /// `blob stats` has no `--json`; it prints `Count:`, `Total:`, `Average:`
    /// and a histogram (`cli/command_blob_stats.go`). `--raw` switches the
    /// sizes from `units.BytesString` to plain integers, which is what makes
    /// this parseable exactly rather than to one decimal place.
    ///
    /// This walks the whole blob list at the provider, so it is a "refresh
    /// size" action, not something to run on every dashboard paint.
    pub async fn blob_stats(&self, ctx: &RunContext) -> KopiaResult<BlobStats> {
        let mut cmd = self.base();
        cmd.command("blob").command("stats").switch("raw");
        let out = cmd.run(ctx).await?;
        BlobStats::parse(&out.stdout).ok_or_else(|| {
            KopiaError::local(
                "blob stats",
                KopiaFailure::Unknown,
                Some("could not read blob statistics from kopia's output".into()),
            )
        })
    }

    /// Logical (pre-deduplication) content statistics, from `content stats --raw`.
    ///
    /// Pairs with [`KopiaDriver::blob_stats`]: blobs are what the destination
    /// actually costs, contents are what it holds. The difference is the dedup
    /// and compression win the GUI advertises.
    pub async fn content_stats(&self, ctx: &RunContext) -> KopiaResult<ContentStats> {
        let mut cmd = self.base();
        cmd.command("content").command("stats").switch("raw");
        let out = cmd.run(ctx).await?;
        Ok(ContentStats::parse(&out.stdout))
    }

    // -----------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------

    pub(super) fn require_passphrase(&self, label: &str) -> KopiaResult<()> {
        match &self.secrets.passphrase {
            Some(p) if !p.is_empty() => Ok(()),
            _ => Err(KopiaError::local(label, KopiaFailure::WrongPassword, None).with_message(
                format!(
                    "No passphrase is available for \"{}\". Unlock the vault and try again.",
                    self.destination_name
                ),
            )),
        }
    }

    /// Create the directories kopia will need before it tries to.
    ///
    /// Kopia creates its own cache directory, but not the parent of a config
    /// file it was handed, and a repository on a not-yet-created folder fails
    /// with a bare "no such file or directory" that says nothing useful.
    fn prepare_local_directories(&self) -> KopiaResult<()> {
        for dir in [self.config_file.parent(), Some(self.cache_dir.as_path()), Some(&self.log_dir)]
            .into_iter()
            .flatten()
        {
            if let Err(e) = std::fs::create_dir_all(dir) {
                return Err(KopiaError::local(
                    "repository",
                    KopiaFailure::PermissionDenied,
                    Some(format!("could not create {}: {e}", dir.display())),
                ));
            }
        }
        if let DestinationKind::LocalRepository { path } | DestinationKind::OneDrive { path, .. } =
            &self.kind
        {
            if let Err(e) = std::fs::create_dir_all(path) {
                return Err(KopiaError::local(
                    "repository",
                    KopiaFailure::PermissionDenied,
                    Some(format!("could not create {}: {e}", path.display())),
                ));
            }
        }
        Ok(())
    }

    /// Run a prepared command. Used by the sibling modules.
    pub(super) async fn run(
        &self,
        cmd: KopiaCommand,
        ctx: &RunContext,
    ) -> KopiaResult<CommandOutput> {
        cmd.run(ctx).await
    }
}

/// Kopia's `--upload-bytes-per-second` value: a byte count, or the literal
/// `unlimited` it accepts to clear the limit.
fn throttle_value(kbps: Option<u32>) -> String {
    match kbps {
        Some(k) if k > 0 => (u64::from(k) * 1024).to_string(),
        _ => "unlimited".to_string(),
    }
}

/// Reduce a user-typed endpoint to the bare `host[:port]` kopia wants.
///
/// `cli/storage_s3.go` defaults `--endpoint` to `s3.amazonaws.com` — no scheme,
/// because the value goes straight into minio-go's `Endpoint`, which rejects
/// one. HTTP vs HTTPS is selected by `--disable-tls`, not by the URL. So
/// `https://gateway.storjshare.io` must be passed as `gateway.storjshare.io`,
/// and an `http://` endpoint additionally implies `--disable-tls`.
///
/// Returns `(host, scheme_requires_plain_http)`.
pub fn s3_endpoint_host(endpoint: &str) -> (String, bool) {
    let trimmed = endpoint.trim();
    let (rest, plain_http) = match trimmed.split_once("://") {
        Some((scheme, rest)) => (rest, scheme.eq_ignore_ascii_case("http")),
        None => (trimmed, false),
    };
    // Drop any path, query or fragment: kopia's prefix, not the URL, selects
    // the location inside the bucket.
    let host =
        rest.split(['/', '?', '#']).next().unwrap_or("").trim().trim_end_matches('.').to_string();
    (host, plain_http)
}

/// The outcome of "Test connection".
#[derive(Debug, Clone)]
pub enum ConnectionTest {
    /// Storage reachable, credentials accepted, repository opened.
    Connected { status: Option<Box<RepositoryStatus>> },
    /// Storage reachable and credentials accepted, but there is no repository
    /// here yet. The user's next step is "Create repository".
    ReachableButEmpty,
}

impl ConnectionTest {
    /// True when there is a repository here to back up into. `false` means the
    /// location is fine but empty, which is the cue for the "Create
    /// repository" button rather than for an error.
    pub fn has_repository(&self) -> bool {
        matches!(self, ConnectionTest::Connected { .. })
    }

    /// One line for the button's result label.
    pub fn summary(&self) -> String {
        match self {
            ConnectionTest::Connected { status: Some(s) } => format!(
                "Connected. {} repository, {} encryption.",
                s.storage_type.as_deref().unwrap_or("unknown"),
                s.encryption.as_deref().unwrap_or("unknown")
            ),
            ConnectionTest::Connected { status: None } => "Connected.".to_string(),
            ConnectionTest::ReachableButEmpty => {
                "Reached the storage location, but there is no backup repository there yet."
                    .to_string()
            }
        }
    }
}

/// The parts of `repository status --json` superbackup cares about.
///
/// Field names verified against `cli.RepositoryStatus` in
/// `cli/command_repository_status.go`, `format.ContentFormat` and
/// `blob.Capacity`. Everything is optional and the untouched document is kept
/// in [`RepositoryStatus::raw`], so a shape change costs a `None` rather than a
/// failed status check.
#[derive(Debug, Clone, Default)]
pub struct RepositoryStatus {
    pub config_file: Option<String>,
    pub unique_id: Option<String>,
    /// `filesystem`, `s3`, … from `blob.ConnectionInfo`.
    pub storage_type: Option<String>,
    pub hash: Option<String>,
    pub encryption: Option<String>,
    pub ecc: Option<String>,
    pub ecc_overhead_percent: Option<u64>,
    pub splitter: Option<String>,
    pub format_version: Option<u64>,
    pub hostname: Option<String>,
    pub username: Option<String>,
    pub read_only: bool,
    /// Volume capacity, for local and network destinations that report one.
    pub volume_total_bytes: Option<u64>,
    pub volume_free_bytes: Option<u64>,
    pub raw: serde_json::Value,
}

impl RepositoryStatus {
    pub fn parse(stdout: &str) -> Option<RepositoryStatus> {
        let v: serde_json::Value = serde_json::from_str(stdout.trim()).ok()?;
        let s = |path: &[&str]| -> Option<String> {
            let mut cur = &v;
            for k in path {
                cur = cur.get(*k)?;
            }
            cur.as_str().map(|s| s.to_string())
        };
        let n = |path: &[&str]| -> Option<u64> {
            let mut cur = &v;
            for k in path {
                cur = cur.get(*k)?;
            }
            cur.as_u64()
        };
        Some(RepositoryStatus {
            config_file: s(&["configFile"]),
            unique_id: s(&["uniqueIDHex"]),
            storage_type: s(&["storage", "type"]),
            hash: s(&["contentFormat", "hash"]),
            encryption: s(&["contentFormat", "encryption"]),
            ecc: s(&["contentFormat", "ecc"]),
            ecc_overhead_percent: n(&["contentFormat", "eccOverheadPercent"]),
            splitter: s(&["objectFormat", "splitter"]),
            format_version: n(&["contentFormat", "version"]),
            hostname: s(&["clientOptions", "hostname"]),
            username: s(&["clientOptions", "username"]),
            read_only: v
                .get("clientOptions")
                .and_then(|c| c.get("readonly"))
                .and_then(|b| b.as_bool())
                .unwrap_or(false),
            volume_total_bytes: n(&["volume", "capacity"]),
            volume_free_bytes: n(&["volume", "available"]),
            raw: v,
        })
    }
}

/// What the destination actually costs at the provider.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BlobStats {
    pub blob_count: u64,
    pub total_bytes: u64,
}

impl BlobStats {
    /// Parse `Count: N` / `Total: N` from `blob stats --raw`.
    ///
    /// Deliberately tolerant of the histogram that follows and of the
    /// `Got N blobs...` progress lines kopia interleaves on large repositories.
    pub fn parse(stdout: &str) -> Option<BlobStats> {
        let mut stats = BlobStats::default();
        let mut saw_total = false;
        for line in stdout.lines() {
            let line = line.trim();
            if let Some(v) = line.strip_prefix("Count:") {
                stats.blob_count = v.trim().parse().unwrap_or(0);
            } else if let Some(v) = line.strip_prefix("Total:") {
                match v.trim().parse() {
                    Ok(n) => {
                        stats.total_bytes = n;
                        saw_total = true;
                    }
                    // Without `--raw` this is `1.2 GB`; accept it rather than
                    // failing outright.
                    Err(_) => {
                        if let Some(n) = super::progress::parse_bytes(v) {
                            stats.total_bytes = n;
                            saw_total = true;
                        }
                    }
                }
            }
        }
        saw_total.then_some(stats)
    }
}

/// Logical content statistics, before deduplication and compression.
///
/// `content stats` prints a free-form report (`cli/command_content_stats.go`)
/// with no `--json`, so only the headline numbers are extracted and the whole
/// text is kept for the "advanced" disclosure.
#[derive(Debug, Clone, Default)]
pub struct ContentStats {
    pub content_count: Option<u64>,
    pub total_bytes: Option<u64>,
    pub report: String,
}

impl ContentStats {
    pub fn parse(stdout: &str) -> ContentStats {
        let mut out = ContentStats { report: stdout.trim().to_string(), ..Default::default() };
        for line in stdout.lines() {
            let line = line.trim();
            if let Some(v) = line.strip_prefix("Count:") {
                out.content_count = v.trim().parse().ok();
            } else if let Some(v) = line.strip_prefix("Total:") {
                out.total_bytes = v.trim().parse().ok().or_else(|| super::progress::parse_bytes(v));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storj_endpoint_is_reduced_to_a_bare_host() {
        assert_eq!(
            s3_endpoint_host("https://gateway.storjshare.io"),
            ("gateway.storjshare.io".to_string(), false)
        );
        assert_eq!(
            s3_endpoint_host("gateway.storjshare.io"),
            ("gateway.storjshare.io".to_string(), false)
        );
        assert_eq!(
            s3_endpoint_host("http://192.168.1.10:9000/"),
            ("192.168.1.10:9000".to_string(), true)
        );
        assert_eq!(
            s3_endpoint_host("  https://s3.eu-central-1.wasabisys.com/some/path?x=1 "),
            ("s3.eu-central-1.wasabisys.com".to_string(), false)
        );
        assert_eq!(s3_endpoint_host(""), (String::new(), false));
    }

    #[test]
    fn throttle_values_match_kopias_units() {
        // 1024 KB/s of ours is 1048576 bytes/s of kopia's.
        assert_eq!(throttle_value(Some(1024)), "1048576");
        assert_eq!(throttle_value(Some(0)), "unlimited");
        assert_eq!(throttle_value(None), "unlimited");
    }

    #[test]
    fn repository_status_json_is_parsed() {
        let json = r#"{
          "configFile": "C:\\data\\kopia\\dest.config",
          "uniqueIDHex": "0a1b2c",
          "clientOptions": {"hostname":"workstation","username":"andreas","readonly":false},
          "storage": {"type":"s3","config":{"bucket":"backups","endpoint":"gateway.storjshare.io"}},
          "contentFormat": {"hash":"BLAKE2B-256-128","encryption":"AES256-GCM-HMAC-SHA256",
                            "ecc":"REED-SOLOMON-CRC32","eccOverheadPercent":2,"version":3},
          "objectFormat": {"splitter":"DYNAMIC-4M-BUZHASH"},
          "volume": {"capacity":1000000000000,"available":250000000000}
        }"#;
        let s = RepositoryStatus::parse(json).expect("parses");
        assert_eq!(s.storage_type.as_deref(), Some("s3"));
        assert_eq!(s.encryption.as_deref(), Some("AES256-GCM-HMAC-SHA256"));
        assert_eq!(s.splitter.as_deref(), Some("DYNAMIC-4M-BUZHASH"));
        assert_eq!(s.ecc_overhead_percent, Some(2));
        assert_eq!(s.format_version, Some(3));
        assert_eq!(s.volume_free_bytes, Some(250_000_000_000));
        assert!(!s.read_only);
        assert!(RepositoryStatus::parse("not json").is_none());
    }

    #[test]
    fn blob_stats_raw_output_is_parsed() {
        let raw = "Count: 12043\nTotal: 88123456789\nAverage: 7318231\nHistogram:\n\n        1 between 0 and 10 (total 4)\n";
        assert_eq!(
            BlobStats::parse(raw),
            Some(BlobStats { blob_count: 12043, total_bytes: 88_123_456_789 })
        );
    }

    #[test]
    fn blob_stats_tolerates_human_units_and_progress_noise() {
        let human = "Got 10000 blobs...\nCount: 5\nTotal: 1.2 GB\nAverage: 240 MB\n";
        assert_eq!(
            BlobStats::parse(human),
            Some(BlobStats { blob_count: 5, total_bytes: 1_200_000_000 })
        );
        assert_eq!(BlobStats::parse("nothing useful"), None);
    }

    #[test]
    fn content_stats_keeps_the_report() {
        let s = ContentStats::parse("Count: 9\nTotal: 1234\nsomething else\n");
        assert_eq!(s.content_count, Some(9));
        assert_eq!(s.total_bytes, Some(1234));
        assert!(s.report.contains("something else"));
    }
}
