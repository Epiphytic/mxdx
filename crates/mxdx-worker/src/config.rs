use anyhow::Result;
use mxdx_types::config::{DefaultsConfig, WorkerConfig, load_merged_config};

/// Matrix credentials for login (from CLI flags, env vars, or config).
/// NOTE: The password field must NEVER be logged or included in tracing output.
#[derive(Debug, Clone)]
pub struct WorkerCredentials {
    pub homeserver: String,
    pub username: String,
    pub password: String,
}

/// CLI arguments that can override config file values.
pub struct WorkerArgs {
    pub trust_anchor: Option<String>,
    pub history_retention: Option<u64>,
    pub cross_signing_mode: Option<String>,
    pub room_name: Option<String>,
    /// Direct room ID — bypasses launcher space creation
    pub room_id: Option<String>,
    pub homeserver: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    /// When true, skip session restore and create a fresh device login.
    pub force_new_device: bool,
    /// Maximum concurrent sessions (overrides config file).
    pub max_sessions: Option<u32>,
    /// Commands allowed to execute (collected from repeated --allowed-command flags).
    pub allowed_commands: Vec<String>,
    /// Working directories allowed (collected from repeated --allowed-cwd flags).
    pub allowed_cwd: Vec<String>,
    /// Authorized Matrix user IDs (collected from repeated --authorized-user flags).
    pub authorized_users: Vec<String>,
}

/// Runtime configuration for the worker, combining defaults + worker config + CLI overrides.
pub struct WorkerRuntimeConfig {
    pub defaults: DefaultsConfig,
    pub worker: WorkerConfig,
    /// Computed from hostname.username.localpart, or explicitly set.
    pub resolved_room_name: String,
    /// Matrix credentials for login. None if not all fields are available.
    pub credentials: Option<WorkerCredentials>,
    /// Direct room ID — bypasses launcher space creation when set.
    pub room_id: Option<String>,
    /// When true, skip session restore and create a fresh device login.
    pub force_new_device: bool,
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
            credentials: None,
            room_id: None,
            force_new_device: false,
        })
    }

    /// Construct directly from pre-built configs (useful for testing).
    pub fn from_parts(defaults: DefaultsConfig, worker: WorkerConfig) -> Self {
        let resolved_room_name = Self::compute_room_name(&defaults, &worker);
        Self {
            defaults,
            worker,
            resolved_room_name,
            credentials: None,
            room_id: None,
            force_new_device: false,
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
        if let Some(ref id) = args.room_id {
            self.room_id = Some(id.clone());
        }

        self.force_new_device = args.force_new_device;

        if let Some(max) = args.max_sessions {
            self.worker.max_sessions = max;
        }
        // Extend (not replace) allowlists so CLI adds to config file values
        if !args.allowed_commands.is_empty() {
            self.worker
                .allowed_commands
                .extend(args.allowed_commands.iter().cloned());
        }
        if !args.allowed_cwd.is_empty() {
            self.worker
                .allowed_cwd
                .extend(args.allowed_cwd.iter().cloned());
        }
        if !args.authorized_users.is_empty() {
            self.worker
                .authorized_users
                .extend(args.authorized_users.iter().cloned());
        }

        // Build credentials: CLI args take highest priority, fall back to first account in defaults.
        let homeserver = args
            .homeserver
            .clone()
            .or_else(|| self.defaults.accounts.first().map(|a| a.homeserver.clone()));

        if let (Some(hs), Some(user), Some(pass)) =
            (homeserver, args.username.clone(), args.password.clone())
        {
            self.credentials = Some(WorkerCredentials {
                homeserver: hs,
                username: user,
                password: pass,
            });
        }

        self
    }

    /// Resolve all configured server accounts.
    /// CLI credentials (if present) become the first/primary account.
    /// Config file accounts with passwords are added as additional servers.
    pub fn resolve_accounts(&self) -> Vec<mxdx_matrix::ServerAccount> {
        let mut accounts = Vec::new();

        // CLI credentials first (highest priority)
        if let Some(ref creds) = self.credentials {
            accounts.push(mxdx_matrix::ServerAccount {
                homeserver: creds.homeserver.clone(),
                username: creds.username.clone(),
                password: creds.password.clone(),
                danger_accept_invalid_certs: false,
            });
        }

        // Config file accounts (skip any that match the CLI homeserver)
        for account in &self.defaults.accounts {
            if let Some(ref password) = account.password {
                let homeserver = &account.homeserver;
                // Don't duplicate CLI account
                let already_added = accounts.iter().any(|a| a.homeserver == *homeserver);
                if !already_added {
                    let username = account
                        .user_id
                        .split(':')
                        .next()
                        .unwrap_or(&account.user_id)
                        .trim_start_matches('@')
                        .to_string();
                    accounts.push(mxdx_matrix::ServerAccount {
                        homeserver: homeserver.clone(),
                        username,
                        password: password.clone(),
                        danger_accept_invalid_certs: false,
                    });
                }
            }
        }

        accounts
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
            room_id: None,
            homeserver: None,
            username: None,
            password: None,
            force_new_device: false,
            max_sessions: None,
            allowed_commands: vec![],
            allowed_cwd: vec![],
            authorized_users: vec![],
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
        assert_eq!(cfg.worker.telemetry_refresh_seconds, 60);
        assert!(cfg.worker.trust_anchor.is_none());
        assert!(cfg.worker.capabilities.extra.is_empty());
    }

    #[test]
    fn computed_room_name_uses_localpart() {
        let defaults = DefaultsConfig {
            accounts: vec![AccountConfig {
                user_id: "@worker:example.com".into(),
                homeserver: "https://example.com".into(),
                password: None,
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
    fn credentials_built_from_cli_args() {
        let defaults = DefaultsConfig::default();
        let worker = WorkerConfig::default();
        let cfg = WorkerRuntimeConfig::from_parts(defaults, worker);

        let args = WorkerArgs {
            trust_anchor: None,
            history_retention: None,
            cross_signing_mode: None,
            room_name: None,
            room_id: None,
            homeserver: Some("https://matrix.example.com".into()),
            username: Some("bot".into()),
            password: Some("secret".into()),
            force_new_device: false,
            max_sessions: None,
            allowed_commands: vec![],
            allowed_cwd: vec![],
            authorized_users: vec![],
        };
        let cfg = cfg.with_cli_overrides(&args);

        let creds = cfg.credentials.expect("credentials should be set");
        assert_eq!(creds.homeserver, "https://matrix.example.com");
        assert_eq!(creds.username, "bot");
        assert_eq!(creds.password, "secret");
    }

    #[test]
    fn credentials_fallback_homeserver_from_defaults() {
        let defaults = DefaultsConfig {
            accounts: vec![AccountConfig {
                user_id: "@worker:fallback.com".into(),
                homeserver: "https://fallback.com".into(),
                password: None,
            }],
            ..Default::default()
        };
        let worker = WorkerConfig::default();
        let cfg = WorkerRuntimeConfig::from_parts(defaults, worker);

        let args = WorkerArgs {
            trust_anchor: None,
            history_retention: None,
            cross_signing_mode: None,
            room_name: None,
            room_id: None,
            homeserver: None, // not provided — should fall back to defaults
            username: Some("bot".into()),
            password: Some("secret".into()),
            force_new_device: false,
            max_sessions: None,
            allowed_commands: vec![],
            allowed_cwd: vec![],
            authorized_users: vec![],
        };
        let cfg = cfg.with_cli_overrides(&args);

        let creds = cfg.credentials.expect("credentials should be set from fallback");
        assert_eq!(creds.homeserver, "https://fallback.com");
        assert_eq!(creds.username, "bot");
    }

    #[test]
    fn credentials_none_when_incomplete() {
        let defaults = DefaultsConfig::default();
        let worker = WorkerConfig::default();
        let cfg = WorkerRuntimeConfig::from_parts(defaults, worker);

        // Only username, no homeserver or password
        let args = WorkerArgs {
            trust_anchor: None,
            history_retention: None,
            cross_signing_mode: None,
            room_name: None,
            room_id: None,
            homeserver: None,
            username: Some("bot".into()),
            password: None,
            force_new_device: false,
            max_sessions: None,
            allowed_commands: vec![],
            allowed_cwd: vec![],
            authorized_users: vec![],
        };
        let cfg = cfg.with_cli_overrides(&args);
        assert!(cfg.credentials.is_none(), "credentials should be None when incomplete");
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

    #[test]
    fn resolve_accounts_from_cli_credentials() {
        let defaults = DefaultsConfig::default();
        let worker = WorkerConfig::default();
        let cfg = WorkerRuntimeConfig::from_parts(defaults, worker);

        let args = WorkerArgs {
            trust_anchor: None,
            history_retention: None,
            cross_signing_mode: None,
            room_name: None,
            room_id: None,
            homeserver: Some("https://matrix.example.com".into()),
            username: Some("bot".into()),
            password: Some("secret".into()),
            force_new_device: false,
            max_sessions: None,
            allowed_commands: vec![],
            allowed_cwd: vec![],
            authorized_users: vec![],
        };
        let cfg = cfg.with_cli_overrides(&args);
        let accounts = cfg.resolve_accounts();

        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].homeserver, "https://matrix.example.com");
        assert_eq!(accounts[0].username, "bot");
        assert_eq!(accounts[0].password, "secret");
    }

    #[test]
    fn resolve_accounts_from_config_with_password() {
        let defaults = DefaultsConfig {
            accounts: vec![
                AccountConfig {
                    user_id: "@worker:server-a.com".into(),
                    homeserver: "https://server-a.com".into(),
                    password: Some("pass-a".into()),
                },
                AccountConfig {
                    user_id: "@worker:server-b.com".into(),
                    homeserver: "https://server-b.com".into(),
                    password: Some("pass-b".into()),
                },
            ],
            ..Default::default()
        };
        let worker = WorkerConfig::default();
        let cfg = WorkerRuntimeConfig::from_parts(defaults, worker);
        let accounts = cfg.resolve_accounts();

        assert_eq!(accounts.len(), 2);
        assert_eq!(accounts[0].homeserver, "https://server-a.com");
        assert_eq!(accounts[0].username, "worker");
        assert_eq!(accounts[1].homeserver, "https://server-b.com");
        assert_eq!(accounts[1].username, "worker");
    }

    #[test]
    fn resolve_accounts_skips_config_without_password() {
        let defaults = DefaultsConfig {
            accounts: vec![AccountConfig {
                user_id: "@worker:no-pass.com".into(),
                homeserver: "https://no-pass.com".into(),
                password: None,
            }],
            ..Default::default()
        };
        let worker = WorkerConfig::default();
        let cfg = WorkerRuntimeConfig::from_parts(defaults, worker);
        let accounts = cfg.resolve_accounts();

        assert!(accounts.is_empty(), "accounts without password should be skipped");
    }

    #[test]
    fn resolve_accounts_cli_plus_config_no_duplicates() {
        let defaults = DefaultsConfig {
            accounts: vec![
                AccountConfig {
                    user_id: "@worker:server-a.com".into(),
                    homeserver: "https://server-a.com".into(),
                    password: Some("pass-a".into()),
                },
                AccountConfig {
                    user_id: "@worker:server-b.com".into(),
                    homeserver: "https://server-b.com".into(),
                    password: Some("pass-b".into()),
                },
            ],
            ..Default::default()
        };
        let worker = WorkerConfig::default();
        let cfg = WorkerRuntimeConfig::from_parts(defaults, worker);

        // CLI credentials match server-a — should not duplicate
        let args = WorkerArgs {
            trust_anchor: None,
            history_retention: None,
            cross_signing_mode: None,
            room_name: None,
            room_id: None,
            homeserver: Some("https://server-a.com".into()),
            username: Some("cli-user".into()),
            password: Some("cli-pass".into()),
            force_new_device: false,
            max_sessions: None,
            allowed_commands: vec![],
            allowed_cwd: vec![],
            authorized_users: vec![],
        };
        let cfg = cfg.with_cli_overrides(&args);
        let accounts = cfg.resolve_accounts();

        assert_eq!(accounts.len(), 2, "CLI + 1 non-duplicate config account");
        assert_eq!(accounts[0].homeserver, "https://server-a.com");
        assert_eq!(accounts[0].username, "cli-user"); // CLI takes priority
        assert_eq!(accounts[1].homeserver, "https://server-b.com");
    }

    #[test]
    fn cli_overrides_max_sessions() {
        let defaults = DefaultsConfig::default();
        let worker = WorkerConfig::default();
        let cfg = WorkerRuntimeConfig::from_parts(defaults, worker);

        let args = WorkerArgs {
            trust_anchor: None,
            history_retention: None,
            cross_signing_mode: None,
            room_name: None,
            room_id: None,
            homeserver: None,
            username: None,
            password: None,
            force_new_device: false,
            max_sessions: Some(20),
            allowed_commands: vec![],
            allowed_cwd: vec![],
            authorized_users: vec![],
        };
        let cfg = cfg.with_cli_overrides(&args);
        assert_eq!(cfg.worker.max_sessions, 20);
    }

    #[test]
    fn cli_extends_allowed_commands() {
        let defaults = DefaultsConfig::default();
        let mut worker = WorkerConfig::default();
        worker.allowed_commands = vec!["echo".into()];
        let cfg = WorkerRuntimeConfig::from_parts(defaults, worker);

        let args = WorkerArgs {
            trust_anchor: None,
            history_retention: None,
            cross_signing_mode: None,
            room_name: None,
            room_id: None,
            homeserver: None,
            username: None,
            password: None,
            force_new_device: false,
            max_sessions: None,
            allowed_commands: vec!["ls".into(), "cat".into()],
            allowed_cwd: vec![],
            authorized_users: vec![],
        };
        let cfg = cfg.with_cli_overrides(&args);
        assert_eq!(cfg.worker.allowed_commands, vec!["echo", "ls", "cat"]);
    }

    #[test]
    fn cli_extends_allowed_cwd() {
        let defaults = DefaultsConfig::default();
        let worker = WorkerConfig::default(); // default has ["/tmp"]
        let cfg = WorkerRuntimeConfig::from_parts(defaults, worker);

        let args = WorkerArgs {
            trust_anchor: None,
            history_retention: None,
            cross_signing_mode: None,
            room_name: None,
            room_id: None,
            homeserver: None,
            username: None,
            password: None,
            force_new_device: false,
            max_sessions: None,
            allowed_commands: vec![],
            allowed_cwd: vec!["/home/worker".into()],
            authorized_users: vec![],
        };
        let cfg = cfg.with_cli_overrides(&args);
        assert_eq!(cfg.worker.allowed_cwd, vec!["/tmp", "/home/worker"]);
    }

    #[test]
    fn cli_extends_authorized_users() {
        let defaults = DefaultsConfig::default();
        let mut worker = WorkerConfig::default();
        worker.authorized_users = vec!["@admin:example.com".into()];
        let cfg = WorkerRuntimeConfig::from_parts(defaults, worker);

        let args = WorkerArgs {
            trust_anchor: None,
            history_retention: None,
            cross_signing_mode: None,
            room_name: None,
            room_id: None,
            homeserver: None,
            username: None,
            password: None,
            force_new_device: false,
            max_sessions: None,
            allowed_commands: vec![],
            allowed_cwd: vec![],
            authorized_users: vec!["@ops:example.com".into()],
        };
        let cfg = cfg.with_cli_overrides(&args);
        assert_eq!(
            cfg.worker.authorized_users,
            vec!["@admin:example.com", "@ops:example.com"]
        );
    }
}
