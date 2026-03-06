use serde::{Deserialize, Serialize};

/// Configuration for the mxdx-policy appservice.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyConfig {
    /// Homeserver base URL (e.g., "http://127.0.0.1:8008")
    pub homeserver_url: String,

    /// Appservice token (as_token) — used by the appservice to authenticate with the homeserver.
    pub as_token: String,

    /// Homeserver token (hs_token) — used by the homeserver to authenticate with the appservice.
    pub hs_token: String,

    /// The server name portion of user IDs (e.g., "example.com")
    pub server_name: String,

    /// The localpart of the appservice's sender user (e.g., "mxdx-policy")
    #[serde(default = "default_sender_localpart")]
    pub sender_localpart: String,

    /// The user namespace prefix claimed by this appservice (e.g., "agent-")
    #[serde(default = "default_user_prefix")]
    pub user_prefix: String,

    /// Port for the appservice HTTP listener (receives events from the homeserver)
    #[serde(default = "default_appservice_port")]
    pub appservice_port: u16,
}

fn default_sender_localpart() -> String {
    "mxdx-policy".to_string()
}

fn default_user_prefix() -> String {
    "agent-".to_string()
}

fn default_appservice_port() -> u16 {
    9100
}

impl PolicyConfig {
    /// Build the appservice URL that the homeserver will push events to.
    pub fn appservice_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.appservice_port)
    }

    /// Build the exclusive user namespace regex for this config.
    /// Matches `@{user_prefix}*:{server_name}`.
    pub fn user_namespace_regex(&self) -> String {
        format!(
            "@{}.*:{}",
            regex_escape(&self.user_prefix),
            regex_escape(&self.server_name)
        )
    }
}

/// Escape regex special characters for use in Matrix namespace patterns.
fn regex_escape(s: &str) -> String {
    let special = ['.', '^', '$', '+', '?', '(', ')', '[', ']', '{', '}', '|', '\\'];
    let mut result = String::with_capacity(s.len());
    for c in s.chars() {
        if special.contains(&c) {
            result.push('\\');
        }
        result.push(c);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_namespace_regex_escapes_dots() {
        let config = PolicyConfig {
            homeserver_url: "http://localhost:8008".to_string(),
            as_token: "as_token".to_string(),
            hs_token: "hs_token".to_string(),
            server_name: "example.com".to_string(),
            sender_localpart: "mxdx-policy".to_string(),
            user_prefix: "agent-".to_string(),
            appservice_port: 9100,
        };
        assert_eq!(config.user_namespace_regex(), "@agent-.*:example\\.com");
    }
}
