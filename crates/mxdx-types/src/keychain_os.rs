//! OS keychain backend using the `keyring` crate.
//!
//! Service name: `"mxdx"` — matches the npm `keytar.setPassword('mxdx', key, value)` calls.

use anyhow::{Context, Result};
use keyring::Entry;

use crate::identity::KeychainBackend;

/// OS-native keychain backend (macOS Keychain, Windows Credential Manager, Linux Secret Service).
pub struct OsKeychain {
    service: String,
}

impl OsKeychain {
    pub fn new() -> Self {
        Self {
            service: "mxdx".to_string(),
        }
    }

    fn entry(&self, key: &str) -> Result<Entry> {
        Entry::new(&self.service, key).context("failed to create keyring entry")
    }
}

impl Default for OsKeychain {
    fn default() -> Self {
        Self::new()
    }
}

impl KeychainBackend for OsKeychain {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let entry = self.entry(key)?;
        match entry.get_password() {
            Ok(password) => Ok(Some(password.into_bytes())),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(anyhow::anyhow!("keyring get failed: {e}")),
        }
    }

    fn set(&self, key: &str, value: &[u8]) -> Result<()> {
        let entry = self.entry(key)?;
        let value_str =
            std::str::from_utf8(value).context("keychain value must be valid UTF-8")?;
        entry
            .set_password(value_str)
            .context("keyring set failed")?;
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<()> {
        let entry = self.entry(key)?;
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()), // idempotent: deleting non-existent is fine
            Err(e) => Err(anyhow::anyhow!("keyring delete failed: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires OS keychain (Secret Service / Keychain / Credential Manager)"]
    fn test_os_keychain_set_get_delete() {
        let kc = OsKeychain::new();
        let key = "mxdx:test_os_keychain_roundtrip";

        // Set
        kc.set(key, b"test-secret-value").unwrap();

        // Get
        let val = kc.get(key).unwrap();
        assert_eq!(val, Some(b"test-secret-value".to_vec()));

        // Delete
        kc.delete(key).unwrap();
        let val = kc.get(key).unwrap();
        assert_eq!(val, None);
    }

    #[test]
    #[ignore = "requires OS keychain (Secret Service / Keychain / Credential Manager)"]
    fn test_os_keychain_get_nonexistent_returns_none() {
        let kc = OsKeychain::new();
        let val = kc
            .get("mxdx:this_key_should_never_exist_12345")
            .unwrap();
        assert_eq!(val, None);
    }
}
