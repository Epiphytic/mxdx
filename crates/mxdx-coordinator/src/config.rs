use anyhow::Result;
use mxdx_types::config::{CoordinatorConfig, DefaultsConfig, load_merged_config};

/// CLI arguments that can override config file values.
pub struct CoordinatorArgs {
    pub room: Option<String>,
    pub capability_room_prefix: Option<String>,
    pub default_on_timeout: Option<String>,
    pub default_on_heartbeat_miss: Option<String>,
}

/// Runtime configuration for the coordinator, combining defaults + coordinator config + CLI overrides.
pub struct CoordinatorRuntimeConfig {
    pub defaults: DefaultsConfig,
    pub coordinator: CoordinatorConfig,
}

impl CoordinatorRuntimeConfig {
    /// Load configuration from `$HOME/.mxdx/defaults.toml` and `$HOME/.mxdx/coordinator.toml`.
    /// Falls back to defaults if files are missing.
    pub fn load() -> Result<Self> {
        let (defaults, coordinator): (DefaultsConfig, CoordinatorConfig) =
            load_merged_config("defaults.toml", "coordinator.toml")?;
        Ok(Self {
            defaults,
            coordinator,
        })
    }

    /// Construct directly from pre-built configs (useful for testing).
    pub fn from_parts(defaults: DefaultsConfig, coordinator: CoordinatorConfig) -> Self {
        Self {
            defaults,
            coordinator,
        }
    }

    /// Apply CLI argument overrides. CLI values take precedence over file config.
    pub fn with_cli_overrides(mut self, args: &CoordinatorArgs) -> Self {
        if let Some(ref room) = args.room {
            self.coordinator.room = Some(room.clone());
        }
        if let Some(ref prefix) = args.capability_room_prefix {
            self.coordinator.capability_room_prefix = Some(prefix.clone());
        }
        if let Some(ref action) = args.default_on_timeout {
            self.coordinator.default_on_timeout = action.clone();
        }
        if let Some(ref action) = args.default_on_heartbeat_miss {
            self.coordinator.default_on_heartbeat_miss = action.clone();
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mxdx_types::config::CoordinatorConfig;

    #[test]
    fn default_values_when_no_config() {
        let defaults = DefaultsConfig::default();
        let coordinator = CoordinatorConfig::default();
        let cfg = CoordinatorRuntimeConfig::from_parts(defaults, coordinator);

        assert!(cfg.coordinator.room.is_none());
        assert!(cfg.coordinator.capability_room_prefix.is_none());
        assert_eq!(cfg.coordinator.default_on_timeout, "escalate");
        assert_eq!(cfg.coordinator.default_on_heartbeat_miss, "escalate");
    }

    #[test]
    fn cli_overrides_take_precedence() {
        let defaults = DefaultsConfig::default();
        let coordinator = CoordinatorConfig {
            room: Some("!original:example.com".into()),
            capability_room_prefix: Some("cap-".into()),
            default_on_timeout: "escalate".into(),
            default_on_heartbeat_miss: "escalate".into(),
        };
        let cfg = CoordinatorRuntimeConfig::from_parts(defaults, coordinator);

        let args = CoordinatorArgs {
            room: Some("!override:example.com".into()),
            capability_room_prefix: Some("new-cap-".into()),
            default_on_timeout: Some("abandon".into()),
            default_on_heartbeat_miss: Some("abandon".into()),
        };
        let cfg = cfg.with_cli_overrides(&args);

        assert_eq!(cfg.coordinator.room, Some("!override:example.com".into()));
        assert_eq!(
            cfg.coordinator.capability_room_prefix,
            Some("new-cap-".into())
        );
        assert_eq!(cfg.coordinator.default_on_timeout, "abandon");
        assert_eq!(cfg.coordinator.default_on_heartbeat_miss, "abandon");
    }

    #[test]
    fn partial_cli_overrides() {
        let defaults = DefaultsConfig::default();
        let coordinator = CoordinatorConfig {
            room: Some("!original:example.com".into()),
            capability_room_prefix: Some("cap-".into()),
            default_on_timeout: "escalate".into(),
            default_on_heartbeat_miss: "escalate".into(),
        };
        let cfg = CoordinatorRuntimeConfig::from_parts(defaults, coordinator);

        let args = CoordinatorArgs {
            room: None,
            capability_room_prefix: None,
            default_on_timeout: Some("abandon".into()),
            default_on_heartbeat_miss: None,
        };
        let cfg = cfg.with_cli_overrides(&args);

        // Only timeout action should be overridden
        assert_eq!(cfg.coordinator.room, Some("!original:example.com".into()));
        assert_eq!(cfg.coordinator.capability_room_prefix, Some("cap-".into()));
        assert_eq!(cfg.coordinator.default_on_timeout, "abandon");
        assert_eq!(cfg.coordinator.default_on_heartbeat_miss, "escalate");
    }

    #[test]
    fn coordinator_config_from_toml() {
        let toml_str = r#"
room = "!coord:example.com"
capability_room_prefix = "worker-"
default_on_timeout = "abandon"
default_on_heartbeat_miss = "escalate"
"#;
        let coordinator: CoordinatorConfig = toml::from_str(toml_str).unwrap();
        let defaults = DefaultsConfig::default();
        let cfg = CoordinatorRuntimeConfig::from_parts(defaults, coordinator);

        assert_eq!(cfg.coordinator.room, Some("!coord:example.com".into()));
        assert_eq!(
            cfg.coordinator.capability_room_prefix,
            Some("worker-".into())
        );
        assert_eq!(cfg.coordinator.default_on_timeout, "abandon");
        assert_eq!(cfg.coordinator.default_on_heartbeat_miss, "escalate");
    }
}
