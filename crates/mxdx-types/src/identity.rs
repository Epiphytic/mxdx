use anyhow::Result;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Device identity
// ---------------------------------------------------------------------------

/// Represents a (host, os_user, matrix_account) device identity
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeviceIdentity {
    pub device_id: String,
    pub user_id: String,
    pub host: String,
    pub os_user: String,
}

// ---------------------------------------------------------------------------
// Keychain key helpers
// ---------------------------------------------------------------------------

/// Keychain entry naming: mxdx/{user_id}/{device_id}
pub fn keychain_key(user_id: &str, device_id: &str) -> String {
    format!("mxdx/{user_id}/{device_id}")
}

/// Keychain entry for trust store: mxdx/{user_id}/trust-store
pub fn trust_store_key(user_id: &str) -> String {
    format!("mxdx/{user_id}/trust-store")
}

// ---------------------------------------------------------------------------
// Keychain backend trait
// ---------------------------------------------------------------------------

/// Abstract keychain backend (OS keychain or file-based fallback).
///
/// Implementations must store values encrypted at rest.
pub trait KeychainBackend: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>>;
    fn set(&self, key: &str, value: &[u8]) -> Result<()>;
    fn delete(&self, key: &str) -> Result<()>;
}

// ---------------------------------------------------------------------------
// In-memory mock (useful for tests across crates)
// ---------------------------------------------------------------------------

/// A trivial in-memory keychain for testing purposes only.
#[derive(Debug, Default)]
pub struct InMemoryKeychain {
    store: std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>,
}

impl InMemoryKeychain {
    pub fn new() -> Self {
        Self::default()
    }
}

impl KeychainBackend for InMemoryKeychain {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let guard = self.store.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(guard.get(key).cloned())
    }

    fn set(&self, key: &str, value: &[u8]) -> Result<()> {
        let mut guard = self.store.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        guard.insert(key.to_owned(), value.to_vec());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<()> {
        let mut guard = self.store.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        guard.remove(key);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_identity_roundtrip_serialization() {
        let identity = DeviceIdentity {
            device_id: "ABCDEF".into(),
            user_id: "@worker:example.com".into(),
            host: "node-01".into(),
            os_user: "deploy".into(),
        };
        let json = serde_json::to_string(&identity).unwrap();
        let restored: DeviceIdentity = serde_json::from_str(&json).unwrap();
        assert_eq!(identity, restored);
    }

    #[test]
    fn keychain_key_generates_correct_format() {
        assert_eq!(
            keychain_key("@user:example.com", "DEVXYZ"),
            "mxdx/@user:example.com/DEVXYZ"
        );
    }

    #[test]
    fn trust_store_key_generates_correct_format() {
        assert_eq!(
            trust_store_key("@user:example.com"),
            "mxdx/@user:example.com/trust-store"
        );
    }

    #[test]
    fn in_memory_keychain_set_get_delete() {
        let kc = InMemoryKeychain::new();

        // Initially empty
        assert_eq!(kc.get("k1").unwrap(), None);

        // Set and get
        kc.set("k1", b"secret").unwrap();
        assert_eq!(kc.get("k1").unwrap(), Some(b"secret".to_vec()));

        // Overwrite
        kc.set("k1", b"new_secret").unwrap();
        assert_eq!(kc.get("k1").unwrap(), Some(b"new_secret".to_vec()));

        // Delete
        kc.delete("k1").unwrap();
        assert_eq!(kc.get("k1").unwrap(), None);

        // Delete non-existent key is a no-op
        kc.delete("nonexistent").unwrap();
    }
}
