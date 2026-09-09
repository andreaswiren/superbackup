//! Deriving a key from a passkey, so a security key or a fingerprint can open
//! the vault.
//!
//! # A passkey does not hold a secret you can borrow
//!
//! The obvious design — "log in with the passkey, then unlock" — does not
//! work, and it is worth saying why, because the wrong version of this is easy
//! to build and quietly worthless.
//!
//! A passkey proves *identity*: the authenticator signs a challenge with a
//! private key it will not hand over. The signature is over a fresh random
//! challenge, so it differs every time and there is nothing in it to derive a
//! key from. An application that "unlocks" on a successful signature has not
//! encrypted anything with the passkey at all — it has written an `if` around
//! its own decryption, and anyone who can edit the configuration or attach a
//! debugger walks straight past it.
//!
//! # What does work: `hmac-secret`
//!
//! CTAP2 authenticators implement an extension for exactly this. The
//! authenticator keeps a secret of its own alongside the credential and, given
//! a salt, returns `HMAC-SHA256(credential secret, salt)` — deterministically,
//! for the same credential and salt, and never revealing the secret itself.
//! Feed it a fixed random salt and you get 32 stable bytes that exist nowhere
//! but inside the authenticator.
//!
//! Those 32 bytes are a wrap key. Superbackup seals the master passphrase
//! under it, exactly as it does for the platform keychain, and the sealed file
//! is worthless without the physical authenticator.
//!
//! This is the same mechanism `systemd-cryptenroll --fido2-device` uses to
//! unlock a LUKS volume, and the same one the WebAuthn `prf` extension exposes
//! to browsers. It is not a clever trick; it is the intended answer.
//!
//! # What this costs, said plainly
//!
//! Enrolling a passkey adds a second way into the vault, and a vault is only
//! ever as strong as its weakest door. Two rules follow, and neither is
//! optional:
//!
//! * **User verification is required, never merely preferred.** A key that
//!   unlocks on a touch alone turns a stolen or borrowed authenticator into a
//!   stolen backup. With verification required, the authenticator asks for its
//!   PIN or a fingerprint first, so it is something you have *and* something
//!   you know.
//! * **The passphrase always still works.** An authenticator can be lost,
//!   reset, or simply left at the office, and its credentials do not survive a
//!   factory reset. A passkey is an additional door, never a replacement one,
//!   because the alternative is a backup nobody can ever restore.
//!
//! # Where this runs
//!
//! In the process with a window. `WebAuthNAuthenticatorGetAssertion` takes an
//! `HWND` and puts the operating system's own prompt on top of it, so a
//! service running with no desktop cannot do this and must not try — the same
//! constraint that sends `ssh-add` to a terminal.

use crate::error::{Error, Result};
use crate::secret::Secret;

/// The length of an `hmac-secret` salt and of the value it returns.
///
/// Fixed by CTAP2 at one SHA-256 block. Not a parameter: an authenticator
/// refuses anything else.
pub const SALT_LEN: usize = 32;

/// The relying party this application's credentials belong to.
///
/// Credentials are scoped to it, so it is fixed for the life of the product:
/// changing it would orphan every passkey anybody has enrolled. It is not a
/// site that exists, and deliberately not a domain somebody else could come to
/// own.
pub const RELYING_PARTY: &str = "superbackup.local";

/// The window the operating system's prompt is parented to.
///
/// Opaque, and constructed only from a real window handle, so that nothing can
/// reach the WebAuthn calls from a process that has no desktop to show a
/// prompt on. On platforms with no implementation it holds nothing.
#[derive(Debug, Clone, Copy)]
pub struct Window(#[allow(dead_code)] isize);

impl Window {
    /// Wrap a native window handle.
    ///
    /// # Safety
    ///
    /// `handle` must be a valid window handle belonging to this process, or
    /// zero to let the platform choose the foreground window.
    pub unsafe fn from_raw(handle: isize) -> Window {
        Window(handle)
    }

    /// Whatever window is in front, which for a single-window application is
    /// this one. Used by the command line, which has no window of its own.
    pub fn foreground() -> Window {
        platform_impl::foreground()
    }
}

/// What this machine can actually do, asked before anything is offered.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Support {
    /// The platform has a WebAuthn implementation at all.
    pub available: bool,
    /// It is new enough to pass a salt through to the authenticator.
    ///
    /// Without this, a passkey can prove who you are and cannot produce a key,
    /// which for this purpose is the same as not being there.
    pub hmac_secret: bool,
    /// A built-in authenticator with user verification — Windows Hello, Touch
    /// ID — is present. A security key on USB works whether or not this is
    /// true.
    pub platform_authenticator: bool,
}

impl Support {
    /// Can a passkey be enrolled here?
    pub fn usable(&self) -> bool {
        self.available && self.hmac_secret
    }

    /// Why not, in a sentence for the interface.
    pub fn why_not(&self) -> Option<&'static str> {
        if !self.available {
            return Some(
                "This computer has no passkey support that superbackup can use. Your master \
                 passphrase still works.",
            );
        }
        if !self.hmac_secret {
            return Some(
                "This version of Windows can sign in with a passkey but cannot derive a key from \
                 one, which is what unlocking needs. Your master passphrase still works.",
            );
        }
        None
    }
}

/// What this machine can do. Cheap; safe to call while drawing a settings page.
pub fn support() -> Support {
    platform_impl::support()
}

/// A credential that has been created but not yet used for anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credential {
    /// The authenticator's own handle for it. Not secret: it identifies the
    /// credential to ask for and reveals nothing without the authenticator.
    pub id: Vec<u8>,
}

/// Create a passkey for superbackup on whatever authenticator the user picks.
///
/// The operating system runs the choosing: which authenticator, and the PIN or
/// fingerprint that verifies the person holding it. Superbackup sees the
/// credential's public identifier and nothing else.
///
/// `label` is what the user will see beside it afterwards; it is also handed to
/// the authenticator, so a security key that lists its credentials shows
/// something recognisable rather than a hexadecimal blob.
pub fn enrol(window: Window, label: &str) -> Result<Credential> {
    platform_impl::enrol(window, label)
}

/// Ask an enrolled credential for the key it derives from `salt`.
///
/// Prompts: the authenticator asks for its PIN or a fingerprint, every time.
/// The same credential and salt always produce the same 32 bytes, which is
/// what makes this a key rather than a login.
///
/// Fails rather than falls back if the authenticator is absent, refuses, or is
/// too old for the extension — an unlock path that quietly degrades is one
/// nobody can reason about.
pub fn derive(window: Window, credential: &Credential, salt: &[u8; SALT_LEN]) -> Result<Secret> {
    platform_impl::derive(window, credential, salt)
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod platform_impl {
    use super::*;

    use windows::core::PCWSTR;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Networking::WindowsWebServices::*;
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

    /// `pHmacSecretSaltValues` appeared in version 5 of the assertion options.
    /// Below that the field this whole module needs does not exist.
    const HMAC_SECRET_OPTIONS_VERSION: u32 = WEBAUTHN_AUTHENTICATOR_GET_ASSERTION_OPTIONS_VERSION_5;

    /// The API version that first carried the assertion-time salt.
    const HMAC_SECRET_API_VERSION: u32 = 4;

    pub fn foreground() -> Window {
        // SAFETY: no arguments, no failure mode; returns null when there is no
        // foreground window, which the API accepts.
        Window(unsafe { GetForegroundWindow() }.0 as isize)
    }

    pub fn support() -> Support {
        // SAFETY: no arguments. Returns 0 when webauthn.dll is too old to have
        // the function, which is itself the answer.
        let version = unsafe { WebAuthNGetApiVersionNumber() };
        if version == 0 {
            return Support::default();
        }
        // SAFETY: writes one BOOL through the pointer the binding supplies.
        let platform = unsafe { WebAuthNIsUserVerifyingPlatformAuthenticatorAvailable() }
            .map(|b| b.as_bool())
            .unwrap_or(false);
        Support {
            available: true,
            hmac_secret: version >= HMAC_SECRET_API_VERSION,
            platform_authenticator: platform,
        }
    }

    pub fn enrol(window: Window, label: &str) -> Result<Credential> {
        let support = support();
        if !support.usable() {
            return Err(Error::Platform(
                support.why_not().unwrap_or("passkeys are not usable here").to_string(),
            ));
        }

        let rp_id = wide(RELYING_PARTY);
        let rp_name = wide("superbackup");
        let rp = WEBAUTHN_RP_ENTITY_INFORMATION {
            dwVersion: WEBAUTHN_RP_ENTITY_INFORMATION_CURRENT_VERSION,
            pwszId: PCWSTR(rp_id.as_ptr()),
            pwszName: PCWSTR(rp_name.as_ptr()),
            pwszIcon: PCWSTR::null(),
        };

        // The user handle. Random rather than derived from the account name:
        // it is written into the authenticator and, on a resident credential,
        // can be read back by anything that can talk to the key. There is
        // nothing to gain by putting a person's name there.
        let mut user_id = crate::crypto::random_bytes(32)?;
        let user_name = wide(label);
        let user = WEBAUTHN_USER_ENTITY_INFORMATION {
            dwVersion: WEBAUTHN_USER_ENTITY_INFORMATION_CURRENT_VERSION,
            cbId: user_id.len() as u32,
            pbId: user_id.as_mut_ptr(),
            pwszName: PCWSTR(user_name.as_ptr()),
            pwszIcon: PCWSTR::null(),
            pwszDisplayName: PCWSTR(user_name.as_ptr()),
        };

        let algorithm = WEBAUTHN_COSE_CREDENTIAL_PARAMETER {
            dwVersion: WEBAUTHN_COSE_CREDENTIAL_PARAMETER_CURRENT_VERSION,
            pwszCredentialType: WEBAUTHN_CREDENTIAL_TYPE_PUBLIC_KEY,
            lAlg: WEBAUTHN_COSE_ALGORITHM_ECDSA_P256_WITH_SHA256,
        };
        let mut algorithms = [algorithm];
        let parameters = WEBAUTHN_COSE_CREDENTIAL_PARAMETERS {
            cCredentialParameters: algorithms.len() as u32,
            pCredentialParameters: algorithms.as_mut_ptr(),
        };

        let mut client_data_json = client_data("webauthn.create")?;
        let client_data = WEBAUTHN_CLIENT_DATA {
            dwVersion: WEBAUTHN_CLIENT_DATA_CURRENT_VERSION,
            cbClientDataJSON: client_data_json.len() as u32,
            pbClientDataJSON: client_data_json.as_mut_ptr(),
            pwszHashAlgId: WEBAUTHN_HASH_ALGORITHM_SHA_256,
        };

        // Ask for the extension at creation. An authenticator that does not
        // implement it still makes the credential, so this is checked again
        // when the key is first derived rather than trusted here.
        let mut enable = windows::core::BOOL::from(true);
        let mut extensions = [WEBAUTHN_EXTENSION {
            pwszExtensionIdentifier: WEBAUTHN_EXTENSIONS_IDENTIFIER_HMAC_SECRET,
            cbExtension: std::mem::size_of::<windows::core::BOOL>() as u32,
            pvExtension: (&mut enable) as *mut _ as *mut std::ffi::c_void,
        }];

        let mut options = WEBAUTHN_AUTHENTICATOR_MAKE_CREDENTIAL_OPTIONS {
            dwVersion: WEBAUTHN_AUTHENTICATOR_MAKE_CREDENTIAL_OPTIONS_VERSION_3,
            dwTimeoutMilliseconds: TIMEOUT_MS,
            Extensions: WEBAUTHN_EXTENSIONS {
                cExtensions: extensions.len() as u32,
                pExtensions: extensions.as_mut_ptr(),
            },
            dwAuthenticatorAttachment: WEBAUTHN_AUTHENTICATOR_ATTACHMENT_ANY,
            // Required, not preferred. See the module documentation: without
            // it a stolen authenticator is a stolen backup.
            dwUserVerificationRequirement: WEBAUTHN_USER_VERIFICATION_REQUIREMENT_REQUIRED,
            // No attestation. Superbackup does not care which make of key this
            // is, and asking would send a hardware identifier somewhere for no
            // benefit.
            dwAttestationConveyancePreference: WEBAUTHN_ATTESTATION_CONVEYANCE_PREFERENCE_NONE,
            ..Default::default()
        };
        // A resident (discoverable) credential, so the key carries it and the
        // credential id in superbackup's own file is a convenience rather than
        // the only copy.
        options.bRequireResidentKey = windows::core::BOOL::from(true);

        // SAFETY: every pointer above outlives this call, and the result is
        // freed below on both paths.
        let attestation = unsafe {
            WebAuthNAuthenticatorMakeCredential(
                HWND(window.0 as *mut std::ffi::c_void),
                &rp,
                &user,
                &parameters,
                &client_data,
                Some(&options),
            )
        }
        .map_err(|e| Error::Platform(describe("creating the passkey", &e)))?;

        // SAFETY: `attestation` is the pointer just returned and is not null
        // on success.
        let credential = unsafe {
            let a = &*attestation;
            Credential {
                id: std::slice::from_raw_parts(a.pbCredentialId, a.cbCredentialId as usize)
                    .to_vec(),
            }
        };
        // SAFETY: freeing exactly what the API allocated, once.
        unsafe { WebAuthNFreeCredentialAttestation(Some(attestation)) };

        if credential.id.is_empty() {
            return Err(Error::Platform(
                "the authenticator produced a passkey with no identifier, which superbackup \
                 cannot ask for again"
                    .into(),
            ));
        }
        Ok(credential)
    }

    pub fn derive(
        window: Window,
        credential: &Credential,
        salt: &[u8; SALT_LEN],
    ) -> Result<Secret> {
        let rp_id = wide(RELYING_PARTY);

        let mut credential_id = credential.id.clone();
        let mut allowed = [WEBAUTHN_CREDENTIAL {
            dwVersion: WEBAUTHN_CREDENTIAL_CURRENT_VERSION,
            cbId: credential_id.len() as u32,
            pbId: credential_id.as_mut_ptr(),
            pwszCredentialType: WEBAUTHN_CREDENTIAL_TYPE_PUBLIC_KEY,
        }];

        let mut client_data_json = client_data("webauthn.get")?;
        let client_data = WEBAUTHN_CLIENT_DATA {
            dwVersion: WEBAUTHN_CLIENT_DATA_CURRENT_VERSION,
            cbClientDataJSON: client_data_json.len() as u32,
            pbClientDataJSON: client_data_json.as_mut_ptr(),
            pwszHashAlgId: WEBAUTHN_HASH_ALGORITHM_SHA_256,
        };

        let mut salt_bytes = *salt;
        let mut hmac_salt = WEBAUTHN_HMAC_SECRET_SALT {
            cbFirst: salt_bytes.len() as u32,
            pbFirst: salt_bytes.as_mut_ptr(),
            cbSecond: 0,
            pbSecond: std::ptr::null_mut(),
        };
        let mut salt_values = WEBAUTHN_HMAC_SECRET_SALT_VALUES {
            pGlobalHmacSalt: &mut hmac_salt,
            cCredWithHmacSecretSaltList: 0,
            pCredWithHmacSecretSaltList: std::ptr::null_mut(),
        };

        let options = WEBAUTHN_AUTHENTICATOR_GET_ASSERTION_OPTIONS {
            dwVersion: HMAC_SECRET_OPTIONS_VERSION,
            dwTimeoutMilliseconds: TIMEOUT_MS,
            CredentialList: WEBAUTHN_CREDENTIALS {
                cCredentials: allowed.len() as u32,
                pCredentials: allowed.as_mut_ptr(),
            },
            dwAuthenticatorAttachment: WEBAUTHN_AUTHENTICATOR_ATTACHMENT_ANY,
            dwUserVerificationRequirement: WEBAUTHN_USER_VERIFICATION_REQUIREMENT_REQUIRED,
            pHmacSecretSaltValues: &mut salt_values,
            ..Default::default()
        };

        // SAFETY: every pointer above outlives this call, and the assertion is
        // freed below on both paths.
        let assertion = unsafe {
            WebAuthNAuthenticatorGetAssertion(
                HWND(window.0 as *mut std::ffi::c_void),
                PCWSTR(rp_id.as_ptr()),
                &client_data,
                Some(&options),
            )
        }
        .map_err(|e| Error::Platform(describe("using the passkey", &e)))?;

        // SAFETY: `assertion` is the pointer just returned and is not null on
        // success; `pHmacSecret` is null when the authenticator did not answer
        // the extension, which is checked before it is read.
        let derived = unsafe {
            let a = &*assertion;
            if a.pHmacSecret.is_null() {
                None
            } else {
                let s = &*a.pHmacSecret;
                if s.pbFirst.is_null() || s.cbFirst as usize != SALT_LEN {
                    None
                } else {
                    Some(std::slice::from_raw_parts(s.pbFirst, s.cbFirst as usize).to_vec())
                }
            }
        };
        // SAFETY: freeing exactly what the API allocated, once. Done before
        // the error return below, so a refusal does not also leak.
        unsafe { WebAuthNFreeAssertion(assertion) };

        match derived {
            Some(bytes) => Ok(Secret::new(bytes)),
            None => Err(Error::Platform(
                "This passkey cannot derive a key. That usually means the authenticator does not \
                 support the hmac-secret extension — most security keys do, and the built-in one \
                 on some versions of Windows does not. Your master passphrase still works."
                    .into(),
            )),
        }
    }

    /// Long enough for somebody to find their security key and their PIN;
    /// short enough that a prompt nobody answers does not sit there for ever.
    const TIMEOUT_MS: u32 = 60_000;

    fn wide(value: &str) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        std::ffi::OsStr::new(value).encode_wide().chain(std::iter::once(0)).collect()
    }

    /// The client data the assertion signs over.
    ///
    /// Superbackup never checks the signature — what it wants is the extension
    /// output — but the challenge is fresh anyway, because a fixed challenge
    /// is the sort of thing that is fine until the day somebody starts
    /// checking.
    fn client_data(kind: &str) -> Result<Vec<u8>> {
        use base64::Engine;
        let challenge = crate::crypto::random_bytes(32)?;
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&challenge);
        Ok(format!(
            "{{\"type\":\"{kind}\",\"challenge\":\"{encoded}\",\"origin\":\"{RELYING_PARTY}\"}}"
        )
        .into_bytes())
    }

    /// The operating system's own words, plus what they mean here.
    fn describe(action: &str, error: &windows::core::Error) -> String {
        // `NTE_NOT_FOUND` and friends come back as HRESULTs whose text is
        // accurate but not helpful on its own.
        let code = error.code().0 as u32;
        let extra = match code {
            // The user closed the prompt or refused the fingerprint.
            0x8009_0021 => " The prompt was cancelled.",
            // No authenticator answered.
            0x8009_0020 => " No authenticator responded in time.",
            _ => "",
        };
        format!("{action} failed: {}{extra}", error.message())
    }
}

// ---------------------------------------------------------------------------
// Everywhere else
// ---------------------------------------------------------------------------
//
// Not "not yet": macOS and Linux both have real ways to do this — Touch ID
// through the Local Authentication framework, and CTAP2 over USB HID through
// libfido2 — and neither is a small piece of work. Windows is the stated
// priority, so this is the honest shape until one of them is written: the
// interface asks `support()` and offers nothing when the answer is no.

#[cfg(not(windows))]
mod platform_impl {
    use super::*;

    pub fn foreground() -> Window {
        Window(0)
    }

    pub fn support() -> Support {
        Support::default()
    }

    pub fn enrol(_window: Window, _label: &str) -> Result<Credential> {
        Err(Error::Platform(unsupported()))
    }

    pub fn derive(
        _window: Window,
        _credential: &Credential,
        _salt: &[u8; SALT_LEN],
    ) -> Result<Secret> {
        Err(Error::Platform(unsupported()))
    }

    fn unsupported() -> String {
        "superbackup can only unlock with a passkey on Windows so far. Your master passphrase \
         works everywhere."
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asking must not panic, whatever this machine is or is not.
    ///
    /// The interface calls this while drawing, so it has to be safe to call on
    /// a build server with no authenticator, no desktop and no user.
    #[test]
    fn asking_what_is_supported_is_always_answerable() {
        let support = support();
        // The two must agree: something unusable has to say why, and something
        // usable must not.
        assert_eq!(support.usable(), support.why_not().is_none());
    }

    /// A machine that cannot derive a key must not be offered passkeys, even
    /// if it can sign in with one.
    ///
    /// This is the distinction the whole module rests on, so it is asserted
    /// rather than left to the reader of `usable`.
    #[test]
    fn signing_in_is_not_the_same_as_deriving_a_key() {
        let sign_in_only =
            Support { available: true, hmac_secret: false, platform_authenticator: true };
        assert!(!sign_in_only.usable());
        assert!(sign_in_only.why_not().is_some_and(|why| why.contains("passphrase")));

        let full = Support { available: true, hmac_secret: true, platform_authenticator: false };
        assert!(full.usable(), "a security key on USB is enough; a built-in one is not required");
    }

    /// Every refusal tells the user their passphrase still works.
    ///
    /// It is the one thing somebody staring at a failed unlock needs to know,
    /// and the moment they most need telling.
    #[test]
    fn every_refusal_says_the_passphrase_still_works() {
        for support in [
            Support::default(),
            Support { available: true, hmac_secret: false, platform_authenticator: false },
        ] {
            let why = support.why_not().expect("an unusable machine says why");
            assert!(why.contains("passphrase"), "{why}");
        }
    }

    /// The salt length is the one CTAP2 fixes. A different one is refused by
    /// the authenticator, so getting it wrong fails on hardware and nowhere
    /// else.
    #[test]
    fn the_salt_is_one_sha256_block() {
        assert_eq!(SALT_LEN, 32);
    }
}
