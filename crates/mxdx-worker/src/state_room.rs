//! Worker state room management.
//!
//! `WorkerStateRoom` manages all persistent worker state in a private E2EE
//! Matrix room using custom state events. This replaces file-based persistence
//! (sessions.json, etc.) with a Matrix-native approach that supports
//! multi-homeserver replication.
//!
//! **Security**: All operations go through E2EE rooms. The `get_or_create`
//! flow validates that the room was created by a trusted entity and has
//! encryption enabled before use.

use anyhow::Result;
use mxdx_matrix::{MatrixClient, OwnedRoomId, OwnedUserId, RoomId};
use mxdx_types::events::state_room::{
    StateRoomEntry, StateRoomSession, StateRoomTopology, TrustedEntity, WorkerStateConfig,
    WorkerStateIdentity, WORKER_STATE_CONFIG, WORKER_STATE_IDENTITY, WORKER_STATE_ROOM,
    WORKER_STATE_ROOM_POINTER, WORKER_STATE_SESSION, WORKER_STATE_TOPOLOGY,
    WORKER_STATE_TRUSTED_CLIENT, WORKER_STATE_TRUSTED_COORDINATOR,
};
use mxdx_types::identity::{state_room_key, KeychainBackend};
use serde_json::Value;

// ---------------------------------------------------------------------------
// WorkerStateRoom
// ---------------------------------------------------------------------------

/// Manages worker state in a private E2EE Matrix room via custom state events.
///
/// Each worker device gets exactly one state room, identified by a deterministic
/// alias derived from (hostname, os_user, localpart). The room is E2EE-enabled
/// and validated on every access.
pub struct WorkerStateRoom {
    room_id: OwnedRoomId,
}

impl WorkerStateRoom {
    /// Find or create and validate the worker state room.
    ///
    /// Flow:
    /// 1. Check keychain for cached room ID
    /// 2. If found, validate trust (creator + encryption) and use it
    /// 3. If validation fails, fall through to alias lookup
    /// 4. Try alias-based discovery via `find_worker_state_room()`
    /// 5. If found, validate and cache; if not found, create new
    /// 6. Write initial identity state event
    pub async fn get_or_create(
        client: &MatrixClient,
        hostname: &str,
        os_user: &str,
        localpart: &str,
        keychain: &dyn KeychainBackend,
        trusted_coordinators: &[OwnedUserId],
    ) -> Result<Self> {
        let user_id = client.user_id();
        let kc_key = state_room_key(user_id.as_str());

        // Step 1: Check keychain for cached room ID
        if let Some(data) = keychain.get(&kc_key)? {
            if let Ok(room_id_str) = String::from_utf8(data) {
                if let Ok(room_id) = OwnedRoomId::try_from(room_id_str.as_str()) {
                    // Step 2: Validate trust
                    match client
                        .validate_state_room(&room_id, user_id, trusted_coordinators)
                        .await
                    {
                        Ok(()) => {
                            tracing::info!(
                                room_id = %room_id,
                                "using cached state room from keychain"
                            );
                            return Ok(Self { room_id });
                        }
                        Err(e) => {
                            // Step 3: Validation failed, fall through
                            tracing::warn!(
                                room_id = %room_id,
                                error = %e,
                                "cached state room failed validation, trying alias lookup"
                            );
                        }
                    }
                }
            }
        }

        // Step 4: Try alias-based discovery
        if let Some(room_id) = client
            .find_worker_state_room(hostname, os_user, localpart)
            .await?
        {
            // Step 5: Validate discovered room
            match client
                .validate_state_room(&room_id, user_id, trusted_coordinators)
                .await
            {
                Ok(()) => {
                    tracing::info!(
                        room_id = %room_id,
                        "found existing state room via alias"
                    );
                    // Cache in keychain
                    if let Err(e) = keychain.set(&kc_key, room_id.as_str().as_bytes()) {
                        tracing::warn!(error = %e, "failed to cache state room ID in keychain");
                    }
                    return Ok(Self { room_id });
                }
                Err(e) => {
                    tracing::warn!(
                        room_id = %room_id,
                        error = %e,
                        "discovered state room failed validation, creating new"
                    );
                }
            }
        }

        // Step 6: Create new state room
        let room_id = client
            .create_worker_state_room(hostname, os_user, localpart)
            .await?;
        tracing::info!(room_id = %room_id, "created new worker state room");

        // Cache in keychain
        if let Err(e) = keychain.set(&kc_key, room_id.as_str().as_bytes()) {
            tracing::warn!(error = %e, "failed to cache state room ID in keychain");
        }

        // Write initial identity state event
        let identity = WorkerStateIdentity {
            device_id: client
                .inner()
                .device_id()
                .map(|d| d.to_string())
                .unwrap_or_default(),
            host: hostname.to_string(),
            os_user: os_user.to_string(),
        };
        let identity_value = serde_json::to_value(&identity)?;
        client
            .send_state_event(&room_id, WORKER_STATE_IDENTITY, "", identity_value)
            .await?;

        Ok(Self { room_id })
    }

    /// Get the room ID of this state room.
    pub fn room_id(&self) -> &RoomId {
        &self.room_id
    }

    // ── Config CRUD ──────────────────────────────────────────────────────

    /// Write the worker configuration state event.
    pub async fn write_config(
        &self,
        client: &MatrixClient,
        config: &WorkerStateConfig,
    ) -> Result<()> {
        let content = serde_json::to_value(config)?;
        client
            .send_state_event(&self.room_id, WORKER_STATE_CONFIG, "", content)
            .await?;
        Ok(())
    }

    /// Read the current worker configuration, or `None` if not yet set.
    pub async fn read_config(
        &self,
        client: &MatrixClient,
    ) -> Result<Option<WorkerStateConfig>> {
        let value = client
            .get_room_state_event(&self.room_id, WORKER_STATE_CONFIG, "")
            .await?;
        deserialize_if_present(&value)
    }

    // ── Session CRUD ─────────────────────────────────────────────────────

    /// Write a session state event with state key `{device_id}/{uuid}`.
    pub async fn write_session(
        &self,
        client: &MatrixClient,
        device_id: &str,
        uuid: &str,
        session: &StateRoomSession,
    ) -> Result<()> {
        let state_key = format_session_key(device_id, uuid);
        let content = serde_json::to_value(session)?;
        client
            .send_state_event(&self.room_id, WORKER_STATE_SESSION, &state_key, content)
            .await?;
        Ok(())
    }

    /// Remove a session by writing empty content to its state key.
    ///
    /// Matrix state events cannot be deleted, but empty content signals removal.
    pub async fn remove_session(
        &self,
        client: &MatrixClient,
        device_id: &str,
        uuid: &str,
    ) -> Result<()> {
        let state_key = format_session_key(device_id, uuid);
        client
            .send_state_event(
                &self.room_id,
                WORKER_STATE_SESSION,
                &state_key,
                serde_json::json!({}),
            )
            .await?;
        Ok(())
    }

    /// Read all active sessions from the state room.
    ///
    /// Filters out entries with empty content (which signal removal).
    pub async fn read_sessions(
        &self,
        client: &MatrixClient,
    ) -> Result<Vec<StateRoomSession>> {
        let entries = client
            .get_all_state_events_of_type(&self.room_id, WORKER_STATE_SESSION)
            .await?;
        let mut sessions = Vec::new();
        for (_state_key, content) in entries {
            if is_empty_content(&content) {
                continue;
            }
            match serde_json::from_value::<StateRoomSession>(content) {
                Ok(session) => sessions.push(session),
                Err(e) => {
                    tracing::warn!(
                        state_key = %_state_key,
                        error = %e,
                        "skipping malformed session state event"
                    );
                }
            }
        }
        Ok(sessions)
    }

    // ── Room tracking CRUD ───────────────────────────────────────────────

    /// Write a room entry with state key being the tracked room's ID.
    pub async fn write_room(
        &self,
        client: &MatrixClient,
        room_id_key: &str,
        entry: &StateRoomEntry,
    ) -> Result<()> {
        let content = serde_json::to_value(entry)?;
        client
            .send_state_event(&self.room_id, WORKER_STATE_ROOM, room_id_key, content)
            .await?;
        Ok(())
    }

    /// Read all tracked rooms from the state room.
    pub async fn read_rooms(
        &self,
        client: &MatrixClient,
    ) -> Result<Vec<StateRoomEntry>> {
        let entries = client
            .get_all_state_events_of_type(&self.room_id, WORKER_STATE_ROOM)
            .await?;
        let mut rooms = Vec::new();
        for (_state_key, content) in entries {
            if is_empty_content(&content) {
                continue;
            }
            match serde_json::from_value::<StateRoomEntry>(content) {
                Ok(entry) => rooms.push(entry),
                Err(e) => {
                    tracing::warn!(
                        state_key = %_state_key,
                        error = %e,
                        "skipping malformed room state event"
                    );
                }
            }
        }
        Ok(rooms)
    }

    // ── Trust CRUD ───────────────────────────────────────────────────────

    /// Write a trusted client entry with state key `{user_id}`.
    pub async fn write_trusted_client(
        &self,
        client: &MatrixClient,
        user_id: &str,
        entity: &TrustedEntity,
    ) -> Result<()> {
        let content = serde_json::to_value(entity)?;
        client
            .send_state_event(
                &self.room_id,
                WORKER_STATE_TRUSTED_CLIENT,
                user_id,
                content,
            )
            .await?;
        Ok(())
    }

    /// Read all trusted clients from the state room.
    pub async fn read_trusted_clients(
        &self,
        client: &MatrixClient,
    ) -> Result<Vec<TrustedEntity>> {
        deserialize_all_of_type(client, &self.room_id, WORKER_STATE_TRUSTED_CLIENT).await
    }

    /// Write a trusted coordinator entry with state key `{user_id}`.
    pub async fn write_trusted_coordinator(
        &self,
        client: &MatrixClient,
        user_id: &str,
        entity: &TrustedEntity,
    ) -> Result<()> {
        let content = serde_json::to_value(entity)?;
        client
            .send_state_event(
                &self.room_id,
                WORKER_STATE_TRUSTED_COORDINATOR,
                user_id,
                content,
            )
            .await?;
        Ok(())
    }

    /// Read all trusted coordinators from the state room.
    pub async fn read_trusted_coordinators(
        &self,
        client: &MatrixClient,
    ) -> Result<Vec<TrustedEntity>> {
        deserialize_all_of_type(client, &self.room_id, WORKER_STATE_TRUSTED_COORDINATOR).await
    }

    // ── Topology ─────────────────────────────────────────────────────────

    /// Write the topology pointers (space, exec, status, logs room IDs).
    pub async fn write_topology(
        &self,
        client: &MatrixClient,
        topology: &StateRoomTopology,
    ) -> Result<()> {
        let content = serde_json::to_value(topology)?;
        client
            .send_state_event(&self.room_id, WORKER_STATE_TOPOLOGY, "", content)
            .await?;
        Ok(())
    }

    /// Read the topology pointers, or `None` if not yet set.
    pub async fn read_topology(
        &self,
        client: &MatrixClient,
    ) -> Result<Option<StateRoomTopology>> {
        let value = client
            .get_room_state_event(&self.room_id, WORKER_STATE_TOPOLOGY, "")
            .await?;
        deserialize_if_present(&value)
    }

    // ── Multi-homeserver write-confirm ────────────────────────────────────

    /// Write a state event to the primary client and replicate to secondaries.
    ///
    /// The primary write must succeed (returns error on failure). Secondary
    /// writes are best-effort: failures are logged but do not fail the call.
    pub async fn write_state_confirmed(
        &self,
        primary: &MatrixClient,
        secondaries: &[&MatrixClient],
        event_type: &str,
        state_key: &str,
        content: Value,
    ) -> Result<()> {
        // Primary write — must succeed
        primary
            .send_state_event(&self.room_id, event_type, state_key, content.clone())
            .await?;

        // Replicate to secondaries — best effort
        for (i, secondary) in secondaries.iter().enumerate() {
            if let Err(e) = secondary
                .send_state_event(&self.room_id, event_type, state_key, content.clone())
                .await
            {
                tracing::warn!(
                    secondary_index = i,
                    event_type = %event_type,
                    state_key = %state_key,
                    error = %e,
                    "secondary state write failed (non-fatal)"
                );
            }
        }

        Ok(())
    }

    // ── Coordinator discovery ────────────────────────────────────────────

    /// Advertise this worker's state room in the exec room.
    ///
    /// Writes a `org.mxdx.worker.state_room` state event to the exec room with
    /// state key `{device_id}` and content `{ "room_id": "<state_room_id>" }`.
    pub async fn advertise_in_exec_room(
        &self,
        client: &MatrixClient,
        exec_room_id: &RoomId,
        device_id: &str,
    ) -> Result<()> {
        let content = serde_json::json!({
            "room_id": self.room_id.as_str(),
        });
        client
            .send_state_event(exec_room_id, WORKER_STATE_ROOM_POINTER, device_id, content)
            .await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Format a session state key: `{device_id}/{uuid}`.
fn format_session_key(device_id: &str, uuid: &str) -> String {
    format!("{device_id}/{uuid}")
}

/// Check if a state event content is empty (signals removal).
fn is_empty_content(value: &Value) -> bool {
    match value {
        Value::Object(map) => map.is_empty(),
        Value::Null => true,
        _ => false,
    }
}

/// Attempt to deserialize a state event value, returning `None` for
/// empty/missing content or error responses (Matrix returns `{"errcode":...}`
/// for missing state events).
fn deserialize_if_present<T: serde::de::DeserializeOwned>(value: &Value) -> Result<Option<T>> {
    // Matrix returns {"errcode": "M_NOT_FOUND", ...} for missing state events
    if value.get("errcode").is_some() {
        return Ok(None);
    }
    if is_empty_content(value) {
        return Ok(None);
    }
    let parsed = serde_json::from_value(value.clone())?;
    Ok(Some(parsed))
}

/// Fetch and deserialize all state events of a given type, filtering out
/// empty content (removed entries) and logging deserialization failures.
async fn deserialize_all_of_type<T: serde::de::DeserializeOwned>(
    client: &MatrixClient,
    room_id: &RoomId,
    event_type: &str,
) -> Result<Vec<T>> {
    let entries = client
        .get_all_state_events_of_type(room_id, event_type)
        .await?;
    let mut results = Vec::new();
    for (state_key, content) in entries {
        if is_empty_content(&content) {
            continue;
        }
        match serde_json::from_value::<T>(content) {
            Ok(item) => results.push(item),
            Err(e) => {
                tracing::warn!(
                    event_type = %event_type,
                    state_key = %state_key,
                    error = %e,
                    "skipping malformed state event"
                );
            }
        }
    }
    Ok(results)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── Pure function tests ──────────────────────────────────────────────

    #[test]
    fn format_session_key_combines_device_and_uuid() {
        assert_eq!(
            format_session_key("DEVICE123", "550e8400-e29b-41d4-a716-446655440000"),
            "DEVICE123/550e8400-e29b-41d4-a716-446655440000"
        );
    }

    #[test]
    fn format_session_key_handles_special_chars() {
        assert_eq!(
            format_session_key("dev/ice", "uuid-with-hyphens"),
            "dev/ice/uuid-with-hyphens"
        );
    }

    #[test]
    fn is_empty_content_true_for_empty_object() {
        assert!(is_empty_content(&json!({})));
    }

    #[test]
    fn is_empty_content_true_for_null() {
        assert!(is_empty_content(&Value::Null));
    }

    #[test]
    fn is_empty_content_false_for_non_empty_object() {
        assert!(!is_empty_content(&json!({"key": "value"})));
    }

    #[test]
    fn is_empty_content_false_for_array() {
        assert!(!is_empty_content(&json!([])));
    }

    #[test]
    fn is_empty_content_false_for_string() {
        assert!(!is_empty_content(&json!("hello")));
    }

    // ── deserialize_if_present tests ─────────────────────────────────────

    #[test]
    fn deserialize_if_present_returns_none_for_errcode() {
        let value = json!({"errcode": "M_NOT_FOUND", "error": "not found"});
        let result: Result<Option<WorkerStateConfig>> = deserialize_if_present(&value);
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn deserialize_if_present_returns_none_for_empty_object() {
        let result: Result<Option<WorkerStateConfig>> = deserialize_if_present(&json!({}));
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn deserialize_if_present_returns_none_for_null() {
        let result: Result<Option<WorkerStateConfig>> = deserialize_if_present(&Value::Null);
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn deserialize_if_present_parses_valid_config() {
        let value = json!({
            "room_name": "mxdx-state-node01.deploy.worker",
            "trust_anchor": null,
            "capabilities": ["linux"],
            "created_at": 1742572800
        });
        let result: Result<Option<WorkerStateConfig>> = deserialize_if_present(&value);
        let config = result.unwrap().unwrap();
        assert_eq!(config.room_name, "mxdx-state-node01.deploy.worker");
        assert_eq!(config.capabilities, vec!["linux"]);
    }

    #[test]
    fn deserialize_if_present_returns_error_for_malformed() {
        // Missing required fields
        let value = json!({"room_name": "test"});
        let result: Result<Option<WorkerStateConfig>> = deserialize_if_present(&value);
        assert!(result.is_err());
    }

    // ── StateRoomSession deserialization ──────────────────────────────────

    #[test]
    fn session_deserialization_filters_empty() {
        let content = json!({});
        assert!(is_empty_content(&content));
    }

    #[test]
    fn session_state_key_format() {
        let key = format_session_key("ABCDEF", "test-uuid-123");
        assert_eq!(key, "ABCDEF/test-uuid-123");
    }

    // ── StateRoomEntry tests ─────────────────────────────────────────────

    #[test]
    fn room_entry_roundtrip() {
        let entry = StateRoomEntry {
            room_id: "!abc:example.com".into(),
            room_name: Some("exec room".into()),
            space_id: Some("!space:example.com".into()),
            role: "exec".into(),
            joined_at: 1742572800,
        };
        let value = serde_json::to_value(&entry).unwrap();
        let back: StateRoomEntry = serde_json::from_value(value).unwrap();
        assert_eq!(entry, back);
    }

    // ── TrustedEntity tests ──────────────────────────────────────────────

    #[test]
    fn trusted_entity_roundtrip() {
        let entity = TrustedEntity {
            user_id: "@admin:example.com".into(),
            verified_at: 1742572800,
            verified_by_device: "DEVICEABC".into(),
        };
        let value = serde_json::to_value(&entity).unwrap();
        let back: TrustedEntity = serde_json::from_value(value).unwrap();
        assert_eq!(entity, back);
    }

    // ── Topology tests ───────────────────────────────────────────────────

    #[test]
    fn topology_roundtrip() {
        let topology = StateRoomTopology {
            space_id: "!space:example.com".into(),
            exec_room_id: "!exec:example.com".into(),
            status_room_id: "!status:example.com".into(),
            logs_room_id: "!logs:example.com".into(),
        };
        let value = serde_json::to_value(&topology).unwrap();
        let back: StateRoomTopology = serde_json::from_value(value).unwrap();
        assert_eq!(topology, back);
    }

    // ── WorkerStateRoom struct tests ─────────────────────────────────────

    #[test]
    fn worker_state_room_room_id_accessor() {
        let room_id: OwnedRoomId = "!test:example.com".try_into().unwrap();
        let state_room = WorkerStateRoom {
            room_id: room_id.clone(),
        };
        assert_eq!(state_room.room_id().as_str(), room_id.as_str());
    }

    #[test]
    fn worker_state_room_advertise_content_format() {
        // Verify the JSON content format for exec room advertisement
        let state_room_id = "!state:example.com";
        let content = serde_json::json!({
            "room_id": state_room_id,
        });
        assert_eq!(content["room_id"], state_room_id);
    }

    // ── Event type constant coverage ─────────────────────────────────────

    #[test]
    fn event_type_constants_are_correct() {
        assert_eq!(WORKER_STATE_CONFIG, "org.mxdx.worker.config");
        assert_eq!(WORKER_STATE_IDENTITY, "org.mxdx.worker.identity");
        assert_eq!(WORKER_STATE_SESSION, "org.mxdx.worker.session");
        assert_eq!(WORKER_STATE_ROOM, "org.mxdx.worker.room");
        assert_eq!(WORKER_STATE_TOPOLOGY, "org.mxdx.worker.topology");
        assert_eq!(WORKER_STATE_ROOM_POINTER, "org.mxdx.worker.state_room");
        assert_eq!(WORKER_STATE_TRUSTED_CLIENT, "org.mxdx.worker.trusted_client");
        assert_eq!(
            WORKER_STATE_TRUSTED_COORDINATOR,
            "org.mxdx.worker.trusted_coordinator"
        );
    }
}
