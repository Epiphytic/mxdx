use serde_json::Value;

use matrix_sdk::ruma::{
    api::client::room::create_room::v3::{CreationContent, Request as CreateRoomRequest},
    events::{
        room::{
            encryption::RoomEncryptionEventContent,
            history_visibility::{HistoryVisibility, RoomHistoryVisibilityEventContent},
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
    pub async fn create_launcher_space(&self, launcher_id: &str) -> Result<LauncherTopology> {
        let server_name = self
            .user_id()
            .server_name()
            .to_string();

        // Create the space room
        let mut creation_content = CreationContent::new();
        creation_content.room_type = Some(RoomType::Space);

        let mut space_request = CreateRoomRequest::new();
        space_request.name = Some(format!("{launcher_id}"));
        space_request.creation_content =
            Some(matrix_sdk::ruma::serde::Raw::new(&creation_content).expect("serialize creation_content"));

        let space_response = self.inner().create_room(space_request).await?;
        let space_id = space_response.room_id().to_owned();

        // Create child rooms
        let exec_room_id = self.create_encrypted_room(&[]).await?;
        let status_room_id = self.create_unencrypted_room(None).await?;
        let logs_room_id = self.create_unencrypted_room(None).await?;

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

        let mut request = CreateRoomRequest::new();
        request.invite = vec![user_id.to_owned()];
        request.is_direct = true;
        request.initial_state = vec![
            encryption_event.to_raw_any(),
            history_event.to_raw_any(),
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

    /// Create an unencrypted room (helper for status/logs rooms).
    async fn create_unencrypted_room(&self, name: Option<&str>) -> Result<OwnedRoomId> {
        let mut request = CreateRoomRequest::new();
        if let Some(n) = name {
            request.name = Some(n.to_string());
        }
        let response = self.inner().create_room(request).await?;
        Ok(response.room_id().to_owned())
    }
}
