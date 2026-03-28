use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::config::CrossSigningMode;

// ---------------------------------------------------------------------------
// Trust store
// ---------------------------------------------------------------------------

/// Persisted trust store — tracks which devices are trusted for E2EE.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrustStore {
    /// The Matrix identity this device trusts as anchor
    pub trust_anchor: String,
    /// Trusted device IDs with their signing keys
    pub trusted_devices: HashMap<String, TrustedDevice>,
}

/// A single trusted device entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrustedDevice {
    pub device_id: String,
    pub user_id: String,
    pub ed25519_key: String,
    /// Unix timestamp (seconds) when the device was cross-signed.
    pub cross_signed_at: u64,
}

impl TrustStore {
    pub fn new(trust_anchor: String) -> Self {
        Self {
            trust_anchor,
            trusted_devices: HashMap::new(),
        }
    }

    pub fn is_trusted(&self, device_id: &str) -> bool {
        self.trusted_devices.contains_key(device_id)
    }

    pub fn add_device(&mut self, device: TrustedDevice) {
        self.trusted_devices.insert(device.device_id.clone(), device);
    }

    pub fn remove_device(&mut self, device_id: &str) {
        self.trusted_devices.remove(device_id);
    }

    pub fn trusted_device_ids(&self) -> Vec<&str> {
        self.trusted_devices.keys().map(|s| s.as_str()).collect()
    }

    /// Merge a trust list from another device.
    ///
    /// - **Auto** mode: adds all devices automatically.
    /// - **Manual** mode: does nothing (requires explicit `add_device` calls).
    pub fn merge_trust_list(&mut self, devices: Vec<TrustedDevice>, mode: CrossSigningMode) {
        match mode {
            CrossSigningMode::Auto => {
                for device in devices {
                    self.add_device(device);
                }
            }
            CrossSigningMode::Manual => {
                // Manual mode requires explicit approval — no automatic merge
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_device(id: &str) -> TrustedDevice {
        TrustedDevice {
            device_id: id.into(),
            user_id: "@worker:example.com".into(),
            ed25519_key: format!("ed25519_key_{id}"),
            cross_signed_at: 1700000000,
        }
    }

    #[test]
    fn add_device_and_check_trusted() {
        let mut store = TrustStore::new("@anchor:example.com".into());
        assert!(!store.is_trusted("DEV1"));

        store.add_device(make_device("DEV1"));
        assert!(store.is_trusted("DEV1"));
    }

    #[test]
    fn remove_device_no_longer_trusted() {
        let mut store = TrustStore::new("@anchor:example.com".into());
        store.add_device(make_device("DEV1"));
        assert!(store.is_trusted("DEV1"));

        store.remove_device("DEV1");
        assert!(!store.is_trusted("DEV1"));
    }

    #[test]
    fn trusted_device_ids_returns_correct_list() {
        let mut store = TrustStore::new("@anchor:example.com".into());
        store.add_device(make_device("A"));
        store.add_device(make_device("B"));
        store.add_device(make_device("C"));

        let mut ids = store.trusted_device_ids();
        ids.sort();
        assert_eq!(ids, vec!["A", "B", "C"]);
    }

    #[test]
    fn merge_trust_list_auto_adds_all() {
        let mut store = TrustStore::new("@anchor:example.com".into());
        let devices = vec![make_device("X"), make_device("Y")];

        store.merge_trust_list(devices, CrossSigningMode::Auto);

        assert!(store.is_trusted("X"));
        assert!(store.is_trusted("Y"));
    }

    #[test]
    fn merge_trust_list_manual_adds_nothing() {
        let mut store = TrustStore::new("@anchor:example.com".into());
        let devices = vec![make_device("X"), make_device("Y")];

        store.merge_trust_list(devices, CrossSigningMode::Manual);

        assert!(!store.is_trusted("X"));
        assert!(!store.is_trusted("Y"));
    }

    #[test]
    fn trust_store_roundtrip_serialization() {
        let mut store = TrustStore::new("@anchor:example.com".into());
        store.add_device(make_device("DEV1"));
        store.add_device(make_device("DEV2"));

        let json = serde_json::to_string(&store).unwrap();
        let restored: TrustStore = serde_json::from_str(&json).unwrap();
        assert_eq!(store, restored);
    }

    #[test]
    fn trust_anchor_matches_expected() {
        let store = TrustStore::new("@root:matrix.org".into());
        assert_eq!(store.trust_anchor, "@root:matrix.org");
        assert!(store.trusted_devices.is_empty());
    }
}
