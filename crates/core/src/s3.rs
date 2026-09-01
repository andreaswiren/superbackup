//! A small, direct S3 client: list buckets, list objects, and say precisely
//! why not.
//!
//! # Why this exists at all
//!
//! Everything else superbackup does with object storage goes through kopia,
//! and that is the right layering — kopia owns the repository format, the
//! chunking and the encryption, and this module has no business duplicating
//! any of it. But kopia can do exactly one thing with a bucket: *open a
//! repository in it*. That leaves three user-visible holes, all of them the
//! same hole:
//!
//! 1. **Credentials could not be checked on their own.** "Test provider" had
//!    to borrow a destination that used the provider, so before the first
//!    destination existed it answered "there is nothing to test against" —
//!    precisely when someone pasting fresh StorJ keys wants to know whether
//!    they pasted them correctly.
//! 2. **Bucket names had to be typed from memory**, because nothing could ask
//!    the account what it owned.
//! 3. **Every failure looked the same.** A wrong secret key, a wrong endpoint,
//!    a bucket that does not exist and a clock that is twenty minutes fast all
//!    produced one unhelpful sentence, and the user debugged the wrong thing.
//!
//! `ListBuckets` closes all three: it is authenticated, so a successful call
//! *is* a credential check, and it returns the names.
//!
//! # Why there is no AWS SDK here
//!
//! `deny.toml` bans a second TLS stack and polices the dependency tree, and
//! the surface actually needed is two GET requests. `reqwest` with rustls is
//! already a dependency; `sha2` and `hmac` are already in the tree. The whole
//! cost of this module is the signing, which is written out below and tested
//! against AWS's own published vectors — see the tests at the bottom of the
//! file.
//!
//! # Why the XML is parsed by hand
//!
//! `quick-xml` 0.30 is in the dependency tree with two denial-of-service
//! advisories that `deny.toml` accepts **on the grounds that the only XML this
//! process parses is D-Bus introspection data from the Linux accessibility
//! bus**. S3 responses are XML from the network, which is a different thing
//! entirely, and routing them through a general-purpose parser would either
//! make that argument false or require adding a *second*, current quick-xml as
//! a real runtime dependency (0.41 is in the lock file only as a Linux-only
//! build dependency of `wayland-scanner`, so it is not linked into the shipped
//! binary and would not come for free).
//!
//! Instead [`xml`] is a scanner for the two response shapes this module
//! actually reads, with hard caps on nesting depth, element count and text
//! length, no recursion, and no allocation proportional to anything but the
//! input. It cannot be the DoS the advisories describe because it has no
//! quadratic path and no unbounded allocation, and the transport refuses a
//! body over [`MAX_RESPONSE_BYTES`] before the parser ever sees it.
//!
//! # Discipline this module inherits
//!
//! Same rules as [`crate::kopia::install`], for the same reasons: explicit
//! connect and request timeouts, a hard cap on the response body enforced
//! while streaming, redirects refused rather than followed, and every message
//! that can reach a log, an IPC frame or the screen passed through
//! [`crate::redact::scrub`].
//!
//! # What this module deliberately does not do
//!
//! No writes. Nothing here creates, deletes or uploads anything. It reads two
//! listings, and a bug in it therefore cannot destroy a backup.

use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

use crate::model::{ProviderKind, StorageProvider};
use crate::redact;
use crate::secret::Secret;

// ---------------------------------------------------------------------------
// Bounds
// ---------------------------------------------------------------------------

/// Ceiling on any response body.
///
/// A `ListAllMyBucketsResult` for a thousand buckets is well under 200 KB and
/// a `ListBucketResult` capped at 1000 keys is under 500 KB. Four megabytes is
/// therefore an order of magnitude of headroom, and small enough that a
/// hostile or broken endpoint cannot make the daemon allocate its way out of
/// memory.
pub const MAX_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;

/// Largest `max-keys` S3 itself honours. Anything above is clamped rather
/// than rejected, because a caller asking for more is not making an error, it
/// is asking for everything.
pub const MAX_KEYS_PER_PAGE: u32 = 1000;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// SHA-256 of the empty string, which is the payload hash for every request
/// this module makes.
const EMPTY_PAYLOAD_SHA256: &str =
    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

/// Key the write probe uses. Namespaced so it can never collide with a kopia
/// blob, and stable so a probe left behind by a crash is recognisable.
pub const WRITE_PROBE_KEY: &str = ".superbackup-write-test";
const WRITE_PROBE_BODY: &[u8] = b"superbackup";

const ALGORITHM: &str = "AWS4-HMAC-SHA256";
const SERVICE: &str = "s3";

/// The `User-Agent` sent to the object store.
fn user_agent() -> String {
    format!("superbackup/{} (+https://github.com/andreaswiren/superbackup)", crate::VERSION)
}

/// Run third-party text through the redactor before it is stored anywhere.
///
/// Applied at construction rather than only at display, so an [`S3Error`] can
/// never *hold* credential-shaped text that some future caller formats without
/// thinking about it.
fn safe(text: impl AsRef<str>) -> String {
    redact::scrub(text.as_ref()).into_owned()
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Why a request did not produce an answer.
///
/// The variants exist because the user's next action is different for each
/// one. "It didn't work" is the least useful thing this module could say, and
/// collapsing a fifteen-minute clock skew into the same message as a wrong
/// secret key sends people to rewrite credentials that were always correct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum S3Error {
    /// The endpoint's host name does not resolve.
    Dns { host: String, detail: String },
    /// The host resolved but nothing accepted a connection.
    Connect { host: String, detail: String },
    /// TLS could not be established — a wrong port, a proxy in the way, or a
    /// certificate this machine does not trust.
    Tls { host: String, detail: String },
    /// The request took longer than [`REQUEST_TIMEOUT`].
    Timeout { host: String },
    /// The access key id is not one this endpoint knows.
    InvalidAccessKeyId,
    /// The access key id is known; the secret key did not produce a matching
    /// signature.
    SignatureDoesNotMatch,
    /// This computer's clock is too far from the server's for S3 to accept a
    /// signature. Not a credential problem at all.
    RequestTimeTooSkewed { server_time: Option<String> },
    /// The credentials are valid and were authenticated, but the policy
    /// attached to them does not permit this call.
    ///
    /// **This is not a bad key.** A key scoped to a single bucket is the
    /// normal, recommended shape, and such a key cannot
    /// `s3:ListAllMyBuckets`. Reporting it as a failed credential check would
    /// be false; see [`S3Error::credentials_accepted`].
    AccessDenied { operation: &'static str },
    /// No bucket of that name at this endpoint.
    NoSuchBucket { bucket: String },
    /// The bucket lives in another region, or behind another endpoint.
    Redirected { region: Option<String>, endpoint: Option<String> },
    /// The endpoint answered with an S3 error this module has no special
    /// handling for.
    Service { code: String, message: String, status: u16 },
    /// Something answered, but not like an S3 endpoint: an HTML login page, a
    /// JSON API, a proxy's error page.
    NotS3 { host: String, status: u16, detail: String },
    /// The body was XML but not a shape this module understands, or was cut
    /// short.
    Malformed { detail: String },
    /// The response exceeded [`MAX_RESPONSE_BYTES`].
    TooLarge { limit: u64 },
    /// The provider itself cannot be turned into a request: no endpoint, an
    /// unusable host, a missing key.
    Configuration { detail: String },
}

impl S3Error {
    /// True when the endpoint positively authenticated the credentials.
    ///
    /// Only [`S3Error::AccessDenied`] qualifies: the signature was verified
    /// before the policy was consulted, so the key and secret are provably
    /// correct even though this particular call was refused. Everything else
    /// leaves the credentials unproven.
    pub fn credentials_accepted(&self) -> bool {
        matches!(self, S3Error::AccessDenied { .. })
    }

    /// True when trying again later could plausibly succeed with no change by
    /// the user — an outage, a flaky link, a laptop on a train.
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            S3Error::Dns { .. }
                | S3Error::Connect { .. }
                | S3Error::Timeout { .. }
                | S3Error::Service { status: 500..=599, .. }
        )
    }

    /// A complete sentence for the user, already redacted.
    pub fn message(&self) -> String {
        let text = match self {
            S3Error::Dns { host, .. } => format!(
                "The address {host} could not be looked up. Check the endpoint for a typo, and \
                 check that this computer is online."
            ),
            S3Error::Connect { host, detail } => format!(
                "Nothing answered at {host} ({detail}). The endpoint may be wrong, or a firewall \
                 or proxy may be blocking the connection."
            ),
            S3Error::Tls { host, detail } => format!(
                "A secure connection to {host} could not be established ({detail}). That usually \
                 means the endpoint is not an HTTPS service on this port, or a proxy is \
                 intercepting the connection with a certificate this computer does not trust."
            ),
            S3Error::Timeout { host } => {
                format!("{host} did not answer within {} seconds.", REQUEST_TIMEOUT.as_secs())
            }
            S3Error::InvalidAccessKeyId => "The access key id was not recognised by this \
                 endpoint. Check that the whole key was copied, and that it belongs to this \
                 provider rather than to another account."
                .to_string(),
            S3Error::SignatureDoesNotMatch => "The access key id was recognised but the secret \
                 key was not accepted. Re-enter the secret key — a truncated paste or a stray \
                 space at either end is the usual cause."
                .to_string(),
            S3Error::RequestTimeTooSkewed { server_time } => {
                let when = match server_time {
                    Some(t) => format!(" The server says the time is {t}."),
                    None => String::new(),
                };
                format!(
                    "This computer's clock is too far out for the storage provider to accept a \
                     request; S3 refuses anything signed more than fifteen minutes away from its \
                     own time.{when} Set the clock (or switch on automatic time) and try again — \
                     the credentials are not the problem."
                )
            }
            S3Error::AccessDenied { operation } => format!(
                "The credentials were accepted, but this key is not allowed to {operation}. That \
                 is normal for a key scoped to a single bucket, and it does not mean the key is \
                 wrong."
            ),
            S3Error::NoSuchBucket { bucket } => {
                format!("There is no bucket named \"{bucket}\" at this endpoint.")
            }
            S3Error::Redirected { region, endpoint } => match (region, endpoint) {
                (Some(region), _) => format!(
                    "The storage provider says this bucket lives in region \"{region}\". Change \
                     the provider's region to match."
                ),
                (None, Some(endpoint)) => format!(
                    "The storage provider redirected the request to {endpoint}. Use that as the \
                     endpoint instead."
                ),
                (None, None) => "The storage provider redirected the request, which usually \
                     means the endpoint or the region is wrong for this bucket."
                    .to_string(),
            },
            S3Error::Service { code, message, status } => format!(
                "The storage provider refused the request: {code} (HTTP {status}). {}",
                end_with_stop(message)
            ),
            S3Error::NotS3 { host, status, detail } => format!(
                "{host} answered with HTTP {status}, but not as an S3 service ({detail}). Check \
                 the endpoint address — a console or dashboard URL is not the S3 endpoint."
            ),
            S3Error::Malformed { detail } => format!(
                "The storage provider's answer could not be read ({detail}). Either the endpoint \
                 is not S3-compatible, or the reply was cut short."
            ),
            S3Error::TooLarge { limit } => format!(
                "The storage provider's answer was larger than the {limit}-byte limit and was \
                 discarded."
            ),
            S3Error::Configuration { detail } => detail.clone(),
        };
        // Belt and braces. Every variant is already built from redacted parts;
        // running the assembled sentence through once more means a variant
        // added later cannot leak by forgetting.
        safe(text)
    }

    /// One short line of what to do next, where there is something specific.
    pub fn hint(&self) -> Option<&'static str> {
        match self {
            S3Error::RequestTimeTooSkewed { .. } => {
                Some("Windows: Settings → Time & language → Date & time → Sync now.")
            }
            S3Error::AccessDenied { .. } => {
                Some("Type the bucket name instead of picking it from a list.")
            }
            S3Error::InvalidAccessKeyId | S3Error::SignatureDoesNotMatch => {
                Some("Generate a fresh key pair if you cannot recover the original.")
            }
            S3Error::NotS3 { .. } => Some("StorJ's S3 endpoint is https://gateway.storjshare.io."),
            _ => None,
        }
    }
}

/// Make a third-party sentence end like one, so the assembled message reads as
/// prose rather than trailing off. An empty message becomes a full stop-free
/// nothing rather than a bare period.
fn end_with_stop(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return "No further detail was given.".to_string();
    }
    if trimmed.ends_with(['.', '!', '?']) {
        trimmed.to_string()
    } else {
        format!("{trimmed}.")
    }
}

impl fmt::Display for S3Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for S3Error {}

impl From<S3Error> for crate::Error {
    fn from(e: S3Error) -> crate::Error {
        match e {
            S3Error::Configuration { .. } => crate::Error::Config(e.message()),
            _ => crate::Error::Remote(e.message()),
        }
    }
}

// ---------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------

/// One HTTP request, fully formed and already signed.
#[derive(Debug, Clone)]
pub struct HttpRequest {
    pub method: &'static str,
    /// Absolute URL, scheme included.
    pub url: String,
    /// Headers to send. `Host` is *not* here: it is derived from the URL by
    /// the transport, and the `host` entry of [`SigningInput::headers`] is
    /// what the signature covers.
    pub headers: Vec<(String, String)>,
    /// Request body. Empty for everything except the write probe.
    #[allow(clippy::doc_markdown)]
    pub body: Vec<u8>,
}

/// What came back.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    /// The `Content-Type`, lowercased, when the server sent one.
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

/// A transport-level failure, before any S3 semantics apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    Dns(String),
    Connect(String),
    Tls(String),
    Timeout,
    TooLarge {
        limit: u64,
    },
    /// A redirect, which this client refuses to follow: the `Authorization`
    /// header is computed for one host and must never be replayed at another.
    Redirect {
        location: Option<String>,
    },
    Other(String),
}

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The single round trip [`S3Client`] needs.
///
/// A trait rather than a hard dependency on `reqwest` so the whole client —
/// signing, error mapping, XML — can be tested without a socket. The tests
/// inject canned responses; production uses [`ReqwestTransport`].
pub trait Transport: fmt::Debug + Send + Sync {
    fn send<'a>(
        &'a self,
        request: HttpRequest,
    ) -> BoxFuture<'a, std::result::Result<HttpResponse, TransportError>>;
}

/// The production transport: rustls, explicit timeouts, a streamed size cap,
/// and no redirect following.
#[derive(Debug, Clone)]
pub struct ReqwestTransport {
    client: reqwest::Client,
    limit: u64,
}

impl ReqwestTransport {
    pub fn new() -> std::result::Result<ReqwestTransport, S3Error> {
        let client = reqwest::Client::builder()
            .user_agent(user_agent())
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            // A signed request is bound to one host by its own signature. A
            // redirect is therefore never something to follow silently: it is
            // either a region/endpoint mismatch the user must know about, or
            // an attempt to replay our request somewhere else.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| S3Error::Configuration {
                detail: safe(format!("the HTTPS client could not be created: {e}")),
            })?;
        Ok(ReqwestTransport { client, limit: MAX_RESPONSE_BYTES })
    }
}

impl Transport for ReqwestTransport {
    fn send<'a>(
        &'a self,
        request: HttpRequest,
    ) -> BoxFuture<'a, std::result::Result<HttpResponse, TransportError>> {
        Box::pin(async move {
            let mut builder = self.client.request(
                reqwest::Method::from_bytes(request.method.as_bytes())
                    .map_err(|e| TransportError::Other(e.to_string()))?,
                &request.url,
            );
            for (name, value) in &request.headers {
                builder = builder.header(name.as_str(), value.as_str());
            }
            if !request.body.is_empty() {
                builder = builder.body(request.body.clone());
            }
            let mut response = builder.send().await.map_err(classify_reqwest_error)?;

            let status = response.status().as_u16();
            if response.status().is_redirection() {
                return Err(TransportError::Redirect {
                    location: response
                        .headers()
                        .get("location")
                        .and_then(|h| h.to_str().ok())
                        .map(|s| s.to_string()),
                });
            }
            let content_type = response
                .headers()
                .get("content-type")
                .and_then(|h| h.to_str().ok())
                .map(|s| s.to_ascii_lowercase());

            if let Some(len) = response.content_length() {
                if len > self.limit {
                    return Err(TransportError::TooLarge { limit: self.limit });
                }
            }
            let mut body: Vec<u8> = Vec::new();
            loop {
                let chunk = response.chunk().await.map_err(classify_reqwest_error)?;
                let Some(chunk) = chunk else { break };
                if body.len() as u64 + chunk.len() as u64 > self.limit {
                    return Err(TransportError::TooLarge { limit: self.limit });
                }
                body.extend_from_slice(&chunk);
            }
            Ok(HttpResponse { status, content_type, body })
        })
    }
}

fn classify_reqwest_error(e: reqwest::Error) -> TransportError {
    // `reqwest` does not expose "this was DNS" or "this was TLS" as flags, so
    // the source chain is flattened once and inspected. Kept as a separate
    // pure function over the flattened text so the classification itself is
    // testable without provoking a real network failure.
    let mut chain = e.to_string();
    let mut source: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(&e);
    let mut depth = 0;
    while let Some(inner) = source {
        chain.push_str(": ");
        chain.push_str(&inner.to_string());
        source = inner.source();
        depth += 1;
        if depth > 8 {
            break;
        }
    }
    classify_chain(&chain, e.is_timeout(), e.is_connect())
}

/// Turn a flattened error chain into the failure the user needs to hear about.
fn classify_chain(chain: &str, is_timeout: bool, is_connect: bool) -> TransportError {
    let lower = chain.to_ascii_lowercase();
    let detail = safe(chain);
    if is_timeout || lower.contains("operation timed out") {
        return TransportError::Timeout;
    }
    if lower.contains("dns error")
        || lower.contains("failed to lookup address")
        || lower.contains("no such host")
        || lower.contains("name or service not known")
        || lower.contains("nodename nor servname")
    {
        return TransportError::Dns(detail);
    }
    if lower.contains("certificate")
        || lower.contains("tls")
        || lower.contains("handshake")
        || lower.contains("unknownissuer")
        || lower.contains("notvalidforname")
    {
        return TransportError::Tls(detail);
    }
    if is_connect
        || lower.contains("connection refused")
        || lower.contains("connection reset")
        || lower.contains("network is unreachable")
    {
        return TransportError::Connect(detail);
    }
    TransportError::Other(detail)
}

// ---------------------------------------------------------------------------
// Credentials
// ---------------------------------------------------------------------------

/// A resolved key pair, ready to sign with.
///
/// Distinct from [`crate::model::S3Credentials`], which holds vault *handles*.
/// This is the resolved form and exists only for as long as one request: both
/// halves are [`Secret`], so they are zeroed on drop and cannot be printed.
/// The access key id is an identifier rather than a secret, but it arrives
/// from the vault as a `Secret` and there is no reason to widen it here.
#[derive(Debug)]
pub struct S3Keys {
    access_key_id: Secret,
    secret_access_key: Secret,
    session_token: Option<Secret>,
}

impl S3Keys {
    pub fn new(access_key_id: Secret, secret_access_key: Secret) -> S3Keys {
        S3Keys { access_key_id, secret_access_key, session_token: None }
    }

    pub fn with_session_token(mut self, token: Option<Secret>) -> S3Keys {
        // An empty token is the same as no token: a rotated long-lived key
        // pair can leave an empty vault entry behind, and sending
        // `x-amz-security-token:` with nothing after it fails every request.
        self.session_token = token.filter(|t| !t.is_empty());
        self
    }

    /// The access key id as text. `None` when it is not valid UTF-8, which
    /// means the vault entry is not an access key at all.
    fn access_key(&self) -> Option<&str> {
        self.access_key_id.expose_str()
    }

    fn token(&self) -> Option<&str> {
        self.session_token.as_ref().and_then(|t| t.expose_str())
    }
}

// ---------------------------------------------------------------------------
// Endpoint and addressing
// ---------------------------------------------------------------------------

/// Where requests go, derived from a [`StorageProvider`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S3Endpoint {
    /// `https` or `http`.
    pub scheme: &'static str,
    /// Authority as it will appear in the `Host` header: host, plus `:port`
    /// when the port is not the scheme's default.
    pub authority: String,
    pub region: String,
    /// Force `host/bucket/key` addressing rather than `bucket.host/key`.
    pub force_path_style: bool,
}

impl S3Endpoint {
    /// Read a provider's connection settings.
    ///
    /// The scheme is decided exactly as [`crate::kopia::driver::s3_endpoint_host`]
    /// decides it for kopia — `tls`, downgraded by an explicit `http://` —
    /// because a bucket must be reached the same way by both code paths. If
    /// this module and kopia disagreed about TLS, "Test provider" would prove
    /// something about a connection the backup never makes.
    pub fn from_provider(provider: &StorageProvider) -> std::result::Result<S3Endpoint, S3Error> {
        let ProviderKind::S3 { endpoint, region, tls, path_style, .. } = &provider.kind;
        let (host, scheme_disables_tls) = crate::kopia::s3_endpoint_host(endpoint);
        if host.is_empty() {
            return Err(S3Error::Configuration {
                detail: "This storage provider has no endpoint. StorJ's is \
                         https://gateway.storjshare.io."
                    .into(),
            });
        }
        let scheme = if *tls && !scheme_disables_tls { "https" } else { "http" };
        let authority = normalise_authority(&host, scheme)?;
        Ok(S3Endpoint {
            scheme,
            authority,
            region: region.trim().to_string(),
            force_path_style: *path_style,
        })
    }

    /// The region to sign with.
    ///
    /// S3 requires a region in the credential scope even where the endpoint
    /// has no concept of one. `us-east-1` is the value every S3-compatible
    /// gateway accepts as "no particular region", which is why it is the
    /// fallback rather than an error.
    fn signing_region(&self) -> &str {
        if self.region.is_empty() {
            "us-east-1"
        } else {
            &self.region
        }
    }

    /// Whether this bucket is addressed as `host/bucket` or `bucket.host`.
    ///
    /// Three things force path style, and any one of them is enough:
    ///
    /// * the provider asks for it;
    /// * the bucket name is not DNS-safe (a dot in the name also breaks
    ///   certificate matching for `bucket.host`, so this is a correctness
    ///   requirement, not a preference);
    /// * the endpoint is not one known to support virtual-hosted addressing.
    ///
    /// The last rule is minio-go's, which is what kopia uses, so both agree
    /// about where an object lives. Only `*.amazonaws.com` is treated as
    /// virtual-host capable; every gateway (StorJ, MinIO, Wasabi, R2) accepts
    /// path style, so defaulting to it cannot produce a request the endpoint
    /// refuses.
    fn uses_path_style(&self, bucket: &str) -> bool {
        if self.force_path_style || !is_dns_compatible_bucket(bucket) {
            return true;
        }
        let host = self.authority.split(':').next().unwrap_or(&self.authority);
        !host.ends_with(".amazonaws.com")
    }
}

/// Normalise `host[:port]` into an authority safe to put in a `Host` header.
///
/// The default port is dropped: `reqwest` derives the `Host` header from the
/// URL and omits `:443` for HTTPS, so signing an authority that kept it would
/// produce a signature over a header the server never sees — which fails
/// identically to a wrong secret key, and would send the user hunting the
/// wrong problem.
fn normalise_authority(host: &str, scheme: &str) -> std::result::Result<String, S3Error> {
    let host = host.trim().trim_end_matches('.');
    let invalid = || S3Error::Configuration {
        detail: format!("\"{host}\" is not a usable endpoint address."),
    };
    if host.is_empty() || host.len() > 255 || host.contains(['/', ' ', '\\', '@', '?', '#']) {
        return Err(invalid());
    }
    // IPv6 literals keep their brackets and their colons.
    if host.starts_with('[') {
        return if host.contains(']') { Ok(host.to_string()) } else { Err(invalid()) };
    }
    let (name, port) = match host.rsplit_once(':') {
        Some((name, port)) => match port.parse::<u16>() {
            Ok(port) => (name, Some(port)),
            Err(_) => return Err(invalid()),
        },
        None => (host, None),
    };
    if name.is_empty()
        || name.starts_with('.')
        || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
    {
        return Err(invalid());
    }
    let default_port = if scheme == "https" { 443 } else { 80 };
    Ok(match port {
        Some(p) if p != default_port => format!("{name}:{p}"),
        _ => name.to_string(),
    })
}

/// Whether a bucket name can be a DNS label of its endpoint.
///
/// Deliberately strict: a name with a dot in it would need a wildcard
/// certificate two levels deep, which no S3 provider issues, so such buckets
/// must be addressed path-style or not at all.
fn is_dns_compatible_bucket(bucket: &str) -> bool {
    (3..=63).contains(&bucket.len())
        && bucket.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !bucket.starts_with('-')
        && !bucket.ends_with('-')
}

/// Reject a bucket name that could change the shape of the request.
///
/// A name is a path segment and, in virtual-hosted mode, part of the host. A
/// slash or a dot-dot in it is either a mistake or an attempt to make one
/// request look like another, and neither should reach the wire.
fn validate_bucket(bucket: &str) -> std::result::Result<(), S3Error> {
    let bad = bucket.is_empty()
        || bucket.len() > 255
        || bucket.contains(['/', '\\', '?', '#', ' ', '@', ':'])
        || bucket.split('.').any(|seg| seg == ".." || seg.is_empty());
    if bad {
        return Err(S3Error::Configuration {
            detail: format!("\"{bucket}\" is not a valid bucket name."),
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Signature Version 4
// ---------------------------------------------------------------------------

/// Everything the signature covers.
///
/// Kept as data rather than being folded into the request builder so that the
/// canonical request and the string to sign can be produced — and compared
/// against AWS's published vectors — without a client, a transport or a
/// network.
#[derive(Debug, Clone)]
pub struct SigningInput {
    pub method: String,
    /// Path, **not** percent-encoded. [`canonical_request`] encodes it once,
    /// which is what S3 requires: unlike every other AWS service, S3 does not
    /// double-encode and does not normalise away `.` or `..` segments.
    pub path: String,
    /// Query parameters, decoded. Order is irrelevant; they are sorted.
    pub query: Vec<(String, String)>,
    /// Headers to sign, including `host`. Names are lowercased and values
    /// whitespace-normalised by [`canonical_request`].
    pub headers: Vec<(String, String)>,
    /// Hex SHA-256 of the request body.
    pub payload_sha256: String,
    pub region: String,
    pub service: String,
    pub timestamp: DateTime<Utc>,
}

/// The three intermediate products of signing, plus the header itself.
///
/// The intermediates are returned rather than discarded because they are
/// exactly what the AWS test vectors publish, so the tests can assert on the
/// real thing instead of on a signature that is merely self-consistent. A
/// self-consistent signer that is wrong is worth nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    pub canonical_request: String,
    pub string_to_sign: String,
    pub signed_headers: String,
    pub signature: String,
    pub authorization: String,
}

/// `YYYYMMDD'T'HHMMSS'Z'`.
pub fn amz_date(timestamp: DateTime<Utc>) -> String {
    timestamp.format("%Y%m%dT%H%M%SZ").to_string()
}

/// `YYYYMMDD`.
pub fn amz_day(timestamp: DateTime<Utc>) -> String {
    timestamp.format("%Y%m%d").to_string()
}

/// Percent-encode per RFC 3986, which is stricter than a URL encoder:
/// `A-Z a-z 0-9 - _ . ~` survive and everything else becomes `%XX` in upper
/// case. A space is `%20`, never `+`.
fn uri_encode(value: &str, encode_slash: bool) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        let c = *byte as char;
        match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => out.push(c),
            '/' if !encode_slash => out.push('/'),
            _ => {
                const HEX: &[u8; 16] = b"0123456789ABCDEF";
                out.push('%');
                out.push(HEX[(byte >> 4) as usize] as char);
                out.push(HEX[(byte & 0x0f) as usize] as char);
            }
        }
    }
    out
}

/// Collapse a header value the way SigV4 requires: trimmed, and every run of
/// whitespace inside it reduced to a single space.
///
/// The written specification carves out an exception for quoted sections;
/// AWS's own published `get-header-value-trim` vector does not honour it and
/// collapses `"a b c"` to `"a b c"` regardless. What servers verify
/// against is the behaviour, not the prose, so the vector wins.
fn canonical_header_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut last_was_space = false;
    for c in value.trim().chars() {
        if c == ' ' || c == '\t' {
            if !last_was_space {
                out.push(' ');
            }
            last_was_space = true;
            continue;
        }
        last_was_space = false;
        out.push(c);
    }
    out
}

/// The canonical request: method, URI, query, headers, signed header list and
/// payload hash, each on its own line.
pub fn canonical_request(input: &SigningInput) -> String {
    let path = if input.path.is_empty() { "/" } else { &input.path };
    let canonical_uri = uri_encode(path, false);

    let mut query: Vec<(String, String)> =
        input.query.iter().map(|(k, v)| (uri_encode(k, true), uri_encode(v, true))).collect();
    query.sort();
    let canonical_query =
        query.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&");

    // A repeated header name is legal and its values are joined with a comma,
    // in the order they were given.
    let mut headers: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, value) in &input.headers {
        headers
            .entry(name.trim().to_ascii_lowercase())
            .or_default()
            .push(canonical_header_value(value));
    }
    let mut canonical_headers = String::new();
    for (name, values) in &headers {
        canonical_headers.push_str(name);
        canonical_headers.push(':');
        canonical_headers.push_str(&values.join(","));
        canonical_headers.push('\n');
    }
    let signed_headers = headers.keys().cloned().collect::<Vec<_>>().join(";");

    // Note the shape: the header block ends with a newline *and* is followed
    // by a blank line, so there are two newlines between the last header and
    // the signed-header list. Getting this wrong is the classic SigV4 bug and
    // it fails exactly like a wrong password.
    format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        input.method.to_ascii_uppercase(),
        canonical_uri,
        canonical_query,
        canonical_headers,
        signed_headers,
        input.payload_sha256
    )
}

/// The credential scope: `YYYYMMDD/region/service/aws4_request`.
pub fn credential_scope(timestamp: DateTime<Utc>, region: &str, service: &str) -> String {
    format!("{}/{region}/{service}/aws4_request", amz_day(timestamp))
}

/// The string to sign: algorithm, timestamp, scope, and the hash of the
/// canonical request.
pub fn string_to_sign(input: &SigningInput, canonical: &str) -> String {
    format!(
        "{ALGORITHM}\n{}\n{}\n{}",
        amz_date(input.timestamp),
        credential_scope(input.timestamp, &input.region, &input.service),
        hex_sha256(canonical.as_bytes())
    )
}

/// The four-step key derivation. Each step re-keys HMAC with the previous
/// result, so the final key is bound to the day, the region and the service —
/// which is what stops a captured signature being replayed anywhere else.
pub fn signing_key(
    secret_access_key: &[u8],
    timestamp: DateTime<Utc>,
    region: &str,
    service: &str,
) -> [u8; 32] {
    let mut seed = Vec::with_capacity(4 + secret_access_key.len());
    seed.extend_from_slice(b"AWS4");
    seed.extend_from_slice(secret_access_key);
    let k_date = hmac_sha256(&seed, amz_day(timestamp).as_bytes());
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, service.as_bytes());
    hmac_sha256(&k_service, b"aws4_request")
}

/// Produce the `Authorization` header and everything that went into it.
pub fn sign(input: &SigningInput, access_key_id: &str, secret_access_key: &[u8]) -> Signature {
    let canonical = canonical_request(input);
    let to_sign = string_to_sign(input, &canonical);
    let key = signing_key(secret_access_key, input.timestamp, &input.region, &input.service);
    let signature = hex::encode(hmac_sha256(&key, to_sign.as_bytes()));

    // The signed-header list is recomputed from the canonical request rather
    // than rebuilt, so the header and the signature can never describe
    // different sets.
    let signed_headers = canonical.lines().rev().nth(1).unwrap_or_default().to_string();

    let authorization = format!(
        "{ALGORITHM} Credential={access_key_id}/{}, SignedHeaders={signed_headers}, \
         Signature={signature}",
        credential_scope(input.timestamp, &input.region, &input.service)
    );
    Signature {
        canonical_request: canonical,
        string_to_sign: to_sign,
        signed_headers,
        signature,
        authorization,
    }
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    // `Hmac::new_from_slice` only fails for key lengths this construction
    // cannot produce, but the error is still handled rather than unwrapped:
    // nothing in this crate panics on data.
    let mut mac = match Hmac::<Sha256>::new_from_slice(key) {
        Ok(mac) => mac,
        Err(_) => return [0u8; 32],
    };
    mac.update(data);
    mac.finalize().into_bytes().into()
}

fn hex_sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

// ---------------------------------------------------------------------------
// Results
// ---------------------------------------------------------------------------

/// One bucket owned by the account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bucket {
    pub name: String,
    pub created_at: Option<DateTime<Utc>>,
}

/// One object key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectSummary {
    pub key: String,
    pub size: u64,
    pub last_modified: Option<DateTime<Utc>>,
}

/// One page of `ListObjectsV2`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObjectListing {
    pub keys: Vec<ObjectSummary>,
    /// `CommonPrefixes`, when a delimiter produced any.
    pub common_prefixes: Vec<String>,
    /// True when the bucket holds more keys under this prefix than were
    /// returned.
    pub truncated: bool,
}

impl ObjectListing {
    /// True when this prefix already holds a kopia repository.
    ///
    /// Kopia writes a `kopia.repository` blob at the root of its prefix before
    /// anything else, so its presence is the cheapest reliable answer to "is
    /// there already a repository here?" — the question a destination editor
    /// needs answered before it offers to create one.
    pub fn holds_kopia_repository(&self) -> bool {
        self.keys.iter().any(|o| {
            let leaf = o.key.rsplit('/').next().unwrap_or(&o.key);
            leaf == "kopia.repository" || leaf.starts_with("kopia.repository")
        })
    }

    /// True when nothing at all is stored under the prefix.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty() && self.common_prefixes.is_empty()
    }
}

// ---------------------------------------------------------------------------
// The client
// ---------------------------------------------------------------------------

/// Two read-only S3 calls, signed and explained.
#[derive(Debug, Clone)]
pub struct S3Client {
    transport: Arc<dyn Transport>,
    /// Overridable so a test can pin the timestamp the signature is built
    /// from. `None` means "now", which is what production uses.
    fixed_time: Option<DateTime<Utc>>,
}

impl S3Client {
    /// The production client.
    pub fn new() -> std::result::Result<S3Client, S3Error> {
        Ok(S3Client { transport: Arc::new(ReqwestTransport::new()?), fixed_time: None })
    }

    /// A client over an injected transport, for tests.
    pub fn with_transport(transport: Arc<dyn Transport>) -> S3Client {
        S3Client { transport, fixed_time: None }
    }

    /// Pin the signing timestamp. Tests only.
    pub fn at(mut self, timestamp: DateTime<Utc>) -> S3Client {
        self.fixed_time = Some(timestamp);
        self
    }

    fn now(&self) -> DateTime<Utc> {
        self.fixed_time.unwrap_or_else(Utc::now)
    }

    /// Every bucket the credentials can see.
    ///
    /// Authenticated, so a success proves the endpoint is reachable, the
    /// clock is close enough, and both halves of the key pair are right.
    pub async fn list_buckets(
        &self,
        provider: &StorageProvider,
        keys: &S3Keys,
    ) -> std::result::Result<Vec<Bucket>, S3Error> {
        let endpoint = S3Endpoint::from_provider(provider)?;
        let body = self
            .send(
                "GET",
                &endpoint,
                &endpoint.authority,
                "/",
                &[],
                Vec::new(),
                keys,
                "list the buckets in this account",
            )
            .await?;
        xml::parse_list_buckets(&body)
    }

    /// One page of the keys under `prefix`.
    ///
    /// `max_keys` is clamped to [`MAX_KEYS_PER_PAGE`]; zero means "let the
    /// server decide", which is a thousand. There is deliberately no
    /// pagination loop: every caller here wants "is there anything here, and
    /// what does the start of it look like", not a full inventory of a bucket
    /// that may hold millions of objects.
    pub async fn list_objects_v2(
        &self,
        provider: &StorageProvider,
        keys: &S3Keys,
        bucket: &str,
        prefix: &str,
        max_keys: u32,
    ) -> std::result::Result<ObjectListing, S3Error> {
        let bucket = bucket.trim();
        validate_bucket(bucket)?;
        let endpoint = S3Endpoint::from_provider(provider)?;

        let (host, base) = self.address(&endpoint, bucket);
        let path = if base.is_empty() { "/".to_string() } else { base };

        let mut query: Vec<(String, String)> = vec![("list-type".into(), "2".into())];
        if !prefix.is_empty() {
            query.push(("prefix".into(), prefix.to_string()));
        }
        if max_keys > 0 {
            query.push(("max-keys".into(), max_keys.min(MAX_KEYS_PER_PAGE).to_string()));
        }

        let body = self
            .send(
                "GET",
                &endpoint,
                &host,
                &path,
                &query,
                Vec::new(),
                keys,
                "list the contents of this bucket",
            )
            .await
            .map_err(|e| match e {
                // `NoSuchBucket` arrives without the name attached from some
                // gateways; put it back so the message can be specific.
                S3Error::NoSuchBucket { bucket: b } if b.is_empty() => {
                    S3Error::NoSuchBucket { bucket: bucket.to_string() }
                }
                other => other,
            })?;
        xml::parse_list_objects(&body)
    }

    /// Whether one exact object key exists.
    ///
    /// Implemented as a one-key `ListObjectsV2` with the full key as the
    /// prefix rather than as a `HEAD`, for two reasons. `HEAD` needs
    /// `s3:GetObject`, which a key scoped to *writing* backups may not have,
    /// while `s3:ListBucket` is required by kopia anyway — so this asks for a
    /// permission the destination must already grant. And a `HEAD` 404 has no
    /// body, which would arrive here as "something that is not S3 answered"
    /// and would have to be special-cased; a listing answers the question in
    /// the same shape as everything else in this module.
    ///
    /// Used to detect kopia's `kopia.repository` format blob **without
    /// opening it**: presence is the whole answer, and reading it would need
    /// the repository key, which is the coupling this exists to avoid.
    pub async fn object_exists(
        &self,
        provider: &StorageProvider,
        keys: &S3Keys,
        bucket: &str,
        key: &str,
    ) -> std::result::Result<bool, S3Error> {
        let listing = self.list_objects_v2(provider, keys, bucket, key, 1).await?;
        Ok(listing.keys.iter().any(|o| o.key == key))
    }

    /// Prove the credentials can actually *write* into this prefix.
    ///
    /// The one thing in this module that is not read-only, and it is opt-in by
    /// name for exactly that reason. It writes an eleven-byte object at
    /// `<prefix>.superbackup-write-test` and deletes it again — the same shape
    /// as the filesystem write probe, and for the same reason: a destination
    /// that is readable but not writable fails every backup, and finding that
    /// out at two in the morning is the failure this check exists to prevent.
    ///
    /// The bounds are deliberate. The key is fixed and namespaced, so it can
    /// never collide with a repository blob (kopia's are `kopia.*`, `p*`,
    /// `q*`, `s*`, `x*`). The body is a constant. Nothing existing is ever
    /// overwritten by name, and a failure to delete is reported rather than
    /// swallowed, so a stray probe object is visible instead of mysterious.
    pub async fn write_probe(
        &self,
        provider: &StorageProvider,
        keys: &S3Keys,
        bucket: &str,
        prefix: &str,
    ) -> std::result::Result<(), S3Error> {
        let bucket = bucket.trim();
        validate_bucket(bucket)?;
        let endpoint = S3Endpoint::from_provider(provider)?;
        let key = format!("{prefix}{WRITE_PROBE_KEY}");
        let (host, base) = self.address(&endpoint, bucket);

        let path = format!("{base}/{key}");
        self.send(
            "PUT",
            &endpoint,
            &host,
            &path,
            &[],
            WRITE_PROBE_BODY.to_vec(),
            keys,
            "write to this bucket",
        )
        .await?;
        self.send(
            "DELETE",
            &endpoint,
            &host,
            &path,
            &[],
            Vec::new(),
            keys,
            "delete from this bucket",
        )
        .await
        .map(|_| ())
    }

    /// The host and path prefix a bucket is addressed by.
    fn address(&self, endpoint: &S3Endpoint, bucket: &str) -> (String, String) {
        if endpoint.uses_path_style(bucket) {
            (endpoint.authority.clone(), format!("/{bucket}"))
        } else {
            (format!("{bucket}.{}", endpoint.authority), String::new())
        }
    }

    /// Sign and send one request, and turn whatever comes back into either a
    /// body or an [`S3Error`] the user can act on.
    #[allow(clippy::too_many_arguments)]
    async fn send(
        &self,
        method: &'static str,
        endpoint: &S3Endpoint,
        host: &str,
        path: &str,
        query: &[(String, String)],
        body: Vec<u8>,
        keys: &S3Keys,
        operation: &'static str,
    ) -> std::result::Result<String, S3Error> {
        let Some(access_key) = keys.access_key() else {
            return Err(S3Error::Configuration {
                detail: "The stored access key id is not readable as text. Re-enter the \
                         credentials for this provider."
                    .into(),
            });
        };
        if access_key.trim().is_empty() {
            return Err(S3Error::Configuration {
                detail: "This provider has no access key id stored yet.".into(),
            });
        }

        let timestamp = self.now();
        // S3 signs the payload hash, and it is sent as a header so the server
        // can verify the body it received is the body that was signed.
        let payload_sha256 =
            if body.is_empty() { EMPTY_PAYLOAD_SHA256.to_string() } else { hex_sha256(&body) };
        let mut headers = vec![
            ("host".to_string(), host.to_string()),
            ("x-amz-content-sha256".to_string(), payload_sha256.clone()),
            ("x-amz-date".to_string(), amz_date(timestamp)),
        ];
        if let Some(token) = keys.token() {
            headers.push(("x-amz-security-token".to_string(), token.to_string()));
        }

        let input = SigningInput {
            method: method.into(),
            path: path.to_string(),
            query: query.to_vec(),
            headers: headers.clone(),
            payload_sha256,
            region: endpoint.signing_region().to_string(),
            service: SERVICE.to_string(),
            timestamp,
        };
        let signature = sign(&input, access_key.trim(), keys.secret_access_key.expose());

        // `host` is not sent explicitly: the transport derives it from the URL,
        // and sending it twice is how a signature and a request come to
        // disagree.
        let mut wire: Vec<(String, String)> =
            headers.into_iter().filter(|(name, _)| name != "host").collect();
        wire.push(("authorization".to_string(), signature.authorization));

        let query_string = query
            .iter()
            .map(|(k, v)| format!("{}={}", uri_encode(k, true), uri_encode(v, true)))
            .collect::<Vec<_>>()
            .join("&");
        let url = if query_string.is_empty() {
            format!("{}://{host}{}", endpoint.scheme, uri_encode(path, false))
        } else {
            format!("{}://{host}{}?{query_string}", endpoint.scheme, uri_encode(path, false))
        };

        let response = self
            .transport
            .send(HttpRequest { method, url, headers: wire, body })
            .await
            .map_err(|e| map_transport_error(e, host))?;

        interpret(response, host, operation)
    }
}

/// Transport failure to user-facing failure.
fn map_transport_error(e: TransportError, host: &str) -> S3Error {
    match e {
        TransportError::Dns(detail) => S3Error::Dns { host: host.to_string(), detail },
        TransportError::Connect(detail) => S3Error::Connect { host: host.to_string(), detail },
        TransportError::Tls(detail) => S3Error::Tls { host: host.to_string(), detail },
        TransportError::Timeout => S3Error::Timeout { host: host.to_string() },
        TransportError::TooLarge { limit } => S3Error::TooLarge { limit },
        TransportError::Redirect { location } => {
            S3Error::Redirected { region: None, endpoint: location.map(safe) }
        }
        TransportError::Other(detail) => S3Error::Connect { host: host.to_string(), detail },
    }
}

/// Turn a response into a body, or into the most specific error available.
fn interpret(
    response: HttpResponse,
    host: &str,
    operation: &'static str,
) -> std::result::Result<String, S3Error> {
    let text = match String::from_utf8(response.body) {
        Ok(text) => text,
        Err(_) => {
            return Err(S3Error::NotS3 {
                host: host.to_string(),
                status: response.status,
                detail: "the reply was not text".into(),
            })
        }
    };

    if (200..300).contains(&response.status) {
        return Ok(text);
    }

    // An S3 error is an XML document with a `Code`. Anything else at a 4xx or
    // 5xx is something that is not S3 answering: a proxy, a login page, an
    // object store's own web console.
    match xml::parse_error(&text) {
        Some(error) => Err(map_service_error(&error, response.status, operation)),
        None => Err(S3Error::NotS3 {
            host: host.to_string(),
            status: response.status,
            detail: describe_non_s3(&text, response.content_type.as_deref()),
        }),
    }
}

/// A one-clause description of a body that is not an S3 error, for the message
/// that tells the user their endpoint is wrong.
fn describe_non_s3(body: &str, content_type: Option<&str>) -> String {
    let trimmed = body.trim_start();
    let shape = if trimmed.starts_with("<!DOCTYPE html") || trimmed.starts_with("<html") {
        "it returned a web page"
    } else if trimmed.starts_with('{') || trimmed.starts_with('[') {
        "it returned JSON"
    } else if trimmed.is_empty() {
        "it returned nothing"
    } else {
        "the reply was not an S3 error document"
    };
    match content_type {
        Some(ct) => safe(format!("{shape}, content-type {ct}")),
        None => shape.to_string(),
    }
}

/// The mapping that decides what the user is told.
///
/// Each arm is a different next action, which is the whole reason the
/// distinctions are drawn: re-copy the key, re-copy the secret, fix the clock,
/// accept a scoped key, fix the region, fix the bucket name.
fn map_service_error(error: &xml::ServiceError, status: u16, operation: &'static str) -> S3Error {
    match error.code.as_str() {
        "InvalidAccessKeyId" | "InvalidAccessKeyID" | "UnknownAccessKey" => {
            S3Error::InvalidAccessKeyId
        }
        "SignatureDoesNotMatch" => S3Error::SignatureDoesNotMatch,
        "RequestTimeTooSkewed" => {
            S3Error::RequestTimeTooSkewed { server_time: error.server_time.clone().map(safe) }
        }
        "AccessDenied" | "AllAccessDisabled" | "Forbidden" => S3Error::AccessDenied { operation },
        "NoSuchBucket" => S3Error::NoSuchBucket { bucket: safe(error.bucket.clone()) },
        "PermanentRedirect" | "TemporaryRedirect" | "AuthorizationHeaderMalformed" => {
            S3Error::Redirected {
                region: error.region.clone().map(safe),
                endpoint: error.endpoint.clone().map(safe),
            }
        }
        // A 403 with an unfamiliar code is still a refusal to authorise, and
        // the safest reading is "the key may be fine but is not allowed here"
        // rather than "your key is wrong".
        _ if status == 403 => S3Error::AccessDenied { operation },
        _ => S3Error::Service {
            code: safe(error.code.clone()),
            message: safe(error.message.clone()),
            status,
        },
    }
}

// ---------------------------------------------------------------------------
// XML
// ---------------------------------------------------------------------------

/// A bounded reader for the three response shapes this module understands.
///
/// Not a general XML parser and not trying to be one. It handles elements,
/// text, attributes (skipped), comments, CDATA, processing instructions and
/// the five predefined entities, and it refuses anything deeper, longer or
/// larger than the caps below. It is iterative, so no input can overflow the
/// stack, and every allocation is bounded by the input length — which the
/// transport has already capped at [`MAX_RESPONSE_BYTES`].
pub mod xml {
    use super::{safe, S3Error};
    use chrono::{DateTime, Utc};
    use std::borrow::Cow;

    /// Deeper than any S3 response, shallow enough that a nesting bomb is
    /// refused rather than absorbed.
    const MAX_DEPTH: usize = 24;
    /// A `ListBucketResult` of 1000 keys is about 6000 elements.
    const MAX_ELEMENTS: usize = 250_000;
    /// No field in these documents is a novel.
    const MAX_TEXT_BYTES: usize = 64 * 1024;

    /// What an S3 `<Error>` document says.
    #[derive(Debug, Clone, Default, PartialEq, Eq)]
    pub struct ServiceError {
        pub code: String,
        pub message: String,
        pub bucket: String,
        pub region: Option<String>,
        pub endpoint: Option<String>,
        /// `ServerTime` from a `RequestTimeTooSkewed` body.
        pub server_time: Option<String>,
    }

    /// Walk `input`, calling `on_close(path, text)` as each element ends.
    ///
    /// `path` is the element stack including the element itself, so a handler
    /// matches on the tail (`["Bucket", "Name"]`) and is not confused by a
    /// gateway that wraps its answer in an extra envelope. `text` is the
    /// element's own character data, entity-decoded, and empty for an element
    /// that had children.
    ///
    /// Returns the root element's name.
    fn walk(input: &str, mut on_close: impl FnMut(&[&str], &str)) -> Result<String, MalformedXml> {
        let bytes = input.as_bytes();
        let mut pos = 0usize;
        let mut stack: Vec<(&str, String)> = Vec::new();
        let mut root: Option<String> = None;
        let mut elements = 0usize;

        while pos < bytes.len() {
            let Some(open) = find(bytes, pos, b'<') else {
                // Trailing text after the last element. Legal whitespace; a
                // truncated document ends here too, and the missing close tags
                // are caught below.
                break;
            };
            if open > pos {
                push_text(&mut stack, &input[pos..open])?;
            }
            let rest = &input[open..];

            if rest.starts_with("<!--") {
                pos = open + skip_to(rest, "-->", 3)?;
                continue;
            }
            if rest.starts_with("<![CDATA[") {
                let end = find_str(rest, "]]>").ok_or(MalformedXml::Truncated)?;
                push_raw(&mut stack, &rest[9..end])?;
                pos = open + end + 3;
                continue;
            }
            if rest.starts_with("<?") {
                pos = open + skip_to(rest, "?>", 2)?;
                continue;
            }
            if rest.starts_with("<!") {
                let end = find_str(rest, ">").ok_or(MalformedXml::Truncated)?;
                pos = open + end + 1;
                continue;
            }
            if rest.starts_with("</") {
                let end = find_str(rest, ">").ok_or(MalformedXml::Truncated)?;
                let name = local_name(rest[2..end].trim());
                let Some((open_name, text)) = stack.pop() else {
                    return Err(MalformedXml::Unbalanced);
                };
                if open_name != name {
                    return Err(MalformedXml::Unbalanced);
                }
                let mut path: Vec<&str> = stack.iter().map(|(n, _)| *n).collect();
                path.push(open_name);
                on_close(&path, text.trim());
                pos = open + end + 1;
                continue;
            }

            // A start tag. The name runs to the first whitespace, `/` or `>`;
            // the rest is attributes, which are skipped but must be scanned so
            // that a `>` inside a quoted attribute value does not end the tag.
            let name_end = rest[1..]
                .find(|c: char| c.is_whitespace() || c == '/' || c == '>')
                .map(|i| i + 1)
                .ok_or(MalformedXml::Truncated)?;
            let name = &rest[1..name_end];
            if name.is_empty() {
                return Err(MalformedXml::Malformed);
            }
            let (tag_end, self_closing) = scan_tag(rest, name_end)?;

            elements += 1;
            if elements > MAX_ELEMENTS {
                return Err(MalformedXml::TooManyElements);
            }
            if root.is_none() {
                root = Some(local_name(name).to_string());
            }
            let local = local_name(name);
            if self_closing {
                let mut path: Vec<&str> = stack.iter().map(|(n, _)| *n).collect();
                path.push(local);
                on_close(&path, "");
            } else {
                if stack.len() >= MAX_DEPTH {
                    return Err(MalformedXml::TooDeep);
                }
                stack.push((local, String::new()));
            }
            pos = open + tag_end;
        }

        if !stack.is_empty() {
            return Err(MalformedXml::Truncated);
        }
        root.ok_or(MalformedXml::Malformed)
    }

    /// Drop an XML namespace prefix: `s3:Name` and `Name` are the same element
    /// as far as these documents are concerned.
    fn local_name(name: &str) -> &str {
        match name.rsplit_once(':') {
            Some((_, local)) => local,
            None => name,
        }
    }

    fn find(bytes: &[u8], from: usize, needle: u8) -> Option<usize> {
        bytes[from..].iter().position(|b| *b == needle).map(|i| i + from)
    }

    fn find_str(haystack: &str, needle: &str) -> Option<usize> {
        haystack.find(needle)
    }

    fn skip_to(rest: &str, terminator: &str, from: usize) -> Result<usize, MalformedXml> {
        match rest[from..].find(terminator) {
            Some(i) => Ok(from + i + terminator.len()),
            None => Err(MalformedXml::Truncated),
        }
    }

    /// Find the `>` that ends a tag, honouring quoted attribute values.
    /// Returns the offset just past it, and whether the tag closed itself.
    fn scan_tag(rest: &str, from: usize) -> Result<(usize, bool), MalformedXml> {
        let bytes = rest.as_bytes();
        let mut i = from;
        let mut quote: Option<u8> = None;
        while i < bytes.len() {
            let b = bytes[i];
            match quote {
                Some(q) if b == q => quote = None,
                Some(_) => {}
                None if b == b'"' || b == b'\'' => quote = Some(b),
                None if b == b'>' => {
                    let self_closing = i > from && bytes[i - 1] == b'/';
                    return Ok((i + 1, self_closing));
                }
                None => {}
            }
            i += 1;
        }
        Err(MalformedXml::Truncated)
    }

    fn push_text(stack: &mut [(&str, String)], raw: &str) -> Result<(), MalformedXml> {
        if raw.trim().is_empty() {
            return Ok(());
        }
        push_raw(stack, &decode_entities(raw))
    }

    fn push_raw(stack: &mut [(&str, String)], text: &str) -> Result<(), MalformedXml> {
        let Some((_, buffer)) = stack.last_mut() else {
            // Character data outside the root element. Ignored rather than
            // rejected: some gateways prepend a byte-order mark.
            return Ok(());
        };
        if buffer.len() + text.len() > MAX_TEXT_BYTES {
            return Err(MalformedXml::TextTooLong);
        }
        buffer.push_str(text);
        Ok(())
    }

    /// The five predefined entities plus numeric references. An entity this
    /// does not know is left as written rather than treated as an error: a
    /// bucket called `a&b` matters less than refusing to show any bucket.
    fn decode_entities(raw: &str) -> Cow<'_, str> {
        if !raw.contains('&') {
            return Cow::Borrowed(raw);
        }
        let mut out = String::with_capacity(raw.len());
        let mut rest = raw;
        while let Some(amp) = rest.find('&') {
            out.push_str(&rest[..amp]);
            let tail = &rest[amp..];
            // A reference is at most `&#x10FFFF;`, so nothing beyond a short
            // window can be one — which keeps this linear.
            let end = tail[..tail.len().min(12)].find(';');
            match end.map(|e| (&tail[1..e], e)) {
                Some(("amp", e)) => {
                    out.push('&');
                    rest = &tail[e + 1..];
                }
                Some(("lt", e)) => {
                    out.push('<');
                    rest = &tail[e + 1..];
                }
                Some(("gt", e)) => {
                    out.push('>');
                    rest = &tail[e + 1..];
                }
                Some(("quot", e)) => {
                    out.push('"');
                    rest = &tail[e + 1..];
                }
                Some(("apos", e)) => {
                    out.push('\'');
                    rest = &tail[e + 1..];
                }
                Some((entity, e)) if entity.starts_with('#') => {
                    let decoded = numeric_entity(&entity[1..]);
                    match decoded {
                        Some(c) => out.push(c),
                        None => out.push_str(&tail[..=e]),
                    }
                    rest = &tail[e + 1..];
                }
                _ => {
                    out.push('&');
                    rest = &tail[1..];
                }
            }
        }
        out.push_str(rest);
        Cow::Owned(out)
    }

    fn numeric_entity(digits: &str) -> Option<char> {
        let value = match digits.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => digits.parse::<u32>().ok()?,
        };
        char::from_u32(value)
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum MalformedXml {
        Truncated,
        Unbalanced,
        Malformed,
        TooDeep,
        TooManyElements,
        TextTooLong,
    }

    impl MalformedXml {
        fn detail(self) -> &'static str {
            match self {
                MalformedXml::Truncated => "the reply ended part-way through",
                MalformedXml::Unbalanced => "its tags do not match up",
                MalformedXml::Malformed => "it is not well-formed XML",
                MalformedXml::TooDeep => "it is nested far deeper than any S3 reply",
                MalformedXml::TooManyElements => "it contains far more elements than any S3 reply",
                MalformedXml::TextTooLong => "one of its values is implausibly long",
            }
        }
    }

    impl From<MalformedXml> for S3Error {
        fn from(e: MalformedXml) -> S3Error {
            S3Error::Malformed { detail: e.detail().into() }
        }
    }

    /// Parse a `ListAllMyBucketsResult`.
    pub fn parse_list_buckets(input: &str) -> Result<Vec<super::Bucket>, S3Error> {
        // An error document with a 200 status happens: some gateways answer a
        // failed request with HTTP 200 and an `<Error>` body. Catching it here
        // means such an endpoint produces the right message rather than
        // "unreadable answer".
        if let Some(error) = parse_error(input) {
            return Err(super::map_service_error(&error, 200, "list the buckets in this account"));
        }
        let mut buckets: Vec<super::Bucket> = Vec::new();
        let mut current = super::Bucket { name: String::new(), created_at: None };
        let root = walk(input, |path, text| match tail(path) {
            ["Bucket", "Name"] => current.name = text.to_string(),
            ["Bucket", "CreationDate"] => current.created_at = parse_timestamp(text),
            [.., "Bucket"] => {
                if !current.name.is_empty() {
                    buckets.push(std::mem::replace(
                        &mut current,
                        super::Bucket { name: String::new(), created_at: None },
                    ));
                } else {
                    current = super::Bucket { name: String::new(), created_at: None };
                }
            }
            _ => {}
        })?;
        if root != "ListAllMyBucketsResult" {
            return Err(S3Error::Malformed {
                detail: safe(format!("it is a <{}> document, not a bucket listing", root)),
            });
        }
        Ok(buckets)
    }

    /// Parse a `ListBucketResult` (`list-type=2`).
    pub fn parse_list_objects(input: &str) -> Result<super::ObjectListing, S3Error> {
        if let Some(error) = parse_error(input) {
            return Err(super::map_service_error(&error, 200, "list the contents of this bucket"));
        }
        let mut listing = super::ObjectListing::default();
        let mut current = super::ObjectSummary { key: String::new(), size: 0, last_modified: None };
        let root = walk(input, |path, text| match tail(path) {
            ["Contents", "Key"] => current.key = text.to_string(),
            ["Contents", "Size"] => current.size = text.parse().unwrap_or(0),
            ["Contents", "LastModified"] => current.last_modified = parse_timestamp(text),
            [.., "Contents"] => {
                if !current.key.is_empty() {
                    listing.keys.push(std::mem::replace(
                        &mut current,
                        super::ObjectSummary { key: String::new(), size: 0, last_modified: None },
                    ));
                }
            }
            ["CommonPrefixes", "Prefix"] => {
                if !text.is_empty() {
                    listing.common_prefixes.push(text.to_string());
                }
            }
            [.., "IsTruncated"] => listing.truncated = text.eq_ignore_ascii_case("true"),
            _ => {}
        })?;
        if root != "ListBucketResult" {
            return Err(S3Error::Malformed {
                detail: safe(format!("it is a <{}> document, not an object listing", root)),
            });
        }
        Ok(listing)
    }

    /// Parse an `<Error>` document. `None` when the body is not one — which is
    /// how "something that is not S3 answered" is detected.
    pub fn parse_error(input: &str) -> Option<ServiceError> {
        let mut error = ServiceError::default();
        let mut saw_error_element = false;
        walk(input, |path, text| {
            match tail(path) {
                ["Error", "Code"] => error.code = text.to_string(),
                ["Error", "Message"] => error.message = text.to_string(),
                ["Error", "BucketName"] => error.bucket = text.to_string(),
                ["Error", "Region"] => error.region = Some(text.to_string()),
                ["Error", "Endpoint"] => error.endpoint = Some(text.to_string()),
                ["Error", "ServerTime"] => error.server_time = Some(text.to_string()),
                _ => {}
            }
            if matches!(tail(path), [.., "Error"]) {
                saw_error_element = true;
            }
        })
        .ok()?;
        (saw_error_element && !error.code.is_empty()).then_some(error)
    }

    /// The last two path components, which is all any rule here matches on.
    fn tail<'a>(path: &'a [&'a str]) -> &'a [&'a str] {
        if path.len() > 2 {
            &path[path.len() - 2..]
        } else {
            path
        }
    }

    fn parse_timestamp(text: &str) -> Option<DateTime<Utc>> {
        DateTime::parse_from_rfc3339(text).ok().map(|t| t.with_timezone(&Utc))
    }
}

/// Format a timestamp the way these listings print one.
pub fn format_timestamp(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    // -- SigV4, against AWS's published test suite -------------------------
    //
    // The vectors below are `aws-signing-test-suite/v4`, the suite AWS
    // publishes with its own signing implementations (access key `AKIDEXAMPLE`,
    // service `service`, region `us-east-1`, 2015-08-30T12:36:00Z). They are
    // reproduced verbatim: the canonical request, the string to sign and the
    // signature are all asserted, so a signer that is internally consistent but
    // wrong at any stage fails here rather than in the field, where a bad
    // signature is indistinguishable from a wrong password.

    const SUITE_ACCESS_KEY: &str = "AKIDEXAMPLE";
    const SUITE_SECRET: &str = "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY";

    fn suite_time() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2015, 8, 30, 12, 36, 0).single().expect("a real instant")
    }

    fn suite_input(
        method: &str,
        path: &str,
        query: &[(&str, &str)],
        headers: &[(&str, &str)],
    ) -> SigningInput {
        let mut all: Vec<(String, String)> = vec![("x-amz-date".into(), amz_date(suite_time()))];
        all.extend(headers.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        SigningInput {
            method: method.into(),
            path: path.into(),
            query: query.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            headers: all,
            payload_sha256: EMPTY_PAYLOAD_SHA256.into(),
            region: "us-east-1".into(),
            service: "service".into(),
            timestamp: suite_time(),
        }
    }

    fn assert_vector(input: SigningInput, creq: &str, sts: &str, expected: &str) {
        let signed = sign(&input, SUITE_ACCESS_KEY, SUITE_SECRET.as_bytes());
        assert_eq!(signed.canonical_request, creq, "canonical request");
        assert_eq!(signed.string_to_sign, sts, "string to sign");
        assert_eq!(signed.signature, expected, "signature");
    }

    #[test]
    fn sigv4_get_vanilla() {
        assert_vector(
            suite_input("GET", "/", &[], &[("Host", "example.amazonaws.com")]),
            "GET\n/\n\nhost:example.amazonaws.com\nx-amz-date:20150830T123600Z\n\n\
             host;x-amz-date\n\
             e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "AWS4-HMAC-SHA256\n20150830T123600Z\n20150830/us-east-1/service/aws4_request\n\
             bb579772317eb040ac9ed261061d46c1f17a8133879d6129b6e1c25292927e63",
            "5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31",
        );
    }

    #[test]
    fn sigv4_get_vanilla_query_order_key_case() {
        // Query parameters sort by encoded key, not by the order they were
        // written, and the sort is case-sensitive byte order.
        assert_vector(
            suite_input(
                "GET",
                "/",
                &[("Param2", "value2"), ("Param1", "value1")],
                &[("Host", "example.amazonaws.com")],
            ),
            "GET\n/\nParam1=value1&Param2=value2\nhost:example.amazonaws.com\n\
             x-amz-date:20150830T123600Z\n\nhost;x-amz-date\n\
             e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "AWS4-HMAC-SHA256\n20150830T123600Z\n20150830/us-east-1/service/aws4_request\n\
             816cd5b414d056048ba4f7c5386d6e0533120fb1fcfa93762cf0fc39e2cf19e0",
            "b97d918cfa904a5beff61c982a1b6f458b799221646efd99d3219ec94cdf2500",
        );
    }

    #[test]
    fn sigv4_get_unreserved_characters_are_not_encoded() {
        assert_vector(
            suite_input(
                "GET",
                "/-._~0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz",
                &[],
                &[("Host", "example.amazonaws.com")],
            ),
            "GET\n/-._~0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz\n\n\
             host:example.amazonaws.com\nx-amz-date:20150830T123600Z\n\nhost;x-amz-date\n\
             e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "AWS4-HMAC-SHA256\n20150830T123600Z\n20150830/us-east-1/service/aws4_request\n\
             6a968768eefaa713e2a6b16b589a8ea192661f098f37349f4e2c0082757446f9",
            "07ef7494c76fa4850883e2b006601f940f8a34d404d0cfa977f52a65bbf5f24f",
        );
    }

    #[test]
    fn sigv4_get_utf8_path_is_percent_encoded_once() {
        assert_vector(
            suite_input("GET", "/\u{1234}", &[], &[("Host", "example.amazonaws.com")]),
            "GET\n/%E1%88%B4\n\nhost:example.amazonaws.com\nx-amz-date:20150830T123600Z\n\n\
             host;x-amz-date\n\
             e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "AWS4-HMAC-SHA256\n20150830T123600Z\n20150830/us-east-1/service/aws4_request\n\
             2a0a97d02205e45ce2e994789806b19270cfbbb0921b278ccf58f5249ac42102",
            "8318018e0b0f223aa2bbf98705b62bb787dc9c0e678f255a891fd03141be5d85",
        );
    }

    #[test]
    fn sigv4_header_values_are_trimmed_and_collapsed() {
        assert_vector(
            suite_input(
                "GET",
                "/",
                &[],
                &[
                    ("Host", "example.amazonaws.com"),
                    ("My-Header1", " value1 "),
                    ("My-Header2", " \"a b c\" "),
                ],
            ),
            "GET\n/\n\nhost:example.amazonaws.com\nmy-header1:value1\nmy-header2:\"a b c\"\n\
             x-amz-date:20150830T123600Z\n\nhost;my-header1;my-header2;x-amz-date\n\
             e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "AWS4-HMAC-SHA256\n20150830T123600Z\n20150830/us-east-1/service/aws4_request\n\
             a726db9b0df21c14f559d0a978e563112acb1b9e05476f0a6a1c7d68f28605c7",
            "acc3ed3afb60bb290fc8d2dd0098b9911fcaa05412b367055dee359757a9c736",
        );
    }

    #[test]
    fn sigv4_session_token_is_signed() {
        assert_vector(
            suite_input(
                "GET",
                "/",
                &[],
                &[
                    ("Host", "example.amazonaws.com"),
                    (
                        "x-amz-security-token",
                        "6e86291e8372ff2a2260956d9b8aae1d763fbf315fa00fa31553b73ebf194267",
                    ),
                ],
            ),
            "GET\n/\n\nhost:example.amazonaws.com\nx-amz-date:20150830T123600Z\n\
             x-amz-security-token:\
             6e86291e8372ff2a2260956d9b8aae1d763fbf315fa00fa31553b73ebf194267\n\n\
             host;x-amz-date;x-amz-security-token\n\
             e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "AWS4-HMAC-SHA256\n20150830T123600Z\n20150830/us-east-1/service/aws4_request\n\
             067b36aa60031588cea4a4cde1f21215227a047690c72247f1d70b32fbbfad2b",
            "07ec1639c89043aa0e3e2de82b96708f198cceab042d4a97044c66dd9f74e7f8",
        );
    }

    #[test]
    fn sigv4_post_sorts_header_names() {
        assert_vector(
            suite_input(
                "POST",
                "/",
                &[],
                &[("Host", "example.amazonaws.com"), ("My-Header1", "value1")],
            ),
            "POST\n/\n\nhost:example.amazonaws.com\nmy-header1:value1\n\
             x-amz-date:20150830T123600Z\n\nhost;my-header1;x-amz-date\n\
             e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "AWS4-HMAC-SHA256\n20150830T123600Z\n20150830/us-east-1/service/aws4_request\n\
             9368318c2967cf6de74404b30c65a91e8f6253e0a8659d6d5319f1a812f87d65",
            "c5410059b04c1ee005303aed430f6e6645f61f4dc9e1461ec8f8916fdf18852c",
        );
    }

    #[test]
    fn authorization_header_has_the_documented_shape() {
        let signed = sign(
            &suite_input("GET", "/", &[], &[("Host", "example.amazonaws.com")]),
            SUITE_ACCESS_KEY,
            SUITE_SECRET.as_bytes(),
        );
        assert_eq!(
            signed.authorization,
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, \
             SignedHeaders=host;x-amz-date, \
             Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
        );
    }

    #[test]
    fn signing_key_derivation_matches_the_documented_four_steps() {
        // The scope is part of the key, not merely part of the string: change
        // the day, the region or the service and the key changes.
        let base = signing_key(SUITE_SECRET.as_bytes(), suite_time(), "us-east-1", "s3");
        let other_region = signing_key(SUITE_SECRET.as_bytes(), suite_time(), "eu-1", "s3");
        let other_service =
            signing_key(SUITE_SECRET.as_bytes(), suite_time(), "us-east-1", "service");
        let other_day = signing_key(
            SUITE_SECRET.as_bytes(),
            Utc.with_ymd_and_hms(2015, 8, 31, 12, 36, 0).single().expect("a real instant"),
            "us-east-1",
            "s3",
        );
        assert_ne!(base, other_region);
        assert_ne!(base, other_service);
        assert_ne!(base, other_day);
    }

    #[test]
    fn uri_encoding_follows_rfc3986_not_form_encoding() {
        assert_eq!(uri_encode("a b", true), "a%20b");
        assert_eq!(uri_encode("a/b", false), "a/b");
        assert_eq!(uri_encode("a/b", true), "a%2Fb");
        assert_eq!(uri_encode("-_.~", true), "-_.~");
        assert_eq!(uri_encode("+", true), "%2B");
        assert_eq!(uri_encode("=", true), "%3D");
    }

    // -- Endpoints and addressing ------------------------------------------

    fn provider(endpoint: &str, region: &str, tls: bool, path_style: bool) -> StorageProvider {
        StorageProvider {
            id: uuid::Uuid::nil(),
            name: "test".into(),
            kind: ProviderKind::S3 {
                endpoint: endpoint.into(),
                region: region.into(),
                credentials: crate::model::S3Credentials::for_provider(&uuid::Uuid::nil()),
                tls,
                path_style,
                flavour: crate::model::S3Flavour::Storj,
                admin_url: None,
            },
            notes: String::new(),
            created_at: Utc::now(),
            last_verified_at: None,
        }
    }

    #[test]
    fn an_endpoint_parses_with_or_without_a_scheme() {
        let with = S3Endpoint::from_provider(&provider(
            "https://gateway.storjshare.io",
            "eu-1",
            true,
            false,
        ))
        .expect("a usable endpoint");
        let without =
            S3Endpoint::from_provider(&provider("gateway.storjshare.io", "eu-1", true, false))
                .expect("a usable endpoint");
        assert_eq!(with, without);
        assert_eq!(with.scheme, "https");
        assert_eq!(with.authority, "gateway.storjshare.io");
    }

    #[test]
    fn an_http_scheme_disables_tls_exactly_as_it_does_for_kopia() {
        let endpoint =
            S3Endpoint::from_provider(&provider("http://minio.local:9000", "", true, true))
                .expect("a usable endpoint");
        assert_eq!(endpoint.scheme, "http");
        assert_eq!(endpoint.authority, "minio.local:9000");
        // And the flag alone does it too.
        let flagged = S3Endpoint::from_provider(&provider("minio.local", "", false, false))
            .expect("a usable endpoint");
        assert_eq!(flagged.scheme, "http");
    }

    #[test]
    fn a_default_port_is_dropped_so_the_host_header_matches_the_signature() {
        let endpoint =
            S3Endpoint::from_provider(&provider("https://example.com:443", "", true, false))
                .expect("a usable endpoint");
        assert_eq!(endpoint.authority, "example.com");
        let plain = S3Endpoint::from_provider(&provider("http://example.com:80", "", false, false))
            .expect("a usable endpoint");
        assert_eq!(plain.authority, "example.com");
    }

    #[test]
    fn an_unusable_endpoint_is_refused_rather_than_sent() {
        for bad in ["", "   ", "https://", "host name", "example.com:notaport"] {
            assert!(
                S3Endpoint::from_provider(&provider(bad, "", true, false)).is_err(),
                "{bad:?} should not produce an endpoint"
            );
        }
    }

    #[test]
    fn path_style_is_used_for_gateways_and_forced_when_asked() {
        let storj = S3Endpoint::from_provider(&provider(
            "https://gateway.storjshare.io",
            "eu-1",
            true,
            false,
        ))
        .expect("a usable endpoint");
        assert!(storj.uses_path_style("backups"), "a gateway is addressed path-style");

        let aws = S3Endpoint::from_provider(&provider(
            "https://s3.amazonaws.com",
            "us-east-1",
            true,
            false,
        ))
        .expect("a usable endpoint");
        assert!(!aws.uses_path_style("backups"), "AWS is addressed virtual-hosted");
        assert!(aws.uses_path_style("my.backups"), "a dotted name breaks the certificate");
        assert!(aws.uses_path_style("ab"), "too short to be a DNS label");

        let forced = S3Endpoint::from_provider(&provider(
            "https://s3.amazonaws.com",
            "us-east-1",
            true,
            true,
        ))
        .expect("a usable endpoint");
        assert!(forced.uses_path_style("backups"), "the provider asked for path style");
    }

    #[test]
    fn a_bucket_name_that_could_reshape_the_request_is_refused() {
        for bad in ["", "a/b", "a\\b", "a?b", "with space", "..", "a..b"] {
            assert!(validate_bucket(bad).is_err(), "{bad:?} should be refused");
        }
        assert!(validate_bucket("my-backups").is_ok());
    }

    #[test]
    fn an_empty_region_signs_as_us_east_1() {
        let endpoint =
            S3Endpoint::from_provider(&provider("gateway.storjshare.io", "", true, false))
                .expect("a usable endpoint");
        assert_eq!(endpoint.signing_region(), "us-east-1");
    }

    // -- XML ----------------------------------------------------------------

    const BUCKETS_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<ListAllMyBucketsResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/">
  <Owner><ID>abc</ID><DisplayName>me</DisplayName></Owner>
  <Buckets>
    <Bucket><Name>photos</Name><CreationDate>2024-01-02T03:04:05.000Z</CreationDate></Bucket>
    <Bucket><Name>dev-backups</Name><CreationDate>2025-06-07T08:09:10.000Z</CreationDate></Bucket>
  </Buckets>
</ListAllMyBucketsResult>"#;

    #[test]
    fn a_bucket_listing_is_read_in_order() {
        let buckets = xml::parse_list_buckets(BUCKETS_XML).expect("a readable listing");
        assert_eq!(
            buckets.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(),
            ["photos", "dev-backups"]
        );
        assert_eq!(
            buckets[0].created_at.map(format_timestamp).as_deref(),
            Some("2024-01-02T03:04:05Z")
        );
    }

    #[test]
    fn a_namespace_prefixed_listing_is_read_the_same_way() {
        let prefixed = r#"<s3:ListAllMyBucketsResult xmlns:s3="urn:x">
            <s3:Buckets><s3:Bucket><s3:Name>only</s3:Name></s3:Bucket></s3:Buckets>
            </s3:ListAllMyBucketsResult>"#;
        let buckets = xml::parse_list_buckets(prefixed).expect("a readable listing");
        assert_eq!(buckets.len(), 1);
        assert_eq!(buckets[0].name, "only");
    }

    #[test]
    fn an_empty_account_lists_no_buckets_rather_than_failing() {
        let empty = "<ListAllMyBucketsResult><Buckets/></ListAllMyBucketsResult>";
        assert_eq!(xml::parse_list_buckets(empty).expect("a readable listing"), Vec::new());
    }

    #[test]
    fn a_truncated_body_is_an_error_and_not_a_panic() {
        for cut in [40, 120, 200, BUCKETS_XML.len() - 5] {
            let partial = &BUCKETS_XML[..cut];
            let result = xml::parse_list_buckets(partial);
            assert!(
                matches!(result, Err(S3Error::Malformed { .. })),
                "a body cut at {cut} should be reported as malformed, got {result:?}"
            );
        }
    }

    #[test]
    fn malformed_input_never_panics() {
        for bad in [
            "",
            "<",
            "</>",
            "<a",
            "<a>",
            "</a>",
            "<a></b>",
            "<a><![CDATA[",
            "<!--",
            "<?xml",
            "<a b=\">\">text</a>",
            "&#xZZZZ;",
            "<a>&</a>",
            "<a>&unknown;</a>",
        ] {
            let _ = xml::parse_list_buckets(bad);
            let _ = xml::parse_list_objects(bad);
            let _ = xml::parse_error(bad);
        }
    }

    #[test]
    fn a_nesting_bomb_is_refused_rather_than_absorbed() {
        let deep = "<a>".repeat(200) + &"</a>".repeat(200);
        assert!(matches!(xml::parse_list_buckets(&deep), Err(S3Error::Malformed { .. })));
    }

    #[test]
    fn entities_are_decoded_and_unknown_ones_are_left_alone() {
        let xml_text = "<ListAllMyBucketsResult><Buckets><Bucket><Name>a&amp;b</Name></Bucket>\
                        <Bucket><Name>c&unknown;d</Name></Bucket>\
                        <Bucket><Name>e&#65;f</Name></Bucket></Buckets></ListAllMyBucketsResult>";
        let buckets = xml::parse_list_buckets(xml_text).expect("a readable listing");
        assert_eq!(
            buckets.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(),
            ["a&b", "c&unknown;d", "eAf"]
        );
    }

    #[test]
    fn an_object_listing_reports_keys_truncation_and_a_repository() {
        let listing_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult>
  <Name>backups</Name><Prefix>superbackup/pc/</Prefix>
  <KeyCount>2</KeyCount><IsTruncated>true</IsTruncated>
  <Contents><Key>superbackup/pc/kopia.repository</Key><Size>661</Size>
    <LastModified>2025-06-07T08:09:10.000Z</LastModified></Contents>
  <Contents><Key>superbackup/pc/p01</Key><Size>4194304</Size></Contents>
  <CommonPrefixes><Prefix>superbackup/pc/x/</Prefix></CommonPrefixes>
</ListBucketResult>"#;
        let listing = xml::parse_list_objects(listing_xml).expect("a readable listing");
        assert_eq!(listing.keys.len(), 2);
        assert_eq!(listing.keys[0].size, 661);
        assert_eq!(listing.common_prefixes, ["superbackup/pc/x/"]);
        assert!(listing.truncated);
        assert!(listing.holds_kopia_repository());
        assert!(!listing.is_empty());
    }

    #[test]
    fn an_empty_prefix_holds_no_repository() {
        let listing_xml = "<ListBucketResult><Name>b</Name><KeyCount>0</KeyCount>\
                           <IsTruncated>false</IsTruncated></ListBucketResult>";
        let listing = xml::parse_list_objects(listing_xml).expect("a readable listing");
        assert!(listing.is_empty());
        assert!(!listing.holds_kopia_repository());
    }

    #[test]
    fn a_document_of_the_wrong_shape_is_reported_as_such() {
        let wrong = "<ListBucketResult><Name>b</Name></ListBucketResult>";
        assert!(matches!(xml::parse_list_buckets(wrong), Err(S3Error::Malformed { .. })));
    }

    // -- Error mapping ------------------------------------------------------

    fn error_body(code: &str, extra: &str) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Error><Code>{code}</Code>\
             <Message>a message</Message>{extra}<RequestId>abc</RequestId></Error>"
        )
    }

    fn mapped(code: &str, status: u16, extra: &str) -> S3Error {
        let parsed = xml::parse_error(&error_body(code, extra)).expect("an error document");
        map_service_error(&parsed, status, "list the buckets in this account")
    }

    #[test]
    fn each_service_error_code_maps_to_its_own_explanation() {
        assert_eq!(mapped("InvalidAccessKeyId", 403, ""), S3Error::InvalidAccessKeyId);
        assert_eq!(mapped("SignatureDoesNotMatch", 403, ""), S3Error::SignatureDoesNotMatch);
        assert_eq!(
            mapped("RequestTimeTooSkewed", 403, "<ServerTime>2026-08-30T10:00:00Z</ServerTime>"),
            S3Error::RequestTimeTooSkewed { server_time: Some("2026-08-30T10:00:00Z".into()) }
        );
        assert_eq!(
            mapped("AccessDenied", 403, ""),
            S3Error::AccessDenied { operation: "list the buckets in this account" }
        );
        assert_eq!(
            mapped("NoSuchBucket", 404, "<BucketName>gone</BucketName>"),
            S3Error::NoSuchBucket { bucket: "gone".into() }
        );
        assert_eq!(
            mapped("PermanentRedirect", 301, "<Region>eu-central-1</Region>"),
            S3Error::Redirected { region: Some("eu-central-1".into()), endpoint: None }
        );
        assert_eq!(
            mapped("InternalError", 500, ""),
            S3Error::Service {
                code: "InternalError".into(),
                message: "a message".into(),
                status: 500
            }
        );
    }

    #[test]
    fn an_unfamiliar_403_is_a_refusal_to_authorise_not_a_bad_key() {
        // Saying "your key is wrong" when the endpoint only said "no" would
        // send the user to regenerate credentials that were always correct.
        assert!(mapped("SomeGatewaySpecificCode", 403, "").credentials_accepted());
    }

    #[test]
    fn access_denied_is_the_only_code_that_proves_the_credentials() {
        assert!(S3Error::AccessDenied { operation: "x" }.credentials_accepted());
        for other in [
            S3Error::InvalidAccessKeyId,
            S3Error::SignatureDoesNotMatch,
            S3Error::RequestTimeTooSkewed { server_time: None },
            S3Error::Timeout { host: "h".into() },
            S3Error::NoSuchBucket { bucket: "b".into() },
        ] {
            assert!(!other.credentials_accepted(), "{other:?} does not prove anything");
        }
    }

    #[test]
    fn every_error_produces_a_complete_sentence() {
        for error in [
            S3Error::Dns { host: "h".into(), detail: "d".into() },
            S3Error::Connect { host: "h".into(), detail: "d".into() },
            S3Error::Tls { host: "h".into(), detail: "d".into() },
            S3Error::Timeout { host: "h".into() },
            S3Error::InvalidAccessKeyId,
            S3Error::SignatureDoesNotMatch,
            S3Error::RequestTimeTooSkewed { server_time: None },
            S3Error::AccessDenied { operation: "list the buckets in this account" },
            S3Error::NoSuchBucket { bucket: "b".into() },
            S3Error::Redirected { region: None, endpoint: None },
            S3Error::Service { code: "c".into(), message: "m".into(), status: 500 },
            S3Error::NotS3 { host: "h".into(), status: 200, detail: "d".into() },
            S3Error::Malformed { detail: "d".into() },
            S3Error::TooLarge { limit: 1 },
            S3Error::Configuration { detail: "This storage provider has no endpoint yet.".into() },
        ] {
            let message = error.message();
            assert!(message.len() > 20, "{error:?} produced {message:?}");
            assert!(message.ends_with('.'), "{error:?} produced {message:?}");
        }
    }

    #[test]
    fn a_non_s3_endpoint_is_named_as_such() {
        let response = HttpResponse {
            status: 404,
            content_type: Some("text/html".into()),
            body: b"<!DOCTYPE html><html><body>Not found</body></html>".to_vec(),
        };
        let error = interpret(response, "console.example.com", "list").expect_err("not S3");
        assert!(matches!(error, S3Error::NotS3 { .. }));
        assert!(error.message().contains("web page"), "{}", error.message());
    }

    #[test]
    fn transport_failures_are_classified_by_what_the_user_must_fix() {
        assert_eq!(
            classify_chain("error trying to connect: dns error: no record found", false, true),
            TransportError::Dns("error trying to connect: dns error: no record found".into())
        );
        assert_eq!(
            classify_chain("invalid peer certificate: UnknownIssuer", false, true),
            TransportError::Tls("invalid peer certificate: UnknownIssuer".into())
        );
        assert_eq!(
            classify_chain("tcp connect error: Connection refused", false, true),
            TransportError::Connect("tcp connect error: Connection refused".into())
        );
        assert_eq!(classify_chain("operation timed out", true, false), TransportError::Timeout);
    }

    // -- The client, over an injected transport -----------------------------

    #[derive(Debug)]
    struct Canned {
        response: std::sync::Mutex<Option<std::result::Result<HttpResponse, TransportError>>>,
        seen: std::sync::Mutex<Vec<HttpRequest>>,
    }

    impl Canned {
        fn ok(status: u16, body: &str) -> Arc<Canned> {
            Arc::new(Canned {
                response: std::sync::Mutex::new(Some(Ok(HttpResponse {
                    status,
                    content_type: Some("application/xml".into()),
                    body: body.as_bytes().to_vec(),
                }))),
                seen: std::sync::Mutex::new(Vec::new()),
            })
        }
        fn failing(e: TransportError) -> Arc<Canned> {
            Arc::new(Canned {
                response: std::sync::Mutex::new(Some(Err(e))),
                seen: std::sync::Mutex::new(Vec::new()),
            })
        }
        fn last(&self) -> HttpRequest {
            self.seen
                .lock()
                .expect("the recorder is not poisoned")
                .last()
                .cloned()
                .expect("a request")
        }
    }

    impl Transport for Canned {
        fn send<'a>(
            &'a self,
            request: HttpRequest,
        ) -> BoxFuture<'a, std::result::Result<HttpResponse, TransportError>> {
            Box::pin(async move {
                self.seen.lock().expect("the recorder is not poisoned").push(request);
                match self.response.lock().expect("the recorder is not poisoned").clone() {
                    Some(Ok(r)) => Ok(r),
                    Some(Err(e)) => Err(e),
                    None => Err(TransportError::Other("no canned response".into())),
                }
            })
        }
    }

    fn keys() -> S3Keys {
        S3Keys::new(Secret::from_str("AKIDEXAMPLE"), Secret::from_str(SUITE_SECRET))
    }

    fn block_on<F: Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime")
            .block_on(future)
    }

    #[test]
    fn list_buckets_signs_and_reads_the_answer() {
        let transport = Canned::ok(200, BUCKETS_XML);
        let client = S3Client::with_transport(transport.clone()).at(suite_time());
        let buckets = block_on(client.list_buckets(
            &provider("https://gateway.storjshare.io", "eu-1", true, false),
            &keys(),
        ))
        .expect("a listing");
        assert_eq!(buckets.len(), 2);

        let request = transport.last();
        assert_eq!(request.url, "https://gateway.storjshare.io/");
        let header = |name: &str| {
            request
                .headers
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };
        assert_eq!(header("x-amz-date"), "20150830T123600Z");
        assert_eq!(header("x-amz-content-sha256"), EMPTY_PAYLOAD_SHA256);
        assert!(header("authorization").contains("/20150830/eu-1/s3/aws4_request"));
        assert!(
            header("authorization").contains("SignedHeaders=host;x-amz-content-sha256;x-amz-date")
        );
        // `Host` is the transport's to set, from the URL, so that the signed
        // value and the sent value cannot drift apart.
        assert!(request.headers.iter().all(|(k, _)| k != "host"));
    }

    #[test]
    fn list_objects_addresses_path_style_and_virtual_hosted_correctly() {
        let listing = "<ListBucketResult><Name>b</Name><IsTruncated>false</IsTruncated>\
                       </ListBucketResult>";
        let transport = Canned::ok(200, listing);
        let client = S3Client::with_transport(transport.clone()).at(suite_time());

        block_on(client.list_objects_v2(
            &provider("https://gateway.storjshare.io", "eu-1", true, false),
            &keys(),
            "backups",
            "superbackup/pc/",
            50,
        ))
        .expect("a listing");
        assert_eq!(
            transport.last().url,
            "https://gateway.storjshare.io/backups\
             ?list-type=2&prefix=superbackup%2Fpc%2F&max-keys=50"
        );

        block_on(client.list_objects_v2(
            &provider("https://s3.amazonaws.com", "us-east-1", true, false),
            &keys(),
            "backups",
            "",
            0,
        ))
        .expect("a listing");
        assert_eq!(transport.last().url, "https://backups.s3.amazonaws.com/?list-type=2");
    }

    #[test]
    fn max_keys_is_clamped_to_what_s3_will_honour() {
        let transport = Canned::ok(
            200,
            "<ListBucketResult><IsTruncated>false</IsTruncated></ListBucketResult>",
        );
        let client = S3Client::with_transport(transport.clone()).at(suite_time());
        block_on(client.list_objects_v2(
            &provider("gateway.storjshare.io", "eu-1", true, false),
            &keys(),
            "backups",
            "",
            10_000,
        ))
        .expect("a listing");
        assert!(transport.last().url.ends_with("max-keys=1000"));
    }

    #[test]
    fn a_service_error_reaches_the_caller_as_its_own_variant() {
        let transport = Canned::ok(403, &error_body("SignatureDoesNotMatch", ""));
        let client = S3Client::with_transport(transport).at(suite_time());
        let error = block_on(
            client.list_buckets(&provider("gateway.storjshare.io", "eu-1", true, false), &keys()),
        )
        .expect_err("a refusal");
        assert_eq!(error, S3Error::SignatureDoesNotMatch);
    }

    #[test]
    fn a_transport_failure_reaches_the_caller_with_the_host_attached() {
        let transport = Canned::failing(TransportError::Dns("no record found".into()));
        let client = S3Client::with_transport(transport).at(suite_time());
        let error = block_on(
            client.list_buckets(&provider("gateway.storjshare.io", "eu-1", true, false), &keys()),
        )
        .expect_err("a failure");
        assert_eq!(
            error,
            S3Error::Dns { host: "gateway.storjshare.io".into(), detail: "no record found".into() }
        );
    }

    #[test]
    fn no_credential_reaches_an_error_message_or_a_debug_line() {
        // The one property that must hold for every path out of this module.
        const SECRET: &str = "sUperSecretKeyMaterial0123456789abcdefGH";
        const ACCESS: &str = "AKIAUNIQUEACCESSKEYID";
        let keys = S3Keys::new(Secret::from_str(ACCESS), Secret::from_str(SECRET))
            .with_session_token(Some(Secret::from_str("session-token-value-9876")));

        // A `Debug` of the credentials themselves reveals nothing.
        let rendered = format!("{keys:?}");
        assert!(!rendered.contains(SECRET));
        assert!(!rendered.contains("session-token-value"));

        for outcome in [
            Canned::ok(403, &error_body("SignatureDoesNotMatch", "")),
            Canned::ok(403, &error_body("InvalidAccessKeyId", "")),
            Canned::ok(200, "<html>not s3</html>"),
            Canned::failing(TransportError::Connect(format!(
                "connect error while sending Authorization: AWS4-HMAC-SHA256 \
                 Credential={ACCESS}, Signature=deadbeef"
            ))),
        ] {
            let client = S3Client::with_transport(outcome).at(suite_time());
            let error = block_on(
                client.list_buckets(&provider("gateway.storjshare.io", "eu-1", true, false), &keys),
            )
            .expect_err("a failure");
            let message = error.message();
            let debug = format!("{error:?}");
            for forbidden in [SECRET, "session-token-value-9876"] {
                assert!(!message.contains(forbidden), "{message}");
                assert!(!debug.contains(forbidden), "{debug}");
            }
            // The access key id is masked too when a gateway quotes our own
            // `Authorization` header back at us. The request *signature* is
            // deliberately not treated as credential material: it is a MAC
            // over one request that expires in fifteen minutes and cannot be
            // reversed to the key, and pretending otherwise would mean
            // redacting the one value a support diagnosis needs.
            assert!(!message.contains(ACCESS), "{message}");
        }
    }

    #[test]
    fn a_provider_with_no_endpoint_fails_before_any_request_is_made() {
        let transport = Canned::ok(200, BUCKETS_XML);
        let client = S3Client::with_transport(transport.clone()).at(suite_time());
        let error = block_on(client.list_buckets(&provider("", "eu-1", true, false), &keys()))
            .expect_err("a configuration error");
        assert!(matches!(error, S3Error::Configuration { .. }));
        assert!(
            transport.seen.lock().expect("the recorder is not poisoned").is_empty(),
            "nothing should have been sent"
        );
    }

    #[test]
    fn object_exists_asks_for_the_exact_key_and_matches_it_exactly() {
        let listing = "<ListBucketResult><Name>b</Name><IsTruncated>false</IsTruncated>                       <Contents><Key>p/kopia.repository</Key><Size>661</Size></Contents>                       </ListBucketResult>";
        let transport = Canned::ok(200, listing);
        let client = S3Client::with_transport(transport.clone()).at(suite_time());
        let p = provider("gateway.storjshare.io", "eu-1", true, false);
        assert!(block_on(client.object_exists(&p, &keys(), "b", "p/kopia.repository"))
            .expect("a listing"));
        assert!(transport.last().url.contains("prefix=p%2Fkopia.repository"));
        assert!(transport.last().url.contains("max-keys=1"));

        // A prefix match is not an exact match: `p/kopia.repository.bak` must
        // not be read as "there is a repository here".
        let near = "<ListBucketResult><Name>b</Name><IsTruncated>false</IsTruncated>                    <Contents><Key>p/kopia.repository.bak</Key><Size>1</Size></Contents>                    </ListBucketResult>";
        let client = S3Client::with_transport(Canned::ok(200, near)).at(suite_time());
        assert!(!block_on(client.object_exists(&p, &keys(), "b", "p/kopia.repository"))
            .expect("a listing"));
    }

    #[test]
    fn the_write_probe_puts_then_deletes_one_namespaced_object() {
        let transport = Canned::ok(204, "");
        let client = S3Client::with_transport(transport.clone()).at(suite_time());
        block_on(client.write_probe(
            &provider("gateway.storjshare.io", "eu-1", true, false),
            &keys(),
            "backups",
            "superbackup/pc/",
        ))
        .expect("a successful probe");

        let seen = transport.seen.lock().expect("the recorder is not poisoned").clone();
        assert_eq!(seen.len(), 2, "one PUT and one DELETE");
        assert_eq!(seen[0].method, "PUT");
        assert_eq!(seen[1].method, "DELETE");
        for request in &seen {
            assert_eq!(
                request.url,
                "https://gateway.storjshare.io/backups/superbackup/pc/.superbackup-write-test"
            );
        }
        assert_eq!(seen[0].body, b"superbackup");
        assert!(seen[1].body.is_empty());
        // The PUT signs the hash of the body it actually sends, not the empty
        // hash, or the server rejects it.
        let sha = |r: &HttpRequest| {
            r.headers
                .iter()
                .find(|(k, _)| k == "x-amz-content-sha256")
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };
        assert_eq!(sha(&seen[0]), hex_sha256(b"superbackup"));
        assert_eq!(sha(&seen[1]), EMPTY_PAYLOAD_SHA256);
    }

    #[test]
    fn a_read_only_key_fails_the_write_probe_and_says_so() {
        let transport = Canned::ok(403, &error_body("AccessDenied", ""));
        let client = S3Client::with_transport(transport).at(suite_time());
        let error = block_on(client.write_probe(
            &provider("gateway.storjshare.io", "eu-1", true, false),
            &keys(),
            "backups",
            "p/",
        ))
        .expect_err("a refusal");
        assert_eq!(error, S3Error::AccessDenied { operation: "write to this bucket" });
        assert!(error.message().contains("write to this bucket"), "{}", error.message());
    }

    #[test]
    fn an_empty_session_token_is_not_sent() {
        let transport = Canned::ok(200, BUCKETS_XML);
        let client = S3Client::with_transport(transport.clone()).at(suite_time());
        let keys = S3Keys::new(Secret::from_str("AKIDEXAMPLE"), Secret::from_str(SUITE_SECRET))
            .with_session_token(Some(Secret::from_str("")));
        block_on(client.list_buckets(&provider("gateway.storjshare.io", "", true, false), &keys))
            .expect("a listing");
        assert!(transport.last().headers.iter().all(|(k, _)| k != "x-amz-security-token"));
    }
}
