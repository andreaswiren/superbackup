//! Pulling and publishing the sealed vault over HTTPS.
//!
//! # What is actually synced
//!
//! One file: `config.sbvault`. Never `config.json`. The vault carries both the
//! secrets and — when the user publishes — the configuration document (see
//! [`crate::crypto::vault`]), so a second machine that pulls it gets the jobs
//! *and* the keys those jobs need, and an onlooker with access to the
//! repository gets neither. Bucket names, endpoints and source paths are
//! reconnaissance, and they stay inside the ciphertext with everything else.
//!
//! Local run history (`state.json`) is never synced. It is per-machine truth
//! and a pull must never overwrite it.
//!
//! # No `git`
//!
//! This module speaks HTTPS to the hosting API directly rather than shelling
//! out to `git`. Three reasons, in order of importance:
//!
//! 1. **Credentials.** Handing a token to `git` means an askpass helper, a
//!    credential store, or a URL with the token embedded in it — and that URL
//!    then appears in `git`'s own error messages, in `.git/config`, and in
//!    process listings. Here the token exists as an `Authorization` header on
//!    one request and nowhere else.
//! 2. **Determinism.** No dependency on which `git` is installed, what the
//!    user's global config says, whether `core.autocrlf` will corrupt the
//!    blob, or whether a credential prompt will block a background service for
//!    ever.
//! 3. **Blast radius.** We need to read one file and write one file. A clone
//!    is a filesystem-wide operation with hooks, submodules and LFS attached.
//!
//! # The order of operations on a pull
//!
//! The bytes coming back are untrusted: an attacker who can write to the
//! repository, or who sits on the network in front of a misconfigured
//! endpoint, chooses them. So:
//!
//! ```text
//!   fetch  ->  parse  ->  verify signature  ->  decrypt  ->  diff
//!                                                              |
//!                                    (the user sees the diff and decides)
//!                                                              |
//!                                              back up local  ->  replace
//! ```
//!
//! Nothing touches the local vault until the pulled bytes have decrypted with
//! a passphrase the user just typed. A vault that does not open is not a
//! vault; writing it over the working one would be handing an attacker a
//! one-request denial of service against every backup on the machine.
//!
//! # Freshness and identity are separate checks from authenticity
//!
//! A blob can be perfectly authentic and still be the wrong one. Two cases
//! matter, and neither is caught by decryption or by a signature:
//!
//! * **Rollback.** A *previously published* version of the same vault carries
//!   a valid signature and opens with the right passphrase, for ever. Serving
//!   it back reinstates an S3 key the user rotated away, restores a
//!   destination they deleted, or undoes a `trusted_signers` change. So
//!   [`verify_pull`] refuses an incoming `updated_at` older than the local
//!   vault's for the same `vault_id`, unless the caller sets
//!   [`PullOptions::allow_rollback`].
//! * **Substitution.** A vault with a different `vault_id` is not a newer
//!   version of yours; it is somebody else's key material. [`apply_pull`]
//!   refuses it unless the caller sets
//!   [`PullOptions::allow_different_vault`].
//!
//! # The remote block never travels
//!
//! [`crate::model::RemoteConfigSource`] is machine-local configuration and is
//! preserved across a pull. It has to be: `trusted_signers` is the pinned
//! signer list, and if the pulled artifact could supply it, then one accepted
//! publish would clear the pin — and repoint `url` — for every pull
//! afterwards. A security control that the thing it protects against can
//! switch off is not a control. The same argument covers `url`, `auth` (a
//! handle to *this* machine's token) and the local pull bookkeeping.

use crate::config::{ConfigStore, Store};
use crate::crypto::{signing, BackupReason, Envelope, Vault};
use crate::error::{Error, Result};
use crate::model::{Config, RemoteAuth, RemoteConfigSource};
use crate::secret::Secret;
use std::collections::BTreeMap;
use std::time::Duration;
use uuid::Uuid;

/// How long a single request may take.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Largest response body we will read.
///
/// Matches the vault parser's own ceiling. A remote that offers more is
/// hostile or broken, and we should stop reading rather than buffer it.
pub const MAX_RESPONSE_BYTES: usize = crate::crypto::envelope::MAX_VAULT_BYTES;

/// `User-Agent` sent with every request. GitHub's API rejects requests without
/// one, and an honest identifier is better for everyone than a browser lie.
const USER_AGENT: &str = concat!("superbackup/", env!("CARGO_PKG_VERSION"));

// ---------------------------------------------------------------------------
// URL construction
// ---------------------------------------------------------------------------

/// How a configured URL was interpreted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    /// `raw.githubusercontent.com`, for a public repository with no token.
    GitHubRaw { url: String, owner: String, repo: String },
    /// GitHub's Contents API, which works for private repositories and is the
    /// only way to write.
    GitHubContents { url: String, owner: String, repo: String },
    /// Any other HTTPS URL, fetched verbatim.
    Direct { url: String },
}

impl Endpoint {
    pub fn url(&self) -> &str {
        match self {
            Endpoint::GitHubRaw { url, .. }
            | Endpoint::GitHubContents { url, .. }
            | Endpoint::Direct { url } => url,
        }
    }
}

/// Work out which URL to fetch for a configured remote.
///
/// Accepts what users actually paste:
///
/// * `https://github.com/owner/repo`, with or without `.git`, with or without
///   a trailing slash;
/// * `https://raw.githubusercontent.com/...` or any other HTTPS URL, used
///   verbatim;
/// * anything else on `github.com` that names an owner and a repository.
///
/// When a token is present the GitHub Contents API is used instead of the raw
/// host, because `raw.githubusercontent.com` does not accept
/// `Authorization` for private repositories — it wants a short-lived signed
/// URL instead, which we cannot mint.
///
/// # Errors
///
/// Anything that is not `https://`. Not a style preference: an attacker who
/// can rewrite a plaintext response chooses which vault we try to open, and a
/// token sent to fetch a private repository would cross the network in clear
/// text.
pub fn resolve_endpoint(source: &RemoteConfigSource, authenticated: bool) -> Result<Endpoint> {
    let url = source.url.trim().trim_end_matches('/');
    if url.is_empty() {
        return Err(Error::Remote("the remote config source has no URL".into()));
    }
    if !url.starts_with("https://") {
        return Err(Error::Remote(format!(
            "remote config must be fetched over HTTPS; {url:?} is not an https:// URL"
        )));
    }

    let path = normalise_repo_path(source.path.trim());
    if path.is_empty() {
        return Err(Error::Remote("the path to the vault inside the repository is empty".into()));
    }
    let branch = if source.branch.trim().is_empty() { "main" } else { source.branch.trim() };

    if let Some((owner, repo)) = github_owner_repo(url) {
        return Ok(if authenticated {
            Endpoint::GitHubContents {
                url: format!(
                    "https://api.github.com/repos/{owner}/{repo}/contents/{path}?ref={branch}"
                ),
                owner,
                repo,
            }
        } else {
            Endpoint::GitHubRaw {
                url: format!("https://raw.githubusercontent.com/{owner}/{repo}/{branch}/{path}"),
                owner,
                repo,
            }
        });
    }

    Ok(Endpoint::Direct { url: url.to_string() })
}

/// Extract `owner/repo` from a github.com URL, if it is one.
fn github_owner_repo(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("https://github.com/")?;
    let mut parts = rest.split('/');
    let owner = parts.next().filter(|s| !s.is_empty())?;
    let repo = parts.next().filter(|s| !s.is_empty())?;
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    Some((owner.to_string(), repo.to_string()))
}

/// Strip leading slashes and reject traversal.
///
/// The path is interpolated into a URL we then fetch; `../../` segments in it
/// would let a configuration file point at a different repository's contents.
fn normalise_repo_path(path: &str) -> String {
    path.split('/')
        .filter(|segment| !segment.is_empty() && *segment != "." && *segment != "..")
        .collect::<Vec<_>>()
        .join("/")
}

// ---------------------------------------------------------------------------
// Fetching
// ---------------------------------------------------------------------------

/// Raw bytes fetched from a remote, before anything has been believed about
/// them.
#[derive(Debug, Clone)]
pub struct FetchedVault {
    /// The sealed vault, exactly as served.
    pub bytes: Vec<u8>,
    /// Where it came from, for the audit log.
    ///
    /// Passed through [`crate::redact::scrub`] before it is stored, because a
    /// generic HTTPS remote is used verbatim and users do paste
    /// `https://token@host/...` into that field.
    pub source_url: String,
    /// The blob SHA reported by the Contents API, needed to overwrite the file
    /// on a later push, and useful as a change marker.
    pub sha: Option<String>,
}

/// An HTTPS client for remote configuration.
#[derive(Debug, Clone)]
pub struct RemoteClient {
    http: reqwest::Client,
}

impl RemoteClient {
    /// Build a client with TLS verification on and redirects limited.
    ///
    /// Redirects are capped rather than disabled because
    /// `raw.githubusercontent.com` legitimately redirects; they are capped
    /// low because an unbounded redirect chain is a way to make a background
    /// service spin.
    pub fn new() -> Result<RemoteClient> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .map_err(|e| Error::Remote(format!("could not build an HTTPS client: {e}")))?;
        Ok(RemoteClient { http })
    }

    /// Download the sealed vault.
    ///
    /// `token` is the personal access token from the vault, when the remote is
    /// configured for one. It is sent as an `Authorization` header and appears
    /// nowhere else — not in the URL, not in an error message, not in a log.
    pub async fn fetch(
        &self,
        source: &RemoteConfigSource,
        token: Option<&Secret>,
    ) -> Result<FetchedVault> {
        let wants_auth = matches!(source.auth, RemoteAuth::Token { .. }) && token.is_some();
        let endpoint = resolve_endpoint(source, wants_auth)?;

        let mut request = self.http.get(endpoint.url());
        if let Some(token) = token {
            let value = token
                .expose_str()
                .ok_or_else(|| Error::Remote("the access token is not valid UTF-8".into()))?;
            request = request.header("Authorization", format!("Bearer {value}"));
        }
        if matches!(endpoint, Endpoint::GitHubContents { .. }) {
            // Ask for the file itself rather than the JSON wrapper. GitHub
            // honours this for blobs under 100 MB; `read_body` copes with
            // either shape anyway.
            request = request
                .header("Accept", "application/vnd.github.raw")
                .header("X-GitHub-Api-Version", "2022-11-28");
        }

        let response = request.send().await.map_err(|e| {
            // reqwest's Display can include the URL, which for a generic
            // HTTPS remote may carry a token the user embedded by hand.
            Error::Remote(format!(
                "could not reach the remote: {}",
                crate::redact::scrub(&e.to_string())
            ))
        })?;

        let status = response.status();
        if !status.is_success() {
            return Err(remote_status_error(status, endpoint.url()));
        }

        let content_length = response.content_length().unwrap_or(0);
        if content_length as usize > MAX_RESPONSE_BYTES {
            return Err(Error::Remote(format!(
                "the remote offered {content_length} bytes; the maximum is {MAX_RESPONSE_BYTES}"
            )));
        }

        let body = response
            .bytes()
            .await
            .map_err(|e| Error::Remote(format!("could not read the response body: {e}")))?;
        if body.len() > MAX_RESPONSE_BYTES {
            return Err(Error::Remote(format!(
                "the remote returned {} bytes; the maximum is {MAX_RESPONSE_BYTES}",
                body.len()
            )));
        }

        let (bytes, sha) = decode_body(&body)?;
        Ok(FetchedVault {
            bytes,
            source_url: crate::redact::scrub(endpoint.url()).into_owned(),
            sha,
        })
    }

    /// Publish a sealed vault to a GitHub repository.
    ///
    /// # This is never automatic
    ///
    /// It requires `allow_push` on the source *and* an explicit
    /// [`PushRequest`] built by a caller that has just been told, in words,
    /// what it is about to overwrite. There is no timer, no "sync on save",
    /// and no retry loop that could turn a bad local state into a bad shared
    /// state on everybody's machine.
    pub async fn push(
        &self,
        source: &RemoteConfigSource,
        token: &Secret,
        request: &PushRequest,
    ) -> Result<String> {
        if !source.allow_push {
            return Err(Error::Remote(
                "publishing is disabled for this remote; enable it in Settings first".into(),
            ));
        }
        if !request.confirmed {
            return Err(Error::Remote(
                "refusing to publish without an explicit confirmation".into(),
            ));
        }
        // Parsing our own payload before uploading it is cheap insurance
        // against publishing a truncated or unsealed file to every machine.
        Envelope::parse(&request.bytes)?;

        let endpoint = resolve_endpoint(source, true)?;
        let (owner, repo) = match &endpoint {
            Endpoint::GitHubContents { owner, repo, .. }
            | Endpoint::GitHubRaw { owner, repo, .. } => (owner.clone(), repo.clone()),
            Endpoint::Direct { .. } => {
                return Err(Error::Remote(
                    "publishing is only supported for GitHub repositories".into(),
                ))
            }
        };
        let path = normalise_repo_path(&source.path);
        let branch = if source.branch.trim().is_empty() { "main" } else { source.branch.trim() };
        let url = format!("https://api.github.com/repos/{owner}/{repo}/contents/{path}");

        let mut body = serde_json::Map::new();
        body.insert("message".into(), request.message.clone().into());
        body.insert("content".into(), crate::crypto::base64_for_upload(&request.bytes).into());
        body.insert("branch".into(), branch.into());
        if let Some(sha) = &request.replaces_sha {
            // Without this, GitHub refuses to overwrite an existing file. With
            // a stale value it refuses too, which is exactly the optimistic
            // concurrency check we want: two machines publishing at once must
            // not silently clobber each other.
            body.insert("sha".into(), sha.clone().into());
        }

        let value = token
            .expose_str()
            .ok_or_else(|| Error::Remote("the access token is not valid UTF-8".into()))?;
        let response = self
            .http
            .put(&url)
            .header("Authorization", format!("Bearer {value}"))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .json(&serde_json::Value::Object(body))
            .send()
            .await
            .map_err(|e| {
                Error::Remote(format!(
                    "could not reach the remote: {}",
                    crate::redact::scrub(&e.to_string())
                ))
            })?;

        let status = response.status();
        if !status.is_success() {
            return Err(remote_status_error(status, &url));
        }
        let parsed: serde_json::Value = response
            .json()
            .await
            .map_err(|e| Error::Remote(format!("could not read the response: {e}")))?;
        Ok(parsed
            .get("commit")
            .and_then(|c| c.get("sha"))
            .and_then(|s| s.as_str())
            .unwrap_or_default()
            .to_string())
    }
}

/// Turn an HTTP status into an error a user can act on.
fn remote_status_error(status: reqwest::StatusCode, url: &str) -> Error {
    let hint = match status.as_u16() {
        401 | 403 => "the token is missing, expired, or lacks `contents` access to this repository",
        404 => {
            "the repository, branch, or file path does not exist (a private repository \
                without a token also looks like a 404)"
        }
        409 => "the file changed on the remote since it was last fetched; pull again first",
        422 => "GitHub rejected the request; the branch may not exist",
        _ => "the remote rejected the request",
    };
    Error::Remote(format!("{status} from {url}: {hint}"))
}

/// GitHub's Contents API returns either the raw file (when `Accept:
/// application/vnd.github.raw` is honoured) or a JSON object with base64
/// content. Handle both, and pull the blob SHA out of the JSON shape so a
/// later push can overwrite the right blob.
fn decode_body(body: &[u8]) -> Result<(Vec<u8>, Option<String>)> {
    // A vault file starts with `{`, and so does the API wrapper, so sniffing
    // the first byte is not enough; look for the wrapper's marker fields.
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) {
        let is_wrapper = value.get("content").and_then(|c| c.as_str()).is_some()
            && value.get("encoding").and_then(|e| e.as_str()) == Some("base64");
        if is_wrapper {
            let encoded: String = value
                .get("content")
                .and_then(|c| c.as_str())
                .unwrap_or_default()
                // GitHub wraps base64 at 60 columns.
                .split_whitespace()
                .collect();
            let decoded = crate::crypto::base64_from_download(&encoded)?;
            let sha = value.get("sha").and_then(|s| s.as_str()).map(|s| s.to_string());
            return Ok((decoded, sha));
        }
    }
    Ok((body.to_vec(), None))
}

// ---------------------------------------------------------------------------
// Diffing
// ---------------------------------------------------------------------------

/// One changed object in a configuration diff.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Change {
    pub id: Uuid,
    /// Name as it appears in the incoming configuration, or the local one for
    /// a removal.
    pub name: String,
}

/// Added, removed and modified objects of one kind.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct ChangeSet {
    pub added: Vec<Change>,
    pub removed: Vec<Change>,
    pub modified: Vec<Change>,
}

impl ChangeSet {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.modified.is_empty()
    }
    pub fn total(&self) -> usize {
        self.added.len() + self.removed.len() + self.modified.len()
    }
}

/// What applying a pulled configuration would change locally.
///
/// Exists so the GUI can show the user what they are about to accept. A
/// config pull replaces job definitions on a machine that is backing up real
/// data; "trust me" is not an acceptable interaction.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct ConfigDiff {
    pub jobs: ChangeSet,
    pub destinations: ChangeSet,
    pub providers: ChangeSet,
    pub projects: ChangeSet,
    /// True when the incoming document targets a different machine identity.
    /// Not an error — a shared vault is expected to contain other machines'
    /// settings — but the GUI must say so before the user's `machine.slug`
    /// changes underneath their existing backups.
    pub machine_identity_changes: bool,
}

impl ConfigDiff {
    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
            && self.destinations.is_empty()
            && self.providers.is_empty()
            && self.projects.is_empty()
            && !self.machine_identity_changes
    }

    pub fn total(&self) -> usize {
        self.jobs.total()
            + self.destinations.total()
            + self.providers.total()
            + self.projects.total()
    }
}

/// Compare two configurations by object identity.
///
/// Matching is by UUID, never by name: renaming a job is a modification, not a
/// delete plus an add, and treating it as the latter would make the diff
/// unreadable exactly when it matters. "Modified" is decided by comparing the
/// canonical JSON of each object, which catches every field without this
/// function having to know what the fields are — including fields added by a
/// later release.
pub fn diff_configs(local: &Config, incoming: &Config) -> Result<ConfigDiff> {
    Ok(ConfigDiff {
        jobs: diff_set(
            local.jobs.iter().map(|j| (j.id, j.name.clone(), j)),
            incoming.jobs.iter().map(|j| (j.id, j.name.clone(), j)),
        )?,
        destinations: diff_set(
            local.destinations.iter().map(|d| (d.id, d.name.clone(), d)),
            incoming.destinations.iter().map(|d| (d.id, d.name.clone(), d)),
        )?,
        providers: diff_set(
            local.providers.iter().map(|p| (p.id, p.name.clone(), p)),
            incoming.providers.iter().map(|p| (p.id, p.name.clone(), p)),
        )?,
        projects: diff_set(
            local.projects.iter().map(|p| (p.id, p.name.clone(), p)),
            incoming.projects.iter().map(|p| (p.id, p.name.clone(), p)),
        )?,
        machine_identity_changes: local.machine.id != incoming.machine.id
            || local.machine.slug != incoming.machine.slug,
    })
}

fn diff_set<'a, T: serde::Serialize + 'a>(
    local: impl Iterator<Item = (Uuid, String, &'a T)>,
    incoming: impl Iterator<Item = (Uuid, String, &'a T)>,
) -> Result<ChangeSet> {
    let to_map = |items: &mut dyn Iterator<Item = (Uuid, String, &'a T)>| -> Result<BTreeMap<Uuid, (String, String)>> {
        let mut map = BTreeMap::new();
        for (id, name, value) in items {
            let json = serde_json::to_string(value)
                .map_err(|e| Error::Remote(format!("could not compare configurations: {e}")))?;
            map.insert(id, (name, json));
        }
        Ok(map)
    };
    let mut local = local;
    let mut incoming = incoming;
    let local = to_map(&mut local)?;
    let incoming = to_map(&mut incoming)?;

    let mut set = ChangeSet::default();
    for (id, (name, json)) in &incoming {
        match local.get(id) {
            None => set.added.push(Change { id: *id, name: name.clone() }),
            Some((_, local_json)) if local_json != json => {
                set.modified.push(Change { id: *id, name: name.clone() })
            }
            Some(_) => {}
        }
    }
    for (id, (name, _)) in &local {
        if !incoming.contains_key(id) {
            set.removed.push(Change { id: *id, name: name.clone() });
        }
    }
    Ok(set)
}

// ---------------------------------------------------------------------------
// Pull
// ---------------------------------------------------------------------------

/// Confirmations the user has explicitly given for this pull.
///
/// Every field defaults to the safe answer, so `PullOptions::default()` is the
/// strict policy. Each one corresponds to a question a human has to be asked
/// in words — "this is older than what you have, really go back?" — which is
/// why they are not inferred.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PullOptions {
    /// Accept a vault older than the local one. See [`verify_pull`].
    pub allow_rollback: bool,
    /// Accept a vault that is not the local one at all. See [`apply_pull`].
    pub allow_different_vault: bool,
}

/// How the incoming vault compares in age with the local one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Freshness {
    /// Strictly newer than the local vault.
    Newer,
    /// Sealed at the same instant — almost always the identical file.
    Same,
    /// Older than the local vault. Accepting it is a rollback.
    Older,
    /// A different vault, so the two timestamps are not comparable.
    Unrelated,
}

/// A verified pull, ready to be shown to the user and then applied.
///
/// Holding one of these is proof that the bytes parsed, that any pinned
/// signature checked out, and that they decrypted with the supplied
/// passphrase. Nothing has been written yet.
#[derive(Debug)]
pub struct PullPlan {
    /// The bytes to write, once the user accepts.
    bytes: Vec<u8>,
    /// Where they came from.
    pub source_url: String,
    /// Blob SHA, for a later push.
    pub sha: Option<String>,
    /// What accepting them would change.
    pub diff: ConfigDiff,
    /// The incoming configuration, when the remote vault published one. It has
    /// already been validated: see [`verify_pull`].
    pub incoming_config: Option<Config>,
    /// Non-fatal complaints about the incoming configuration, to show next to
    /// the diff.
    pub incoming_warnings: Vec<crate::config::Issue>,
    /// Identity of the incoming vault.
    pub vault_id: Uuid,
    /// True when the incoming vault is a different vault, not a newer version
    /// of the local one. [`apply_pull`] refuses these by default.
    pub different_vault: bool,
    /// How the incoming vault compares in age with the local one.
    pub freshness: Freshness,
    /// Signed age difference between the two vaults: positive when the
    /// incoming one is newer. Lets the GUI say "this is 3 days older than what
    /// you have" rather than just refusing.
    pub age_delta: chrono::Duration,
    /// True when this plan was built with an explicit rollback confirmation.
    pub rollback_confirmed: bool,
    /// True when the incoming configuration carried its own
    /// [`crate::model::RemoteConfigSource`], which [`apply_pull`] will discard
    /// in favour of the local one. Worth showing: it is either a publisher
    /// that has not been updated, or an attempt to repoint this machine.
    pub incoming_remote_ignored: bool,
    /// Handles present remotely but not locally, and vice versa.
    pub secrets_added: Vec<String>,
    pub secrets_removed: Vec<String>,
}

impl PullPlan {
    /// The verified sealed bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// Verify fetched bytes against the local state and produce a plan.
///
/// This is where the untrusted blob stops being untrusted, and the order is
/// the security property:
///
/// 1. **Parse.** Bounded, allocation-checked, no cryptography.
/// 2. **Check the signature**, if `trusted_signers` is non-empty. A pinned
///    list that cannot be checked — which is the case in any build without an
///    Ed25519 implementation — is a **rejection**, not a pass. A security
///    control must not evaporate in the build where it is inconvenient.
/// 3. **Decrypt** with the passphrase the user just typed. This is the real
///    authentication: only someone holding the master passphrase could have
///    produced a vault that opens.
/// 4. **Diff**, and hand the result back for a human to look at.
pub fn verify_pull(
    fetched: &FetchedVault,
    local_config: &Config,
    local_vault: &Vault,
    source: &RemoteConfigSource,
    passphrase: &Secret,
) -> Result<PullPlan> {
    verify_pull_with(
        fetched,
        local_config,
        local_vault,
        source,
        passphrase,
        &PullOptions::default(),
    )
}

/// [`verify_pull`] with explicit confirmations from the user.
///
/// The only way to accept a rollback, and the reason it is a separate function
/// rather than a flag on the common one: the strict behaviour has the short
/// name, so a caller gets it by not thinking about it.
pub fn verify_pull_with(
    fetched: &FetchedVault,
    local_config: &Config,
    local_vault: &Vault,
    source: &RemoteConfigSource,
    passphrase: &Secret,
    options: &PullOptions,
) -> Result<PullPlan> {
    let envelope = Envelope::parse(&fetched.bytes)?;

    if !source.trusted_signers.is_empty() {
        verify_signature(&envelope, &source.trusted_signers)?;
    }

    // Decrypting is what proves the bytes came from someone with the master
    // passphrase. Until this succeeds we know nothing about them.
    let incoming = Vault::unlock(&fetched.bytes, passphrase)?;

    // Freshness. Both timestamps are inside the AEAD's associated data, so
    // neither can be edited without destroying the ciphertext — but an
    // attacker does not need to edit anything to replay a blob the user
    // published last month, and that blob is authentic in every sense.
    let different_vault = incoming.id() != local_vault.id();
    let local_time = local_vault.header().updated_at;
    let incoming_time = incoming.header().updated_at;
    let age_delta = incoming_time - local_time;
    let freshness = if different_vault {
        Freshness::Unrelated
    } else if age_delta > chrono::Duration::zero() {
        Freshness::Newer
    } else if age_delta.is_zero() {
        Freshness::Same
    } else {
        Freshness::Older
    };
    if freshness == Freshness::Older && !options.allow_rollback {
        return Err(Error::Remote(format!(
            "the remote is serving an older version of this vault: it was sealed {} \
             before the copy already on this machine ({incoming_time} against \
             {local_time}). Accepting it would undo everything changed since, which \
             may include a credential that was rotated away or a destination that was \
             deleted. Confirm a rollback explicitly if that is really what you want.",
            humantime::format_duration((-age_delta).to_std().unwrap_or(std::time::Duration::ZERO))
        )));
    }

    let mut incoming_config = incoming.embedded_config()?.cloned();

    // Validate before anything is written, not after. `apply_pull` replaces
    // the vault first and the configuration second; discovering the
    // configuration is unusable at that point would leave the machine with a
    // new vault and an old, now-mismatched config. Validating here means
    // `apply_pull` cannot fail halfway.
    let mut incoming_warnings = Vec::new();
    let mut incoming_remote_ignored = false;
    if let Some(config) = &mut incoming_config {
        // The remote block is machine-local and never travels. Strip it here,
        // before validation and before the diff, so that neither the plan the
        // user reviews nor the config `apply_pull` writes can carry a
        // publisher-supplied `trusted_signers` or `url`.
        incoming_remote_ignored = config.remote.is_some();
        config.remote = local_config.remote.clone();
        crate::config::normalise(config);
        let report = crate::config::validate(config);
        if !report.is_ok() {
            return Err(Error::Remote(format!(
                "the configuration published at this remote is not valid, so it has not been \
                 applied: {}",
                report.errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; ")
            )));
        }
        incoming_warnings = report.warnings;
    }

    let diff = match &incoming_config {
        Some(config) => diff_configs(local_config, config)?,
        None => ConfigDiff::default(),
    };

    let (secrets_added, secrets_removed) = match local_vault.list_refs() {
        Ok(local_refs) => {
            let remote_refs = incoming.list_refs()?;
            let local_set: std::collections::BTreeSet<_> =
                local_refs.iter().map(|r| r.as_str().to_string()).collect();
            let remote_set: std::collections::BTreeSet<_> =
                remote_refs.iter().map(|r| r.as_str().to_string()).collect();
            (
                remote_set.difference(&local_set).cloned().collect(),
                local_set.difference(&remote_set).cloned().collect(),
            )
        }
        // A locked local vault cannot be compared, and that is fine: the diff
        // of secret *handles* is a convenience, not a safety check.
        Err(_) => (Vec::new(), Vec::new()),
    };

    Ok(PullPlan {
        bytes: fetched.bytes.clone(),
        source_url: fetched.source_url.clone(),
        sha: fetched.sha.clone(),
        diff,
        incoming_config,
        incoming_warnings,
        vault_id: incoming.id(),
        different_vault,
        freshness,
        age_delta,
        rollback_confirmed: options.allow_rollback && freshness == Freshness::Older,
        incoming_remote_ignored,
        secrets_added,
        secrets_removed,
    })
}

/// Check a vault's detached signature against a pinned signer list.
///
/// # Errors
///
/// [`Error::Remote`] when the vault carries no signature but signers are
/// pinned, when the signer is not on the pinned list, when the embedded public
/// key does not hash to the fingerprint it claims, or when the signature does
/// not verify over [`Envelope::signing_payload`]. Every one of those is a
/// rejection; there is no path through this function that reports a problem
/// and continues.
pub fn verify_signature(envelope: &Envelope, trusted_signers: &[String]) -> Result<()> {
    let Some(signature) = &envelope.signature else {
        return Err(Error::Remote(
            "this remote pins trusted signers, but the vault it served is not signed".into(),
        ));
    };
    let trusted =
        trusted_signers.iter().any(|s| s.trim().eq_ignore_ascii_case(signature.signer.trim()));
    if !trusted {
        return Err(Error::Remote(format!(
            "the vault was signed by {}, which is not in this remote's trusted signer list",
            signature.signer
        )));
    }
    // `signing::verify` re-derives the fingerprint from the embedded public key
    // and refuses if it does not match `signer`, so passing the pinning check
    // above and passing this check cannot happen under two different keys.
    let payload = envelope.signing_payload()?;
    signing::verify(&signature.signer, &signature.public_key, &payload, &signature.signature)
        .map_err(|e| Error::Remote(format!("the vault's signature did not verify: {e}")))
}

/// Apply a verified plan: back up the local vault, then replace it.
///
/// The backup is taken from the live file, not from memory, so it is a true
/// copy of what the user had. Only after the replacement succeeds is the
/// configuration written, and only when the incoming vault actually published
/// one — a vault with no embedded configuration updates the secrets and leaves
/// local job definitions alone, which is the right behaviour for a machine
/// that shares credentials but not schedules.
///
/// # Errors
///
/// [`Error::Remote`] when the plan describes a *different* vault. `vault_id`
/// exists precisely so that "a newer version of mine" and "a stranger's key
/// material" can be told apart, and the refusal has to live here rather than
/// in a confirmation dialog somewhere else in the tree — a check that only
/// exists in one caller's UI is not a check. Use [`apply_pull_with`] and
/// [`PullOptions::allow_different_vault`] when adopting another vault really
/// is the intent, which it is exactly once: joining an existing installation.
pub fn apply_pull(store: &mut Store, plan: &PullPlan) -> Result<()> {
    apply_pull_with(store, plan, &PullOptions::default())
}

/// [`apply_pull`] with explicit confirmations from the user.
pub fn apply_pull_with(store: &mut Store, plan: &PullPlan, options: &PullOptions) -> Result<()> {
    if plan.different_vault && !options.allow_different_vault {
        return Err(Error::Remote(format!(
            "the remote is serving a different vault ({}) from the one on this machine \
             ({}). This is not a newer version of your configuration; it is another \
             installation's key material, and applying it would replace every \
             credential you hold. Confirm explicitly if you are deliberately joining \
             that installation.",
            plan.vault_id,
            store.vault().id()
        )));
    }
    if plan.freshness == Freshness::Older && !plan.rollback_confirmed {
        // Belt and braces: a plan built by `verify_pull` can never be in this
        // state, but a plan is a plain struct and could be constructed or
        // mutated by a caller between verification and application.
        return Err(Error::Remote(
            "this plan describes a rollback that was never confirmed".into(),
        ));
    }

    store.vault_file_mut().replace_with(plan.bytes(), BackupReason::RemotePull)?;
    if let Some(config) = &plan.incoming_config {
        // `verify_pull` already replaced the incoming remote block with the
        // local one; re-assert it here so that a hand-built plan cannot smuggle
        // a `trusted_signers: []` past the check either.
        let mut config = config.clone();
        config.remote = store.config().remote.clone();
        store.set_config(config)?;
    }
    Ok(())
}

/// Write a pulled configuration without touching the vault.
///
/// For the "review the diff, take the jobs, keep my own keys" flow.
///
/// Takes the whole [`PullPlan`] rather than a bare [`Config`] on purpose: a
/// signature that accepted any configuration would accept the publisher's
/// *unsanitised* one, and this is exactly the path on which nobody is
/// re-checking the remote block. The local block is re-asserted here from
/// what is on disk, as it is in [`apply_pull`].
pub fn apply_config_only(paths: &crate::paths::Paths, plan: &PullPlan) -> Result<()> {
    let Some(config) = &plan.incoming_config else {
        return Err(Error::Remote(
            "the vault at this remote does not publish a configuration; there is nothing to apply without also replacing the local secrets"
                .into(),
        ));
    };
    let store = ConfigStore::new(paths.clone());
    // Lenient: a local configuration too broken to validate must not stop the
    // user replacing it with a good one from the remote.
    let (local, _, _) = store.load_lenient()?;
    let mut config = config.clone();
    config.remote = local.remote;
    store.save(&config).map(|_| ())
}

// ---------------------------------------------------------------------------
// Push
// ---------------------------------------------------------------------------

/// An explicit request to publish. Constructing one is the confirmation.
#[derive(Debug, Clone)]
pub struct PushRequest {
    /// The sealed vault to publish.
    pub bytes: Vec<u8>,
    /// Commit message.
    pub message: String,
    /// Blob SHA being replaced, from the last fetch. `None` creates the file.
    pub replaces_sha: Option<String>,
    /// Set by [`PushRequest::confirmed`]; without it, [`RemoteClient::push`]
    /// refuses.
    confirmed: bool,
}

impl PushRequest {
    /// Build an unconfirmed request. It cannot be sent until
    /// [`PushRequest::confirmed`] is called, which exists so that "publish"
    /// cannot happen as a side effect of constructing a value.
    pub fn new(bytes: Vec<u8>, message: impl Into<String>) -> PushRequest {
        PushRequest { bytes, message: message.into(), replaces_sha: None, confirmed: false }
    }

    pub fn replacing(mut self, sha: Option<String>) -> PushRequest {
        self.replaces_sha = sha;
        self
    }

    /// Mark the request as explicitly confirmed by a human.
    pub fn confirmed(mut self) -> PushRequest {
        self.confirmed = true;
        self
    }

    pub fn is_confirmed(&self) -> bool {
        self.confirmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SecretRef;

    fn source(url: &str) -> RemoteConfigSource {
        RemoteConfigSource {
            url: url.into(),
            branch: "main".into(),
            path: "config.sbvault".into(),
            auth: RemoteAuth::None,
            auto_pull: false,
            pull_interval_minutes: 60,
            allow_push: false,
            last_pull_at: None,
            last_known_commit: None,
            trusted_signers: Vec::new(),
        }
    }

    #[test]
    fn github_urls_become_raw_urls_when_anonymous() {
        for url in [
            "https://github.com/andreas/cfg",
            "https://github.com/andreas/cfg.git",
            "https://github.com/andreas/cfg/",
        ] {
            let endpoint = resolve_endpoint(&source(url), false).expect("resolve");
            assert_eq!(
                endpoint.url(),
                "https://raw.githubusercontent.com/andreas/cfg/main/config.sbvault",
                "{url}"
            );
        }
    }

    #[test]
    fn github_urls_use_the_contents_api_when_authenticated() {
        let endpoint =
            resolve_endpoint(&source("https://github.com/andreas/cfg"), true).expect("resolve");
        assert_eq!(
            endpoint.url(),
            "https://api.github.com/repos/andreas/cfg/contents/config.sbvault?ref=main"
        );
    }

    #[test]
    fn other_https_urls_are_used_verbatim() {
        let endpoint =
            resolve_endpoint(&source("https://files.example.com/vaults/mine.sbvault"), false)
                .expect("resolve");
        assert!(matches!(endpoint, Endpoint::Direct { .. }));
        assert_eq!(endpoint.url(), "https://files.example.com/vaults/mine.sbvault");
    }

    #[test]
    fn non_https_urls_are_refused() {
        for url in [
            "http://github.com/a/b",
            "git@github.com:a/b.git",
            "file:///etc/passwd",
            "ftp://example.com/x",
            "",
        ] {
            assert!(resolve_endpoint(&source(url), false).is_err(), "{url} must be refused");
        }
    }

    #[test]
    fn repository_paths_cannot_escape_the_repository() {
        let mut s = source("https://github.com/andreas/cfg");
        s.path = "../../../other/repo/secrets".into();
        let endpoint = resolve_endpoint(&s, false).expect("resolve");
        assert!(!endpoint.url().contains(".."), "{}", endpoint.url());
        assert_eq!(
            endpoint.url(),
            "https://raw.githubusercontent.com/andreas/cfg/main/other/repo/secrets"
        );

        let mut s = source("https://github.com/andreas/cfg");
        s.path = "///".into();
        assert!(resolve_endpoint(&s, false).is_err(), "an empty path must be refused");
    }

    #[test]
    fn a_pinned_signer_list_fails_closed_without_a_signature() {
        let mut vault = Vault::create_unchecked(
            &Secret::from_str("pass"),
            crate::crypto::KdfParams::insecure_for_tests().expect("kdf"),
        )
        .expect("vault");
        let bytes = vault.seal().expect("seal");
        let envelope = Envelope::parse(&bytes).expect("parse");

        let err = verify_signature(&envelope, &["abc123".to_string()]).expect_err("must reject");
        assert!(matches!(err, Error::Remote(_)), "{err:?}");
        assert!(format!("{err}").contains("not signed"));
    }

    #[test]
    fn an_untrusted_signer_is_rejected_before_the_algorithm_is_consulted() {
        let mut vault = Vault::create_unchecked(
            &Secret::from_str("pass"),
            crate::crypto::KdfParams::insecure_for_tests().expect("kdf"),
        )
        .expect("vault");
        let bytes = vault.seal().expect("seal");
        let mut envelope = Envelope::parse(&bytes).expect("parse");
        envelope.signature = Some(crate::crypto::VaultSignature {
            algorithm: crate::crypto::SignatureAlgorithm::Ed25519,
            signer: "00000000000000000000000000000000".into(),
            public_key: vec![0u8; 32],
            signature: vec![0u8; 64],
        });
        let err = verify_signature(&envelope, &["11111111111111111111111111111111".to_string()])
            .expect_err("must reject");
        assert!(format!("{err}").contains("trusted signer list"), "{err}");
    }

    #[test]
    fn diffing_matches_by_id_so_a_rename_is_a_modification() {
        let mut local = Config::default();
        let mut job = crate::model::Job {
            id: Uuid::from_u128(1),
            name: "nightly".into(),
            project_id: None,
            description: String::new(),
            sources: Vec::new(),
            destination_ids: Vec::new(),
            schedule: crate::model::Schedule::Manual,
            exclusions: Default::default(),
            bandwidth: None,
            retention: None,
            enabled: true,
            timeout_minutes: None,
            hooks: Default::default(),
            continue_on_destination_error: true,
            created_at: chrono::Utc::now(),
            tags: Vec::new(),
        };
        local.jobs.push(job.clone());

        let mut incoming = local.clone();
        incoming.jobs[0].name = "nightly-renamed".into();
        let diff = diff_configs(&local, &incoming).expect("diff");
        assert_eq!(diff.jobs.modified.len(), 1);
        assert!(diff.jobs.added.is_empty());
        assert!(diff.jobs.removed.is_empty());
        assert_eq!(diff.jobs.modified[0].name, "nightly-renamed");

        // A genuinely new job is an addition, and a dropped one a removal.
        job.id = Uuid::from_u128(2);
        job.name = "weekly".into();
        let mut incoming = local.clone();
        incoming.jobs.push(job);
        incoming.jobs.remove(0);
        let diff = diff_configs(&local, &incoming).expect("diff");
        assert_eq!(diff.jobs.added.len(), 1);
        assert_eq!(diff.jobs.removed.len(), 1);
        assert!(diff.jobs.modified.is_empty());

        // Identical documents diff to nothing.
        let diff = diff_configs(&local, &local.clone()).expect("diff");
        assert!(diff.is_empty(), "{diff:?}");
    }

    #[test]
    fn push_requires_both_the_setting_and_an_explicit_confirmation() {
        let request = PushRequest::new(vec![1, 2, 3], "publish");
        assert!(!request.is_confirmed(), "construction alone must not authorise a publish");
        assert!(request.clone().confirmed().is_confirmed());
    }

    #[test]
    fn contents_api_wrappers_and_raw_bodies_both_decode() {
        let raw = br#"{"magic":"SBVAULT"}"#;
        let (bytes, sha) = decode_body(raw).expect("raw");
        assert_eq!(bytes, raw);
        assert!(sha.is_none());

        let wrapper = serde_json::json!({
            "content": "eyJtYWdpYyI6\nIlNCVkFVTFQifQ==",
            "encoding": "base64",
            "sha": "deadbeef",
        });
        let body = serde_json::to_vec(&wrapper).expect("wrapper");
        let (bytes, sha) = decode_body(&body).expect("wrapped");
        assert_eq!(bytes, br#"{"magic":"SBVAULT"}"#);
        assert_eq!(sha.as_deref(), Some("deadbeef"));
    }

    #[test]
    fn secret_handle_diffs_are_reported_without_the_secrets() {
        let kdf = || crate::crypto::KdfParams::insecure_for_tests().expect("kdf");
        let mut local = Vault::create_unchecked(&Secret::from_str("pass"), kdf()).expect("local");
        local.put(SecretRef("s3.access:1".into()), Secret::from_str("LOCALKEY")).expect("put");
        local.seal().expect("seal");

        let mut remote = Vault::create_unchecked(&Secret::from_str("pass"), kdf()).expect("remote");
        remote.put(SecretRef("s3.access:2".into()), Secret::from_str("REMOTEKEY")).expect("put");
        let bytes = remote.seal().expect("seal");

        let fetched = FetchedVault {
            bytes,
            source_url: "https://example.com/config.sbvault".into(),
            sha: None,
        };
        let plan = verify_pull(
            &fetched,
            &Config::default(),
            &local,
            &source("https://github.com/a/b"),
            &Secret::from_str("pass"),
        )
        .expect("verify");

        assert_eq!(plan.secrets_added, vec!["s3.access:2".to_string()]);
        assert_eq!(plan.secrets_removed, vec!["s3.access:1".to_string()]);
        assert!(plan.different_vault, "two independently created vaults are not the same vault");
        let rendered = format!("{plan:?}");
        assert!(!rendered.contains("REMOTEKEY"), "{rendered}");
        assert!(!rendered.contains("LOCALKEY"), "{rendered}");
    }

    #[test]
    fn a_pull_that_does_not_decrypt_is_rejected() {
        let mut remote = Vault::create_unchecked(
            &Secret::from_str("their-pass"),
            crate::crypto::KdfParams::insecure_for_tests().expect("kdf"),
        )
        .expect("remote");
        let bytes = remote.seal().expect("seal");
        let local = Vault::create_unchecked(
            &Secret::from_str("my-pass"),
            crate::crypto::KdfParams::insecure_for_tests().expect("kdf"),
        )
        .expect("local");

        let fetched = FetchedVault {
            bytes,
            source_url: "https://example.com/config.sbvault".into(),
            sha: None,
        };
        let err = verify_pull(
            &fetched,
            &Config::default(),
            &local,
            &source("https://github.com/a/b"),
            &Secret::from_str("my-pass"),
        )
        .expect_err("must not accept a vault it cannot open");
        assert!(matches!(err, Error::BadPassphrase));
    }
}
