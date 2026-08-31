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
    /// True when this destination is a replica of another. Everything else a
    /// driver does is identical for a replica — connecting, listing, restoring,
    /// verifying — so this exists for exactly one purpose: refusing
    /// [`KopiaDriver::create_repository`]. See the note there.
    is_replica: bool,
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
            return Err(KopiaError::local(label, KopiaFailure::Unusable, None).with_message(
                format!(
                "\"{}\" points at a storage provider that no longer exists in the configuration.",
                destination.name
            ),
            ));
        }

        Ok(KopiaDriver {
            binary,
            destination_id: destination.id,
            destination_name: destination.name.clone(),
            kind: destination.kind.clone(),
            provider: provider.cloned(),
            encryption: destination.encryption.clone().unwrap_or_default(),
            secrets,
            is_replica: destination.is_replica(),
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
        append_storage_args(cmd, &self.kind, self.provider.as_ref())
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
        // A replica must never be created, and the refusal lives here rather
        // than at each call site because the call sites include a button in the
        // GUI. Creating a repository mints a fresh unique id in its format
        // blob, and `repository sync-to` refuses a destination whose format
        // blob does not match the source's — so one press of "Create
        // repository" would permanently break the chain, and would do it
        // silently until the next backup ran.
        if self.is_replica {
            return Err(KopiaError::local("repository create", KopiaFailure::Unusable, None)
                .with_message(format!(
                    "\"{}\" is a copy of another destination's repository, so it is created by \
                     the first backup that replicates into it, not here. Creating one now would \
                     make a different repository that could never be synchronised.",
                    self.destination_name
                )));
        }
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
    // Replication
    // -----------------------------------------------------------------

    /// Replicate this repository into a second storage location with
    /// `kopia repository sync-to`.
    ///
    /// This is the chained-backup primitive: the sources are read, chunked and
    /// encrypted **once**, into this repository, and the offsite copy is then
    /// made from the resulting blobs rather than from the user's disk a second
    /// time.
    ///
    /// # What kopia does, and what the caller must already know
    ///
    /// `sync-to` is blob-level replication, verified against
    /// `cli/command_repository_sync.go`. It lists both storage locations,
    /// copies what the destination is missing (and, with `--update`, what the
    /// source has a newer copy of), and optionally deletes what the source no
    /// longer has. Before any of that it compares the two `kopia.repository`
    /// format blobs and **refuses two different repositories**:
    /// `destination repository contains incompatible data`.
    ///
    /// So the destination is not an independently keyed repository and cannot
    /// be made into one. It ends up holding this repository's format blob, and
    /// therefore this repository's master key, and it opens with this
    /// repository's passphrase. [`crate::model::Destination::replicate_from`]
    /// documents the model consequences; the validator enforces them.
    ///
    /// # Guarantees, matching the rest of this module
    ///
    /// * The destination's object-store credentials travel in the environment
    ///   and never in argv — see [`KopiaCommand::replace_secret_env`] for why
    ///   they *replace* rather than shadow this repository's own.
    /// * Cancellation kills the child and awaits its reap, because
    ///   [`KopiaCommand::run`] does; there is no `select!` around it here.
    /// * Progress is streamed: `--progress` is passed explicitly because kopia
    ///   suppresses `outputSyncProgress` entirely when stdout is not a
    ///   terminal, and stdout is a pipe.
    ///
    /// # Flags
    ///
    /// Verified against the flag registrations in
    /// `cli/command_repository_sync.go`: `--update` (default true, left
    /// alone), `--delete`, `--dry-run`, `--parallel`, `--must-exist`,
    /// `--times`.
    ///
    /// # The dry-run trap
    ///
    /// `--dry-run` is **not** on its own side-effect-free.
    /// `runSyncWithStorage` calls `ensureRepositoriesHaveSameFormatBlob`
    /// *before* it checks the dry-run flag, and that function writes the source
    /// format blob to a destination that has none. A rehearsal against an empty
    /// bucket would therefore create the repository there. So a dry run here
    /// always passes `--must-exist` as well, which turns that case into a clean
    /// refusal instead of a write. A rehearsal that reports "nothing happened"
    /// while having initialised a repository would be a lie of exactly the kind
    /// this codebase spends its effort avoiding.
    pub async fn sync_to(
        &self,
        target: &SyncTarget,
        options: &SyncOptions,
        ctx: &RunContext,
    ) -> KopiaResult<SyncOutcome> {
        let label = "repository sync-to";
        self.require_passphrase(label)?;

        let mut cmd = self.base();
        cmd.global_bool("progress", true);
        cmd.command("repository").command("sync-to");
        append_storage_args(&mut cmd, &target.kind, target.provider.as_ref())?;

        // The environment now belongs to the *destination*: the source
        // repository is opened from its own stored connection profile, which
        // carries its credentials, while kopia binds the `sync-to` storage
        // subcommand's `--access-key` and friends to these variables.
        match (&target.secrets.access_key, &target.secrets.secret_key) {
            (Some(access), Some(secret)) => {
                cmd.replace_secret_env("AWS_ACCESS_KEY_ID", access);
                cmd.replace_secret_env("AWS_SECRET_ACCESS_KEY", secret);
                match &target.secrets.session_token {
                    Some(token) => cmd.replace_secret_env("AWS_SESSION_TOKEN", token),
                    None => cmd.clear_secret_env("AWS_SESSION_TOKEN"),
                };
            }
            _ if matches!(target.kind, DestinationKind::S3 { .. }) => {
                return Err(KopiaError::local(label, KopiaFailure::StorageAuth, None)
                    .with_message(format!(
                        "No storage credentials are available for \"{}\", so the offsite \
                         copy cannot be written.",
                        target.name
                    )));
            }
            // A filesystem destination needs no credentials, and this
            // repository's own are harmless there.
            _ => {}
        }

        if options.dry_run {
            // See "The dry-run trap" above: these two go together or not at all.
            cmd.switch("dry-run").switch("must-exist");
        } else if options.must_exist {
            cmd.switch("must-exist");
        }
        if options.delete {
            cmd.switch("delete");
        }
        if options.times {
            cmd.switch("times");
        }
        if let Some(parallel) = options.parallel.filter(|p| *p > 0) {
            cmd.flag("parallel", parallel.to_string());
        }

        let mut ctx = ctx.clone();
        if ctx.current_path.is_none() {
            // kopia's sync progress line has no "current blob" field, so the
            // GUI is given the thing a person actually wants to read.
            ctx.current_path = Some(target.name.clone());
        }

        let out = self.run(cmd, &ctx).await?;

        let mut progress = out.progress;
        progress.current_path = None;
        progress.estimated_seconds_remaining = Some(0);

        // kopia rate-limits `outputSyncProgress` (`nextSyncOutputTime.
        // ShouldOutput`), so the frame that would have shown the last blobs is
        // frequently never printed, and there is no machine-readable summary at
        // the end the way `snapshot create --json` gives one. The command
        // succeeded, which means everything the inventory line listed was
        // copied — so the planned figures are the truth, and keeping the last
        // sampled ones would park a finished replication at 87% in the history
        // for ever.
        //
        // For a rehearsal this reports what *would* be copied, which is the
        // same convention the snapshot dry run uses (`snapshot estimate` fills
        // the processed counters); the "nothing was copied" warning carries the
        // distinction.
        progress.files_processed = progress.files_processed.max(progress.files_total.unwrap_or(0));
        progress.bytes_processed = progress.bytes_processed.max(progress.bytes_total.unwrap_or(0));
        progress.files_total = Some(progress.files_processed);
        progress.bytes_total = Some(progress.bytes_processed);
        // Every replicated byte crossed the wire: kopia only copies blobs the
        // destination was missing, so there is no dedup saving to subtract.
        progress.bytes_uploaded = progress.bytes_processed;

        Ok(SyncOutcome {
            blobs_copied: progress.files_processed,
            bytes_copied: progress.bytes_processed,
            progress,
            warnings: out.warnings,
            dry_run: options.dry_run,
        })
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

// ---------------------------------------------------------------------------
// Replication
// ---------------------------------------------------------------------------

/// The second storage location of a [`KopiaDriver::sync_to`].
///
/// Deliberately *not* a second [`KopiaDriver`]. A driver owns a config file, a
/// cache directory and a repository passphrase; a sync destination has none of
/// those, because it is not connected to and is not a separate repository —
/// it is a place to put this repository's blobs. Modelling it as a driver
/// would invite exactly the mistake the type exists to prevent: creating a
/// repository there, with its own key, that `sync-to` would then refuse.
#[derive(Debug, Clone)]
pub struct SyncTarget {
    id: Uuid,
    name: String,
    kind: DestinationKind,
    provider: Option<StorageProvider>,
    /// Object-store credentials only. A passphrase here would be meaningless:
    /// the replica opens with the *source* repository's passphrase.
    secrets: DestinationSecrets,
}

impl SyncTarget {
    /// Describe a destination as a replication target.
    ///
    /// Fails fast on the two shapes kopia could never satisfy — a folder
    /// mirror, which holds files rather than blobs, and an S3 destination
    /// whose provider has gone missing from the configuration.
    pub fn new(
        destination: &Destination,
        provider: Option<&StorageProvider>,
        secrets: DestinationSecrets,
    ) -> KopiaResult<SyncTarget> {
        let label = format!("destination {}", destination.name);
        if !destination.kind.is_repository() {
            return Err(KopiaError::local(label, KopiaFailure::Unusable, None).with_message(
                format!(
                    "\"{}\" is a folder mirror, so a repository cannot be replicated into it.",
                    destination.name
                ),
            ));
        }
        if matches!(destination.kind, DestinationKind::S3 { .. }) && provider.is_none() {
            return Err(KopiaError::local(label, KopiaFailure::Unusable, None).with_message(
                format!(
                    "\"{}\" points at a storage provider that no longer exists in the \
                     configuration.",
                    destination.name
                ),
            ));
        }
        Ok(SyncTarget {
            id: destination.id,
            name: destination.name.clone(),
            kind: destination.kind.clone(),
            provider: provider.cloned(),
            // A passphrase would never be used and must not be carried around:
            // the whole point of a replica is that it has none of its own.
            secrets: DestinationSecrets {
                passphrase: None,
                access_key: secrets.access_key,
                secret_key: secrets.secret_key,
                session_token: secrets.session_token,
            },
        })
    }

    pub fn destination_id(&self) -> &Uuid {
        &self.id
    }
    pub fn destination_name(&self) -> &str {
        &self.name
    }
}

/// Knobs for one `repository sync-to`.
#[derive(Debug, Clone)]
pub struct SyncOptions {
    /// Remove blobs from the replica that the source no longer holds.
    ///
    /// Off by default, matching kopia. With it off, a replica keeps blobs the
    /// source's retention has since expired — which costs storage but is the
    /// safe direction: a mistaken deletion at the source cannot propagate.
    pub delete: bool,
    /// Copy parallelism. Kopia defaults to 1, which is far too slow for an
    /// offsite bucket over a domestic uplink.
    pub parallel: Option<u32>,
    /// Refuse a destination that holds no repository format blob yet.
    ///
    /// This is what stops an unmounted external drive or a mistyped bucket
    /// from quietly becoming a brand-new offsite copy of everything.
    pub must_exist: bool,
    /// Synchronise blob timestamps where the backend supports them.
    pub times: bool,
    /// Report what would be copied without copying it. Implies
    /// [`SyncOptions::must_exist`]; see [`KopiaDriver::sync_to`].
    pub dry_run: bool,
}

impl Default for SyncOptions {
    fn default() -> Self {
        SyncOptions {
            delete: false,
            // Eight is kopia's own recommendation for object storage and is
            // conservative for a filesystem destination; it is the difference
            // between a chained offsite copy finishing overnight and not.
            parallel: Some(8),
            must_exist: false,
            times: false,
            dry_run: false,
        }
    }
}

/// What a finished `repository sync-to` produced.
#[derive(Debug, Clone, Default)]
pub struct SyncOutcome {
    pub blobs_copied: u64,
    pub bytes_copied: u64,
    /// Final counters, in the shape the run history stores. Blob counts occupy
    /// the file fields; see `ProgressTracker::apply_sync`.
    pub progress: crate::state::Progress,
    pub warnings: Vec<String>,
    /// True when nothing was written because this was a rehearsal.
    pub dry_run: bool,
}

/// Express one storage location as a kopia storage subcommand and its flags.
///
/// Shared by `repository create`, `repository connect` and
/// `repository sync-to`, which all take the same per-provider subcommand
/// (`cli/storage_providers.go` registers one under each). Keeping it in one
/// function is what guarantees a chained destination is addressed exactly the
/// way the same destination would be addressed directly — a `--prefix` or a
/// `--disable-tls` that applied to one path and not the other would produce
/// two different storage locations under one name.
fn append_storage_args(
    cmd: &mut KopiaCommand,
    kind: &DestinationKind,
    provider: Option<&StorageProvider>,
) -> KopiaResult<()> {
    match kind {
        DestinationKind::LocalRepository { path } | DestinationKind::OneDrive { path, .. } => {
            cmd.command("filesystem").flag("path", path);
        }
        DestinationKind::S3 { bucket, prefix, .. } => {
            let Some(StorageProvider {
                kind: ProviderKind::S3 { endpoint, region, tls, .. }, ..
            }) = provider
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
