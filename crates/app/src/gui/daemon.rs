//! The link to the daemon.
//!
//! The window is a client like any other: it opens one `ipc::Client`, keeps a
//! subscription open for events, progress and status, and issues requests for
//! everything else. A background thread owns the runtime; the UI thread never
//! blocks on I/O and never awaits.
//!
//! The transport is behind a trait with two implementations:
//!
//! * [`IpcDaemon`] — a real connection to a running instance.
//! * [`MockDaemon`] — `ipc::testing::MockHandler` dispatched in-process, which
//!   is how every screen is developed and smoke-tested without a daemon.
//!
//! # Repaints
//!
//! egui is immediate mode and repaints on demand (L14). This module is the only
//! thing that wakes the UI when nothing local changed: the worker calls
//! `Context::request_repaint()` after it posts a message, and at no other time.
//! An idle window with no running job therefore reaches zero frames per second.

// The interface is a library-shaped tree inside a binary crate. Its components,
// view models and fixtures are also compiled by `crates/app/tests/gui_app.rs`
// as a separate crate, so items that are used and tested there look unused from
// the binary's side. The allow is scoped to this module rather than the crate.
#![allow(dead_code)]
use std::future::Future;
use std::pin::Pin;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::time::Duration;

use superbackup_core::error::{Error, ErrorCode};
use superbackup_core::ipc::protocol::{
    ErrorPayload, Reply, Request, RequestContext, StreamItem, Topic,
};

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// What a reply is for. The app routes on this rather than on the reply's
/// shape, because several commands answer with the same payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    Status,
    Version,
    Service,
    Jobs,
    Destinations,
    Providers,
    Settings,
    History,
    Doctor,
    Unlock,
    Lock,
    Pause,
    /// A destination probe, keyed by the destination it belongs to.
    TestDestination(uuid::Uuid),
    TestProvider(uuid::Uuid),
    CreateRepository(uuid::Uuid),
    Snapshots(uuid::Uuid),
    Browse(uuid::Uuid, String),
    Restore,
    /// A job run was requested; the name is carried so the toast can say which.
    RunJob(String),
    StopRun,
    SaveJob(String),
    DeleteJob(String),
    SaveDestination(String),
    DeleteDestination(String),
    SaveProvider(String),
    DeleteProvider(String),
    SecretRefs,
    /// Anything whose reply the interface does not need to route.
    Fire,
}

/// One message from the worker to the UI thread.
#[derive(Debug)]
pub enum Incoming {
    Reply(Intent, Reply),
    Failed(Intent, ErrorPayload),
    Stream(Box<StreamItem>),
    /// The link came up or went down. Drives the `DaemonUnreachable` banner.
    Link { up: bool, detail: Option<String> },
}

struct Call {
    intent: Intent,
    request: Request,
}

/// The transport the worker drives.
pub trait Daemon: Send + Sync + 'static {
    fn call(self: Arc<Self>, request: Request) -> BoxFuture<superbackup_core::Result<Reply>>;

    /// Forward stream items into `sink` until the subscription ends.
    fn stream(
        self: Arc<Self>,
        topics: Vec<Topic>,
        sink: tokio::sync::mpsc::UnboundedSender<StreamItem>,
    ) -> BoxFuture<superbackup_core::Result<()>>;

    /// The kopia version and build identity, when the transport knows it
    /// without asking. Used only for the status strip's first paint.
    fn describe(&self) -> Option<String> {
        None
    }
}

// ---------------------------------------------------------------------------
// The bridge
// ---------------------------------------------------------------------------

/// The UI thread's handle on the worker.
pub struct Bridge {
    to_worker: Sender<Call>,
    from_worker: Receiver<Incoming>,
    /// Set once the first request succeeds or fails, so the interface can tell
    /// "not asked yet" from "asked and the daemon is not there".
    pub link_up: bool,
}

impl Bridge {
    /// Start a worker for `daemon`, subscribed to every topic.
    pub fn spawn(daemon: Arc<dyn Daemon>, ctx: egui::Context) -> Bridge {
        let (to_worker, calls) = std::sync::mpsc::channel::<Call>();
        let (results, from_worker) = std::sync::mpsc::channel::<Incoming>();

        std::thread::Builder::new()
            .name("superbackup-gui-ipc".into())
            .spawn(move || worker(daemon, calls, results, ctx))
            .ok();

        Bridge { to_worker, from_worker, link_up: false }
    }

    /// Ask the daemon for something. Dropping the worker is not an error the
    /// interface reports twice — the link banner already says so.
    pub fn send(&self, intent: Intent, request: Request) {
        let _ = self.to_worker.send(Call { intent, request });
    }

    /// Everything that arrived since the last frame.
    pub fn drain(&mut self) -> Vec<Incoming> {
        let mut out = Vec::new();
        loop {
            match self.from_worker.try_recv() {
                Ok(msg) => {
                    if let Incoming::Link { up, .. } = &msg {
                        self.link_up = *up;
                    }
                    out.push(msg);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.link_up = false;
                    break;
                }
            }
        }
        out
    }
}

fn worker(
    daemon: Arc<dyn Daemon>,
    calls: Receiver<Call>,
    results: Sender<Incoming>,
    ctx: egui::Context,
) {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            let _ = results.send(Incoming::Link { up: false, detail: Some(e.to_string()) });
            ctx.request_repaint();
            return;
        }
    };

    // The subscription: one task forwarding into an unbounded channel, drained
    // below alongside the request results.
    let (stream_tx, mut stream_rx) = tokio::sync::mpsc::unbounded_channel::<StreamItem>();
    {
        let daemon = daemon.clone();
        let results = results.clone();
        let ctx = ctx.clone();
        runtime.spawn(async move {
            if let Err(e) = daemon.stream(Topic::all(), stream_tx).await {
                let _ = results.send(Incoming::Link { up: false, detail: Some(e.to_string()) });
                ctx.request_repaint();
            }
        });
    }

    let (reply_tx, mut reply_rx) =
        tokio::sync::mpsc::unbounded_channel::<(Intent, superbackup_core::Result<Reply>)>();

    loop {
        // Block briefly on the UI thread's queue so this thread is asleep
        // whenever nothing is happening.
        match calls.recv_timeout(Duration::from_millis(25)) {
            Ok(call) => {
                let daemon = daemon.clone();
                let reply_tx = reply_tx.clone();
                runtime.spawn(async move {
                    let outcome = daemon.call(call.request).await;
                    let _ = reply_tx.send((call.intent, outcome));
                });
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }

        let mut woke = false;
        while let Ok((intent, outcome)) = reply_rx.try_recv() {
            woke = true;
            match outcome {
                Ok(reply) => {
                    if !matches!(intent, Intent::Fire) {
                        let _ = results.send(Incoming::Link { up: true, detail: None });
                    }
                    let _ = results.send(Incoming::Reply(intent, reply));
                }
                Err(e) => {
                    let payload = ErrorPayload::from_error(&e);
                    if matches!(
                        payload.code,
                        ErrorCode::DaemonUnreachable | ErrorCode::Ipc
                    ) {
                        let _ = results
                            .send(Incoming::Link { up: false, detail: Some(payload.message.clone()) });
                    }
                    let _ = results.send(Incoming::Failed(intent, payload));
                }
            }
        }
        while let Ok(item) = stream_rx.try_recv() {
            woke = true;
            let _ = results.send(Incoming::Stream(Box::new(item)));
        }
        if woke {
            // The only place the interface is woken from outside itself.
            ctx.request_repaint();
        }
    }
}

// ---------------------------------------------------------------------------
// The real transport
// ---------------------------------------------------------------------------

/// A connection to a running instance over the platform's local socket.
pub struct IpcDaemon {
    endpoint: String,
    timeout: Duration,
    client: tokio::sync::Mutex<Option<superbackup_core::ipc::Client>>,
}

impl IpcDaemon {
    pub fn new(endpoint: impl Into<String>, timeout: Duration) -> IpcDaemon {
        IpcDaemon {
            endpoint: endpoint.into(),
            timeout,
            client: tokio::sync::Mutex::new(None),
        }
    }

    async fn connected(&self) -> superbackup_core::Result<superbackup_core::ipc::Client> {
        let mut guard = self.client.lock().await;
        if let Some(client) = guard.as_ref() {
            return Ok(client.clone());
        }
        let client =
            superbackup_core::ipc::Client::connect_with(&self.endpoint, self.timeout).await?;
        *guard = Some(client.clone());
        Ok(client)
    }

    async fn drop_connection(&self) {
        *self.client.lock().await = None;
    }
}

impl Daemon for IpcDaemon {
    fn call(self: Arc<Self>, request: Request) -> BoxFuture<superbackup_core::Result<Reply>> {
        Box::pin(async move {
            let client = self.connected().await?;
            match client.request(request).await {
                Ok(reply) => Ok(reply),
                Err(e) => {
                    // A dead pipe must not be reused; the next call reconnects.
                    if matches!(e.code(), ErrorCode::Ipc | ErrorCode::DaemonUnreachable) {
                        self.drop_connection().await;
                    }
                    Err(e)
                }
            }
        })
    }

    fn stream(
        self: Arc<Self>,
        topics: Vec<Topic>,
        sink: tokio::sync::mpsc::UnboundedSender<StreamItem>,
    ) -> BoxFuture<superbackup_core::Result<()>> {
        Box::pin(async move {
            loop {
                let client = match self.connected().await {
                    Ok(c) => c,
                    Err(e) => {
                        // Retry rather than giving up: a daemon that is being
                        // restarted should reconnect on its own.
                        tokio::time::sleep(Duration::from_secs(2)).await;
                        if sink.is_closed() {
                            return Err(e);
                        }
                        continue;
                    }
                };
                match client.subscribe(topics.clone()).await {
                    Ok(mut subscription) => {
                        while let Some(item) = subscription.next().await {
                            if sink.send(item).is_err() {
                                return Ok(());
                            }
                        }
                    }
                    Err(_) => {
                        tokio::time::sleep(Duration::from_secs(2)).await;
                    }
                }
                self.drop_connection().await;
                if sink.is_closed() {
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        })
    }
}

// ---------------------------------------------------------------------------
// The mock transport
// ---------------------------------------------------------------------------

/// `ipc::testing::MockHandler`, dispatched in-process through the same
/// `protocol::dispatch` the daemon uses.
///
/// This is not a stub of the interface's own making: every request travels the
/// real command table and comes back as a real `Reply`, so a screen developed
/// against it behaves the same against a daemon.
pub struct MockDaemon {
    handler: Arc<superbackup_core::ipc::testing::MockHandler>,
}

impl MockDaemon {
    pub fn new(handler: Arc<superbackup_core::ipc::testing::MockHandler>) -> MockDaemon {
        MockDaemon { handler }
    }

    pub fn handler(&self) -> &Arc<superbackup_core::ipc::testing::MockHandler> {
        &self.handler
    }
}

impl Daemon for MockDaemon {
    fn call(self: Arc<Self>, request: Request) -> BoxFuture<superbackup_core::Result<Reply>> {
        Box::pin(async move {
            let ctx = RequestContext::local();
            match request {
                // The transport answers these two itself, exactly as the
                // server does; reaching the dispatcher with them is a bug.
                Request::Schema {} => Ok(Reply::Schema(
                    superbackup_core::ipc::protocol::SchemaReply {
                        schema: Box::new(superbackup_core::ipc::protocol::schema()),
                    },
                )),
                Request::Subscribe { .. } => Err(Error::Ipc(
                    "subscribe is answered by the transport, not by a request".into(),
                )),
                other => {
                    superbackup_core::ipc::protocol::dispatch(&*self.handler, &ctx, other).await
                }
            }
        })
    }

    fn stream(
        self: Arc<Self>,
        topics: Vec<Topic>,
        sink: tokio::sync::mpsc::UnboundedSender<StreamItem>,
    ) -> BoxFuture<superbackup_core::Result<()>> {
        Box::pin(async move {
            let ctx = RequestContext::local();
            let mut rx = {
                use superbackup_core::ipc::protocol::Handler;
                self.handler.event_stream(&ctx, &topics)?
            };
            loop {
                match rx.recv().await {
                    Ok(item) => {
                        if sink.send(item).is_err() {
                            return Ok(());
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(missed)) => {
                        // Backpressure is a fact to report, not an error to
                        // hide: the interface resynchronises from `status`.
                        if sink.send(StreamItem::Lagged { missed }).is_err() {
                            return Ok(());
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return Ok(()),
                }
            }
        })
    }
}

/// The set of requests the window issues on open, in the order it wants them.
pub fn initial_requests() -> Vec<(Intent, Request)> {
    vec![
        (Intent::Version, Request::Version {}),
        (Intent::Status, Request::Status {}),
        (Intent::Settings, Request::SettingsGet {}),
        (Intent::Jobs, Request::JobList { include_disabled: true }),
        (Intent::Destinations, Request::DestinationList {}),
        (Intent::Providers, Request::ProviderList {}),
        (Intent::Service, Request::ServiceStatus {}),
        (Intent::History, Request::JobHistory { job: None, limit: 200 }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use superbackup_core::ipc::testing::MockHandler;

    fn mock() -> Arc<MockDaemon> {
        Arc::new(MockDaemon::new(Arc::new(MockHandler::new())))
    }

    #[test]
    fn the_mock_answers_every_command_the_window_opens_with() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a current-thread runtime");
        let daemon = mock();
        for (intent, request) in initial_requests() {
            let reply = runtime.block_on(daemon.clone().call(request));
            assert!(reply.is_ok(), "{intent:?} was refused: {:?}", reply.err());
        }
    }

    #[test]
    fn a_failing_daemon_produces_an_error_payload_rather_than_a_panic() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a current-thread runtime");
        let handler = Arc::new(MockHandler::new());
        // The mock maps most armed codes onto `Internal`; what matters here is
        // that a refusal comes back as an error the interface can render,
        // rather than as a panic or a malformed reply.
        handler.fail_with(Some(ErrorCode::DaemonUnreachable));
        let daemon = Arc::new(MockDaemon::new(handler.clone()));
        let outcome = runtime.block_on(daemon.clone().call(Request::Status {}));
        assert!(outcome.is_err(), "the handler was armed to fail");

        handler.fail_with(Some(ErrorCode::Locked));
        let outcome = runtime.block_on(daemon.call(Request::JobRun {
            job: "anything".into(),
            dry_run: false,
        }));
        assert_eq!(
            outcome.expect_err("a locked vault refuses").code(),
            ErrorCode::Locked
        );
    }

    #[test]
    fn the_bridge_delivers_replies_to_the_ui_thread() {
        let ctx = egui::Context::default();
        let mut bridge = Bridge::spawn(mock(), ctx);
        bridge.send(Intent::Status, Request::Status {});

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut got = false;
        while std::time::Instant::now() < deadline && !got {
            for msg in bridge.drain() {
                if let Incoming::Reply(Intent::Status, Reply::Status(_)) = msg {
                    got = true;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(got, "the status reply never reached the UI thread");
        assert!(bridge.link_up);
    }

    #[test]
    fn published_stream_items_reach_the_ui_thread() {
        let handler = Arc::new(MockHandler::new());
        let daemon = Arc::new(MockDaemon::new(handler.clone()));
        let mut bridge = Bridge::spawn(daemon, egui::Context::default());

        // Give the subscription a moment to attach before publishing.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while handler.subscriber_count() == 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        handler.publish(StreamItem::Event {
            event: Box::new(superbackup_core::state::Event::info("job.started", "off we go")),
        });

        let mut got = false;
        while std::time::Instant::now() < deadline && !got {
            for msg in bridge.drain() {
                if let Incoming::Stream(item) = msg {
                    if let StreamItem::Event { event } = *item {
                        assert_eq!(event.kind, "job.started");
                        got = true;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(got, "the published event never arrived");
    }
}
