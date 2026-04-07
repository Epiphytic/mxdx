//! Direct Matrix client-server REST API helpers.
//!
//! This module deliberately does NOT use matrix-sdk. It exists for two reasons:
//!   1. Workers/clients need to discover rooms even when the SDK's local cache
//!      is stale (after session restore, before initial sync).
//!   2. Diagnostic and cleanup tools must work without conflicting with a
//!      running daemon's crypto store.
//!
//! All requests have a 10s timeout. All errors are returned as `Result`.

use anyhow::{Context, Result};
use matrix_sdk::ruma::{OwnedRoomId, RoomId};
use serde::Deserialize;
use std::time::Duration;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

pub struct RestClient {
    homeserver: String,
    access_token: String,
    http: reqwest::Client,
}

#[derive(Debug, Clone)]
pub struct EncryptionState {
    pub algorithm: String,
    pub encrypt_state_events: bool,
}

impl RestClient {
    pub fn new(homeserver: &str, access_token: &str) -> Self {
        Self {
            homeserver: homeserver.trim_end_matches('/').to_string(),
            access_token: access_token.to_string(),
            http: reqwest::Client::builder()
                .timeout(DEFAULT_TIMEOUT)
                .build()
                .expect("reqwest client build"),
        }
    }

    pub async fn list_joined_rooms(&self) -> Result<Vec<OwnedRoomId>> {
        #[derive(Deserialize)]
        struct R {
            joined_rooms: Vec<String>,
        }
        let url = format!("{}/_matrix/client/v3/joined_rooms", self.homeserver);
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&self.access_token)
            .send()
            .await?;
        if !resp.status().is_success() {
            anyhow::bail!("list_joined_rooms: HTTP {}", resp.status());
        }
        let r: R = resp.json().await?;
        r.joined_rooms
            .into_iter()
            .map(|s| {
                RoomId::parse(&s)
                    .map(|id| id.to_owned())
                    .context("invalid room id")
            })
            .collect()
    }

    pub async fn list_invited_rooms(&self) -> Result<Vec<OwnedRoomId>> {
        let url = format!(
            "{}/_matrix/client/v3/sync?timeout=0",
            self.homeserver
        );
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&self.access_token)
            .send()
            .await?;
        if !resp.status().is_success() {
            anyhow::bail!("list_invited_rooms (sync): HTTP {}", resp.status());
        }
        let v: serde_json::Value = resp.json().await?;
        let invite = v
            .get("rooms")
            .and_then(|r| r.get("invite"))
            .and_then(|i| i.as_object());
        let mut out = Vec::new();
        if let Some(map) = invite {
            for (rid, _) in map {
                if let Ok(id) = RoomId::parse(rid) {
                    out.push(id.to_owned());
                }
            }
        }
        Ok(out)
    }
}
