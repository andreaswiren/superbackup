//! Handing one secret to a process that had to be started with more rights
//! than this one has.
//!
//! # The problem
//!
//! Installing a Windows service under a user account needs two things this
//! process cannot have at once: the account's password, which a person types
//! into a window, and administrator rights, which a window does not have. The
//! rights come from starting a second copy under the elevation prompt — and
//! that copy has no way to see what was typed into the first.
//!
//! # Why not the obvious ways
//!
//! **Not a command-line argument.** Every process on the machine can read
//! another's command line. This is the one rule this codebase does not bend:
//! secrets reach a child through a pipe or an environment it controls, never
//! through argv.
//!
//! **Not a file.** A file exists between being written and being read, with
//! whatever the filesystem's defaults are, and is recoverable afterwards on
//! most of them. Encrypting it moves the problem to a key with the same
//! difficulty.
//!
//! **Not the environment.** An elevated child is started by `ShellExecuteEx`,
//! which does not take an environment block — the child inherits the shell's.
//!
//! # What this does
//!
//! A named pipe with an explicit DACL granting the current account, `SYSTEM`
//! and `BUILTIN\Administrators` and nobody else — the same descriptor the
//! daemon's own endpoint uses, for the same reason. The pipe's *name* is
//! random and goes on the command line, which is fine: it is not a secret, and
//! it is useless to anybody the DACL excludes.
//!
//! The elevated child is the same account under consent elevation, and an
//! administrator under over-the-shoulder elevation. Both are in the DACL.
//!
//! The secret is sent once, the pipe is closed, and it exists nowhere else.

use std::time::Duration;

use crate::error::{Error, Result};
use crate::secret::Secret;

/// How long the sender waits for the elevated process to collect the secret.
///
/// Long enough for somebody to find the administrator prompt behind another
/// window and answer it; short enough that a declined prompt does not leave a
/// pipe open all afternoon holding a password.
const COLLECT_TIMEOUT: Duration = Duration::from_secs(120);

/// A one-shot channel for one secret.
///
/// Created by the process that has the secret; its [`name`](Handover::name) is
/// passed to the process that needs it.
pub struct Handover {
    name: String,
    listener: interprocess::local_socket::tokio::Listener,
}

impl std::fmt::Debug for Handover {
    /// The name and nothing else. The listener has no useful representation,
    /// and the secret this carries never reaches the struct at all — it is
    /// passed to `send` and written straight out.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Handover").field("name", &self.name).finish_non_exhaustive()
    }
}

impl Handover {
    /// Open a channel with a name nobody can guess.
    pub fn open() -> Result<Handover> {
        use interprocess::local_socket::{ListenerOptions, ToNsName};

        let name = format!("superbackup-handover-{}", uuid::Uuid::new_v4().simple());
        let ns = endpoint_name(&name);
        let listener = crate::ipc::security::create_listener(
            ListenerOptions::new().name(
                ns.clone()
                    .to_ns_name::<interprocess::local_socket::GenericNamespaced>()
                    .map_err(|e| Error::Platform(format!("handover name: {e}")))?,
            ),
            &ns,
        )?;
        Ok(Handover { name, listener })
    }

    /// The name to give the other process. Not a secret.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Wait for the other process to connect, hand it the secret, and stop.
    ///
    /// Exactly one connection is served. A second one would be a second
    /// process asking for the same secret, which nothing legitimate does.
    pub async fn send(self, secret: &Secret) -> Result<()> {
        use interprocess::local_socket::traits::tokio::Listener as _;
        use tokio::io::AsyncWriteExt;

        let stream = tokio::time::timeout(COLLECT_TIMEOUT, self.listener.accept())
            .await
            .map_err(|_| {
                Error::Platform(
                    "the elevated installer did not start, so nothing was sent to it.".into(),
                )
            })?
            .map_err(|e| Error::io("accepting the elevated installer's connection", e))?;

        let exposed = secret.expose_zeroizing_string().ok_or_else(|| {
            Error::Platform("the password could not be read back to send it".into())
        })?;
        let mut stream = stream;
        stream
            .write_all(exposed.as_bytes())
            .await
            .map_err(|e| Error::io("handing the password to the elevated installer", e))?;
        stream.shutdown().await.map_err(|e| Error::io("closing the handover", e))?;
        Ok(())
    }
}

/// Collect the secret from a channel opened by another process.
///
/// Called by the elevated copy, with the name it was given on the command line.
pub async fn collect(name: &str) -> Result<Secret> {
    use interprocess::local_socket::traits::tokio::Stream as _;
    use interprocess::local_socket::ToNsName;
    use tokio::io::AsyncReadExt;

    let ns = endpoint_name(name);
    let target = ns
        .clone()
        .to_ns_name::<interprocess::local_socket::GenericNamespaced>()
        .map_err(|e| Error::Platform(format!("handover name: {e}")))?;
    let mut stream = interprocess::local_socket::tokio::Stream::connect(target)
        .await
        .map_err(|e| Error::io("connecting to the process that has the password", e))?;

    // Capped hard. Whatever is on the other end, this is one password.
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let read = stream
            .read(&mut chunk)
            .await
            .map_err(|e| Error::io("reading the password from the handover", e))?;
        if read == 0 {
            break;
        }
        if buffer.len() + read > 4096 {
            return Err(Error::Platform(
                "the handover sent far more than a password and was abandoned".into(),
            ));
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    let text = String::from_utf8(buffer)
        .map_err(|_| Error::Platform("the handover did not send text".into()))?;
    Ok(Secret::from_string(text))
}

/// The channel's address.
///
/// `interprocess` maps a namespaced name onto a named pipe on Windows and an
/// abstract or filesystem socket on Unix, so one spelling covers both. The
/// `.sock` suffix is what its Unix mapping expects and is harmless on the pipe
/// name Windows gets.
fn endpoint_name(name: &str) -> String {
    format!("{name}.sock")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The secret crosses the channel and nothing else does.
    #[tokio::test]
    async fn one_secret_reaches_the_other_side() {
        let handover = Handover::open().expect("open");
        let name = handover.name().to_string();

        let sender = tokio::spawn(async move {
            handover.send(&Secret::from_string("hunter2 with a space".into())).await
        });
        let collected = collect(&name).await.expect("collect");
        sender.await.expect("join").expect("send");

        assert_eq!(
            collected.expose_zeroizing_string().as_deref().map(|s| s.as_str()),
            Some("hunter2 with a space"),
            "the password must arrive exactly as it was typed, spaces and all"
        );
    }

    /// The name is not the secret, and it is never the same twice.
    ///
    /// It goes on a command line, where every process on the machine can read
    /// it, so it has to be worth nothing to anyone the DACL excludes — and it
    /// has to be unguessable, so that a second install a minute later cannot
    /// collide with one still in flight.
    #[tokio::test]
    async fn the_channel_name_is_random_and_carries_nothing() {
        let a = Handover::open().expect("a");
        let b = Handover::open().expect("b");
        assert_ne!(a.name(), b.name());
        assert!(a.name().starts_with("superbackup-handover-"));
        assert!(a.name().len() > "superbackup-handover-".len() + 16);
    }
}
