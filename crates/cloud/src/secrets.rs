//! Where credentials are kept: the system's secret store (the macOS Keychain, Windows Credential
//! Manager, the Linux kernel keyring), never the preferences file. [`MemoryStore`] keeps them for
//! the session only (tests, and where no secret store is available).

use std::collections::HashMap;
use std::sync::Mutex;

/// The keychain service name DLightroom's secrets are filed under.
pub const SERVICE: &str = "DLightroom";

/// A place to keep secrets by name.
pub trait SecretStore: Send + Sync {
    fn get(&self, name: &str) -> Result<Option<String>, String>;
    fn set(&self, name: &str, value: &str) -> Result<(), String>;
    fn delete(&self, name: &str) -> Result<(), String>;
    /// Where the secrets go, for the UI ("macOS Keychain").
    fn describe(&self) -> &'static str;
}

/// The system's secret store, under [`SERVICE`].
#[cfg(not(target_arch = "wasm32"))]
pub struct KeychainStore;

#[cfg(not(target_arch = "wasm32"))]
impl KeychainStore {
    fn entry(name: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new(SERVICE, name).map_err(|e| format!("the system keychain isn't available: {e}"))
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl SecretStore for KeychainStore {
    fn get(&self, name: &str) -> Result<Option<String>, String> {
        match Self::entry(name)?.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(format!("the system keychain couldn't be read: {e}")),
        }
    }

    fn set(&self, name: &str, value: &str) -> Result<(), String> {
        Self::entry(name)?.set_password(value).map_err(|e| format!("the system keychain couldn't store the credential: {e}"))
    }

    fn delete(&self, name: &str) -> Result<(), String> {
        match Self::entry(name)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(format!("the system keychain couldn't remove the credential: {e}")),
        }
    }

    fn describe(&self) -> &'static str {
        if cfg!(target_os = "macos") {
            "macOS Keychain"
        } else if cfg!(windows) {
            "Windows Credential Manager"
        } else {
            "the kernel keyring (kept until you log out)"
        }
    }
}

/// Secrets kept in memory for this session only.
#[derive(Default)]
pub struct MemoryStore(Mutex<HashMap<String, String>>);

impl SecretStore for MemoryStore {
    fn get(&self, name: &str) -> Result<Option<String>, String> {
        Ok(self.0.lock().map_err(|_| "secret store poisoned".to_string())?.get(name).cloned())
    }

    fn set(&self, name: &str, value: &str) -> Result<(), String> {
        self.0.lock().map_err(|_| "secret store poisoned".to_string())?.insert(name.into(), value.into());
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<(), String> {
        self.0.lock().map_err(|_| "secret store poisoned".to_string())?.remove(name);
        Ok(())
    }

    fn describe(&self) -> &'static str {
        "memory (this session only)"
    }
}

/// The store the desktop app uses: the system keychain; on the web, memory.
pub fn default_store() -> std::sync::Arc<dyn SecretStore> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::sync::Arc::new(KeychainStore)
    }
    #[cfg(target_arch = "wasm32")]
    {
        std::sync::Arc::new(MemoryStore::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_memory_store_keeps_and_forgets() {
        let s = MemoryStore::default();
        assert_eq!(s.get("k").unwrap(), None);
        s.set("k", "v").unwrap();
        assert_eq!(s.get("k").unwrap().as_deref(), Some("v"));
        s.delete("k").unwrap();
        s.delete("k").unwrap();
        assert_eq!(s.get("k").unwrap(), None);
    }
}
