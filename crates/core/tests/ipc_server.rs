//! Transport tests: the server, the client, and the ways a peer can misbehave.
//!
//! Every test binds a real endpoint — a named pipe on Windows, a unix socket
//! in a temporary directory elsewhere — and speaks the real protocol over it.
//! Nothing is mocked below the [`Handler`], because the parts most worth
//! testing are exactly the ones a mock socket would hide: framing, the size
//! cap, peer checks, and shutdown.

use std::sync::Arc;
use std::time::Duration;

use superbackup_core::error::{Error, ErrorCode};
use superbackup_core::ipc::protocol::{AckReply, Reply, Request, RequestId, SecretString};
use superbackup_core::ipc::testing::MockHandler;
use superbackup_core::ipc::{
    Client, ClientFrame, Limits, Server, ServerHandle, ServerOptions, StreamItem, Topic,
};
use superbackup_core::state::Event;

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// A unique endpoint for one test, so tests can run in parallel.
fn endpoint(tag: &str) -> String {
    let unique = format!("{}-{}-{}", std::process::id(), tag, uuid::Uuid::new_v4().simple());
    if cfg!(windows) {
        format!(r"\\.\pipe\superbackup-test-{unique}")
    } else {
        let dir = std::env::temp_dir().join(format!("sb-ipc-{unique}"));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir.join("sb.sock").display().to_string()
    }
}

struct Harness {
    endpoint: String,
    handler: Arc<MockHandler>,
    handle: ServerHandle,
    server: tokio::task::JoinHandle<superbackup_core::error::Result<()>>,
}

impl Harness {
    fn start(tag: &str) -> Harness {
        Harness::start_with(tag, Limits::default(), Arc::new(MockHandler::new()))
    }

    fn start_with(tag: &str, limits: Limits, handler: Arc<MockHandler>) -> Harness {
        let endpoint = endpoint(tag);
        let options = ServerOptions { limits, replace_existing: true };
        let server = Server::bind(&endpoint, Arc::clone(&handler), options)
            .unwrap_or_else(|e| panic!("binding {endpoint}: {e}"));
        let handle = server.handle();
        let task = tokio::spawn(server.serve());
        Harness { endpoint, handler, handle, server: task }
    }

    async fn client(&self) -> Client {
        Client::connect(&self.endpoint)
            .await
            .unwrap_or_else(|e| panic!("connecting to {}: {e}", self.endpoint))
    }

    /// Shut down and wait for the accept loop to finish.
    async fn stop(self) {
        self.handle.shutdown();
        let outcome = tokio::time::timeout(Duration::from_secs(10), self.server)
            .await
            .expect("the server must stop within ten seconds")
            .expect("the server task must not panic");
        outcome.expect("serve() must return Ok on a clean shutdown");
    }
}

// ---------------------------------------------------------------------------
// Happy path
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_request_reaches_the_handler_and_the_reply_comes_back() {
    let h = Harness::start("basic");
    let client = h.client().await;

    // The greeting arrives before anything is sent.
    assert_eq!(client.hello().protocol, superbackup_core::ipc::PROTOCOL_VERSION);

    client.ping().await.expect("ping");
    assert_eq!(h.handler.calls("ping"), 1, "the request must reach the handler");

    let snapshot = client.status().await.expect("status");
    assert_eq!(snapshot.machine_slug, "mock");

    h.stop().await;
}

#[tokio::test]
async fn the_schema_is_answered_by_the_transport_without_the_handler() {
    let h = Harness::start("schema");
    let client = h.client().await;

    let schema = client.schema().await.expect("schema");
    assert!(!schema.commands.is_empty());
    assert!(schema.commands.iter().any(|c| c.name == "job.run"));
    // Discovery must not depend on the daemon implementing anything.
    assert_eq!(h.handler.calls("schema"), 0);

    // The limits it publishes must be the ones actually in force.
    assert_eq!(schema.limits.max_line_bytes, Limits::default().max_line_bytes);

    h.stop().await;
}

#[tokio::test]
async fn a_handler_error_becomes_a_typed_error_response() {
    let h = Harness::start("errors");
    let client = h.client().await;

    h.handler.fail_with(Some(ErrorCode::Locked));
    let error = client.status().await.expect_err("the handler was told to fail");
    assert_eq!(error.code(), ErrorCode::Locked, "the code must survive the round trip");

    h.handler.fail_with(None);
    client.status().await.expect("and recover afterwards");

    h.stop().await;
}

// ---------------------------------------------------------------------------
// Concurrency
// ---------------------------------------------------------------------------

#[tokio::test]
async fn many_clients_are_served_at_once() {
    let h = Harness::start("concurrent");

    let mut tasks = Vec::new();
    for _ in 0..8 {
        let endpoint = h.endpoint.clone();
        tasks.push(tokio::spawn(async move {
            let client = Client::connect(&endpoint).await.expect("connect");
            for _ in 0..5 {
                client.ping().await.expect("ping");
            }
        }));
    }
    for task in tasks {
        task.await.expect("client task");
    }

    assert_eq!(h.handler.calls("ping"), 40, "every request from every client must arrive");
    h.stop().await;
}

#[tokio::test]
async fn requests_on_one_connection_are_pipelined() {
    let h = Harness::start("pipelined");
    let client = h.client().await;

    // Each handler call stalls; if the server serialised them, twelve calls at
    // 100ms each would take 1.2s. Concurrency makes it about 100ms.
    h.handler.stall(Some(Duration::from_millis(100)));
    let started = std::time::Instant::now();
    let mut tasks = Vec::new();
    for _ in 0..12 {
        let client = client.clone();
        tasks.push(tokio::spawn(async move { client.ping().await }));
    }
    for task in tasks {
        task.await.expect("join").expect("ping");
    }
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_millis(900),
        "requests were serialised: twelve 100ms calls took {elapsed:?}"
    );

    h.handler.stall(None);
    h.stop().await;
}

// ---------------------------------------------------------------------------
// Streaming
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_subscription_streams_events_and_stops_when_dropped() {
    let h = Harness::start("subscribe");
    let client = h.client().await;

    let mut subscription = client.subscribe(vec![Topic::Events]).await.expect("subscribe");
    assert_eq!(subscription.topics().to_vec(), vec![Topic::Events]);

    // Publish until the subscriber has actually attached; `send` on a
    // broadcast channel with no receivers reaches nobody, and attachment
    // happens on the server's task, not ours.
    let publisher = Arc::clone(&h.handler);
    let pump = tokio::spawn(async move {
        for i in 0..200u32 {
            publisher.publish(StreamItem::Event {
                event: Box::new(Event::info("test.event", format!("event {i}"))),
            });
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    });

    let item = tokio::time::timeout(Duration::from_secs(5), subscription.next())
        .await
        .expect("an item must arrive within five seconds")
        .expect("the stream must not end");
    match item {
        StreamItem::Event { event } => assert_eq!(event.kind, "test.event"),
        other => panic!("expected an event, got {other:?}"),
    }

    // Dropping the subscription cancels it at the daemon.
    drop(subscription);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        h.handler.subscriber_count(),
        0,
        "dropping a Subscription must cancel it at the daemon"
    );

    pump.abort();
    h.stop().await;
}

#[tokio::test]
async fn topic_filtering_excludes_unsubscribed_items() {
    let h = Harness::start("topics");
    let client = h.client().await;
    let mut subscription = client.subscribe(vec![Topic::Status]).await.expect("subscribe");

    let publisher = Arc::clone(&h.handler);
    let pump = tokio::spawn(async move {
        for _ in 0..100 {
            // Only events; a status-only subscriber must see none of them.
            publisher.publish(StreamItem::Event {
                event: Box::new(Event::info("noise", "should not be delivered")),
            });
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    });

    let got = tokio::time::timeout(Duration::from_millis(400), subscription.next()).await;
    assert!(got.is_err(), "an unsubscribed topic must not be delivered: {got:?}");

    pump.abort();
    h.stop().await;
}

#[tokio::test]
async fn a_slow_subscriber_is_told_it_lagged_instead_of_stalling_the_daemon() {
    // A deliberately tiny fan-out buffer, so a consumer that does not read
    // falls behind immediately.
    let handler = Arc::new(MockHandler::with_capacity(4));
    let h = Harness::start_with("lagging", Limits::default(), Arc::clone(&handler));
    let client = h.client().await;
    let mut subscription = client.subscribe(vec![Topic::Events]).await.expect("subscribe");

    // Do not read. Flood.
    let publisher = Arc::clone(&handler);
    let started = std::time::Instant::now();
    for i in 0..5_000u32 {
        publisher.publish(StreamItem::Event {
            event: Box::new(Event::info("flood", format!("{i}"))),
        });
    }
    let publish_time = started.elapsed();
    assert!(
        publish_time < Duration::from_secs(5),
        "publishing must never block on a slow subscriber; it took {publish_time:?}"
    );

    // Now drain, and find the marker that says how much was lost.
    let mut saw_lagged = false;
    for _ in 0..1024 {
        match tokio::time::timeout(Duration::from_secs(2), subscription.next()).await {
            Ok(Some(StreamItem::Lagged { missed })) => {
                assert!(missed > 0, "a lag marker must say how many items were lost");
                saw_lagged = true;
                break;
            }
            Ok(Some(_)) => continue,
            Ok(None) => break,
            Err(_) => break,
        }
    }
    assert!(saw_lagged, "a subscriber that fell behind must be told, not silently starved");

    h.stop().await;
}

#[tokio::test]
async fn a_refused_subscription_is_an_error_not_a_dead_stream() {
    let h = Harness::start("refused");
    let client = h.client().await;
    h.handler.refuse_subscriptions(true);

    let error = client.subscribe(vec![]).await.expect_err("the handler refused");
    assert_eq!(error.code(), ErrorCode::Ipc);

    // The connection must still work afterwards.
    client.ping().await.expect("the connection survives a refused subscription");
    h.stop().await;
}

// ---------------------------------------------------------------------------
// Hostile input
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_oversized_line_is_refused_without_buffering_it() {
    let limits = Limits { max_line_bytes: 4096, ..Limits::default() };
    let h = Harness::start_with("oversize", limits, Arc::new(MockHandler::new()));

    let mut raw = RawClient::connect(&h.endpoint).await;
    raw.expect_hello().await;

    // Well over the cap. The daemon must reject it on length alone, before
    // the JSON parser is ever handed the bytes.
    let huge = format!("{{\"pad\":\"{}\"}}", "x".repeat(32 * 1024));
    // The write may not complete: the daemon stops reading the moment the cap
    // is exceeded and then closes. Both outcomes are correct; what matters is
    // that it told us why first.
    let _ = raw.write_raw(&huge).await;

    let frame = raw.read_frame().await.expect("the daemon must answer, not just hang up");
    match frame {
        superbackup_core::ipc::ServerFrame::Error { body, .. } => {
            assert_eq!(body.code, ErrorCode::Ipc);
            assert!(
                body.message.contains("limit"),
                "the refusal must name the limit: {}",
                body.message
            );
        }
        other => panic!("expected an error frame, got {other:?}"),
    }

    // And the daemon is still serving everyone else.
    let client = h.client().await;
    client.ping().await.expect("the daemon survives an oversized line");
    h.stop().await;
}

#[tokio::test]
async fn malformed_input_is_answered_and_the_connection_survives() {
    let h = Harness::start("malformed");
    let mut raw = RawClient::connect(&h.endpoint).await;
    raw.expect_hello().await;

    for garbage in [
        "not json at all",
        "{}",
        r#"{"type":"request"}"#,
        r#"{"type":"request","id":1,"body":{"cmd":"no.such.command"}}"#,
        r#"{"type":"request","id":2,"body":{"cmd":"job.run"}}"#,
        r#"{"type":"nonsense","id":3}"#,
        "[]",
        "null",
    ] {
        assert!(raw.write_raw(garbage).await, "writing `{garbage}`");
        let frame = raw
            .read_frame()
            .await
            .unwrap_or_else(|| panic!("the daemon dropped the connection on `{garbage}`"));
        assert!(
            matches!(frame, superbackup_core::ipc::ServerFrame::Error { .. }),
            "`{garbage}` should produce an error frame, got {frame:?}"
        );
    }

    // Still usable for a well-formed request.
    raw.write_frame(&ClientFrame::Request {
        id: RequestId(99),
        protocol: superbackup_core::ipc::PROTOCOL_VERSION,
        body: Request::Ping {},
    })
    .await;
    match raw.read_frame().await.expect("a reply") {
        superbackup_core::ipc::ServerFrame::Ok { id, body } => {
            assert_eq!(id, RequestId(99));
            assert!(matches!(*body, Reply::Ack(AckReply {})));
        }
        other => panic!("expected ok, got {other:?}"),
    }

    h.stop().await;
}

#[tokio::test]
async fn a_protocol_mismatch_is_refused_per_request_with_a_clear_message() {
    let h = Harness::start("version");
    let mut raw = RawClient::connect(&h.endpoint).await;
    raw.expect_hello().await;

    assert!(
        raw.write_raw(&format!(
            r#"{{"type":"request","id":1,"protocol":{},"body":{{"cmd":"ping"}}}}"#,
            superbackup_core::ipc::PROTOCOL_VERSION + 1
        ))
        .await,
        "writing the mismatched request"
    );

    match raw.read_frame().await.expect("a reply") {
        superbackup_core::ipc::ServerFrame::Error { body, .. } => {
            assert_eq!(body.code, ErrorCode::Ipc);
            assert!(
                body.message.contains("upgrade the daemon"),
                "the message must say what to do: {}",
                body.message
            );
        }
        other => panic!("expected an error frame, got {other:?}"),
    }

    h.stop().await;
}

#[tokio::test]
async fn a_panicking_handler_costs_one_request_not_the_daemon() {
    let h = Harness::start("panic");
    let client = h.client().await;
    h.handler.panic_on(Some("status"));

    let error = client.status().await.expect_err("the handler panicked");
    assert_eq!(error.code(), ErrorCode::Internal);

    h.handler.panic_on(None);
    client.ping().await.expect("the connection must survive a panicking handler");
    h.stop().await;
}

#[tokio::test]
async fn a_hanging_handler_is_abandoned_rather_than_owning_the_connection() {
    let limits = Limits {
        handler_timeout: Duration::from_millis(150),
        ..Limits::default()
    };
    let h = Harness::start_with("hang", limits, Arc::new(MockHandler::new()));
    let client = h.client().await;

    h.handler.stall(Some(Duration::from_secs(30)));
    let error = tokio::time::timeout(Duration::from_secs(5), client.status())
        .await
        .expect("the transport must give up on its own")
        .expect_err("a hanging handler must produce an error");
    assert_eq!(error.code(), ErrorCode::Ipc);

    h.handler.stall(None);
    client.ping().await.expect("the connection survives");
    h.stop().await;
}

#[tokio::test]
async fn requests_are_rate_limited_per_connection() {
    let limits = Limits {
        max_requests_per_second: 1,
        request_burst: 5,
        ..Limits::default()
    };
    let h = Harness::start_with("ratelimit", limits, Arc::new(MockHandler::new()));
    let client = h.client().await;

    let mut refusals = 0u32;
    for _ in 0..40 {
        if let Err(e) = client.ping().await {
            assert_eq!(e.code(), ErrorCode::Ipc);
            assert!(
                e.to_string().contains("rate limit"),
                "the refusal must say why: {e}"
            );
            refusals += 1;
        }
    }
    assert!(refusals > 0, "a client far above the limit must be refused at least once");
    assert!(
        h.handler.calls("ping") <= 40 - refusals,
        "refused requests must not reach the handler"
    );

    h.stop().await;
}

#[tokio::test]
async fn the_connection_limit_refuses_politely() {
    let limits = Limits { max_connections: 2, ..Limits::default() };
    let h = Harness::start_with("connlimit", limits, Arc::new(MockHandler::new()));

    let a = h.client().await;
    let b = h.client().await;
    a.ping().await.expect("first");
    b.ping().await.expect("second");

    // The third is accepted at the socket level and then told why it cannot
    // be served, which is more useful than a silent hang.
    let mut raw = RawClient::connect(&h.endpoint).await;
    raw.expect_hello().await;
    match raw.read_frame().await.expect("a refusal") {
        superbackup_core::ipc::ServerFrame::Error { body, .. } => {
            assert!(
                body.message.contains("connections"),
                "the refusal must explain itself: {}",
                body.message
            );
        }
        other => panic!("expected an error frame, got {other:?}"),
    }

    drop(raw);
    h.stop().await;
}

// ---------------------------------------------------------------------------
// Shutdown
// ---------------------------------------------------------------------------

#[tokio::test]
async fn shutdown_with_clients_connected_says_goodbye() {
    let h = Harness::start("shutdown");
    let client = h.client().await;
    let mut subscription = client.subscribe(vec![]).await.expect("subscribe");
    client.ping().await.expect("ping");

    let handle = h.handle.clone();
    let server = h.server;
    let endpoint = h.endpoint.clone();
    handle.shutdown();

    // The server stops within a reasonable time even though a client is still
    // connected and streaming.
    let outcome = tokio::time::timeout(Duration::from_secs(10), server)
        .await
        .expect("the server must stop even with clients connected")
        .expect("the server task must not panic");
    outcome.expect("serve() must return Ok");

    // The subscription ends rather than hanging.
    let ended = tokio::time::timeout(Duration::from_secs(5), subscription.next())
        .await
        .expect("the stream must terminate");
    assert!(ended.is_none(), "the stream must end, not deliver more items");

    // And a request now fails with a clear error instead of blocking.
    let error = client.ping().await.expect_err("the daemon is gone");
    assert_eq!(error.code(), ErrorCode::DaemonUnreachable);

    // The endpoint is released, so a replacement daemon can take it.
    let reconnect = Client::connect(&endpoint).await;
    assert!(
        matches!(reconnect, Err(Error::DaemonUnreachable)),
        "nothing should be listening after shutdown, got {reconnect:?}"
    );
}

#[tokio::test]
async fn shutdown_is_idempotent() {
    let h = Harness::start("double-shutdown");
    h.handle.shutdown();
    h.handle.shutdown();
    assert!(h.handle.is_shutting_down());
    h.stop().await;
}

// ---------------------------------------------------------------------------
// Client behaviour with no daemon
// ---------------------------------------------------------------------------

#[tokio::test]
async fn connecting_to_nothing_says_the_daemon_is_not_running() {
    let nowhere = endpoint("absent");
    let error = Client::connect(&nowhere).await.expect_err("nothing is listening");
    assert_eq!(
        error.code(),
        ErrorCode::DaemonUnreachable,
        "a missing daemon must not leak a raw OS error: {error}"
    );
    // The message and hint are what the CLI prints.
    assert!(error.to_string().contains("daemon is not running"), "{error}");
    assert!(
        error.hint().is_some_and(|h| h.contains("service status") || h.contains("tray")),
        "the hint must tell the user what to do"
    );
}

#[tokio::test]
async fn the_blocking_client_needs_no_runtime_from_its_caller() {
    let h = Harness::start("blocking");
    let endpoint = h.endpoint.clone();

    // Deliberately on a plain blocking thread with no runtime of its own,
    // which is exactly the situation the CLI is in.
    let joined = tokio::task::spawn_blocking(move || {
        let client = superbackup_core::ipc::BlockingClient::connect(&endpoint)
            .expect("the blocking client must connect without a caller-provided runtime");
        let snapshot = client.status().expect("status");
        let schema = client.schema().expect("schema");
        (snapshot.machine_slug, schema.commands.len())
    })
    .await
    .expect("blocking task");

    assert_eq!(joined.0, "mock");
    assert!(joined.1 > 0);
    h.stop().await;
}

#[tokio::test]
async fn the_blocking_client_reports_an_absent_daemon_too() {
    let nowhere = endpoint("absent-blocking");
    let error = tokio::task::spawn_blocking(move || {
        superbackup_core::ipc::BlockingClient::connect(&nowhere).err()
    })
    .await
    .expect("blocking task")
    .expect("nothing is listening");
    assert_eq!(error.code(), ErrorCode::DaemonUnreachable);
}

// ---------------------------------------------------------------------------
// Secrets end to end
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_passphrase_survives_the_wire_and_unlocks() {
    let h = Harness::start("unlock");
    let client = h.client().await;

    let locked = client.status().await.expect("status");
    assert!(!locked.unlocked);

    let reply = client
        .unlock(SecretString::from_string("correct horse battery staple".into()))
        .await
        .expect("unlock");
    assert!(reply.unlocked);

    let unlocked = client.status().await.expect("status");
    assert!(unlocked.unlocked, "the unlock must have taken effect in the handler");

    h.stop().await;
}

// ---------------------------------------------------------------------------
// A raw client, for the things the typed client will not do
// ---------------------------------------------------------------------------

/// Speaks the wire protocol without the safety rails, so tests can send the
/// malformed and oversized input a real client never would.
struct RawClient {
    reader: tokio::io::BufReader<interprocess::local_socket::tokio::RecvHalf>,
    writer: interprocess::local_socket::tokio::SendHalf,
    line: Vec<u8>,
}

impl RawClient {
    async fn connect(endpoint: &str) -> RawClient {
        use interprocess::local_socket::tokio::prelude::*;
        use interprocess::local_socket::GenericFilePath;

        let name = endpoint.to_fs_name::<GenericFilePath>().expect("endpoint name");
        let stream = interprocess::local_socket::tokio::Stream::connect(name)
            .await
            .unwrap_or_else(|e| panic!("connecting to {endpoint}: {e}"));
        let (recv, send) = stream.split();
        RawClient {
            reader: tokio::io::BufReader::new(recv),
            writer: send,
            line: Vec::new(),
        }
    }

    /// Write one line. Returns false when the daemon hung up mid-write, which
    /// for an oversized or hostile line is a legal outcome rather than a test
    /// failure — the assertion that matters is what it said before it did.
    async fn write_raw(&mut self, text: &str) -> bool {
        use tokio::io::AsyncWriteExt;
        let payload = format!("{text}\n");
        match tokio::time::timeout(
            Duration::from_secs(5),
            self.writer.write_all(payload.as_bytes()),
        )
        .await
        {
            Ok(Ok(())) => {
                let _ = self.writer.flush().await;
                true
            }
            _ => false,
        }
    }

    async fn write_frame(&mut self, frame: &ClientFrame) {
        let json = serde_json::to_string(frame).expect("serialise");
        assert!(self.write_raw(&json).await, "the daemon closed the connection mid-write");
    }

    async fn read_frame(&mut self) -> Option<superbackup_core::ipc::ServerFrame> {
        let read = tokio::time::timeout(
            Duration::from_secs(10),
            superbackup_core::ipc::codec::read_line(
                &mut self.reader,
                &mut self.line,
                8 * 1024 * 1024,
            ),
        )
        .await
        .expect("the daemon must answer within ten seconds");
        read.ok()?;
        serde_json::from_slice(&self.line).ok()
    }

    async fn expect_hello(&mut self) {
        match self.read_frame().await {
            Some(superbackup_core::ipc::ServerFrame::Hello { .. }) => {}
            other => panic!("expected a hello frame first, got {other:?}"),
        }
    }
}
