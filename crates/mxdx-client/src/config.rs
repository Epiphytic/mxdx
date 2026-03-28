use anyhow::Result;
use mxdx_types::config::{ClientConfig, DefaultsConfig, load_merged_config};

/// CLI arguments that can override config file values.
pub struct ClientArgs {
    pub worker_room: Option<String>,
    pub coordinator_room: Option<String>,
    pub timeout: Option<u64>,
    pub heartbeat_interval: Option<u64>,
    pub interactive: bool,
    pub no_room_output: bool,
}

/// Runtime configuration for the client, combining defaults + client config + CLI overrides.
pub struct ClientRuntimeConfig {
    pub defaults: DefaultsConfig,
    pub client: ClientConfig,
}

impl ClientRuntimeConfig {
    /// Load configuration from `$HOME/.mxdx/defaults.toml` and `$HOME/.mxdx/client.toml`.
    /// Falls back to defaults if files are missing.
    pub fn load() -> Result<Self> {
        let (defaults, client): (DefaultsConfig, ClientConfig) =
            load_merged_config("defaults.toml", "client.toml")?;
        Ok(Self { defaults, client })
    }

    /// Construct directly from pre-built configs (useful for testing).
    pub fn from_parts(defaults: DefaultsConfig, client: ClientConfig) -> Self {
        Self { defaults, client }
    }

    /// Apply CLI argument overrides. CLI values take precedence over file config.
    pub fn with_cli_overrides(mut self, args: &ClientArgs) -> Self {
        if let Some(ref room) = args.worker_room {
            self.client.default_worker_room = Some(room.clone());
        }
        if let Some(ref room) = args.coordinator_room {
            self.client.coordinator_room = Some(room.clone());
        }
        if let Some(timeout) = args.timeout {
            self.client.session.timeout_seconds = Some(timeout);
        }
        if let Some(interval) = args.heartbeat_interval {
            self.client.session.heartbeat_interval = interval;
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mxdx_types::config::ClientConfig;

    #[test]
    fn cli_overrides_take_precedence() {
        let defaults = DefaultsConfig::default();
        let client = ClientConfig {
            default_worker_room: Some("original-room".into()),
            coordinator_room: Some("original-coord".into()),
            ..Default::default()
        };
        let cfg = ClientRuntimeConfig::from_parts(defaults, client);

        let args = ClientArgs {
            worker_room: Some("override-room".into()),
            coordinator_room: Some("override-coord".into()),
            timeout: Some(120),
            heartbeat_interval: Some(15),
            interactive: true,
            no_room_output: false,
        };
        let cfg = cfg.with_cli_overrides(&args);

        assert_eq!(
            cfg.client.default_worker_room,
            Some("override-room".into())
        );
        assert_eq!(
            cfg.client.coordinator_room,
            Some("override-coord".into())
        );
        assert_eq!(cfg.client.session.timeout_seconds, Some(120));
        assert_eq!(cfg.client.session.heartbeat_interval, 15);
    }

    #[test]
    fn default_values_when_no_config() {
        let defaults = DefaultsConfig::default();
        let client = ClientConfig::default();
        let cfg = ClientRuntimeConfig::from_parts(defaults, client);

        assert!(cfg.client.default_worker_room.is_none());
        assert!(cfg.client.coordinator_room.is_none());
        assert!(cfg.client.session.timeout_seconds.is_none());
        assert_eq!(cfg.client.session.heartbeat_interval, 30);
        assert!(!cfg.client.session.interactive);
        assert!(!cfg.client.session.no_room_output);
    }

    #[test]
    fn partial_cli_overrides_preserve_existing() {
        let defaults = DefaultsConfig::default();
        let client = ClientConfig {
            default_worker_room: Some("keep-this".into()),
            coordinator_room: Some("keep-this-too".into()),
            ..Default::default()
        };
        let cfg = ClientRuntimeConfig::from_parts(defaults, client);

        let args = ClientArgs {
            worker_room: None,
            coordinator_room: None,
            timeout: Some(60),
            heartbeat_interval: None,
            interactive: false,
            no_room_output: false,
        };
        let cfg = cfg.with_cli_overrides(&args);

        assert_eq!(cfg.client.default_worker_room, Some("keep-this".into()));
        assert_eq!(cfg.client.coordinator_room, Some("keep-this-too".into()));
        assert_eq!(cfg.client.session.timeout_seconds, Some(60));
        assert_eq!(cfg.client.session.heartbeat_interval, 30);
    }
}
