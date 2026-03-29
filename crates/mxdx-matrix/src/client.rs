use std::time::Duration;

use matrix_sdk::{
    authentication::{matrix::MatrixSession, SessionTokens},
    config::SyncSettings,
    room::MessagesOptions,
    ruma::{
        api::client::room::create_room::v3::Request as CreateRoomRequest,
        events::{room::encryption::RoomEncryptionEventContent, EmptyStateKey, InitialStateEvent},
        OwnedRoomId, OwnedUserId, RoomId, UserId,
    },
    Client, SessionMeta,
};
use serde_json::Value;

use crate::error::{MatrixClientError, Result};

pub struct MatrixClient {
    client: Client,
    _store_dir: tempfile::TempDir,
    room_creation_delay: Option<Duration>,
    room_creation_timeout: Duration,
}

impl MatrixClient {
    /// Login to an existing account on the homeserver with E2EE enabled.
    /// Use this for public Matrix servers where accounts are pre-registered.
    ///
    /// The `server_name_or_url` can be either:
    /// - A server name like `matrix.org` (triggers .well-known discovery)
    /// - A full URL like `https://matrix-client.matrix.org` (used directly)
    pub async fn login_and_connect(
        server_name_or_url: &str,
        username: &str,
        password: &str,
    ) -> Result<Self> {
        let store_dir = tempfile::TempDir::new().map_err(|e| MatrixClientError::Other(e.into()))?;

        let builder = Client::builder().sqlite_store(store_dir.path(), None);

        // If it looks like a URL (has ://), use it directly.
        // Otherwise treat it as a server name and let the SDK do .well-known discovery.
        let client = if server_name_or_url.contains("://") {
            builder.homeserver_url(server_name_or_url)
        } else {
            builder.server_name_or_homeserver_url(server_name_or_url)
        }
        .build()
        .await?;

        client
            .matrix_auth()
            .login_username(username, password)
            .initial_device_display_name("mxdx")
            .await?;

        // Initial sync to upload device keys — required before creating encrypted rooms.
        // Without this, room creation hangs on rate-limited servers (e.g., matrix.org).
        client
            .sync_once(SyncSettings::default().timeout(Duration::from_secs(5)))
            .await?;

        Ok(MatrixClient {
            client,
            _store_dir: store_dir,
            room_creation_delay: None,
            room_creation_timeout: Duration::from_secs(30),
        })
    }

    /// Connect to a homeserver by restoring a session from an existing access token.
    /// Requires `user_id` (e.g. `@worker:example.com`) and `device_id` (e.g. `FABRICBOT`).
    /// The device_id must match the one that generated the access_token on the server.
    pub async fn connect_with_token(
        homeserver_url: &str,
        access_token: &str,
        user_id: &str,
        device_id: &str,
    ) -> Result<Self> {
        let store_dir = tempfile::TempDir::new().map_err(|e| MatrixClientError::Other(e.into()))?;

        let client = Client::builder()
            .homeserver_url(homeserver_url)
            .sqlite_store(store_dir.path(), None)
            .build()
            .await?;

        let session = MatrixSession {
            meta: SessionMeta {
                user_id: user_id
                    .try_into()
                    .map_err(|e: matrix_sdk::IdParseError| MatrixClientError::Other(e.into()))?,
                device_id: device_id.into(),
            },
            tokens: SessionTokens {
                access_token: access_token.to_string(),
                refresh_token: None,
            },
        };

        client
            .restore_session(session)
            .await
            .map_err(|e| MatrixClientError::Other(e.into()))?;

        client
            .sync_once(SyncSettings::default().timeout(Duration::from_secs(5)))
            .await?;

        Ok(MatrixClient {
            client,
            _store_dir: store_dir,
            room_creation_delay: None,
            room_creation_timeout: Duration::from_secs(30),
        })
    }

    /// Register a new user on the homeserver and connect with E2EE enabled.
    /// Uses the Matrix register API with registration token auth, then
    /// builds a matrix-sdk Client with sqlite store for crypto state.
    /// For self-hosted servers with token registration enabled.
    pub async fn register_and_connect(
        homeserver_url: &str,
        username: &str,
        password: &str,
        registration_token: &str,
    ) -> Result<Self> {
        // Register user via REST API (same approach as TuwunelInstance::register_user)
        let http_client = reqwest::Client::new();
        let reg_url = format!("{homeserver_url}/_matrix/client/v3/register");
        let body = serde_json::json!({
            "username": username,
            "password": password,
            "auth": {
                "type": "m.login.registration_token",
                "token": registration_token
            }
        });

        let resp = http_client
            .post(&reg_url)
            .json(&body)
            .send()
            .await
            .map_err(|e| MatrixClientError::Registration(e.to_string()))?;

        if !resp.status().is_success() {
            let err_body = resp
                .text()
                .await
                .unwrap_or_else(|_| "unknown error".to_string());
            return Err(MatrixClientError::Registration(format!(
                "Registration failed: {err_body}"
            )));
        }

        // Build the matrix-sdk client with sqlite store for E2EE
        let store_dir = tempfile::TempDir::new().map_err(|e| MatrixClientError::Other(e.into()))?;

        let client = Client::builder()
            .homeserver_url(homeserver_url)
            .sqlite_store(store_dir.path(), None)
            .build()
            .await?;

        // Login with the registered credentials
        client
            .matrix_auth()
            .login_username(username, password)
            .initial_device_display_name("mxdx-test")
            .await?;

        // Initial sync to upload device keys.
        client
            .sync_once(SyncSettings::default().timeout(Duration::from_secs(5)))
            .await?;

        Ok(MatrixClient {
            client,
            _store_dir: store_dir,
            room_creation_delay: None,
            room_creation_timeout: Duration::from_secs(30),
        })
    }

    /// Check if the client is logged in.
    pub fn is_logged_in(&self) -> bool {
        self.client.user_id().is_some()
    }

    /// Check if E2EE crypto is enabled.
    pub async fn crypto_enabled(&self) -> bool {
        self.client.encryption().ed25519_key().await.is_some()
    }

    /// Get the user ID of the logged-in user.
    pub fn user_id(&self) -> &UserId {
        self.client
            .user_id()
            .expect("Client is not logged in — no user_id")
    }

    /// Create an encrypted room and invite the given users.
    pub async fn create_encrypted_room(&self, invite: &[OwnedUserId]) -> Result<OwnedRoomId> {
        let encryption_event = InitialStateEvent::new(
            EmptyStateKey,
            RoomEncryptionEventContent::with_recommended_defaults(),
        );

        let mut request = CreateRoomRequest::new();
        request.invite = invite.to_vec();
        request.initial_state = vec![encryption_event.to_raw_any()];

        let response = self.create_room_with_timeout(request).await?;
        Ok(response.room_id().to_owned())
    }

    /// Create a DM room with a single user (encrypted, direct).
    pub async fn create_dm(&self, user_id: &UserId) -> Result<OwnedRoomId> {
        let encryption_event = InitialStateEvent::new(
            EmptyStateKey,
            RoomEncryptionEventContent::with_recommended_defaults(),
        );

        let mut request = CreateRoomRequest::new();
        request.invite = vec![user_id.to_owned()];
        request.is_direct = true;
        request.initial_state = vec![encryption_event.to_raw_any()];

        let response = self.create_room_with_timeout(request).await?;
        Ok(response.room_id().to_owned())
    }

    /// Join a room by ID.
    pub async fn join_room(&self, room_id: &RoomId) -> Result<()> {
        self.client.join_room_by_id(room_id).await?;
        Ok(())
    }

    pub async fn invite_user(&self, room_id: &RoomId, user_id: &UserId) -> Result<()> {
        let room = self
            .client
            .get_room(room_id)
            .ok_or_else(|| MatrixClientError::RoomNotFound(room_id.to_string()))?;

        room.invite_user_by_id(user_id).await?;
        Ok(())
    }

    /// Send a custom event to a room. The payload should have "type" and "content" fields.
    /// Returns the Matrix event ID of the sent event.
    pub async fn send_event(&self, room_id: &RoomId, payload: Value) -> Result<String> {
        let room = self
            .client
            .get_room(room_id)
            .ok_or_else(|| MatrixClientError::RoomNotFound(room_id.to_string()))?;

        let event_type = payload["type"]
            .as_str()
            .unwrap_or("org.mxdx.unknown")
            .to_string();
        let content = payload["content"].clone();

        let response = room.send_raw(&event_type, content).await?;
        Ok(response.event_id.to_string())
    }

    /// Send a custom event as a thread reply to an existing event.
    /// Adds `m.relates_to` with `rel_type: "m.thread"` to the content.
    /// Returns the Matrix event ID of the sent event.
    pub async fn send_threaded_event(
        &self,
        room_id: &RoomId,
        event_type: &str,
        in_reply_to_event_id: &str,
        content: Value,
    ) -> Result<String> {
        let room = self
            .client
            .get_room(room_id)
            .ok_or_else(|| MatrixClientError::RoomNotFound(room_id.to_string()))?;

        let mut body = match content {
            Value::Object(map) => map,
            _ => serde_json::Map::new(),
        };

        body.insert(
            "m.relates_to".to_string(),
            serde_json::json!({
                "rel_type": "m.thread",
                "event_id": in_reply_to_event_id
            }),
        );

        let response = room.send_raw(event_type, Value::Object(body)).await?;
        Ok(response.event_id.to_string())
    }

    /// Send a state event to a room.
    pub async fn send_state_event(
        &self,
        room_id: &RoomId,
        event_type: &str,
        state_key: &str,
        content: Value,
    ) -> Result<()> {
        let room = self
            .client
            .get_room(room_id)
            .ok_or_else(|| MatrixClientError::RoomNotFound(room_id.to_string()))?;

        room.send_state_event_raw(event_type, state_key, content)
            .await?;
        Ok(())
    }

    /// Perform a single sync cycle.
    pub async fn sync_once(&self) -> Result<()> {
        self.client
            .sync_once(SyncSettings::default().timeout(Duration::from_secs(1)))
            .await?;
        Ok(())
    }

    /// Sync and collect decrypted timeline events for a specific room within a timeout.
    /// Extracts new events from the sync response timeline, which are automatically
    /// decrypted by the SDK for E2EE rooms.
    pub async fn sync_and_collect_events(
        &self,
        room_id: &RoomId,
        timeout: Duration,
    ) -> Result<Vec<Value>> {
        let deadline = tokio::time::Instant::now() + timeout;
        let mut sync_token: Option<String> = None;

        while tokio::time::Instant::now() < deadline {
            let settings = match &sync_token {
                Some(token) => SyncSettings::default()
                    .timeout(Duration::from_secs(1))
                    .token(token.clone()),
                None => SyncSettings::default().timeout(Duration::from_secs(1)),
            };

            let response = self.client.sync_once(settings).await?;
            sync_token = Some(response.next_batch.clone());

            // Extract timeline events from the sync response for our room
            let mut collected: Vec<Value> = Vec::new();

            if let Some(joined) = response.rooms.joined.get(&room_id.to_owned()) {
                for timeline_event in &joined.timeline.events {
                    // TimelineEvent has .raw() which returns the decrypted event JSON
                    let json_str = timeline_event.raw().json().get();
                    if let Ok(json) = serde_json::from_str::<Value>(json_str) {
                        let event_type = json.get("type").and_then(|t| t.as_str());
                        // Skip infrastructure events
                        if event_type != Some("m.room.encrypted")
                            && event_type != Some("m.room.encryption")
                            && event_type != Some("m.room.member")
                            && event_type != Some("m.room.power_levels")
                        {
                            collected.push(json);
                        }
                    }
                }
            }

            if !collected.is_empty() {
                return Ok(collected);
            }
        }

        Ok(Vec::new())
    }

    /// Wait until E2EE key exchange completes for a room, with timeout.
    /// Syncs in a loop until the room has encryption keys for all members.
    pub async fn wait_for_key_exchange(&self, room_id: &RoomId, timeout: Duration) -> Result<()> {
        let deadline = tokio::time::Instant::now() + timeout;

        while tokio::time::Instant::now() < deadline {
            self.sync_once().await?;

            let room = match self.client.get_room(room_id) {
                Some(r) => r,
                None => continue,
            };

            if !room.encryption_state().is_encrypted() {
                continue;
            }

            let members = room
                .members(matrix_sdk::RoomMemberships::ACTIVE)
                .await
                .map_err(|e| MatrixClientError::Other(e.into()))?;

            let mut all_keys_available = true;
            for member in &members {
                let user_id = member.user_id();
                let devices = self
                    .client
                    .encryption()
                    .get_user_devices(user_id)
                    .await
                    .map_err(|e| MatrixClientError::Other(e.into()))?;

                if devices.devices().count() == 0 {
                    all_keys_available = false;
                    break;
                }
            }

            if all_keys_available && !members.is_empty() {
                return Ok(());
            }
        }

        Err(MatrixClientError::KeyExchangeTimeout(format!(
            "Timed out waiting for key exchange in room {room_id}"
        )))
    }

    /// Set an optional delay between room creation calls (for rate-limited servers).
    pub fn set_room_creation_delay(&mut self, delay: Option<Duration>) {
        self.room_creation_delay = delay;
    }

    /// Get the configured room creation delay.
    pub fn room_creation_delay(&self) -> Option<Duration> {
        self.room_creation_delay
    }

    /// Set the timeout for room creation (default: 30s).
    /// Public servers with aggressive rate limiting may need 120s+.
    pub fn set_room_creation_timeout(&mut self, timeout: Duration) {
        self.room_creation_timeout = timeout;
    }

    /// Get access to the inner matrix-sdk Client (escape hatch for advanced use).
    pub fn inner(&self) -> &Client {
        &self.client
    }

    /// Create a room with a 30-second timeout.
    /// matrix-sdk silently retries on 429 rate-limit responses, which can hang
    /// indefinitely. This wrapper fails fast with a clear error.
    pub(crate) async fn create_room_with_timeout(
        &self,
        request: CreateRoomRequest,
    ) -> Result<matrix_sdk::room::Room> {
        match tokio::time::timeout(self.room_creation_timeout, self.client.create_room(request))
            .await
        {
            Ok(result) => Ok(result?),
            Err(_) => Err(MatrixClientError::RoomCreationTimeout(format!(
                "Room creation timed out after {}s — server may be rate-limiting",
                self.room_creation_timeout.as_secs()
            ))),
        }
    }
}
