use anyhow::Result;
use mxdx_types::config::{ClientConfig, DefaultsConfig, load_merged_config};

/// Matrix credentials for connecting to a homeserver.
/// These are resolved from CLI flags, environment variables, or config files.
/// NOTE: Never log or display the password field.
#[derive(Debug, Clone)]
pub struct ClientCredentials {
    pub homeserver: String,
    pub username: String,
    pub password: String,
}

/// CLI arguments that can override config file values.
pub struct ClientArgs {
    pub worker_room: Option<String>,
    pub coordinator_room: Option<String>,
    pub timeout: Option<u64>,
    pub heartbeat_interval: Option<u64>,
    pub interactive: bool,
    pub no_room_output: bool,
    pub homeserver: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    /// When true, skip session restore and create a fresh device login.
    pub force_new_device: bool,
}

/// Runtime configuration for the client, combining defaults + client config + CLI overrides.
pub struct ClientRuntimeConfig {
    pub defaults: DefaultsConfig,
    pub client: ClientConfig,
    pub credentials: Option<ClientCredentials>,
    /// When true, skip session restore and create a fresh device login.
    pub force_new_device: bool,
}

impl ClientRuntimeConfig {
    /// Load configuration from `$HOME/.mxdx/defaults.toml` and `$HOME/.mxdx/client.toml`.
    /// Falls back to defaults if files are missing.
    pub fn load() -> Result<Self> {
        let (defaults, client): (DefaultsConfig, ClientConfig) =
            load_merged_config("defaults.toml", "client.toml")?;
        Ok(Self {
            defaults,
            client,
            credentials: None,
            force_new_device: false,
        })
    }

    /// Construct directly from pre-built configs (useful for testing).
    pub fn from_parts(defaults: DefaultsConfig, client: ClientConfig) -> Self {
        Self {
            defaults,
            client,
            credentials: None,
            force_new_device: false,
        }
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
        self.force_new_device = args.force_new_device;

        // Build credentials if all three parts are present
        if let (Some(hs), Some(user), Some(pass)) =
            (&args.homeserver, &args.username, &args.password)
        {
            self.credentials = Some(ClientCredentials {
                homeserver: hs.clone(),
                username: user.clone(),
                password: pass.clone(),
            });
        }
        self
    }

    /// Require credentials or return an error.
    pub fn require_credentials(&self) -> Result<&ClientCredentials> {
        self.credentials.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "Matrix credentials required. Set --homeserver, --username, --password \
                 or MXDX_HOMESERVER, MXDX_USERNAME, MXDX_PASSWORD environment variables."
            )
        })
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

    /// Resolve accounts for a specific profile.
    /// If the profile specifies accounts, filter to just those.
    /// If not (or profile doesn't exist), use all accounts from defaults.
    pub fn resolve_accounts_for_profile(&self, profile: &str) -> Vec<mxdx_matrix::ServerAccount> {
        let profile_config = self.client.daemon.profiles.get(profile);
        let filter_accounts = profile_config.and_then(|p| p.accounts.as_ref());

        let all_accounts = self.resolve_accounts();

        match filter_accounts {
            Some(filter) => {
                all_accounts.into_iter()
                    .filter(|a| {
                        let user_at_server = format!("@{}:{}",
                            a.username,
                            a.homeserver
                                .trim_start_matches("https://")
                                .trim_start_matches("http://")
                        );
                        filter.iter().any(|f| f == &user_at_server)
                    })
                    .collect()
            }
            None => all_accounts,
        }
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
            homeserver: Some("matrix.example.com".into()),
            username: Some("alice".into()),
            password: Some("secret".into()),
            force_new_device: false,
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
        assert!(cfg.credentials.is_some());
        let creds = cfg.credentials.unwrap();
        assert_eq!(creds.homeserver, "matrix.example.com");
        assert_eq!(creds.username, "alice");
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
            homeserver: None,
            username: None,
            password: None,
            force_new_device: false,
        };
        let cfg = cfg.with_cli_overrides(&args);

        assert_eq!(cfg.client.default_worker_room, Some("keep-this".into()));
        assert_eq!(cfg.client.coordinator_room, Some("keep-this-too".into()));
        assert_eq!(cfg.client.session.timeout_seconds, Some(60));
        assert_eq!(cfg.client.session.heartbeat_interval, 30);
    }

    #[test]
    fn resolve_accounts_from_cli_credentials() {
        let defaults = DefaultsConfig::default();
        let client = ClientConfig::default();
        let cfg = ClientRuntimeConfig::from_parts(defaults, client);

        let args = ClientArgs {
            worker_room: None,
            coordinator_room: None,
            timeout: None,
            heartbeat_interval: None,
            interactive: false,
            no_room_output: false,
            homeserver: Some("https://matrix.example.com".into()),
            username: Some("alice".into()),
            password: Some("secret".into()),
            force_new_device: false,
        };
        let cfg = cfg.with_cli_overrides(&args);
        let accounts = cfg.resolve_accounts();

        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].homeserver, "https://matrix.example.com");
        assert_eq!(accounts[0].username, "alice");
    }

    #[test]
    fn resolve_accounts_from_config_with_password() {
        use mxdx_types::config::AccountConfig;

        let defaults = DefaultsConfig {
            accounts: vec![
                AccountConfig {
                    user_id: "@alice:server-a.com".into(),
                    homeserver: "https://server-a.com".into(),
                    password: Some("pass-a".into()),
                },
                AccountConfig {
                    user_id: "@alice:server-b.com".into(),
                    homeserver: "https://server-b.com".into(),
                    password: Some("pass-b".into()),
                },
            ],
            ..Default::default()
        };
        let client = ClientConfig::default();
        let cfg = ClientRuntimeConfig::from_parts(defaults, client);
        let accounts = cfg.resolve_accounts();

        assert_eq!(accounts.len(), 2);
        assert_eq!(accounts[0].username, "alice");
        assert_eq!(accounts[1].username, "alice");
    }

    #[test]
    fn resolve_accounts_empty_when_no_credentials() {
        let defaults = DefaultsConfig::default();
        let client = ClientConfig::default();
        let cfg = ClientRuntimeConfig::from_parts(defaults, client);
        let accounts = cfg.resolve_accounts();

        assert!(accounts.is_empty());
    }

    #[test]
    fn resolve_accounts_for_profile_filters_by_account() {
        use mxdx_types::config::{AccountConfig, DaemonConfig, ProfileConfig};
        use std::collections::HashMap;

        let defaults = DefaultsConfig {
            accounts: vec![
                AccountConfig {
                    user_id: "@alice:server-a.com".into(),
                    homeserver: "https://server-a.com".into(),
                    password: Some("pass-a".into()),
                },
                AccountConfig {
                    user_id: "@bob:server-b.com".into(),
                    homeserver: "https://server-b.com".into(),
                    password: Some("pass-b".into()),
                },
            ],
            ..Default::default()
        };

        let mut profiles = HashMap::new();
        profiles.insert("staging".to_string(), ProfileConfig {
            accounts: Some(vec!["@alice:server-a.com".to_string()]),
        });
        profiles.insert("default".to_string(), ProfileConfig {
            accounts: None,  // use all
        });

        let client = ClientConfig {
            daemon: DaemonConfig {
                profiles,
                ..Default::default()
            },
            ..Default::default()
        };

        let cfg = ClientRuntimeConfig::from_parts(defaults, client);

        // "default" profile with no filter → all accounts
        let all = cfg.resolve_accounts_for_profile("default");
        assert_eq!(all.len(), 2);

        // "staging" profile → only alice
        let staging = cfg.resolve_accounts_for_profile("staging");
        assert_eq!(staging.len(), 1);
        assert_eq!(staging[0].username, "alice");

        // unknown profile → all accounts (fallback)
        let unknown = cfg.resolve_accounts_for_profile("nonexistent");
        assert_eq!(unknown.len(), 2);
    }
}
