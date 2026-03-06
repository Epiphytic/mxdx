use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct LauncherConfig {
    pub global: GlobalConfig,
    #[serde(default)]
    pub homeservers: Vec<HomeserverConfig>,
    #[serde(default)]
    pub capabilities: CapabilitiesConfig,
    #[serde(default)]
    pub telemetry: TelemetryConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct GlobalConfig {
    #[serde(deserialize_with = "deserialize_non_empty_string")]
    pub launcher_id: String,
    #[serde(default)]
    pub data_dir: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct HomeserverConfig {
    pub url: String,
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Default)]
pub struct CapabilitiesConfig {
    #[serde(default)]
    pub mode: CapabilityMode,
    #[serde(default)]
    pub allowed_commands: Vec<String>,
    #[serde(default)]
    pub allowed_cwd_prefixes: Vec<String>,
    #[serde(default = "default_max_sessions")]
    pub max_sessions: u32,
}

fn default_max_sessions() -> u32 {
    5
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum CapabilityMode {
    #[default]
    Allowlist,
    Denylist,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct TelemetryConfig {
    #[serde(default)]
    pub detail_level: TelemetryDetail,
    #[serde(default = "default_poll_interval")]
    pub poll_interval_seconds: u64,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            detail_level: TelemetryDetail::default(),
            poll_interval_seconds: default_poll_interval(),
        }
    }
}

fn default_poll_interval() -> u64 {
    60
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum TelemetryDetail {
    #[default]
    Full,
    Summary,
}

/// Validates that the config file at `path` has secure permissions (0600).
/// Logs a warning if the file is readable by group or others.
pub fn validate_config_permissions(path: &std::path::Path) -> Result<(), anyhow::Error> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = std::fs::metadata(path)?;
    let mode = metadata.permissions().mode();
    if mode & 0o077 != 0 {
        tracing::warn!(
            path = %path.display(),
            mode = format!("{:04o}", mode & 0o777),
            "Config file is readable by group or others. Recommended: chmod 0600"
        );
    }
    Ok(())
}

fn deserialize_non_empty_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    if s.is_empty() {
        return Err(serde::de::Error::custom("value must not be empty"));
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_config_parses() {
        let toml = r#"
            [global]
            launcher_id = "belthanior"
            data_dir = "/tmp/mxdx"

            [[homeservers]]
            url = "https://hs1.example.com"
            username = "launcher-1"
            password = "secret"

            [capabilities]
            mode = "allowlist"
            allowed_commands = ["cargo", "git", "npm"]
            allowed_cwd_prefixes = ["/workspace"]
            max_sessions = 10

            [telemetry]
            detail_level = "full"
            poll_interval_seconds = 30
        "#;
        let config: LauncherConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.global.launcher_id, "belthanior");
        assert_eq!(config.capabilities.mode, CapabilityMode::Allowlist);
        assert_eq!(config.capabilities.allowed_cwd_prefixes, vec!["/workspace"]);
    }

    #[test]
    fn invalid_config_fails_fast() {
        let toml = r#"
            [global]
            launcher_id = ""
        "#;
        let result: Result<LauncherConfig, _> = toml::from_str(toml);
        assert!(result.is_err());
    }

    #[test]
    fn config_supports_telemetry_detail_levels() {
        let full_toml = r#"
            [global]
            launcher_id = "test"
            data_dir = "/tmp"
            [[homeservers]]
            url = "https://hs.example.com"
            username = "u"
            password = "p"
            [telemetry]
            detail_level = "summary"
        "#;
        let config: LauncherConfig = toml::from_str(full_toml).unwrap();
        assert_eq!(config.telemetry.detail_level, TelemetryDetail::Summary);
    }
}
