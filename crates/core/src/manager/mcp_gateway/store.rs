//! The daemon is the sole owner of gateway grants. One Keychain item per resource and
//! data-directory instance; a Keychain replacement commits the complete rotated grant.
use std::{io, path::Path};

use serde::{Deserialize, Serialize};

#[cfg(windows)]
#[path = "windows_store.rs"]
mod windows;
#[cfg(windows)]
pub(super) use windows::WindowsStore;

#[derive(Clone, Serialize, Deserialize)]
pub struct Grant {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: u64,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub token_endpoint: String,
    pub revocation_endpoint: Option<String>,
}

/// Implementations must replace each resource's entire value atomically. Never log data.
pub trait TokenStore: Send + Sync {
    fn load(&self, resource: &str) -> io::Result<Option<Grant>>;
    fn save(&self, resource: &str, grant: &Grant) -> io::Result<()>;
    fn delete(&self, resource: &str) -> io::Result<()>;
}

pub struct KeychainStore {
    service: String,
    #[cfg(all(windows, not(test)))]
    windows: WindowsStore,
}
impl KeychainStore {
    pub fn new(data_dir: &Path) -> Self {
        let instance = std::fs::canonicalize(data_dir).unwrap_or_else(|_| data_dir.to_owned());
        Self {
            #[cfg(all(windows, not(test)))]
            windows: WindowsStore::new(data_dir),
            service: format!(
                "ai.brigadier.mcp-gateway.{}",
                blake3::hash(instance.as_os_str().as_encoded_bytes()).to_hex()
            ),
        }
    }
    pub fn service_name(&self) -> &str {
        &self.service
    }
}

#[cfg(all(target_os = "macos", not(test)))]
impl TokenStore for KeychainStore {
    fn load(&self, resource: &str) -> io::Result<Option<Grant>> {
        match security_framework::passwords::get_generic_password(&self.service, resource) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|_| io::Error::other("Invalid gateway grant")),
            Err(error) if error.code() == -25300 => Ok(None),
            Err(_) => Err(io::Error::other("Gateway Keychain read failed")),
        }
    }
    fn save(&self, resource: &str, grant: &Grant) -> io::Result<()> {
        let bytes = serde_json::to_vec(grant)
            .map_err(|_| io::Error::other("Cannot encode gateway grant"))?;
        security_framework::passwords::set_generic_password(&self.service, resource, &bytes)
            .map_err(|_| io::Error::other("Gateway Keychain write failed"))
    }
    fn delete(&self, resource: &str) -> io::Result<()> {
        match security_framework::passwords::delete_generic_password(&self.service, resource) {
            Ok(()) => Ok(()),
            Err(error) if error.code() == -25300 => Ok(()),
            Err(_) => Err(io::Error::other("Gateway Keychain delete failed")),
        }
    }
}

#[cfg(all(windows, not(test)))]
impl TokenStore for KeychainStore {
    fn load(&self, resource: &str) -> io::Result<Option<Grant>> {
        self.windows.load(resource)
    }
    fn save(&self, resource: &str, grant: &Grant) -> io::Result<()> {
        self.windows.save(resource, grant)
    }
    fn delete(&self, resource: &str) -> io::Result<()> {
        self.windows.delete(resource)
    }
}

#[cfg(any(all(not(target_os = "macos"), not(windows)), test))]
impl TokenStore for KeychainStore {
    fn load(&self, _: &str) -> io::Result<Option<Grant>> {
        Ok(None)
    }
    fn save(&self, _: &str, _: &Grant) -> io::Result<()> {
        Err(io::Error::other("Gateway Keychain unavailable"))
    }
    fn delete(&self, _: &str) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
#[derive(Default)]
pub struct MemoryStore(pub std::sync::Mutex<std::collections::HashMap<String, Grant>>);
#[cfg(test)]
impl TokenStore for MemoryStore {
    fn load(&self, resource: &str) -> io::Result<Option<Grant>> {
        Ok(self.0.lock().unwrap().get(resource).cloned())
    }
    fn save(&self, resource: &str, grant: &Grant) -> io::Result<()> {
        self.0
            .lock()
            .unwrap()
            .insert(resource.into(), grant.clone());
        Ok(())
    }
    fn delete(&self, resource: &str) -> io::Result<()> {
        self.0.lock().unwrap().remove(resource);
        Ok(())
    }
}
