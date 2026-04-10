use serde_json::Value;

use crate::error::{MatrixClientError, Result};
use crate::MatrixClient;
use matrix_sdk::ruma::{
    api::client::room::create_room::v3::{CreationContent, Request as CreateRoomRequest},
    events::{
        room::{
            encryption::RoomEncryptionEventContent,
            history_visibility::{HistoryVisibility, RoomHistoryVisibilityEventContent},
            topic::RoomTopicEventContent,
        },
        EmptyStateKey, InitialStateEvent,
    },
    serde::Raw,
    OwnedRoomId, OwnedUserId, RoomId, UserId,
};

/// Custom field key inside `m.room.create` content identifying the launcher
/// this room belongs to. `m.room.create` is NEVER encrypted per Matrix spec
/// (it's the foundational state event that establishes the room), so this
/// field is the one place we can put discovery metadata that survives
/// MSC4362 encrypted state events.
pub const MXDX_LAUNCHER_ID_KEY: &str = "org.mxdx.launcher_id";

/// Custom field key inside `m.room.create` content identifying the role of
/// this room within the launcher topology (`space`, `exec`, or `logs`).
pub const MXDX_ROLE_KEY: &str = "org.mxdx.role";

/// Build a `Raw<CreationContent>` with mxdx discovery fields embedded in the
/// room's foundational `m.room.create` event. Because `m.room.create` is
/// never wrapped by MSC4362 encryption, these fields can be read via plain
/// REST (`/rooms/{id}/state/m.room.create/`) and are unaffected by key
/// exchange or crypto-store state.
fn mxdx_creation_content_raw(
    launcher_id: &str,
    role: &str,
    is_space: bool,
) -> Raw<CreationContent> {
    let mut obj = serde_json::Map::new();
    if is_space {
        obj.insert(
            "type".to_string(),
            Value::String("m.space".to_string()),
        );
    }
    obj.insert(
        MXDX_LAUNCHER_ID_KEY.to_string(),
        Value::String(launcher_id.to_string()),
    );
    obj.insert(
        MXDX_ROLE_KEY.to_string(),
        Value::String(role.to_string()),
    );
    let json_string = serde_json::to_string(&Value::Object(obj))
        .expect("mxdx creation content serializes");
    Raw::from_json_string(json_string)
        .expect("mxdx creation content is valid JSON")
}

/// Room IDs for a launcher space and its child rooms.
/// Topology: space (container) + exec (encrypted, all client interaction) + logs (worker operational logs).
/// There is no status room — worker telemetry goes to the exec room.
#[derive(Debug, Clone)]
pub struct LauncherTopology {
    pub space_id: OwnedRoomId,
    pub exec_room_id: OwnedRoomId,
    pub logs_room_id: OwnedRoomId,
}

impl MatrixClient {
    /// Create a launcher space with exec and logs child rooms.
    /// The space is a Matrix Space (m.space), exec is encrypted, logs is unencrypted.
    /// All rooms are named and tagged with topics for discoverability.
    /// Worker telemetry goes to the exec room (no separate status room).
    pub async fn create_launcher_space(&self, launcher_id: &str) -> Result<LauncherTopology> {
        let server_name = self.user_id().server_name().to_string();

        // Space room. The launcher_id + role marker go in `m.room.create`
        // (never encrypted). Everything else (topic, name, child links) stays
        // encrypted via MSC4362.
        let space_topic = InitialStateEvent::new(
            EmptyStateKey,
            RoomTopicEventContent::new(format!("org.mxdx.launcher.space:{launcher_id}")),
        );
        let space_encryption = InitialStateEvent::new(
            EmptyStateKey,
            RoomEncryptionEventContent::with_recommended_defaults().with_encrypted_state(),
        );
        let mut space_request = CreateRoomRequest::new();
        space_request.name = Some(format!("mxdx: {launcher_id}"));
        space_request.creation_content =
            Some(mxdx_creation_content_raw(launcher_id, "space", true));
        space_request.initial_state = vec![space_topic.to_raw_any(), space_encryption.to_raw_any()];

        let space_response = self.create_room_with_timeout(space_request).await?;
        let space_id = space_response.room_id().to_owned();

        // Create child rooms (with optional delay for rate-limited servers)
        let delay = self.room_creation_delay();

        let exec_room_id = self
            .create_mxdx_encrypted_room(
                &format!("mxdx: {launcher_id} — exec"),
                &format!("org.mxdx.launcher.exec:{launcher_id}"),
                &[],
                launcher_id,
                "exec",
            )
            .await?;
        if let Some(d) = delay {
            tokio::time::sleep(d).await;
        }
        let logs_room_id = self
            .create_mxdx_encrypted_room(
                &format!("mxdx: {launcher_id} — logs"),
                &format!("org.mxdx.launcher.logs:{launcher_id}"),
                &[],
                launcher_id,
                "logs",
            )
            .await?;

        // Link child rooms to space via m.space.child state events
        let via = serde_json::json!({ "via": [server_name] });
        for child_id in [&exec_room_id, &logs_room_id] {
            self.send_state_event(&space_id, "m.space.child", child_id.as_str(), via.clone())
                .await?;
        }

        Ok(LauncherTopology {
            space_id,
            exec_room_id,
            logs_room_id,
        })
    }

    /// Find an existing launcher space for the given launcher_id.
    ///
    /// This used to scan `room.topic()` over the SDK's cached state, but
    /// MSC4362 encrypts `m.room.topic`, and a freshly-joined client may
    /// not yet have the megolm key needed to decrypt it (keys arrive via
    /// to-device messages on the next encrypted send, not on join). So the
    /// SDK-cached topic can legitimately be `None` for every candidate
    /// room, breaking discovery.
    ///
    /// Instead, dispatch to the REST-based discovery path which keys on
    /// `m.room.create.content.org.mxdx.launcher_id`. `m.room.create` is
    /// never encrypted per Matrix spec, so the fields are always readable
    /// regardless of crypto state. Both the worker and the client use the
    /// same discovery mechanism now.
    pub async fn find_launcher_space(&self, launcher_id: &str) -> Result<Option<LauncherTopology>> {
        // NOTE: No sync_once here — callers (connect_multi, connect_with_keychain)
        // already sync during connection setup. The REST-based discovery below
        // queries the server directly and handles invites via REST.

        let homeserver = self.inner().homeserver().to_string();
        let access_token = self
            .access_token()
            .ok_or_else(|| MatrixClientError::Other(anyhow::anyhow!(
                "find_launcher_space: client has no access token (not logged in)"
            )))?;
        self.find_launcher_space_via_rest(launcher_id, &homeserver, &access_token)
            .await
    }

    /// Find the launcher topology by querying the Matrix REST API directly,
    /// bypassing the SDK's local cache. Matches rooms by the custom fields
    /// (`org.mxdx.launcher_id`, `org.mxdx.role`) embedded in each room's
    /// `m.room.create` content. That event is never encrypted per Matrix
    /// spec, so this works regardless of MSC4362 encrypted-state status and
    /// regardless of whether the caller has any room keys.
    ///
    /// Follows `m.room.tombstone` chains to the latest replacement before
    /// matching, so self-healed topologies resolve to their current rooms.
    pub async fn find_launcher_space_via_rest(
        &self,
        launcher_id: &str,
        homeserver: &str,
        access_token: &str,
    ) -> Result<Option<LauncherTopology>> {
        use crate::rest::RestClient;
        let rest = RestClient::new(homeserver, access_token);

        // Auto-accept any pending invites first (worker may have been re-invited
        // after a previous set of rooms got self-healed/tombstoned).
        for invited in rest.list_invited_rooms().await.unwrap_or_default() {
            if let Err(e) = self.join_room(&invited).await {
                tracing::debug!(room_id=%invited, error=%e, "could not auto-join invited room");
            }
        }

        let joined = rest.list_joined_rooms().await?;
        let mut space: Option<OwnedRoomId> = None;
        let mut exec: Option<OwnedRoomId> = None;
        let mut logs: Option<OwnedRoomId> = None;

        for rid in joined {
            // Follow tombstone chain to the live replacement room.
            let mut current = rid.clone();
            for _ in 0..10 {
                match rest.get_room_tombstone(&current).await {
                    Ok(Some(replacement)) => {
                        tracing::debug!(old=%current, new=%replacement, "following tombstone");
                        current = replacement;
                    }
                    _ => break,
                }
            }

            // Read discovery metadata from m.room.create (never encrypted).
            let create = match rest.get_room_create(&current).await {
                Ok(Some(v)) => v,
                _ => continue,
            };
            let lid = create
                .get(MXDX_LAUNCHER_ID_KEY)
                .and_then(|v| v.as_str());
            if lid != Some(launcher_id) {
                continue;
            }
            let role = create.get(MXDX_ROLE_KEY).and_then(|v| v.as_str());
            match role {
                Some("space") if space.is_none() => space = Some(current),
                Some("exec") if exec.is_none() => exec = Some(current),
                Some("logs") if logs.is_none() => logs = Some(current),
                _ => {}
            }
        }

        match (space, exec, logs) {
            (Some(s), Some(e), Some(l)) => Ok(Some(LauncherTopology {
                space_id: s,
                exec_room_id: e,
                logs_room_id: l,
            })),
            (_, Some(e), _) => {
                tracing::warn!(launcher_id=%launcher_id, "partial topology — only exec room found");
                Ok(Some(LauncherTopology {
                    space_id: e.clone(),
                    exec_room_id: e.clone(),
                    logs_room_id: e,
                }))
            }
            _ => Ok(None),
        }
    }

    /// Find an existing launcher space or create a new one.
    /// NOTE: Only workers/launchers should call this. Clients must use find_launcher_space()
    /// and fail if no worker room is found — clients must never create rooms.
    pub async fn get_or_create_launcher_space(
        &self,
        launcher_id: &str,
    ) -> Result<LauncherTopology> {
        if let Some(topology) = self.find_launcher_space(launcher_id).await? {
            return Ok(topology);
        }
        self.create_launcher_space(launcher_id).await
    }

    /// Create an encrypted DM room with history_visibility set to "joined" (mxdx-aew).
    pub async fn create_terminal_session_dm(&self, user_id: &UserId) -> Result<OwnedRoomId> {
        let encryption_event = InitialStateEvent::new(
            EmptyStateKey,
            RoomEncryptionEventContent::with_recommended_defaults().with_encrypted_state(),
        );

        let history_event = InitialStateEvent::new(
            EmptyStateKey,
            RoomHistoryVisibilityEventContent::new(HistoryVisibility::Joined),
        );

        let topic_event = InitialStateEvent::new(
            EmptyStateKey,
            RoomTopicEventContent::new(format!(
                "org.mxdx.terminal.session:{}",
                user_id.localpart()
            )),
        );

        let mut request = CreateRoomRequest::new();
        request.name = Some(format!("mxdx: terminal — {}", user_id.localpart()));
        request.invite = vec![user_id.to_owned()];
        request.is_direct = true;
        request.initial_state = vec![
            encryption_event.to_raw_any(),
            history_event.to_raw_any(),
            topic_event.to_raw_any(),
        ];

        let response = self.create_room_with_timeout(request).await?;
        Ok(response.room_id().to_owned())
    }

    /// Send an m.room.tombstone state event to mark a room as replaced.
    pub async fn tombstone_room(
        &self,
        room_id: &RoomId,
        replacement_room_id: &RoomId,
    ) -> Result<()> {
        let content = serde_json::json!({
            "body": "This room has been replaced",
            "replacement_room": replacement_room_id.to_string(),
        });
        self.send_state_event(room_id, "m.room.tombstone", "", content)
            .await
    }

    /// Fetch a specific state event from a room via the REST API, with a state key.
    pub async fn get_room_state_event(
        &self,
        room_id: &RoomId,
        event_type: &str,
        state_key: &str,
    ) -> Result<Value> {
        let homeserver = self.inner().homeserver();
        let access_token = self
            .inner()
            .access_token()
            .expect("Client is not logged in \u{2014} no access_token");

        let encoded_state_key = percent_encode_path_segment(state_key);
        let url = format!(
            "{}_matrix/client/v3/rooms/{}/state/{}/{}",
            homeserver, room_id, event_type, encoded_state_key,
        );

        let http_client = reqwest::Client::new();
        let resp = http_client
            .get(&url)
            .bearer_auth(&access_token)
            .send()
            .await
            .map_err(|e| crate::error::MatrixClientError::Other(e.into()))?;

        let body: Value = resp
            .json()
            .await
            .map_err(|e| crate::error::MatrixClientError::Other(e.into()))?;

        Ok(body)
    }

    /// Fetch a specific state event from a room via the REST API.
    pub async fn get_room_state(&self, room_id: &RoomId, event_type: &str) -> Result<Value> {
        let homeserver = self.inner().homeserver();
        let access_token = self
            .inner()
            .access_token()
            .expect("Client is not logged in — no access_token");

        let url = format!(
            "{}_matrix/client/v3/rooms/{}/state/{}",
            homeserver, room_id, event_type,
        );

        let http_client = reqwest::Client::new();
        let resp = http_client
            .get(&url)
            .bearer_auth(&access_token)
            .send()
            .await
            .map_err(|e| crate::error::MatrixClientError::Other(e.into()))?;

        let body: Value = resp
            .json()
            .await
            .map_err(|e| crate::error::MatrixClientError::Other(e.into()))?;

        Ok(body)
    }

    /// Create a named encrypted room with a topic for discoverability.
    pub async fn create_named_encrypted_room(
        &self,
        name: &str,
        topic: &str,
        invite: &[matrix_sdk::ruma::OwnedUserId],
    ) -> Result<OwnedRoomId> {
        let encryption_event = InitialStateEvent::new(
            EmptyStateKey,
            RoomEncryptionEventContent::with_recommended_defaults().with_encrypted_state(),
        );
        let topic_event =
            InitialStateEvent::new(EmptyStateKey, RoomTopicEventContent::new(topic.to_string()));

        let mut request = CreateRoomRequest::new();
        request.name = Some(name.to_string());
        request.invite = invite.to_vec();
        request.initial_state = vec![encryption_event.to_raw_any(), topic_event.to_raw_any()];

        let response = self.create_room_with_timeout(request).await?;
        Ok(response.room_id().to_owned())
    }

    /// Create a named encrypted room that also carries the mxdx launcher_id
    /// and role in its `m.room.create` content. These fields remain readable
    /// via plain REST even when every other state event is encrypted via
    /// MSC4362, making them the reliable discovery mechanism for launcher
    /// topologies. `role` must be one of `"exec"`, `"logs"`, or `"space"`.
    pub async fn create_mxdx_encrypted_room(
        &self,
        name: &str,
        topic: &str,
        invite: &[matrix_sdk::ruma::OwnedUserId],
        launcher_id: &str,
        role: &str,
    ) -> Result<OwnedRoomId> {
        let encryption_event = InitialStateEvent::new(
            EmptyStateKey,
            RoomEncryptionEventContent::with_recommended_defaults().with_encrypted_state(),
        );
        let topic_event =
            InitialStateEvent::new(EmptyStateKey, RoomTopicEventContent::new(topic.to_string()));

        let mut request = CreateRoomRequest::new();
        request.name = Some(name.to_string());
        request.invite = invite.to_vec();
        request.initial_state = vec![encryption_event.to_raw_any(), topic_event.to_raw_any()];
        request.creation_content =
            Some(mxdx_creation_content_raw(launcher_id, role, false));

        let response = self.create_room_with_timeout(request).await?;
        Ok(response.room_id().to_owned())
    }

    /// Create a named unencrypted room with a topic for discoverability.
    pub async fn create_named_unencrypted_room(
        &self,
        name: &str,
        topic: &str,
    ) -> Result<OwnedRoomId> {
        let topic_event =
            InitialStateEvent::new(EmptyStateKey, RoomTopicEventContent::new(topic.to_string()));

        let mut request = CreateRoomRequest::new();
        request.name = Some(name.to_string());
        request.initial_state = vec![topic_event.to_raw_any()];

        let response = self.create_room_with_timeout(request).await?;
        Ok(response.room_id().to_owned())
    }

    // ── Worker state room operations ──────────────────────────────────

    /// Create an encrypted worker state room with a deterministic alias and topic.
    ///
    /// Room alias: `#mxdx-state-{hostname}.{os_user}.{localpart}:{server}`
    /// Topic: `org.mxdx.worker.state:{hostname}.{os_user}.{localpart}`
    /// The room is E2EE-enabled with `HistoryVisibility::Joined`.
    pub async fn create_worker_state_room(
        &self,
        hostname: &str,
        os_user: &str,
        localpart: &str,
    ) -> Result<OwnedRoomId> {
        let alias_localpart = format!("mxdx-state-{hostname}.{os_user}.{localpart}");
        let topic = format!("org.mxdx.worker.state:{hostname}.{os_user}.{localpart}");

        let encryption_event = InitialStateEvent::new(
            EmptyStateKey,
            RoomEncryptionEventContent::with_recommended_defaults().with_encrypted_state(),
        );
        let history_event = InitialStateEvent::new(
            EmptyStateKey,
            RoomHistoryVisibilityEventContent::new(HistoryVisibility::Joined),
        );
        let topic_event =
            InitialStateEvent::new(EmptyStateKey, RoomTopicEventContent::new(topic));

        let mut request = CreateRoomRequest::new();
        request.name = Some(format!(
            "mxdx: state — {hostname}.{os_user}.{localpart}"
        ));
        request.room_alias_name = Some(alias_localpart);
        request.initial_state = vec![
            encryption_event.to_raw_any(),
            history_event.to_raw_any(),
            topic_event.to_raw_any(),
        ];

        let response = self.create_room_with_timeout(request).await?;
        Ok(response.room_id().to_owned())
    }

    /// Find an existing worker state room by alias lookup, falling back to topic scan.
    ///
    /// Tries the canonical alias first via `GET /_matrix/client/v3/directory/room/{alias}`,
    /// then falls back to scanning joined rooms by topic.
    pub async fn find_worker_state_room(
        &self,
        hostname: &str,
        os_user: &str,
        localpart: &str,
    ) -> Result<Option<OwnedRoomId>> {
        let server_name = self.user_id().server_name().to_string();
        let alias = format!(
            "#mxdx-state-{hostname}.{os_user}.{localpart}:{server_name}"
        );

        // Try alias resolution first
        if let Some(room_id) = self.resolve_room_alias(&alias).await? {
            return Ok(Some(room_id));
        }

        // Fall back to topic scan
        let expected_topic =
            format!("org.mxdx.worker.state:{hostname}.{os_user}.{localpart}");
        self.sync_once().await?;
        for room in self.inner().joined_rooms() {
            if room.topic().unwrap_or_default() == expected_topic {
                return Ok(Some(room.room_id().to_owned()));
            }
        }

        Ok(None)
    }

    /// Validate a state room: check creator and encryption.
    ///
    /// - Fetches all room state and finds the `m.room.create` event envelope.
    ///   Checks `content.creator` first (room versions < 11), falls back to
    ///   `sender` (room version 11+, where `creator` is deprecated).
    /// - Verifies the creator/sender is either the own account or a trusted coordinator.
    /// - Checks that `m.room.encryption` state event exists (room must be E2EE).
    /// - Returns `Err(StateRoomRejected)` if either check fails.
    pub async fn validate_state_room(
        &self,
        room_id: &RoomId,
        own_user_id: &UserId,
        trusted_coordinators: &[OwnedUserId],
    ) -> Result<()> {
        // Fetch all state events (full envelopes with sender, type, content, state_key)
        let all_state = self.get_all_room_state(room_id).await?;

        // Find m.room.create event
        let create_event = all_state
            .iter()
            .find(|e| e.get("type").and_then(|v| v.as_str()) == Some("m.room.create"))
            .ok_or_else(|| {
                MatrixClientError::StateRoomRejected(
                    "m.room.create state event not found in room".into(),
                )
            })?;

        // Room version 11+ deprecates content.creator in favour of the event sender.
        // Try content.creator first, fall back to event.sender.
        let creator = extract_room_creator(create_event);

        let is_own = creator == own_user_id.as_str();
        let is_trusted_coordinator = trusted_coordinators
            .iter()
            .any(|c| c.as_str() == creator);

        if !is_own && !is_trusted_coordinator {
            return Err(MatrixClientError::StateRoomRejected(format!(
                "Room creator '{creator}' is neither own account nor a trusted coordinator"
            )));
        }

        // Check m.room.encryption exists
        let has_encryption = all_state
            .iter()
            .any(|e| e.get("type").and_then(|v| v.as_str()) == Some("m.room.encryption"));

        if !has_encryption {
            return Err(MatrixClientError::StateRoomRejected(
                "Room is not encrypted — m.room.encryption state event missing".into(),
            ));
        }

        Ok(())
    }

    /// Resolve a room alias to a room ID via the REST API.
    ///
    /// Returns `None` if the alias does not exist (404), propagates other errors.
    async fn resolve_room_alias(&self, alias: &str) -> Result<Option<OwnedRoomId>> {
        let homeserver = self.inner().homeserver();
        let access_token = self
            .inner()
            .access_token()
            .expect("Client is not logged in — no access_token");

        let encoded_alias = percent_encode_path_segment(alias);

        let url = format!(
            "{}_matrix/client/v3/directory/room/{}",
            homeserver, encoded_alias,
        );

        let http_client = reqwest::Client::new();
        let resp = http_client
            .get(&url)
            .bearer_auth(&access_token)
            .send()
            .await
            .map_err(|e| MatrixClientError::Other(e.into()))?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(MatrixClientError::Other(anyhow::anyhow!(
                "Room alias resolution failed (HTTP {status}): {body}"
            )));
        }

        let body: Value = resp
            .json()
            .await
            .map_err(|e| MatrixClientError::Other(e.into()))?;

        if let Some(room_id_str) = body.get("room_id").and_then(|v| v.as_str()) {
            let room_id: OwnedRoomId = room_id_str
                .try_into()
                .map_err(|e: matrix_sdk::IdParseError| MatrixClientError::Other(e.into()))?;
            Ok(Some(room_id))
        } else {
            Ok(None)
        }
    }

    /// Delete a room alias via the REST API.
    ///
    /// Used to reclaim an alias bound to a room we can no longer join (e.g.
    /// after the account was removed from a private room). Returns `Ok(())` on
    /// success or 404 (alias already gone). Propagates other errors.
    pub async fn delete_room_alias(&self, alias: &str) -> Result<()> {
        let homeserver = self.inner().homeserver();
        let access_token = self
            .inner()
            .access_token()
            .expect("Client is not logged in — no access_token");

        let encoded_alias = percent_encode_path_segment(alias);
        let url = format!(
            "{}_matrix/client/v3/directory/room/{}",
            homeserver, encoded_alias,
        );

        let http_client = reqwest::Client::new();
        let resp = http_client
            .delete(&url)
            .bearer_auth(&access_token)
            .send()
            .await
            .map_err(|e| MatrixClientError::Other(e.into()))?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND || resp.status().is_success() {
            Ok(())
        } else {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            Err(MatrixClientError::Other(anyhow::anyhow!(
                "Room alias deletion failed (HTTP {status}): {body}"
            )))
        }
    }

    /// Fetch all state events from a room as full event envelopes.
    ///
    /// Uses `GET /_matrix/client/v3/rooms/{roomId}/state` which returns an array
    /// of full event objects (with `type`, `state_key`, `sender`, `content`, etc.).
    async fn get_all_room_state(&self, room_id: &RoomId) -> Result<Vec<Value>> {
        let homeserver = self.inner().homeserver();
        let access_token = self
            .inner()
            .access_token()
            .expect("Client is not logged in — no access_token");

        let url = format!(
            "{}_matrix/client/v3/rooms/{}/state",
            homeserver, room_id,
        );

        let http_client = reqwest::Client::new();
        let resp = http_client
            .get(&url)
            .bearer_auth(&access_token)
            .send()
            .await
            .map_err(|e| MatrixClientError::Other(e.into()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(MatrixClientError::Other(anyhow::anyhow!(
                "Failed to fetch room state for {room_id} (HTTP {status}): {body}"
            )));
        }

        let body: Value = resp
            .json()
            .await
            .map_err(|e| MatrixClientError::Other(e.into()))?;

        Ok(body.as_array().cloned().unwrap_or_default())
    }

    /// Fetch all state events of a given type from a room.
    ///
    /// Uses `GET /_matrix/client/v3/rooms/{roomId}/state` to get ALL state,
    /// then filters by the specified event type.
    /// Returns a Vec of (state_key, content) pairs.
    pub async fn get_all_state_events_of_type(
        &self,
        room_id: &RoomId,
        event_type: &str,
    ) -> Result<Vec<(String, Value)>> {
        let events = self.get_all_room_state(room_id).await?;
        Ok(filter_state_events_by_type(&events, event_type))
    }
}

/// Percent-encode a string for use as a URL path segment.
///
/// Encodes all characters except unreserved characters (RFC 3986):
/// ALPHA, DIGIT, '-', '_', '.', '~'.
fn percent_encode_path_segment(input: &str) -> String {
    input
        .bytes()
        .flat_map(|b| {
            if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.' || b == b'~' {
                vec![b as char]
            } else {
                format!("%{:02X}", b).chars().collect()
            }
        })
        .collect()
}

/// Extract the room creator from an `m.room.create` event envelope.
///
/// In room versions < 11, the creator is in `content.creator`.
/// In room version 11+, `content.creator` is deprecated and the event `sender`
/// is the authoritative creator.
/// Returns the creator user ID string, or empty string if neither is present.
fn extract_room_creator(create_event: &Value) -> &str {
    create_event
        .get("content")
        .and_then(|c| c.get("creator"))
        .and_then(|v| v.as_str())
        .or_else(|| create_event.get("sender").and_then(|v| v.as_str()))
        .unwrap_or("")
}

/// Filter a list of state events (as JSON values) by event type.
/// Returns (state_key, content) pairs for matching events.
/// Extracted as a pure function for testability.
fn filter_state_events_by_type(events: &[Value], event_type: &str) -> Vec<(String, Value)> {
    let mut results = Vec::new();
    for event in events {
        let etype = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if etype == event_type {
            let state_key = event
                .get("state_key")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let content = event
                .get("content")
                .cloned()
                .unwrap_or(Value::Object(serde_json::Map::new()));
            results.push((state_key, content));
        }
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn filter_state_events_matches_correct_type() {
        let events = vec![
            json!({
                "type": "org.mxdx.worker.session",
                "state_key": "sess-001",
                "content": {"uuid": "sess-001", "state": "running"}
            }),
            json!({
                "type": "m.room.encryption",
                "state_key": "",
                "content": {"algorithm": "m.megolm.v1.aes-sha2"}
            }),
            json!({
                "type": "org.mxdx.worker.session",
                "state_key": "sess-002",
                "content": {"uuid": "sess-002", "state": "completed"}
            }),
            json!({
                "type": "org.mxdx.worker.room",
                "state_key": "!abc:example.com",
                "content": {"room_id": "!abc:example.com", "role": "exec"}
            }),
        ];

        let results = filter_state_events_by_type(&events, "org.mxdx.worker.session");
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "sess-001");
        assert_eq!(results[0].1["uuid"], "sess-001");
        assert_eq!(results[1].0, "sess-002");
        assert_eq!(results[1].1["state"], "completed");
    }

    #[test]
    fn filter_state_events_returns_empty_for_no_matches() {
        let events = vec![
            json!({
                "type": "m.room.member",
                "state_key": "@user:example.com",
                "content": {"membership": "join"}
            }),
        ];
        let results = filter_state_events_by_type(&events, "org.mxdx.worker.session");
        assert!(results.is_empty());
    }

    #[test]
    fn filter_state_events_handles_empty_input() {
        let results = filter_state_events_by_type(&[], "org.mxdx.worker.session");
        assert!(results.is_empty());
    }

    #[test]
    fn filter_state_events_missing_state_key_defaults_to_empty() {
        let events = vec![json!({
            "type": "org.mxdx.worker.config",
            "content": {"room_name": "test"}
        })];
        let results = filter_state_events_by_type(&events, "org.mxdx.worker.config");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "");
    }

    #[test]
    fn filter_state_events_missing_content_defaults_to_empty_object() {
        let events = vec![json!({
            "type": "org.mxdx.worker.config",
            "state_key": ""
        })];
        let results = filter_state_events_by_type(&events, "org.mxdx.worker.config");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].1, json!({}));
    }

    // ── percent_encode_path_segment tests ─────────────────────────────

    #[test]
    fn percent_encode_unreserved_chars_unchanged() {
        assert_eq!(
            percent_encode_path_segment("abc-DEF_012.~"),
            "abc-DEF_012.~"
        );
    }

    #[test]
    fn percent_encode_room_alias() {
        // #mxdx-state-host.user.local:example.com
        // '#' -> %23, ':' -> %3A
        assert_eq!(
            percent_encode_path_segment("#mxdx-state-host.user.local:example.com"),
            "%23mxdx-state-host.user.local%3Aexample.com"
        );
    }

    #[test]
    fn percent_encode_state_key_with_special_chars() {
        // @user:example.com -> %40user%3Aexample.com
        assert_eq!(
            percent_encode_path_segment("@user:example.com"),
            "%40user%3Aexample.com"
        );
    }

    #[test]
    fn percent_encode_empty_string() {
        assert_eq!(percent_encode_path_segment(""), "");
    }

    // ── extract_room_creator tests ────────────────────────────────────

    #[test]
    fn extract_creator_from_content_creator_field() {
        // Room version < 11: content.creator is authoritative
        let event = json!({
            "type": "m.room.create",
            "sender": "@someone:example.com",
            "content": {
                "creator": "@owner:example.com",
                "room_version": "10"
            }
        });
        assert_eq!(extract_room_creator(&event), "@owner:example.com");
    }

    #[test]
    fn extract_creator_falls_back_to_sender_when_no_content_creator() {
        // Room version 11+: content.creator is absent, sender is authoritative
        let event = json!({
            "type": "m.room.create",
            "sender": "@owner:example.com",
            "content": {
                "room_version": "11"
            }
        });
        assert_eq!(extract_room_creator(&event), "@owner:example.com");
    }

    #[test]
    fn extract_creator_falls_back_to_sender_when_content_missing() {
        let event = json!({
            "type": "m.room.create",
            "sender": "@owner:example.com"
        });
        assert_eq!(extract_room_creator(&event), "@owner:example.com");
    }

    #[test]
    fn extract_creator_returns_empty_when_neither_present() {
        let event = json!({
            "type": "m.room.create",
            "content": {}
        });
        assert_eq!(extract_room_creator(&event), "");
    }
}
