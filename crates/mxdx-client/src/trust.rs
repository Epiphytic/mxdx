use anyhow::Result;
use mxdx_types::config::CrossSigningMode;
use mxdx_types::identity::KeychainBackend;
use mxdx_types::trust::{TrustStore, TrustedDevice};

// ---------------------------------------------------------------------------
// Client trust — wraps TrustStore with keychain persistence and CLI operations
// ---------------------------------------------------------------------------

pub struct ClientTrust {
    store: TrustStore,
    keychain: Box<dyn KeychainBackend>,
    user_id: String,
}

impl ClientTrust {
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

    /// List all trusted devices.
    pub fn list_trusted(&self) -> Vec<&TrustedDevice> {
        self.store.trusted_devices.values().collect()
    }

    /// Add a trusted device and persist the updated store.
    pub fn add_device(&mut self, device: TrustedDevice) -> Result<()> {
        self.store.add_device(device);
        self.persist()?;
        Ok(())
    }

    /// Remove a trusted device by device_id and persist the updated store.
    pub fn remove_device(&mut self, device_id: &str) -> Result<()> {
        self.store.trusted_devices.retain(|k, _| k != device_id);
        self.persist()?;
        Ok(())
    }

    /// Pull a trust list from another device according to the cross-signing
    /// mode, then persist.
    pub fn pull_trust_list(
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

    /// Set the trust anchor to a new user ID and persist.
    pub fn set_trust_anchor(&mut self, user_id: &str) -> Result<()> {
        self.store.trust_anchor = user_id.to_string();
        self.persist()?;
        Ok(())
    }

    /// Check if a device is trusted.
    pub fn is_device_trusted(&self, device_id: &str) -> bool {
        self.store.is_trusted(device_id)
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
            user_id: "@client:example.com".into(),
            ed25519_key: format!("ed25519_key_{id}"),
            cross_signed_at: 1700000000,
        }
    }

    #[test]
    fn new_trust_store_is_empty() {
        let kc = Box::new(InMemoryKeychain::new());
        let ct = ClientTrust::load_or_create(
            kc,
            "@client:example.com",
            "@anchor:example.com",
        )
        .unwrap();

        assert!(ct.list_trusted().is_empty());
        assert_eq!(ct.trust_anchor(), "@anchor:example.com");
    }

    #[test]
    fn add_and_list_devices() {
        let kc = Box::new(InMemoryKeychain::new());
        let mut ct = ClientTrust::load_or_create(
            kc,
            "@client:example.com",
            "@anchor:example.com",
        )
        .unwrap();

        ct.add_device(make_device("DEV1")).unwrap();
        ct.add_device(make_device("DEV2")).unwrap();

        let trusted = ct.list_trusted();
        assert_eq!(trusted.len(), 2);
        assert!(ct.is_device_trusted("DEV1"));
        assert!(ct.is_device_trusted("DEV2"));
    }

    #[test]
    fn remove_device() {
        let kc = Box::new(InMemoryKeychain::new());
        let mut ct = ClientTrust::load_or_create(
            kc,
            "@client:example.com",
            "@anchor:example.com",
        )
        .unwrap();

        ct.add_device(make_device("DEV1")).unwrap();
        ct.add_device(make_device("DEV2")).unwrap();
        assert_eq!(ct.list_trusted().len(), 2);

        ct.remove_device("DEV1").unwrap();
        assert!(!ct.is_device_trusted("DEV1"));
        assert!(ct.is_device_trusted("DEV2"));
        assert_eq!(ct.list_trusted().len(), 1);
    }

    #[test]
    fn pull_trust_list_auto_adds_all() {
        let kc = Box::new(InMemoryKeychain::new());
        let mut ct = ClientTrust::load_or_create(
            kc,
            "@client:example.com",
            "@anchor:example.com",
        )
        .unwrap();

        let devices = vec![make_device("X"), make_device("Y")];
        ct.pull_trust_list(devices, CrossSigningMode::Auto).unwrap();

        assert!(ct.is_device_trusted("X"));
        assert!(ct.is_device_trusted("Y"));
    }

    #[test]
    fn pull_trust_list_manual_adds_nothing() {
        let kc = Box::new(InMemoryKeychain::new());
        let mut ct = ClientTrust::load_or_create(
            kc,
            "@client:example.com",
            "@anchor:example.com",
        )
        .unwrap();

        let devices = vec![make_device("X"), make_device("Y")];
        ct.pull_trust_list(devices, CrossSigningMode::Manual).unwrap();

        assert!(!ct.is_device_trusted("X"));
        assert!(!ct.is_device_trusted("Y"));
    }

    #[test]
    fn trust_anchor_operations() {
        let kc = Box::new(InMemoryKeychain::new());
        let mut ct = ClientTrust::load_or_create(
            kc,
            "@client:example.com",
            "@anchor:example.com",
        )
        .unwrap();

        assert_eq!(ct.trust_anchor(), "@anchor:example.com");

        ct.set_trust_anchor("@new-anchor:example.com").unwrap();
        assert_eq!(ct.trust_anchor(), "@new-anchor:example.com");
    }

    #[test]
    fn trust_store_persists_and_reloads() {
        let mut store = TrustStore::new("@anchor:example.com".into());
        store.add_device(make_device("DEV1"));
        let data = serde_json::to_vec(&store).unwrap();

        let kc = InMemoryKeychain::new();
        let key = mxdx_types::identity::trust_store_key("@client:example.com");
        kc.set(&key, &data).unwrap();

        let ct = ClientTrust::load_or_create(
            Box::new(kc),
            "@client:example.com",
            "@anchor:example.com",
        )
        .unwrap();

        assert!(ct.is_device_trusted("DEV1"));
        assert_eq!(ct.trust_anchor(), "@anchor:example.com");
    }
}
