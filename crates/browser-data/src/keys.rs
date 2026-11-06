//! The keys that open Chromium's encrypted cookies, on each OS.
//!
//! | Prefix | macOS | Windows | Linux |
//! |---|---|---|---|
//! | `v10` | AES-128-CBC, key from the Keychain password | AES-256-GCM, key from `Local State`, sealed with DPAPI | AES-128-CBC, key from the fixed password "peanuts" |
//! | `v11` | as `v10` | not used | AES-128-CBC, key from the keyring's password |
//! | `v20` | not used | App-Bound: never opened | not used |
//!
//! Chrome's `v20` App-Bound cookies are locked to the browser itself. This crate never tries to
//! open them, and never reads `os_crypt.app_bound_encrypted_key`: reaching that key means posing
//! as the browser, which is how cookie-stealing malware works. They count as unreadable, with
//! their domains named.
//!
//! The Linux logic ([`ChromiumKeys::from_linux_secret`] and the empty-password fallback in
//! `chromium.rs`) is ported from bb, `apps/desktop/src/browser-import/chromium-keys.ts` and
//! `chromium-cookies.ts` (<https://github.com/get-bb/bb>), under this notice:
//!
//! > MIT License
//! >
//! > Copyright (c) 2026 Michael Yong
//! >
//! > Permission is hereby granted, free of charge, to any person obtaining a copy of this software
//! > and associated documentation files (the "Software"), to deal in the Software without
//! > restriction, including without limitation the rights to use, copy, modify, merge, publish,
//! > distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the
//! > Software is furnished to do so, subject to the following conditions:
//! >
//! > The above copyright notice and this permission notice shall be included in all copies or
//! > substantial portions of the Software.
//! >
//! > THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING
//! > BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
//! > NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM,
//! > DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
//! > OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

use aes::cipher::block_padding::Pkcs7;
use aes::cipher::{BlockModeEncrypt, KeyIvInit};
use aes_gcm::aead::{Aead, AeadCore, OsRng};
use aes_gcm::{Aes256Gcm, KeyInit};

use crate::{Error, Profile, SecretError};

/// PBKDF2 salt Chromium uses on macOS and Linux.
const SALT: &[u8] = b"saltysalt";
/// PBKDF2 iterations Chromium uses on macOS.
const MACOS_ITERATIONS: u32 = 1003;
/// PBKDF2 iterations Chromium uses on Linux.
const LINUX_ITERATIONS: u32 = 1;
/// The password Chromium on Linux uses for every `v10` value.
const LINUX_V10_PASSWORD: &[u8] = b"peanuts";

fn derive(password: &[u8], iterations: u32) -> [u8; 16] {
    pbkdf2::pbkdf2_hmac_array::<sha1::Sha1, 16>(password, SALT, iterations)
}

/// What opens a Chromium browser's cookies. Holds secrets, so its `Debug` shows none.
#[derive(Clone, Default)]
pub struct ChromiumKeys {
    pub(crate) cbc_v10: Option<[u8; 16]>,
    pub(crate) cbc_v11: Option<[u8; 16]>,
    /// Linux: Chromium seals with the empty password when it can't reach a keyring.
    pub(crate) cbc_empty: Option<[u8; 16]>,
    pub(crate) gcm_v10: Option<[u8; 32]>,
    /// Why `cbc_v11` is missing, for telling the user.
    v11_failure: Option<SecretError>,
}

impl std::fmt::Debug for ChromiumKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChromiumKeys")
            .field("cbc_v10", &self.cbc_v10.is_some())
            .field("cbc_v11", &self.cbc_v11.is_some())
            .field("cbc_empty", &self.cbc_empty.is_some())
            .field("gcm_v10", &self.gcm_v10.is_some())
            .field("v11_failure", &self.v11_failure)
            .finish()
    }
}

impl ChromiumKeys {
    /// Keys from the browser's "Safe Storage" password in the macOS Keychain.
    pub fn from_macos_password(password: &[u8]) -> Self {
        let key = derive(password, MACOS_ITERATIONS);
        Self {
            cbc_v10: Some(key),
            cbc_v11: Some(key),
            ..Self::default()
        }
    }

    /// Keys from the unsealed master key in Windows' `Local State`. `v20` values stay closed.
    pub fn from_windows_key(key: [u8; 32]) -> Self {
        Self {
            gcm_v10: Some(key),
            ..Self::default()
        }
    }

    /// Keys for Linux, from the keyring lookup of the browser's password. `v10` values never need
    /// the keyring, so a keyring that is absent or has no entry still gives keys, with
    /// [`v11_failure`](Self::v11_failure) saying why `v11` values can't open. One the user has to
    /// unlock or allow is an error, because they can fix it and try again.
    pub fn from_linux_secret(secret: Result<&[u8], SecretError>) -> Result<Self, Error> {
        let mut keys = Self {
            cbc_v10: Some(derive(LINUX_V10_PASSWORD, LINUX_ITERATIONS)),
            cbc_empty: Some(derive(b"", LINUX_ITERATIONS)),
            ..Self::default()
        };
        match secret {
            Ok(secret) => keys.cbc_v11 = Some(derive(secret, LINUX_ITERATIONS)),
            Err(failure @ (SecretError::Locked | SecretError::Denied)) => {
                return Err(Error::Secret(failure));
            }
            Err(failure) => keys.v11_failure = Some(failure),
        }
        Ok(keys)
    }

    /// Why this browser's `v11` values can't be opened, when the keyring wouldn't help.
    pub fn v11_failure(&self) -> Option<&SecretError> {
        self.v11_failure.as_ref()
    }
}

/// Fetches the keys that open `profile`'s cookies from wherever this OS keeps them. macOS may ask
/// the user to allow it; Windows and Linux never ask. Not meaningful for other browser families,
/// which don't encrypt cookies.
pub fn chromium_keys(profile: &Profile) -> Result<ChromiumKeys, Error> {
    #[cfg(target_os = "macos")]
    {
        let service = profile
            .keychain_service
            .as_deref()
            .ok_or(Error::Secret(SecretError::NotFound))?;
        let password = crate::chromium_password(service, profile.keychain_account.as_deref())
            .map_err(|err| match err {
                Error::Keychain(err) => Error::Secret(err.into()),
                other => other,
            })?;
        Ok(ChromiumKeys::from_macos_password(&password))
    }
    #[cfg(windows)]
    {
        let path = profile
            .local_state
            .as_deref()
            .ok_or(Error::Secret(SecretError::NotFound))?;
        let state = std::fs::read(path).map_err(|err| crate::sqlite::io_error(path, err))?;
        let key = dpapi::unprotect(&windows_sealed_key(&state)?)?;
        let key: [u8; 32] = key
            .try_into()
            .map_err(|_| Error::Format("Local State key isn't 32 bytes".to_owned()))?;
        Ok(ChromiumKeys::from_windows_key(key))
    }
    #[cfg(target_os = "linux")]
    {
        let Some(application) = profile
            .linux_secret_application
            .as_deref()
            .filter(|_| !crate::key_off_limits(profile))
        else {
            return ChromiumKeys::from_linux_secret(Err(SecretError::NotFound));
        };
        let secret = crate::secret_service::lookup(application);
        ChromiumKeys::from_linux_secret(secret.as_deref().map_err(Clone::clone))
    }
    #[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
    {
        let _ = profile;
        Err(Error::Secret(SecretError::NoStore))
    }
}

/// The DPAPI-sealed master key from a `Local State` file: `os_crypt.encrypted_key`, base64 of
/// `DPAPI` and the sealed bytes. A browser with none has nothing this crate can open.
#[cfg(any(windows, test))]
pub(crate) fn windows_sealed_key(local_state: &[u8]) -> Result<Vec<u8>, Error> {
    use base64::Engine;

    let state: serde_json::Value = serde_json::from_slice(local_state)
        .map_err(|err| Error::Format(format!("Local State: {err}")))?;
    let encoded = state
        .pointer("/os_crypt/encrypted_key")
        .and_then(serde_json::Value::as_str)
        .ok_or(Error::Secret(SecretError::NotFound))?;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|err| Error::Format(format!("Local State key: {err}")))?;
    decoded
        .strip_prefix(b"DPAPI")
        .map(<[u8]>::to_vec)
        .ok_or_else(|| Error::Format("Local State key has no DPAPI prefix".to_owned()))
}

/// Windows' Data Protection API, which seals data to the signed-in user.
#[cfg(windows)]
#[allow(unsafe_code)]
pub(crate) mod dpapi {
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, GetLastError, LocalFree};
    use windows_sys::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    };

    use crate::{Error, SecretError};

    /// DPAPI's answer when the key was sealed for another user or machine.
    const NTE_BAD_KEY_STATE: u32 = 0x8009_000B;

    /// Seals data for this user. Refuses rather than showing any prompt.
    #[cfg(test)]
    pub(crate) fn protect(plain: &[u8]) -> Result<Vec<u8>, Error> {
        let too_long = || Error::Format("key too long".to_owned());
        let input = CRYPT_INTEGER_BLOB {
            cbData: u32::try_from(plain.len()).map_err(|_| too_long())?,
            pbData: plain.as_ptr().cast_mut(),
        };
        let mut output = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: null_mut(),
        };
        // SAFETY: `input` borrows `plain`, which outlives the call and isn't written through.
        // `output` is an out-parameter the call fills on success.
        let ok = unsafe {
            CryptProtectData(
                &raw const input,
                null(),
                null(),
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &raw mut output,
            )
        };
        if ok == 0 {
            // SAFETY: only reads this thread's last error.
            let code = unsafe { GetLastError() };
            return Err(Error::Secret(SecretError::Other(format!(
                "DPAPI error {code:#x}"
            ))));
        }
        // SAFETY: on success `output` points to `cbData` bytes that DPAPI allocated with
        // LocalAlloc; they are copied out, then freed once.
        let sealed = unsafe {
            let sealed = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
            LocalFree(output.pbData.cast());
            sealed
        };
        Ok(sealed)
    }

    /// Opens data sealed for this user. Refuses rather than showing any prompt.
    pub(crate) fn unprotect(sealed: &[u8]) -> Result<Vec<u8>, Error> {
        let too_long = || Error::Format("sealed key too long".to_owned());
        let input = CRYPT_INTEGER_BLOB {
            cbData: u32::try_from(sealed.len()).map_err(|_| too_long())?,
            pbData: sealed.as_ptr().cast_mut(),
        };
        let mut output = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: null_mut(),
        };
        // SAFETY: `input` borrows `sealed`, which outlives the call and isn't written through.
        // `output` is an out-parameter the call fills on success.
        let ok = unsafe {
            CryptUnprotectData(
                &raw const input,
                null_mut(),
                null(),
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &raw mut output,
            )
        };
        if ok == 0 {
            // SAFETY: only reads this thread's last error.
            let code = unsafe { GetLastError() };
            return Err(Error::Secret(match code {
                ERROR_ACCESS_DENIED | NTE_BAD_KEY_STATE => SecretError::Denied,
                code => SecretError::Other(format!("DPAPI error {code:#x}")),
            }));
        }
        // SAFETY: on success `output` points to `cbData` bytes that DPAPI allocated with
        // LocalAlloc; they are copied out, then freed once.
        let plain = unsafe {
            let plain = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
            LocalFree(output.pbData.cast());
            plain
        };
        Ok(plain)
    }
}

#[cfg(test)]
mod tests {
    use base64::Engine;

    use super::*;

    fn state(key: &[u8]) -> Vec<u8> {
        let encoded = base64::engine::general_purpose::STANDARD.encode(key);
        format!(r#"{{"os_crypt":{{"encrypted_key":"{encoded}"}}}}"#).into_bytes()
    }

    #[test]
    fn the_sealed_key_is_found_and_its_prefix_removed() {
        assert_eq!(
            windows_sealed_key(&state(b"DPAPIsealed")).unwrap(),
            b"sealed"
        );
    }

    #[test]
    fn a_bad_local_state_is_told_apart_from_a_missing_key() {
        assert!(matches!(
            windows_sealed_key(&state(b"sealed")),
            Err(Error::Format(_))
        ));
        assert!(matches!(
            windows_sealed_key(br#"{"os_crypt":{"encrypted_key":"***"}}"#),
            Err(Error::Format(_))
        ));
        assert!(matches!(windows_sealed_key(b"nope"), Err(Error::Format(_))));
        assert!(matches!(
            windows_sealed_key(b"{}"),
            Err(Error::Secret(SecretError::NotFound))
        ));
    }

    #[test]
    fn the_app_bound_key_is_never_taken_for_the_master_key() {
        let state = br#"{"os_crypt":{"app_bound_encrypted_key":"QVBQQnNlYWxlZA=="}}"#;
        assert!(matches!(
            windows_sealed_key(state),
            Err(Error::Secret(SecretError::NotFound))
        ));
    }

    #[test]
    fn linux_keyring_states() {
        // A usable keyring opens both prefixes.
        let keys = ChromiumKeys::from_linux_secret(Ok(b"secret")).unwrap();
        assert!(keys.cbc_v10.is_some() && keys.cbc_v11.is_some() && keys.v11_failure().is_none());
        // No keyring, or no entry: v10 still works and the reason is kept.
        for failure in [SecretError::NoStore, SecretError::NotFound] {
            let keys = ChromiumKeys::from_linux_secret(Err(failure.clone())).unwrap();
            assert!(keys.cbc_v10.is_some() && keys.cbc_v11.is_none());
            assert_eq!(keys.v11_failure(), Some(&failure));
        }
        // Locked or refused: the user can fix that, so it is an error.
        for failure in [SecretError::Locked, SecretError::Denied] {
            assert!(matches!(
                ChromiumKeys::from_linux_secret(Err(failure.clone())),
                Err(Error::Secret(found)) if found == failure
            ));
        }
    }

    /// Stands in for DPAPI: only `unseal` undoes it.
    #[test]
    fn keys_are_hidden_from_debug_output() {
        let shown = format!("{:?}", ChromiumKeys::from_windows_key([7; 32]));
        assert!(shown.contains("gcm_v10: true"));
        assert!(!shown.contains('7'));
    }

    /// Runs only on Windows: DPAPI itself, from a fixture `Local State` to usable keys.
    #[cfg(windows)]
    #[test]
    fn dpapi_opens_a_key_from_local_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Local State");
        let mut blob = b"DPAPI".to_vec();
        blob.extend(dpapi::protect(&[9; 32]).unwrap());
        std::fs::write(&path, state(&blob)).unwrap();
        let profile = Profile {
            id: "chrome/Default".to_owned(),
            browser: "Google Chrome".to_owned(),
            name: "Default".to_owned(),
            family: crate::Family::Chromium,
            dir: dir.path().to_owned(),
            app_path: None,
            keychain_service: None,
            keychain_account: None,
            local_state: Some(path),
            linux_secret_application: None,
            has_cookies: true,
            has_history: false,
            has_bookmarks: false,
            readable: true,
            is_default: true,
        };
        assert_eq!(chromium_keys(&profile).unwrap().gcm_v10, Some([9; 32]));
    }

    /// Runs only on Windows: data sealed for someone else is refused, without any prompt.
    #[cfg(windows)]
    #[test]
    fn dpapi_rejects_garbage() {
        assert!(matches!(
            dpapi::unprotect(b"not sealed"),
            Err(Error::Secret(_))
        ));
    }
}
