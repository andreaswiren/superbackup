//! Newline-delimited JSON framing.
//!
//! Two operations, both of which have to be right or the transport is a
//! liability:
//!
//! * [`read_line`] reads one line **without ever buffering more than the
//!   cap**. `AsyncBufReadExt::read_line` cannot be used here: it grows its
//!   destination until it finds a newline, so a peer that writes a gigabyte of
//!   `x` and no `\n` takes the daemon's memory with it. This implementation
//!   inspects the buffered chunk, stops the moment the accumulated length
//!   would exceed the cap, and reports [`LineError::TooLong`] instead.
//! * [`write_line`] serialises a frame and appends exactly one `\n`, in a
//!   single write, so two concurrent writers cannot interleave halves of two
//!   frames on the wire.
//!
//! The read buffer is zeroed before it is reused. A `vault.unlock` line
//! contains the master passphrase; leaving it in a recycled `Vec` for the rest
//! of the connection's life would be careless. See
//! [`SecretString`](super::SecretString) for what this does and does not
//! achieve.

use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};
use zeroize::Zeroize;

/// Why a line could not be read.
#[derive(Debug)]
pub enum LineError {
    /// The peer closed the connection cleanly between frames.
    Eof,
    /// The line exceeded the cap. The caller must close the connection: the
    /// remainder of the oversized line is still in the stream and there is no
    /// safe way to resynchronise, because we cannot tell attacker-chosen
    /// bytes from the start of a legitimate next frame.
    TooLong {
        /// The cap that was exceeded, for the error message the peer is sent.
        limit: usize,
    },
    /// The socket failed.
    Io(std::io::Error),
}

impl std::fmt::Display for LineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LineError::Eof => f.write_str("connection closed by peer"),
            LineError::TooLong { limit } => {
                write!(f, "line exceeds the {limit}-byte limit")
            }
            LineError::Io(e) => write!(f, "socket error: {e}"),
        }
    }
}

impl std::error::Error for LineError {}

impl From<std::io::Error> for LineError {
    fn from(e: std::io::Error) -> Self {
        LineError::Io(e)
    }
}

/// Read one `\n`-terminated line into `buf`, appending nothing beyond the cap.
///
/// `buf` is cleared and zeroed on entry, so the caller may reuse one buffer
/// for the life of a connection without leaving an old passphrase in it.
///
/// The trailing `\n` and any `\r` before it are stripped, so the caller gets
/// exactly the JSON. A blank line yields an empty `buf`; callers treat that as
/// a keep-alive and ignore it, which lets a client hold a connection open
/// through a firewall or a pipe-idle timer without inventing a ping command.
pub async fn read_line<R>(
    reader: &mut R,
    buf: &mut Vec<u8>,
    limit: usize,
) -> std::result::Result<(), LineError>
where
    R: AsyncBufRead + Unpin,
{
    buf.zeroize();
    buf.clear();

    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            // A partial line at EOF is discarded rather than delivered: a
            // half-written frame is not a frame, and guessing would let a
            // truncated request execute.
            return Err(LineError::Eof);
        }

        match available.iter().position(|b| *b == b'\n') {
            Some(idx) => {
                // `idx` bytes plus the newline are consumed; only `idx` are kept.
                if buf.len() + idx > limit {
                    return Err(LineError::TooLong { limit });
                }
                buf.extend_from_slice(&available[..idx]);
                reader.consume(idx + 1);
                if buf.last() == Some(&b'\r') {
                    buf.pop();
                }
                return Ok(());
            }
            None => {
                let take = available.len();
                if buf.len() + take > limit {
                    // Do not consume: the connection is about to be closed and
                    // consuming would only pull more attacker-chosen bytes
                    // into our address space.
                    return Err(LineError::TooLong { limit });
                }
                buf.extend_from_slice(available);
                reader.consume(take);
            }
        }
    }
}

/// Serialise `frame` and write it as one line.
///
/// One `write_all` for the whole frame including its newline. Splitting the
/// newline into a second write would let a concurrently scheduled writer slip
/// a frame in between and corrupt both.
pub async fn write_line<W, T>(writer: &mut W, frame: &T) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let mut line = serde_json::to_vec(frame).map_err(|e| {
        // A frame that will not serialise is a bug in this crate, not a peer
        // problem; surface it as an I/O error so the connection loop reports
        // it rather than silently dropping the response.
        std::io::Error::new(std::io::ErrorKind::InvalidData, e)
    })?;
    line.push(b'\n');
    writer.write_all(&line).await?;
    writer.flush().await
}

/// Parse one line into a frame.
///
/// Malformed JSON is a peer error, never a panic and never a reason to drop
/// the connection: the caller answers with an `error` frame and reads the next
/// line. A client that is one version ahead and sends a command this build
/// does not know gets `unknown variant`, which is the correct, actionable
/// answer.
pub fn parse<T: DeserializeOwned>(line: &[u8]) -> std::result::Result<T, serde_json::Error> {
    serde_json::from_slice(line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::BufReader;

    async fn read_all(input: &[u8], limit: usize) -> Vec<std::result::Result<String, LineError>> {
        let mut reader = BufReader::new(input);
        let mut buf = Vec::new();
        let mut out = Vec::new();
        loop {
            match read_line(&mut reader, &mut buf, limit).await {
                Ok(()) => out.push(Ok(String::from_utf8_lossy(&buf).into_owned())),
                Err(LineError::Eof) => return out,
                Err(e) => {
                    out.push(Err(e));
                    return out;
                }
            }
        }
    }

    #[tokio::test]
    async fn splits_on_newlines_and_strips_cr() {
        let got = read_all(b"one\r\ntwo\n\nthree\n", 1024).await;
        let lines: Vec<&str> =
            got.iter().filter_map(|r| r.as_ref().ok()).map(|s| s.as_str()).collect();
        assert_eq!(lines, vec!["one", "two", "", "three"]);
    }

    #[tokio::test]
    async fn rejects_a_line_over_the_cap_without_buffering_it() {
        let huge = format!("{}\n", "x".repeat(64));
        let got = read_all(huge.as_bytes(), 16).await;
        assert!(matches!(got.last(), Some(Err(LineError::TooLong { limit: 16 }))));
    }

    #[tokio::test]
    async fn a_line_exactly_at_the_cap_is_accepted() {
        let exact = format!("{}\n", "x".repeat(16));
        let got = read_all(exact.as_bytes(), 16).await;
        assert!(matches!(got.first(), Some(Ok(s)) if s.len() == 16), "{got:?}");
    }

    #[tokio::test]
    async fn unterminated_trailing_data_is_eof_not_a_line() {
        let got = read_all(b"complete\nincomplete", 1024).await;
        assert_eq!(got.len(), 1, "a line without a newline must not be delivered: {got:?}");
    }
}
