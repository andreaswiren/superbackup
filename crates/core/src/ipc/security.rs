//! Endpoint hardening: who is allowed to reach the daemon at all.
//!
//! Everything else in this module tree assumes the peer is a local process
//! that is *allowed* to talk to the daemon. This file is what makes that
//! assumption true. It runs before any protocol code sees a byte.
//!
//! # Threat
//!
//! The daemon holds an unlocked vault and can read every file the user (or,
//! as a service, the machine) can read. Anyone who can write to its endpoint
//! can ask it to restore a snapshot over an arbitrary path, publish
//! configuration to a remote repository, or brute-force the master passphrase
//! at whatever rate the socket allows. The endpoint is therefore a privilege
//! boundary, and a default-permissioned endpoint is a privilege escalation.
//!
//! # Windows
//!
//! A named pipe created with a null security descriptor is accessible to
//! every authenticated account on the machine — including a second, less
//! privileged user session on a shared PC. [`owner_only_descriptor`] builds an
//! explicit DACL granting `FILE_ALL_ACCESS` to exactly three principals:
//!
//! * the account the daemon is running as (from its own process token),
//! * `NT AUTHORITY\SYSTEM` (S-1-5-18), so a service and a tray can cooperate
//!   and so an administrator can recover the endpoint,
//! * `BUILTIN\Administrators` (S-1-5-32-544), which can take ownership of the
//!   object anyway; naming it is honest rather than permissive.
//!
//! There is no "everyone" ACE, no inherited ACE, and no null DACL. Remote
//! access is refused separately: `interprocess` creates its pipes with
//! `PIPE_REJECT_REMOTE_CLIENTS` unless asked otherwise, so the endpoint is
//! unreachable over SMB.
//!
//! If the descriptor cannot be built, binding **fails**. Falling back to a
//! default-permissioned pipe would turn a rare error into a silent
//! machine-wide hole.
//!
//! # Unix
//!
//! The socket is created with mode `0600` — `interprocess` `fchmod`s before
//! `bind`, which closes the umask race — inside a directory this crate creates
//! `0700`. Both matter: on Linux the socket's own mode is enforced on
//! `connect`, while on some BSDs it historically was not, and the directory
//! permission is what holds in that case.
//!
//! `fchmod` on a socket is rejected by macOS, where `interprocess` turns it
//! into an `Unsupported` error from the *create* call — so on that platform the
//! preferred path does not bind at all. [`create_listener`] falls back to
//! binding inside a `umask(0o077)` window and then proves the result with
//! [`verify_endpoint_mode`], which chmods to `0600` and refuses to serve if the
//! filesystem did not take it.
//!
//! Belt and braces, every accepted connection is asked for `SO_PEERCRED` and
//! its effective uid is compared with the daemon's own. A mismatch is
//! disconnected before the protocol runs. Note the deliberate asymmetry with
//! Windows: the uid check is a *second* control, not the first one. A
//! filesystem permission that lets the wrong user connect and then hangs up on
//! them still leaked the endpoint's existence and burned a file descriptor.

use crate::error::{Error, Result};
use interprocess::local_socket::tokio::Listener;
use interprocess::local_socket::ListenerOptions;

use super::protocol::PeerIdentity;

/// Make sure the endpoint's containing directory exists and is private.
///
/// Unix only in effect: on Windows the pipe namespace is not a filesystem and
/// there is nothing to create.
pub fn prepare_endpoint(endpoint: &str) -> Result<()> {
    #[cfg(unix)]
    {
        let path = std::path::Path::new(endpoint);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| Error::io(format!("creating IPC directory {}", dir.display()), e))?;
            // 0700: nobody but us may even enumerate the socket.
            crate::paths::harden_dir(dir)?;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = endpoint;
    }
    Ok(())
}

/// Create the listener with platform access control applied.
///
/// Fails rather than returning a weakened listener: an endpoint that anyone
/// can reach is worse than no endpoint at all, because the failure is silent.
///
/// ## Windows
///
/// Attaches the owner-only security descriptor and creates the pipe.
///
/// ## Unix
///
/// Preferred path: `interprocess` `fchmod`s the socket to `0600` *before*
/// `bind`, so there is no window in which it exists with the umask's
/// permissions.
///
/// `fchmod` on a socket is not portable. Linux, FreeBSD 14.3+ and OpenBSD
/// accept it; **macOS returns `EINVAL`**, which `interprocess` surfaces as
/// [`ErrorKind::Unsupported`](std::io::ErrorKind::Unsupported) from the
/// *create* call — so there the preferred path does not merely skip the
/// hardening, it refuses to bind at all, and the daemon would not run on a
/// platform this product supports.
///
/// Fallback path, taken only on that error: bind inside a `umask(0o077)`
/// window so the socket is created `0700` rather than with whatever the
/// inherited umask allows, then [`verify_endpoint_mode`] chmods it to `0600`
/// and **checks the result**, failing closed if the filesystem did not take
/// it. `umask` is per process rather than per thread, so a file created by
/// another thread during the window would also be created restrictively; the
/// window is one `bind`, and a mutex serialises binds against each other. That
/// residual race is the price of the platform not supporting the clean call,
/// and it errs towards *more* restrictive permissions.
pub fn create_listener(options: ListenerOptions<'_>, endpoint: &str) -> Result<Listener> {
    #[cfg(windows)]
    {
        use interprocess::os::windows::local_socket::ListenerOptionsExt;
        let sd = owner_only_descriptor()?;
        options.security_descriptor(sd).create_tokio().map_err(|e| bind_error(endpoint, e))
    }
    #[cfg(unix)]
    {
        use interprocess::os::unix::local_socket::ListenerOptionsExt;
        use interprocess::TryClone;

        let fallback = options.try_clone().map_err(|e| bind_error(endpoint, e))?;
        match options.mode(0o600).create_tokio() {
            Ok(listener) => {
                verify_endpoint_mode(endpoint)?;
                Ok(listener)
            }
            Err(e) if e.kind() == std::io::ErrorKind::Unsupported => {
                tracing::debug!(
                    endpoint,
                    "this platform does not support fchmod on a socket; binding the IPC \
                     endpoint under a restrictive umask instead"
                );
                let listener = {
                    let _guard = UmaskGuard::restrictive();
                    fallback.create_tokio()
                }
                .map_err(|e| bind_error(endpoint, e))?;
                verify_endpoint_mode(endpoint)?;
                Ok(listener)
            }
            Err(e) => Err(bind_error(endpoint, e)),
        }
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = (options, endpoint);
        Err(Error::Platform("no supported IPC access-control mechanism on this platform".into()))
    }
}

/// Translate a bind failure into something a user can act on.
fn bind_error(endpoint: &str, e: std::io::Error) -> Error {
    match e.kind() {
        std::io::ErrorKind::AddrInUse => {
            Error::Ipc(format!("another superbackup daemon is already listening on {endpoint}"))
        }
        std::io::ErrorKind::PermissionDenied => {
            Error::Ipc(format!("not permitted to create the IPC endpoint {endpoint}: {e}"))
        }
        _ => Error::Ipc(format!("could not create the IPC endpoint {endpoint}: {e}")),
    }
}

/// Chmod the endpoint to `0600` and prove it took.
///
/// The proof is the point. A chmod alone would leave a filesystem that
/// silently ignores the mode serving a world-writable socket while every log
/// line said it was hardened.
pub fn verify_endpoint_mode(endpoint: &str) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let path = std::path::Path::new(endpoint);
        if !path.exists() {
            return Err(Error::Ipc(format!(
                "the IPC endpoint {endpoint} does not exist after binding"
            )));
        }
        crate::paths::harden_file(path)?;
        let mode = std::fs::metadata(path)
            .map_err(|e| Error::io(format!("reading the mode of {endpoint}"), e))?
            .permissions()
            .mode()
            & 0o777;
        if mode & 0o077 != 0 {
            return Err(Error::Ipc(format!(
                "refusing to serve on {endpoint}: it is mode {mode:04o} and this filesystem \
                 would not restrict it to 0600, so other users could connect"
            )));
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = endpoint;
        Ok(())
    }
}

/// Serialises the `umask` window in [`create_listener`]'s fallback path.
///
/// `umask` is per *process*, not per thread, so two threads binding at once
/// could restore each other's value. Binding happens once at start-up, so
/// contention is theoretical, but a lock is cheaper than reasoning about it.
#[cfg(unix)]
static UMASK_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// `mode_t`, which is not the same width everywhere.
///
/// Declaring `umask` with the wrong integer width is an ABI mismatch, so this
/// follows the platform headers rather than guessing.
#[cfg(unix)]
#[allow(non_camel_case_types)]
mod mode_t_def {
    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "tvos",
        target_os = "watchos",
        target_os = "freebsd",
        target_os = "dragonfly",
    ))]
    pub type mode_t = u16;

    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "tvos",
        target_os = "watchos",
        target_os = "freebsd",
        target_os = "dragonfly",
    )))]
    pub type mode_t = u32;
}

#[cfg(unix)]
extern "C" {
    /// Sets the process file-mode creation mask and returns the previous one.
    /// Cannot fail and has no other effect.
    fn umask(mask: mode_t_def::mode_t) -> mode_t_def::mode_t;
}

/// Sets `umask` to `0o077` and restores the previous value on drop, including
/// on an unwind.
#[cfg(unix)]
struct UmaskGuard {
    previous: mode_t_def::mode_t,
    _lock: std::sync::MutexGuard<'static, ()>,
}

#[cfg(unix)]
impl UmaskGuard {
    fn restrictive() -> UmaskGuard {
        // A poisoned lock guards nothing but an integer; take it anyway rather
        // than skipping the umask and binding with the inherited one.
        let lock = UMASK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let previous = unsafe { umask(0o077) };
        UmaskGuard { previous, _lock: lock }
    }
}

#[cfg(unix)]
impl Drop for UmaskGuard {
    fn drop(&mut self) {
        unsafe { umask(self.previous) };
    }
}

/// The effective uid of this process, or `None` off unix.
#[cfg(unix)]
fn own_uid() -> Option<u32> {
    // `libc` is not a direct dependency of this crate and the transport must
    // not add one to ask a two-line question. `geteuid` is in libc, which is
    // linked into every unix Rust binary, cannot fail, and has had this
    // signature since the 1980s.
    extern "C" {
        fn geteuid() -> u32;
    }
    Some(unsafe { geteuid() })
}

/// No uid concept on this platform; access control is the endpoint's DACL.
#[cfg(not(unix))]
#[allow(dead_code)]
fn own_uid() -> Option<u32> {
    None
}

/// Decide whether an accepted connection may proceed, and describe its peer.
///
/// `Err` means "hang up now". `Ok` means the connection is allowed; the
/// returned [`PeerIdentity`] is recorded on every
/// [`RequestContext`](super::RequestContext) so the
/// daemon can attribute actions in its event log.
///
/// # Failure modes
///
/// **On unix this fails closed.** If `SO_PEERCRED`/`getpeereid` cannot be read,
/// or reports no uid, the connection is refused. `THREAT_MODEL.md` §A2 promises
/// "peer-UID verification on Unix" without qualification, and a soft-fail would
/// make that promise conditional on an error path nobody can observe. The check
/// also cannot legitimately fail: every unix target this crate supports reports
/// a peer uid for a connected `AF_UNIX` socket — Linux and the other
/// `ucred`-based systems through `SO_PEERCRED`, macOS and the BSDs through
/// `LOCAL_PEERCRED`/`getpeereid`, which `interprocess` wraps for all of them.
/// So the closed path costs a working deployment nothing and the open path
/// would have cost the guarantee everything.
///
/// **On Windows it does not apply.** The platform reports only a pid, which is
/// reused and races with the lookup, so there is nothing here to verify; the
/// pipe's DACL is the control and it has already run.
pub fn verify_peer(stream: &interprocess::local_socket::tokio::Stream) -> Result<PeerIdentity> {
    use interprocess::local_socket::traits::StreamCommon as _;

    #[cfg(unix)]
    {
        let creds = stream.peer_creds().map_err(|e| {
            Error::Ipc(format!(
                "refusing a connection whose peer credentials could not be read: {e}"
            ))
        })?;
        let pid = creds.pid().and_then(|p| u32::try_from(i64::from(p)).ok());
        let peer = creds.euid().map(u32::from).ok_or_else(|| {
            Error::Ipc(
                "refusing a connection: this platform reported no peer uid for a unix socket"
                    .into(),
            )
        })?;
        let mine = own_uid().ok_or_else(|| {
            Error::Ipc("refusing a connection: this process has no effective uid".into())
        })?;
        if peer != mine {
            return Err(Error::Ipc(format!(
                "refusing a connection from uid {peer}: this endpoint serves uid {mine} only"
            )));
        }
        Ok(PeerIdentity { pid, uid: Some(peer), same_user: true })
    }
    #[cfg(not(unix))]
    {
        // `Pid` is `u32` on Windows and `pid_t` (`i32`) on unix. Widening to
        // `i64` first is lossless from either and narrowing back is checked.
        let pid = stream
            .peer_creds()
            .ok()
            .and_then(|c| c.pid())
            .and_then(|p| u32::try_from(i64::from(p)).ok());
        Ok(PeerIdentity { pid, uid: None, same_user: false })
    }
}

// ---------------------------------------------------------------------------
// Windows security descriptor
// ---------------------------------------------------------------------------

/// Build a security descriptor whose DACL grants full access to the current
/// user, `SYSTEM` and `BUILTIN\Administrators`, and to nobody else.
///
/// The result is an *owned, absolute* descriptor: `interprocess` deep-copies
/// our ACL onto the local heap, so every temporary allocated here is released
/// before this function returns.
///
/// `interprocess` exposes exactly the hook needed for this
/// (`ListenerOptionsExt::security_descriptor`) but deliberately provides no
/// way to *construct* a descriptor beyond SDDL parsing, which would need a
/// `widestring` dependency this crate does not have. Building the ACL by hand
/// through the `windows` crate is the remaining option and is what this does.
#[cfg(windows)]
pub fn owner_only_descriptor(
) -> Result<interprocess::os::windows::security_descriptor::SecurityDescriptor> {
    use interprocess::os::windows::security_descriptor::{
        AsSecurityDescriptorExt, BorrowedSecurityDescriptor,
    };
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{
        AddAccessAllowedAce, AllocateAndInitializeSid, FreeSid, GetLengthSid, GetTokenInformation,
        InitializeAcl, InitializeSecurityDescriptor, SetSecurityDescriptorDacl, TokenUser,
        ACCESS_ALLOWED_ACE, ACL, ACL_REVISION, PSECURITY_DESCRIPTOR, PSID, SECURITY_DESCRIPTOR,
        SECURITY_NT_AUTHORITY, TOKEN_QUERY, TOKEN_USER,
    };
    use windows::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    /// `SECURITY_DESCRIPTOR_REVISION`, from `winnt.h`. Not re-exported by the
    /// `windows` crate features this crate enables, and it has been 1 since
    /// Windows NT 3.1.
    const SD_REVISION: u32 = 1;
    /// `SECURITY_LOCAL_SYSTEM_RID` — S-1-5-18.
    const RID_LOCAL_SYSTEM: u32 = 18;
    /// `SECURITY_BUILTIN_DOMAIN_RID` — the `S-1-5-32` prefix.
    const RID_BUILTIN_DOMAIN: u32 = 32;
    /// `DOMAIN_ALIAS_RID_ADMINS` — S-1-5-32-544.
    const RID_ADMINS: u32 = 544;

    /// Frees a SID from `AllocateAndInitializeSid` however we leave the
    /// function, including on the error paths below.
    struct OwnedSid(PSID);
    impl Drop for OwnedSid {
        fn drop(&mut self) {
            if !self.0.is_invalid() {
                unsafe { FreeSid(self.0) };
            }
        }
    }

    fn oserr(what: &str, e: windows::core::Error) -> Error {
        Error::Platform(format!("{what} failed while securing the IPC endpoint: {e}"))
    }

    unsafe {
        // --- the account this process runs as -------------------------------
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)
            .map_err(|e| oserr("OpenProcessToken", e))?;

        let mut needed = 0u32;
        // First call is expected to fail with ERROR_INSUFFICIENT_BUFFER; it is
        // how the required size is discovered.
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut needed);
        if needed == 0 {
            let _ = CloseHandle(token);
            return Err(Error::Platform(
                "GetTokenInformation reported a zero-length token user".into(),
            ));
        }
        // `u64`-aligned so the `TOKEN_USER` and the SID it points at are
        // correctly aligned for the API that reads them back.
        let mut token_buf = vec![0u64; (needed as usize).div_ceil(8)];
        let token_result = GetTokenInformation(
            token,
            TokenUser,
            Some(token_buf.as_mut_ptr().cast()),
            needed,
            &mut needed,
        );
        let _ = CloseHandle(token);
        token_result.map_err(|e| oserr("GetTokenInformation", e))?;

        let token_user = &*(token_buf.as_ptr() as *const TOKEN_USER);
        let user_sid = token_user.User.Sid;
        if user_sid.is_invalid() {
            return Err(Error::Platform("process token has no user SID".into()));
        }

        // --- SYSTEM and Administrators --------------------------------------
        let mut system = PSID::default();
        AllocateAndInitializeSid(
            &SECURITY_NT_AUTHORITY,
            1,
            RID_LOCAL_SYSTEM,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            &mut system,
        )
        .map_err(|e| oserr("AllocateAndInitializeSid(SYSTEM)", e))?;
        let system = OwnedSid(system);

        let mut admins = PSID::default();
        AllocateAndInitializeSid(
            &SECURITY_NT_AUTHORITY,
            2,
            RID_BUILTIN_DOMAIN,
            RID_ADMINS,
            0,
            0,
            0,
            0,
            0,
            0,
            &mut admins,
        )
        .map_err(|e| oserr("AllocateAndInitializeSid(Administrators)", e))?;
        let admins = OwnedSid(admins);

        let principals = [user_sid, system.0, admins.0];

        // --- the ACL ---------------------------------------------------------
        // One ACCESS_ALLOWED_ACE per principal. `SidStart` is the first DWORD
        // of the variable-length SID, so the fixed part of the ACE is its size
        // minus that DWORD.
        let ace_overhead = std::mem::size_of::<ACCESS_ALLOWED_ACE>() - std::mem::size_of::<u32>();
        let mut acl_bytes = std::mem::size_of::<ACL>();
        for sid in principals {
            acl_bytes += ace_overhead + GetLengthSid(sid) as usize;
        }
        let acl_bytes = u32::try_from(acl_bytes)
            .map_err(|_| Error::Platform("IPC ACL size overflowed a u32".into()))?;

        // `ACL` must be DWORD-aligned; a `Vec<u32>` guarantees that, a
        // `Vec<u8>` does not.
        let mut acl_buf = vec![0u32; (acl_bytes as usize).div_ceil(4)];
        let acl = acl_buf.as_mut_ptr() as *mut ACL;
        InitializeAcl(acl, acl_bytes, ACL_REVISION).map_err(|e| oserr("InitializeAcl", e))?;
        for sid in principals {
            // FILE_ALL_ACCESS rather than GENERIC_ALL: generic rights are
            // mapped at access-check time and are a common source of ACLs
            // that grant more than their author intended.
            AddAccessAllowedAce(acl, ACL_REVISION, FILE_ALL_ACCESS.0, sid)
                .map_err(|e| oserr("AddAccessAllowedAce", e))?;
        }

        // --- the descriptor --------------------------------------------------
        let mut sd = SECURITY_DESCRIPTOR::default();
        let psd = PSECURITY_DESCRIPTOR((&raw mut sd).cast());
        InitializeSecurityDescriptor(psd, SD_REVISION)
            .map_err(|e| oserr("InitializeSecurityDescriptor", e))?;
        // `daclpresent = true` with a non-null ACL. A *null* ACL here would
        // mean "grant everyone everything", which is the exact bug this
        // function exists to avoid; an *absent* DACL would inherit.
        SetSecurityDescriptorDacl(psd, true, Some(acl.cast_const()), false)
            .map_err(|e| oserr("SetSecurityDescriptorDacl", e))?;

        // `to_owned_sd` deep-copies the descriptor and its ACL onto the local
        // heap under `interprocess`'s ownership, so `acl_buf`, `token_buf` and
        // the two SIDs can all be released when this scope ends.
        let borrowed = BorrowedSecurityDescriptor::from_ptr(psd.0.cast_const());
        let owned = borrowed
            .to_owned_sd()
            .map_err(|e| Error::Platform(format!("copying the IPC security descriptor: {e}")))?;
        drop(system);
        drop(admins);
        Ok(owned)
    }
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    #[test]
    fn windows_descriptor_builds() {
        // The descriptor is validated by `IsValidSecurityDescriptor` inside
        // `interprocess` under `debug_assert!`, which tests run with. A
        // malformed ACL therefore fails here rather than at bind time on a
        // user's machine.
        super::owner_only_descriptor().expect("owner-only security descriptor must build");
    }

    #[cfg(unix)]
    #[test]
    fn own_uid_is_available_on_unix() {
        assert!(super::own_uid().is_some());
    }
}
