use std::time::Duration;

use matrix_sdk::{
    config::SyncSettings,
    room::MessagesOptions,
    ruma::{
        api::client::room::create_room::v3::Request as CreateRoomRequest,
        events::{
            room::encryption::RoomEncryptionEventContent, EmptyStateKey, InitialStateEvent,
        },
        OwnedRoomId, OwnedUserId, RoomId, UserId,
    },
    Client,
};
use serde_json::Value;

use crate::error::{MatrixClientError, Result};

pub struct MatrixClient {
    client: Client,
    _store_dir: tempfile::TempDir,
    room_creation_delay: Option<Duration>,
}

impl MatrixClient {
    /// Login to an existing account on the homeserver with E2EE enabled.
    /// Use this for public Matrix servers where accounts are pre-registered.
    pub async fn login_and_connect(
        homeserver_url: &str,
        username: &str,
        password: &str,
    ) -> Result<Self> {
        let store_dir =
            tempfile::TempDir::new().map_err(|e| MatrixClientError::Other(e.into()))?;

        let client = Client::builder()
            .homeserver_url(homeserver_url)
            .sqlite_store(store_dir.path(), None)
            .build()
            .await?;

        client
            .matrix_auth()
            .login_username(username, password)
            .initial_device_display_name("mxdx")
            .await?;

        Ok(MatrixClient {
            client,
            _store_dir: store_dir,
            room_creation_delay: None,
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
        let store_dir =
            tempfile::TempDir::new().map_err(|e| MatrixClientError::Other(e.into()))?;

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

        Ok(MatrixClient {
            client,
            _store_dir: store_dir,
            room_creation_delay: None,
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
        let encryption_event =
            InitialStateEvent::new(EmptyStateKey, RoomEncryptionEventContent::with_recommended_defaults());

        let mut request = CreateRoomRequest::new();
        request.invite = invite.to_vec();
        request.initial_state = vec![encryption_event.to_raw_any()];

        let response = self.client.create_room(request).await?;
        Ok(response.room_id().to_owned())
    }

    /// Create a DM room with a single user (encrypted, direct).
    pub async fn create_dm(&self, user_id: &UserId) -> Result<OwnedRoomId> {
        let encryption_event =
            InitialStateEvent::new(EmptyStateKey, RoomEncryptionEventContent::with_recommended_defaults());

        let mut request = CreateRoomRequest::new();
        request.invite = vec![user_id.to_owned()];
        request.is_direct = true;
        request.initial_state = vec![encryption_event.to_raw_any()];

        let response = self.client.create_room(request).await?;
        Ok(response.room_id().to_owned())
    }

    /// Join a room by ID.
    pub async fn join_room(&self, room_id: &RoomId) -> Result<()> {
        self.client.join_room_by_id(room_id).await?;
        Ok(())
    }

    /// Send a custom event to a room. The payload should have "type" and "content" fields.
    pub async fn send_event(&self, room_id: &RoomId, payload: Value) -> Result<()> {
        let room = self
            .client
            .get_room(room_id)
            .ok_or_else(|| MatrixClientError::RoomNotFound(room_id.to_string()))?;

        let event_type = payload["type"]
            .as_str()
            .unwrap_or("org.mxdx.unknown")
            .to_string();
        let content = payload["content"].clone();

        room.send_raw(&event_type, content).await?;
        Ok(())
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
    /// Uses Room::messages() which automatically decrypts E2EE events.
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

            // After syncing, use Room::messages() to get decrypted events
            if let Some(room) = self.client.get_room(room_id) {
                let messages = room.messages(MessagesOptions::backward()).await?;
                let mut collected: Vec<Value> = Vec::new();
                for event in &messages.chunk {
                    if let Ok(json) = serde_json::to_value(event.raw().json()) {
                        let event_type = json.get("type").and_then(|t| t.as_str());
                        // Skip state events and encrypted events that weren't decrypted
                        if event_type != Some("m.room.encrypted")
                            && event_type != Some("m.room.encryption")
                            && event_type != Some("m.room.member")
                        {
                            collected.push(json);
                        }
                    }
                }
                if !collected.is_empty() {
                    return Ok(collected);
                }
            }
        }

        Ok(Vec::new())
    }

    /// Wait until E2EE key exchange completes for a room, with timeout.
    /// Syncs in a loop until the room has encryption keys for all members.
    pub async fn wait_for_key_exchange(
        &self,
        room_id: &RoomId,
        timeout: Duration,
    ) -> Result<()> {
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

            let members = room.members(matrix_sdk::RoomMemberships::ACTIVE).await
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

    /// Get access to the inner matrix-sdk Client (escape hatch for advanced use).
    pub fn inner(&self) -> &Client {
        &self.client
    }
}
