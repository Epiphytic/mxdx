use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::config::PolicyConfig;

/// Appservice registration ID used for Tuwunel.
const APPSERVICE_ID: &str = "mxdx-policy";

/// Trait for registering an appservice with a Matrix homeserver.
/// Implementations handle the server-specific mechanism (admin commands, file-based, etc.).
pub trait AppserviceRegistrar: Send + Sync {
    fn register(
        &self,
        registration: &AppserviceRegistration,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send;
}

/// Registers an appservice with Tuwunel by sending the YAML via the admin room.
pub struct TuwunelRegistrar {
    pub homeserver_url: String,
    pub admin_access_token: String,
}

impl AppserviceRegistrar for TuwunelRegistrar {
    async fn register(&self, registration: &AppserviceRegistration) -> anyhow::Result<()> {
        register_appservice_tuwunel(&self.homeserver_url, &self.admin_access_token, registration)
            .await
    }
}

/// For servers where the appservice is already registered out-of-band.
/// Validates that the registration YAML file exists and contains required fields.
pub struct ManualRegistrar {
    pub registration_path: std::path::PathBuf,
}

impl AppserviceRegistrar for ManualRegistrar {
    async fn register(&self, _registration: &AppserviceRegistration) -> anyhow::Result<()> {
        let path = &self.registration_path;
        if !path.exists() {
            anyhow::bail!(
                "Manual registration file not found: {}",
                path.display()
            );
        }

        let content = std::fs::read_to_string(path)?;
        validate_registration_yaml(&content, path)?;

        Ok(())
    }
}

/// Check that a registration YAML string contains the required fields.
fn validate_registration_yaml(content: &str, path: &Path) -> anyhow::Result<()> {
    for field in ["id:", "as_token:", "hs_token:"] {
        if !content.contains(field) {
            anyhow::bail!(
                "Registration file {} missing required field '{}'",
                path.display(),
                field.trim_end_matches(':')
            );
        }
    }
    Ok(())
}

/// Matrix appservice registration document.
/// Serializes to the YAML format expected by Tuwunel's `appservices register` command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppserviceRegistration {
    pub id: String,
    pub url: String,
    pub as_token: String,
    pub hs_token: String,
    pub sender_localpart: String,
    pub namespaces: Namespaces,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Namespaces {
    pub users: Vec<NamespaceEntry>,
    pub rooms: Vec<NamespaceEntry>,
    pub aliases: Vec<NamespaceEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamespaceEntry {
    pub exclusive: bool,
    pub regex: String,
}

impl AppserviceRegistration {
    /// Build an appservice registration from a PolicyConfig.
    /// Claims the `@agent-*` namespace exclusively.
    pub fn from_config(config: &PolicyConfig) -> Self {
        AppserviceRegistration {
            id: APPSERVICE_ID.to_string(),
            url: config.appservice_url(),
            as_token: config.as_token.clone(),
            hs_token: config.hs_token.clone(),
            sender_localpart: config.sender_localpart.clone(),
            namespaces: Namespaces {
                users: vec![NamespaceEntry {
                    exclusive: true,
                    regex: config.user_namespace_regex(),
                }],
                rooms: vec![],
                aliases: vec![],
            },
        }
    }

    /// Serialize the registration to YAML (for Tuwunel admin commands).
    pub fn to_yaml(&self) -> Result<String, serde_json::Error> {
        // We use serde_json to build the structure, then manually format YAML
        // since we don't want to add a serde_yaml dependency.
        Ok(self.format_yaml())
    }

    /// Format the registration as YAML without requiring serde_yaml.
    fn format_yaml(&self) -> String {
        let mut yaml = String::new();
        yaml.push_str(&format!("id: {}\n", self.id));
        yaml.push_str(&format!("url: \"{}\"\n", self.url));
        yaml.push_str(&format!("as_token: \"{}\"\n", self.as_token));
        yaml.push_str(&format!("hs_token: \"{}\"\n", self.hs_token));
        yaml.push_str(&format!("sender_localpart: \"{}\"\n", self.sender_localpart));
        yaml.push_str("namespaces:\n");
        yaml.push_str("  users:\n");
        for entry in &self.namespaces.users {
            yaml.push_str(&format!("    - exclusive: {}\n", entry.exclusive));
            yaml.push_str(&format!("      regex: '{}'\n", entry.regex));
        }
        yaml.push_str("  rooms: []\n");
        yaml.push_str("  aliases: []\n");
        yaml
    }
}

/// Convenience wrapper: registers via `TuwunelRegistrar` for backward compatibility.
pub async fn register_appservice(
    homeserver_url: &str,
    admin_access_token: &str,
    registration: &AppserviceRegistration,
) -> anyhow::Result<()> {
    let registrar = TuwunelRegistrar {
        homeserver_url: homeserver_url.to_string(),
        admin_access_token: admin_access_token.to_string(),
    };
    registrar.register(registration).await
}

/// Register the appservice with a Tuwunel instance by sending the YAML to the admin room.
/// The `admin_client` must be the first registered user (server admin) with a valid access token.
///
/// This sends the `!admin appservices register` command followed by the YAML content
/// to the `#admins` room.
async fn register_appservice_tuwunel(
    homeserver_url: &str,
    admin_access_token: &str,
    registration: &AppserviceRegistration,
) -> anyhow::Result<()> {
    let http_client = reqwest::Client::new();

    // Find the #admins room
    let admin_room_id = find_admin_room(homeserver_url, admin_access_token, &http_client).await?;

    // Send the appservice registration command.
    // Tuwunel requires the YAML to be in a markdown code block.
    let yaml = registration.format_yaml();
    let message = format!("!admin appservices register\n```yaml\n{yaml}```");

    let send_url = format!(
        "{homeserver_url}/_matrix/client/v3/rooms/{room_id}/send/m.room.message/{txn_id}",
        room_id = urlencoded(&admin_room_id),
        txn_id = uuid::Uuid::new_v4(),
    );

    let body = serde_json::json!({
        "msgtype": "m.text",
        "body": message,
    });

    let resp = http_client
        .put(&send_url)
        .bearer_auth(admin_access_token)
        .json(&body)
        .send()
        .await?;

    if !resp.status().is_success() {
        let err_body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Failed to send appservice registration command: {err_body}");
    }

    // Wait for Tuwunel to process the admin command, then verify it succeeded
    // by syncing and checking for a confirmation response from the admin bot.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut since: Option<String> = None;
    let sync_url = format!("{homeserver_url}/_matrix/client/v3/sync");

    while tokio::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let mut params = vec![("timeout".to_string(), "1000".to_string())];
        if let Some(ref token) = since {
            params.push(("since".to_string(), token.clone()));
        }

        let sync_resp = http_client
            .get(&sync_url)
            .bearer_auth(admin_access_token)
            .query(&params)
            .send()
            .await?;

        let body: serde_json::Value = sync_resp.json().await?;
        if let Some(next) = body["next_batch"].as_str() {
            since = Some(next.to_string());
        }

        // Look for the admin bot's response in timeline events
        if let Some(joined) = body["rooms"]["join"].as_object() {
            for (_room_id, room_data) in joined {
                if let Some(events) = room_data["timeline"]["events"].as_array() {
                    for event in events {
                        if let Some(msg_body) = event["content"]["body"].as_str() {
                            if msg_body.contains("Appservice registered") {
                                return Ok(());
                            }
                            if msg_body.contains("Command failed") {
                                anyhow::bail!(
                                    "Appservice registration failed: {}",
                                    msg_body
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    anyhow::bail!("Timed out waiting for appservice registration confirmation")
}

/// Find the #admins room ID by syncing and looking for it.
async fn find_admin_room(
    homeserver_url: &str,
    access_token: &str,
    client: &reqwest::Client,
) -> anyhow::Result<String> {
    let sync_url = format!("{homeserver_url}/_matrix/client/v3/sync");

    let resp = client
        .get(&sync_url)
        .bearer_auth(access_token)
        .query(&[("timeout", "1000")])
        .send()
        .await?;

    if !resp.status().is_success() {
        let err_body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Sync failed: {err_body}");
    }

    let body: serde_json::Value = resp.json().await?;

    // Look through joined rooms for the #admins room.
    // The admin room's canonical alias is typically #admins:<server_name>
    if let Some(joined) = body["rooms"]["join"].as_object() {
        for (room_id, room_data) in joined {
            // Check state events for room name or canonical alias
            if let Some(events) = room_data["state"]["events"].as_array() {
                for event in events {
                    let event_type = event["type"].as_str().unwrap_or_default();
                    if event_type == "m.room.canonical_alias" {
                        if let Some(alias) = event["content"]["alias"].as_str() {
                            if alias.starts_with("#admins:") {
                                return Ok(room_id.clone());
                            }
                        }
                    }
                }
            }
        }

        // Fallback: if there's only one joined room, it's likely the admin room
        // (first registered user auto-joins only #admins)
        let rooms: Vec<&String> = joined.keys().collect();
        if rooms.len() == 1 {
            return Ok(rooms[0].clone());
        }
    }

    anyhow::bail!("Could not find #admins room. Is this the first registered (admin) user?")
}

fn urlencoded(s: &str) -> String {
    s.replace('!', "%21")
        .replace(':', "%3A")
        .replace('#', "%23")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PolicyConfig;

    #[test]
    fn registration_yaml_has_correct_format() {
        let config = PolicyConfig {
            homeserver_url: "http://localhost:8008".to_string(),
            as_token: "test_as_token".to_string(),
            hs_token: "test_hs_token".to_string(),
            server_name: "test.localhost".to_string(),
            sender_localpart: "mxdx-policy".to_string(),
            user_prefix: "agent-".to_string(),
            appservice_port: 9100,
        };

        let reg = AppserviceRegistration::from_config(&config);
        let yaml = reg.format_yaml();

        assert!(yaml.contains("id: mxdx-policy"));
        assert!(yaml.contains("as_token: \"test_as_token\""));
        assert!(yaml.contains("hs_token: \"test_hs_token\""));
        assert!(yaml.contains("sender_localpart: \"mxdx-policy\""));
        assert!(yaml.contains("exclusive: true"));
        assert!(yaml.contains("@agent-.*:test\\.localhost"));
    }

    #[test]
    fn registration_claims_exclusive_namespace() {
        let config = PolicyConfig {
            homeserver_url: "http://localhost:8008".to_string(),
            as_token: "as".to_string(),
            hs_token: "hs".to_string(),
            server_name: "example.com".to_string(),
            sender_localpart: "mxdx-policy".to_string(),
            user_prefix: "agent-".to_string(),
            appservice_port: 9100,
        };

        let reg = AppserviceRegistration::from_config(&config);
        assert_eq!(reg.namespaces.users.len(), 1);
        assert!(reg.namespaces.users[0].exclusive);
        assert_eq!(reg.namespaces.users[0].regex, "@agent-.*:example\\.com");
    }
}
