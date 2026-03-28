use anyhow::Result;
use mxdx_types::config::{DefaultsConfig, WorkerConfig, load_merged_config};

/// CLI arguments that can override config file values.
pub struct WorkerArgs {
    pub trust_anchor: Option<String>,
    pub history_retention: Option<u64>,
    pub cross_signing_mode: Option<String>,
    pub room_name: Option<String>,
}

/// Runtime configuration for the worker, combining defaults + worker config + CLI overrides.
pub struct WorkerRuntimeConfig {
    pub defaults: DefaultsConfig,
    pub worker: WorkerConfig,
    /// Computed from hostname.username.localpart, or explicitly set.
    pub resolved_room_name: String,
}

impl WorkerRuntimeConfig {
    /// Load configuration from `$HOME/.mxdx/defaults.toml` and `$HOME/.mxdx/worker.toml`.
    /// Falls back to defaults if files are missing.
    pub fn load() -> Result<Self> {
        let (defaults, worker): (DefaultsConfig, WorkerConfig) =
            load_merged_config("defaults.toml", "worker.toml")?;
        let resolved_room_name = Self::compute_room_name(&defaults, &worker);
        Ok(Self {
            defaults,
            worker,
            resolved_room_name,
        })
    }

    /// Construct directly from pre-built configs (useful for testing).
    pub fn from_parts(defaults: DefaultsConfig, worker: WorkerConfig) -> Self {
        let resolved_room_name = Self::compute_room_name(&defaults, &worker);
        Self {
            defaults,
            worker,
            resolved_room_name,
        }
    }

    /// Apply CLI argument overrides. CLI values take precedence over file config.
    pub fn with_cli_overrides(mut self, args: &WorkerArgs) -> Self {
        if let Some(ref anchor) = args.trust_anchor {
            self.worker.trust_anchor = Some(anchor.clone());
        }
        if let Some(retention) = args.history_retention {
            self.worker.history_retention = retention;
        }
        if let Some(ref name) = args.room_name {
            self.resolved_room_name = name.clone();
        }
        self
    }

    /// Compute the default room name: `{hostname}.{username}.{localpart}`.
    /// If `worker.room_name` is set, use that instead.
    fn compute_room_name(defaults: &DefaultsConfig, worker: &WorkerConfig) -> String {
        if let Some(ref name) = worker.room_name {
            return name.clone();
        }
        let host = hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_else(|_| "unknown".into());
        let user = whoami::username();
        let localpart = defaults
            .accounts
            .first()
            .map(|a| {
                a.user_id
                    .split(':')
                    .next()
                    .unwrap_or(&a.user_id)
                    .trim_start_matches('@')
                    .to_string()
            })
            .unwrap_or_else(|| "anon".into());
        format!("{host}.{user}.{localpart}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mxdx_types::config::{AccountConfig, WorkerConfig};

    #[test]
    fn explicit_room_name_is_used() {
        let defaults = DefaultsConfig::default();
        let worker = WorkerConfig {
            room_name: Some("my-custom-room".into()),
            ..Default::default()
        };
        let cfg = WorkerRuntimeConfig::from_parts(defaults, worker);
        assert_eq!(cfg.resolved_room_name, "my-custom-room");
    }

    #[test]
    fn cli_overrides_take_precedence() {
        let defaults = DefaultsConfig::default();
        let worker = WorkerConfig {
            trust_anchor: Some("@original:example.com".into()),
            history_retention: 90,
            ..Default::default()
        };
        let cfg = WorkerRuntimeConfig::from_parts(defaults, worker);

        let args = WorkerArgs {
            trust_anchor: Some("@override:example.com".into()),
            history_retention: Some(7),
            cross_signing_mode: None,
            room_name: Some("cli-room".into()),
        };
        let cfg = cfg.with_cli_overrides(&args);

        assert_eq!(cfg.worker.trust_anchor, Some("@override:example.com".into()));
        assert_eq!(cfg.worker.history_retention, 7);
        assert_eq!(cfg.resolved_room_name, "cli-room");
    }

    #[test]
    fn default_values_when_no_config() {
        let defaults = DefaultsConfig::default();
        let worker = WorkerConfig::default();
        let cfg = WorkerRuntimeConfig::from_parts(defaults, worker);

        assert_eq!(cfg.worker.history_retention, 90);
        assert_eq!(cfg.worker.telemetry_refresh_seconds, 300);
        assert!(cfg.worker.trust_anchor.is_none());
        assert!(cfg.worker.capabilities.extra.is_empty());
    }

    #[test]
    fn computed_room_name_uses_localpart() {
        let defaults = DefaultsConfig {
            accounts: vec![AccountConfig {
                user_id: "@worker:example.com".into(),
                homeserver: "https://example.com".into(),
            }],
            ..Default::default()
        };
        let worker = WorkerConfig::default();
        let cfg = WorkerRuntimeConfig::from_parts(defaults, worker);

        // Room name should end with ".worker" (the localpart)
        assert!(
            cfg.resolved_room_name.ends_with(".worker"),
            "Expected room name to end with '.worker', got: {}",
            cfg.resolved_room_name
        );
    }

    #[test]
    fn computed_room_name_uses_anon_when_no_accounts() {
        let defaults = DefaultsConfig::default();
        let worker = WorkerConfig::default();
        let cfg = WorkerRuntimeConfig::from_parts(defaults, worker);

        assert!(
            cfg.resolved_room_name.ends_with(".anon"),
            "Expected room name to end with '.anon', got: {}",
            cfg.resolved_room_name
        );
    }

    #[test]
    fn worker_config_from_toml_with_capabilities() {
        let toml_str = r#"
trust_anchor = "@admin:example.com"
history_retention = 30

[capabilities]
extra = ["docker", "gpu"]
"#;
        let worker: WorkerConfig = toml::from_str(toml_str).unwrap();
        let defaults = DefaultsConfig::default();
        let cfg = WorkerRuntimeConfig::from_parts(defaults, worker);

        assert_eq!(cfg.worker.trust_anchor, Some("@admin:example.com".into()));
        assert_eq!(cfg.worker.history_retention, 30);
        assert_eq!(cfg.worker.capabilities.extra, vec!["docker", "gpu"]);
    }
}
