//! Chained keychain backend: tries a primary backend first, falls back to a secondary.
//!
//! The default configuration uses [`OsKeychain`](crate::keychain_os::OsKeychain) as the primary
//! and [`FileKeychain`](crate::keychain_file::FileKeychain) as the fallback.

use anyhow::Result;

use crate::identity::KeychainBackend;
use crate::keychain_file::FileKeychain;
use crate::keychain_os::OsKeychain;

/// A keychain that delegates to a primary backend and falls back to a secondary.
pub struct ChainedKeychain {
    primary: Box<dyn KeychainBackend>,
    fallback: Box<dyn KeychainBackend>,
}

impl ChainedKeychain {
    /// Create a `ChainedKeychain` with explicit primary and fallback backends.
    pub fn new(primary: Box<dyn KeychainBackend>, fallback: Box<dyn KeychainBackend>) -> Self {
        Self { primary, fallback }
    }

    /// Create the default chain: OS keychain as primary, file keychain as fallback.
    ///
    /// If the OS keychain is unavailable at runtime (e.g., headless server without
    /// Secret Service), operations will transparently fall back to the file backend.
    pub fn default_chain() -> Result<Self> {
        let primary = Box::new(OsKeychain::new());
        let fallback = Box::new(FileKeychain::new()?);
        Ok(Self { primary, fallback })
    }
}

impl KeychainBackend for ChainedKeychain {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        // Try primary first
        match self.primary.get(key) {
            Ok(Some(val)) => return Ok(Some(val)),
            Ok(None) => {
                // Primary has no entry, try fallback
            }
            Err(_) => {
                // Primary errored (e.g., no Secret Service), try fallback
            }
        }
        self.fallback.get(key)
    }

    fn set(&self, key: &str, value: &[u8]) -> Result<()> {
        // Always write to BOTH backends. The OS keychain may silently fail to
        // persist (e.g., session-scoped Secret Service on Linux), so the file
        // keychain ensures durability. On read, the primary is tried first for
        // speed; file fallback catches cases where the OS keychain lost data.
        let _ = self.primary.set(key, value); // best-effort
        self.fallback.set(key, value)
    }

    fn delete(&self, key: &str) -> Result<()> {
        // Delete from both, ignore individual errors as long as at least one succeeds
        let primary_result = self.primary.delete(key);
        let fallback_result = self.fallback.delete(key);

        // If both fail, return the fallback error (more likely to be meaningful)
        if primary_result.is_err() && fallback_result.is_err() {
            fallback_result
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::InMemoryKeychain;
    use std::sync::Arc;

    /// A shared-state keychain wrapper that lets us inspect the store after
    /// the keychain has been moved into a `ChainedKeychain`.
    #[derive(Clone)]
    struct SharedKeychain {
        store: Arc<std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>>,
    }

    impl SharedKeychain {
        fn new() -> Self {
            Self {
                store: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            }
        }

        fn contains(&self, key: &str) -> bool {
            self.store.lock().unwrap().contains_key(key)
        }

        fn get_value(&self, key: &str) -> Option<Vec<u8>> {
            self.store.lock().unwrap().get(key).cloned()
        }
    }

    impl KeychainBackend for SharedKeychain {
        fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
            Ok(self.store.lock().unwrap().get(key).cloned())
        }
        fn set(&self, key: &str, value: &[u8]) -> Result<()> {
            self.store.lock().unwrap().insert(key.to_owned(), value.to_vec());
            Ok(())
        }
        fn delete(&self, key: &str) -> Result<()> {
            self.store.lock().unwrap().remove(key);
            Ok(())
        }
    }

    #[test]
    fn test_chained_keychain_primary_takes_precedence() {
        let primary = InMemoryKeychain::new();
        let fallback = InMemoryKeychain::new();

        primary.set("k", b"primary-value").unwrap();
        fallback.set("k", b"fallback-value").unwrap();

        let chain = ChainedKeychain::new(Box::new(primary), Box::new(fallback));
        let val = chain.get("k").unwrap();
        assert_eq!(val, Some(b"primary-value".to_vec()));
    }

    #[test]
    fn test_chained_keychain_fallback_on_primary_miss() {
        let primary = InMemoryKeychain::new();
        let fallback = InMemoryKeychain::new();

        // Only set in fallback
        fallback.set("k", b"fallback-value").unwrap();

        let chain = ChainedKeychain::new(Box::new(primary), Box::new(fallback));
        let val = chain.get("k").unwrap();
        assert_eq!(val, Some(b"fallback-value".to_vec()));
    }

    #[test]
    fn test_chained_keychain_set_only_writes_primary_on_success() {
        let primary = SharedKeychain::new();
        let fallback = SharedKeychain::new();

        let primary_clone = primary.clone();
        let fallback_clone = fallback.clone();

        let chain = ChainedKeychain::new(Box::new(primary), Box::new(fallback));
        chain.set("k", b"secret").unwrap();

        // Primary should have the value
        assert_eq!(
            primary_clone.get_value("k"),
            Some(b"secret".to_vec()),
            "primary should have the value"
        );
        // Fallback should NOT have the value (primary succeeded, no disk write)
        assert!(
            !fallback_clone.contains("k"),
            "fallback should NOT receive writes when primary succeeds"
        );
    }

    #[test]
    fn test_chained_keychain_delete_removes_from_both() {
        let primary = SharedKeychain::new();
        let fallback = SharedKeychain::new();

        let primary_clone = primary.clone();
        let fallback_clone = fallback.clone();

        primary.set("k", b"p").unwrap();
        fallback.set("k", b"f").unwrap();

        let chain = ChainedKeychain::new(Box::new(primary), Box::new(fallback));
        chain.delete("k").unwrap();

        // Both should be empty
        assert!(!primary_clone.contains("k"), "primary should be cleared");
        assert!(!fallback_clone.contains("k"), "fallback should be cleared");

        // Get should return None
        assert_eq!(chain.get("k").unwrap(), None);
    }
}
