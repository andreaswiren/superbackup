//! Hostile review of the IPC transport's denial-of-service controls.
//!
//! Claim under test (THREAT_MODEL.md §A7, ipc/mod.rs, ipc/protocol.rs):
//! "caps line length so a client cannot exhaust memory, caps concurrent
//! connections, and rate-limits per connection", "a malicious client cannot
//! OOM or wedge the daemon".
//!
//! The line cap holds. The connection cap and the per-connection limits do
//! not bound what one client can make the daemon allocate.

use std::sync::Arc;
use std::time::Duration;

use superbackup_core::ipc::protocol::{Request, RequestId};
use superbackup_core::ipc::testing::MockHandler;
use superbackup_core::ipc::{ClientFrame, Limits, Server, ServerHandle, ServerOptions, Topic};

fn endpoint(tag: &str) -> String {
    let unique = format!("{}-{}-{}", std::process::id(), tag, uuid::Uuid::new_v4().simple());
    if cfg!(windows) {
        format!(r"\\.\pipe\superbackup-review-{unique}")
    } else {
        let dir = std::env::temp_dir().join(format!("sb-review-ipc-{unique}"));
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
    fn start(tag: &str, limits: Limits) -> Harness {
        let endpoint = endpoint(tag);
        let handler = Arc::new(MockHandler::new());
        let options = ServerOptions { limits, replace_existing: true };
        let server = Server::bind(&endpoint, Arc::clone(&handler), options)
            .unwrap_or_else(|e| panic!("binding {endpoint}: {e}"));
        let handle = server.handle();
        let server = tokio::spawn(server.serve());
        Harness { endpoint, handler, handle, server }
    }

    async fn stop(self) {
        self.handle.shutdown();
        let _ = tokio::time::timeout(Duration::from_secs(10), self.server).await;
    }
}

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
        RawClient { reader: tokio::io::BufReader::new(recv), writer: send, line: Vec::new() }
    }

    async fn write_frame(&mut self, frame: &ClientFrame) -> bool {
        use tokio::io::AsyncWriteExt;
        let mut payload = serde_json::to_vec(frame).expect("serialise");
        payload.push(b'\n');
        match tokio::time::timeout(Duration::from_secs(5), self.writer.write_all(&payload)).await {
            Ok(Ok(())) => {
                let _ = self.writer.flush().await;
                true
            }
            _ => false,
        }
    }

    async fn read_frame(&mut self) -> Option<superbackup_core::ipc::ServerFrame> {
        let read = tokio::time::timeout(
            Duration::from_secs(10),
            superbackup_core::ipc::codec::read_line(&mut self.reader, &mut self.line, 8 << 20),
        )
        .await
        .ok()?;
        read.ok()?;
        serde_json::from_slice(&self.line).ok()
    }
}

/// `connection_loop` keeps a `HashMap<RequestId, JoinHandle>` of live
/// subscriptions with **no cap**. `subscribe` is intercepted by the transport,
/// so it never touches `max_inflight`; each new request id spawns another
/// `pump` task and another `broadcast::Receiver`.
///
/// One connection can therefore hold an unbounded number of subscriptions,
/// paced only by the rate limiter (50/s by default — 180 000 an hour). Each
/// receiver also makes every published event O(subscribers) to fan out, so
/// this degrades the engine's event publishing as well as the daemon's memory.
///
/// 100 here is only what fits inside the default burst; nothing in the code
/// stops it at any number.
#[tokio::test]
async fn one_connection_can_open_unbounded_subscriptions() {
    let limits = Limits::default();
    let burst = limits.request_burst as usize;
    let h = Harness::start("subs", limits);

    let mut raw = RawClient::connect(&h.endpoint).await;
    raw.read_frame().await.expect("hello");

    for id in 1..=burst as u64 {
        assert!(
            raw.write_frame(&ClientFrame::Request {
                id: RequestId(id),
                protocol: superbackup_core::ipc::PROTOCOL_VERSION,
                body: Request::Subscribe { topics: vec![Topic::Status] },
            })
            .await,
            "the daemon closed the connection at subscription {id}"
        );
    }
    // Drain the acknowledgements so the writer channel does not back up.
    for _ in 0..burst {
        raw.read_frame().await.expect("a reply per subscribe");
    }

    let live = h.handler.subscriber_count();
    assert!(
        live < 16,
        "one connection holds {live} concurrent subscriptions; there is no \
         per-connection subscription cap, and `max_inflight` ({}) does not \
         apply because `subscribe` is handled by the transport",
        16
    );
    h.stop().await;
}

/// `max_connections` is a semaphore whose permits are held for the whole life
/// of a connection, and `connection_loop` has **no idle timeout**: its
/// `select!` waits on shutdown, `read_line`, and finished requests, and nothing
/// else.
///
/// So a client that connects `max_connections` times and then says nothing at
/// all holds every permit forever. The tray, the GUI and the CLI are locked out
/// of their own daemon until it is restarted.
#[tokio::test]
async fn silent_connections_hold_the_connection_cap_forever() {
    let limits = Limits { max_connections: 2, ..Limits::default() };
    let h = Harness::start("wedge", limits);

    // Two connections that greet and then go silent. Kept alive in scope.
    let mut squatters = Vec::new();
    for _ in 0..2 {
        let mut c = RawClient::connect(&h.endpoint).await;
        c.read_frame().await.expect("hello");
        squatters.push(c);
    }

    // Give the daemon a generous chance to reclaim them.
    tokio::time::sleep(Duration::from_secs(2)).await;

    // A legitimate client now tries to connect.
    let mut victim = RawClient::connect(&h.endpoint).await;
    victim.read_frame().await.expect("hello");
    let second = victim.read_frame().await.expect("a second frame");
    let refused = matches!(
        &second,
        superbackup_core::ipc::ServerFrame::Error { body, .. }
            if body.message.contains("already serving")
    );

    assert!(
        !refused,
        "two idle connections that sent nothing have permanently consumed the \
         whole connection cap; a real client is refused with {second:?} and no \
         idle timeout will ever reclaim the permits"
    );
    drop(squatters);
    h.stop().await;
}
