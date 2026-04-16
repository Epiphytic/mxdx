use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use matrix_sdk::{
    authentication::{matrix::MatrixSession, AuthSession, SessionTokens},
    config::SyncSettings,
    room::MessagesOptions,
    ruma::{
        api::client::{
            message::send_message_event,
            room::create_room::v3::Request as CreateRoomRequest,
        },
        events::{room::encryption::RoomEncryptionEventContent, EmptyStateKey, InitialStateEvent},
        serde::Raw,
        OwnedRoomId, OwnedUserId, RoomId, TransactionId, UserId,
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
            .sync_once(SyncSettings::default().timeout(Duration::from_secs(1)))
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
            .sync_once(SyncSettings::default().timeout(Duration::from_secs(1)))
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
            .sync_once(SyncSettings::default().timeout(Duration::from_secs(1)))
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
            .sync_once(SyncSettings::default().timeout(Duration::from_secs(1)))
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
            .sync_once(SyncSettings::default().timeout(Duration::from_secs(1)))
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
            RoomEncryptionEventContent::with_recommended_defaults().with_encrypted_state(),
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
            RoomEncryptionEventContent::with_recommended_defaults().with_encrypted_state(),
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

    /// Send a Matrix VoIP `m.call.*` signaling event via the existing
    /// encrypted room send path. Thin wrapper over `room.send_raw`
    /// (matrix-sdk 0.16 encrypts in-flight using the existing outbound
    /// Megolm session) — deliberately introduces no new encryption code.
    ///
    /// # Cardinal rule
    ///
    /// This function refuses to send into an unencrypted room. Every Matrix
    /// event mxdx emits must be Megolm-encrypted on the wire (see
    /// `CLAUDE.md`): signaling events that carry the `mxdx_session_key`
    /// extension field (see `mxdx-p2p::signaling::events`) are protected
    /// by room E2EE because session rooms are always MSC4362-encrypted.
    /// A non-E2EE room would leak the session key in plaintext, so we
    /// hard-fail the send instead.
    ///
    /// # Parameters
    /// - `room_id` — the session room (exec room in mxdx topology).
    /// - `event_type` — must start with `m.call.`. Unknown call event
    ///   types (outside the five recognized in `mxdx-p2p`) are accepted
    ///   on the send side for forward compatibility with future Matrix
    ///   VoIP spec additions; the receive-side parser's Unknown variant
    ///   handles the inverse case.
    /// - `content` — the already-built event content (typically
    ///   `serde_json::to_value(&build_invite(...))` from mxdx-p2p).
    ///
    /// Returns the Matrix event ID of the sent event.
    pub async fn send_call_event(
        &self,
        room_id: &RoomId,
        event_type: &str,
        content: Value,
    ) -> Result<String> {
        if !event_type.starts_with("m.call.") {
            return Err(MatrixClientError::Other(anyhow::anyhow!(
                "send_call_event refused: event type `{}` is not an m.call.* type",
                event_type
            )));
        }

        let room = self
            .client
            .get_room(room_id)
            .ok_or_else(|| MatrixClientError::RoomNotFound(room_id.to_string()))?;

        if !room.encryption_state().is_encrypted() {
            return Err(MatrixClientError::Other(anyhow::anyhow!(
                "send_call_event refused: room {} is not E2EE — cardinal rule requires encrypted rooms",
                room_id
            )));
        }

        // Route through the same room.send_raw path as send_event and
        // send_megolm — matrix-sdk Megolm-encrypts the content in-flight
        // using the room's existing outbound session. No new encryption
        // code, no separate keystore.
        let response = room.send_raw(event_type, content).await?;
        Ok(response.event_id.to_string())
    }

    /// Encrypt content via the room's Megolm session, returning a sealed
    /// [`Megolm<Bytes>`] containing the already-encrypted ciphertext.
    ///
    /// Per ADR `2026-04-15-megolm-bytes-newtype.md` (second addendum —
    /// byte-identical ciphertext restored), the `Megolm<Bytes>` now wraps
    /// the actual `m.room.encrypted` JSON produced by
    /// `OlmMachine::encrypt_room_event_raw`. Both the P2P path and the
    /// Matrix fallback path carry the same bytes:
    ///
    /// - **Matrix fallback**: [`Self::send_megolm`] posts the already-
    ///   encrypted content as `m.room.encrypted` without re-encrypting.
    /// - **P2P path**: `P2PTransport::try_send(Megolm<Bytes>)` wraps the
    ///   ciphertext in an AES-GCM frame.
    ///
    /// Uses `Client::olm_machine_for_testing()` per ADR
    /// `docs/adr/2026-04-16-matrix-sdk-testing-feature.md` — the `testing`
    /// cargo feature gates the only stable accessor for
    /// `OlmMachine::encrypt_room_event_raw` in matrix-sdk 0.16.
    pub async fn encrypt_for_room(
        &self,
        room_id: &RoomId,
        event_type: &str,
        content: Value,
    ) -> Result<crate::Megolm<crate::Bytes>> {
        let room = self
            .client
            .get_room(room_id)
            .ok_or_else(|| MatrixClientError::RoomNotFound(room_id.to_string()))?;

        if !room.encryption_state().is_encrypted() {
            return Err(MatrixClientError::Other(anyhow::anyhow!(
                "encrypt_for_room refused: room {} is not E2EE — cardinal rule requires encrypted rooms",
                room_id
            )));
        }

        // Ensure room members and keys are synced so the Megolm outbound
        // session exists. sync_members() is public and idempotent.
        room.sync_members()
            .await
            .map_err(|e| MatrixClientError::Other(e.into()))?;

        // ADR 2026-04-16-matrix-sdk-testing-feature.md: use olm_machine_for_testing()
        // to access OlmMachine::encrypt_room_event_raw — the only stable accessor in
        // matrix-sdk 0.16. The `testing` feature gates this, not the underlying primitive.
        let olm_guard = self.client.olm_machine_for_testing().await;
        let olm = olm_guard
            .as_ref()
            .ok_or_else(|| MatrixClientError::Other(anyhow::anyhow!(
                "OlmMachine not initialized — E2EE not ready"
            )))?;

        let raw_content = Raw::from_json(
            serde_json::value::to_raw_value(&content)
                .map_err(|e| MatrixClientError::Other(anyhow::anyhow!("serialize content: {e}")))?
        );

        let encrypted = olm
            .encrypt_room_event_raw(room_id, event_type, &raw_content)
            .await
            .map_err(|e| MatrixClientError::Other(anyhow::anyhow!("Megolm encrypt: {e}")))?;

        let encrypted_bytes = serde_json::to_vec(&encrypted)
            .map_err(|e| MatrixClientError::Other(anyhow::anyhow!("serialize encrypted: {e}")))?;

        Ok(crate::crypto_envelope::Megolm(encrypted_bytes))
    }

    /// Send a `Megolm<Bytes>` payload via the Matrix fallback path.
    ///
    /// The payload is already Megolm-encrypted (produced by
    /// [`Self::encrypt_for_room`]). This method sends it as an
    /// `m.room.encrypted` event without re-encrypting — both P2P and
    /// Matrix paths carry byte-identical ciphertext.
    ///
    /// Returns the Matrix event ID of the sent event.
    pub async fn send_megolm(
        &self,
        room_id: &RoomId,
        _event_type: &str,
        payload: crate::Megolm<crate::Bytes>,
    ) -> Result<String> {
        let room = self
            .client
            .get_room(room_id)
            .ok_or_else(|| MatrixClientError::RoomNotFound(room_id.to_string()))?;

        if !room.encryption_state().is_encrypted() {
            return Err(MatrixClientError::Other(anyhow::anyhow!(
                "send_megolm refused: room {} is not E2EE — cardinal rule requires encrypted rooms",
                room_id
            )));
        }

        // The payload is already m.room.encrypted JSON from encrypt_for_room.
        // Send directly without re-encrypting.
        let bytes = payload.into_ciphertext_bytes();
        let content: Raw<ruma::events::room::encrypted::RoomEncryptedEventContent> =
            Raw::from_json(
                serde_json::from_slice::<Box<serde_json::value::RawValue>>(&bytes)
                    .map_err(|e| MatrixClientError::Other(anyhow::anyhow!("parse encrypted payload: {e}")))?,
            );

        let txn_id = TransactionId::new();
        let request = send_message_event::v3::Request::new_raw(
            room_id.to_owned(),
            txn_id,
            "m.room.encrypted".into(),
            content.cast(),
        );
        let response = self
            .client
            .send(request)
            .await
            .map_err(|e| MatrixClientError::Other(e.into()))?;

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

    /// Perform a sync with `full_state: true`.
    ///
    /// Forces the server to re-send all room state, even for rooms the client
    /// already knows about. This is needed after receiving new E2EE keys so
    /// the SDK can re-decrypt MSC4362 encrypted state events that failed to
    /// decrypt during earlier incremental syncs.
    pub async fn sync_full_state(&self) -> Result<()> {
        self.client
            .sync_once(
                SyncSettings::default()
                    .timeout(Duration::from_secs(2))
                    .full_state(true),
            )
            .await?;
        Ok(())
    }

    /// Read all state events of a given type from the SDK's local cache.
    ///
    /// Unlike [`get_room_state`] (HTTP call), this reads from the SDK's
    /// local store populated by `sync_once`.
    ///
    /// For MSC4362-encrypted state events the SDK stores them under
    /// `m.room.encrypted` in the state store, NOT under the inner type.
    /// This method handles that: if no events are found under the requested
    /// type, it queries `m.room.encrypted` events and decrypts them,
    /// filtering for those whose inner type matches `event_type`.
    ///
    /// Returns `(state_key, content)` pairs.
    pub async fn get_state_events_cached(
        &self,
        room_id: &RoomId,
        event_type: &str,
    ) -> Result<Vec<(String, Value)>> {
        use matrix_sdk::deserialized_responses::RawAnySyncOrStrippedState;
        use matrix_sdk::ruma::events::StateEventType;

        let room = self
            .client
            .get_room(room_id)
            .ok_or_else(|| MatrixClientError::RoomNotFound(room_id.to_string()))?;

        // First, try the requested type directly (works for unencrypted rooms
        // or if the SDK ever starts storing decrypted MSC4362 under the inner type).
        let ev_type = StateEventType::from(event_type);
        let raws = room
            .get_state_events(ev_type)
            .await
            .map_err(|e| MatrixClientError::Other(e.into()))?;

        if !raws.is_empty() {
            let mut results = Vec::new();
            for raw in raws {
                let raw_json = match &raw {
                    RawAnySyncOrStrippedState::Sync(r) => r.json().get(),
                    RawAnySyncOrStrippedState::Stripped(r) => r.json().get(),
                };
                let value: Value = match serde_json::from_str(raw_json) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let state_key = value
                    .get("state_key")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let content = value.get("content").cloned().unwrap_or_default();
                results.push((state_key, content));
            }
            if !results.is_empty() {
                return Ok(results);
            }
        }

        // MSC4362 fallback: the SDK stores encrypted state events under
        // m.room.encrypted. Read those and decrypt, filtering for the
        // requested inner event type.
        let encrypted_type = StateEventType::from("m.room.encrypted");
        let encrypted_raws = room
            .get_state_events(encrypted_type)
            .await
            .map_err(|e| MatrixClientError::Other(e.into()))?;

        let mut results = Vec::new();
        for raw in encrypted_raws {
            let raw_json_str = match &raw {
                RawAnySyncOrStrippedState::Sync(r) => r.json().get().to_owned(),
                RawAnySyncOrStrippedState::Stripped(r) => r.json().get().to_owned(),
            };

            // Try to decrypt via the room's decrypt_event method.
            // Cast the raw state event to the type expected by decrypt_event.
            if let RawAnySyncOrStrippedState::Sync(sync_raw) = &raw {
                use matrix_sdk::ruma::events::room::encrypted::OriginalSyncRoomEncryptedEvent;
                let cast_raw: &matrix_sdk::ruma::serde::Raw<OriginalSyncRoomEncryptedEvent> =
                    sync_raw.cast_ref_unchecked();
                match room.decrypt_event(cast_raw, None).await {
                    Ok(timeline_event) => {
                        let decrypted_json_str = timeline_event.raw().json().get();
                        let decrypted: Value = match serde_json::from_str(decrypted_json_str) {
                            Ok(v) => v,
                            Err(_) => continue,
                        };
                        // Check if the decrypted event's type matches our target
                        let inner_type = decrypted
                            .get("type")
                            .and_then(|t| t.as_str())
                            .unwrap_or("");
                        if inner_type != event_type {
                            continue;
                        }
                        let state_key = decrypted
                            .get("state_key")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let content = decrypted.get("content").cloned().unwrap_or_default();
                        results.push((state_key, content));
                    }
                    Err(e) => {
                        tracing::trace!(
                            error = %e,
                            "get_state_events_cached: failed to decrypt m.room.encrypted state event"
                        );
                        continue;
                    }
                }
            } else {
                // Stripped events (invited rooms) — try to parse directly
                let value: Value = match serde_json::from_str(&raw_json_str) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let inner_type = value
                    .get("type")
                    .and_then(|t| t.as_str())
                    .unwrap_or("");
                if inner_type == event_type {
                    let state_key = value
                        .get("state_key")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let content = value.get("content").cloned().unwrap_or_default();
                    results.push((state_key, content));
                }
            }
        }

        Ok(results)
    }

    /// Check worker liveness via REST API, without requiring E2EE decryption.
    ///
    /// For MSC4362 rooms, the SDK may not be able to decrypt telemetry state
    /// events (missing megolm keys). This method reads the full room state via
    /// the REST API and looks for telemetry events by checking their
    /// `origin_server_ts` (server-set, always visible even for encrypted events).
    ///
    /// Returns `(state_key, content)` pairs. For encrypted events where
    /// decryption isn't possible, returns a synthetic content with just the
    /// `origin_server_ts` converted to an ISO timestamp and `status: "online"`.
    pub async fn get_telemetry_via_rest(
        &self,
        room_id: &RoomId,
        event_type: &str,
    ) -> Result<Vec<(String, Value)>> {
        let homeserver = self.inner().homeserver().to_string();
        let access_token = self
            .access_token()
            .ok_or_else(|| MatrixClientError::Other(anyhow::anyhow!("not logged in")))?;

        let rest = crate::rest::RestClient::new(&homeserver, &access_token);
        let all_state = rest
            .get_room_full_state(room_id)
            .await
            .map_err(|e| MatrixClientError::Other(e))?;

        let mut results = Vec::new();
        for event in all_state {
            let ev_type = event.get("type").and_then(|t| t.as_str()).unwrap_or("");
            let state_key = event
                .get("state_key")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            // Direct match (unencrypted rooms or server that exposes inner type)
            if ev_type == event_type {
                let content = event.get("content").cloned().unwrap_or_default();
                results.push((state_key, content));
                continue;
            }

            // MSC4362: encrypted state events appear as m.room.encrypted.
            // The state_key is "{inner_type}:{original_state_key}", e.g.
            // "org.mxdx.host_telemetry:worker/belthanior.liamhelmer.e2etest-test1".
            // We can't decrypt them here, but we CAN read origin_server_ts
            // to determine freshness.
            let msc4362_prefix = format!("{}:", event_type);
            if ev_type == "m.room.encrypted" && state_key.starts_with(&msc4362_prefix) {
                // Extract the original state_key from the compound key
                let state_key = state_key[msc4362_prefix.len()..].to_string();
                let origin_ts = event
                    .get("origin_server_ts")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);

                if origin_ts == 0 {
                    continue;
                }

                // Convert origin_server_ts (millis since epoch) to ISO 8601
                let secs = origin_ts / 1000;
                let millis = origin_ts % 1000;
                let day_secs = secs % 86400;
                let hour = day_secs / 3600;
                let minute = (day_secs % 3600) / 60;
                let second = day_secs % 60;
                let days = secs / 86400;
                // Civil date from days since epoch
                let z = days as i64 + 719468;
                let era = if z >= 0 { z } else { z - 146096 } / 146097;
                let doe = (z - era * 146097) as u64;
                let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
                let y = yoe as i64 + era * 400;
                let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
                let mp = (5 * doy + 2) / 153;
                let d = doy - (153 * mp + 2) / 5 + 1;
                let m = if mp < 10 { mp + 3 } else { mp - 9 };
                let y = if m <= 2 { y + 1 } else { y };
                let ts = format!(
                    "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
                    y, m, d, hour, minute, second, millis
                );

                // Synthetic telemetry content using server timestamp.
                // Capabilities are unknown (encrypted), so empty = allow all.
                let synthetic = serde_json::json!({
                    "timestamp": ts,
                    "heartbeat_interval_ms": 60000,
                    "status": "online",
                    "capabilities": [],
                    "_encrypted": true,
                });
                results.push((state_key, synthetic));
            }
        }

        Ok(results)
    }

    /// Sync and collect decrypted timeline events for a specific room.
    ///
    /// Performs a single sync with the given timeout (server long-poll duration),
    /// then extracts new timeline events from the response. Events encrypted with
    /// megolm keys already in the crypto store are automatically decrypted by the SDK.
    ///
    /// If encrypted events are found that couldn't be decrypted (key not yet received),
    /// an additional sync is performed to receive the key, then `room.messages()` is
    /// used to read retroactively-decrypted events from the server.
    ///
    /// The caller should poll this in a loop with a short timeout (e.g., 2s) to
    /// receive events promptly.
    ///
    /// # Historical bug
    ///
    /// Earlier, the `room.messages()` fallback was gated on
    /// `saw_encrypted && collected.is_empty()`. That guard is wrong: when a
    /// single sync batch contains both a decryptable event (e.g. a telemetry
    /// state update) *and* an undecryptable `m.room.encrypted` event (e.g. a
    /// session output whose megolm key has not yet arrived via to-device),
    /// `collected` would be non-empty so the fallback was skipped. The
    /// encrypted event was silently dropped and the next sync advanced past
    /// it — the client would then wait forever for a result that was already
    /// posted. The fix always runs the fallback whenever any encrypted event
    /// was observed, and deduplicates by `event_id` against events we already
    /// captured from the inline sync pass.
    pub async fn sync_and_collect_events(
        &self,
        room_id: &RoomId,
        timeout: Duration,
    ) -> Result<Vec<Value>> {
        let response = self.client
            .sync_once(SyncSettings::default().timeout(timeout))
            .await?;

        let mut collected: Vec<Value> = Vec::new();
        let mut seen_ids: HashSet<String> = HashSet::new();
        let mut saw_encrypted = false;

        if let Some(joined) = response.rooms.joined.get(&room_id.to_owned()) {
            let timeline_count = joined.timeline.events.len();
            for timeline_event in &joined.timeline.events {
                let json_str = timeline_event.raw().json().get();
                if let Ok(json) = serde_json::from_str::<Value>(json_str) {
                    let event_type = json.get("type").and_then(|t| t.as_str());
                    let is_state = json.get("state_key").is_some();
                    match classify_sync_event(event_type, is_state) {
                        SyncFilterDecision::Encrypted => {
                            saw_encrypted = true;
                        }
                        SyncFilterDecision::InfraIgnored => {}
                        SyncFilterDecision::Deliver => {
                            if let Some(eid) = json.get("event_id").and_then(|e| e.as_str()) {
                                seen_ids.insert(eid.to_string());
                            }
                            collected.push(json);
                        }
                    }
                }
            }
            if timeline_count > 0 {
                tracing::info!(
                    timeline_events = timeline_count,
                    collected = collected.len(),
                    saw_encrypted,
                    room_id = %room_id,
                    "sync_and_collect_events: processed timeline"
                );
            }
        }

        // If we saw any undecryptable events, the megolm key may be arriving
        // in the next sync via to-device. Do one more sync to receive it, then
        // read via room.messages() which reflects retroactive decryption.
        //
        // Do NOT gate this on `collected.is_empty()` — if the same sync batch
        // contained a decryptable event alongside the encrypted one, the old
        // guard would skip the fallback and the encrypted event (e.g. a
        // session result) would be silently dropped.
        if saw_encrypted {
            tracing::info!("saw undecryptable events, doing extra sync for key exchange");
            let _ = self.client
                .sync_once(SyncSettings::default().timeout(Duration::from_secs(2)))
                .await;

            // Now read from the server — the SDK will decrypt with newly-received keys
            let room = self.client.get_room(room_id);
            if let Some(room) = room {
                if let Ok(messages) = room.messages(MessagesOptions::backward()).await {
                    let msg_count = messages.chunk.len();
                    let mut still_encrypted = 0u32;
                    let mut decrypted_types = Vec::new();
                    for event in &messages.chunk {
                        let json_str = event.raw().json().get();
                        if let Ok(json) = serde_json::from_str::<Value>(json_str) {
                            let event_type = json.get("type").and_then(|t| t.as_str());
                            let is_state = json.get("state_key").is_some();
                            match classify_sync_event(event_type, is_state) {
                                SyncFilterDecision::Encrypted => {
                                    still_encrypted += 1;
                                }
                                SyncFilterDecision::InfraIgnored => {}
                                SyncFilterDecision::Deliver => {
                                    if let Some(t) = event_type {
                                        decrypted_types.push(t.to_string());
                                    }
                                    // Dedupe against the inline sync pass so
                                    // we don't return the same event twice
                                    // within a single call.
                                    if let Some(eid) = json
                                        .get("event_id")
                                        .and_then(|e| e.as_str())
                                    {
                                        if !seen_ids.insert(eid.to_string()) {
                                            continue;
                                        }
                                    }
                                    collected.push(json);
                                }
                            }
                        }
                    }
                    tracing::info!(
                        msg_count,
                        still_encrypted,
                        decrypted = decrypted_types.len(),
                        types = ?decrypted_types,
                        "messages() fallback results"
                    );
                }
            }
        }

        if !collected.is_empty() {
            tracing::info!(
                total = collected.len(),
                room_id = %room_id,
                "sync_and_collect_events: returning events"
            );
        }
        Ok(collected)
    }

    /// Wait until E2EE key exchange completes for a room, with timeout.
    ///
    /// **Fast path**: If the crypto store already has keys for all room members
    /// (e.g., from a prior session), returns immediately without syncing.
    ///
    /// **Slow path**: If any keys are missing (fresh login, new member joined),
    /// syncs in a loop until keys arrive or timeout is reached.
    pub async fn wait_for_key_exchange(&self, room_id: &RoomId, timeout: Duration) -> Result<()> {
        // Fast path: check if we already have keys cached from a prior session.
        // This avoids any network round-trips on session restore.
        if self.has_cached_keys_for_room(room_id).await? {
            tracing::info!(room_id = %room_id, "E2EE keys already cached, skipping sync loop");
            return Ok(());
        }

        // Slow path: keys missing, need to sync until they arrive
        tracing::info!(room_id = %room_id, "E2EE keys not cached, entering sync loop");
        let deadline = tokio::time::Instant::now() + timeout;

        while tokio::time::Instant::now() < deadline {
            self.sync_once().await?;

            if self.has_cached_keys_for_room(room_id).await? {
                return Ok(());
            }
        }

        Err(MatrixClientError::KeyExchangeTimeout(format!(
            "Timed out waiting for key exchange in room {room_id}"
        )))
    }

    /// Check if the crypto store already has device keys for all members of a room.
    /// Returns true if we can encrypt to all members without needing a sync.
    async fn has_cached_keys_for_room(&self, room_id: &RoomId) -> Result<bool> {
        let room = match self.client.get_room(room_id) {
            Some(r) => r,
            None => return Ok(false),
        };

        // Room encryption state not yet known — need sync
        if !room.encryption_state().is_encrypted() {
            let member_count = room.joined_members_count();
            if member_count <= 1 {
                // Single-member room: no one to exchange keys with
                return Ok(true);
            }
            return Ok(false);
        }

        let members = room
            .members(matrix_sdk::RoomMemberships::ACTIVE)
            .await
            .map_err(|e| MatrixClientError::Other(e.into()))?;

        if members.len() <= 1 {
            return Ok(true);
        }

        for member in &members {
            let user_id = member.user_id();
            let devices = self
                .client
                .encryption()
                .get_user_devices(user_id)
                .await
                .map_err(|e| MatrixClientError::Other(e.into()))?;

            if devices.devices().count() == 0 {
                return Ok(false);
            }
        }

        Ok(true)
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

    /// Returns the current access token from the underlying matrix-sdk client,
    /// if logged in.
    pub fn access_token(&self) -> Option<String> {
        self.client.access_token()
    }

    /// Leave and forget a room. Best-effort; non-fatal if the room is unknown.
    pub async fn leave_and_forget_room(
        &self,
        room_id: &matrix_sdk::ruma::RoomId,
    ) -> Result<()> {
        if let Some(room) = self.client.get_room(room_id) {
            if let Err(e) = room.leave().await {
                tracing::warn!(room_id=%room_id, error=%e, "leave_room failed");
            }
            if let Err(e) = room.forget().await {
                tracing::warn!(room_id=%room_id, error=%e, "forget_room failed");
            }
        }
        Ok(())
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

    // ── P2P ephemeral handshake key (ADR 2026-04-16-ephemeral-key-cross-cert) ──

    /// Publish the local device's per-session ephemeral Ed25519 public key
    /// in a Megolm-encrypted state event `m.mxdx.p2p.ephemeral_key`.
    ///
    /// The state_key is the publisher's device_id (not user_id) so multiple
    /// devices on the same account can each carry their own ephemeral key.
    /// Content is `{ ephemeral_ed25519_b64, published_at }`.
    ///
    /// Cardinal rule: the room MUST be E2EE (MSC4362 is enabled project-wide).
    /// `send_state_event_raw` writes through `room.send_state_event_raw` which
    /// picks up the room's encryption config and Megolm-encrypts the state
    /// event in flight.
    pub async fn publish_p2p_ephemeral_key(
        &self,
        room_id: &RoomId,
        device_id: &str,
        ephemeral_public_key: [u8; 32],
    ) -> Result<()> {
        let room = self
            .client
            .get_room(room_id)
            .ok_or_else(|| MatrixClientError::RoomNotFound(room_id.to_string()))?;

        if !room.encryption_state().is_encrypted() {
            return Err(MatrixClientError::Other(anyhow::anyhow!(
                "publish_p2p_ephemeral_key refused: room {} is not E2EE — cardinal rule requires encrypted rooms (MSC4362 for state events)",
                room_id
            )));
        }

        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD_NO_PAD
            .encode(ephemeral_public_key);
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let content = serde_json::json!({
            "ephemeral_ed25519_b64": b64,
            "published_at": now_ms,
        });
        self.send_state_event(room_id, "m.mxdx.p2p.ephemeral_key", device_id, content)
            .await
    }

    /// Look up a peer device's per-session ephemeral Ed25519 public key via
    /// the session room's cached state events.
    ///
    /// Returns `None` if:
    /// - no matching state event exists
    /// - the publishing device is not cross-signed by its owner (rejected
    ///   per ADR 2026-04-16-ephemeral-key-cross-cert.md)
    /// - the payload is malformed
    ///
    /// The cross-signing check gates acceptance: a rogue device injected
    /// into the user's account without cross-signing is rejected even if it
    /// publishes a valid-looking ephemeral key.
    pub async fn get_p2p_ephemeral_key(
        &self,
        room_id: &RoomId,
        peer_user_id: &UserId,
        peer_device_id: &str,
    ) -> Result<Option<[u8; 32]>> {
        // Gate on cross-signing first.
        let devices = self
            .client
            .encryption()
            .get_user_devices(peer_user_id)
            .await
            .map_err(|e| MatrixClientError::Other(e.into()))?;

        let device = match devices
            .devices()
            .find(|d| d.device_id().as_str() == peer_device_id)
        {
            Some(d) => d,
            None => {
                tracing::debug!(
                    user_id = %peer_user_id,
                    device_id = peer_device_id,
                    "get_p2p_ephemeral_key: peer device unknown — rejecting"
                );
                return Ok(None);
            }
        };

        if !device.is_cross_signed_by_owner() {
            tracing::warn!(
                user_id = %peer_user_id,
                device_id = peer_device_id,
                "get_p2p_ephemeral_key: peer device not cross-signed — rejecting"
            );
            return Ok(None);
        }

        let events = self
            .get_state_events_cached(room_id, "m.mxdx.p2p.ephemeral_key")
            .await?;

        for (state_key, content) in events {
            if state_key != peer_device_id {
                continue;
            }
            let b64 = match content.get("ephemeral_ed25519_b64").and_then(|v| v.as_str()) {
                Some(s) => s,
                None => continue,
            };
            use base64::Engine;
            let bytes = match base64::engine::general_purpose::STANDARD_NO_PAD.decode(b64) {
                Ok(b) => b,
                Err(_) => continue,
            };
            if bytes.len() != 32 {
                continue;
            }
            let mut out = [0u8; 32];
            out.copy_from_slice(&bytes);
            return Ok(Some(out));
        }
        Ok(None)
    }
}

/// Classification of a Matrix event type for mxdx's sync receive path.
///
/// `sync_and_collect_events` passes events of every type through to consumers
/// except a small infra denylist. This enum documents the classification
/// and makes it unit-testable — it's used internally by the sync path match
/// arm at `sync_and_collect_events`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncFilterDecision {
    /// The event is undecryptable room encryption metadata — drives the
    /// saw_encrypted fallback path (for non-state events).
    Encrypted,
    /// Infra event that's not a consumer payload (membership, encryption
    /// config, power levels). Silently ignored by the sync collector.
    InfraIgnored,
    /// Consumer payload — includes all mxdx session events (`mxdx.session.*`),
    /// all Matrix VoIP call events (`m.call.*`), telemetry events, and any
    /// other application-level event types.
    Deliver,
}

/// Classify a Matrix event type for the sync receive path. Pure function,
/// testable without a live Matrix client.
///
/// Recognized Matrix VoIP event types (`m.call.*`) are classified as
/// [`SyncFilterDecision::Deliver`] so the Phase 5 state machine receives
/// them through `sync_and_collect_events`. The function is intentionally
/// permissive: unknown types default to `Deliver` rather than being
/// dropped, so future event schemas don't require coordinated code changes.
pub fn classify_sync_event(event_type: Option<&str>, is_state: bool) -> SyncFilterDecision {
    match event_type {
        Some("m.room.encrypted") => {
            // MSC4362 state events are handled by the SDK's state processor;
            // timeline-level encrypted events drive the re-sync fallback.
            if is_state {
                SyncFilterDecision::InfraIgnored
            } else {
                SyncFilterDecision::Encrypted
            }
        }
        Some("m.room.encryption") | Some("m.room.member") | Some("m.room.power_levels") => {
            SyncFilterDecision::InfraIgnored
        }
        _ => SyncFilterDecision::Deliver,
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

    // -----------------------------------------------------------------------
    // T-43 — sync filter classifier tests. Ensure m.call.* events are NOT
    // dropped by the sync path; the Phase 5 state machine needs them.
    // -----------------------------------------------------------------------

    #[test]
    fn classify_delivers_all_m_call_types() {
        for ty in crate::CALL_EVENT_TYPES {
            assert_eq!(
                classify_sync_event(Some(ty), false),
                SyncFilterDecision::Deliver,
                "m.call.* event type `{ty}` must pass through sync filter"
            );
        }
    }

    #[test]
    fn classify_delivers_unknown_m_call_types_for_forward_compat() {
        // Future Matrix call event types (m.call.reject, m.call.negotiate,
        // etc.) must also be delivered to consumers — the Rust parser
        // surfaces them as ParsedCallEvent::Unknown. If the sync filter
        // were a positive-only allowlist, spec additions would silently
        // drop until the allowlist was updated.
        assert_eq!(
            classify_sync_event(Some("m.call.reject"), false),
            SyncFilterDecision::Deliver
        );
        assert_eq!(
            classify_sync_event(Some("m.call.negotiate"), false),
            SyncFilterDecision::Deliver
        );
    }

    #[test]
    fn classify_delivers_mxdx_session_events() {
        assert_eq!(
            classify_sync_event(Some("mxdx.session.start"), false),
            SyncFilterDecision::Deliver
        );
        assert_eq!(
            classify_sync_event(Some("mxdx.session.output"), false),
            SyncFilterDecision::Deliver
        );
    }

    #[test]
    fn classify_infra_ignored() {
        for ty in [
            "m.room.encryption",
            "m.room.member",
            "m.room.power_levels",
        ] {
            assert_eq!(
                classify_sync_event(Some(ty), false),
                SyncFilterDecision::InfraIgnored,
                "type {ty} should be infra-ignored"
            );
        }
    }

    #[test]
    fn classify_timeline_encrypted_triggers_fallback() {
        // m.room.encrypted on a timeline event (no state_key) drives the
        // saw_encrypted retry logic.
        assert_eq!(
            classify_sync_event(Some("m.room.encrypted"), false),
            SyncFilterDecision::Encrypted
        );
    }

    #[test]
    fn classify_state_encrypted_is_infra_ignored() {
        // MSC4362 state events (m.room.encrypted with state_key) are
        // handled by the SDK's state processor, not the timeline
        // fallback — classified as InfraIgnored.
        assert_eq!(
            classify_sync_event(Some("m.room.encrypted"), true),
            SyncFilterDecision::InfraIgnored
        );
    }

    #[test]
    fn classify_missing_type_delivers() {
        // No `type` field — default permissive case, don't drop.
        assert_eq!(
            classify_sync_event(None, false),
            SyncFilterDecision::Deliver
        );
    }

    #[test]
    fn call_event_types_constant_matches_mxdx_p2p() {
        // Locks the list of recognized call event types. If this changes,
        // crates/mxdx-p2p/src/signaling/parse.rs CALL_EVENT_TYPES must
        // change in lockstep. Both reference the 2026-04-15 m.call
        // wire-format ADR.
        assert_eq!(
            crate::CALL_EVENT_TYPES,
            &[
                "m.call.invite",
                "m.call.answer",
                "m.call.candidates",
                "m.call.hangup",
                "m.call.select_answer"
            ]
        );
    }
}
