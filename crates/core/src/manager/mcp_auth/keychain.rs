//! The system's Keychain, behind a trait so tests never touch the user's.

use zeroize::Zeroizing;

#[cfg(any(test, target_os = "macos"))]
use super::stores::OWN_SERVICE;

/// Why a store couldn't be read or written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StoreError {
    /// The user said no to the system's prompt.
    #[cfg_attr(not(any(test, target_os = "macos")), allow(dead_code))]
    Denied,
    /// Reading it now would ask the user (a quiet read), or another read is asking them.
    #[cfg_attr(not(any(test, target_os = "macos")), allow(dead_code))]
    Busy,
    /// A store Brigadier can't read or write on this system.
    Unsupported,
    /// There is no such store here, or it failed: the reason, never a value.
    Failed(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Denied => f.write_str("macOS wasn't allowed to let Brigadier read it"),
            Self::Busy => f.write_str("the Keychain is asking the user"),
            Self::Unsupported => f.write_str("sign-ins can't be read or kept on this system"),
            Self::Failed(reason) => f.write_str(reason),
        }
    }
}

/// Generic passwords, by service and account.
pub(crate) trait Keychain: Send + Sync {
    /// An item's data; `None` when there is no such item. A `quiet` read never shows the
    /// system's prompt: it fails with [`StoreError::Busy`] instead.
    fn get(
        &self,
        service: &str,
        account: &str,
        quiet: bool,
    ) -> Result<Option<Zeroizing<Vec<u8>>>, StoreError>;
    /// Whether an item exists, from its attributes alone: never asks the user.
    fn exists(&self, service: &str, account: &str) -> Result<bool, StoreError>;
    /// Makes or replaces one of Brigadier's own items.
    fn set(&self, service: &str, account: &str, data: &[u8]) -> Result<(), StoreError>;
    /// Deletes Brigadier's own item `account` ([`OWN_SERVICE`]), if there is one: never an item
    /// of another service.
    fn remove_own(&self, account: &str) -> Result<(), StoreError>;
    /// Whether there is a Keychain here at all.
    fn supported(&self) -> bool {
        true
    }
}

/// The login Keychain (macOS).
#[cfg(target_os = "macos")]
#[cfg_attr(test, allow(dead_code))]
pub(crate) struct System;

#[cfg(target_os = "macos")]
#[cfg_attr(test, allow(dead_code))]
mod mac {
    use std::sync::Mutex;

    use security_framework::item::{ItemClass, ItemSearchOptions, Limit};
    use security_framework::os::macos::keychain::SecKeychain;
    use security_framework::passwords;
    use zeroize::Zeroizing;

    use super::{Keychain, OWN_SERVICE, StoreError, System};

    /// `errSecItemNotFound`.
    const NOT_FOUND: i32 = -25300;
    /// `errSecInteractionNotAllowed`: a quiet read that would have asked.
    const WOULD_ASK: i32 = -25308;
    /// `userCanceledErr` and `errSecAuthFailed`: the user said no.
    const DENIED: [i32; 2] = [-128, -25293];

    /// Turning the prompts off is for the whole process: a quiet read holds this while they are
    /// off, and a read that may ask waits for it, so neither runs inside the other.
    static PROMPTS: Mutex<()> = Mutex::new(());

    fn error(code: i32) -> StoreError {
        match code {
            WOULD_ASK => StoreError::Busy,
            code if DENIED.contains(&code) => StoreError::Denied,
            code => StoreError::Failed(format!("Keychain error {code}")),
        }
    }

    fn search(service: &str, account: &str) -> ItemSearchOptions {
        let mut search = ItemSearchOptions::new();
        search
            .class(ItemClass::generic_password())
            .service(service)
            .account(account);
        search
    }

    impl Keychain for System {
        fn get(
            &self,
            service: &str,
            account: &str,
            quiet: bool,
        ) -> Result<Option<Zeroizing<Vec<u8>>>, StoreError> {
            let _held = if quiet {
                let held = PROMPTS.try_lock().map_err(|_| StoreError::Busy)?;
                let off =
                    SecKeychain::disable_user_interaction().map_err(|err| error(err.code()))?;
                Some((held, off))
            } else {
                None
            };
            let _asking = (!quiet).then(|| PROMPTS.lock().unwrap_or_else(|p| p.into_inner()));
            match passwords::get_generic_password(service, account) {
                Ok(data) => Ok(Some(Zeroizing::new(data))),
                Err(err) if err.code() == NOT_FOUND => Ok(None),
                Err(err) => Err(error(err.code())),
            }
        }

        fn exists(&self, service: &str, account: &str) -> Result<bool, StoreError> {
            match search(service, account)
                .load_attributes(true)
                .limit(Limit::Max(1))
                .search()
            {
                Ok(items) => Ok(!items.is_empty()),
                Err(err) if err.code() == NOT_FOUND => Ok(false),
                Err(err) => Err(error(err.code())),
            }
        }

        fn set(&self, service: &str, account: &str, data: &[u8]) -> Result<(), StoreError> {
            let _asking = PROMPTS.lock().unwrap_or_else(|p| p.into_inner());
            passwords::set_generic_password(service, account, data).map_err(|err| error(err.code()))
        }

        fn remove_own(&self, account: &str) -> Result<(), StoreError> {
            let _asking = PROMPTS.lock().unwrap_or_else(|p| p.into_inner());
            match passwords::delete_generic_password(OWN_SERVICE, account) {
                Ok(()) => Ok(()),
                Err(err) if err.code() == NOT_FOUND => Ok(()),
                Err(err) => Err(error(err.code())),
            }
        }
    }
}

/// No Keychain on this system: no store in it can be read or kept.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub(crate) struct Missing;

impl Keychain for Missing {
    fn get(&self, _: &str, _: &str, _: bool) -> Result<Option<Zeroizing<Vec<u8>>>, StoreError> {
        Err(StoreError::Unsupported)
    }

    fn exists(&self, _: &str, _: &str) -> Result<bool, StoreError> {
        Err(StoreError::Unsupported)
    }

    fn set(&self, _: &str, _: &str, _: &[u8]) -> Result<(), StoreError> {
        Err(StoreError::Unsupported)
    }

    fn remove_own(&self, _: &str) -> Result<(), StoreError> {
        Err(StoreError::Unsupported)
    }

    fn supported(&self) -> bool {
        false
    }
}

/// Tests: a Keychain in memory. Items of other apps (`others`) ask the user on a read, which
/// a quiet read can't do: it fails with [`StoreError::Busy`], and a read that asks fails with
/// [`StoreError::Denied`] while `deny` is set.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct Memory {
    pub items: std::sync::Mutex<std::collections::HashMap<(String, String), Vec<u8>>>,
    pub others: std::sync::Mutex<std::collections::HashSet<String>>,
    pub deny: std::sync::atomic::AtomicBool,
    /// Each read's service and whether it was quiet.
    pub reads: std::sync::Mutex<Vec<(String, bool)>>,
}

#[cfg(test)]
impl Memory {
    pub(crate) fn put(&self, service: &str, account: &str, data: &[u8]) {
        self.items
            .lock()
            .unwrap()
            .insert((service.into(), account.into()), data.to_vec());
    }

    pub(crate) fn item(&self, service: &str, account: &str) -> Option<Vec<u8>> {
        self.items
            .lock()
            .unwrap()
            .get(&(service.into(), account.into()))
            .cloned()
    }
}

#[cfg(test)]
impl Keychain for Memory {
    fn get(
        &self,
        service: &str,
        account: &str,
        quiet: bool,
    ) -> Result<Option<Zeroizing<Vec<u8>>>, StoreError> {
        self.reads.lock().unwrap().push((service.into(), quiet));
        let found = self.item(service, account);
        if found.is_some() && self.others.lock().unwrap().contains(service) {
            if quiet {
                return Err(StoreError::Busy);
            }
            if self.deny.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(StoreError::Denied);
            }
        }
        Ok(found.map(Zeroizing::new))
    }

    fn exists(&self, service: &str, account: &str) -> Result<bool, StoreError> {
        Ok(self.item(service, account).is_some())
    }

    fn set(&self, service: &str, account: &str, data: &[u8]) -> Result<(), StoreError> {
        self.put(service, account, data);
        Ok(())
    }

    fn remove_own(&self, account: &str) -> Result<(), StoreError> {
        self.items
            .lock()
            .unwrap()
            .remove(&(OWN_SERVICE.into(), account.into()));
        Ok(())
    }
}

/// This system's Keychain.
#[cfg_attr(test, allow(dead_code))]
pub(crate) fn system() -> std::sync::Arc<dyn Keychain> {
    #[cfg(target_os = "macos")]
    return std::sync::Arc::new(System);
    #[cfg(not(target_os = "macos"))]
    return std::sync::Arc::new(Missing);
}
