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
//! Belt and braces, every accepted connection is asked for `SO_PEERCRED` and
//! its effective uid is compared with the daemon's own. A mismatch is
//! disconnected before the protocol runs. Note the deliberate asymmetry with
//! Windows: the uid check is a *second* control, not the first one. A
//! filesystem permission that lets the wrong user connect and then hangs up on
//! them still leaked the endpoint's existence and burned a file descriptor.

use crate::error::{Error, Result};
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
            std::fs::create_dir_all(dir).map_err(|e| {
                Error::io(format!("creating IPC directory {}", dir.display()), e)
            })?;
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

/// Apply platform access control to the listener before it is created.
///
/// Returns an error rather than a weakened listener when the platform refuses
/// to give us the protection we asked for.
pub fn harden_listener(options: ListenerOptions<'_>) -> Result<ListenerOptions<'_>> {
    #[cfg(windows)]
    {
        use interprocess::os::windows::local_socket::ListenerOptionsExt;
        let sd = owner_only_descriptor()?;
        Ok(options.security_descriptor(sd))
    }
    #[cfg(unix)]
    {
        use interprocess::os::unix::local_socket::ListenerOptionsExt;
        // `fchmod` before `bind`, so there is no window in which the socket
        // exists with the umask's permissions.
        Ok(options.mode(0o600))
    }
    #[cfg(not(any(windows, unix)))]
    {
        Err(Error::Platform(
            "no supported IPC access-control mechanism on this platform".into(),
        ))
    }
}

/// Re-assert permissions on the endpoint once it exists.
///
/// Redundant with [`harden_listener`] on the platforms where `mode` is
/// honoured, and the only protection on any platform where it silently is
/// not. Cheap enough to do unconditionally.
pub fn finalise_endpoint(endpoint: &str) -> Result<()> {
    #[cfg(unix)]
    {
        let path = std::path::Path::new(endpoint);
        if path.exists() {
            crate::paths::harden_file(path)?;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = endpoint;
    }
    Ok(())
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
/// Failing to *obtain* credentials is not by itself grounds for refusal: some
/// platforms do not report them at all, and treating "unknown" as "hostile"
/// would make the daemon unusable there while adding nothing on the platforms
/// that do report them. The controls that actually keep strangers out — the
/// DACL and the socket mode — have already run by this point.
pub fn verify_peer(stream: &interprocess::local_socket::tokio::Stream) -> Result<PeerIdentity> {
    use interprocess::local_socket::traits::StreamCommon as _;

    let creds = match stream.peer_creds() {
        Ok(c) => c,
        Err(_) => return Ok(PeerIdentity::default()),
    };

    // `Pid` is `u32` on Windows and `pid_t` (`i32`) on unix. Widening to `i64`
    // first is lossless from either, and narrowing back is a real, checked
    // conversion on both — so this needs no `cfg` and no lint exception.
    let pid = creds.pid().and_then(|p| u32::try_from(i64::from(p)).ok());

    #[cfg(unix)]
    {
        let uid = creds.euid().map(u32::from);
        match (uid, own_uid()) {
            (Some(peer), Some(mine)) if peer != mine => Err(Error::Ipc(format!(
                "refusing a connection from uid {peer}: this endpoint serves uid {mine} only"
            ))),
            (Some(peer), Some(_)) => {
                Ok(PeerIdentity { pid, uid: Some(peer), same_user: true })
            }
            (uid, _) => Ok(PeerIdentity { pid, uid, same_user: false }),
        }
    }
    #[cfg(not(unix))]
    {
        // Windows reports only a pid. Deliberately *not* used for
        // authorisation: pids are reused, and looking up a token by pid races
        // with that reuse. The pipe's DACL is the control.
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
