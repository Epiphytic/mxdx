use std::collections::HashMap;
use std::io::{Read, Write};

use age::x25519::{Identity, Recipient};
use anyhow::{Context, Result};

/// Encrypted secret store backed by age x25519 encryption.
///
/// Each value is individually encrypted with the holder's public key.
/// The store can be serialized/deserialized for persistence.
pub struct SecretStore {
    identity: Identity,
    recipient: Recipient,
    entries: HashMap<String, Vec<u8>>,
}

impl SecretStore {
    /// Creates a new empty store with the given identity.
    pub fn new(identity: Identity) -> Self {
        let recipient = identity.to_public();
        Self {
            identity,
            recipient,
            entries: HashMap::new(),
        }
    }

    /// Creates a store with a generated key for tests.
    /// Only available in test builds (security requirement mxdx-tky).
    #[cfg(test)]
    pub fn new_with_test_key() -> Self {
        Self::new(Identity::generate())
    }

    /// Encrypts and stores a value under the given key.
    pub fn add(&mut self, key: &str, value: &str) -> Result<()> {
        let encrypted =
            encrypt_value(&self.recipient, value.as_bytes()).context("failed to encrypt secret")?;
        self.entries.insert(key.to_string(), encrypted);
        Ok(())
    }

    /// Decrypts and returns the value for the given key, or None if not found.
    pub fn get(&self, key: &str) -> Result<Option<String>> {
        let Some(encrypted) = self.entries.get(key) else {
            return Ok(None);
        };
        let decrypted =
            decrypt_value(&self.identity, encrypted).context("failed to decrypt secret")?;
        let plaintext =
            String::from_utf8(decrypted).context("decrypted value is not valid UTF-8")?;
        Ok(Some(plaintext))
    }

    /// Serializes the store (key->ciphertext map) to bytes.
    pub fn serialize(&self) -> Result<Vec<u8>> {
        let serializable: Vec<(&str, &[u8])> = self
            .entries
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_slice()))
            .collect();
        serde_json::to_vec(&serializable).context("failed to serialize store")
    }

    /// Deserializes a store from bytes, using the provided identity for future decryption.
    pub fn deserialize(data: &[u8], identity: &Identity) -> Result<Self> {
        let entries: Vec<(String, Vec<u8>)> =
            serde_json::from_slice(data).context("failed to deserialize store")?;
        let recipient = identity.to_public();
        Ok(Self {
            identity: identity.clone(),
            recipient,
            entries: entries.into_iter().collect(),
        })
    }

    /// Returns a reference to the store's identity (private key).
    pub fn key(&self) -> &Identity {
        &self.identity
    }
}

fn encrypt_value(recipient: &Recipient, plaintext: &[u8]) -> Result<Vec<u8>> {
    let encryptor =
        age::Encryptor::with_recipients(std::iter::once(recipient as &dyn age::Recipient))
            .map_err(|e| anyhow::anyhow!("encryption setup failed: {e}"))?;
    let mut encrypted = vec![];
    let mut writer = encryptor
        .wrap_output(&mut encrypted)
        .context("failed to create encryption writer")?;
    writer.write_all(plaintext)?;
    writer.finish()?;
    Ok(encrypted)
}

fn decrypt_value(identity: &Identity, ciphertext: &[u8]) -> Result<Vec<u8>> {
    let decryptor = age::Decryptor::new(ciphertext)
        .map_err(|e| anyhow::anyhow!("decryption setup failed: {e}"))?;
    let mut reader = decryptor
        .decrypt(std::iter::once(identity as &dyn age::Identity))
        .map_err(|e| anyhow::anyhow!("decryption failed: {e}"))?;
    let mut decrypted = vec![];
    reader.read_to_end(&mut decrypted)?;
    Ok(decrypted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_store_add_retrieve_round_trip() {
        let mut store = SecretStore::new_with_test_key();
        store.add("github.token", "ghp_testtoken123").unwrap();
        let retrieved = store.get("github.token").unwrap().unwrap();
        assert_eq!(retrieved, "ghp_testtoken123");
    }

    #[test]
    fn secrets_store_unknown_key_returns_none() {
        let store = SecretStore::new_with_test_key();
        assert!(store.get("nonexistent").unwrap().is_none());
    }

    #[test]
    fn secrets_store_survives_serialize_deserialize() {
        let mut store = SecretStore::new_with_test_key();
        store.add("key1", "value1").unwrap();
        let serialized = store.serialize().unwrap();
        let store2 = SecretStore::deserialize(&serialized, store.key()).unwrap();
        assert_eq!(store2.get("key1").unwrap().unwrap(), "value1");
    }

    #[test]
    fn test_key_constructor_is_test_only() {
        // If this compiles, new_with_test_key is available in test mode
        let _ = SecretStore::new_with_test_key();
    }
}
