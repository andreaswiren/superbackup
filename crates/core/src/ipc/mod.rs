//! Inter-process control surface: one daemon, many front ends.
//!
//! The tray, the GUI and the CLI are three views of a single background
//! process. They talk to it here, over a local socket — a named pipe on
//! Windows, a unix-domain socket everywhere else — carrying newline-delimited
//! JSON.
//!
//! ```text
//!   tray ─┐
//!   GUI  ─┼─▶ ipc::client ─▶ local socket ─▶ ipc::server ─▶ Handler ─▶ engine
//!   CLI  ─┤                                                   (daemon)
//!  agent ─┘
//! ```
//!
//! Start with [`protocol`]: it is the specification, and it is written to be
//! implementable by another team without reading any Rust.
//!
//! # Why this shape
//!
//! **Newline-delimited JSON, not a binary codec.** The daemon is a privileged
//! long-running process that users will one day need to debug in the field.
//! A protocol you can drive from a terminal, paste into a bug report, and
//! implement from a PowerShell script is worth more than the microseconds a
//! binary framing would save on a socket that carries a few messages a second.
//!
//! **A generated schema, not a document.** [`Request`], the [`Handler`] trait,
//! the dispatcher and [`Schema`] all come out of one macro invocation. An AI
//! agent asks `{"cmd":"schema"}` and gets the truth, not a copy of the truth
//! that stopped being true two releases ago.
//!
//! **Secrets go in, never out.** There is no request that returns plaintext
//! credential material. See [`protocol`] for the reasoning and the
//! consequences.
//!
//! # Security posture
//!
//! An IPC endpoint is a privilege boundary. The daemon may run as SYSTEM or
//! root while its clients are ordinary user processes, so the transport
//! assumes the peer is hostile until the OS says otherwise:
//!
//! * **Windows.** The named pipe is created with an explicit DACL granting
//!   full access to exactly three principals — the creating user, `SYSTEM`
//!   and `BUILTIN\Administrators` — and to nobody else. Creating it with the
//!   default (inherited, null) DACL would leave it open to every account on
//!   the machine. `PIPE_REJECT_REMOTE_CLIENTS` is set, so the endpoint is
//!   unreachable over SMB. If the descriptor cannot be built, [`Server::bind`]
//!   **fails** rather than falling back to an open pipe.
//! * **Unix.** The socket is created mode `0600` inside a `0700` directory,
//!   and every accepted connection's `SO_PEERCRED` uid is compared against the
//!   daemon's own effective uid. A different user is disconnected before a
//!   byte is read.
//! * **Both.** Oversized lines close the connection, requests are rate limited
//!   per connection, total connections are capped, and a slow subscriber is
//!   made to drop items rather than allowed to grow a queue inside the daemon.
//!
//! What the transport does *not* do is treat "reached the socket" as
//! "authorised to act as SYSTEM". Anything that touches secret material
//! requires an unlocked vault, and the vault opens only for the master
//! passphrase. See [`protocol::flag::elevated`].
//!
//! # Layout
//!
//! * [`protocol`] — the wire contract: requests, replies, frames, schema.
//! * [`schema`] — the types that describe the contract to a machine.
//! * [`codec`] — line framing, with the size cap enforced.
//! * [`security`] — endpoint hardening, per platform.
//! * [`server`] — the daemon side: accept, dispatch, stream, shut down.
//! * [`client`] — the front-end side, async and blocking.
//! * [`testing`] — a [`MockHandler`](testing::MockHandler) for tests in this
//!   crate and in the daemon.

pub mod client;
pub mod codec;
pub mod protocol;
pub mod schema;
pub mod security;
pub mod server;
pub mod testing;

pub use client::{AutoStart, BlockingClient, Client, Subscription};
pub use protocol::{
    check_protocol, commands, dispatch, schema as protocol_schema, ClientFrame, ConflictPolicy,
    ErrorPayload, Handler, PeerIdentity, Reply, Request, RequestContext, RequestId, SecretString,
    ServerFrame, StreamItem, Topic,
};
pub use schema::{CommandSpec, LimitsSpec, ParamSpec, ReplySpec, Schema, TopicSpec};
pub use server::{Server, ServerHandle, ServerOptions};

/// The protocol version this build speaks.
///
/// Bumped when a change would break an existing client: a request removed, a
/// parameter's meaning changed, a reply's shape changed. Adding a command, a
/// reply variant, or an optional parameter does **not** bump it — clients
/// ignore unknown keys and never see commands they do not send, so those are
/// backwards compatible by construction.
pub const PROTOCOL_VERSION: u32 = 1;

/// The oldest client protocol this daemon still serves.
///
/// Kept as its own constant so that dropping support for an old version is a
/// deliberate one-line change with a visible diff, rather than a side effect
/// of bumping [`PROTOCOL_VERSION`].
pub const MIN_PROTOCOL_VERSION: u32 = 1;

/// Transport limits. Every one of them exists to stop one local process from
/// degrading a daemon that other processes depend on.
///
/// The defaults are generous for a desktop backup tool and mean for an
/// attacker. They are published to clients in
/// [`Schema::limits`](schema::Schema::limits) so a well-behaved client can
/// stay inside them instead of discovering them by being disconnected.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Longest accepted line, in bytes, newline included.
    ///
    /// 1 MiB. The largest legitimate request is a `job.update` carrying a job
    /// with a long exclusion list — a few kilobytes. The largest legitimate
    /// *response* is a status snapshot with two hundred runs, which is
    /// bounded by [`crate::state::MAX_HISTORY`] and stays well under this.
    ///
    /// A longer line is not truncated and not buffered: the connection is
    /// closed. Truncating would hand the parser a fragment of attacker-chosen
    /// JSON, and buffering is precisely the OOM this cap exists to prevent.
    pub max_line_bytes: usize,

    /// Sustained requests per second, per connection.
    ///
    /// A GUI polling four times a second and a CLI issuing one command are
    /// three orders of magnitude below this. A process spinning on
    /// `vault.unlock` to brute-force a passphrase is not: at this rate an
    /// eight-character passphrase still takes longer than the heat death of
    /// the machine, and the daemon stays responsive to the real client while
    /// it happens.
    pub max_requests_per_second: u32,

    /// How far above the sustained rate a burst may go before it is rejected.
    /// A GUI that opens and fires twenty requests to paint a dashboard is
    /// normal; sustaining twenty a second is not.
    pub request_burst: u32,

    /// Concurrent connections across all clients.
    ///
    /// Tray, GUI, one or two CLI invocations, and headroom. Beyond this the
    /// daemon answers with an error frame and closes, rather than accepting a
    /// connection it will not service — a client that is told "too many
    /// connections" can retry, one that is silently queued cannot tell the
    /// difference from a hang.
    pub max_connections: usize,

    /// Items buffered for one subscriber before the oldest are dropped.
    ///
    /// The number that decides how far behind a client may fall before it is
    /// told it missed something. Large enough that a GUI redrawing at 60Hz
    /// never lags; small enough that a stopped debugger costs the daemon a
    /// few hundred kilobytes rather than everything.
    pub stream_buffer: usize,

    /// How long a handler may take before the transport gives up on it and
    /// answers with an error.
    ///
    /// Handlers are supposed to return promptly and do long work in the
    /// background — `job.run` returns a run id, it does not wait for the
    /// backup. This cap catches the case where one does not, so a wedged
    /// handler costs one request rather than the connection.
    pub handler_timeout: std::time::Duration,

    /// Requests one connection may have in flight at once.
    ///
    /// Requests are pipelined and answered concurrently, which is why every
    /// frame carries an id. This bounds the fan-out so a client cannot make
    /// the daemon spawn without limit.
    pub max_inflight: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_line_bytes: 1024 * 1024,
            max_requests_per_second: 50,
            request_burst: 100,
            max_connections: 32,
            stream_buffer: 512,
            handler_timeout: std::time::Duration::from_secs(120),
            max_inflight: 16,
        }
    }
}
