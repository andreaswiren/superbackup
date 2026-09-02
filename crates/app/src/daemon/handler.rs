//! The [`Handler`] implementation: sixty commands wired to the store, the
//! engine, the kopia driver, the platform layer and the remote.
//!
//! ## What every method here obeys
//!
//! 1. **No secret leaves.** There is no `GetSecret` in the protocol and this
//!    file adds none. `vault.set_secret` and
//!    `provider.rotate_credentials` take [`SecretString`]s in; nothing returns
//!    one, and no error message, hint, detail or status field is built from
//!    one. `vault.list_refs` returns handles, never values.
//! 2. **No panic from input.** Every lookup is a `Result`, every numeric
//!    conversion is saturating, and no method indexes, unwraps or slices on
//!    data that arrived over the socket.
//! 3. **No lock across an await that could re-enter.** The store's `tokio`
//!    mutex is taken, used, and dropped inside one statement group; nothing
//!    calls back into the handler while holding it.
//! 4. **Handlers return promptly.** Anything that can take minutes — a
//!    backup, a restore, a repository creation over a slow link — returns a
//!    run id and does its work on a task. That is the transport's documented
//!    expectation, and the reason `handler_timeout` exists as a backstop
//!    rather than as a normal path.
//!
//! ## Authorisation
//!
//! The daemon may run as SYSTEM while its clients are ordinary user
//! processes, so authorisation is on what the caller *knows*, not on the fact
//! that it reached the socket. Everything that touches secret material goes
//! through [`DaemonHandler::require_unlocked`], which is the single place that
//! turns "the vault is locked" into [`Error::Locked`].

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::{Duration as ChronoDuration, Utc};
use superbackup_core::ipc::protocol::*;
use superbackup_core::ipc::{Handler, RequestContext, StreamItem, Topic};
use superbackup_core::kopia::{
    cancellation, EventSink, KopiaEvent, RestoreOptions, RunContext, MINIMUM_KOPIA_VERSION,
};
use superbackup_core::model::{
    Destination, DestinationKind, EncryptionSettings, Job, PauseState, ProviderKind, SecretRef,
    Settings, StorageProvider,
};
use superbackup_core::platform::{self, ServiceScope};
use superbackup_core::secret::Secret;
use superbackup_core::state::{Event, RunStatus, Severity, Trigger};
use superbackup_core::{Error, ErrorCode, Result};
use tokio::sync::broadcast;
use uuid::Uuid;

use super::executor::build_driver;
use super::runtime::{
    health_summary, resolve_destination, resolve_job, resolve_provider, settings_changed_event,
    Runtime,
};

/// Largest directory listing `snapshot.browse` will serialise in one reply.
///
/// A `node_modules` with forty thousand entries must not become a forty-
/// megabyte IPC frame; the reply carries `truncated: true` and the caller
/// descends or filters instead.
const MAX_LISTING_ENTRIES: usize = 2_000;

/// Cap on `snapshot.list` when the caller passes 0 or something absurd.
const MAX_SNAPSHOTS: usize = 500;

/// Kopia's format blob. Its presence, and nothing else, is what "there is a
/// repository here" means — reading it needs the repository key, and needing
/// the key is precisely the coupling `dest.test` exists without.
const KOPIA_FORMAT_BLOB: &str = "kopia.repository";

/// The same blob as kopia's **filesystem** backend writes it.
///
/// `repo/blob/filesystem` appends a `.f` suffix to every blob id it stores, so
/// on a local folder or a OneDrive folder the format blob is on disk as
/// `kopia.repository.f`. Object stores have no such suffix, which is why the
/// S3 path matches the bare name.
///
/// Looking only for the bare name meant a local repository that had just been
/// created successfully still reported "reachable, but it has no backup
/// repository in it yet" — sending the user to press "Create repository" on a
/// repository that already existed.
const KOPIA_FORMAT_BLOB_FS: &str = "kopia.repository.f";

/// What one `ListBuckets` against a provider established.
///
/// Deliberately not a `Result`: "the endpoint answered and refused to list"
/// is neither a success nor a failure, it is a *qualified* success, and a type
/// with only two outcomes would force that third case into whichever of them
/// fitted worse. `credentials_ok` without `listed` is the case that exists.
#[derive(Debug, Clone)]
struct BucketProbe {
    buckets: Vec<BucketInfo>,
    listed: bool,
    credentials_ok: bool,
    detail: Option<String>,
    latency_ms: Option<u64>,
}

/// The IPC surface of a running daemon.
#[derive(Debug, Clone)]
pub struct DaemonHandler {
    runtime: Arc<Runtime>,
}

impl DaemonHandler {
    pub fn new(runtime: Arc<Runtime>) -> DaemonHandler {
        DaemonHandler { runtime }
    }

    /// Refuse anything that needs secret material while the vault is locked.
    async fn require_unlocked(&self) -> Result<()> {
        if self.runtime.store.lock().await.is_locked() {
            return Err(Error::Locked);
        }
        Ok(())
    }

    /// The current configuration, cloned out from under the lock.
    async fn config(&self) -> superbackup_core::model::Config {
        self.runtime.store.lock().await.config().clone()
    }

    /// Apply a change to the configuration: validate, persist, hand it to the
    /// scheduler, and publish a fresh status.
    ///
    /// Every mutating command funnels through this so that "saved but the
    /// scheduler never heard about it" cannot happen — the failure that makes
    /// a user edit a schedule twice and then stop trusting the app.
    async fn commit(
        &self,
        mutate: impl FnOnce(&mut superbackup_core::model::Config) -> Result<()>,
    ) -> Result<superbackup_core::model::Config> {
        let mut store = self.runtime.store.lock().await;
        let mut config = store.config().clone();
        mutate(&mut config)?;
        store.set_config(config)?;
        let saved = store.config().clone();
        drop(store);
        self.runtime.push_config(&saved);
        self.runtime.publish_status().await;
        Ok(saved)
    }

    /// Look up a destination and build a kopia driver for it.
    /// The object id of a snapshot's root directory.
    ///
    /// Accepts either identifier: an object id is returned unchanged, so a
    /// caller that already resolved one pays nothing, and a manifest id is
    /// looked up. A snapshot that cannot be found is named in the error rather
    /// than surfacing as one of kopia's content-id complaints.
    async fn snapshot_root_object(
        &self,
        driver: &superbackup_core::kopia::KopiaDriver,
        snapshot: &str,
    ) -> Result<String> {
        let snapshot = snapshot.trim();
        if snapshot.is_empty() {
            return Err(Error::Validation("no snapshot was named".into()));
        }
        if is_object_id(snapshot) {
            return Ok(snapshot.to_string());
        }
        let manifests = driver.browse_roots(&RunContext::new()).await.map_err(kopia_to_error)?;
        let found = manifests
            .iter()
            .find(|m| m.id == snapshot)
            .ok_or_else(|| Error::Validation(format!("no snapshot with id {snapshot} is here")))?;
        let root = found
            .root_entry
            .as_ref()
            .map(|e| e.object_id.clone())
            .filter(|o| !o.is_empty())
            .ok_or_else(|| {
                Error::Validation(format!(
                    "snapshot {snapshot} records no root directory, so there is nothing to browse.                      A run that was interrupted before it finished can leave one like this."
                ))
            })?;
        Ok(root)
    }

    async fn driver_for(
        &self,
        needle: &str,
    ) -> Result<(Destination, superbackup_core::kopia::KopiaDriver)> {
        let binary = self.runtime.kopia().ok_or(Error::KopiaMissing)?;
        let store = self.runtime.store.lock().await;
        let destination = resolve_destination(store.config(), needle)?.clone();
        let driver = build_driver(&store, &self.runtime.paths, binary, &destination);
        drop(store);
        Ok((destination, driver?))
    }

    /// Resolve a provider's stored key pair for one S3 request.
    ///
    /// The resolved material lives only as long as the returned [`S3Keys`],
    /// which zeroes both halves on drop. Nothing here logs, formats or
    /// returns it: the only thing that ever sees the secret is the signer.
    async fn provider_keys(
        &self,
        provider: &StorageProvider,
    ) -> Result<superbackup_core::s3::S3Keys> {
        let ProviderKind::S3 { credentials, .. } = &provider.kind;
        self.resolve_keys(credentials, &provider.name).await
    }

    /// Resolve one credential handle set. Shared by the provider probe and the
    /// destination probe so a destination with its own key pair and a
    /// destination inheriting the provider's are read the same way.
    async fn resolve_keys(
        &self,
        credentials: &superbackup_core::model::S3Credentials,
        owner: &str,
    ) -> Result<superbackup_core::s3::S3Keys> {
        let store = self.runtime.store.lock().await;
        let access = store.secret(&credentials.access_key_ref)?;
        let secret = store.secret(&credentials.secret_key_ref)?;
        let (Some(access), Some(secret)) = (access, secret) else {
            // Not `Error::Locked` and not a vault error: the vault is fine and
            // simply has nothing under this handle, which is what an unsaved
            // provider looks like. Saying "the vault has no entry for
            // s3.access:…" reads like corruption for what is an unfinished
            // form.
            return Err(Error::Validation(format!(
                "\"{owner}\" has no credentials stored yet. Enter an access key and a secret key \
                 and save it first."
            )));
        };
        let token = match &credentials.session_token_ref {
            Some(handle) => store.secret(handle)?,
            None => None,
        };
        Ok(superbackup_core::s3::S3Keys::new(access, secret).with_session_token(token))
    }

    /// Record that this provider's credentials were accepted just now.
    ///
    /// `last_verified_at` was cleared on a key rotation but never set by
    /// anything, so the providers screen's "Last checked" column could only
    /// ever be empty. A successful, authenticated `ListBuckets` is precisely
    /// the event that column is about.
    ///
    /// A failure to persist it is swallowed: a bookkeeping timestamp must
    /// never turn a successful credential check into a reported failure.
    async fn mark_provider_verified(&self, id: Uuid) {
        let now = Utc::now();
        if let Err(e) = self
            .commit(move |config| {
                if let Some(slot) = config.providers.iter_mut().find(|p| p.id == id) {
                    slot.last_verified_at = Some(now);
                }
                Ok(())
            })
            .await
        {
            tracing::warn!(error = %e, "could not record the provider check timestamp");
        }
    }

    /// Reach an S3 destination without a repository key.
    ///
    /// Three facts in at most three round trips: the bucket answers and the
    /// credentials are accepted (the listing), the prefix can be written to
    /// (the probe), and whether a repository is already there (the listing
    /// again, targeted at the exact format-blob key).
    async fn probe_s3_destination(
        &self,
        destination: &Destination,
        bucket: &str,
        prefix: &str,
    ) -> ProbeReply {
        let unreachable = |detail: String| ProbeReply {
            reachable: false,
            writable: false,
            latency_ms: None,
            repository_present: None,
            detail: Some(detail),
        };

        let config = self.config().await;
        let Some(provider) =
            destination.kind.provider_id().and_then(|id| config.provider(id)).cloned()
        else {
            return unreachable(format!(
                "\"{}\" points at a storage provider that is no longer in the configuration.",
                destination.name
            ));
        };
        // A destination may pin its own key pair; `effective_credentials`
        // resolves the override before the provider's.
        let credentials = match destination.kind.effective_credentials(Some(&provider)) {
            Some(c) => c.clone(),
            None => return unreachable(format!("\"{}\" has no credentials.", destination.name)),
        };
        let keys = match self.resolve_keys(&credentials, &provider.name).await {
            Ok(keys) => keys,
            Err(e) => return unreachable(e.to_string()),
        };
        let client = match superbackup_core::s3::S3Client::new() {
            Ok(client) => client,
            Err(e) => return unreachable(e.message()),
        };

        let describe = |e: superbackup_core::s3::S3Error| match e.hint() {
            Some(hint) => format!("{} {hint}", e.message()),
            None => e.message(),
        };

        // One targeted listing: it proves endpoint, TLS, credentials and
        // bucket existence, and its result *is* the repository answer.
        let format_blob = format!("{prefix}{}", KOPIA_FORMAT_BLOB);
        let repository_present =
            match client.object_exists(&provider, &keys, bucket, &format_blob).await {
                Ok(present) => present,
                Err(e) => return unreachable(describe(e)),
            };

        // Reachable and authenticated from here on: a write failure is a
        // permission problem to report, not a reason to claim the bucket
        // cannot be reached.
        let (writable, write_problem) =
            match client.write_probe(&provider, &keys, bucket, prefix).await {
                Ok(()) => (true, None),
                Err(e) => (false, Some(describe(e))),
            };

        ProbeReply {
            reachable: true,
            writable,
            latency_ms: None,
            repository_present: Some(repository_present),
            detail: match (writable, repository_present) {
                (false, _) => write_problem,
                (true, false) => Some(copy_no_repository_yet(&destination.kind)),
                (true, true) => None,
            },
        }
    }

    /// Ask a provider for its bucket list, and classify what came back.
    ///
    /// One place, because `provider.test`, `provider.list_buckets` and the
    /// destination editor's picker must agree about what "the credentials are
    /// fine but the key cannot list" means. Three call sites deciding that
    /// separately is how one of them ends up telling the user their key is
    /// wrong when it is not.
    async fn probe_provider(&self, provider: &StorageProvider) -> BucketProbe {
        let keys = match self.provider_keys(provider).await {
            Ok(keys) => keys,
            Err(e) => {
                return BucketProbe {
                    buckets: Vec::new(),
                    listed: false,
                    credentials_ok: false,
                    detail: Some(e.to_string()),
                    latency_ms: None,
                }
            }
        };
        let client = match superbackup_core::s3::S3Client::new() {
            Ok(client) => client,
            Err(e) => {
                return BucketProbe {
                    buckets: Vec::new(),
                    listed: false,
                    credentials_ok: false,
                    detail: Some(e.message()),
                    latency_ms: None,
                }
            }
        };
        let started = std::time::Instant::now();
        let result = client.list_buckets(provider, &keys).await;
        let latency = Some(started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64);
        match result {
            Ok(buckets) => BucketProbe {
                buckets: buckets
                    .into_iter()
                    .map(|b| BucketInfo { name: b.name, created_at: b.created_at })
                    .collect(),
                listed: true,
                credentials_ok: true,
                detail: None,
                latency_ms: latency,
            },
            Err(e) => BucketProbe {
                buckets: Vec::new(),
                listed: false,
                // `AccessDenied` means the signature verified before the
                // policy refused, so the key pair is provably right.
                credentials_ok: e.credentials_accepted(),
                detail: Some(match e.hint() {
                    Some(hint) => format!("{} {hint}", e.message()),
                    None => e.message(),
                }),
                latency_ms: latency,
            },
        }
    }

    fn unlocked_reply(&self, unlocked: bool) -> UnlockedReply {
        UnlockedReply { unlocked, auto_lock_at: self.runtime.auto_lock_at() }
    }

    async fn pause_reply(&self) -> PauseReply {
        PauseReply { pause: self.config().await.settings.pause.clone() }
    }

    fn service_reply(&self, detail: Option<String>) -> ServiceReply {
        let status = self.runtime.service_status();
        let autostart = platform::autostart::is_enabled().unwrap_or(false);
        // Never fatal: a launcher entry that cannot be read must not stop the
        // service page from telling the user about the service.
        let (in_menu, menu_path) = match platform::autostart::AutostartSpec::current() {
            Ok(spec) => (
                platform::shortcut::status(&spec).is_installed(),
                Some(platform::shortcut::location()),
            ),
            Err(_) => (false, None),
        };
        ServiceReply {
            installed: status.installed,
            running: status.state == platform::ServiceState::Running,
            autostart,
            scope: if self.runtime.paths.service_scope { "system".into() } else { "user".into() },
            detail: detail.or(status.detail),
            in_applications_menu: in_menu,
            applications_menu_path: menu_path,
        }
    }

    /// `repository status` against one destination, captured verbatim.
    ///
    /// Every reason it could not run — a locked vault, a folder mirror, a
    /// missing destination — becomes a *record* rather than an error, because
    /// `kopia.probe` is a diagnostic and half an answer beats none.
    async fn status_invocation(
        &self,
        needle: &str,
        ctx: &RunContext,
    ) -> superbackup_core::kopia::RawInvocation {
        use superbackup_core::kopia::RawInvocation;
        const LABEL: &str = "repository status";

        if self.runtime.store.lock().await.is_locked() {
            return RawInvocation::not_attempted(
                LABEL,
                "The vault is locked, so the repository's encryption key cannot be resolved. \
                 Unlock superbackup and check again.",
            );
        }
        match self.driver_for(needle).await {
            Ok((_, driver)) => driver.status_invocation(ctx).await,
            Err(e) => RawInvocation::not_attempted(LABEL, e.to_string()),
        }
    }

    async fn remote_status_reply(&self, detail: Option<String>) -> RemoteStatusReply {
        let config = self.config().await;
        let Some(remote) = &config.remote else {
            return RemoteStatusReply {
                url: None,
                branch: None,
                last_pull_at: None,
                last_known_commit: None,
                local_changes: false,
                remote_changes: false,
                detail: Some("No remote configuration repository is set up.".into()),
            };
        };
        let staged = self.runtime.pull();
        let remote_changes = staged.as_ref().map(|p| !p.diff.is_empty()).unwrap_or(false);
        RemoteStatusReply {
            url: Some(remote.url.clone()),
            branch: Some(remote.branch.clone()),
            last_pull_at: remote.last_pull_at,
            last_known_commit: remote.last_known_commit.clone(),
            local_changes: config.updated_at > remote.last_pull_at,
            remote_changes,
            detail,
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers that are pure enough to test on their own
// ---------------------------------------------------------------------------

/// Map the protocol's conflict policy onto kopia's restore flags.
///
/// `KeepBoth` is refused rather than approximated. kopia's `restore`
/// (`cli/command_restore.go`) offers overwrite, skip-existing and
/// delete-extra, and nothing that writes `file (1).txt` — so honouring the
/// request would mean silently doing something else with the user's data
/// during a *restore*, which is the last operation in the program where a
/// surprise is acceptable.
pub fn restore_options(conflict: ConflictPolicy) -> Result<RestoreOptions> {
    let base = RestoreOptions::default();
    match conflict {
        ConflictPolicy::Skip => {
            Ok(RestoreOptions { overwrite_files: false, skip_existing: true, ..base })
        }
        ConflictPolicy::Overwrite => {
            Ok(RestoreOptions { overwrite_files: true, skip_existing: false, ..base })
        }
        // Neither overwrite nor skip: kopia then fails on the first collision,
        // which is exactly `Fail`.
        ConflictPolicy::Fail => {
            Ok(RestoreOptions { overwrite_files: false, skip_existing: false, ..base })
        }
        ConflictPolicy::KeepBoth => Err(Error::Validation(
            "kopia cannot restore a file alongside an existing one. Choose Skip, Overwrite or \
             Fail, or restore into an empty folder and merge afterwards."
                .into(),
        )),
    }
}

/// `snapshot.browse` addresses an entry as `<snapshot id>/<path inside it>`.
/// Normalise the caller's path into that shape without letting it escape.
/// Is this already a kopia object id, rather than a snapshot manifest id?
///
/// The two are easy to confuse and kopia accepts them in different places: a
/// manifest id names a snapshot, an object id names the directory *inside* it,
/// and `kopia show` takes only the second. A directory object id is `k`
/// followed by 64 hex characters; a manifest id is 32 hex characters with no
/// prefix, so the two cannot be mistaken for one another.
fn is_object_id(candidate: &str) -> bool {
    let Some(rest) = candidate.strip_prefix('k') else { return false };
    rest.len() > 32 && rest.chars().all(|c| c.is_ascii_hexdigit())
}

pub fn browse_target(snapshot: &str, path: &str) -> Result<String> {
    if snapshot.trim().is_empty() {
        return Err(Error::Validation("no snapshot was named".into()));
    }
    if snapshot.contains('/') || snapshot.contains('\\') {
        return Err(Error::Validation("a snapshot id contains no path separators".into()));
    }
    let cleaned: Vec<&str> =
        path.split(['/', '\\']).filter(|p| !p.is_empty() && *p != ".").collect();
    if cleaned.contains(&"..") {
        return Err(Error::Validation("a path inside a snapshot cannot contain `..`".into()));
    }
    if cleaned.is_empty() {
        return Ok(snapshot.to_string());
    }
    Ok(format!("{snapshot}/{}", cleaned.join("/")))
}

/// Normalise the `path` echoed back in a listing reply.
pub fn normalised_path(path: &str) -> String {
    path.split(['/', '\\']).filter(|p| !p.is_empty() && *p != ".").collect::<Vec<_>>().join("/")
}

fn entry_kind(entry: &superbackup_core::kopia::DirEntry) -> EntryKind {
    use superbackup_core::kopia::EntryType;
    match entry.entry_type {
        EntryType::Directory => EntryKind::Directory,
        EntryType::Symlink => EntryKind::Symlink,
        _ => EntryKind::File,
    }
}

/// Turn a kopia manifest into the protocol's snapshot summary.
fn snapshot_info(
    manifest: &superbackup_core::kopia::SnapshotManifest,
    destination_id: Uuid,
) -> SnapshotInfo {
    let totals = manifest.totals();
    SnapshotInfo {
        id: manifest.id.clone(),
        destination_id,
        job_id: manifest.tag("superbackup-job").and_then(|v| Uuid::parse_str(v).ok()),
        created_at: manifest.start_time.unwrap_or_else(Utc::now),
        source_path: manifest.source.path.clone(),
        file_count: totals.map(|(files, _)| files),
        total_bytes: totals.map(|(_, bytes)| bytes),
        incomplete: !manifest.is_complete(),
        tags: manifest.tags.iter().map(|(k, v)| format!("{k}={v}")).collect(),
    }
}

/// Map the driver's own provenance type onto the wire one.
///
/// Two enums rather than one re-export, because the protocol deliberately
/// depends on nothing below the model layer; see [`KopiaProvenance`].
pub fn provenance_of(source: superbackup_core::kopia::KopiaSource) -> KopiaProvenance {
    use superbackup_core::kopia::KopiaSource;
    match source {
        KopiaSource::Configured => KopiaProvenance::Configured,
        KopiaSource::SystemPath => KopiaProvenance::SystemPath,
        KopiaSource::Bundled => KopiaProvenance::Bundled,
    }
}

/// The wire spelling of an update policy, matching its serde rename.
fn update_policy_wire(policy: superbackup_core::model::UpdatePolicy) -> &'static str {
    use superbackup_core::model::UpdatePolicy;
    match policy {
        UpdatePolicy::Off => "off",
        UpdatePolicy::Notify => "notify",
        UpdatePolicy::Automatic => "automatic",
    }
}

fn invocation(raw: superbackup_core::kopia::RawInvocation) -> KopiaInvocation {
    KopiaInvocation {
        label: raw.label,
        command_line: raw.command_line,
        secret_env: raw.secret_env,
        exit_code: raw.exit_code,
        stdout: raw.stdout,
        stderr: raw.stderr,
        duration_ms: raw.duration_ms,
        ok: raw.ok,
    }
}

/// Put every discovery route on the wire, marking the one that won.
///
/// The descriptions come from
/// [`superbackup_core::kopia::describe_routes`], which lives beside discovery
/// itself so the two cannot drift; this only translates them and adds the
/// "chosen" bit, which is the caller's knowledge rather than discovery's.
pub fn discovery_routes(
    settings: &Settings,
    paths: &superbackup_core::paths::Paths,
    chosen: KopiaProvenance,
) -> Vec<KopiaRoute> {
    let mut routes: Vec<KopiaRoute> = superbackup_core::kopia::describe_routes(settings, paths)
        .into_iter()
        .map(|r| {
            let provenance = r.source.map(provenance_of).unwrap_or(KopiaProvenance::None);
            KopiaRoute {
                provenance,
                path: r.path,
                outcome: r.outcome,
                chosen: provenance == chosen,
            }
        })
        .collect();
    if chosen == KopiaProvenance::None {
        routes.push(KopiaRoute {
            provenance: KopiaProvenance::None,
            path: None,
            outcome: "No route produced a usable kopia.".into(),
            chosen: true,
        });
    }
    routes
}

fn check(id: &str, title: &str, status: CheckStatus) -> DoctorCheck {
    DoctorCheck {
        id: id.to_string(),
        title: title.to_string(),
        status,
        detail: None,
        hint: None,
        fixable: false,
    }
}

// ---------------------------------------------------------------------------
// The Handler
// ---------------------------------------------------------------------------

impl Handler for DaemonHandler {
    // -- status -----------------------------------------------------------

    async fn ping(&self, _ctx: &RequestContext) -> Result<AckReply> {
        Ok(AckReply {})
    }

    async fn status(&self, _ctx: &RequestContext) -> Result<StatusReply> {
        Ok(StatusReply { snapshot: Box::new(self.runtime.current_snapshot().await) })
    }

    async fn version(&self, _ctx: &RequestContext) -> Result<VersionReply> {
        Ok(VersionReply {
            version: superbackup_core::VERSION.to_string(),
            protocol: superbackup_core::ipc::PROTOCOL_VERSION,
            min_protocol: superbackup_core::ipc::MIN_PROTOCOL_VERSION,
            target_os: std::env::consts::OS.to_string(),
            target_arch: std::env::consts::ARCH.to_string(),
            kopia_version: self.runtime.kopia_version(),
            service_scope: self.runtime.paths.service_scope,
            build: crate::build::short(),
        })
    }

    async fn health(&self, _ctx: &RequestContext) -> Result<HealthReply> {
        let snapshot = self.runtime.current_snapshot().await;
        let config = self.config().await;
        let (summary, reasons) = health_summary(&snapshot, &config);
        Ok(HealthReply { health: snapshot.health, summary, reasons })
    }

    async fn doctor(&self, _ctx: &RequestContext, fix: bool) -> Result<DoctorReply> {
        let mut checks = Vec::new();
        let mut fixed = Vec::new();

        // kopia
        match self.runtime.kopia() {
            Some(binary) => {
                let mut c = check("kopia.present", "kopia is installed", CheckStatus::Pass);
                c.detail = Some(format!("{} ({})", binary.version(), binary.source().title()));
                checks.push(c);
            }
            None => {
                let mut c = check("kopia.present", "kopia is installed", CheckStatus::Fail);
                c.hint = Some(format!(
                    "superbackup needs kopia {MINIMUM_KOPIA_VERSION} or newer. Open Settings → \
                     Kopia binary to install it."
                ));
                checks.push(c);
            }
        }

        // vault and configuration
        let store = self.runtime.store.lock().await;
        let locked = store.is_locked();
        let config = store.config().clone();
        let report = superbackup_core::config::validate(&config);
        drop(store);

        let mut vault = check(
            "vault.unlocked",
            "The vault is unlocked",
            if locked { CheckStatus::Warn } else { CheckStatus::Pass },
        );
        if locked {
            vault.hint = Some("Unlock superbackup so scheduled backups can run.".into());
        }
        checks.push(vault);

        let mut valid = check(
            "config.valid",
            "The configuration is valid",
            if report.is_ok() { CheckStatus::Pass } else { CheckStatus::Fail },
        );
        if !report.is_ok() {
            valid.detail =
                Some(report.errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; "));
        }
        checks.push(valid);

        // jobs with no destination at all — a job that can never run
        let orphaned: Vec<&str> = config
            .jobs
            .iter()
            .filter(|j| j.destination_ids.iter().all(|id| config.destination(id).is_none()))
            .map(|j| j.name.as_str())
            .collect();
        let mut wired = check(
            "job.destinations",
            "Every job has a destination",
            if orphaned.is_empty() { CheckStatus::Pass } else { CheckStatus::Fail },
        );
        if !orphaned.is_empty() {
            wired.detail = Some(format!("{} has no usable destination", orphaned.join(", ")));
            wired.hint = Some("Open the job and choose where it should back up to.".into());
        }
        checks.push(wired);

        // destinations, only when there is a vault to resolve them against
        if locked {
            checks.push(check(
                "dest.reachable",
                "Destinations are reachable",
                CheckStatus::Skipped,
            ));
        } else {
            for destination in &config.destinations {
                if !destination.enabled {
                    continue;
                }
                let id = format!("dest.reachable.{}", destination.id);
                match &destination.kind {
                    DestinationKind::LocalMirror { path }
                    | DestinationKind::LocalRepository { path }
                    | DestinationKind::OneDrive { path, .. } => {
                        let exists = path.exists();
                        let mut c = check(
                            &id,
                            &format!("\"{}\" is reachable", destination.name),
                            if exists { CheckStatus::Pass } else { CheckStatus::Fail },
                        );
                        if !exists {
                            c.hint = Some(
                                "The folder is missing. Reconnect the drive, or point the \
                                 destination somewhere else."
                                    .into(),
                            );
                        }
                        checks.push(c);
                    }
                    DestinationKind::S3 { .. } => {
                        // A network round trip per destination would make
                        // `doctor` take minutes on a laptop with a dozen
                        // buckets. `dest.test` is the deliberate, per
                        // destination version of this check.
                        checks.push(check(
                            &id,
                            &format!("\"{}\" is reachable", destination.name),
                            CheckStatus::Skipped,
                        ));
                    }
                }
            }
        }

        // service and autostart
        let service = self.runtime.service_status();
        let mut svc = check(
            "service.healthy",
            "The background service is healthy",
            match (service.installed, service.state) {
                (false, _) => CheckStatus::Skipped,
                (true, platform::ServiceState::Running) => CheckStatus::Pass,
                _ => CheckStatus::Fail,
            },
        );
        svc.detail = service.detail.clone();
        checks.push(svc);

        let autostart_spec = platform::autostart::AutostartSpec::current().ok();
        if let Some(spec) = &autostart_spec {
            let status = platform::autostart::status(spec).ok();
            let needs_repair = status.as_ref().map(|s| s.state.needs_repair()).unwrap_or(false);
            let mut c = check(
                "autostart.healthy",
                "Start at login points at this build",
                if needs_repair { CheckStatus::Warn } else { CheckStatus::Pass },
            );
            c.fixable = needs_repair;
            if needs_repair && fix {
                match platform::autostart::heal(spec) {
                    Ok(Some(event)) => {
                        self.runtime.record_event(event);
                        fixed.push("autostart.healthy".to_string());
                        c.status = CheckStatus::Pass;
                        c.fixable = false;
                    }
                    Ok(None) => {}
                    Err(e) => c.detail = Some(e.to_string()),
                }
            }
            checks.push(c);
        }

        // Warnings do not clear `ok`; only failures do.
        let ok = !checks.iter().any(|c| c.status == CheckStatus::Fail);
        Ok(DoctorReply { ok, checks, fixed })
    }

    /// Everything the Settings screen needs to let a user satisfy themselves
    /// that kopia works, including the raw output of running it.
    ///
    /// Discovery is re-run rather than served from the cached
    /// [`KopiaBinary`](superbackup_core::kopia::KopiaBinary) the daemon found
    /// at startup. A cached answer says what was true when the process
    /// started, possibly days ago and possibly before an antivirus quarantined
    /// the file; the whole point of this command is to answer "is it working
    /// *now*".
    async fn kopia_probe(
        &self,
        _ctx: &RequestContext,
        destination: Option<String>,
        check_for_update: bool,
    ) -> Result<KopiaProbeReply> {
        use superbackup_core::kopia::{configured_floor, KopiaBinary, KopiaInstaller};

        let config = self.config().await;
        let settings = config.settings.clone();
        let paths = &self.runtime.paths;
        let managed_path = paths.bundled_kopia();

        let resolved = KopiaBinary::discover(&settings, paths).await;
        let (path, provenance, version, banner, detail) = match &resolved {
            Ok(binary) => (
                Some(binary.path().display().to_string()),
                provenance_of(binary.source()),
                Some(binary.version().to_string()),
                Some(binary.banner().to_string()),
                None,
            ),
            Err(e) => (None, KopiaProvenance::None, None, None, Some(e.to_string())),
        };

        let mut invocations = Vec::new();
        let run_ctx = RunContext::new();
        if let Ok(binary) = &resolved {
            invocations.push(invocation(
                superbackup_core::kopia::version_invocation(binary.path(), &run_ctx).await,
            ));
        } else {
            invocations.push(invocation(superbackup_core::kopia::RawInvocation::not_attempted(
                "--version",
                detail.clone().unwrap_or_else(|| "no kopia executable was found".into()),
            )));
        }

        // The repository half, when a destination was named. It needs the
        // vault, so a locked one is reported as a not-attempted invocation
        // rather than failing the whole command — the version half is still
        // worth having.
        if let Some(needle) = &destination {
            invocations.push(invocation(self.status_invocation(needle, &run_ctx).await));
        }

        let installer = KopiaInstaller::new(paths).ok();
        let managed_version = match &installer {
            Some(i) => i.installed_version().await.map(|v| v.to_string()),
            None => None,
        };
        let (update_available, update_summary) = match (&installer, check_for_update) {
            (Some(i), true) => {
                let check = i.check_for_update(&settings, Utc::now()).await;
                (check.available_version().map(|v| v.to_string()), Some(check.summary()))
            }
            _ => (None, None),
        };

        Ok(KopiaProbeReply {
            path,
            provenance,
            version,
            banner,
            routes: discovery_routes(&settings, paths, provenance),
            managed_path: managed_path.display().to_string(),
            managed_version,
            update_policy: update_policy_wire(settings.kopia.auto_update).to_string(),
            update_available,
            update_summary,
            minimum_version: configured_floor(&settings).to_string(),
            invocations,
            detail,
        })
    }

    // -- jobs -------------------------------------------------------------

    async fn list_jobs(&self, _ctx: &RequestContext, include_disabled: bool) -> Result<JobsReply> {
        let config = self.config().await;
        let jobs = config.jobs.iter().filter(|j| include_disabled || j.enabled).cloned().collect();
        Ok(JobsReply { jobs })
    }

    async fn get_job(&self, _ctx: &RequestContext, job: String) -> Result<JobReply> {
        let config = self.config().await;
        Ok(JobReply { job: Box::new(resolve_job(&config, &job)?.clone()) })
    }

    async fn create_job(&self, _ctx: &RequestContext, job: Box<Job>) -> Result<JobReply> {
        let mut created = *job;
        // The daemon assigns the id, whatever the client sent: a client that
        // could choose ids could overwrite a job by guessing one.
        created.id = Uuid::new_v4();
        created.created_at = Utc::now();
        let name = created.name.clone();
        let id = created.id;
        let stored = created.clone();
        self.commit(move |config| {
            if config.jobs.iter().any(|j| j.name.eq_ignore_ascii_case(&stored.name)) {
                return Err(Error::Validation(format!(
                    "a job called \"{}\" already exists",
                    stored.name
                )));
            }
            config.jobs.push(stored);
            Ok(())
        })
        .await?;
        self.runtime.record_event(
            Event::info("job.created", format!("Job \"{name}\" was created.")).with_job(id),
        );
        let config = self.config().await;
        Ok(JobReply {
            job: Box::new(
                config.job(&id).cloned().ok_or_else(|| Error::JobNotFound(id.to_string()))?,
            ),
        })
    }

    async fn update_job(&self, _ctx: &RequestContext, job: Box<Job>) -> Result<JobReply> {
        let replacement = *job;
        let id = replacement.id;
        let name = replacement.name.clone();
        let stored = replacement.clone();
        self.commit(move |config| {
            let slot = config
                .jobs
                .iter_mut()
                .find(|j| j.id == stored.id)
                .ok_or_else(|| Error::JobNotFound(stored.id.to_string()))?;
            // `created_at` is history, not intent: a client that round-trips a
            // job must not be able to rewrite when it was made.
            let created_at = slot.created_at;
            *slot = stored;
            slot.created_at = created_at;
            Ok(())
        })
        .await?;
        self.runtime.record_event(
            Event::info("job.updated", format!("Job \"{name}\" was updated.")).with_job(id),
        );
        let config = self.config().await;
        Ok(JobReply {
            job: Box::new(
                config.job(&id).cloned().ok_or_else(|| Error::JobNotFound(id.to_string()))?,
            ),
        })
    }

    async fn delete_job(&self, _ctx: &RequestContext, job: String) -> Result<AckReply> {
        let config = self.config().await;
        let target = resolve_job(&config, &job)?.clone();
        // Stopping first, so a job cannot be deleted out from under a run that
        // is still writing to a repository.
        if let Some(scheduler) = self.runtime.scheduler() {
            let _ = scheduler.cancel_job(target.id);
        }
        let id = target.id;
        self.commit(move |config| {
            config.jobs.retain(|j| j.id != id);
            Ok(())
        })
        .await?;
        self.runtime.record_event(Event::info(
            "job.deleted",
            format!("Job \"{}\" was deleted. Its snapshots were kept.", target.name),
        ));
        Ok(AckReply {})
    }

    async fn set_job_enabled(
        &self,
        _ctx: &RequestContext,
        job: String,
        enabled: bool,
    ) -> Result<JobReply> {
        let config = self.config().await;
        let id = resolve_job(&config, &job)?.id;
        self.commit(move |config| {
            let slot = config.job_mut(&id).ok_or_else(|| Error::JobNotFound(id.to_string()))?;
            slot.enabled = enabled;
            Ok(())
        })
        .await?;
        let config = self.config().await;
        let updated = config.job(&id).cloned().ok_or_else(|| Error::JobNotFound(id.to_string()))?;
        self.runtime.record_event(
            Event::info(
                "job.enabled",
                format!(
                    "Job \"{}\" was {}.",
                    updated.name,
                    if enabled { "enabled" } else { "disabled" }
                ),
            )
            .with_job(id),
        );
        Ok(JobReply { job: Box::new(updated) })
    }

    async fn run_job(
        &self,
        _ctx: &RequestContext,
        job: String,
        dry_run: bool,
    ) -> Result<StartedReply> {
        let config = self.config().await;
        let target = resolve_job(&config, &job)?.clone();
        if self.runtime.store.lock().await.is_locked() {
            return Err(Error::Locked);
        }

        if dry_run {
            return super::dryrun::start(&self.runtime, target).await;
        }

        let scheduler = self.runtime.require_scheduler()?;
        let run_id = scheduler.run_now(target.id, Trigger::Manual).await?;

        // A queued-but-not-started run is a normal answer, not a failure: it
        // means `max_parallel_jobs` is already satisfied.
        let status = scheduler.status().await.ok();
        let started =
            status.as_ref().map(|s| s.running.values().any(|r| *r == run_id)).unwrap_or(true);
        let note = (!started).then(|| {
            "Queued behind another backup; it will start when a slot frees up.".to_string()
        });
        Ok(StartedReply { run_id, started, note })
    }

    async fn stop_run(&self, _ctx: &RequestContext, run_id: Uuid) -> Result<StoppedReply> {
        // Idempotent by contract: stopping a run that already finished is a
        // success with an empty list, not an error.
        let Some(job_id) = self.runtime.job_for_run(&run_id) else {
            return Ok(StoppedReply { stopped: vec![] });
        };
        let scheduler = self.runtime.require_scheduler()?;
        scheduler.cancel_job(job_id)?;
        Ok(StoppedReply { stopped: vec![run_id] })
    }

    async fn stop_all_runs(&self, _ctx: &RequestContext) -> Result<StoppedReply> {
        let runs = self.runtime.active_runs();
        let scheduler = self.runtime.require_scheduler()?;
        let mut stopped = Vec::new();
        for run in runs {
            if scheduler.cancel_job(run.job_id).is_ok() {
                stopped.push(run.run_id);
            }
        }
        Ok(StoppedReply { stopped })
    }

    async fn job_history(
        &self,
        _ctx: &RequestContext,
        job: Option<String>,
        limit: u32,
    ) -> Result<RunsReply> {
        let filter = match &job {
            Some(needle) => {
                let config = self.config().await;
                Some(resolve_job(&config, needle)?.id)
            }
            None => None,
        };
        // 0 means "the daemon's own cap"; anything larger is clamped rather
        // than refused, so a client asking for a million gets two hundred.
        let cap = if limit == 0 {
            superbackup_core::state::MAX_HISTORY
        } else {
            (limit as usize).min(superbackup_core::state::MAX_HISTORY)
        };
        let persisted = self.runtime.persisted.lock().await;
        let runs = persisted
            .history
            .iter()
            .filter(|r| filter.is_none_or(|id| r.job_id == id))
            .take(cap)
            .cloned()
            .collect();
        Ok(RunsReply { runs })
    }

    // -- destinations -----------------------------------------------------

    async fn list_destinations(&self, _ctx: &RequestContext) -> Result<DestinationsReply> {
        Ok(DestinationsReply { destinations: self.config().await.destinations })
    }

    async fn get_destination(
        &self,
        _ctx: &RequestContext,
        destination: String,
    ) -> Result<DestinationReply> {
        let config = self.config().await;
        Ok(DestinationReply {
            destination: Box::new(resolve_destination(&config, &destination)?.clone()),
        })
    }

    async fn create_destination(
        &self,
        _ctx: &RequestContext,
        destination: Box<Destination>,
    ) -> Result<DestinationReply> {
        let mut created = *destination;
        created.id = Uuid::new_v4();
        created.created_at = Utc::now();
        // The passphrase, and only then the handle that names it.
        //
        // This used to mint the handle on its own, with a comment explaining
        // that a generated passphrase "needs a handle before anything can
        // store one against it" — and nothing ever did. Every repository
        // destination was therefore written with a `passphrase_ref` pointing
        // at a vault entry that did not exist, and every operation needing it
        // failed with "the vault has no entry for repo-passphrase:…": creating
        // the repository, verifying it, restoring from it. A reference that
        // resolves to nothing is worse than no reference, because the rest of
        // the application reads it as "this is set up".
        //
        // So the secret is generated and stored first, and the handle is
        // written only once it points at something real.
        if created.kind.is_repository() && created.passphrase_ref.is_none() {
            let source = created
                .encryption
                .as_ref()
                .map(|e| e.passphrase_source)
                .unwrap_or(superbackup_core::model::PassphraseSource::Generated);
            match source {
                // Derived from the master key on demand; there is nothing to
                // store and nothing to point at.
                superbackup_core::model::PassphraseSource::DerivedFromMaster => {}
                superbackup_core::model::PassphraseSource::Generated => {
                    self.require_unlocked().await?;
                    let passphrase = superbackup_core::crypto::generate_passphrase()?;
                    let handle = SecretRef::new("repo-passphrase", &created.id);
                    let mut store = self.runtime.store.lock().await;
                    store.put_secret(handle.clone(), passphrase)?;
                    drop(store);
                    created.passphrase_ref = Some(handle);
                }
                // The user supplies this one. No handle is written until they
                // have, because a handle is what tells the rest of the
                // application the passphrase exists.
                superbackup_core::model::PassphraseSource::UserSupplied => {}
            }
        }
        let id = created.id;
        let name = created.name.clone();
        let stored = created.clone();
        self.commit(move |config| {
            if config.destinations.iter().any(|d| d.name.eq_ignore_ascii_case(&stored.name)) {
                return Err(Error::Validation(format!(
                    "a destination called \"{}\" already exists",
                    stored.name
                )));
            }
            config.destinations.push(stored);
            Ok(())
        })
        .await?;
        self.runtime.record_event(
            Event::info("dest.created", format!("Destination \"{name}\" was added."))
                .with_destination(id),
        );
        let config = self.config().await;
        Ok(DestinationReply {
            destination: Box::new(config.destination(&id).cloned().ok_or_else(|| {
                Error::Validation(format!("destination {id} vanished during creation"))
            })?),
        })
    }

    async fn update_destination(
        &self,
        _ctx: &RequestContext,
        destination: Box<Destination>,
    ) -> Result<DestinationReply> {
        let replacement = *destination;
        let id = replacement.id;
        let name = replacement.name.clone();
        let stored = replacement.clone();
        self.commit(move |config| {
            let slot =
                config.destinations.iter_mut().find(|d| d.id == stored.id).ok_or_else(|| {
                    Error::Validation(format!("no destination with id {}", stored.id))
                })?;
            let created_at = slot.created_at;
            // The passphrase handle is not the client's to change: repointing
            // it at another destination's secret would open the wrong
            // repository, or silently orphan this one's key.
            let passphrase_ref = slot.passphrase_ref.clone();
            *slot = stored;
            slot.created_at = created_at;
            slot.passphrase_ref = passphrase_ref;
            Ok(())
        })
        .await?;
        // Configuration changes belong in Activity too. Only backup runs were
        // recorded, so the log could not answer "when did this destination
        // change?" — which is the question asked right after a backup starts
        // going somewhere unexpected.
        self.runtime.record_event(
            Event::info("dest.updated", format!("Destination \"{name}\" was changed."))
                .with_destination(id),
        );
        let config = self.config().await;
        Ok(DestinationReply {
            destination: Box::new(config.destination(&id).cloned().ok_or_else(|| {
                Error::Validation(format!("destination {id} vanished during update"))
            })?),
        })
    }

    async fn delete_destination(
        &self,
        _ctx: &RequestContext,
        destination: String,
        force: bool,
    ) -> Result<AckReply> {
        let config = self.config().await;
        let target = resolve_destination(&config, &destination)?.clone();
        let users: Vec<String> =
            config.jobs_using(&target.id).iter().map(|j| j.name.clone()).collect();
        if !users.is_empty() && !force {
            return Err(Error::Validation(format!(
                "\"{}\" is still used by {} ({}). Delete it with force to remove it from those \
                 jobs.",
                target.name,
                users.len(),
                users.join(", ")
            )));
        }
        let id = target.id;
        self.commit(move |config| {
            config.destinations.retain(|d| d.id != id);
            for job in &mut config.jobs {
                job.destination_ids.retain(|d| *d != id);
            }
            Ok(())
        })
        .await?;
        self.runtime.record_event(Event::warn(
            "dest.deleted",
            format!(
                "Destination \"{}\" was removed from the configuration. Its stored data was not \
                 touched.",
                target.name
            ),
        ));
        Ok(AckReply {})
    }

    /// Answer "can I reach this place with these credentials?" — and report
    /// "is there a repository here?" as a separate fact.
    ///
    /// These used to be one answer, and the coupling was backwards. The old
    /// path built a [`superbackup_core::kopia::KopiaDriver`] and asked kopia to
    /// open the repository, which needs the repository encryption key. A
    /// destination that has been added but not yet created has no key, so the
    /// test could not run at all — and a perfectly reachable bucket with
    /// correct credentials reported as *unreachable*, which sends the user to
    /// debug the one thing that was never wrong.
    ///
    /// So reachability is now established without any key at all:
    ///
    /// * S3 — one `ListObjectsV2`, which proves the endpoint resolves, TLS
    ///   succeeds, the credentials are accepted and the bucket exists, plus a
    ///   bounded write probe, because a bucket that cannot be written to fails
    ///   every backup.
    /// * local, OneDrive — the directory probe: exists, and a file can be
    ///   written and removed.
    ///
    /// Whether a repository is present is then answered by *looking for*
    /// kopia's `kopia.repository` format blob, never by opening it. Opening it
    /// is `dest.check_key`'s job and needs the key this deliberately does not
    /// touch.
    async fn test_destination(
        &self,
        _ctx: &RequestContext,
        destination: String,
    ) -> Result<ProbeReply> {
        self.require_unlocked().await?;
        let config = self.config().await;
        let target = resolve_destination(&config, &destination)?.clone();
        let started = std::time::Instant::now();
        let elapsed = || started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;

        if let DestinationKind::S3 { bucket, prefix, .. } = &target.kind {
            let reply = self.probe_s3_destination(&target, bucket, prefix).await;
            return Ok(ProbeReply { latency_ms: Some(elapsed()), ..reply });
        }

        let Some(path) = target.kind.local_path().cloned() else {
            return Err(Error::Internal(format!("\"{}\" has no location to test", target.name)));
        };
        let (reachable, writable, problem) = probe_directory(&path).await;

        // A folder mirror is a plain copy and holds no repository, so the
        // question does not apply to it rather than being answered "no".
        let repository_present = if target.kind.is_repository() && reachable {
            Some(
                tokio::task::spawn_blocking(move || {
                    path.join(KOPIA_FORMAT_BLOB_FS).is_file()
                        || path.join(KOPIA_FORMAT_BLOB).is_file()
                })
                .await
                .unwrap_or(false),
            )
        } else {
            None
        };

        let detail =
            match (reachable, writable, repository_present) {
                (true, true, Some(false)) => Some(copy_no_repository_yet(&target.kind)),
                (true, true, _) => None,
                _ => Some(problem.unwrap_or_else(|| {
                    "The location could not be reached or written to.".to_string()
                })),
            };
        Ok(ProbeReply {
            reachable,
            writable,
            latency_ms: Some(elapsed()),
            repository_present,
            detail,
        })
    }

    async fn check_encryption_key(
        &self,
        _ctx: &RequestContext,
        destination: String,
        key: Option<SecretString>,
    ) -> Result<KeyCheckReply> {
        self.require_unlocked().await?;
        let config = self.config().await;
        let target = resolve_destination(&config, &destination)?.clone();
        if !target.kind.is_repository() {
            return Err(Error::Validation(format!(
                "\"{}\" is a folder mirror. It holds plain copies, has no encryption key, and \
                 there is nothing to check.",
                target.name
            )));
        }

        let binary = self.runtime.kopia().ok_or(Error::KopiaMissing)?;
        let candidate = key.map(|k| k.into_secret());
        if candidate.as_ref().is_some_and(|c| c.is_empty()) {
            return Err(Error::Validation("an empty key cannot open a repository".into()));
        }
        let driver = {
            let store = self.runtime.store.lock().await;
            super::executor::build_driver_with(
                &store,
                &self.runtime.paths,
                binary,
                &target,
                candidate,
            )?
        };

        let ctx = RunContext::new();
        let outcome = driver.test_connection(&ctx).await;
        let reply = match outcome {
            Ok(superbackup_core::kopia::ConnectionTest::Connected { status }) => KeyCheckReply {
                destination_id: target.id,
                valid: true,
                no_repository: false,
                repository_id: status.and_then(|s| s.unique_id),
                detail: None,
            },
            // Reachable, but empty. The key is neither right nor wrong, and
            // saying "wrong key" here would send a user hunting for a problem
            // they do not have.
            Ok(superbackup_core::kopia::ConnectionTest::ReachableButEmpty) => KeyCheckReply {
                destination_id: target.id,
                valid: false,
                no_repository: true,
                repository_id: None,
                detail: Some(format!(
                    "The location for \"{}\" is reachable but holds no repository yet, so there \
                     is nothing to check the key against.",
                    target.name
                )),
            },
            Err(e) => KeyCheckReply {
                destination_id: target.id,
                valid: false,
                no_repository: false,
                repository_id: None,
                detail: Some(e.message.clone()),
            },
        };

        // The outcome is worth recording either way: a key that stopped
        // working is the single most important thing that can happen to a
        // backup, and a check that says so should leave a trace.
        self.runtime.record_event(
            Event::new(
                if reply.valid { Severity::Info } else { Severity::Warning },
                "dest.key_checked",
                if reply.valid {
                    format!("The encryption key for \"{}\" opened the repository.", target.name)
                } else if reply.no_repository {
                    format!("\"{}\" holds no repository to check a key against.", target.name)
                } else {
                    format!(
                        "The encryption key offered for \"{}\" did not open the repository.",
                        target.name
                    )
                },
            )
            .with_destination(target.id),
        );
        Ok(reply)
    }

    async fn create_repository(
        &self,
        _ctx: &RequestContext,
        destination: String,
        encryption: Option<EncryptionSettings>,
    ) -> Result<RepositoryReply> {
        self.require_unlocked().await?;

        // Encryption settings belong to the destination, and the driver reads
        // them from it, so an override is stored first and then used.
        if let Some(settings) = encryption {
            let config = self.config().await;
            let id = resolve_destination(&config, &destination)?.id;
            self.commit(move |config| {
                let slot = config
                    .destinations
                    .iter_mut()
                    .find(|d| d.id == id)
                    .ok_or_else(|| Error::Validation(format!("no destination with id {id}")))?;
                slot.encryption = Some(settings);
                Ok(())
            })
            .await?;
        }

        // A repository with a generated passphrase needs one *before* the
        // repository exists, and it must be written to the vault first: a
        // repository whose password is not in the vault is unopenable.
        self.ensure_repository_passphrase(&destination).await?;

        let (target, driver) = self.driver_for(&destination).await?;
        let ctx = RunContext::new();
        let created = match driver.connect_repository(&ctx).await {
            Ok(()) => false,
            Err(e) if e.failure == superbackup_core::kopia::KopiaFailure::RepositoryNotFound => {
                driver.create_repository(&ctx).await.map_err(kopia_to_error)?;
                driver.connect_repository(&ctx).await.map_err(kopia_to_error)?;
                true
            }
            Err(e) => return Err(kopia_to_error(e)),
        };
        let status = driver.repository_status(&ctx).await.ok();
        self.runtime.record_event(
            Event::info(
                if created { "repo.created" } else { "repo.connected" },
                format!(
                    "{} the repository at \"{}\".",
                    if created { "Created" } else { "Connected to" },
                    target.name
                ),
            )
            .with_destination(target.id),
        );
        self.runtime.publish_status().await;
        Ok(RepositoryReply {
            destination_id: target.id,
            connected: true,
            repository_id: status.and_then(|s| s.unique_id),
            created,
        })
    }

    async fn connect_repository(
        &self,
        _ctx: &RequestContext,
        destination: String,
    ) -> Result<RepositoryReply> {
        self.require_unlocked().await?;
        let (target, driver) = self.driver_for(&destination).await?;
        let ctx = RunContext::new();
        driver.connect_repository(&ctx).await.map_err(kopia_to_error)?;
        let status = driver.repository_status(&ctx).await.ok();
        self.runtime.record_event(
            Event::info(
                "repo.connected",
                format!("Connected to the repository at \"{}\".", target.name),
            )
            .with_destination(target.id),
        );
        Ok(RepositoryReply {
            destination_id: target.id,
            connected: true,
            repository_id: status.and_then(|s| s.unique_id),
            created: false,
        })
    }

    async fn disconnect_repository(
        &self,
        _ctx: &RequestContext,
        destination: String,
    ) -> Result<RepositoryReply> {
        let config = self.config().await;
        let target = resolve_destination(&config, &destination)?.clone();
        // Disconnecting must work with a locked vault: it is how a user gets
        // out of trouble, and it needs no secret. Best effort against kopia,
        // then remove the per-destination config file either way.
        if !self.runtime.store.lock().await.is_locked() {
            if let Ok((_, driver)) = self.driver_for(&destination).await {
                let _ = driver.disconnect_repository(&RunContext::new()).await;
            }
        }
        let config_file = self.runtime.paths.kopia_config_for(&target.id);
        if config_file.exists() {
            if let Err(e) = std::fs::remove_file(&config_file) {
                tracing::warn!(error = %e, "could not remove the kopia config file");
            }
        }
        self.runtime.record_event(
            Event::info(
                "repo.disconnected",
                format!("Disconnected from the repository at \"{}\".", target.name),
            )
            .with_destination(target.id),
        );
        Ok(RepositoryReply {
            destination_id: target.id,
            connected: false,
            repository_id: None,
            created: false,
        })
    }

    async fn destination_stats(
        &self,
        _ctx: &RequestContext,
        destination: String,
        refresh: bool,
    ) -> Result<StorageStatsReply> {
        self.require_unlocked().await?;
        let config = self.config().await;
        let target = resolve_destination(&config, &destination)?.clone();

        if !refresh {
            if let Some(cached) = self.runtime.cached_stats(&target.id) {
                return Ok(cached);
            }
        }

        // A folder mirror has no repository to interrogate; the honest answer
        // is the size on disk, which nothing here is willing to walk. Report
        // what is known and no more.
        if !target.kind.is_repository() {
            let reply = StorageStatsReply {
                destination_id: target.id,
                snapshot_count: 0,
                logical_bytes: None,
                stored_bytes: None,
                last_snapshot_at: None,
                computed_at: Utc::now(),
            };
            self.runtime.cache_stats(reply.clone());
            return Ok(reply);
        }

        let (_, driver) = self.driver_for(&destination).await?;
        let ctx = RunContext::new();
        let snapshots = driver.list_snapshots(None, true, &ctx).await.map_err(kopia_to_error)?;
        let blobs = driver.blob_stats(&ctx).await.ok();
        let logical: u64 =
            snapshots.iter().filter_map(|s| s.totals().map(|(_, bytes)| bytes)).sum();
        let reply = StorageStatsReply {
            destination_id: target.id,
            snapshot_count: snapshots.len() as u64,
            logical_bytes: (logical > 0).then_some(logical),
            stored_bytes: blobs.map(|b| b.total_bytes),
            last_snapshot_at: snapshots.iter().filter_map(|s| s.start_time).max(),
            computed_at: Utc::now(),
        };
        self.runtime.cache_stats(reply.clone());
        Ok(reply)
    }

    // -- providers --------------------------------------------------------

    async fn list_providers(&self, _ctx: &RequestContext) -> Result<ProvidersReply> {
        Ok(ProvidersReply { providers: self.config().await.providers })
    }

    async fn get_provider(&self, _ctx: &RequestContext, provider: String) -> Result<ProviderReply> {
        let config = self.config().await;
        Ok(ProviderReply { provider: Box::new(resolve_provider(&config, &provider)?.clone()) })
    }

    async fn create_provider(
        &self,
        _ctx: &RequestContext,
        provider: Box<StorageProvider>,
    ) -> Result<ProviderReply> {
        let mut created = *provider;
        created.id = Uuid::new_v4();
        created.created_at = Utc::now();
        let name = created.name.clone();
        // Credential handles name the provider that owns them, so they can
        // only be minted once the id is known. `ProviderKind` has one variant
        // today; the match keeps this correct when a second one arrives.
        match &mut created.kind {
            ProviderKind::S3 { credentials, .. } => {
                *credentials = superbackup_core::model::S3Credentials::for_provider(&created.id);
            }
        }
        let id = created.id;
        let stored = created.clone();
        self.commit(move |config| {
            if config.providers.iter().any(|p| p.name.eq_ignore_ascii_case(&stored.name)) {
                return Err(Error::Validation(format!(
                    "a provider called \"{}\" already exists",
                    stored.name
                )));
            }
            config.providers.push(stored);
            Ok(())
        })
        .await?;
        self.runtime.record_event(Event::info(
            "provider.created",
            format!("Storage provider \"{name}\" was added."),
        ));
        let config = self.config().await;
        Ok(ProviderReply {
            provider: Box::new(config.provider(&id).cloned().ok_or_else(|| {
                Error::Validation(format!("provider {id} vanished during creation"))
            })?),
        })
    }

    async fn update_provider(
        &self,
        _ctx: &RequestContext,
        provider: Box<StorageProvider>,
    ) -> Result<ProviderReply> {
        let replacement = *provider;
        let id = replacement.id;
        let name = replacement.name.clone();
        let stored = replacement.clone();
        self.commit(move |config| {
            let slot =
                config.providers.iter_mut().find(|p| p.id == stored.id).ok_or_else(|| {
                    Error::Validation(format!("no provider with id {}", stored.id))
                })?;
            let created_at = slot.created_at;
            // Credential handles are the daemon's, not the client's — see
            // `update_destination` for the same reasoning.
            let ProviderKind::S3 { credentials, .. } = &slot.kind;
            let existing = credentials.clone();
            *slot = stored;
            slot.created_at = created_at;
            let ProviderKind::S3 { credentials, .. } = &mut slot.kind;
            *credentials = existing;
            Ok(())
        })
        .await?;
        self.runtime.record_event(Event::info(
            "provider.updated",
            format!("Storage provider \"{name}\" was changed."),
        ));
        let config = self.config().await;
        Ok(ProviderReply {
            provider: Box::new(config.provider(&id).cloned().ok_or_else(|| {
                Error::Validation(format!("provider {id} vanished during update"))
            })?),
        })
    }

    async fn delete_provider(
        &self,
        _ctx: &RequestContext,
        provider: String,
        force: bool,
    ) -> Result<AckReply> {
        let config = self.config().await;
        let target = resolve_provider(&config, &provider)?.clone();
        let users = config.destinations_using(&target.id);
        if !users.is_empty() && !force {
            return Err(Error::Validation(format!(
                "\"{}\" is still used by {} destination(s). Delete it with force to remove it \
                 anyway.",
                target.name,
                users.len()
            )));
        }
        let id = target.id;
        let handles: Vec<SecretRef> = target.secret_refs().into_iter().cloned().collect();
        self.commit(move |config| {
            config.providers.retain(|p| p.id != id);
            Ok(())
        })
        .await?;
        // Forget the credentials, but only when the vault is open: silently
        // skipping is better than refusing the delete, and the orphan
        // collector will catch them later.
        let mut store = self.runtime.store.lock().await;
        if !store.is_locked() {
            for handle in handles {
                if let Err(e) = store.vault_file_mut().vault_mut().remove(&handle) {
                    tracing::warn!(error = %e, "could not remove a provider credential");
                }
            }
            if let Err(e) = store.vault_file_mut().save() {
                tracing::warn!(error = %e, "could not save the vault after deleting a provider");
            }
        }
        drop(store);
        self.runtime.record_event(Event::info(
            "provider.deleted",
            format!("Storage provider \"{}\" was deleted.", target.name),
        ));
        Ok(AckReply {})
    }

    /// Test a provider by signing a real `ListBuckets` against it.
    ///
    /// This used to borrow the first destination that used the provider and
    /// ask kopia to open a repository there, because kopia is the only thing
    /// that could talk to S3. That made the answer useless exactly when it was
    /// wanted: before any destination exists — which is when someone has just
    /// pasted a key pair and wants to know whether it is right — it could only
    /// say "there is nothing to test against".
    ///
    /// `ListBuckets` is authenticated, so a success proves the endpoint
    /// resolves, TLS works, the clock is close enough, and both halves of the
    /// key pair are correct. It writes nothing, which is why `writable` is
    /// false rather than optimistic: proving a write needs a destination and a
    /// repository, and claiming otherwise here would be a lie the user only
    /// discovers during a backup.
    async fn test_provider(&self, _ctx: &RequestContext, provider: String) -> Result<ProbeReply> {
        self.require_unlocked().await?;
        let config = self.config().await;
        let target = resolve_provider(&config, &provider)?.clone();
        let probe = self.probe_provider(&target).await;

        // A key that authenticates and then is not allowed to list buckets is
        // a *working* key — scoping a key to one bucket is the recommended
        // shape, and `s3:ListAllMyBuckets` is exactly what such a key lacks.
        // Reporting that as unreachable would send the user to regenerate
        // credentials that were never wrong.
        let reachable = probe.listed || probe.credentials_ok;
        let detail =
            probe.detail.clone().or_else(|| Some(copy_buckets_visible(probe.buckets.len())));
        if reachable {
            self.mark_provider_verified(target.id).await;
        }
        Ok(ProbeReply {
            reachable,
            // A provider is an account, not a place. Nothing was written to,
            // and nothing here may claim otherwise.
            writable: false,
            latency_ms: probe.latency_ms,
            // Whether some bucket contains a repository is not a property of
            // an account, so the question does not apply.
            repository_present: None,
            detail,
        })
    }

    async fn list_buckets(&self, _ctx: &RequestContext, provider: String) -> Result<BucketsReply> {
        self.require_unlocked().await?;
        let config = self.config().await;
        let target = resolve_provider(&config, &provider)?.clone();
        let probe = self.probe_provider(&target).await;
        if probe.listed || probe.credentials_ok {
            self.mark_provider_verified(target.id).await;
        }
        Ok(BucketsReply {
            provider_id: target.id,
            buckets: probe.buckets,
            listed: probe.listed,
            credentials_ok: probe.credentials_ok,
            detail: probe.detail,
            latency_ms: probe.latency_ms,
        })
    }

    async fn list_objects(
        &self,
        _ctx: &RequestContext,
        provider: String,
        bucket: String,
        prefix: String,
        max_keys: u32,
    ) -> Result<ObjectsReply> {
        self.require_unlocked().await?;
        let config = self.config().await;
        let target = resolve_provider(&config, &provider)?.clone();
        let keys = self.provider_keys(&target).await?;
        let client =
            superbackup_core::s3::S3Client::new().map_err(superbackup_core::Error::from)?;

        // A failed listing is an answer, not an error: "I could not look" must
        // never be the thing that stops a destination being created.
        let unavailable = |detail: String| ObjectsReply {
            bucket: bucket.clone(),
            prefix: prefix.clone(),
            keys: Vec::new(),
            truncated: false,
            holds_repository: false,
            listed: false,
            detail: Some(detail),
        };
        match client.list_objects_v2(&target, &keys, &bucket, &prefix, max_keys).await {
            Ok(listing) => {
                // `holds_repository` is answered by an exact lookup rather
                // than by scanning the page above, because the page is capped:
                // a prefix with more objects than `max_keys` could hold a
                // repository the caller never saw, and answering "no" to
                // "would creating one here collide?" is the wrong way round to
                // be wrong.
                let format_blob = format!("{prefix}{KOPIA_FORMAT_BLOB}");
                let holds_repository = listing.holds_kopia_repository()
                    || client
                        .object_exists(&target, &keys, &bucket, &format_blob)
                        .await
                        .unwrap_or(false);
                Ok(ObjectsReply {
                    bucket,
                    prefix,
                    holds_repository,
                    truncated: listing.truncated,
                    keys: listing
                        .keys
                        .into_iter()
                        .map(|o| ObjectInfo {
                            key: o.key,
                            size: o.size,
                            last_modified: o.last_modified,
                        })
                        .collect(),
                    listed: true,
                    detail: None,
                })
            }
            Err(e) => Ok(unavailable(match e.hint() {
                Some(hint) => format!("{} {hint}", e.message()),
                None => e.message(),
            })),
        }
    }

    async fn provider_used_by(
        &self,
        _ctx: &RequestContext,
        provider: String,
    ) -> Result<UsedByReply> {
        let config = self.config().await;
        let target = resolve_provider(&config, &provider)?;
        let destinations: Vec<Reference> = config
            .destinations_using(&target.id)
            .iter()
            .map(|d| Reference { id: d.id, name: d.name.clone() })
            .collect();
        let destination_ids: BTreeSet<Uuid> = destinations.iter().map(|d| d.id).collect();
        let jobs = config
            .jobs
            .iter()
            .filter(|j| j.destination_ids.iter().any(|id| destination_ids.contains(id)))
            .map(|j| Reference { id: j.id, name: j.name.clone() })
            .collect();
        Ok(UsedByReply { destinations, jobs })
    }

    async fn rotate_provider_credentials(
        &self,
        _ctx: &RequestContext,
        provider: String,
        access_key_id: SecretString,
        secret_access_key: SecretString,
        session_token: Option<SecretString>,
    ) -> Result<ProviderReply> {
        self.require_unlocked().await?;
        if access_key_id.expose().is_empty() || secret_access_key.expose().is_empty() {
            return Err(Error::Validation("an access key and secret are both required".into()));
        }
        let config = self.config().await;
        let target = resolve_provider(&config, &provider)?.clone();
        // `ProviderKind` has one variant today, so this is an irrefutable
        // binding rather than a match; a second variant would make it a
        // compile error here, which is where the decision belongs.
        let ProviderKind::S3 { credentials, .. } = &target.kind;

        let mut store = self.runtime.store.lock().await;
        store.put_secret(credentials.access_key_ref.clone(), access_key_id.into_secret())?;
        store.put_secret(credentials.secret_key_ref.clone(), secret_access_key.into_secret())?;
        match (&credentials.session_token_ref, session_token) {
            (Some(handle), Some(token)) => {
                store.put_secret(handle.clone(), token.into_secret())?;
            }
            (Some(handle), None) => {
                // Rotating to a long-lived key pair: the stale session token
                // must go, or kopia will present an expired one.
                let handle = handle.clone();
                if let Err(e) = store.vault_file_mut().vault_mut().remove(&handle) {
                    tracing::warn!(error = %e, "could not clear the old session token");
                }
                store.vault_file_mut().save()?;
            }
            _ => {}
        }
        drop(store);

        let id = target.id;
        self.commit(move |config| {
            if let Some(slot) = config.providers.iter_mut().find(|p| p.id == id) {
                slot.last_verified_at = None;
            }
            Ok(())
        })
        .await?;
        self.runtime.record_event(Event::info(
            "provider.rotated",
            format!("The credentials for \"{}\" were replaced.", target.name),
        ));
        let config = self.config().await;
        Ok(ProviderReply {
            provider: Box::new(config.provider(&id).cloned().ok_or_else(|| {
                Error::Validation(format!("provider {id} vanished during rotation"))
            })?),
        })
    }

    // -- snapshots and restore -------------------------------------------

    async fn list_snapshots(
        &self,
        _ctx: &RequestContext,
        destination: String,
        job: Option<String>,
        limit: u32,
    ) -> Result<SnapshotsReply> {
        self.require_unlocked().await?;
        let config = self.config().await;
        let job_filter = match &job {
            Some(needle) => Some(resolve_job(&config, needle)?.id),
            None => None,
        };
        let (target, driver) = self.driver_for(&destination).await?;
        let manifests = driver.browse_roots(&RunContext::new()).await.map_err(kopia_to_error)?;
        let cap = if limit == 0 { MAX_SNAPSHOTS } else { (limit as usize).min(MAX_SNAPSHOTS) };
        let snapshots = manifests
            .iter()
            .map(|m| snapshot_info(m, target.id))
            .filter(|s| job_filter.is_none_or(|id| s.job_id == Some(id)))
            .take(cap)
            .collect();
        Ok(SnapshotsReply { snapshots })
    }

    async fn browse_snapshot(
        &self,
        _ctx: &RequestContext,
        destination: String,
        snapshot: String,
        path: String,
    ) -> Result<ListingReply> {
        self.require_unlocked().await?;
        // The path is checked first, before anything is opened. A `..` is
        // refused on its own terms rather than depending on whether the
        // destination resolves or the snapshot exists — otherwise a traversal
        // attempt gets whatever error those happen to produce instead.
        browse_target(&snapshot, &path)?;
        let (_, driver) = self.driver_for(&destination).await?;
        // Browsing addresses a kopia **object**, not a snapshot manifest.
        //
        // `snapshot.list` reports a manifest id, and that is what every client
        // holds and passes back — but `kopia show` wants the object id of the
        // snapshot's root directory, and rejects a manifest id outright:
        // `invalid content ID: "3e0f…" (17 vs 33)`. So the restore browser has
        // never listed anything on any platform.
        //
        // Resolving here rather than changing the wire format keeps clients
        // holding the one identifier they already know, and means a caller
        // cannot get this wrong again by passing the obvious thing.
        let root = self.snapshot_root_object(&driver, &snapshot).await?;
        let target = browse_target(&root, &path)?;
        let entries =
            driver.list_directory(&target, &RunContext::new()).await.map_err(kopia_to_error)?;
        let truncated = entries.len() > MAX_LISTING_ENTRIES;
        Ok(ListingReply {
            path: normalised_path(&path),
            entries: entries
                .iter()
                .take(MAX_LISTING_ENTRIES)
                .map(|e| SnapshotEntry {
                    name: e.name.clone(),
                    kind: entry_kind(e),
                    size_bytes: e.size,
                    modified_at: e.modified_at,
                    object_id: (!e.object_id.is_empty()).then(|| e.object_id.clone()),
                })
                .collect(),
            truncated,
        })
    }

    async fn restore_snapshot(
        &self,
        _ctx: &RequestContext,
        destination: String,
        snapshot: String,
        path: String,
        target: PathBuf,
        conflict: ConflictPolicy,
        dry_run: bool,
    ) -> Result<StartedReply> {
        self.require_unlocked().await?;
        if !target.is_absolute() {
            return Err(Error::Validation("the restore target must be an absolute path".into()));
        }
        let options = restore_options(conflict)?;
        let (dest, driver) = self.driver_for(&destination).await?;
        // The same resolution browsing uses. `kopia restore` happens to accept
        // a manifest id as well, but a *path inside* one has to be addressed
        // from the root object — and having restore and browse disagree about
        // what a snapshot id means is how the two drift apart.
        // As in `browse_snapshot`: the path is checked before the snapshot is
        // looked up.
        browse_target(&snapshot, &path)?;
        let root = self.snapshot_root_object(&driver, &snapshot).await?;
        let source = browse_target(&root, &path)?;

        if dry_run {
            // kopia's restore has no dry run. Listing the entry is the closest
            // honest thing: it proves the snapshot and path resolve, without
            // writing anything.
            driver.list_directory(&source, &RunContext::new()).await.map_err(kopia_to_error)?;
            return Ok(StartedReply {
                run_id: Uuid::new_v4(),
                started: false,
                note: Some(format!(
                    "Dry run: {} would be restored to {}. Nothing was written.",
                    source,
                    target.display()
                )),
            });
        }

        let run_id = Uuid::new_v4();
        let runtime = Arc::clone(&self.runtime);
        let destination_id = dest.id;
        let destination_name = dest.name.clone();
        let target_display = target.display().to_string();
        let restore_into = target.clone();
        tokio::spawn(async move {
            let (events, rx) = EventSink::channel(64);
            let (handle, token) = cancellation();
            // A restore ends when the daemon does; nothing else can cancel it,
            // so it is bound to the shutdown signal rather than to a run.
            let mut shutdown = runtime.subscribe_shutdown();
            tokio::spawn(async move {
                if shutdown.recv().await.is_ok() {
                    handle.cancel();
                }
            });

            let pump_runtime = Arc::clone(&runtime);
            let pump = tokio::spawn(async move {
                let mut rx = rx;
                while let Some(event) = rx.recv().await {
                    if let KopiaEvent::Progress(progress) = event {
                        pump_runtime.publish(StreamItem::Progress {
                            run_id,
                            job_id: run_id,
                            destination_id,
                            status: RunStatus::Running,
                            progress: Box::new(progress),
                        });
                    }
                }
            });

            let ctx = RunContext::new().with_cancel(token).with_events(events);
            let outcome = driver.restore(&source, &restore_into, &options, &ctx).await;
            // `ctx` owns an `EventSink`; the pump ends only when the last one
            // is dropped, so joining before this would wait for ever.
            drop(ctx);
            let _ = pump.await;

            let event = match &outcome {
                Ok(result) => {
                    runtime.publish(StreamItem::Progress {
                        run_id,
                        job_id: run_id,
                        destination_id,
                        status: RunStatus::Succeeded,
                        progress: Box::new(result.progress.clone()),
                    });
                    Event::info(
                        "restore.finished",
                        format!(
                            "Restore from \"{destination_name}\" to {target_display} finished."
                        ),
                    )
                }
                Err(e) => Event::error(
                    "restore.failed",
                    format!("Restore from \"{destination_name}\" failed: {}", e.message),
                ),
            }
            .with_destination(destination_id)
            .with_run(run_id);
            runtime.record_event(event);
        });

        Ok(StartedReply {
            run_id,
            started: true,
            note: Some(format!("Restoring into {}.", target.display())),
        })
    }

    async fn delete_snapshot(
        &self,
        _ctx: &RequestContext,
        destination: String,
        snapshot: String,
    ) -> Result<AckReply> {
        self.require_unlocked().await?;
        if snapshot.trim().is_empty() {
            return Err(Error::Validation("no snapshot was named".into()));
        }
        let (target, driver) = self.driver_for(&destination).await?;
        // `confirm: true` is this call site stating, in code, that a human
        // asked. The IPC command is itself the confirmation.
        driver
            .delete_snapshot(&snapshot, true, &RunContext::new())
            .await
            .map_err(kopia_to_error)?;
        self.runtime.record_event(
            Event::warn(
                "snapshot.deleted",
                format!("A snapshot was deleted from \"{}\".", target.name),
            )
            .with_destination(target.id),
        );
        Ok(AckReply {})
    }

    // -- vault ------------------------------------------------------------

    async fn unlock_vault(
        &self,
        _ctx: &RequestContext,
        passphrase: SecretString,
    ) -> Result<UnlockedReply> {
        let secret = passphrase.into_secret();
        {
            let mut store = self.runtime.store.lock().await;
            if store.is_locked() {
                store.unlock(&secret)?;
            }
        }
        super::lifecycle::on_unlocked(&self.runtime, secret).await;
        Ok(self.unlocked_reply(true))
    }

    async fn lock_vault(&self, _ctx: &RequestContext) -> Result<UnlockedReply> {
        super::lifecycle::lock(&self.runtime, "vault.locked", "The vault was locked.").await;
        Ok(self.unlocked_reply(false))
    }

    async fn vault_is_unlocked(&self, _ctx: &RequestContext) -> Result<UnlockedReply> {
        let unlocked = !self.runtime.store.lock().await.is_locked();
        Ok(self.unlocked_reply(unlocked))
    }

    async fn change_passphrase(
        &self,
        _ctx: &RequestContext,
        current: SecretString,
        replacement: SecretString,
    ) -> Result<AckReply> {
        self.require_unlocked().await?;
        super::rekey::change_passphrase(
            &self.runtime,
            current.into_secret(),
            replacement.into_secret(),
        )
        .await?;
        Ok(AckReply {})
    }

    async fn set_secret(
        &self,
        _ctx: &RequestContext,
        secret_ref: SecretRef,
        value: SecretString,
    ) -> Result<AckReply> {
        self.require_unlocked().await?;
        if value.expose().is_empty() {
            return Err(Error::Validation("refusing to store an empty secret".into()));
        }
        if secret_ref.as_str().trim().is_empty() {
            return Err(Error::Validation("a secret needs a handle".into()));
        }
        let handle = secret_ref.clone();
        let mut store = self.runtime.store.lock().await;
        store.put_secret(handle, value.into_secret())?;
        drop(store);
        // The handle is not secret; the value is, and it appears nowhere here.
        self.runtime.record_event(Event::info(
            "vault.secret_set",
            format!("A credential was stored under {secret_ref}."),
        ));
        Ok(AckReply {})
    }

    /// The protocol's one sanctioned path for secret material to leave the
    /// daemon. Read the `vault.export_keys` entry in
    /// `crates/core/src/ipc/protocol.rs` before touching this, and
    /// `THREAT_MODEL.md` §A7 for the recorded exception.
    ///
    /// This method's share of the bounds:
    ///
    /// * The vault must be unlocked **and** the master passphrase must be
    ///   re-presented and verified against the sealed vault. Reaching the
    ///   socket is not enough — the daemon may be SYSTEM while the caller is
    ///   not, and this is the one request where that distinction is decisive.
    /// * Rate-limited, so the socket is not an oracle.
    /// * Logged: that an export happened, how many repositories it covered,
    ///   and nothing else. No part of the document reaches a log, an event, an
    ///   error message or a tracing span.
    /// * No file is written here. The document goes back to the caller, which
    ///   saves it where the *user* chose. A path parameter would turn an
    ///   elevated daemon into an arbitrary-file-write primitive, which is a
    ///   far worse hole than the disclosure this feature is about.
    async fn export_encryption_keys(
        &self,
        _ctx: &RequestContext,
        passphrase: SecretString,
    ) -> Result<KeyExportReply> {
        self.require_unlocked().await?;
        if let Some(wait) = self.runtime.export_cooldown_remaining() {
            return Err(Error::Validation(format!(
                "An encryption key export was made moments ago. Try again in {} second{}.",
                wait.as_secs().max(1),
                if wait.as_secs() == 1 { "" } else { "s" }
            )));
        }

        // Verified against the sealed vault rather than against the cached
        // master key: the cached key proves only that *somebody* unlocked this
        // daemon at some point, which is exactly the assumption this check
        // exists to refuse.
        let secret = passphrase.into_secret();
        {
            let store = self.runtime.store.lock().await;
            let sealed = store.vault().sealed_bytes().to_vec();
            drop(store);
            superbackup_core::crypto::Vault::unlock(&sealed, &secret)
                .map_err(|_| Error::BadPassphrase)?;
        }

        let generated_at = Utc::now();
        let (export, file_name) = {
            let store = self.runtime.store.lock().await;
            let name = super::keyexport::suggested_file_name(store.config(), generated_at);
            (super::keyexport::build(&store, generated_at), name)
        };
        self.runtime.note_export();

        // Counts and names of what was *left out*; never a byte of the
        // document itself.
        self.runtime.record_event(Event::new(
            Severity::Warning,
            "vault.keys_exported",
            format!(
                "Repository encryption keys were exported for {} destination{}. Anyone holding \
                 that file can read those backups.",
                export.exported,
                if export.exported == 1 { "" } else { "s" }
            ),
        ));
        tracing::warn!(
            destinations = export.exported,
            omitted = export.omitted.len(),
            "repository encryption keys were exported"
        );

        Ok(KeyExportReply {
            document: export.document,
            destinations: export.exported,
            omitted: export.omitted,
            suggested_file_name: file_name,
            generated_at,
        })
    }

    async fn list_secret_refs(&self, _ctx: &RequestContext) -> Result<SecretRefsReply> {
        self.require_unlocked().await?;
        let store = self.runtime.store.lock().await;
        let refs = store.vault().list_refs()?;
        Ok(SecretRefsReply { refs })
    }

    // -- control ----------------------------------------------------------

    async fn pause(
        &self,
        _ctx: &RequestContext,
        seconds: Option<u64>,
        reason: Option<String>,
    ) -> Result<PauseReply> {
        // A pause longer than a year is a typo, not an intention.
        let until = match seconds {
            Some(s) if s > 0 => {
                Some(Utc::now() + ChronoDuration::seconds(s.min(365 * 24 * 3600) as i64))
            }
            _ => None,
        };
        let state = PauseState { paused: true, until, reason: reason.clone() };
        self.commit(move |config| {
            config.settings.pause = state;
            Ok(())
        })
        .await?;
        self.runtime.record_event(Event::info(
            "control.paused",
            match until {
                Some(at) => format!("Backups paused until {}.", at.format("%H:%M")),
                None => "Backups paused until you resume them.".to_string(),
            },
        ));
        Ok(self.pause_reply().await)
    }

    async fn resume(&self, _ctx: &RequestContext) -> Result<PauseReply> {
        self.commit(|config| {
            config.settings.pause = PauseState::default();
            Ok(())
        })
        .await?;
        self.runtime.record_event(Event::info("control.resumed", "Backups resumed."));
        Ok(self.pause_reply().await)
    }

    async fn pause_state(&self, _ctx: &RequestContext) -> Result<PauseReply> {
        Ok(self.pause_reply().await)
    }

    async fn set_bandwidth(
        &self,
        _ctx: &RequestContext,
        bandwidth: superbackup_core::model::BandwidthSettings,
    ) -> Result<BandwidthReply> {
        let stored = bandwidth.clone();
        self.commit(move |config| {
            config.settings.bandwidth = stored;
            Ok(())
        })
        .await?;
        Ok(BandwidthReply { bandwidth })
    }

    async fn reload_config(&self, _ctx: &RequestContext) -> Result<AckReply> {
        let mut store = self.runtime.store.lock().await;
        store.reload_config()?;
        let config = store.config().clone();
        drop(store);
        self.runtime.push_config(&config);
        self.runtime.publish_status().await;
        self.runtime.record_event(Event::info(
            "config.reloaded",
            "The configuration was re-read from disk.",
        ));
        Ok(AckReply {})
    }

    async fn shutdown(&self, _ctx: &RequestContext, stop_runs: bool) -> Result<AckReply> {
        self.runtime.record_event(Event::info(
            "daemon.stopping",
            if stop_runs {
                "Shutting down and stopping any running backups."
            } else {
                "Shutting down once running backups finish."
            },
        ));
        // Returns immediately; the reply must reach the client before the
        // socket closes, so the actual teardown happens on the main task.
        self.runtime.request_shutdown(stop_runs);
        Ok(AckReply {})
    }

    // -- settings ---------------------------------------------------------

    async fn get_settings(&self, _ctx: &RequestContext) -> Result<SettingsReply> {
        Ok(SettingsReply { settings: Box::new(self.config().await.settings) })
    }

    /// Rename this machine, without moving its destination folder.
    ///
    /// The label and the slug are deliberately separate. The slug is the
    /// folder name under every destination root, and renaming it would leave
    /// repositories where kopia can no longer find them — so a rename changes
    /// what people read and nothing on disk. The manifest written beside the
    /// backups carries the new label, so someone browsing the drive still sees
    /// the current name.
    ///
    /// Until this existed there was no way at all to rename a machine: the
    /// Settings field rebuilt itself from the snapshot on every frame, so it
    /// discarded each keystroke, and there was nothing for it to call anyway.
    async fn rename_machine(&self, _ctx: &RequestContext, label: String) -> Result<AckReply> {
        let label = label.trim().to_string();
        if label.is_empty() {
            return Err(Error::Validation("a machine label cannot be empty".into()));
        }
        if label.chars().count() > 64 {
            return Err(Error::Validation("a machine label is at most 64 characters".into()));
        }

        let mut event = None;
        self.commit(|config| {
            event = superbackup_core::platform::identity::rename(&mut config.machine, &label);
            Ok(())
        })
        .await?;

        if let Some(event) = event {
            self.runtime.record_event(event);
        }
        Ok(AckReply {})
    }

    async fn update_settings(
        &self,
        _ctx: &RequestContext,
        settings: Box<Settings>,
    ) -> Result<SettingsReply> {
        let before = self.config().await.settings;
        let replacement = (*settings).clone();
        self.commit(move |config| {
            config.settings = replacement;
            Ok(())
        })
        .await?;
        let after = self.config().await.settings;

        self.runtime.notifier.update_settings(after.notifications.clone());
        // Re-arm the auto-lock against the new interval, so shortening it
        // takes effect now rather than after the next unlock.
        let unlocked = !self.runtime.store.lock().await.is_locked();
        if unlocked {
            self.runtime.arm_auto_lock(after.auto_lock_minutes);
        }

        // The setting *is* the consent, so it takes effect on the transition
        // rather than at the next unlock: switching it off must destroy what
        // is already cached, and switching it on must cache what the daemon is
        // holding right now — otherwise a user who ticks the box and reboots
        // finds it did nothing.
        if before.use_os_keychain != after.use_os_keychain {
            self.apply_keychain_setting(after.use_os_keychain, unlocked).await;
        }

        if let Some(event) = settings_changed_event(&before, &after) {
            self.runtime.record_event(event);
        }
        Ok(SettingsReply { settings: Box::new(after) })
    }

    // -- remote configuration ---------------------------------------------

    async fn remote_pull(&self, _ctx: &RequestContext) -> Result<RemoteStatusReply> {
        self.require_unlocked().await?;
        let config = self.config().await;
        let source = config
            .remote
            .clone()
            .ok_or_else(|| Error::Remote("no remote configuration repository is set up".into()))?;

        let token = match &source.auth {
            superbackup_core::model::RemoteAuth::Token { token_ref } => {
                let store = self.runtime.store.lock().await;
                store.secret(token_ref)?
            }
            _ => None,
        };
        let client = superbackup_core::remote::RemoteClient::new()?;
        let fetched = client.fetch(&source, token.as_ref()).await?;

        // Verification needs the passphrase, not the derived keys: the fetched
        // vault is a *different* file with a different salt.
        let passphrase = self.runtime.master()?;
        let store = self.runtime.store.lock().await;
        let plan = superbackup_core::remote::verify_pull(
            &fetched,
            store.config(),
            store.vault(),
            &source,
            &passphrase,
        )?;
        drop(store);

        let changes = plan.diff.total();
        self.runtime.set_pull(Some(plan));
        self.runtime.record_event(Event::info(
            "remote.pulled",
            format!(
                "Fetched the shared configuration: {changes} change(s) are waiting to be applied."
            ),
        ));
        Ok(self.remote_status_reply(Some(format!("{changes} change(s) available"))).await)
    }

    /// Hand out the sealed configuration so it can be written to a file.
    ///
    /// The same document `remote.push` publishes — sealed under the master
    /// passphrase — so moving a configuration between machines does not
    /// require setting up a Git remote first. It carries every repository key,
    /// which is precisely why it is handed out sealed rather than as readable
    /// configuration: this is a copy of the vault, not an export of its
    /// contents, and it is worth exactly as much to an attacker as the vault
    /// file already on disk, which is to say nothing without the passphrase.
    async fn export_config(&self, _ctx: &RequestContext) -> Result<ConfigDocumentReply> {
        self.require_unlocked().await?;
        let payload = {
            let mut store = self.runtime.store.lock().await;
            store.publication_payload()?
        };
        let size_bytes = payload.len() as u64;
        let label = superbackup_core::model::slugify(&self.config().await.machine.label);
        let stamp = Utc::now().format("%Y%m%d");
        self.runtime.record_event(Event::info(
            "config.exported",
            "The sealed configuration was exported to a document.",
        ));
        Ok(ConfigDocumentReply {
            document: superbackup_core::crypto::base64_for_upload(&payload),
            suggested_filename: format!("superbackup-{label}-{stamp}.sbvault"),
            size_bytes,
        })
    }

    /// Verify an exported document and report what applying it would change.
    ///
    /// Deliberately routed through the very same verification a pull uses:
    /// signature checking, decryption under the master passphrase, validation
    /// of the incoming configuration, the rollback guard, and the "this is a
    /// different vault entirely" guard. A manual file is not more trustworthy
    /// than a Git remote for having been carried on a stick — if anything it
    /// is less, because nothing recorded where it came from.
    ///
    /// Nothing is written. The plan is staged exactly as a pull's is, so
    /// `remote.apply` accepts it through the code path that already exists.
    async fn import_config(
        &self,
        _ctx: &RequestContext,
        document: String,
        allow_rollback: bool,
    ) -> Result<RemoteDiffReply> {
        self.require_unlocked().await?;
        let bytes = superbackup_core::crypto::base64_from_download(document.trim()).map_err(|_| {
            Error::Validation(
                "that is not a superbackup configuration document. Export one with                  `config.export`, or choose the .sbvault file you were given."
                    .into(),
            )
        })?;
        if bytes.is_empty() {
            return Err(Error::Validation("the configuration document is empty".into()));
        }

        let fetched = superbackup_core::remote::FetchedVault {
            bytes,
            // Recorded in the audit log, so it says what actually happened.
            source_url: "a file chosen on this machine".to_string(),
            // No blob SHA: a file has no place to be pushed back to, and
            // pretending otherwise would let a later push overwrite a remote
            // using a marker that never came from it.
            sha: None,
        };
        let passphrase = self.runtime.master()?;
        let options =
            superbackup_core::remote::PullOptions { allow_rollback, allow_different_vault: false };

        let plan = {
            let store = self.runtime.store.lock().await;
            let config = store.config().clone();
            let source = config.remote.clone().unwrap_or_else(imported_source);
            superbackup_core::remote::verify_pull_with(
                &fetched,
                &config,
                store.vault(),
                &source,
                &passphrase,
                &options,
            )?
        };
        let changes = describe_diff(&plan.diff);
        self.runtime.set_pull(Some(plan));
        self.runtime.record_event(Event::info(
            "config.imported",
            format!("A configuration document was read: {} change(s) to review.", changes.len()),
        ));
        Ok(RemoteDiffReply { changes, remote_commit: None })
    }

    async fn remote_diff(&self, _ctx: &RequestContext) -> Result<RemoteDiffReply> {
        self.require_unlocked().await?;
        let plan = self.runtime.pull().ok_or_else(|| {
            Error::Remote("nothing has been pulled yet; run `remote.pull` first".into())
        })?;
        Ok(RemoteDiffReply { changes: describe_diff(&plan.diff), remote_commit: plan.sha.clone() })
    }

    async fn remote_apply(&self, _ctx: &RequestContext) -> Result<RemoteStatusReply> {
        self.require_unlocked().await?;
        let plan = self.runtime.pull().ok_or_else(|| {
            Error::Remote(
                "nothing has been pulled yet. Pull and review the changes before applying them."
                    .into(),
            )
        })?;
        let changes = plan.diff.total();

        let mut store = self.runtime.store.lock().await;
        superbackup_core::remote::apply_pull(&mut store, &plan)?;
        // The pulled vault is sealed under the same master passphrase, so the
        // daemon can reopen it without asking again — and must, or the machine
        // would be silently locked out until the next manual unlock.
        let passphrase = self.runtime.master()?;
        store.unlock(&passphrase)?;
        let config = store.config().clone();
        drop(store);

        self.runtime.set_pull(None);
        self.runtime.push_config(&config);
        self.runtime.publish_status().await;
        self.runtime.record_event(Event::info(
            "remote.applied",
            format!("Applied {changes} change(s) from the shared configuration."),
        ));
        Ok(self.remote_status_reply(Some("applied".into())).await)
    }

    async fn remote_push(
        &self,
        _ctx: &RequestContext,
        message: Option<String>,
    ) -> Result<RemoteStatusReply> {
        self.require_unlocked().await?;
        let config = self.config().await;
        let source = config
            .remote
            .clone()
            .ok_or_else(|| Error::Remote("no remote configuration repository is set up".into()))?;
        if !source.allow_push {
            return Err(Error::Remote(
                "publishing is disabled for this remote; enable it in Settings first".into(),
            ));
        }
        let superbackup_core::model::RemoteAuth::Token { token_ref } = &source.auth else {
            return Err(Error::Remote(
                "publishing needs a personal access token; add one in Settings".into(),
            ));
        };
        let store_token = {
            let store = self.runtime.store.lock().await;
            store.require_secret(token_ref)?
        };

        let (payload, sha) = {
            let mut store = self.runtime.store.lock().await;
            let payload = store.publication_payload()?;
            (payload, source.last_known_commit.clone())
        };
        let request = superbackup_core::remote::PushRequest::new(
            payload,
            message.unwrap_or_else(|| {
                format!("superbackup: configuration from {}", config.machine.label)
            }),
        )
        .replacing(sha)
        .confirmed();
        let client = superbackup_core::remote::RemoteClient::new()?;
        let new_sha = client.push(&source, &store_token, &request).await?;

        self.commit(move |config| {
            if let Some(remote) = &mut config.remote {
                remote.last_known_commit = Some(new_sha.clone());
            }
            Ok(())
        })
        .await?;
        self.runtime.record_event(Event::info(
            "remote.pushed",
            "The sealed configuration was published to the shared repository.",
        ));
        Ok(self.remote_status_reply(Some("published".into())).await)
    }

    // -- service ----------------------------------------------------------

    async fn service_status(&self, _ctx: &RequestContext) -> Result<ServiceReply> {
        Ok(self.service_reply(None))
    }

    async fn install_service(&self, _ctx: &RequestContext) -> Result<ServiceReply> {
        let options = platform::ServiceOptions::current(&self.runtime.paths)?;
        if options.requires_elevation() && !platform::service::is_elevated() {
            return Err(Error::Service(
                "installing the background service needs administrator rights. Start superbackup \
                 with \"Run as administrator\" and try again."
                    .into(),
            ));
        }
        platform::service::install(&options)?;
        // Started here rather than left to the next boot: an installed service
        // that is not running looks identical to a broken one.
        if let Err(e) = platform::service::start(&options.name, options.scope) {
            tracing::warn!(error = %e, "the service was installed but did not start");
        }
        self.runtime.invalidate_service_status();
        self.runtime.record_event(Event::info(
            "service.installed",
            "The background service was installed.",
        ));
        Ok(self.service_reply(Some(service_reach_summary(&self.config().await))))
    }

    async fn uninstall_service(&self, _ctx: &RequestContext) -> Result<ServiceReply> {
        let options = platform::ServiceOptions::current(&self.runtime.paths)?;
        if options.requires_elevation() && !platform::service::is_elevated() {
            return Err(Error::Service(
                "removing the background service needs administrator rights.".into(),
            ));
        }
        let _ = platform::service::stop(&options.name, options.scope);
        platform::service::uninstall(&options.name, options.scope)?;
        self.runtime.invalidate_service_status();
        self.runtime.record_event(Event::info(
            "service.uninstalled",
            "The background service was removed. Your configuration and backups were not touched.",
        ));
        Ok(self.service_reply(None))
    }

    /// Add superbackup to this user's applications menu, or take it out.
    ///
    /// Deliberately its own switch rather than a side effect of autostart:
    /// "be findable in the Start menu" and "run at every login" are different
    /// decisions, and plenty of people want the first without the second.
    async fn set_shortcut(&self, _ctx: &RequestContext, enabled: bool) -> Result<ServiceReply> {
        let spec = platform::autostart::AutostartSpec::current()?;
        let detail = if enabled {
            let path = platform::shortcut::install(&spec)?;
            self.runtime.record_event(Event::info(
                "app.shortcut_added",
                "superbackup was added to the applications menu.",
            ));
            Some(format!("Added to the applications menu: {}", path.display()))
        } else {
            let removed = platform::shortcut::remove()?;
            self.runtime.record_event(Event::info(
                "app.shortcut_removed",
                "superbackup was removed from the applications menu.",
            ));
            Some(if removed {
                "Removed from the applications menu.".to_string()
            } else {
                "It was not in the applications menu.".to_string()
            })
        };
        Ok(self.service_reply(detail))
    }

    async fn set_autostart(&self, _ctx: &RequestContext, enabled: bool) -> Result<ServiceReply> {
        if enabled {
            platform::autostart::enable(&platform::autostart::AutostartSpec::current()?)?;
        } else {
            platform::autostart::disable()?;
        }
        self.commit(move |config| {
            config.settings.start_at_login = enabled;
            Ok(())
        })
        .await?;
        Ok(self.service_reply(None))
    }

    // -- streaming --------------------------------------------------------

    fn event_stream(
        &self,
        _ctx: &RequestContext,
        topics: &[Topic],
    ) -> Result<broadcast::Receiver<StreamItem>> {
        Ok(self.runtime.subscribe_stream(topics))
    }
}

// ---------------------------------------------------------------------------
// Support
// ---------------------------------------------------------------------------

impl DaemonHandler {
    /// React to `use_os_keychain` being switched on or off.
    ///
    /// Turning it off is destructive and immediate: the cache is the thing the
    /// user just withdrew consent for. Turning it on is best effort — the
    /// vault may be locked, in which case there is nothing to cache yet and
    /// the next unlock will do it.
    async fn apply_keychain_setting(&self, enabled: bool, unlocked: bool) {
        if !enabled {
            match super::keychain::forget(&self.runtime.paths).await {
                Ok(()) => self.runtime.record_event(Event::info(
                    "vault.keychain_cleared",
                    "The saved passphrase was removed. superbackup will ask for it again.",
                )),
                Err(e) => self.runtime.record_event(Event::new(
                    Severity::Warning,
                    "vault.keychain_not_cleared",
                    format!(
                        "The saved passphrase could not be removed from the keychain ({e}). Remove the superbackup entry by hand."
                    ),
                )),
            }
            return;
        }
        if !unlocked {
            // Nothing to cache: the passphrase is only in memory while open.
            return;
        }
        let Ok(passphrase) = self.runtime.master() else { return };
        match super::keychain::store(&self.runtime.paths, &passphrase).await {
            Ok(()) => self.runtime.record_event(Event::info(
                "vault.keychain_stored",
                "Your passphrase is now saved in this machine's credential store.",
            )),
            Err(e) => self.runtime.record_event(Event::new(
                Severity::Warning,
                "vault.keychain_failed",
                format!("{} ({e})", super::keychain::explain_unavailable()),
            )),
        }
    }

    /// Make sure a repository destination has a passphrase in the vault before
    /// its repository is created.
    ///
    /// Order matters and is not negotiable: a repository whose passphrase was
    /// never written to the vault is unopenable, and no amount of retrying
    /// recovers it. So the secret is generated and *saved* first, and only
    /// then does kopia create anything.
    async fn ensure_repository_passphrase(&self, needle: &str) -> Result<()> {
        let mut store = self.runtime.store.lock().await;
        let destination = resolve_destination(store.config(), needle)?.clone();
        if !destination.kind.is_repository() {
            return Ok(());
        }
        // A derived passphrase needs nothing stored; `destination_passphrase`
        // computes it from the master key.
        let source = destination
            .encryption
            .as_ref()
            .map(|e| e.passphrase_source)
            .unwrap_or(superbackup_core::model::PassphraseSource::Generated);
        if source == superbackup_core::model::PassphraseSource::DerivedFromMaster {
            return Ok(());
        }
        let Some(handle) = destination.passphrase_ref.clone() else {
            return Err(Error::Config(format!(
                "destination \"{}\" has no passphrase handle",
                destination.name
            )));
        };
        if store.secret(&handle)?.is_some() {
            return Ok(());
        }
        if source == superbackup_core::model::PassphraseSource::UserSupplied {
            return Err(Error::Config(format!(
                "\"{}\" is set to use a passphrase you supply, but none has been stored. Set it \
                 with `vault.set_secret` before creating the repository.",
                destination.name
            )));
        }
        // 32 random bytes, base64: 256 bits of entropy, and safe to pass
        // through an environment variable.
        let generated = superbackup_core::crypto::base64_for_upload(
            &superbackup_core::crypto::random_bytes(32)?,
        );
        store.put_secret(handle, Secret::from_string(generated))?;
        Ok(())
    }
}

/// Turn a kopia failure into the crate's error type for an IPC reply.
/// Map a driver failure onto the crate-wide error type.
///
/// For anything classified, the headline is the actionable sentence and
/// kopia's own words would only get in the way. For anything *un*classified it
/// is the reverse: `KopiaFailure::Unknown`'s headline is the literal string
/// "kopia reported an error", which tells nobody anything — and this function
/// used to pass exactly that and drop `detail` on the floor, so every
/// unrecognised failure reached the user as "kopia exited with status 1: kopia
/// reported an error." The one piece of information that would have explained
/// it was captured, carried all the way here, and then discarded.
/// A stand-in `RemoteConfigSource` for a document that came from a file.
///
/// `verify_pull_with` takes one because a pull always has a remote behind it.
/// An import does not, so this describes the only thing that is actually true:
/// nothing was fetched, and there is nowhere to push back to. Pinning is left
/// empty deliberately — a file cannot be pinned to a key the way a repository
/// can, and pretending it could would be a security claim we cannot honour.
fn imported_source() -> superbackup_core::model::RemoteConfigSource {
    superbackup_core::model::RemoteConfigSource {
        url: "file://imported".into(),
        branch: String::new(),
        path: String::new(),
        auth: superbackup_core::model::RemoteAuth::None,
        auto_pull: false,
        pull_interval_minutes: 0,
        allow_push: false,
        last_pull_at: None,
        last_known_commit: None,
        trusted_signers: Vec::new(),
    }
}

fn kopia_to_error(e: superbackup_core::kopia::KopiaError) -> Error {
    // Log the whole thing before narrowing it. The detail is already redacted
    // by the capture, and a failure nobody can explain afterwards is how a
    // bug report becomes unactionable.
    if let Some(detail) = &e.detail {
        tracing::warn!(
            command = %e.command,
            status = e.status.unwrap_or(-1),
            failure = ?e.failure,
            "kopia failed: {detail}"
        );
    } else {
        tracing::warn!(
            command = %e.command,
            status = e.status.unwrap_or(-1),
            failure = ?e.failure,
            "kopia failed and printed nothing that could be captured"
        );
    }

    let headline = match (&e.failure, &e.detail) {
        // Unclassified: kopia's own text is the only real information there
        // is, so it becomes the message rather than being hidden behind a
        // sentence that says nothing.
        (superbackup_core::kopia::KopiaFailure::Unknown, Some(detail)) => {
            format!("{} {detail}", e.message)
        }
        _ => e.message.clone(),
    };

    match e.failure.error_code() {
        ErrorCode::BadPassphrase => Error::BadPassphrase,
        ErrorCode::RepoNotConnected => Error::RepoNotConnected(headline),
        ErrorCode::RepoExists => Error::RepoExists(headline),
        ErrorCode::KopiaMissing => Error::KopiaMissing,
        ErrorCode::JobCancelled => Error::JobCancelled(headline),
        _ => Error::Kopia { status: e.status.unwrap_or(-1), stderr: headline },
    }
}

/// Can a folder be written to? Used by `dest.test` for folder mirrors, where
/// there is no repository to connect to.
async fn probe_directory(path: &std::path::Path) -> (bool, bool, Option<String>) {
    let path = path.to_path_buf();
    let result = tokio::task::spawn_blocking(move || {
        if let Err(e) = std::fs::create_dir_all(&path) {
            return (false, false, Some(format!("{e}")));
        }
        let probe = path.join(".superbackup-write-test");
        match std::fs::write(&probe, b"superbackup") {
            Ok(()) => {
                let _ = std::fs::remove_file(&probe);
                (true, true, None)
            }
            Err(e) => (true, false, Some(format!("{e}"))),
        }
    })
    .await;
    result.unwrap_or((false, false, Some("the write probe did not complete".into())))
}

/// Render a [`superbackup_core::remote::ConfigDiff`] as protocol changes.
fn describe_diff(diff: &superbackup_core::remote::ConfigDiff) -> Vec<ConfigChange> {
    let mut out = Vec::new();
    let sets = [
        ("job", &diff.jobs),
        ("destination", &diff.destinations),
        ("provider", &diff.providers),
        ("project", &diff.projects),
    ];
    for (entity, set) in sets {
        for (kind, changes) in [
            (ChangeKind::Added, &set.added),
            (ChangeKind::Removed, &set.removed),
            (ChangeKind::Modified, &set.modified),
        ] {
            for change in changes {
                out.push(ConfigChange {
                    kind,
                    entity: entity.to_string(),
                    id: Some(change.id),
                    name: change.name.clone(),
                    summary: match kind {
                        ChangeKind::Added => format!("{entity} \"{}\" would be added", change.name),
                        ChangeKind::Removed => {
                            format!("{entity} \"{}\" would be removed", change.name)
                        }
                        ChangeKind::Modified => {
                            format!("{entity} \"{}\" would change", change.name)
                        }
                    },
                });
            }
        }
    }
    if diff.machine_identity_changes {
        out.push(ConfigChange {
            kind: ChangeKind::Modified,
            entity: "settings".into(),
            id: None,
            name: "This machine".into(),
            summary: "The shared configuration describes a different machine identity; the local \
                      one is kept."
                .into(),
        });
    }
    out
}

/// An honest sentence about what a LocalSystem service can and cannot reach.
///
/// Shown after `service.install` rather than left for the user to discover
/// three days later when their OneDrive destination has never been written.
pub fn service_reach_summary(config: &superbackup_core::model::Config) -> String {
    let account = platform::service::ServiceAccount::LocalSystem;
    let mut unsupported = Vec::new();
    let mut degraded = Vec::new();
    for destination in &config.destinations {
        match platform::service::destination_support(
            &destination.kind,
            &account,
            ServiceScope::System,
        ) {
            platform::service::SupportLevel::Supported => {}
            platform::service::SupportLevel::Degraded { .. } => {
                degraded.push(destination.name.clone())
            }
            platform::service::SupportLevel::Unsupported { .. } => {
                unsupported.push(destination.name.clone())
            }
        }
    }
    if unsupported.is_empty() && degraded.is_empty() {
        return "Every destination is reachable from the service.".to_string();
    }
    let mut parts = Vec::new();
    if !unsupported.is_empty() {
        parts.push(format!(
            "The service cannot reach {}; back those up from the tray app instead.",
            unsupported.join(", ")
        ));
    }
    if !degraded.is_empty() {
        parts.push(format!("{} will work with caveats.", degraded.join(", ")));
    }
    parts.join(" ")
}

/// What to say when a destination exists but its repository does not yet.
///
/// Not an error: the user added a destination and has not yet run the one-time
/// step that initialises encrypted storage in it.
///
/// The wording has to carry three things, because the first version carried
/// none of them and the user quite reasonably asked what it meant. It must name
/// the right thing — a bucket is not a folder — say what a repository *is* in
/// terms of what it does for them, and say where the control lives, because
/// "use Create repository" is useless if you do not know which screen it is on.
fn copy_no_repository_yet(kind: &DestinationKind) -> String {
    let place = match kind {
        DestinationKind::S3 { .. } => "bucket",
        DestinationKind::OneDrive { .. } => "OneDrive folder",
        _ => "folder",
    };
    format!(
        "The {place} is reachable, but it has no backup repository in it yet. A repository \
         is the encrypted store superbackup writes snapshots into, and it is created once \
         per destination. Open this destination from the Destinations list and choose \
         \"Create repository\" to set it up."
    )
}

/// What a successful credential check actually established.
///
/// Says "signed in" rather than "connected", because that is the stronger and
/// more useful claim: the endpoint verified a signature, so the key pair is
/// right and the clock is close enough. The bucket count is the evidence.
fn copy_buckets_visible(count: usize) -> String {
    match count {
        0 => "The credentials were accepted. This account owns no buckets yet.".to_string(),
        1 => "The credentials were accepted; 1 bucket is visible.".to_string(),
        n => format!("The credentials were accepted; {n} buckets are visible."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The restore browser addresses an *object*, and passing a manifest id
    /// is what stopped it ever listing anything: kopia answered `invalid
    /// content ID: "3e0f…" (17 vs 33)`. The two ids cannot be confused by
    /// shape, so telling them apart is exact rather than a guess.
    #[test]
    fn a_manifest_id_is_never_mistaken_for_an_object_id() {
        // What `snapshot.list` reports: 32 hex characters, no prefix.
        assert!(!is_object_id("3e0f129d2f627d3bffdbc03aed2c95e1"));
        // What `kopia show` wants: `k` and 64 hex characters.
        assert!(is_object_id("k631816d2e632adec48a21d05dfbc873cf4e1e9ec27eae3643582504d78eae2a4"));
        // Neither.
        assert!(!is_object_id(""));
        assert!(!is_object_id("k"));
        assert!(!is_object_id("knot-hex-at-all-but-quite-long-indeed-really"));
        // A manifest id that happens to begin with `k` is still too short.
        assert!(!is_object_id("k3e0f129d2f627d3bffdbc03aed2c95"));
    }

    #[test]
    fn browse_targets_are_built_without_letting_a_path_escape() {
        assert_eq!(browse_target("k123", "").ok(), Some("k123".to_string()));
        assert_eq!(browse_target("k123", "/src/main.rs").ok(), Some("k123/src/main.rs".into()));
        assert_eq!(browse_target("k123", "src\\lib").ok(), Some("k123/src/lib".into()));
        assert!(browse_target("k123", "../../etc/passwd").is_err());
        assert!(browse_target("k1/23", "x").is_err());
        assert!(browse_target("  ", "x").is_err());
    }

    #[test]
    fn conflict_policies_map_onto_flags_kopia_actually_has() {
        let skip = restore_options(ConflictPolicy::Skip).expect("skip");
        assert!(skip.skip_existing && !skip.overwrite_files);
        let over = restore_options(ConflictPolicy::Overwrite).expect("overwrite");
        assert!(over.overwrite_files && !over.skip_existing);
        let fail = restore_options(ConflictPolicy::Fail).expect("fail");
        assert!(!fail.overwrite_files && !fail.skip_existing);
        // Refused rather than approximated: see the function's documentation.
        assert!(restore_options(ConflictPolicy::KeepBoth).is_err());
    }

    #[test]
    fn paths_are_normalised_for_the_reply() {
        assert_eq!(normalised_path("/a//b/./c"), "a/b/c");
        assert_eq!(normalised_path(""), "");
        assert_eq!(normalised_path("a\\b"), "a/b");
    }

    #[test]
    fn a_service_summary_names_what_local_system_cannot_reach() {
        let mut config = superbackup_core::model::Config::default();
        config
            .destinations
            .push(superbackup_core::engine::testing::test_mirror("mapped", r"H:\backups"));
        let summary = service_reach_summary(&config);
        if cfg!(windows) {
            assert!(summary.contains("mapped"), "{summary}");
        }
    }
}
