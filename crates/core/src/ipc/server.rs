//! The daemon side of the IPC surface.
//!
//! [`Server::bind`] hardens and opens the endpoint; [`Server::serve`] accepts
//! connections until a [`ServerHandle`] asks it to stop.
//!
//! # Shape of a connection
//!
//! ```text
//!                      ┌── reader loop ──┐  parses one line at a time,
//!   socket ─▶ split ─▶ │                 │  rate limited, size capped
//!                      └── writer task ──┘  sole owner of the send half
//!                                ▲
//!               ┌────────────────┼────────────────┐
//!          request tasks    subscription pumps    hello / bye
//! ```
//!
//! One task owns the send half and every other task reaches it through a
//! bounded channel. That is what makes it safe to answer requests
//! concurrently and stream events at the same time: two producers can never
//! interleave halves of a frame on the wire, and a slow socket applies
//! backpressure to producers instead of growing an unbounded queue.
//!
//! # Concurrency and ordering
//!
//! Requests on one connection are answered **concurrently and possibly out of
//! order**, bounded by [`Limits::max_inflight`]. That is why every frame
//! carries an id. A client that needs ordering must wait for a response
//! before sending the next request — which is what the client in this crate
//! does for everything except subscriptions.
//!
//! # Isolation
//!
//! Each request runs in its own task, so a handler that panics fails that one
//! request with an `internal` error and leaves the connection, the other
//! in-flight requests and the daemon running. A handler that hangs is
//! abandoned after [`Limits::handler_timeout`] with an `ipc` error rather than
//! being allowed to occupy the connection forever.
//!
//! # Backpressure on subscriptions
//!
//! The daemon's event fan-out is a [`tokio::sync::broadcast`] channel, so
//! publishing never blocks on a slow subscriber. A subscriber that falls
//! behind loses the *oldest* items and is told how many with
//! [`StreamItem::Lagged`], which is a signal to resynchronise with `status`
//! rather than an error. If a client stops reading its socket entirely, the
//! writer's bounded channel fills, the connection stops making progress, and
//! after a send timeout it is closed — the engine is never involved.
//!
//! # Graceful shutdown
//!
//! [`ServerHandle::shutdown`] stops the accept loop and tells every live
//! connection to finish. Each one ends its subscriptions with `end`, sends
//! `bye`, flushes and closes, so a connected client reports "the daemon
//! stopped" rather than "the pipe broke". [`Server::serve`] returns only once
//! every connection task has finished.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use interprocess::local_socket::tokio::prelude::*;
use interprocess::local_socket::tokio::{Listener, Stream};
use interprocess::local_socket::{GenericFilePath, ListenerOptions};
use tokio::io::BufReader;
use tokio::sync::{broadcast, mpsc, watch, Semaphore};
use tokio::task::JoinSet;

use crate::error::{Error, ErrorCode, Result};

use super::codec::{self, LineError};
use super::protocol::{
    check_protocol, dispatch, schema_with_limits, ClientFrame, ErrorPayload, Handler, PeerIdentity,
    Reply, Request, RequestContext, RequestId, SchemaReply, ServerFrame, StreamItem,
    SubscribedReply, Topic,
};
use super::security;
use super::Limits;
use super::{MIN_PROTOCOL_VERSION, PROTOCOL_VERSION};

/// How long the transport will wait to hand a frame to the writer before
/// giving up on the connection.
///
/// Reaching this means the peer has stopped reading its socket entirely for
/// this long. There is no useful way to serve such a client, and continuing to
/// hold its slot denies it to a working one.
const WRITE_QUEUE_TIMEOUT: Duration = Duration::from_secs(30);

/// Consecutive rate-limit violations before the connection is closed.
///
/// One violation is a burst; sixty-four in a row is a client in a spin loop,
/// and answering it forever costs more than disconnecting it.
const MAX_RATE_VIOLATIONS: u32 = 64;

/// Consecutive `accept` failures before the accept loop gives up.
///
/// Prevents a permanently broken listener from spinning a core.
const MAX_ACCEPT_FAILURES: u32 = 16;

/// How the server is configured.
///
/// Every field has a defensible default, so a daemon that just wants to serve
/// can write `ServerOptions::default()` and get the safe behaviour.
#[derive(Debug, Clone, Default)]
pub struct ServerOptions {
    /// Transport limits. See [`Limits`].
    pub limits: Limits,
    /// Take over an endpoint that already exists.
    ///
    /// Off by default, and that default is a safety property, not a
    /// preference: two daemons driving the same repositories is a data-loss
    /// bug, so `bind` refuses and says so instead of silently displacing a
    /// running instance. Single-instance enforcement belongs to the lock file
    /// ([`crate::paths::Paths::lock_file`]); this flag exists for the
    /// recovery case where a crashed daemon left a corpse socket behind.
    pub replace_existing: bool,
}

/// A remote control for a running [`Server`].
///
/// Cloneable and cheap. Hand one to a signal handler, to the service control
/// dispatcher, and to whatever implements `control.shutdown`.
#[derive(Debug, Clone)]
pub struct ServerHandle {
    shutdown: Arc<watch::Sender<bool>>,
}

impl ServerHandle {
    /// Ask the server to stop accepting and to close its connections politely.
    ///
    /// Returns immediately; [`Server::serve`] returns once the last connection
    /// has finished. Calling it twice is harmless.
    ///
    /// `send_replace` rather than `send`: `send` fails and *discards the
    /// value* when there are no receivers, which would silently lose a
    /// shutdown requested between [`Server::bind`] and [`Server::serve`] —
    /// exactly the window a signal handler installed at startup lives in.
    pub fn shutdown(&self) {
        self.shutdown.send_replace(true);
    }

    /// Whether shutdown has already been requested.
    pub fn is_shutting_down(&self) -> bool {
        *self.shutdown.borrow()
    }
}

/// The IPC server.
///
/// Generic over the [`Handler`] rather than boxed, so handler methods can
/// return `impl Future` and the daemon pays no dynamic dispatch on a hot path
/// like progress streaming.
#[derive(Debug)]
pub struct Server<H: Handler> {
    listener: Listener,
    endpoint: String,
    handler: Arc<H>,
    limits: Limits,
    shutdown: Arc<watch::Sender<bool>>,
    /// Subscribed at bind time, so a shutdown requested before `serve` is
    /// polled is still observed rather than marked "already seen".
    shutdown_rx: watch::Receiver<bool>,
    connections: Arc<Semaphore>,
    next_connection_id: AtomicU64,
}

impl<H: Handler> Server<H> {
    /// Open and harden the endpoint.
    ///
    /// `endpoint` comes from [`crate::paths::Paths::ipc_endpoint`], which
    /// already picks a named pipe path on Windows, a socket path elsewhere,
    /// and a distinct name for the machine-wide service instance.
    ///
    /// Must be called inside a Tokio runtime: the listener registers with the
    /// reactor as it is created.
    ///
    /// Access control is applied *before* the endpoint becomes visible — see
    /// [`security`]. If it cannot be applied, this fails
    /// rather than opening an endpoint anyone could reach.
    pub fn bind(endpoint: &str, handler: Arc<H>, options: ServerOptions) -> Result<Server<H>> {
        security::prepare_endpoint(endpoint)?;

        let name = endpoint
            .to_fs_name::<GenericFilePath>()
            .map_err(|e| Error::Ipc(format!("`{endpoint}` is not a usable IPC endpoint: {e}")))?;

        let opts = ListenerOptions::new().name(name).try_overwrite(options.replace_existing);
        let opts = security::harden_listener(opts)?;

        let listener = opts.create_tokio().map_err(|e| match e.kind() {
            std::io::ErrorKind::AddrInUse => Error::Ipc(format!(
                "another superbackup daemon is already listening on {endpoint}"
            )),
            std::io::ErrorKind::PermissionDenied => Error::Ipc(format!(
                "not permitted to create the IPC endpoint {endpoint}: {e}"
            )),
            _ => Error::Ipc(format!("could not create the IPC endpoint {endpoint}: {e}")),
        })?;

        security::finalise_endpoint(endpoint)?;

        let (shutdown, shutdown_rx) = watch::channel(false);
        Ok(Server {
            listener,
            endpoint: endpoint.to_string(),
            handler,
            connections: Arc::new(Semaphore::new(options.limits.max_connections)),
            limits: options.limits,
            shutdown: Arc::new(shutdown),
            shutdown_rx,
            next_connection_id: AtomicU64::new(1),
        })
    }

    /// The endpoint this server is listening on.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// A handle for shutting the server down from elsewhere.
    ///
    /// Take this *before* calling [`serve`](Self::serve), which consumes the
    /// server.
    pub fn handle(&self) -> ServerHandle {
        ServerHandle { shutdown: Arc::clone(&self.shutdown) }
    }

    /// Accept connections until shutdown, then wait for them to finish.
    ///
    /// Never returns an error for a per-connection problem: a broken client is
    /// logged and forgotten. An `Err` here means the listener itself failed
    /// repeatedly, which is fatal to the daemon's usefulness and should be
    /// reported as such.
    pub async fn serve(self) -> Result<()> {
        let mut shutdown_rx = self.shutdown_rx.clone();
        let mut connections: JoinSet<()> = JoinSet::new();
        let mut accept_failures = 0u32;

        tracing::info!(endpoint = %self.endpoint, "IPC server listening");

        while !*shutdown_rx.borrow() {
            tokio::select! {
                // Shutdown wins a tie: once asked to stop, do not pick up one
                // more connection that will immediately be told to go away.
                biased;

                // Only ever set to `true`, and a dropped sender is equally a
                // reason to stop, so any completion means "shut down".
                _ = shutdown_rx.changed() => break,

                accepted = self.listener.accept() => {
                    match accepted {
                        Ok(stream) => {
                            accept_failures = 0;
                            self.spawn_connection(stream, &mut connections);
                        }
                        Err(e) => {
                            accept_failures += 1;
                            tracing::warn!(error = %e, "IPC accept failed");
                            if accept_failures >= MAX_ACCEPT_FAILURES {
                                return Err(Error::Ipc(format!(
                                    "IPC listener failed {accept_failures} times in a row; \
                                     last error: {e}"
                                )));
                            }
                        }
                    }
                }

                // Reap finished connections so the JoinSet does not grow for
                // the life of the daemon.
                Some(_) = connections.join_next() => {}
            }
        }

        tracing::info!(
            endpoint = %self.endpoint,
            live = connections.len(),
            "IPC server shutting down"
        );

        // Connections observe the same watch channel and are already winding
        // down; wait for each to say goodbye and close.
        while connections.join_next().await.is_some() {}
        Ok(())
    }

    fn spawn_connection(&self, stream: Stream, set: &mut JoinSet<()>) {
        let permit = match Arc::clone(&self.connections).try_acquire_owned() {
            Ok(p) => p,
            Err(_) => {
                // Refuse politely and immediately. A client that is told
                // "too many connections" can retry; one that is silently
                // queued cannot tell a busy daemon from a hung one.
                let limit = self.limits.max_connections;
                let service_scope = false;
                set.spawn(async move {
                    let mut stream = stream;
                    let hello = ServerFrame::Hello {
                        protocol: PROTOCOL_VERSION,
                        min_protocol: MIN_PROTOCOL_VERSION,
                        version: crate::VERSION.to_string(),
                        service_scope,
                    };
                    let _ = codec::write_line(&mut stream, &hello).await;
                    let refusal = ServerFrame::Error {
                        id: RequestId(0),
                        body: ErrorPayload::new(
                            ErrorCode::Ipc,
                            format!("the daemon is already serving {limit} connections"),
                        )
                        .with_hint("Close an unused superbackup window and try again."),
                    };
                    let _ = codec::write_line(&mut stream, &refusal).await;
                    let _ = codec::write_line(
                        &mut stream,
                        &ServerFrame::Bye { reason: "connection limit reached".into() },
                    )
                    .await;
                });
                tracing::warn!(limit, "refused an IPC connection: limit reached");
                return;
            }
        };

        let id = self.next_connection_id.fetch_add(1, Ordering::Relaxed);
        let handler = Arc::clone(&self.handler);
        let limits = self.limits;
        let shutdown = self.shutdown.subscribe();

        set.spawn(async move {
            let _permit = permit;
            if let Err(e) = serve_connection(stream, id, handler, limits, shutdown).await {
                tracing::debug!(connection = id, error = %e, "IPC connection ended");
            }
        });
    }
}

// ---------------------------------------------------------------------------
// One connection
// ---------------------------------------------------------------------------

/// Token bucket, one per connection.
///
/// Not a sliding window: a bucket lets a GUI open and fire twenty requests to
/// paint a dashboard (a burst) while still refusing a process that sustains
/// twenty a second forever, which is the shape of the traffic this needs to
/// tell apart.
#[derive(Debug)]
struct RateLimiter {
    tokens: f64,
    burst: f64,
    rate: f64,
    last: Instant,
}

impl RateLimiter {
    fn new(limits: &Limits) -> RateLimiter {
        RateLimiter {
            tokens: f64::from(limits.request_burst),
            burst: f64::from(limits.request_burst),
            rate: f64::from(limits.max_requests_per_second),
            last: Instant::now(),
        }
    }

    fn allow(&mut self) -> bool {
        let now = Instant::now();
        self.tokens = (self.tokens + now.duration_since(self.last).as_secs_f64() * self.rate)
            .min(self.burst);
        self.last = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// The outbound side of one connection.
///
/// Cloned into every request task and subscription pump. Nothing else may
/// touch the socket's send half.
#[derive(Debug, Clone)]
struct Outbound(mpsc::Sender<ServerFrame>);

impl Outbound {
    /// Queue a frame, sanitising it on the way.
    ///
    /// `Err` means the connection is finished — the writer is gone, or the
    /// peer has not read anything for [`WRITE_QUEUE_TIMEOUT`]. Either way the
    /// caller should stop.
    async fn send(&self, frame: ServerFrame) -> std::result::Result<(), ()> {
        match self.0.send_timeout(frame.sanitise(), WRITE_QUEUE_TIMEOUT).await {
            Ok(()) => Ok(()),
            Err(_) => Err(()),
        }
    }
}

async fn serve_connection<H: Handler>(
    stream: Stream,
    connection_id: u64,
    handler: Arc<H>,
    limits: Limits,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    // Access control first, before a byte of protocol is parsed.
    let peer = match security::verify_peer(&stream) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(connection = connection_id, error = %e, "rejected an IPC peer");
            return Err(e);
        }
    };

    let (recv_half, mut send_half) = stream.split();
    // `stream_buffer` frames may sit here before producers feel backpressure.
    // This is the published limit, so what a client is told matches what it
    // gets.
    let (tx, mut rx) = mpsc::channel::<ServerFrame>(limits.stream_buffer.max(1));
    let out = Outbound(tx);

    // Sole owner of the send half.
    let writer = tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            if codec::write_line(&mut send_half, &frame).await.is_err() {
                break;
            }
        }
    });

    let result = connection_loop(
        recv_half,
        connection_id,
        peer,
        handler,
        limits,
        &mut shutdown,
        out.clone(),
    )
    .await;

    // Dropping the last `Outbound` closes the writer's channel, which ends the
    // writer task once it has drained what is queued — including the `bye`.
    drop(out);
    let _ = writer.await;
    result
}

#[allow(clippy::too_many_arguments)]
async fn connection_loop<H: Handler>(
    recv_half: interprocess::local_socket::tokio::RecvHalf,
    connection_id: u64,
    peer: PeerIdentity,
    handler: Arc<H>,
    limits: Limits,
    shutdown: &mut watch::Receiver<bool>,
    out: Outbound,
) -> Result<()> {
    let mut reader = BufReader::new(recv_half);
    let mut line = Vec::with_capacity(4096);
    let mut limiter = RateLimiter::new(&limits);
    let mut violations = 0u32;
    let mut inflight: JoinSet<()> = JoinSet::new();
    let mut subscriptions: std::collections::HashMap<RequestId, tokio::task::JoinHandle<()>> =
        std::collections::HashMap::new();

    // Greet unprompted, so a client learns the protocol range before it sends
    // anything — including before it sends a passphrase. Sent even to a
    // connection that is about to be turned away, so the client's own
    // handshake completes and it reports "the daemon stopped" rather than
    // "that was not a superbackup daemon".
    if out
        .send(ServerFrame::Hello {
            protocol: PROTOCOL_VERSION,
            min_protocol: MIN_PROTOCOL_VERSION,
            version: crate::VERSION.to_string(),
            service_scope: false,
        })
        .await
        .is_err()
    {
        return Ok(());
    }

    // A connection accepted in the instant before shutdown was requested has a
    // receiver that already considers `true` seen, so `changed()` below would
    // never fire for it. Check the value directly once.
    if *shutdown.borrow() {
        let _ = out.send(ServerFrame::Bye { reason: "the daemon is shutting down".into() }).await;
        return Ok(());
    }

    let stop_reason = loop {
        tokio::select! {
            biased;

            _ = shutdown.changed() => break Some("the daemon is shutting down"),

            read = codec::read_line(&mut reader, &mut line, limits.max_line_bytes) => {
                match read {
                    Ok(()) => {}
                    Err(LineError::Eof) => break None,
                    Err(LineError::TooLong { limit }) => {
                        // Cannot resynchronise: the rest of the oversized line
                        // is still in the stream and is attacker-chosen.
                        let _ = out.send(ServerFrame::Error {
                            id: RequestId(0),
                            body: ErrorPayload::new(
                                ErrorCode::Ipc,
                                format!("request line exceeds the {limit}-byte limit"),
                            )
                            .with_hint("Split the work into smaller requests."),
                        }).await;
                        break Some("oversized request line");
                    }
                    Err(LineError::Io(e)) => {
                        tracing::debug!(connection = connection_id, error = %e, "IPC read failed");
                        break None;
                    }
                }

                // A blank line is a keep-alive. Costs nothing, and lets a
                // client hold a connection open without a ping command.
                if line.is_empty() {
                    continue;
                }

                let frame: ClientFrame = match codec::parse(&line) {
                    Ok(f) => f,
                    Err(e) => {
                        // Malformed input is answered, never fatal: a client
                        // one version ahead sending an unknown command must
                        // get a useful error, not a dropped socket.
                        if out.send(ServerFrame::Error {
                            id: RequestId(0),
                            body: ErrorPayload::new(
                                ErrorCode::Ipc,
                                format!("malformed request: {e}"),
                            )
                            .with_hint("Send `{\"cmd\":\"schema\"}` for the accepted requests."),
                        }).await.is_err() {
                            break None;
                        }
                        continue;
                    }
                };

                match frame {
                    ClientFrame::Cancel { id } => {
                        // Cancelling something that is not a live subscription
                        // is the normal outcome of a race, not an error.
                        if let Some(task) = subscriptions.remove(&id) {
                            task.abort();
                            if out.send(ServerFrame::End { id }).await.is_err() {
                                break None;
                            }
                        }
                    }

                    ClientFrame::Request { id, protocol, body } => {
                        if !limiter.allow() {
                            violations += 1;
                            let _ = out.send(ServerFrame::Error {
                                id,
                                body: ErrorPayload::new(
                                    ErrorCode::Ipc,
                                    format!(
                                        "rate limit exceeded: at most {} requests per second",
                                        limits.max_requests_per_second
                                    ),
                                )
                                .with_hint("Subscribe to a stream instead of polling."),
                            }).await;
                            if violations >= MAX_RATE_VIOLATIONS {
                                break Some("sustained rate-limit violations");
                            }
                            continue;
                        }
                        violations = 0;

                        if let Err(e) = check_protocol(protocol) {
                            if out.send(ServerFrame::error(id, &e)).await.is_err() {
                                break None;
                            }
                            continue;
                        }

                        let ctx = RequestContext {
                            connection: connection_id,
                            request_id: id,
                            protocol,
                            peer,
                        };

                        match body {
                            // Answered by the transport: generated in-process,
                            // so it cannot be stale and the daemon need not
                            // know about it.
                            Request::Schema {} => {
                                let reply = Reply::Schema(SchemaReply {
                                    schema: Box::new(schema_with_limits(&limits)),
                                });
                                if out.send(ServerFrame::Ok { id, body: Box::new(reply) })
                                    .await
                                    .is_err()
                                {
                                    break None;
                                }
                            }

                            // Also transport-level: a stream, not a response.
                            Request::Subscribe { topics } => {
                                if let Some(old) = subscriptions.remove(&id) {
                                    // Re-subscribing on a live id replaces the
                                    // stream rather than running two.
                                    old.abort();
                                }
                                let topics =
                                    if topics.is_empty() { Topic::all() } else { topics };
                                match handler.event_stream(&ctx, &topics) {
                                    Ok(events) => {
                                        let reply = Reply::Subscribed(SubscribedReply {
                                            subscription: id,
                                            topics: topics.clone(),
                                        });
                                        if out
                                            .send(ServerFrame::Ok { id, body: Box::new(reply) })
                                            .await
                                            .is_err()
                                        {
                                            break None;
                                        }
                                        subscriptions.insert(
                                            id,
                                            tokio::spawn(pump(id, events, topics, out.clone())),
                                        );
                                    }
                                    Err(e) => {
                                        if out.send(ServerFrame::error(id, &e)).await.is_err() {
                                            break None;
                                        }
                                    }
                                }
                            }

                            request => {
                                // Bound the fan-out: a client cannot make the
                                // daemon spawn without limit.
                                while inflight.len() >= limits.max_inflight {
                                    if inflight.join_next().await.is_none() {
                                        break;
                                    }
                                }
                                let handler = Arc::clone(&handler);
                                let out = out.clone();
                                let timeout = limits.handler_timeout;
                                inflight.spawn(async move {
                                    let frame = run_request(&handler, ctx, request, timeout).await;
                                    let _ = out.send(frame).await;
                                });
                            }
                        }
                    }
                }
            }

            Some(_) = inflight.join_next() => {}
        }
    };

    // Wind down: end every stream, let in-flight requests answer, say goodbye.
    for (id, task) in subscriptions.drain() {
        task.abort();
        let _ = out.send(ServerFrame::End { id }).await;
    }
    while inflight.join_next().await.is_some() {}
    if let Some(reason) = stop_reason {
        let _ = out.send(ServerFrame::Bye { reason: reason.to_string() }).await;
    }

    // Zero the last line read: it may have been a `vault.unlock`.
    {
        use zeroize::Zeroize;
        line.zeroize();
    }
    Ok(())
}

/// Run one request, converting every possible outcome into a frame.
///
/// The inner `spawn` is what gives panic isolation: a handler that panics
/// produces a `JoinError` here and an `internal` error frame there, instead of
/// unwinding through the connection loop and killing the connection.
async fn run_request<H: Handler>(
    handler: &Arc<H>,
    ctx: RequestContext,
    request: Request,
    timeout: Duration,
) -> ServerFrame {
    let id = ctx.request_id;
    let command = request.command();
    let handler = Arc::clone(handler);
    let task = tokio::spawn(async move { dispatch(&*handler, &ctx, request).await });

    match tokio::time::timeout(timeout, task).await {
        Ok(Ok(Ok(reply))) => ServerFrame::Ok { id, body: Box::new(reply) },
        Ok(Ok(Err(e))) => ServerFrame::error(id, &e),
        Ok(Err(join)) => {
            tracing::error!(command, error = %join, "IPC handler panicked");
            ServerFrame::Error {
                id,
                body: ErrorPayload::new(
                    ErrorCode::Internal,
                    format!("the handler for `{command}` panicked"),
                )
                .with_hint("This is a bug in superbackup; the log holds the backtrace."),
            }
        }
        Err(_) => {
            tracing::error!(command, ?timeout, "IPC handler timed out");
            ServerFrame::Error {
                id,
                body: ErrorPayload::new(
                    ErrorCode::Ipc,
                    format!("`{command}` did not finish within {}s", timeout.as_secs()),
                ),
            }
        }
    }
}

/// Forward one subscription's items until it is cancelled or the daemon stops.
///
/// The only place [`StreamItem::Lagged`] is produced. Note what it does *not*
/// do: it never buffers on the client's behalf and never slows the producer.
/// The broadcast channel drops the oldest items and reports how many, which is
/// exactly the policy a live view wants — a tray icon showing the state from
/// two minutes ago is worse than one that admits it fell behind and
/// resynchronises.
async fn pump(
    id: RequestId,
    mut events: broadcast::Receiver<StreamItem>,
    topics: Vec<Topic>,
    out: Outbound,
) {
    loop {
        match events.recv().await {
            Ok(item) => {
                if !item.matches(&topics) {
                    continue;
                }
                if out.send(ServerFrame::Stream { id, body: Box::new(item) }).await.is_err() {
                    return;
                }
            }
            Err(broadcast::error::RecvError::Lagged(missed)) => {
                let marker = StreamItem::Lagged { missed };
                if out.send(ServerFrame::Stream { id, body: Box::new(marker) }).await.is_err() {
                    return;
                }
            }
            Err(broadcast::error::RecvError::Closed) => {
                let _ = out.send(ServerFrame::End { id }).await;
                return;
            }
        }
    }
}
