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
    #[serde(default = "default_max_sessions")]
    pub max_sessions: u32,
    #[serde(default)]
    pub allowed_commands: Vec<String>,
    #[serde(default = "default_allowed_cwd")]
    pub allowed_cwd: Vec<String>,
    #[serde(default)]
    pub authorized_users: Vec<String>,
    /// P2P transport feature flag and tuning (Phase 6 wiring; default off per §4.7).
    #[serde(default)]
    pub p2p: P2pConfig,
    // npm-only fields (ADR 2026-04-29 Pillar 1, req 3) — Option<T> with serde(default)
    // so existing Rust configs without these keys continue to deserialize correctly.
    #[serde(default)]
    pub telemetry: Option<String>,
    #[serde(default)]
    pub use_tmux: Option<String>,
    #[serde(default)]
    pub batch_ms: Option<u64>,
    #[serde(default)]
    pub p2p_batch_ms: Option<u64>,
    #[serde(default)]
    pub p2p_advertise_ips: Option<bool>,
    #[serde(default)]
    pub p2p_turn_only: Option<bool>,
    #[serde(default)]
    pub registration_token: Option<String>,
    #[serde(default)]
    pub admin_user: Option<String>,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            room_name: None,
            trust_anchor: None,
            history_retention: default_history_retention(),
            capabilities: CapabilitiesConfig::default(),
            telemetry_refresh_seconds: default_telemetry_refresh(),
            max_sessions: default_max_sessions(),
            allowed_commands: Vec::new(),
            allowed_cwd: default_allowed_cwd(),
            authorized_users: Vec::new(),
            p2p: P2pConfig::default(),
            telemetry: None,
            use_tmux: None,
            batch_ms: None,
            p2p_batch_ms: None,
            p2p_advertise_ips: None,
            p2p_turn_only: None,
            registration_token: None,
            admin_user: None,
        }
    }
}

/// P2P transport config (storm §4.7). `enabled` defaults to **true** —
/// flipped from false in Phase-9 T-91 after three consecutive green
/// nightly perf runs. With `enabled = false` (or `--no-p2p` CLI flag),
/// worker/client behave exactly as before Phase 6 (no regression).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct P2pConfig {
    /// Master feature flag. Default: true (Phase-9 T-91). When false,
    /// P2PTransport is never constructed and all send paths use Matrix.
    /// Override at runtime with `--no-p2p` (client) or `p2p.enabled = false`
    /// in worker.toml / client.toml.
    #[serde(default = "default_p2p_enabled")]
    pub enabled: bool,
    /// Optional idle timeout override (seconds). Default: 300 (5 minutes).
    #[serde(default = "default_p2p_idle_timeout_seconds")]
    pub idle_timeout_seconds: u64,
}

impl Default for P2pConfig {
    fn default() -> Self {
        Self {
            enabled: default_p2p_enabled(),
            idle_timeout_seconds: default_p2p_idle_timeout_seconds(),
        }
    }
}

fn default_p2p_enabled() -> bool {
    true
}

fn default_p2p_idle_timeout_seconds() -> u64 {
    300
}

fn default_history_retention() -> u64 {
    90
}

fn default_telemetry_refresh() -> u64 {
    60
}

fn default_max_sessions() -> u32 {
    5
}

fn default_allowed_cwd() -> Vec<String> {
    vec!["/tmp".to_string()]
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
    #[serde(default)]
    pub daemon: DaemonConfig,
    /// P2P transport feature flag and tuning (Phase 6 wiring; default off per §4.7).
    #[serde(default)]
    pub p2p: P2pConfig,
    // npm-only fields (ADR 2026-04-29 Pillar 1, req 3)
    #[serde(default)]
    pub batch_ms: Option<u64>,
    #[serde(default)]
    pub p2p_batch_ms: Option<u64>,
    #[serde(default)]
    pub registration_token: Option<String>,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            default_worker_room: None,
            coordinator_room: None,
            session: SessionDefaults::default(),
            daemon: DaemonConfig::default(),
            p2p: P2pConfig::default(),
            batch_ms: None,
            p2p_batch_ms: None,
            registration_token: None,
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
// Daemon config types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DaemonConfig {
    #[serde(default = "default_idle_timeout")]
    pub idle_timeout_seconds: u64,
    #[serde(default)]
    pub profiles: std::collections::HashMap<String, ProfileConfig>,
    #[serde(default)]
    pub websocket: Option<WebSocketTransportConfig>,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            idle_timeout_seconds: default_idle_timeout(),
            profiles: std::collections::HashMap::new(),
            websocket: None,
        }
    }
}

fn default_idle_timeout() -> u64 {
    1200
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ProfileConfig {
    #[serde(default)]
    pub accounts: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WebSocketTransportConfig {
    #[serde(default = "default_ws_bind")]
    pub bind: String,
    #[serde(default = "default_ws_port")]
    pub port: u16,
}

fn default_ws_bind() -> String {
    "127.0.0.1".into()
}

fn default_ws_port() -> u16 {
    9390
}

// ---------------------------------------------------------------------------
// Config loading utilities
// ---------------------------------------------------------------------------

/// Get the config directory ($HOME/.mxdx/)
pub fn config_dir() -> PathBuf {
    dirs::home_dir().expect("no home directory").join(".mxdx")
}

/// Detect legacy `[launcher]` or `[client]` section wrappers in a config file.
/// If detected: write `<path>.legacy.bak`, rewrite the file as flat top-level keys,
/// log a warning to stderr, and return the migrated content.
/// If not detected: return the original content unchanged.
///
/// This implements ADR 2026-04-29 req 6a: migration must occur before any
/// security-critical field values are parsed, to prevent silent zero-fielding.
pub fn migrate_legacy_section_if_needed(path: &std::path::Path) -> anyhow::Result<String> {
    let content = std::fs::read_to_string(path)?;

    // Fast path: check if any legacy section header exists
    let has_launcher = content.lines().any(|l| l.trim() == "[launcher]");
    let has_client = content.lines().any(|l| l.trim() == "[client]");

    if !has_launcher && !has_client {
        return Ok(content);
    }

    let section_name = if has_launcher { "launcher" } else { "client" };

    // Parse TOML and extract the section table
    let doc: toml::Value = toml::from_str(&content).map_err(|e| {
        anyhow::anyhow!("Failed to parse legacy config at {}: {}", path.display(), e)
    })?;

    let section_table = doc
        .get(section_name)
        .and_then(|v| v.as_table())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Config at {} has [{}] header but could not extract section table",
                path.display(),
                section_name
            )
        })?;

    // Build flat top-level TOML from the section's fields
    let flat_value = toml::Value::Table(section_table.clone());
    let migrated = toml::to_string_pretty(&flat_value)
        .map_err(|e| anyhow::anyhow!("Failed to serialize migrated config: {}", e))?;

    // Write the legacy backup before modifying anything
    let bak_path = {
        let mut p = path.to_path_buf();
        let name = p
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("config")
            .to_string();
        p.set_file_name(format!("{}.legacy.bak", name));
        p
    };
    std::fs::write(&bak_path, &content)?;
    #[cfg(unix)]
    {
        // Preserve original file permissions on the backup
        if let Ok(meta) = std::fs::metadata(path) {
            let _ = std::fs::set_permissions(&bak_path, meta.permissions());
        }
    }

    // Overwrite the original file with flat-key layout
    std::fs::write(path, &migrated)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }

    eprintln!(
        "mxdx: WARNING: config file {} used legacy [{}] section wrapper. \
         Migrated to flat-key layout. Original saved to {}. \
         See ADR docs/adr/2026-04-29-rust-npm-binary-parity.md for details.",
        path.display(),
        section_name,
        bak_path.display()
    );

    Ok(migrated)
}

/// Load config from $HOME/.mxdx/{filename}, returns default if file doesn't exist
pub fn load_config<T: DeserializeOwned + Default>(filename: &str) -> anyhow::Result<T> {
    load_config_from_dir(filename, &config_dir())
}

/// Load config from a specific directory (testable variant).
pub fn load_config_from_dir<T: DeserializeOwned + Default>(
    filename: &str,
    dir: &std::path::Path,
) -> anyhow::Result<T> {
    let path = dir.join(filename);
    if !path.exists() {
        return Ok(T::default());
    }
    let content = migrate_legacy_section_if_needed(&path)?;
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
        assert!(
            cfg.accounts[0].password.is_none(),
            "no password field means None"
        );
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
        assert_eq!(cfg.telemetry_refresh_seconds, 60);
        assert_eq!(cfg.room_name, Some("test-room".into()));
        assert!(cfg.capabilities.extra.is_empty());
        assert_eq!(cfg.max_sessions, 5);
        assert!(cfg.allowed_commands.is_empty());
        assert_eq!(cfg.allowed_cwd, vec!["/tmp"]);
        assert!(cfg.authorized_users.is_empty());
    }

    #[test]
    fn worker_config_security_fields_from_toml() {
        let toml_str = r#"
max_sessions = 10
allowed_commands = ["echo", "ls", "cat"]
allowed_cwd = ["/tmp", "/home/worker"]
authorized_users = ["@admin:example.com", "@ops:example.com"]
"#;
        let cfg: WorkerConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.max_sessions, 10);
        assert_eq!(cfg.allowed_commands, vec!["echo", "ls", "cat"]);
        assert_eq!(cfg.allowed_cwd, vec!["/tmp", "/home/worker"]);
        assert_eq!(
            cfg.authorized_users,
            vec!["@admin:example.com", "@ops:example.com"]
        );
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
        assert_eq!(worker.telemetry_refresh_seconds, 60);
        assert_eq!(worker.max_sessions, 5);
        assert!(worker.allowed_commands.is_empty());
        assert_eq!(worker.allowed_cwd, vec!["/tmp"]);
        assert!(worker.authorized_users.is_empty());

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
        assert!(
            parsed.accounts[0].password.is_none(),
            "password should be stripped"
        );
        assert!(
            parsed.accounts[1].password.is_none(),
            "password should be stripped"
        );
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
        assert_eq!(
            mode, 0o600,
            "file should have 0o600 permissions, got {:o}",
            mode
        );
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
    fn daemon_config_deserializes_with_defaults() {
        let toml_str = "";
        let cfg: DaemonConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.idle_timeout_seconds, 1200);
        assert!(cfg.profiles.is_empty());
    }

    #[test]
    fn daemon_config_with_profiles() {
        let toml_str = r#"
idle_timeout_seconds = 0

[profiles.default]

[profiles.staging]
accounts = ["@worker:staging.mxdx.dev"]
"#;
        let cfg: DaemonConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.idle_timeout_seconds, 0);
        assert_eq!(cfg.profiles.len(), 2);
        assert!(cfg.profiles["default"].accounts.is_none());
        assert_eq!(cfg.profiles["staging"].accounts.as_ref().unwrap().len(), 1);
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

    // T-3.2: legacy-section migration tests

    #[test]
    fn migrate_legacy_launcher_section_rewrites_flat_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("worker.toml");
        let legacy = r#"
[launcher]
username = "belthanior"
allowed_commands = ["echo", "ls"]
allowed_cwd = ["/tmp"]
max_sessions = 3
"#;
        std::fs::write(&path, legacy).unwrap();

        let migrated = migrate_legacy_section_if_needed(&path).unwrap();

        // Flat-key TOML parses as WorkerConfig without error
        let cfg: WorkerConfig = toml::from_str(&migrated).unwrap();
        assert_eq!(cfg.max_sessions, 3);
        assert_eq!(cfg.allowed_commands, vec!["echo", "ls"]);
        assert_eq!(cfg.allowed_cwd, vec!["/tmp"]);

        // .legacy.bak preserves original content byte-for-byte
        let bak = std::fs::read_to_string(dir.path().join("worker.toml.legacy.bak")).unwrap();
        assert_eq!(bak, legacy);

        // File on disk is now the flat-key version
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert_eq!(on_disk, migrated);
    }

    #[test]
    fn migrate_legacy_client_section_rewrites_flat_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("client.toml");
        let legacy = r#"
[client]
username = "liamhelmer"
servers = ["https://matrix.org"]
batch_ms = 100
"#;
        std::fs::write(&path, legacy).unwrap();

        let migrated = migrate_legacy_section_if_needed(&path).unwrap();

        let cfg: ClientConfig = toml::from_str(&migrated).unwrap();
        assert_eq!(cfg.batch_ms, Some(100));

        let bak = std::fs::read_to_string(dir.path().join("client.toml.legacy.bak")).unwrap();
        assert_eq!(bak, legacy);
    }

    #[test]
    fn migrate_legacy_no_section_passthrough() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("worker.toml");
        let flat = r#"max_sessions = 7
allowed_commands = ["cat"]
"#;
        std::fs::write(&path, flat).unwrap();

        let result = migrate_legacy_section_if_needed(&path).unwrap();
        assert_eq!(result, flat);

        // No .legacy.bak written
        assert!(!dir.path().join("worker.toml.legacy.bak").exists());
    }

    /// Security-critical field survival test (ADR 2026-04-29 req 6a, T-3.2 blocker):
    /// authorized_users, allowed_commands, and trust_anchor MUST be byte-for-byte
    /// identical in the migrated flat-key output. Silent loss of any field is a
    /// security defect — it would silently open authorization to all users.
    #[test]
    fn migrate_security_critical_fields_survive_migration() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("worker.toml");

        // Representative legacy [launcher]-wrapped config containing all three
        // security-critical fields from ADR req 6a.
        let legacy = r#"
[launcher]
authorized_users = ["@alice:example.com", "@bob:example.com"]
allowed_commands = ["echo", "ls", "cat"]
trust_anchor = "@admin:example.com"
max_sessions = 5
"#;
        std::fs::write(&path, legacy).unwrap();

        let migrated = migrate_legacy_section_if_needed(&path).unwrap();
        let cfg: WorkerConfig = toml::from_str(&migrated).unwrap();

        // authorized_users: byte-for-byte identical values
        assert_eq!(
            cfg.authorized_users,
            vec!["@alice:example.com", "@bob:example.com"],
            "authorized_users must survive migration intact"
        );

        // allowed_commands: byte-for-byte identical values
        assert_eq!(
            cfg.allowed_commands,
            vec!["echo", "ls", "cat"],
            "allowed_commands must survive migration intact"
        );

        // trust_anchor: byte-for-byte identical value
        assert_eq!(
            cfg.trust_anchor,
            Some("@admin:example.com".to_string()),
            "trust_anchor must survive migration intact"
        );

        // Confirm the .legacy.bak contains the original content
        let bak = std::fs::read_to_string(dir.path().join("worker.toml.legacy.bak")).unwrap();
        assert_eq!(bak, legacy, ".legacy.bak must be byte-for-byte original");
    }

    #[test]
    fn load_config_from_dir_migrates_legacy_on_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("worker.toml");

        let legacy = r#"
[launcher]
authorized_users = ["@ops:example.com"]
allowed_commands = ["echo"]
trust_anchor = "@trust:example.com"
max_sessions = 2
"#;
        std::fs::write(&path, legacy).unwrap();

        let cfg: WorkerConfig = load_config_from_dir("worker.toml", dir.path()).unwrap();

        assert_eq!(cfg.authorized_users, vec!["@ops:example.com"]);
        assert_eq!(cfg.allowed_commands, vec!["echo"]);
        assert_eq!(cfg.trust_anchor, Some("@trust:example.com".to_string()));
        assert_eq!(cfg.max_sessions, 2);

        // After load, file should be in flat format, bak should exist
        assert!(dir.path().join("worker.toml.legacy.bak").exists());
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(
            !on_disk.contains("[launcher]"),
            "migrated file must not contain [launcher]"
        );
    }
}
