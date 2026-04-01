use std::path::{Path, PathBuf};
use std::time::Duration;

use matrix_sdk::{
    authentication::{matrix::MatrixSession, AuthSession, SessionTokens},
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

/// Compute a short 16-hex-char hash of the input string using FNV-1a.
/// Used to derive deterministic, filesystem-safe directory names from server
/// URLs or user IDs without leaking the full identifier.
///
/// Uses FNV-1a (not `DefaultHasher`) because `DefaultHasher` output is not
/// stable across Rust versions or platforms, which would silently orphan
/// persistent crypto store directories after a toolchain upgrade.
pub fn short_hash(input: &str) -> String {
    let mut hash: u64 = 14695981039346656037; // FNV offset basis
    for byte in input.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(1099511628211); // FNV prime
    }
    format!("{:016x}", hash)
}

/// Compute the default persistent crypto store base path for a given role.
///
/// Returns `~/.mxdx/crypto/{role}/` (e.g. `~/.mxdx/crypto/worker/`).
/// Returns `None` if the home directory cannot be determined.
///
/// If `MXDX_STORE_DIR` is set, uses that directory instead (for test isolation).
pub fn default_store_base_path(role: &str) -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("MXDX_STORE_DIR") {
        return Some(PathBuf::from(dir).join(role));
    }
    dirs::home_dir().map(|home| home.join(".mxdx").join("crypto").join(role))
}

/// Manages the lifecycle of the sqlite crypto store directory.
///
/// - `Temp`: backed by `tempfile::TempDir`, deleted on drop (for tests).
/// - `Persistent`: backed by a fixed path, survives process exit (for production).
pub(crate) enum StoreDir {
    Temp(tempfile::TempDir),
    Persistent(PathBuf),
}

impl StoreDir {
    pub(crate) fn path(&self) -> &Path {
        match self {
            StoreDir::Temp(t) => t.path(),
            StoreDir::Persistent(p) => p.as_path(),
        }
    }
}

pub struct MatrixClient {
    client: Client,
    _store_dir: StoreDir,
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
        Self::login_and_connect_opts(server_name_or_url, username, password, false).await
    }

    /// Login with option to accept invalid TLS certificates (for self-signed certs
    /// in federated testing). NEVER use `danger_accept_invalid_certs: true` in production.
    pub async fn login_and_connect_opts(
        server_name_or_url: &str,
        username: &str,
        password: &str,
        danger_accept_invalid_certs: bool,
    ) -> Result<Self> {
        let tmp = tempfile::TempDir::new().map_err(|e| MatrixClientError::Other(e.into()))?;
        let store_dir = StoreDir::Temp(tmp);

        let mut builder = Client::builder().sqlite_store(store_dir.path(), None);

        if danger_accept_invalid_certs {
            builder = builder.disable_ssl_verification();
        }

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

    /// Login with a persistent crypto store that survives process restarts.
    /// The `store_path` directory is created (with 0o700 permissions on Unix) if it
    /// does not exist. E2EE keys are preserved across restarts, avoiding new-device
    /// creation on every login.
    pub async fn login_and_connect_persistent(
        server_name_or_url: &str,
        username: &str,
        password: &str,
        store_path: PathBuf,
        danger_accept_invalid_certs: bool,
    ) -> Result<Self> {
        Self::login_and_connect_persistent_with_passphrase(
            server_name_or_url, username, password, store_path,
            danger_accept_invalid_certs, None,
        ).await
    }

    /// Login with a persistent crypto store and an optional passphrase for at-rest encryption.
    ///
    /// When `store_passphrase` is `Some`, the SQLite crypto store is encrypted with the
    /// given passphrase, protecting E2EE private keys on disk. When `None`, the store
    /// is unencrypted (suitable for tests only).
    pub async fn login_and_connect_persistent_with_passphrase(
        server_name_or_url: &str,
        username: &str,
        password: &str,
        store_path: PathBuf,
        danger_accept_invalid_certs: bool,
        store_passphrase: Option<&str>,
    ) -> Result<Self> {
        Self::ensure_store_dir(&store_path)?;
        let store_dir = StoreDir::Persistent(store_path);

        let mut builder = Client::builder().sqlite_store(store_dir.path(), store_passphrase);

        if danger_accept_invalid_certs {
            builder = builder.disable_ssl_verification();
        }

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

    /// Restore a session from an access token using a persistent crypto store.
    /// The `store_path` directory is created (with 0o700 permissions on Unix) if it
    /// does not exist.
    pub async fn connect_with_token_persistent(
        homeserver_url: &str,
        access_token: &str,
        user_id: &str,
        device_id: &str,
        store_path: PathBuf,
    ) -> Result<Self> {
        Self::connect_with_token_persistent_with_passphrase(
            homeserver_url, access_token, user_id, device_id, store_path, None,
        ).await
    }

    /// Restore a session with a persistent crypto store and optional passphrase.
    pub async fn connect_with_token_persistent_with_passphrase(
        homeserver_url: &str,
        access_token: &str,
        user_id: &str,
        device_id: &str,
        store_path: PathBuf,
        store_passphrase: Option<&str>,
    ) -> Result<Self> {
        Self::ensure_store_dir(&store_path)?;
        let store_dir = StoreDir::Persistent(store_path);

        let client = Client::builder()
            .homeserver_url(homeserver_url)
            .sqlite_store(store_dir.path(), store_passphrase)
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

    /// Create the store directory with secure permissions if it doesn't exist.
    fn ensure_store_dir(path: &Path) -> Result<()> {
        std::fs::create_dir_all(path)
            .map_err(|e| MatrixClientError::Other(anyhow::anyhow!(
                "Failed to create crypto store directory {}: {e}", path.display()
            )))?;

        // Set restrictive permissions on Unix (owner-only read/write/execute).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| MatrixClientError::Other(anyhow::anyhow!(
                    "Failed to set permissions on {}: {e}", path.display()
                )))?;
        }

        Ok(())
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
        let tmp = tempfile::TempDir::new().map_err(|e| MatrixClientError::Other(e.into()))?;
        let store_dir = StoreDir::Temp(tmp);

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
        let tmp = tempfile::TempDir::new().map_err(|e| MatrixClientError::Other(e.into()))?;
        let store_dir = StoreDir::Temp(tmp);

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

    // ── Session export ─────────────────────────────────────────────────

    /// Export session data for keychain storage.
    /// Returns the current session's user_id, device_id, access_token, and homeserver_url.
    ///
    /// The `homeserver_url` parameter is the original server URL used to connect
    /// (before any .well-known redirection), ensuring keychain keys are consistent.
    ///
    /// **Security**: The returned `SessionData` contains the access token.
    /// Callers must store it encrypted (e.g., via `KeychainBackend`).
    pub fn export_session(&self, homeserver_url: &str) -> Result<crate::session::SessionData> {
        let user_id = self
            .client
            .user_id()
            .ok_or_else(|| MatrixClientError::Other(anyhow::anyhow!("not logged in")))?
            .to_string();
        let device_id = self
            .client
            .device_id()
            .ok_or_else(|| MatrixClientError::Other(anyhow::anyhow!("no device id")))?
            .to_string();
        let session = self
            .client
            .session()
            .ok_or_else(|| MatrixClientError::Other(anyhow::anyhow!("no active session")))?;
        let access_token = match session {
            AuthSession::Matrix(ms) => ms.tokens.access_token.clone(),
            _ => {
                return Err(MatrixClientError::Other(anyhow::anyhow!(
                    "unsupported auth type (expected Matrix auth)"
                )))
            }
        };
        Ok(crate::session::SessionData {
            user_id,
            device_id,
            access_token,
            homeserver_url: homeserver_url.to_string(),
        })
    }

    // ── Cross-signing ──────────────────────────────────────────────────

    /// Bootstrap cross-signing keys (master, user-signing, self-signing) and
    /// upload them. Tries without auth first (UIA grace period right after
    /// login), falls back to password auth if the server requires UIA.
    pub async fn bootstrap_cross_signing(&self, password: Option<&str>) -> Result<()> {
        use matrix_sdk::ruma::api::client::uiaa;

        let encryption = self.client.encryption();

        match encryption.bootstrap_cross_signing(None).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                // If no password provided, can't handle UIA
                let password = password.ok_or_else(|| {
                    MatrixClientError::Other(anyhow::anyhow!(
                        "Cross-signing bootstrap requires UIA but no password provided: {e}"
                    ))
                })?;

                let uiaa_info = e.as_uiaa_response().ok_or_else(|| {
                    MatrixClientError::Other(anyhow::anyhow!(
                        "Cross-signing bootstrap failed (not UIA): {e}"
                    ))
                })?;

                let session = uiaa_info.session.clone();
                let user_id = self.user_id();

                let mut password_auth = uiaa::Password::new(
                    uiaa::UserIdentifier::UserIdOrLocalpart(user_id.localpart().to_owned()),
                    password.to_owned(),
                );
                password_auth.session = session;

                encryption
                    .bootstrap_cross_signing(Some(uiaa::AuthData::Password(password_auth)))
                    .await?;
            }
        }
        Ok(())
    }

    /// Bootstrap cross-signing only if not already set up.
    /// No-op if keys exist and private parts are in the local crypto store.
    /// Falls back to full bootstrap if private keys are missing.
    pub async fn bootstrap_cross_signing_if_needed(&self, password: Option<&str>) -> Result<()> {
        let encryption = self.client.encryption();
        match encryption.bootstrap_cross_signing_if_needed(None).await {
            Ok(()) => return Ok(()),
            Err(_) => {}
        }
        // Fall back to full bootstrap
        self.bootstrap_cross_signing(password).await
    }

    /// Verify our own user identity (marks as locally verified).
    /// Must be done before verifying other users.
    pub async fn verify_own_identity(&self) -> Result<()> {
        let user_id = self.user_id().to_owned();
        let identity = self
            .client
            .encryption()
            .get_user_identity(&user_id)
            .await
            .map_err(|e| MatrixClientError::Other(e.into()))?
            .ok_or_else(|| {
                MatrixClientError::Other(anyhow::anyhow!(
                    "No identity found — bootstrap cross-signing first"
                ))
            })?;
        identity
            .verify()
            .await
            .map_err(|e| MatrixClientError::Other(e.into()))?;
        Ok(())
    }

    /// Verify another user's identity by signing their master key.
    /// Both users must have bootstrapped cross-signing first.
    pub async fn verify_user(&self, user_id: &UserId) -> Result<()> {
        let identity = self
            .client
            .encryption()
            .get_user_identity(user_id)
            .await
            .map_err(|e| MatrixClientError::Other(e.into()))?
            .ok_or_else(|| {
                MatrixClientError::Other(anyhow::anyhow!(
                    "No identity found for {} — they may not have bootstrapped cross-signing",
                    user_id
                ))
            })?;
        identity
            .verify()
            .await
            .map_err(|e| MatrixClientError::Other(e.into()))?;
        Ok(())
    }

    /// Check if a user's identity is verified from our perspective.
    pub async fn is_user_verified(&self, user_id: &UserId) -> Result<bool> {
        let identity = self
            .client
            .encryption()
            .get_user_identity(user_id)
            .await
            .map_err(|e| MatrixClientError::Other(e.into()))?;
        Ok(identity.map(|i| i.is_verified()).unwrap_or(false))
    }

    /// Get all verified user IDs in a room by scanning active members.
    pub async fn get_verified_user_ids_in_room(
        &self,
        room_id: &RoomId,
    ) -> Result<Vec<OwnedUserId>> {
        let room = self
            .client
            .get_room(room_id)
            .ok_or_else(|| MatrixClientError::RoomNotFound(room_id.to_string()))?;

        let members = room
            .members(matrix_sdk::RoomMemberships::ACTIVE)
            .await
            .map_err(|e| MatrixClientError::Other(e.into()))?;

        let mut verified = Vec::new();
        for member in &members {
            let uid = member.user_id();
            if self.is_user_verified(uid).await? {
                verified.push(uid.to_owned());
            }
        }
        Ok(verified)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_persistent_store_dir_survives_drop() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir_path = tmp.path().join("persistent_test");
        std::fs::create_dir_all(&dir_path).unwrap();

        let store = StoreDir::Persistent(dir_path.clone());
        assert!(dir_path.exists());

        // Drop the StoreDir — persistent variant must NOT delete the directory
        drop(store);
        assert!(
            dir_path.exists(),
            "Persistent store directory should survive drop"
        );
    }

    #[test]
    fn test_temp_store_dir_cleaned_on_drop() {
        let store = StoreDir::Temp(tempfile::TempDir::new().unwrap());
        let path = store.path().to_owned();
        assert!(path.exists());

        drop(store);
        assert!(
            !path.exists(),
            "Temp store directory should be deleted on drop"
        );
    }

    #[test]
    fn test_store_dir_path_returns_correct_path() {
        // Temp variant
        let tmp = tempfile::TempDir::new().unwrap();
        let expected = tmp.path().to_owned();
        let store = StoreDir::Temp(tmp);
        assert_eq!(store.path(), expected);

        // Persistent variant
        let p = PathBuf::from("/tmp/mxdx-test-persistent");
        let store = StoreDir::Persistent(p.clone());
        assert_eq!(store.path(), p);
    }

    #[test]
    fn test_ensure_store_dir_creates_directory() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("nested").join("crypto");
        assert!(!dir.exists());

        MatrixClient::ensure_store_dir(&dir).unwrap();
        assert!(dir.exists());

        // On Unix, verify permissions are 0o700
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700, "Directory should have 0o700 permissions");
        }
    }

    #[test]
    fn test_short_hash_deterministic() {
        let h1 = short_hash("https://matrix.example.com");
        let h2 = short_hash("https://matrix.example.com");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 16, "Short hash should be 16 hex chars");
    }

    #[test]
    fn test_short_hash_stable_across_versions() {
        // FNV-1a must produce the same output forever (persistent directory names).
        // If this test fails after a code change, existing crypto stores will be orphaned.
        assert_eq!(short_hash("https://matrix.example.com"), "c43cb7cfa4a1fda8");
    }

    #[test]
    fn test_short_hash_differs_for_different_inputs() {
        let h1 = short_hash("https://server-a.example.com");
        let h2 = short_hash("https://server-b.example.com");
        assert_ne!(h1, h2);
    }

    #[test]
    fn short_hash_different_users_same_server_differ() {
        let hash_alice = short_hash("alice@https://matrix.org");
        let hash_bob = short_hash("bob@https://matrix.org");
        assert_ne!(hash_alice, hash_bob, "different users on same server must get different hashes");
    }

    #[test]
    fn test_default_store_base_path_has_correct_structure() {
        // This test may fail in environments without a home directory,
        // which is acceptable (CI containers, etc.)
        if let Some(path) = default_store_base_path("worker") {
            assert!(path.ends_with("worker"));
            let parent = path.parent().unwrap();
            assert!(parent.ends_with("crypto"));
            let grandparent = parent.parent().unwrap();
            assert!(grandparent.ends_with(".mxdx"));
        }
    }
}
