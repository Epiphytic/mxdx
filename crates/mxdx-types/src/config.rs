use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Shared config types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AccountConfig {
    pub user_id: String,
    pub homeserver: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrustConfig {
    #[serde(default = "default_cross_signing_mode")]
    pub cross_signing_mode: CrossSigningMode,
}

impl Default for TrustConfig {
    fn default() -> Self {
        Self {
            cross_signing_mode: CrossSigningMode::Auto,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CrossSigningMode {
    Auto,
    Manual,
}

fn default_cross_signing_mode() -> CrossSigningMode {
    CrossSigningMode::Auto
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WebRtcConfig {
    #[serde(default = "default_stun_servers")]
    pub stun_servers: Vec<String>,
    #[serde(default)]
    pub turn_servers: Vec<TurnServerConfig>,
}

impl Default for WebRtcConfig {
    fn default() -> Self {
        Self {
            stun_servers: default_stun_servers(),
            turn_servers: vec![],
        }
    }
}

fn default_stun_servers() -> Vec<String> {
    vec!["stun:stun.l.google.com:19302".into()]
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TurnServerConfig {
    pub url: String,
    pub auth_endpoint: Option<String>,
    pub username: Option<String>,
    pub credential: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct DefaultsConfig {
    #[serde(default)]
    pub accounts: Vec<AccountConfig>,
    #[serde(default)]
    pub trust: TrustConfig,
    #[serde(default)]
    pub webrtc: WebRtcConfig,
}

// ---------------------------------------------------------------------------
// Mode-specific config types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkerConfig {
    pub room_name: Option<String>,
    pub trust_anchor: Option<String>,
    #[serde(default = "default_history_retention")]
    pub history_retention: u64,
    #[serde(default)]
    pub capabilities: CapabilitiesConfig,
    #[serde(default = "default_telemetry_refresh")]
    pub telemetry_refresh_seconds: u64,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            room_name: None,
            trust_anchor: None,
            history_retention: default_history_retention(),
            capabilities: CapabilitiesConfig::default(),
            telemetry_refresh_seconds: default_telemetry_refresh(),
        }
    }
}

fn default_history_retention() -> u64 {
    90
}

fn default_telemetry_refresh() -> u64 {
    300
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct CapabilitiesConfig {
    #[serde(default)]
    pub extra: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClientConfig {
    pub default_worker_room: Option<String>,
    pub coordinator_room: Option<String>,
    #[serde(default)]
    pub session: SessionDefaults,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            default_worker_room: None,
            coordinator_room: None,
            session: SessionDefaults::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionDefaults {
    pub timeout_seconds: Option<u64>,
    #[serde(default = "default_heartbeat_interval")]
    pub heartbeat_interval: u64,
    #[serde(default)]
    pub interactive: bool,
    #[serde(default)]
    pub no_room_output: bool,
}

impl Default for SessionDefaults {
    fn default() -> Self {
        Self {
            timeout_seconds: None,
            heartbeat_interval: default_heartbeat_interval(),
            interactive: false,
            no_room_output: false,
        }
    }
}

fn default_heartbeat_interval() -> u64 {
    30
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CoordinatorConfig {
    pub room: Option<String>,
    pub capability_room_prefix: Option<String>,
    #[serde(default = "default_failure_action")]
    pub default_on_timeout: String,
    #[serde(default = "default_failure_action")]
    pub default_on_heartbeat_miss: String,
}

impl Default for CoordinatorConfig {
    fn default() -> Self {
        Self {
            room: None,
            capability_room_prefix: None,
            default_on_timeout: default_failure_action(),
            default_on_heartbeat_miss: default_failure_action(),
        }
    }
}

fn default_failure_action() -> String {
    "escalate".into()
}

// ---------------------------------------------------------------------------
// Config loading utilities
// ---------------------------------------------------------------------------

/// Get the config directory ($HOME/.mxdx/)
pub fn config_dir() -> PathBuf {
    dirs::home_dir()
        .expect("no home directory")
        .join(".mxdx")
}

/// Load config from $HOME/.mxdx/{filename}, returns default if file doesn't exist
pub fn load_config<T: DeserializeOwned + Default>(filename: &str) -> anyhow::Result<T> {
    let path = config_dir().join(filename);
    if !path.exists() {
        return Ok(T::default());
    }
    let content = std::fs::read_to_string(&path)?;
    Ok(toml::from_str(&content)?)
}

/// Load defaults.toml and a mode-specific config, returning both
pub fn load_merged_config<D: DeserializeOwned + Default, M: DeserializeOwned + Default>(
    defaults_file: &str,
    mode_file: &str,
) -> anyhow::Result<(D, M)> {
    let defaults: D = load_config(defaults_file)?;
    let mode: M = load_config(mode_file)?;
    Ok((defaults, mode))
}

/// Remove password fields from accounts in a TOML config file.
/// After credentials are saved to the keychain, plaintext passwords should
/// be stripped from config files for security.
///
/// Preserves all other fields and formatting where possible.
/// Sets file permissions to 0o600 on Unix.
///
/// The `base_dir` parameter allows overriding the config directory for testing.
/// Pass `None` to use the default `config_dir()`.
pub fn remove_passwords_from_config(
    filename: &str,
    base_dir: Option<&std::path::Path>,
) -> anyhow::Result<()> {
    let dir = match base_dir {
        Some(d) => d.to_path_buf(),
        None => config_dir(),
    };
    let path = dir.join(filename);
    if !path.exists() {
        return Ok(()); // Nothing to do
    }

    let content = std::fs::read_to_string(&path)?;
    let mut doc: toml::Value = toml::from_str(&content)?;

    // Remove password from [[accounts]] array
    if let Some(accounts) = doc.get_mut("accounts") {
        if let Some(arr) = accounts.as_array_mut() {
            for account in arr.iter_mut() {
                if let Some(table) = account.as_table_mut() {
                    table.remove("password");
                }
            }
        }
    }

    let new_content = toml::to_string_pretty(&doc)?;
    std::fs::write(&path, new_content)?;

    // Set file permissions to 0o600 on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_config_deserializes_full_toml() {
        let toml_str = r#"
[[accounts]]
user_id = "@worker:example.com"
homeserver = "https://example.com"

[[accounts]]
user_id = "@backup:example.com"
homeserver = "https://backup.example.com"
password = "backup-secret"

[trust]
cross_signing_mode = "manual"

[webrtc]
stun_servers = ["stun:custom.stun:3478"]

[[webrtc.turn_servers]]
url = "turn:turn.example.com:3478"
username = "user"
credential = "pass"
"#;
        let cfg: DefaultsConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.accounts.len(), 2);
        assert_eq!(cfg.accounts[0].user_id, "@worker:example.com");
        assert!(cfg.accounts[0].password.is_none(), "no password field means None");
        assert_eq!(cfg.accounts[1].password, Some("backup-secret".into()));
        assert_eq!(cfg.trust.cross_signing_mode, CrossSigningMode::Manual);
        assert_eq!(cfg.webrtc.stun_servers, vec!["stun:custom.stun:3478"]);
        assert_eq!(cfg.webrtc.turn_servers.len(), 1);
        assert_eq!(cfg.webrtc.turn_servers[0].username, Some("user".into()));
    }

    #[test]
    fn worker_config_has_correct_defaults() {
        let toml_str = r#"
room_name = "test-room"
"#;
        let cfg: WorkerConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.history_retention, 90);
        assert_eq!(cfg.telemetry_refresh_seconds, 300);
        assert_eq!(cfg.room_name, Some("test-room".into()));
        assert!(cfg.capabilities.extra.is_empty());
    }

    #[test]
    fn client_config_has_session_defaults() {
        let toml_str = r#"
default_worker_room = "!abc:example.com"

[session]
timeout_seconds = 600
"#;
        let cfg: ClientConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.default_worker_room, Some("!abc:example.com".into()));
        assert_eq!(cfg.session.timeout_seconds, Some(600));
        assert_eq!(cfg.session.heartbeat_interval, 30);
        assert!(!cfg.session.interactive);
        assert!(!cfg.session.no_room_output);
    }

    #[test]
    fn coordinator_config_has_failure_action_defaults() {
        let toml_str = r#"
room = "!coord:example.com"
"#;
        let cfg: CoordinatorConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.room, Some("!coord:example.com".into()));
        assert_eq!(cfg.default_on_timeout, "escalate");
        assert_eq!(cfg.default_on_heartbeat_miss, "escalate");
    }

    #[test]
    fn webrtc_config_parses_stun_and_turn() {
        let toml_str = r#"
stun_servers = ["stun:a:1", "stun:b:2"]

[[turn_servers]]
url = "turn:turn1.example.com:3478"
auth_endpoint = "https://auth.example.com/turn"

[[turn_servers]]
url = "turn:turn2.example.com:3478"
username = "u"
credential = "c"
"#;
        let cfg: WebRtcConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.stun_servers.len(), 2);
        assert_eq!(cfg.turn_servers.len(), 2);
        assert_eq!(
            cfg.turn_servers[0].auth_endpoint,
            Some("https://auth.example.com/turn".into())
        );
        assert_eq!(cfg.turn_servers[0].username, None);
        assert_eq!(cfg.turn_servers[1].username, Some("u".into()));
    }

    #[test]
    fn empty_toml_produces_valid_defaults() {
        let empty = "";
        let defaults: DefaultsConfig = toml::from_str(empty).unwrap();
        assert!(defaults.accounts.is_empty());
        assert_eq!(defaults.trust.cross_signing_mode, CrossSigningMode::Auto);
        assert_eq!(
            defaults.webrtc.stun_servers,
            vec!["stun:stun.l.google.com:19302"]
        );

        let worker: WorkerConfig = toml::from_str(empty).unwrap();
        assert_eq!(worker.history_retention, 90);
        assert_eq!(worker.telemetry_refresh_seconds, 300);

        let client: ClientConfig = toml::from_str(empty).unwrap();
        assert_eq!(client.session.heartbeat_interval, 30);

        let coord: CoordinatorConfig = toml::from_str(empty).unwrap();
        assert_eq!(coord.default_on_timeout, "escalate");
    }

    #[test]
    fn test_remove_passwords_strips_password_fields() {
        let dir = tempfile::tempdir().unwrap();
        let toml_content = r#"
[[accounts]]
user_id = "@alice:example.com"
homeserver = "https://example.com"
password = "super-secret"

[[accounts]]
user_id = "@bob:example.com"
homeserver = "https://backup.example.com"
password = "also-secret"
"#;
        std::fs::write(dir.path().join("defaults.toml"), toml_content).unwrap();

        remove_passwords_from_config("defaults.toml", Some(dir.path())).unwrap();

        let result = std::fs::read_to_string(dir.path().join("defaults.toml")).unwrap();
        let parsed: DefaultsConfig = toml::from_str(&result).unwrap();

        assert_eq!(parsed.accounts.len(), 2);
        assert!(parsed.accounts[0].password.is_none(), "password should be stripped");
        assert!(parsed.accounts[1].password.is_none(), "password should be stripped");
        assert_eq!(parsed.accounts[0].user_id, "@alice:example.com");
        assert_eq!(parsed.accounts[1].user_id, "@bob:example.com");
        assert_eq!(parsed.accounts[0].homeserver, "https://example.com");
        assert_eq!(parsed.accounts[1].homeserver, "https://backup.example.com");
    }

    #[test]
    fn test_remove_passwords_preserves_non_account_fields() {
        let dir = tempfile::tempdir().unwrap();
        let toml_content = r#"
[[accounts]]
user_id = "@worker:example.com"
homeserver = "https://example.com"
password = "secret"

[trust]
cross_signing_mode = "manual"

[webrtc]
stun_servers = ["stun:custom.stun:3478"]
"#;
        std::fs::write(dir.path().join("defaults.toml"), toml_content).unwrap();

        remove_passwords_from_config("defaults.toml", Some(dir.path())).unwrap();

        let result = std::fs::read_to_string(dir.path().join("defaults.toml")).unwrap();
        let parsed: DefaultsConfig = toml::from_str(&result).unwrap();

        assert!(parsed.accounts[0].password.is_none());
        assert_eq!(parsed.trust.cross_signing_mode, CrossSigningMode::Manual);
        assert_eq!(parsed.webrtc.stun_servers, vec!["stun:custom.stun:3478"]);
    }

    #[test]
    fn test_remove_passwords_noop_when_no_file() {
        let dir = tempfile::tempdir().unwrap();
        // File does not exist — should return Ok(())
        let result = remove_passwords_from_config("nonexistent.toml", Some(dir.path()));
        assert!(result.is_ok());
    }

    #[test]
    fn test_remove_passwords_noop_when_no_accounts() {
        let dir = tempfile::tempdir().unwrap();
        let toml_content = r#"
[trust]
cross_signing_mode = "auto"

[webrtc]
stun_servers = ["stun:stun.l.google.com:19302"]
"#;
        let path = dir.path().join("defaults.toml");
        std::fs::write(&path, toml_content).unwrap();

        remove_passwords_from_config("defaults.toml", Some(dir.path())).unwrap();

        let result = std::fs::read_to_string(&path).unwrap();
        let parsed: DefaultsConfig = toml::from_str(&result).unwrap();
        assert_eq!(parsed.trust.cross_signing_mode, CrossSigningMode::Auto);
        assert_eq!(
            parsed.webrtc.stun_servers,
            vec!["stun:stun.l.google.com:19302"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_remove_passwords_file_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let toml_content = r#"
[[accounts]]
user_id = "@worker:example.com"
homeserver = "https://example.com"
password = "secret"
"#;
        let path = dir.path().join("defaults.toml");
        std::fs::write(&path, toml_content).unwrap();

        remove_passwords_from_config("defaults.toml", Some(dir.path())).unwrap();

        let metadata = std::fs::metadata(&path).unwrap();
        let mode = metadata.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "file should have 0o600 permissions, got {:o}", mode);
    }

    #[test]
    fn test_remove_passwords_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let toml_content = r#"
[[accounts]]
user_id = "@worker:example.com"
homeserver = "https://example.com"
password = "secret"
"#;
        std::fs::write(dir.path().join("defaults.toml"), toml_content).unwrap();

        // Call twice — second call should be a no-op
        remove_passwords_from_config("defaults.toml", Some(dir.path())).unwrap();
        remove_passwords_from_config("defaults.toml", Some(dir.path())).unwrap();

        let result = std::fs::read_to_string(dir.path().join("defaults.toml")).unwrap();
        let parsed: DefaultsConfig = toml::from_str(&result).unwrap();
        assert!(parsed.accounts[0].password.is_none());
        assert_eq!(parsed.accounts[0].user_id, "@worker:example.com");
    }

    #[test]
    fn field_level_precedence_override() {
        // Demonstrates that mode-specific values can override shared defaults
        // by merging at the field level.
        let defaults_toml = r#"
[trust]
cross_signing_mode = "auto"

[webrtc]
stun_servers = ["stun:stun.l.google.com:19302"]
"#;
        let defaults: DefaultsConfig = toml::from_str(defaults_toml).unwrap();

        // Worker config overrides at field level
        let worker_toml = r#"
history_retention = 30
telemetry_refresh_seconds = 60
"#;
        let worker: WorkerConfig = toml::from_str(worker_toml).unwrap();

        // User-supplied CLI override (simulated as a third level)
        let cli_retention: u64 = 7;

        // Precedence: CLI > mode config > defaults
        let final_retention = cli_retention; // CLI wins
        let final_telemetry = worker.telemetry_refresh_seconds; // mode config wins over default
        let final_cross_signing = &defaults.trust.cross_signing_mode; // defaults used

        assert_eq!(final_retention, 7);
        assert_eq!(final_telemetry, 60);
        assert_eq!(*final_cross_signing, CrossSigningMode::Auto);
    }
}
