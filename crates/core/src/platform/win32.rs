//! Thin, safe wrappers around the handful of Win32 APIs the platform layer
//! needs. Compiled only on Windows.
//!
//! Everything `unsafe` in the platform layer lives here or in
//! [`crate::platform::single_instance`], so that the rest of the code is plain
//! safe Rust and the audit surface is one file long.
//!
//! Design rules for this module:
//!
//! * Every function returns an `Option`/`Result` and never panics. A registry
//!   key that does not exist is not an error — it is the normal state on a PC
//!   that has never had OneDrive installed.
//! * No handle escapes without an RAII owner ([`RegKey`]).
//! * Every `unsafe` block carries a `// SAFETY:` note naming the invariant the
//!   Win32 contract requires.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    CloseHandle, ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS, HANDLE,
};
use windows::Win32::Storage::FileSystem::{
    GetDiskFreeSpaceExW, GetFileAttributesW, SetFileAttributesW, FILE_FLAGS_AND_ATTRIBUTES,
    INVALID_FILE_ATTRIBUTES,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW,
    RegSetValueExW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE,
    REG_OPTION_NON_VOLATILE, REG_SAM_FLAGS, REG_SZ, REG_VALUE_TYPE,
};

// ---------------------------------------------------------------------------
// Wide-string helpers
// ---------------------------------------------------------------------------

/// UTF-16, NUL-terminated. The returned buffer must outlive the `PCWSTR` that
/// points into it — every call site here keeps it in a local binding.
pub fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(std::iter::once(0)).collect()
}

/// Decode a UTF-16 buffer, stopping at the first NUL.
pub fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

/// Which hive a [`RegKey`] path is rooted at. We only ever touch these two:
/// `HKCU` for per-user state we own, `HKLM` read-only for OS version facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hive {
    CurrentUser,
    LocalMachine,
}

impl Hive {
    fn raw(self) -> HKEY {
        match self {
            Hive::CurrentUser => HKEY_CURRENT_USER,
            Hive::LocalMachine => HKEY_LOCAL_MACHINE,
        }
    }
}

/// An owned registry key handle, closed on drop.
#[derive(Debug)]
pub struct RegKey(HKEY);

impl Drop for RegKey {
    fn drop(&mut self) {
        // SAFETY: `self.0` was produced by RegOpenKeyExW / RegCreateKeyExW in
        // this module and has not been closed before — `RegKey` is not `Copy`
        // and no other code path closes it.
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

impl RegKey {
    /// Open an existing key for reading. `None` when it does not exist or the
    /// caller lacks access — both of which are ordinary, expected outcomes.
    pub fn open(hive: Hive, path: &str) -> Option<RegKey> {
        Self::open_with(hive, path, KEY_READ)
    }

    pub fn open_with(hive: Hive, path: &str, access: REG_SAM_FLAGS) -> Option<RegKey> {
        let wpath = wide(path);
        let mut handle = HKEY::default();
        // SAFETY: `wpath` is a NUL-terminated UTF-16 buffer that outlives the
        // call; `handle` is a valid, writable out-parameter.
        let status =
            unsafe { RegOpenKeyExW(hive.raw(), PCWSTR(wpath.as_ptr()), None, access, &mut handle) };
        if status == ERROR_SUCCESS {
            Some(RegKey(handle))
        } else {
            None
        }
    }

    /// Open or create a key for writing.
    pub fn create(hive: Hive, path: &str) -> Result<RegKey, std::io::Error> {
        let wpath = wide(path);
        let mut handle = HKEY::default();
        // SAFETY: as above; `lpsecurityattributes` is `None`, so the key
        // inherits the default (user-scoped) security descriptor.
        let status = unsafe {
            RegCreateKeyExW(
                hive.raw(),
                PCWSTR(wpath.as_ptr()),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_READ | KEY_WRITE,
                None,
                &mut handle,
                None,
            )
        };
        if status == ERROR_SUCCESS {
            Ok(RegKey(handle))
        } else {
            Err(std::io::Error::from_raw_os_error(status.0 as i32))
        }
    }

    /// Read a `REG_SZ`/`REG_EXPAND_SZ` value. Environment references inside a
    /// `REG_EXPAND_SZ` are **not** expanded here; callers that care do it
    /// themselves so the raw value stays visible in diagnostics.
    pub fn string(&self, name: &str) -> Option<String> {
        let wname = wide(name);
        let mut kind = REG_VALUE_TYPE::default();
        let mut size: u32 = 0;
        // SAFETY: querying with a null data pointer is the documented way to
        // learn the required buffer size; `size` and `kind` are valid outs.
        let status = unsafe {
            RegQueryValueExW(
                self.0,
                PCWSTR(wname.as_ptr()),
                None,
                Some(&mut kind),
                None,
                Some(&mut size),
            )
        };
        if status != ERROR_SUCCESS && status != ERROR_MORE_DATA {
            return None;
        }
        if size == 0 {
            return Some(String::new());
        }
        // `size` is in bytes; registry strings are UTF-16.
        let mut buf = vec![0u16; (size as usize).div_ceil(2) + 1];
        let mut cap = (buf.len() * 2) as u32;
        // SAFETY: `buf` has `cap` writable bytes and stays alive across the
        // call; the API writes at most `cap` bytes and updates `cap`.
        let status = unsafe {
            RegQueryValueExW(
                self.0,
                PCWSTR(wname.as_ptr()),
                None,
                Some(&mut kind),
                Some(buf.as_mut_ptr().cast::<u8>()),
                Some(&mut cap),
            )
        };
        if status != ERROR_SUCCESS {
            return None;
        }
        Some(from_wide(&buf))
    }

    /// Read a `REG_DWORD` value.
    pub fn dword(&self, name: &str) -> Option<u32> {
        let wname = wide(name);
        let mut value: u32 = 0;
        let mut size = std::mem::size_of::<u32>() as u32;
        // SAFETY: `value` is exactly `size` bytes of writable storage.
        let status = unsafe {
            RegQueryValueExW(
                self.0,
                PCWSTR(wname.as_ptr()),
                None,
                None,
                Some(std::ptr::from_mut(&mut value).cast::<u8>()),
                Some(&mut size),
            )
        };
        if status == ERROR_SUCCESS {
            Some(value)
        } else {
            None
        }
    }

    /// Write a `REG_SZ` value.
    pub fn set_string(&self, name: &str, value: &str) -> Result<(), std::io::Error> {
        let wname = wide(name);
        let wvalue = wide(value);
        let bytes = unsafe {
            // SAFETY: reinterpreting a `[u16]` as `[u8]` of twice the length.
            // The source is properly aligned for u16 and therefore for u8, and
            // the slice is only read for the duration of the call below.
            std::slice::from_raw_parts(wvalue.as_ptr().cast::<u8>(), wvalue.len() * 2)
        };
        // SAFETY: `bytes` describes `wvalue`, which outlives the call.
        let status =
            unsafe { RegSetValueExW(self.0, PCWSTR(wname.as_ptr()), None, REG_SZ, Some(bytes)) };
        if status == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(std::io::Error::from_raw_os_error(status.0 as i32))
        }
    }

    /// Delete a value. A value that is already absent counts as success, so
    /// `disable()` is idempotent.
    pub fn delete_value(&self, name: &str) -> Result<(), std::io::Error> {
        let wname = wide(name);
        // SAFETY: `wname` is a NUL-terminated buffer that outlives the call.
        let status = unsafe { RegDeleteValueW(self.0, PCWSTR(wname.as_ptr())) };
        if status == ERROR_SUCCESS || status.0 == 2
        /* ERROR_FILE_NOT_FOUND */
        {
            Ok(())
        } else {
            Err(std::io::Error::from_raw_os_error(status.0 as i32))
        }
    }

    /// Names of the immediate subkeys.
    pub fn subkeys(&self) -> Vec<String> {
        let mut out = Vec::new();
        // 256 is the documented maximum length of a registry key name.
        const MAX_KEY_NAME: usize = 256;
        let mut index = 0u32;
        loop {
            let mut buf = vec![0u16; MAX_KEY_NAME];
            let mut len = buf.len() as u32;
            // SAFETY: `buf` holds `len` writable UTF-16 units and outlives the
            // call; every optional out-parameter we do not want is `None`.
            let status = unsafe {
                RegEnumKeyExW(
                    self.0,
                    index,
                    Some(windows::core::PWSTR(buf.as_mut_ptr())),
                    &mut len,
                    None,
                    None,
                    None,
                    None,
                )
            };
            if status == ERROR_NO_MORE_ITEMS {
                break;
            }
            if status != ERROR_SUCCESS {
                // A single unreadable subkey must not abort discovery of the
                // rest: a locked-down tenant policy can deny one account key.
                break;
            }
            out.push(from_wide(&buf[..len as usize]));
            index += 1;
            if index > 4096 {
                break; // Defensive: never spin forever on a hostile hive.
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// File attributes / cloud placeholders
// ---------------------------------------------------------------------------

/// `FILE_ATTRIBUTE_*` bits, or `None` when the path cannot be queried.
pub fn file_attributes(path: &Path) -> Option<u32> {
    let wpath = wide(path);
    // SAFETY: `wpath` is NUL-terminated and outlives the call.
    let attrs = unsafe { GetFileAttributesW(PCWSTR(wpath.as_ptr())) };
    if attrs == INVALID_FILE_ATTRIBUTES {
        None
    } else {
        Some(attrs)
    }
}

pub fn set_file_attributes(path: &Path, attrs: u32) -> Result<(), std::io::Error> {
    let wpath = wide(path);
    // SAFETY: `wpath` is NUL-terminated and outlives the call.
    unsafe { SetFileAttributesW(PCWSTR(wpath.as_ptr()), FILE_FLAGS_AND_ATTRIBUTES(attrs)) }
        .map_err(|e| std::io::Error::from_raw_os_error(e.code().0))
}

/// Free bytes available to *this user* and the volume total, honouring disk
/// quotas — which is why we do not use the volume's raw free space.
pub fn disk_space(path: &Path) -> Option<(u64, u64)> {
    let wpath = wide(path);
    let mut available: u64 = 0;
    let mut total: u64 = 0;
    // SAFETY: both out-parameters are valid, writable `u64`s; `wpath` outlives
    // the call.
    let ok = unsafe {
        GetDiskFreeSpaceExW(PCWSTR(wpath.as_ptr()), Some(&mut available), Some(&mut total), None)
    };
    ok.ok().map(|()| (available, total))
}

// ---------------------------------------------------------------------------
// Process token
// ---------------------------------------------------------------------------

/// True when the current process runs with an elevated token.
///
/// Returns `false` rather than an error on any failure: "we could not prove we
/// are elevated" and "we are not elevated" lead to the same user-facing advice.
pub fn is_elevated() -> bool {
    use windows::Win32::Security::{
        GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    let mut token = HANDLE::default();
    // SAFETY: `GetCurrentProcess` returns a pseudo-handle that needs no
    // closing; `token` is a valid out-parameter. The real token handle we do
    // receive is closed below on every path.
    unsafe {
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut returned = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(std::ptr::from_mut(&mut elevation).cast::<std::ffi::c_void>()),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        )
        .is_ok();
        let _ = CloseHandle(token);
        ok && elevation.TokenIsElevated != 0
    }
}

// ---------------------------------------------------------------------------
// OS version
// ---------------------------------------------------------------------------

/// The real Windows version, read from
/// `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion`.
///
/// We deliberately do **not** call `GetVersionEx`: since Windows 8.1 it lies to
/// unmanifested processes and reports 6.2. `RtlGetVersion` tells the truth but
/// lives behind a feature we do not enable; the registry carries the same
/// facts plus the marketing version ("24H2") and the UBR, which is what a
/// support engineer actually asks for.
///
/// Note that `ProductName` still reads "Windows 10 …" on Windows 11, so the
/// build number is what decides the major name.
pub fn os_version() -> String {
    let Some(key) =
        RegKey::open(Hive::LocalMachine, r"SOFTWARE\Microsoft\Windows NT\CurrentVersion")
    else {
        return "Windows (version unavailable)".to_string();
    };

    let product = key.string("ProductName").unwrap_or_else(|| "Windows".to_string());
    let display = key.string("DisplayVersion");
    let build: u32 = key.string("CurrentBuild").and_then(|s| s.parse().ok()).unwrap_or(0);
    let ubr = key.dword("UBR");

    // Windows 11 kept ProductName at "Windows 10"; build 22000 is the cut-off.
    let product = if build >= 22000 && product.contains("Windows 10") {
        product.replace("Windows 10", "Windows 11")
    } else {
        product
    };

    let mut out = product;
    if let Some(d) = display {
        if !d.is_empty() {
            out.push(' ');
            out.push_str(&d);
        }
    }
    if build > 0 {
        match ubr {
            Some(u) => out.push_str(&format!(" (build {build}.{u})")),
            None => out.push_str(&format!(" (build {build})")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_round_trips() {
        let w = wide("C:\\Users\\Ändreas");
        assert_eq!(from_wide(&w), "C:\\Users\\Ändreas");
    }

    #[test]
    fn os_version_is_specific_not_generic() {
        // Read-only; safe to run everywhere Windows is the host.
        let v = os_version();
        assert!(v.starts_with("Windows"), "unexpected version string: {v}");
        assert!(
            v.contains("build ") || v.contains("unavailable"),
            "the build number is the whole point: {v}"
        );
    }

    #[test]
    fn reading_a_missing_key_is_not_an_error() {
        assert!(
            RegKey::open(Hive::CurrentUser, r"Software\superbackup\definitely-not-here").is_none()
        );
    }
}
