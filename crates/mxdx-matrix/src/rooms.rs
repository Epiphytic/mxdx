use serde_json::Value;

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
    room::RoomType,
    OwnedRoomId, RoomId, UserId,
};
use crate::error::Result;
use crate::MatrixClient;

/// Room IDs for a launcher space and its child rooms.
#[derive(Debug, Clone)]
pub struct LauncherTopology {
    pub space_id: OwnedRoomId,
    pub exec_room_id: OwnedRoomId,
    pub status_room_id: OwnedRoomId,
    pub logs_room_id: OwnedRoomId,
}

impl MatrixClient {
    /// Create a launcher space with exec, status, and logs child rooms.
    /// The space is a Matrix Space (m.space), exec is encrypted, status and logs are unencrypted.
    /// All rooms are named and tagged with topics for discoverability.
    pub async fn create_launcher_space(&self, launcher_id: &str) -> Result<LauncherTopology> {
        let server_name = self
            .user_id()
            .server_name()
            .to_string();

        // Create the space room
        let mut creation_content = CreationContent::new();
        creation_content.room_type = Some(RoomType::Space);

        let space_topic = InitialStateEvent::new(
            EmptyStateKey,
            RoomTopicEventContent::new(format!("org.mxdx.launcher.space:{launcher_id}")),
        );

        let mut space_request = CreateRoomRequest::new();
        space_request.name = Some(format!("mxdx: {launcher_id}"));
        space_request.creation_content =
            Some(matrix_sdk::ruma::serde::Raw::new(&creation_content).expect("serialize creation_content"));
        space_request.initial_state = vec![space_topic.to_raw_any()];

        let space_response = self.inner().create_room(space_request).await?;
        let space_id = space_response.room_id().to_owned();

        // Create child rooms (with optional delay for rate-limited servers)
        let delay = self.room_creation_delay();

        let exec_room_id = self.create_named_encrypted_room(
            &format!("mxdx: {launcher_id} — exec"),
            &format!("org.mxdx.launcher.exec:{launcher_id}"),
            &[],
        ).await?;
        if let Some(d) = delay {
            tokio::time::sleep(d).await;
        }
        let status_room_id = self.create_named_unencrypted_room(
            &format!("mxdx: {launcher_id} — status"),
            &format!("org.mxdx.launcher.status:{launcher_id}"),
        ).await?;
        if let Some(d) = delay {
            tokio::time::sleep(d).await;
        }
        let logs_room_id = self.create_named_unencrypted_room(
            &format!("mxdx: {launcher_id} — logs"),
            &format!("org.mxdx.launcher.logs:{launcher_id}"),
        ).await?;

        // Link child rooms to space via m.space.child state events
        let via = serde_json::json!({ "via": [server_name] });
        for child_id in [&exec_room_id, &status_room_id, &logs_room_id] {
            self.send_state_event(&space_id, "m.space.child", child_id.as_str(), via.clone())
                .await?;
        }

        Ok(LauncherTopology {
            space_id,
            exec_room_id,
            status_room_id,
            logs_room_id,
        })
    }

    /// Find an existing launcher space by scanning joined rooms for a matching topic.
    /// Returns None if no space is found for this launcher_id.
    pub async fn find_launcher_space(&self, launcher_id: &str) -> Result<Option<LauncherTopology>> {
        let expected_space_topic = format!("org.mxdx.launcher.space:{launcher_id}");
        let expected_exec_topic = format!("org.mxdx.launcher.exec:{launcher_id}");
        let expected_status_topic = format!("org.mxdx.launcher.status:{launcher_id}");
        let expected_logs_topic = format!("org.mxdx.launcher.logs:{launcher_id}");

        // Sync to ensure we have current room state
        self.sync_once().await?;

        let mut space_id: Option<OwnedRoomId> = None;
        let mut exec_room_id: Option<OwnedRoomId> = None;
        let mut status_room_id: Option<OwnedRoomId> = None;
        let mut logs_room_id: Option<OwnedRoomId> = None;

        for room in self.inner().joined_rooms() {
            let topic = room.topic().unwrap_or_default();
            let rid = room.room_id().to_owned();

            if topic == expected_space_topic {
                space_id = Some(rid);
            } else if topic == expected_exec_topic {
                exec_room_id = Some(rid);
            } else if topic == expected_status_topic {
                status_room_id = Some(rid);
            } else if topic == expected_logs_topic {
                logs_room_id = Some(rid);
            }
        }

        match (space_id, exec_room_id, status_room_id, logs_room_id) {
            (Some(s), Some(e), Some(st), Some(l)) => Ok(Some(LauncherTopology {
                space_id: s,
                exec_room_id: e,
                status_room_id: st,
                logs_room_id: l,
            })),
            _ => Ok(None),
        }
    }

    /// Find an existing launcher space or create a new one.
    pub async fn get_or_create_launcher_space(&self, launcher_id: &str) -> Result<LauncherTopology> {
        if let Some(topology) = self.find_launcher_space(launcher_id).await? {
            return Ok(topology);
        }
        self.create_launcher_space(launcher_id).await
    }

    /// Create an encrypted DM room with history_visibility set to "joined" (mxdx-aew).
    pub async fn create_terminal_session_dm(&self, user_id: &UserId) -> Result<OwnedRoomId> {
        let encryption_event = InitialStateEvent::new(
            EmptyStateKey,
            RoomEncryptionEventContent::with_recommended_defaults(),
        );

        let history_event = InitialStateEvent::new(
            EmptyStateKey,
            RoomHistoryVisibilityEventContent::new(HistoryVisibility::Joined),
        );

        let topic_event = InitialStateEvent::new(
            EmptyStateKey,
            RoomTopicEventContent::new(format!("org.mxdx.terminal.session:{}", user_id.localpart())),
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

        let response = self.inner().create_room(request).await?;
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

    /// Fetch a specific state event from a room via the REST API.
    pub async fn get_room_state(
        &self,
        room_id: &RoomId,
        event_type: &str,
    ) -> Result<Value> {
        let homeserver = self.inner().homeserver();
        let access_token = self
            .inner()
            .access_token()
            .expect("Client is not logged in — no access_token");

        let url = format!(
            "{}_matrix/client/v3/rooms/{}/state/{}",
            homeserver,
            room_id,
            event_type,
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
    async fn create_named_encrypted_room(
        &self,
        name: &str,
        topic: &str,
        invite: &[matrix_sdk::ruma::OwnedUserId],
    ) -> Result<OwnedRoomId> {
        let encryption_event =
            InitialStateEvent::new(EmptyStateKey, RoomEncryptionEventContent::with_recommended_defaults());
        let topic_event =
            InitialStateEvent::new(EmptyStateKey, RoomTopicEventContent::new(topic.to_string()));

        let mut request = CreateRoomRequest::new();
        request.name = Some(name.to_string());
        request.invite = invite.to_vec();
        request.initial_state = vec![encryption_event.to_raw_any(), topic_event.to_raw_any()];

        let response = self.inner().create_room(request).await?;
        Ok(response.room_id().to_owned())
    }

    /// Create a named unencrypted room with a topic for discoverability.
    async fn create_named_unencrypted_room(&self, name: &str, topic: &str) -> Result<OwnedRoomId> {
        let topic_event =
            InitialStateEvent::new(EmptyStateKey, RoomTopicEventContent::new(topic.to_string()));

        let mut request = CreateRoomRequest::new();
        request.name = Some(name.to_string());
        request.initial_state = vec![topic_event.to_raw_any()];

        let response = self.inner().create_room(request).await?;
        Ok(response.room_id().to_owned())
    }
}
