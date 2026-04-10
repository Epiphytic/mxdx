use anyhow::Result;
use mxdx_matrix::{MatrixClient, MultiHsClient};
use mxdx_types::events::session::{
    SESSION_HEARTBEAT, SESSION_OUTPUT, SESSION_RESULT, SESSION_START,
};
use serde::{de::DeserializeOwned, Serialize};
use std::time::Duration;

/// Abstraction over Matrix room operations for the client.
/// This trait allows testing with mocks without requiring a real Matrix server.
pub trait ClientRoomOps: Send + Sync {
    /// Find a room by name or alias
    fn find_room(
        &self,
        room_name: &str,
    ) -> impl std::future::Future<Output = Result<Option<String>>> + Send;

    /// Post an event to a room
    fn post_event(
        &self,
        room_id: &str,
        event_type: &str,
        content: serde_json::Value,
    ) -> impl std::future::Future<Output = Result<String>> + Send;

    /// Post a threaded event to a session's thread
    fn post_to_thread(
        &self,
        room_id: &str,
        thread_root: &str,
        event_type: &str,
        content: serde_json::Value,
    ) -> impl std::future::Future<Output = Result<String>> + Send;

    /// Read state events of a given type from a room
    fn read_state_events(
        &self,
        room_id: &str,
        event_type: &str,
    ) -> impl std::future::Future<Output = Result<Vec<(String, serde_json::Value)>>> + Send;

    /// Sync and return incoming client-relevant events
    fn sync_events(
        &self,
    ) -> impl std::future::Future<Output = Result<Vec<IncomingClientEvent>>> + Send;
}

/// Client-side incoming events from Matrix sync.
#[derive(Debug, Clone)]
pub enum IncomingClientEvent {
    /// Worker has started a session
    SessionStart {
        event_id: String,
        session_uuid: String,
        content: serde_json::Value,
    },
    /// Worker is sending session output
    SessionOutput {
        event_id: String,
        session_uuid: String,
        content: serde_json::Value,
    },
    /// Worker heartbeat for an active session
    SessionHeartbeat {
        event_id: String,
        session_uuid: String,
        content: serde_json::Value,
    },
    /// Worker is reporting final session result
    SessionResult {
        event_id: String,
        session_uuid: String,
        content: serde_json::Value,
    },
}

/// Concrete holder for a client's room reference.
///
/// Holds the room_id for the target worker room. The actual Matrix SDK
/// integration (via `mxdx-matrix::MatrixClient`) will be wired up later.
pub struct ClientRoom {
    room_id: String,
}

impl ClientRoom {
    pub fn new(room_id: String) -> Self {
        Self { room_id }
    }

    pub fn room_id(&self) -> &str {
        &self.room_id
    }
}

/// Live Matrix-backed room operations for the client.
/// Wraps a `MultiHsClient` for multi-homeserver failover
/// and a specific room to execute commands against.
pub struct MatrixClientRoom {
    multi: MultiHsClient,
    room_id: mxdx_matrix::OwnedRoomId,
}

impl MatrixClientRoom {
    pub fn new(multi: MultiHsClient, room_id: mxdx_matrix::OwnedRoomId) -> Self {
        Self { multi, room_id }
    }

    /// Construct from a single `MatrixClient` (backward compat / testing).
    pub fn from_single_client(client: MatrixClient, room_id: mxdx_matrix::OwnedRoomId) -> Self {
        let server = "single".to_string();
        let multi = MultiHsClient::from_clients(vec![(server, client, 0.0)], None);
        Self { multi, room_id }
    }

    pub fn room_id(&self) -> &mxdx_matrix::RoomId {
        &self.room_id
    }

    /// Access the preferred (active) `MatrixClient` for operations not
    /// yet wrapped by `MultiHsClient` (e.g., state reads).
    pub fn client(&self) -> &MatrixClient {
        self.multi.preferred()
    }

    /// Access the `MultiHsClient` mutably (for send operations with failover).
    pub fn multi(&mut self) -> &mut MultiHsClient {
        &mut self.multi
    }

    /// Number of connected homeservers.
    pub fn server_count(&self) -> usize {
        self.multi.server_count()
    }

    /// Get the user ID of the logged-in user as a string.
    pub fn user_id_string(&self) -> String {
        self.multi.user_id().to_string()
    }

    /// Post an event with failover through MultiHsClient.
    pub async fn post_event_mut(
        &mut self,
        event_type: &str,
        content: serde_json::Value,
    ) -> Result<String> {
        let payload = serde_json::json!({
            "type": event_type,
            "content": content,
        });
        let event_id = self.multi.send_event(&self.room_id, payload).await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(event_id)
    }

    /// Sync events with failover through MultiHsClient.
    pub async fn sync_events_mut(&mut self) -> Result<Vec<IncomingClientEvent>> {
        let raw_events = self
            .multi
            .sync_and_collect_events(&self.room_id, Duration::from_secs(2))
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        if !raw_events.is_empty() {
            tracing::debug!(
                count = raw_events.len(),
                room_id = %self.room_id,
                "sync_events_mut received raw events"
            );
            for raw in &raw_events {
                let event_type = raw.get("type").and_then(|t| t.as_str()).unwrap_or("unknown");
                let has_uuid = raw.get("content")
                    .and_then(|c| c.get("session_uuid"))
                    .is_some();
                tracing::debug!(event_type, has_uuid, "raw event");
            }
        }

        let mut events = Vec::new();
        for raw in raw_events {
            let event_type = raw.get("type").and_then(|t| t.as_str()).unwrap_or("");
            let event_id = raw.get("event_id")
                .and_then(|e| e.as_str())
                .unwrap_or("")
                .to_string();
            let content = raw.get("content").cloned().unwrap_or_default();
            let session_uuid = content
                .get("session_uuid")
                .and_then(|u| u.as_str())
                .unwrap_or("")
                .to_string();

            if session_uuid.is_empty() {
                continue;
            }

            match event_type {
                SESSION_START => events.push(IncomingClientEvent::SessionStart {
                    event_id,
                    session_uuid,
                    content,
                }),
                SESSION_OUTPUT => events.push(IncomingClientEvent::SessionOutput {
                    event_id,
                    session_uuid,
                    content,
                }),
                SESSION_HEARTBEAT => events.push(IncomingClientEvent::SessionHeartbeat {
                    event_id,
                    session_uuid,
                    content,
                }),
                SESSION_RESULT => events.push(IncomingClientEvent::SessionResult {
                    event_id,
                    session_uuid,
                    content,
                }),
                _ => {} // Ignore unknown event types
            }
        }

        Ok(events)
    }
}

impl ClientRoomOps for MatrixClientRoom {
    async fn find_room(&self, room_name: &str) -> Result<Option<String>> {
        let topology = self.multi.preferred().find_launcher_space(room_name).await?;
        Ok(topology.map(|t| t.exec_room_id.to_string()))
    }

    async fn post_event(
        &self,
        _room_id: &str,
        event_type: &str,
        content: serde_json::Value,
    ) -> Result<String> {
        let payload = serde_json::json!({
            "type": event_type,
            "content": content,
        });
        let event_id = self.multi.preferred().send_event(&self.room_id, payload).await?;
        Ok(event_id)
    }

    async fn post_to_thread(
        &self,
        _room_id: &str,
        thread_root: &str,
        event_type: &str,
        content: serde_json::Value,
    ) -> Result<String> {
        let event_id = self
            .multi.preferred()
            .send_threaded_event(&self.room_id, event_type, thread_root, content)
            .await?;
        Ok(event_id)
    }

    async fn read_state_events(
        &self,
        _room_id: &str,
        event_type: &str,
    ) -> Result<Vec<(String, serde_json::Value)>> {
        // Read from SDK local cache (populated by sync). Handles MSC4362
        // decryption automatically. No network call.
        let cached = self
            .multi.preferred()
            .get_state_events_cached(&self.room_id, event_type)
            .await;
        // SDK cache may return old decrypted events from a prior megolm
        // session (the old state is stored under the inner type). Check if
        // the SDK cache shows a live worker first (fast path). Only fall
        // back to REST (which reads origin_server_ts from the full room
        // state) if the SDK cache is empty or shows stale/offline.
        let sdk_entries = cached.unwrap_or_default();

        // Fast path: if SDK cache has fresh data, skip REST entirely
        let sdk_summary = crate::liveness::summarize_worker_liveness(&sdk_entries);
        if sdk_summary.online > 0 {
            tracing::debug!(
                count = sdk_entries.len(),
                event_type,
                "read_state_events: SDK cache shows live worker, skipping REST"
            );
            return Ok(sdk_entries);
        }

        let rest_entries = self
            .multi.preferred()
            .get_telemetry_via_rest(&self.room_id, event_type)
            .await
            .unwrap_or_default();

        if sdk_entries.is_empty() && rest_entries.is_empty() {
            tracing::debug!(event_type, "read_state_events: no entries from SDK cache or REST");
            return Ok(vec![]);
        }

        // If we have both SDK and REST entries for the same state_key,
        // prefer whichever has the fresher timestamp. REST entries use
        // origin_server_ts (always correct), SDK entries use the
        // decrypted timestamp (may be stale if from an old session).
        if !rest_entries.is_empty() {
            let mut merged = std::collections::HashMap::new();
            for (key, val) in &sdk_entries {
                merged.insert(key.clone(), val.clone());
            }
            for (key, val) in &rest_entries {
                let rest_ts = val.get("timestamp").and_then(|t| t.as_str()).unwrap_or("");
                let existing = merged.get(key);
                let should_replace = match existing {
                    None => true,
                    Some(existing_val) => {
                        let existing_ts = existing_val.get("timestamp").and_then(|t| t.as_str()).unwrap_or("");
                        // REST entry is fresher if its timestamp is later
                        rest_ts > existing_ts
                    }
                };
                if should_replace {
                    merged.insert(key.clone(), val.clone());
                }
            }
            tracing::debug!(
                sdk = sdk_entries.len(),
                rest = rest_entries.len(),
                merged = merged.len(),
                event_type,
                "read_state_events: merged SDK cache + REST"
            );
            Ok(merged.into_iter().collect())
        } else {
            tracing::debug!(
                count = sdk_entries.len(),
                event_type,
                "read_state_events: using SDK cache only"
            );
            Ok(sdk_entries)
        }
    }

    async fn sync_events(&self) -> Result<Vec<IncomingClientEvent>> {
        let raw_events = self
            .multi.preferred()
            .sync_and_collect_events(&self.room_id, Duration::from_secs(5))
            .await?;

        let mut events = Vec::new();
        for raw in raw_events {
            let event_type = raw.get("type").and_then(|t| t.as_str()).unwrap_or("");
            let event_id = raw.get("event_id")
                .and_then(|e| e.as_str())
                .unwrap_or("")
                .to_string();
            let content = raw.get("content").cloned().unwrap_or_default();
            let session_uuid = content
                .get("session_uuid")
                .and_then(|u| u.as_str())
                .unwrap_or("")
                .to_string();

            if session_uuid.is_empty() {
                continue;
            }

            match event_type {
                SESSION_START => events.push(IncomingClientEvent::SessionStart {
                    event_id,
                    session_uuid,
                    content,
                }),
                SESSION_OUTPUT => events.push(IncomingClientEvent::SessionOutput {
                    event_id,
                    session_uuid,
                    content,
                }),
                SESSION_HEARTBEAT => events.push(IncomingClientEvent::SessionHeartbeat {
                    event_id,
                    session_uuid,
                    content,
                }),
                SESSION_RESULT => events.push(IncomingClientEvent::SessionResult {
                    event_id,
                    session_uuid,
                    content,
                }),
                _ => {} // Ignore unknown event types
            }
        }

        Ok(events)
    }
}

/// Connect to Matrix using multiple homeserver accounts and resolve the worker's exec room.
/// Returns a `MatrixClientRoom` ready for sending/receiving events with failover.
///
/// For backward compatibility, also accepts single-server connection parameters.
///
/// Session restore: When a keychain is available, tries to restore each server's
/// session (reusing the same device ID) before falling back to fresh login.
pub async fn connect_multi(
    accounts: &[mxdx_matrix::ServerAccount],
    worker_room: &str,
    direct_room_id: Option<&str>,
    force_new_device: bool,
) -> Result<MatrixClientRoom> {
    if accounts.is_empty() {
        anyhow::bail!(
            "No Matrix accounts configured (use --homeserver/--username/--password or config file)"
        );
    }

    tracing::info!(
        servers = accounts.len(),
        worker = %worker_room,
        "connecting to Matrix"
    );

    // Create keychain for session restore (OS keychain -> file fallback)
    let keychain: Box<dyn mxdx_types::identity::KeychainBackend> =
        match mxdx_types::keychain_chain::ChainedKeychain::default_chain() {
            Ok(kc) => Box::new(kc),
            Err(e) => {
                tracing::warn!(error = %e, "failed to create keychain, session restore disabled");
                Box::new(mxdx_types::identity::InMemoryKeychain::new())
            }
        };

    let store_base = mxdx_matrix::default_store_base_path("client");
    let (mut multi, fresh_logins) = MultiHsClient::connect_with_keychain(
        accounts,
        None,
        store_base,
        Some(keychain.as_ref()),
        force_new_device,
    )
    .await
    .map_err(|e| anyhow::anyhow!("{e}"))?;

    tracing::info!(
        user_id = %multi.user_id(),
        servers = multi.server_count(),
        preferred = %multi.preferred_server(),
        fresh_logins = ?fresh_logins,
        "connected to Matrix"
    );

    // After fresh login, remove passwords from config (now saved in keychain).
    // Set MXDX_KEEP_PASSWORDS=1 to skip stripping (used by E2E test suite).
    let keep_passwords = std::env::var("MXDX_KEEP_PASSWORDS").map_or(false, |v| v == "1");
    if !keep_passwords && fresh_logins.iter().any(|&f| f) {
        if let Err(e) = mxdx_types::config::remove_passwords_from_config("defaults.toml", None) {
            tracing::warn!(error = %e, "failed to remove passwords from config");
        }
    }

    let any_fresh = fresh_logins.iter().any(|&f| f);

    let room_id = if let Some(rid_str) = direct_room_id {
        // Use a direct room ID (bypasses space discovery, for E2E tests or pre-arranged rooms)
        let rid = mxdx_matrix::OwnedRoomId::try_from(rid_str)
            .map_err(|e| anyhow::anyhow!("Invalid room ID '{}': {}", rid_str, e))?;

        if any_fresh {
            // Fresh login: need to sync, join, and exchange keys from scratch
            multi.sync_once().await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            if let Err(e) = multi.join_room(&rid).await {
                tracing::warn!(room_id = %rid, error = %e, "join_room failed (may already be a member)");
            }
            tracing::info!(room_id = %rid, "waiting for E2EE key exchange");
            multi
                .wait_for_key_exchange(&rid, std::time::Duration::from_secs(15))
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
        } else {
            // Session restore: device already has keys, just do a quick sync
            // to catch up on any events we missed while offline
            multi.sync_once().await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        tracing::info!(room_id = %rid, "using direct room ID");
        rid
    } else {
        // Discover the worker's launcher space — clients must NEVER create rooms.
        // If no worker room is found, fail immediately with a clear error.
        let topology = multi.find_launcher_space(worker_room).await
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .ok_or_else(|| anyhow::anyhow!(
                "No worker room found for '{}'. Has the worker started and invited this client?",
                worker_room
            ))?;
        let rid = topology.exec_room_id;
        tracing::info!(exec_room = %rid, "discovered worker exec room");

        // Key exchange: on session restore, keys are already cached in the
        // crypto store so wait_for_key_exchange returns immediately (fast path).
        // On fresh login, this blocks until keys arrive.
        multi
            .wait_for_key_exchange(&rid, std::time::Duration::from_secs(15))
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        rid
    };

    // Bootstrap cross-signing: on fresh login this sets up keys;
    // on session restore this no-ops quickly (keys already exist).
    multi.bootstrap_and_sync_trust(&room_id).await;

    Ok(MatrixClientRoom::new(multi, room_id))
}

/// Connect to Matrix and resolve the worker's exec room (single-server backward compat).
/// Returns a `MatrixClientRoom` ready for sending/receiving events.
pub async fn connect(
    homeserver: &str,
    username: &str,
    password: &str,
    worker_room: &str,
    direct_room_id: Option<&str>,
) -> Result<MatrixClientRoom> {
    let accounts = vec![mxdx_matrix::ServerAccount {
        homeserver: homeserver.to_string(),
        username: username.to_string(),
        password: password.to_string(),
        danger_accept_invalid_certs: false,
    }];
    connect_multi(&accounts, worker_room, direct_room_id, false).await
}

/// Helper to serialize a typed event into a JSON Value for posting.
pub fn serialize_event<T: Serialize>(event: &T) -> Result<serde_json::Value> {
    Ok(serde_json::to_value(event)?)
}

/// Helper to deserialize a JSON Value into a typed event.
pub fn deserialize_event<T: DeserializeOwned>(value: &serde_json::Value) -> Result<T> {
    Ok(serde_json::from_value(value.clone())?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_event_produces_json() {
        let data = serde_json::json!({"bin": "/bin/echo", "args": ["hello"]});
        let value = serialize_event(&data).expect("serialization should succeed");
        assert_eq!(value["bin"], "/bin/echo");
        assert_eq!(value["args"], serde_json::json!(["hello"]));
    }

    #[test]
    fn deserialize_event_from_json() {
        let json = serde_json::json!({
            "uuid": "test-uuid-5678",
            "bin": "/usr/bin/ls",
        });

        let result: serde_json::Value = deserialize_event(&json).expect("deserialization should succeed");
        assert_eq!(result["uuid"], "test-uuid-5678");
        assert_eq!(result["bin"], "/usr/bin/ls");
    }

    #[test]
    fn incoming_client_event_variants_construct_and_match() {
        let events = vec![
            IncomingClientEvent::SessionStart {
                event_id: "$ev1".to_string(),
                session_uuid: "uuid-1".to_string(),
                content: serde_json::json!({"status": "started"}),
            },
            IncomingClientEvent::SessionOutput {
                event_id: "$ev2".to_string(),
                session_uuid: "uuid-2".to_string(),
                content: serde_json::json!({"data": "output line"}),
            },
            IncomingClientEvent::SessionHeartbeat {
                event_id: "$ev3".to_string(),
                session_uuid: "uuid-3".to_string(),
                content: serde_json::json!({"ts": 1700000000}),
            },
            IncomingClientEvent::SessionResult {
                event_id: "$ev4".to_string(),
                session_uuid: "uuid-4".to_string(),
                content: serde_json::json!({"exit_code": 0}),
            },
        ];

        let mut start_count = 0;
        let mut output_count = 0;
        let mut heartbeat_count = 0;
        let mut result_count = 0;

        for event in &events {
            match event {
                IncomingClientEvent::SessionStart { session_uuid, .. } => {
                    assert_eq!(session_uuid, "uuid-1");
                    start_count += 1;
                }
                IncomingClientEvent::SessionOutput { session_uuid, .. } => {
                    assert_eq!(session_uuid, "uuid-2");
                    output_count += 1;
                }
                IncomingClientEvent::SessionHeartbeat { session_uuid, .. } => {
                    assert_eq!(session_uuid, "uuid-3");
                    heartbeat_count += 1;
                }
                IncomingClientEvent::SessionResult { session_uuid, .. } => {
                    assert_eq!(session_uuid, "uuid-4");
                    result_count += 1;
                }
            }
        }

        assert_eq!(start_count, 1);
        assert_eq!(output_count, 1);
        assert_eq!(heartbeat_count, 1);
        assert_eq!(result_count, 1);
    }

    #[test]
    fn client_room_stores_and_returns_room_id() {
        let room = ClientRoom::new("!abc123:example.com".to_string());
        assert_eq!(room.room_id(), "!abc123:example.com");

        let room2 = ClientRoom::new("!xyz789:matrix.org".to_string());
        assert_eq!(room2.room_id(), "!xyz789:matrix.org");
    }
}
