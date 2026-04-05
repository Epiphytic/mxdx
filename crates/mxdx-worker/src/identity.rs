use anyhow::Result;
use mxdx_types::identity::{DeviceIdentity, KeychainBackend};

// ---------------------------------------------------------------------------
// Worker identity — stable device ID across restarts
// ---------------------------------------------------------------------------

pub struct WorkerIdentity {
    identity: DeviceIdentity,
    #[allow(dead_code)]
    keychain: Box<dyn KeychainBackend>,
}

impl WorkerIdentity {
    /// Load an existing identity from the keychain, or create a new one.
    ///
    /// The device ID is persisted under `mxdx/{user_id}/device-id` so that a
    /// worker that restarts on the same host keeps the same Matrix device.
    pub fn load_or_create(
        keychain: Box<dyn KeychainBackend>,
        user_id: &str,
        host: &str,
        os_user: &str,
    ) -> Result<Self> {
        let device_id_key = format!("mxdx/{user_id}/device-id");

        if let Some(data) = keychain.get(&device_id_key)? {
            let device_id = String::from_utf8(data)?;
            let identity = DeviceIdentity {
                device_id,
                user_id: user_id.to_string(),
                host: host.to_string(),
                os_user: os_user.to_string(),
            };
            return Ok(Self { identity, keychain });
        }

        // Create new device ID and persist it
        let device_id = uuid::Uuid::new_v4().to_string();
        keychain.set(&device_id_key, device_id.as_bytes())?;

        let identity = DeviceIdentity {
            device_id,
            user_id: user_id.to_string(),
            host: host.to_string(),
            os_user: os_user.to_string(),
        };

        Ok(Self { identity, keychain })
    }

    pub fn identity(&self) -> &DeviceIdentity {
        &self.identity
    }

    pub fn device_id(&self) -> &str {
        &self.identity.device_id
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use mxdx_types::identity::InMemoryKeychain;

    #[test]
    fn creates_new_identity_when_keychain_is_empty() {
        let kc = Box::new(InMemoryKeychain::new());
        let wi = WorkerIdentity::load_or_create(
            kc,
            "@worker:example.com",
            "node-01",
            "deploy",
        )
        .unwrap();

        assert_eq!(wi.identity().user_id, "@worker:example.com");
        assert_eq!(wi.identity().host, "node-01");
        assert_eq!(wi.identity().os_user, "deploy");
        assert!(!wi.device_id().is_empty());
    }

    #[test]
    fn loads_existing_identity_stable_device_id() {
        // Pre-seed the keychain with a known device ID
        let kc = InMemoryKeychain::new();
        let known_device_id = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        kc.set(
            "mxdx/@worker:example.com/device-id",
            known_device_id.as_bytes(),
        )
        .unwrap();

        let wi = WorkerIdentity::load_or_create(
            Box::new(kc),
            "@worker:example.com",
            "node-01",
            "deploy",
        )
        .unwrap();

        assert_eq!(wi.device_id(), known_device_id);
    }

    #[test]
    fn device_id_is_valid_uuid() {
        let kc = Box::new(InMemoryKeychain::new());
        let wi = WorkerIdentity::load_or_create(
            kc,
            "@worker:example.com",
            "node-01",
            "deploy",
        )
        .unwrap();

        // Must parse as a valid UUID v4
        let parsed = uuid::Uuid::parse_str(wi.device_id()).unwrap();
        assert_eq!(parsed.get_version(), Some(uuid::Version::Random));
    }
}
