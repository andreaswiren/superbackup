//! The front-end side of the IPC surface.
//!
//! Three entry points, for three kinds of caller:
//!
//! * [`Client`] — async, cloneable, multiplexing. The tray and the GUI hold
//!   one for the life of the process and use it from many tasks at once.
//! * [`Subscription`] — a live stream of [`StreamItem`]s from the same
//!   connection, for `superbackup watch` and the GUI's progress bars.
//! * [`BlockingClient`] — the same thing for the CLI, which should not have
//!   to think about runtimes to ask a question and print an answer.
//!
//! # Multiplexing
//!
//! One connection, many outstanding requests. A driver task owns the socket;
//! [`Client::request`] allocates an id, parks a one-shot channel under it, and
//! awaits. Responses are matched back by id, which is why the protocol carries
//! one. This means a GUI can poll status while a subscription streams progress
//! and a user triggers a run, over a single pipe, with no locking beyond a
//! mutex around the pending-request map.
//!
//! # When nothing is listening
//!
//! The one error message that matters most in this whole module is the one a
//! user sees when the daemon is not running. `The system cannot find the file
//! specified. (os error 2)` is useless. [`Error::DaemonUnreachable`] carries
//! "the superbackup daemon is not running" and a hint that names the command
//! to fix it, and [`Client::connect`] maps every "nothing is listening" flavour
//! of OS error onto it — including Windows' `ERROR_FILE_NOT_FOUND` and
//! `ERROR_PIPE_BUSY`, which are not `NotFound` and `ConnectionRefused`.
//!
//! [`Client::connect_or_start`] goes further and offers to start the daemon.
//! It is opt-in because a CLI that silently spawns a background service the
//! user did not ask for is a surprise, and because in a service installation
//! starting a second, user-scoped daemon would be the wrong fix.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use interprocess::local_socket::tokio::prelude::*;
use interprocess::local_socket::tokio::Stream;
use interprocess::local_socket::GenericFilePath;
use tokio::io::BufReader;
use tokio::sync::{mpsc, oneshot};

use crate::error::{Error, ErrorCode, Result};
use crate::state::StatusSnapshot;

use super::codec::{self, LineError};
use super::protocol::{
    ClientFrame, ErrorPayload, Reply, Request, RequestId, SchemaReply, SecretString, ServerFrame,
    StatusReply, StreamItem, Topic, UnlockedReply, VersionReply,
};
use super::schema::Schema;
use super::{Limits, MIN_PROTOCOL_VERSION, PROTOCOL_VERSION};

/// Default ceiling on one request/response round trip.
///
/// Generous, because a `dest.test` against a cold S3 bucket legitimately takes
/// seconds and timing it out would turn a slow answer into a wrong one. Long
/// work does not happen inside a request — `job.run` returns a run id — so
/// nothing legitimate should approach this.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// Items buffered for one [`Subscription`] before the client starts dropping.
///
/// Mirrors the daemon's policy one hop closer to the consumer: a UI that
/// stalls loses the oldest items and is told, rather than growing a queue in
/// the client process.
const SUBSCRIPTION_DEPTH: usize = 256;

/// What the daemon said about itself when the connection opened.
#[derive(Debug, Clone)]
pub struct Hello {
    pub protocol: u32,
    pub min_protocol: u32,
    pub version: String,
    /// True when this is the machine-wide service instance.
    pub service_scope: bool,
}

/// A request awaiting its response.
enum Pending {
    /// An ordinary request/response.
    Once(oneshot::Sender<std::result::Result<Reply, ErrorPayload>>),
    /// A subscription: one reply, then items until `end`.
    Stream {
        reply: Option<oneshot::Sender<std::result::Result<Reply, ErrorPayload>>>,
        items: mpsc::Sender<StreamItem>,
        /// Items dropped because the consumer is not keeping up, reported as
        /// a [`StreamItem::Lagged`] as soon as there is room to say so.
        dropped: u64,
    },
}

/// The pending-request table, shared between the read loop and every clone of
/// the client. A plain `std::sync::Mutex`: every critical section is a hash
/// map lookup with no `await` inside, so an async mutex would buy nothing and
/// cost a scheduler hop on the hot path.
type PendingMap = Arc<Mutex<HashMap<RequestId, Pending>>>;

struct Inner {
    outbound: mpsc::Sender<ClientFrame>,
    pending: PendingMap,
    next_id: AtomicU64,
    hello: Hello,
    endpoint: String,
    timeout: Duration,
}

/// An asynchronous, cloneable connection to the daemon.
///
/// Cloning shares the connection; it does not open a second one. Dropping the
/// last clone closes the socket and ends every subscription taken from it.
#[derive(Debug, Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Inner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("endpoint", &self.endpoint)
            .field("daemon_version", &self.hello.version)
            .field("protocol", &self.hello.protocol)
            .finish()
    }
}

impl Client {
    /// Connect to a daemon listening on `endpoint`.
    ///
    /// `endpoint` comes from [`crate::paths::Paths::ipc_endpoint`]. The
    /// daemon's `hello` frame is read and version-checked before this returns,
    /// so a caller never sends a request — or a passphrase — to a daemon it
    /// cannot talk to.
    ///
    /// Returns [`Error::DaemonUnreachable`] when nothing is listening, so the
    /// CLI can say "start the tray app" instead of printing an OS error code.
    pub async fn connect(endpoint: &str) -> Result<Client> {
        Client::connect_with(endpoint, DEFAULT_TIMEOUT).await
    }

    /// [`connect`](Client::connect) with a non-default request timeout.
    pub async fn connect_with(endpoint: &str, timeout: Duration) -> Result<Client> {
        let name = endpoint
            .to_fs_name::<GenericFilePath>()
            .map_err(|e| Error::Ipc(format!("`{endpoint}` is not a usable IPC endpoint: {e}")))?;

        let stream = Stream::connect(name).await.map_err(|e| classify_connect_error(&e))?;

        let (recv_half, mut send_half) = stream.split();
        let mut reader = BufReader::new(recv_half);
        let mut line = Vec::with_capacity(1024);

        // The daemon greets first. Anything else is not a daemon we know.
        codec::read_line(&mut reader, &mut line, Limits::default().max_line_bytes)
            .await
            .map_err(|e| match e {
                LineError::Eof => Error::DaemonUnreachable,
                other => Error::Ipc(format!("reading the daemon's greeting: {other}")),
            })?;

        let hello = match codec::parse::<ServerFrame>(&line) {
            Ok(ServerFrame::Hello { protocol, min_protocol, version, service_scope }) => {
                Hello { protocol, min_protocol, version, service_scope }
            }
            Ok(ServerFrame::Error { body, .. }) => return Err(body.into_error()),
            Ok(other) => {
                return Err(Error::Ipc(format!(
                    "expected a hello frame from the daemon, got {}",
                    frame_kind(&other)
                )))
            }
            Err(e) => {
                return Err(Error::Ipc(format!(
                    "the process listening on {endpoint} is not a superbackup daemon: {e}"
                )))
            }
        };

        // Negotiate before anything is sent. A mismatch here is a clear,
        // actionable message; a mismatch discovered on request seventeen is a
        // support ticket.
        if PROTOCOL_VERSION < hello.min_protocol || PROTOCOL_VERSION > hello.protocol {
            return Err(Error::Ipc(format!(
                "this client speaks IPC protocol {PROTOCOL_VERSION} (accepts \
                 {MIN_PROTOCOL_VERSION}..={PROTOCOL_VERSION}) but the daemon at {endpoint} \
                 (version {}) speaks {}..={}; upgrade whichever is older",
                hello.version, hello.min_protocol, hello.protocol
            )));
        }

        let (outbound, mut outbox) = mpsc::channel::<ClientFrame>(64);
        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));

        // Writer: sole owner of the send half.
        tokio::spawn(async move {
            while let Some(frame) = outbox.recv().await {
                if codec::write_line(&mut send_half, &frame).await.is_err() {
                    break;
                }
            }
        });

        // Reader: sole owner of the receive half, and the only thing that
        // resolves a pending request.
        let reader_pending = Arc::clone(&pending);
        let max_line = Limits::default().max_line_bytes;
        tokio::spawn(async move {
            read_loop(reader, line, reader_pending, max_line).await;
        });

        Ok(Client {
            inner: Arc::new(Inner {
                outbound,
                pending,
                next_id: AtomicU64::new(1),
                hello,
                endpoint: endpoint.to_string(),
                timeout,
            }),
        })
    }

    /// Connect, starting the daemon first if nothing is listening.
    ///
    /// Opt-in, because silently spawning a background process the user did not
    /// ask for is a surprise, and because on a machine with the service
    /// installed the right fix is to start the *service*, not a second
    /// user-scoped daemon.
    ///
    /// Retries until [`AutoStart::wait`] elapses, then reports
    /// [`Error::DaemonUnreachable`] as if it had never tried, because from the
    /// user's point of view that is what happened.
    pub async fn connect_or_start(endpoint: &str, autostart: &AutoStart) -> Result<Client> {
        match Client::connect(endpoint).await {
            Ok(client) => return Ok(client),
            Err(Error::DaemonUnreachable) => {}
            Err(other) => return Err(other),
        }

        autostart.spawn()?;

        let deadline = std::time::Instant::now() + autostart.wait;
        let mut backoff = Duration::from_millis(25);
        loop {
            match Client::connect(endpoint).await {
                Ok(client) => return Ok(client),
                Err(Error::DaemonUnreachable) if std::time::Instant::now() < deadline => {
                    tokio::time::sleep(backoff).await;
                    // Back off so a daemon that takes two seconds to open its
                    // endpoint is not hammered a thousand times first.
                    backoff = (backoff * 2).min(Duration::from_millis(250));
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// What the daemon said about itself when the connection opened.
    pub fn hello(&self) -> &Hello {
        &self.inner.hello
    }

    /// The endpoint this client is connected to.
    pub fn endpoint(&self) -> &str {
        &self.inner.endpoint
    }

    /// Send a request and await its typed reply.
    ///
    /// A daemon-side failure comes back as an [`Error`] carrying the same
    /// [`ErrorCode`](crate::error::ErrorCode) the daemon sent, so a caller can
    /// branch on `error.code()` exactly as it would in-process.
    pub async fn request(&self, request: Request) -> Result<Reply> {
        let id = self.inner.next_request_id();
        let (tx, rx) = oneshot::channel();
        self.inner.register(id, Pending::Once(tx));

        let frame =
            ClientFrame::Request { id, protocol: PROTOCOL_VERSION, body: request };
        if self.inner.outbound.send(frame).await.is_err() {
            self.inner.forget(id);
            return Err(Error::DaemonUnreachable);
        }

        match tokio::time::timeout(self.inner.timeout, rx).await {
            Ok(Ok(Ok(reply))) => Ok(reply),
            Ok(Ok(Err(payload))) => Err(payload.into_error()),
            // The reader task dropped the sender: the connection is gone.
            Ok(Err(_)) => Err(Error::DaemonUnreachable),
            Err(_) => {
                self.inner.forget(id);
                Err(Error::Ipc(format!(
                    "the daemon did not answer within {}s",
                    self.inner.timeout.as_secs()
                )))
            }
        }
    }

    /// Open a live stream of events, progress and status.
    ///
    /// An empty `topics` list means every topic. The stream is registered
    /// *before* the request is sent, so an item published between the
    /// daemon's reply and this function returning is not lost.
    pub async fn subscribe(&self, topics: Vec<Topic>) -> Result<Subscription> {
        let id = self.inner.next_request_id();
        let (reply_tx, reply_rx) = oneshot::channel();
        let (items_tx, items_rx) = mpsc::channel(SUBSCRIPTION_DEPTH);
        self.inner
            .register(id, Pending::Stream { reply: Some(reply_tx), items: items_tx, dropped: 0 });

        let frame = ClientFrame::Request {
            id,
            protocol: PROTOCOL_VERSION,
            body: Request::Subscribe { topics: topics.clone() },
        };
        if self.inner.outbound.send(frame).await.is_err() {
            self.inner.forget(id);
            return Err(Error::DaemonUnreachable);
        }

        let confirmed = match tokio::time::timeout(self.inner.timeout, reply_rx).await {
            Ok(Ok(Ok(Reply::Subscribed(s)))) => s,
            Ok(Ok(Ok(other))) => {
                self.inner.forget(id);
                return Err(Error::Ipc(format!(
                    "expected a `subscribed` reply, got `{}`",
                    other.tag()
                )));
            }
            Ok(Ok(Err(payload))) => {
                self.inner.forget(id);
                return Err(payload.into_error());
            }
            Ok(Err(_)) => return Err(Error::DaemonUnreachable),
            Err(_) => {
                self.inner.forget(id);
                return Err(Error::Ipc("the daemon did not confirm the subscription".into()));
            }
        };

        Ok(Subscription {
            id,
            topics: confirmed.topics,
            items: items_rx,
            outbound: self.inner.outbound.clone(),
        })
    }

    // -- convenience ------------------------------------------------------
    //
    // A handful of wrappers for the calls every front end makes. Everything
    // else goes through `request`, which keeps this list from becoming a
    // second, hand-maintained copy of the command surface.

    /// Cheapest possible round trip. Used to confirm a freshly started daemon
    /// is actually serving.
    pub async fn ping(&self) -> Result<()> {
        self.request(Request::Ping {}).await.map(|_| ())
    }

    /// The full runtime snapshot.
    pub async fn status(&self) -> Result<StatusSnapshot> {
        match self.request(Request::Status {}).await? {
            Reply::Status(StatusReply { snapshot }) => Ok(*snapshot),
            other => Err(unexpected("status", &other)),
        }
    }

    /// Build and protocol identity.
    pub async fn version(&self) -> Result<VersionReply> {
        match self.request(Request::Version {}).await? {
            Reply::Version(v) => Ok(v),
            other => Err(unexpected("version", &other)),
        }
    }

    /// The daemon's own description of everything it can do.
    pub async fn schema(&self) -> Result<Schema> {
        match self.request(Request::Schema {}).await? {
            Reply::Schema(SchemaReply { schema }) => Ok(*schema),
            other => Err(unexpected("schema", &other)),
        }
    }

    /// Unlock the vault.
    ///
    /// Takes a [`SecretString`] rather than a `String` so the caller is
    /// pushed into holding the passphrase in a zeroing buffer too.
    pub async fn unlock(&self, passphrase: SecretString) -> Result<UnlockedReply> {
        match self.request(Request::VaultUnlock { passphrase }).await? {
            Reply::Unlocked(u) => Ok(u),
            other => Err(unexpected("unlocked", &other)),
        }
    }
}

fn unexpected(want: &str, got: &Reply) -> Error {
    Error::Ipc(format!("expected a `{want}` reply, got `{}`", got.tag()))
}

fn frame_kind(frame: &ServerFrame) -> &'static str {
    match frame {
        ServerFrame::Hello { .. } => "hello",
        ServerFrame::Ok { .. } => "ok",
        ServerFrame::Error { .. } => "error",
        ServerFrame::Stream { .. } => "stream",
        ServerFrame::End { .. } => "end",
        ServerFrame::Bye { .. } => "bye",
    }
}

impl Inner {
    fn next_request_id(&self) -> RequestId {
        RequestId(self.next_id.fetch_add(1, Ordering::Relaxed))
    }

    fn register(&self, id: RequestId, pending: Pending) {
        if let Ok(mut map) = self.pending.lock() {
            map.insert(id, pending);
        }
    }

    fn forget(&self, id: RequestId) {
        if let Ok(mut map) = self.pending.lock() {
            map.remove(&id);
        }
    }
}

/// Read frames until the connection ends, resolving pending requests.
async fn read_loop<R>(
    mut reader: BufReader<R>,
    mut line: Vec<u8>,
    pending: PendingMap,
    max_line: usize,
) where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut goodbye: Option<String> = None;

    loop {
        match codec::read_line(&mut reader, &mut line, max_line).await {
            Ok(()) => {}
            Err(_) => break,
        }
        if line.is_empty() {
            continue;
        }

        let frame: ServerFrame = match codec::parse(&line) {
            Ok(f) => f,
            Err(e) => {
                // A frame this client cannot parse is the daemon's problem,
                // not this connection's: skip it and keep serving the
                // requests that can still be answered.
                tracing::warn!(error = %e, "unparseable IPC frame from the daemon");
                continue;
            }
        };

        match frame {
            ServerFrame::Hello { .. } => {}
            ServerFrame::Ok { id, body } => resolve(&pending, id, Ok(*body)),
            ServerFrame::Error { id, body } => {
                if id == RequestId(0) {
                    // Not attributable to a request: a malformed line we sent,
                    // or a connection-level refusal.
                    tracing::warn!(code = ?body.code, message = %body.message, "IPC error");
                    continue;
                }
                resolve(&pending, id, Err(body));
            }
            ServerFrame::Stream { id, body } => deliver(&pending, id, *body),
            ServerFrame::End { id } => {
                if let Ok(mut map) = pending.lock() {
                    map.remove(&id);
                }
            }
            ServerFrame::Bye { reason } => goodbye = Some(reason),
        }
    }

    // The connection is gone. Fail everything still waiting rather than
    // leaving a caller blocked until its timeout: "the daemon stopped" now is
    // better than "no answer" in sixty seconds.
    let message = goodbye.unwrap_or_else(|| "the connection to the daemon was closed".to_string());
    if let Ok(mut map) = pending.lock() {
        for (_, entry) in map.drain() {
            if let Pending::Once(tx) | Pending::Stream { reply: Some(tx), .. } = entry {
                let _ = tx.send(Err(ErrorPayload::new(ErrorCode::DaemonUnreachable, &message)));
            }
        }
    }

    use zeroize::Zeroize;
    line.zeroize();
}

fn resolve(
    pending: &Mutex<HashMap<RequestId, Pending>>,
    id: RequestId,
    outcome: std::result::Result<Reply, ErrorPayload>,
) {
    let Ok(mut map) = pending.lock() else { return };
    match map.get_mut(&id) {
        Some(Pending::Stream { reply, .. }) => {
            if let Some(tx) = reply.take() {
                let failed = outcome.is_err();
                let _ = tx.send(outcome);
                if failed {
                    map.remove(&id);
                }
            }
        }
        Some(Pending::Once(_)) => {
            if let Some(Pending::Once(tx)) = map.remove(&id) {
                let _ = tx.send(outcome);
            }
        }
        // A response to a request that timed out and was forgotten, or a
        // duplicate id from a broken daemon. Neither is worth a fuss.
        None => {}
    }
}

fn deliver(pending: &Mutex<HashMap<RequestId, Pending>>, id: RequestId, item: StreamItem) {
    let Ok(mut map) = pending.lock() else { return };
    let Some(Pending::Stream { items, dropped, .. }) = map.get_mut(&id) else { return };

    // Never `await` while holding the map lock, and never block the reader on
    // one slow consumer: the same drop-oldest policy the daemon applies, one
    // hop closer to the consumer.
    if *dropped > 0 {
        if items.try_send(StreamItem::Lagged { missed: *dropped }).is_ok() {
            *dropped = 0;
        } else {
            *dropped += 1;
            return;
        }
    }
    if items.try_send(item).is_err() {
        *dropped += 1;
    }
}

/// A live stream of daemon events on one request id.
///
/// Dropping it cancels the subscription: the daemon is sent a `cancel` frame
/// and stops producing, so a closed window does not leave the daemon
/// serialising progress nobody reads.
#[derive(Debug)]
pub struct Subscription {
    id: RequestId,
    topics: Vec<Topic>,
    items: mpsc::Receiver<StreamItem>,
    outbound: mpsc::Sender<ClientFrame>,
}

impl Subscription {
    /// The next item, or `None` when the daemon ended the stream or the
    /// connection closed.
    ///
    /// Watch for [`StreamItem::Lagged`]: it means items were dropped and the
    /// consumer should resynchronise with `status` rather than assume
    /// continuity.
    pub async fn next(&mut self) -> Option<StreamItem> {
        self.items.recv().await
    }

    /// The topics the daemon confirmed, which may be broader than the request
    /// when it asked for everything.
    pub fn topics(&self) -> &[Topic] {
        &self.topics
    }

    /// The request id this stream is carried on.
    pub fn id(&self) -> RequestId {
        self.id
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        // Best effort and non-blocking: if the queue to the writer is full or
        // the connection is already gone, the daemon finds out when the socket
        // closes.
        let _ = self.outbound.try_send(ClientFrame::Cancel { id: self.id });
    }
}

// ---------------------------------------------------------------------------
// Starting the daemon
// ---------------------------------------------------------------------------

/// How to start a daemon that is not running.
///
/// Deliberately explicit about *what* to run rather than guessing: on a
/// machine with the service installed, the answer is "start the service", and
/// only the caller knows that.
#[derive(Debug, Clone)]
pub struct AutoStart {
    /// Program to run.
    pub program: PathBuf,
    /// Arguments to pass.
    pub args: Vec<String>,
    /// How long to keep retrying the connection after spawning.
    pub wait: Duration,
}

impl AutoStart {
    /// Run this executable again with the given arguments, which for
    /// superbackup means `superbackup daemon`.
    ///
    /// Uses the current executable rather than a `PATH` lookup so that a
    /// portable install starts *itself* and not a different copy that happens
    /// to be earlier in `PATH`.
    pub fn current_exe(args: &[&str]) -> Result<AutoStart> {
        let program = std::env::current_exe()
            .map_err(|e| Error::io("locating this executable to start the daemon", e))?;
        Ok(AutoStart {
            program,
            args: args.iter().map(|s| (*s).to_string()).collect(),
            wait: Duration::from_secs(10),
        })
    }

    /// Spawn the daemon, detached.
    ///
    /// Standard streams go to null: the daemon logs to its own file, and a
    /// child inheriting the CLI's stdout would interleave its log with the
    /// command's output, which is how `--json` gets corrupted.
    fn spawn(&self) -> Result<()> {
        use std::process::{Command, Stdio};
        Command::new(&self.program)
            .args(&self.args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| {
                Error::io(format!("starting the daemon ({})", self.program.display()), e)
            })?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Blocking wrapper
// ---------------------------------------------------------------------------

/// A synchronous client, for the CLI.
///
/// `superbackup status` should not have to know what a runtime is. This owns a
/// minimal current-thread runtime internally and hides it: construct one, call
/// methods, print, exit.
///
/// Do not use it from inside an async context — driving a runtime from a
/// runtime worker thread panics. Async callers already have [`Client`].
#[derive(Debug)]
pub struct BlockingClient {
    runtime: tokio::runtime::Runtime,
    client: Client,
}

impl BlockingClient {
    /// Connect, blocking until the daemon greets or the attempt fails.
    pub fn connect(endpoint: &str) -> Result<BlockingClient> {
        let runtime = build_runtime()?;
        let client = runtime.block_on(Client::connect(endpoint))?;
        Ok(BlockingClient { runtime, client })
    }

    /// Connect, starting the daemon first if nothing is listening.
    pub fn connect_or_start(endpoint: &str, autostart: &AutoStart) -> Result<BlockingClient> {
        let runtime = build_runtime()?;
        let client = runtime.block_on(Client::connect_or_start(endpoint, autostart))?;
        Ok(BlockingClient { runtime, client })
    }

    /// Send a request and wait for its reply.
    pub fn request(&self, request: Request) -> Result<Reply> {
        self.runtime.block_on(self.client.request(request))
    }

    /// The full runtime snapshot.
    pub fn status(&self) -> Result<StatusSnapshot> {
        self.runtime.block_on(self.client.status())
    }

    /// The daemon's own description of everything it can do, for
    /// `superbackup help --json`.
    pub fn schema(&self) -> Result<Schema> {
        self.runtime.block_on(self.client.schema())
    }

    /// What the daemon said about itself when the connection opened.
    pub fn hello(&self) -> &Hello {
        self.client.hello()
    }

    /// Run a subscription to completion, calling `on_item` for each item.
    ///
    /// This is what `superbackup watch` is: a blocking loop over a stream.
    /// Return `false` from `on_item` to stop.
    pub fn watch<F>(&self, topics: Vec<Topic>, mut on_item: F) -> Result<()>
    where
        F: FnMut(StreamItem) -> bool,
    {
        self.runtime.block_on(async {
            let mut subscription = self.client.subscribe(topics).await?;
            while let Some(item) = subscription.next().await {
                if !on_item(item) {
                    break;
                }
            }
            Ok(())
        })
    }

    /// The underlying async client, for a caller that has both worlds.
    pub fn inner(&self) -> &Client {
        &self.client
    }
}

fn build_runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| Error::io("creating a runtime for the IPC client", e))
}

/// Map an OS connect error onto [`Error::DaemonUnreachable`] where that is
/// what it means.
///
/// The interesting cases are the ones that are *not* `NotFound` or
/// `ConnectionRefused`:
///
/// * `ERROR_FILE_NOT_FOUND` (2) — the named pipe does not exist. Windows
///   surfaces this as `NotFound`, but only sometimes.
/// * `ERROR_PIPE_BUSY` (231) — the pipe exists but every instance is in use.
///   A daemon *is* running; it is just momentarily saturated. Reported as
///   unreachable because the user's next action is the same either way, but
///   with its own message so a support log distinguishes them.
fn classify_connect_error(e: &std::io::Error) -> Error {
    use std::io::ErrorKind::*;
    match e.kind() {
        NotFound | ConnectionRefused | AddrNotAvailable => Error::DaemonUnreachable,
        PermissionDenied => Error::Ipc(format!(
            "not permitted to connect to the superbackup daemon: {e}. The daemon may be \
             running as another user."
        )),
        _ => match e.raw_os_error() {
            Some(2) => Error::DaemonUnreachable,
            Some(231) => Error::Ipc(
                "the superbackup daemon is busy and could not accept another connection; \
                 try again in a moment"
                    .into(),
            ),
            _ => Error::Ipc(format!("could not reach the superbackup daemon: {e}")),
        },
    }
}
