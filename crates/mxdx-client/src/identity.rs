use anyhow::Result;
use mxdx_types::identity::{DeviceIdentity, KeychainBackend};

// ---------------------------------------------------------------------------
// Client identity — stable device ID across restarts
// ---------------------------------------------------------------------------

pub struct ClientIdentity {
    identity: DeviceIdentity,
    #[allow(dead_code)]
    keychain: Box<dyn KeychainBackend>,
}

impl ClientIdentity {
    /// Load an existing identity from the keychain, or create a new one.
    ///
    /// The device ID is persisted under `mxdx/{user_id}/device-id` so that a
    /// client that restarts keeps the same Matrix device.
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
        let ci = ClientIdentity::load_or_create(
            kc,
            "@client:example.com",
            "workstation-01",
            "alice",
        )
        .unwrap();

        assert_eq!(ci.identity().user_id, "@client:example.com");
        assert_eq!(ci.identity().host, "workstation-01");
        assert_eq!(ci.identity().os_user, "alice");
        assert!(!ci.device_id().is_empty());
    }

    #[test]
    fn loads_existing_identity_stable_device_id() {
        let kc = InMemoryKeychain::new();
        let known_device_id = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        kc.set(
            "mxdx/@client:example.com/device-id",
            known_device_id.as_bytes(),
        )
        .unwrap();

        let ci = ClientIdentity::load_or_create(
            Box::new(kc),
            "@client:example.com",
            "workstation-01",
            "alice",
        )
        .unwrap();

        assert_eq!(ci.device_id(), known_device_id);
    }

    #[test]
    fn device_id_is_valid_uuid() {
        let kc = Box::new(InMemoryKeychain::new());
        let ci = ClientIdentity::load_or_create(
            kc,
            "@client:example.com",
            "workstation-01",
            "alice",
        )
        .unwrap();

        let parsed = uuid::Uuid::parse_str(ci.device_id()).unwrap();
        assert_eq!(parsed.get_version(), Some(uuid::Version::Random));
    }
}
