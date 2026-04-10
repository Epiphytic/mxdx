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

    fn state_url(&self, room: &RoomId, event_type: &str) -> String {
        let encoded = urlencoding::encode(room.as_str());
        format!("{}/_matrix/client/v3/rooms/{}/state/{}/", self.homeserver, encoded, event_type)
    }

    pub async fn get_room_topic(&self, room: &RoomId) -> Result<Option<String>> {
        let url = self.state_url(room, "m.room.topic");
        let resp = self.http.get(&url).bearer_auth(&self.access_token).send().await?;
        if resp.status().as_u16() == 404 { return Ok(None); }
        if !resp.status().is_success() {
            anyhow::bail!("get_room_topic: HTTP {}", resp.status());
        }
        let v: serde_json::Value = resp.json().await?;
        Ok(v.get("topic").and_then(|t| t.as_str()).map(String::from))
    }

    /// Fetch the content of the `m.room.create` state event.
    ///
    /// By Matrix spec `m.room.create` is the foundational event that
    /// establishes the room — it is NEVER encrypted, even with MSC4362
    /// encrypted state events enabled (there would be no way to decrypt it
    /// before joining). This makes it the one place where discovery
    /// metadata (`org.mxdx.launcher_id`, `org.mxdx.role`) can be published
    /// and reliably read via plain REST, regardless of crypto state.
    ///
    /// Returns the raw JSON body of the event (not just the content wrapper).
    /// Caller should look up custom fields directly off the returned value.
    pub async fn get_room_create(&self, room: &RoomId) -> Result<Option<serde_json::Value>> {
        let url = self.state_url(room, "m.room.create");
        let resp = self.http.get(&url).bearer_auth(&self.access_token).send().await?;
        if resp.status().as_u16() == 404 { return Ok(None); }
        if !resp.status().is_success() {
            anyhow::bail!("get_room_create: HTTP {}", resp.status());
        }
        let v: serde_json::Value = resp.json().await?;
        Ok(Some(v))
    }

    pub async fn get_room_name(&self, room: &RoomId) -> Result<Option<String>> {
        let url = self.state_url(room, "m.room.name");
        let resp = self.http.get(&url).bearer_auth(&self.access_token).send().await?;
        if resp.status().as_u16() == 404 { return Ok(None); }
        if !resp.status().is_success() {
            anyhow::bail!("get_room_name: HTTP {}", resp.status());
        }
        let v: serde_json::Value = resp.json().await?;
        Ok(v.get("name").and_then(|n| n.as_str()).map(String::from))
    }

    pub async fn get_room_encryption(&self, room: &RoomId) -> Result<Option<EncryptionState>> {
        let url = self.state_url(room, "m.room.encryption");
        let resp = self.http.get(&url).bearer_auth(&self.access_token).send().await?;
        if resp.status().as_u16() == 404 { return Ok(None); }
        if !resp.status().is_success() {
            anyhow::bail!("get_room_encryption: HTTP {}", resp.status());
        }
        let v: serde_json::Value = resp.json().await?;
        let algorithm = v.get("algorithm").and_then(|a| a.as_str()).unwrap_or("").to_string();
        let encrypt_state_events = v.get("encrypt_state_events").and_then(|b| b.as_bool())
            .or_else(|| v.get("io.element.msc4362.encrypt_state_events").and_then(|b| b.as_bool()))
            .unwrap_or(false);
        Ok(Some(EncryptionState { algorithm, encrypt_state_events }))
    }

    pub async fn get_room_tombstone(&self, room: &RoomId) -> Result<Option<OwnedRoomId>> {
        let url = self.state_url(room, "m.room.tombstone");
        let resp = self.http.get(&url).bearer_auth(&self.access_token).send().await?;
        if resp.status().as_u16() == 404 { return Ok(None); }
        if !resp.status().is_success() {
            anyhow::bail!("get_room_tombstone: HTTP {}", resp.status());
        }
        let v: serde_json::Value = resp.json().await?;
        let replacement = v.get("replacement_room").and_then(|r| r.as_str());
        match replacement {
            Some(s) => Ok(Some(RoomId::parse(s)?.to_owned())),
            None => Ok(None),
        }
    }

    /// Fetch the full list of state events for a room.
    ///
    /// Returns the raw JSON array from `GET /_matrix/client/v3/rooms/{roomId}/state`.
    /// Each event includes `type`, `state_key`, `content`, `origin_server_ts`, `sender`, etc.
    /// This is useful for reading metadata (like `origin_server_ts`) of MSC4362
    /// encrypted state events that can't be decrypted without room keys.
    pub async fn get_room_full_state(&self, room: &RoomId) -> Result<Vec<serde_json::Value>> {
        let encoded = urlencoding::encode(room.as_str());
        let url = format!(
            "{}/_matrix/client/v3/rooms/{}/state",
            self.homeserver, encoded
        );
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&self.access_token)
            .send()
            .await?;
        if !resp.status().is_success() {
            anyhow::bail!("get_room_full_state: HTTP {}", resp.status());
        }
        let events: Vec<serde_json::Value> = resp.json().await?;
        Ok(events)
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
