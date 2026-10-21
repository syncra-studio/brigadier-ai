//! The system's Keychain, behind a trait so tests never touch the user's.

use zeroize::Zeroizing;

/// Why a store couldn't be read or written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StoreError {
    /// The user said no to the system's prompt.
    Denied,
    /// Reading it now would ask the user (a quiet read), or another read is asking them.
    Busy,
    /// There is no such store here, or it failed: the reason, never a value.
    Failed(String),
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
    /// Replaces an existing item's data, never making one.
    fn update(&self, service: &str, account: &str, data: &[u8]) -> Result<(), StoreError>;
    /// Makes or replaces one of Brigadier's own items.
    fn set(&self, service: &str, account: &str, data: &[u8]) -> Result<(), StoreError>;
}

/// The login Keychain (macOS).
#[cfg(target_os = "macos")]
#[cfg_attr(test, allow(dead_code))]
pub(crate) struct System;

#[cfg(target_os = "macos")]
#[cfg_attr(test, allow(dead_code))]
mod mac {
    use std::sync::Mutex;

    use security_framework::item::{
        ItemClass, ItemSearchOptions, ItemUpdateOptions, ItemUpdateValue, Limit, update_item,
    };
    use security_framework::os::macos::keychain::SecKeychain;
    use security_framework::passwords;
    use zeroize::Zeroizing;

    use super::{Keychain, StoreError, System};

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

        fn update(&self, service: &str, account: &str, data: &[u8]) -> Result<(), StoreError> {
            let _asking = PROMPTS.lock().unwrap_or_else(|p| p.into_inner());
            let mut update = ItemUpdateOptions::new();
            update.set_value(ItemUpdateValue::Data(
                core_foundation::data::CFData::from_buffer(data),
            ));
            update_item(&search(service, account), &update).map_err(|err| error(err.code()))
        }

        fn set(&self, service: &str, account: &str, data: &[u8]) -> Result<(), StoreError> {
            let _asking = PROMPTS.lock().unwrap_or_else(|p| p.into_inner());
            passwords::set_generic_password(service, account, data).map_err(|err| error(err.code()))
        }
    }
}

/// No Keychain on this system: every store in it is unknown.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub(crate) struct Missing;

impl Keychain for Missing {
    fn get(&self, _: &str, _: &str, _: bool) -> Result<Option<Zeroizing<Vec<u8>>>, StoreError> {
        Err(StoreError::Failed("there is no Keychain here".into()))
    }

    fn exists(&self, _: &str, _: &str) -> Result<bool, StoreError> {
        Err(StoreError::Failed("there is no Keychain here".into()))
    }

    fn update(&self, _: &str, _: &str, _: &[u8]) -> Result<(), StoreError> {
        Err(StoreError::Failed("there is no Keychain here".into()))
    }

    fn set(&self, _: &str, _: &str, _: &[u8]) -> Result<(), StoreError> {
        Err(StoreError::Failed("there is no Keychain here".into()))
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

    fn update(&self, service: &str, account: &str, data: &[u8]) -> Result<(), StoreError> {
        let mut items = self.items.lock().unwrap();
        match items.get_mut(&(service.into(), account.into())) {
            Some(item) => {
                *item = data.to_vec();
                Ok(())
            }
            None => Err(StoreError::Failed("no such item".into())),
        }
    }

    fn set(&self, service: &str, account: &str, data: &[u8]) -> Result<(), StoreError> {
        self.put(service, account, data);
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
