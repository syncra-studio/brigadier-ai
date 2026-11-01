//! Per-user DPAPI encryption; only ciphertext ever reaches the filesystem.
#![allow(unsafe_code)]

use super::{Grant, TokenStore};
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    ptr,
};
use windows_sys::Win32::{
    Foundation::LocalFree,
    Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    },
};

pub(crate) struct WindowsStore {
    directory: PathBuf,
}

impl WindowsStore {
    pub(crate) fn new(data_dir: &Path) -> Self {
        Self {
            directory: data_dir.join("mcp-gateway-grants"),
        }
    }

    fn path(&self, resource: &str) -> PathBuf {
        self.directory.join(format!(
            "{}.dpapi",
            blake3::hash(resource.as_bytes()).to_hex()
        ))
    }
}

impl TokenStore for WindowsStore {
    fn load(&self, resource: &str) -> io::Result<Option<Grant>> {
        let encrypted = match fs::read(self.path(resource)) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let bytes = crypt(&encrypted, false)?;
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|_| io::Error::other("Invalid gateway grant"))
    }

    fn save(&self, resource: &str, grant: &Grant) -> io::Result<()> {
        let bytes = serde_json::to_vec(grant)
            .map_err(|_| io::Error::other("Cannot encode gateway grant"))?;
        let encrypted = crypt(&bytes, true)?;
        fs::create_dir_all(&self.directory)?;
        let temporary = self.directory.join(format!("{}.tmp", uuid::Uuid::new_v4()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        let result = (|| {
            file.write_all(&encrypted)?;
            file.sync_all()?;
            drop(file);
            // On Windows rename uses MoveFileExW with MOVEFILE_REPLACE_EXISTING.
            // Both paths are in the same directory, so readers see a complete grant.
            fs::rename(&temporary, self.path(resource))
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    fn delete(&self, resource: &str) -> io::Result<()> {
        match fs::remove_file(self.path(resource)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }
}

// DPAPI allocates output with LocalAlloc; free it even if copying unwinds.
struct Output(CRYPT_INTEGER_BLOB);
impl Drop for Output {
    fn drop(&mut self) {
        // SAFETY: DPAPI owns this allocation until transferred to us on success.
        unsafe { LocalFree(self.0.pbData.cast()) };
    }
}

fn crypt(bytes: &[u8], protect: bool) -> io::Result<Vec<u8>> {
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes
            .len()
            .try_into()
            .map_err(|_| io::Error::other("Gateway grant exceeds DPAPI input size"))?,
        pbData: bytes.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    // SAFETY: input references live bytes for the duration of the call. DPAPI does
    // not mutate input. Null optional parameters select the current user's scope;
    // no machine-wide flag is set and UI is forbidden for the daemon.
    let success = unsafe {
        if protect {
            CryptProtectData(
                &input,
                ptr::null(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                ptr::null_mut(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    };
    if success == 0 {
        return Err(io::Error::last_os_error());
    }
    let output = Output(output);
    if output.0.cbData == 0 {
        return Ok(Vec::new());
    }
    // SAFETY: successful DPAPI calls return cbData initialized bytes, owned by
    // output until its Drop runs after this copy.
    Ok(unsafe { std::slice::from_raw_parts(output.0.pbData, output.0.cbData as usize) }.to_vec())
}
