use anyhow::Result;
use mxdx_types::config::CrossSigningMode;
use mxdx_types::identity::KeychainBackend;
use mxdx_types::trust::{TrustStore, TrustedDevice};

// ---------------------------------------------------------------------------
// Worker trust — wraps TrustStore with keychain persistence
// ---------------------------------------------------------------------------

pub struct WorkerTrust {
    store: TrustStore,
    keychain: Box<dyn KeychainBackend>,
    user_id: String,
}

impl WorkerTrust {
    /// Load an existing trust store from the keychain, or create a new one
    /// anchored to `trust_anchor`.
    pub fn load_or_create(
        keychain: Box<dyn KeychainBackend>,
        user_id: &str,
        trust_anchor: &str,
    ) -> Result<Self> {
        let key = mxdx_types::identity::trust_store_key(user_id);
        let store = if let Some(data) = keychain.get(&key)? {
            serde_json::from_slice(&data)?
        } else {
            TrustStore::new(trust_anchor.to_string())
        };
        Ok(Self {
            store,
            keychain,
            user_id: user_id.to_string(),
        })
    }

    /// Check if a device is trusted.
    pub fn is_device_trusted(&self, device_id: &str) -> bool {
        self.store.is_trusted(device_id)
    }

    /// Add a trusted device and persist the updated store.
    pub fn add_trusted_device(&mut self, device: TrustedDevice) -> Result<()> {
        self.store.add_device(device);
        self.persist()?;
        Ok(())
    }

    /// Merge a trust list from another device according to the cross-signing
    /// mode, then persist.
    pub fn merge_trust_list(
        &mut self,
        devices: Vec<TrustedDevice>,
        mode: CrossSigningMode,
    ) -> Result<()> {
        self.store.merge_trust_list(devices, mode);
        self.persist()?;
        Ok(())
    }

    /// Get the trust anchor identity.
    pub fn trust_anchor(&self) -> &str {
        &self.store.trust_anchor
    }

    fn persist(&self) -> Result<()> {
        let key = mxdx_types::identity::trust_store_key(&self.user_id);
        let data = serde_json::to_vec(&self.store)?;
        self.keychain.set(&key, &data)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use mxdx_types::identity::InMemoryKeychain;

    fn make_device(id: &str) -> TrustedDevice {
        TrustedDevice {
            device_id: id.into(),
            user_id: "@worker:example.com".into(),
            ed25519_key: format!("ed25519_key_{id}"),
            cross_signed_at: 1700000000,
        }
    }

    #[test]
    fn new_trust_store_is_empty() {
        let kc = Box::new(InMemoryKeychain::new());
        let wt = WorkerTrust::load_or_create(
            kc,
            "@worker:example.com",
            "@anchor:example.com",
        )
        .unwrap();

        assert!(!wt.is_device_trusted("DEV1"));
        assert_eq!(wt.trust_anchor(), "@anchor:example.com");
    }

    #[test]
    fn add_device_then_trusted() {
        let kc = Box::new(InMemoryKeychain::new());
        let mut wt = WorkerTrust::load_or_create(
            kc,
            "@worker:example.com",
            "@anchor:example.com",
        )
        .unwrap();

        wt.add_trusted_device(make_device("DEV1")).unwrap();
        assert!(wt.is_device_trusted("DEV1"));
    }

    #[test]
    fn merge_auto_adds_all_devices() {
        let kc = Box::new(InMemoryKeychain::new());
        let mut wt = WorkerTrust::load_or_create(
            kc,
            "@worker:example.com",
            "@anchor:example.com",
        )
        .unwrap();

        let devices = vec![make_device("X"), make_device("Y")];
        wt.merge_trust_list(devices, CrossSigningMode::Auto).unwrap();

        assert!(wt.is_device_trusted("X"));
        assert!(wt.is_device_trusted("Y"));
    }

    #[test]
    fn merge_manual_adds_nothing() {
        let kc = Box::new(InMemoryKeychain::new());
        let mut wt = WorkerTrust::load_or_create(
            kc,
            "@worker:example.com",
            "@anchor:example.com",
        )
        .unwrap();

        let devices = vec![make_device("X"), make_device("Y")];
        wt.merge_trust_list(devices, CrossSigningMode::Manual).unwrap();

        assert!(!wt.is_device_trusted("X"));
        assert!(!wt.is_device_trusted("Y"));
    }

    #[test]
    fn trust_store_persists_and_reloads() {
        // Build a trust store with one device, serialize it, and seed the
        // keychain so that load_or_create finds persisted data.
        let mut store = TrustStore::new("@anchor:example.com".into());
        store.add_device(make_device("DEV1"));
        let data = serde_json::to_vec(&store).unwrap();

        let kc = InMemoryKeychain::new();
        let key = mxdx_types::identity::trust_store_key("@worker:example.com");
        kc.set(&key, &data).unwrap();

        // Reload from pre-seeded keychain
        let wt = WorkerTrust::load_or_create(
            Box::new(kc),
            "@worker:example.com",
            "@anchor:example.com",
        )
        .unwrap();

        assert!(wt.is_device_trusted("DEV1"));
        assert_eq!(wt.trust_anchor(), "@anchor:example.com");
    }
}
