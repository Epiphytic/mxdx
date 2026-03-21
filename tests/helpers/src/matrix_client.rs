use anyhow::{bail, Context, Result};
use std::time::Duration;

/// Lightweight test Matrix client for registration and basic API calls.
#[derive(Debug, Clone)]
pub struct TestMatrixClient {
    pub user_id: String,
    pub access_token: String,
    pub device_id: String,
    pub homeserver_url: String,
}

impl TestMatrixClient {
    pub fn mxid(&self) -> &str {
        &self.user_id
    }

    fn client(&self) -> reqwest::Client {
        reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .build()
            .expect("Failed to build HTTP client")
    }

    /// Create a new room and return its room_id.
    pub async fn create_room(&self) -> Result<String> {
        let url = format!("{}/_matrix/client/v3/createRoom", self.homeserver_url);
        let resp = self
            .client()
            .post(&url)
            .bearer_auth(&self.access_token)
            .json(&serde_json::json!({
                "visibility": "private",
                "preset": "private_chat"
            }))
            .send()
            .await
            .context("Failed to create room")?;

        let status = resp.status();
        let body: serde_json::Value = resp.json().await?;
        if !status.is_success() {
            bail!("Create room failed ({}): {}", status, body);
        }
        Ok(body["room_id"]
            .as_str()
            .context("Missing room_id")?
            .to_string())
    }

    /// Invite a user to a room.
    pub async fn invite(&self, room_id: &str, user_id: &str) -> Result<()> {
        let url = format!(
            "{}/_matrix/client/v3/rooms/{}/invite",
            self.homeserver_url,
            urlencoded(room_id)
        );
        let resp = self
            .client()
            .post(&url)
            .bearer_auth(&self.access_token)
            .json(&serde_json::json!({ "user_id": user_id }))
            .send()
            .await
            .context("Failed to send invite")?;

        let status = resp.status();
        if !status.is_success() {
            let body: serde_json::Value = resp.json().await?;
            bail!("Invite failed ({}): {}", status, body);
        }
        Ok(())
    }

    /// Wait for an invite to appear in sync.
    pub async fn wait_for_invite(&self, room_id: &str, timeout: Duration) -> Result<()> {
        let deadline = tokio::time::Instant::now() + timeout;
        let mut since: Option<String> = None;

        loop {
            if tokio::time::Instant::now() > deadline {
                bail!("Timed out waiting for invite to room {}", room_id);
            }

            let url = format!("{}/_matrix/client/v3/sync", self.homeserver_url);
            let mut params = vec![("timeout".to_string(), "1000".to_string())];
            if let Some(ref token) = since {
                params.push(("since".to_string(), token.clone()));
            }

            let resp = self
                .client()
                .get(&url)
                .bearer_auth(&self.access_token)
                .query(&params)
                .send()
                .await
                .context("Sync request failed")?;

            let body: serde_json::Value = resp.json().await?;

            // Update since token
            if let Some(next) = body["next_batch"].as_str() {
                since = Some(next.to_string());
            }

            // Check for invite
            if let Some(invite) = body["rooms"]["invite"].as_object() {
                if invite.contains_key(room_id) {
                    return Ok(());
                }
            }

            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
}

fn urlencoded(s: &str) -> String {
    s.replace('!', "%21")
        .replace(':', "%3A")
        .replace('#', "%23")
}
