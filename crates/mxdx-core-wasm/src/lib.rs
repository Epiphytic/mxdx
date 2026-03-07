use wasm_bindgen::prelude::*;
use matrix_sdk::{
    config::SyncSettings,
    room::MessagesOptions,
    ruma::{
        api::client::room::create_room::v3::{CreationContent, Request as CreateRoomRequest},
        events::{
            room::{
                encryption::RoomEncryptionEventContent,
                topic::RoomTopicEventContent,
            },
            EmptyStateKey, InitialStateEvent,
        },
        room::RoomType,
    },
    Client,
};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[wasm_bindgen(start)]
pub fn init() {
    console_error_panic_hook::set_once();
}

/// Smoke test: returns the matrix-sdk version string to prove it compiled.
#[wasm_bindgen]
pub fn sdk_version() -> String {
    "matrix-sdk-0.16-wasm".to_string()
}

/// Room IDs for a launcher space topology, serialized to/from JS.
#[derive(Serialize, Deserialize)]
pub struct LauncherTopology {
    pub space_id: String,
    pub exec_room_id: String,
    pub status_room_id: String,
    pub logs_room_id: String,
}

fn to_js_err(e: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&e.to_string())
}

#[wasm_bindgen]
pub struct WasmMatrixClient {
    client: Client,
}

#[wasm_bindgen]
impl WasmMatrixClient {
    /// Register a new user on a homeserver with a registration token.
    #[wasm_bindgen(js_name = "register")]
    pub async fn register(
        homeserver_url: &str,
        username: &str,
        password: &str,
        registration_token: &str,
    ) -> Result<WasmMatrixClient, JsValue> {
        let client = Client::builder()
            .homeserver_url(homeserver_url)
            .build()
            .await
            .map_err(to_js_err)?;

        let reg_url = format!("{homeserver_url}/_matrix/client/v3/register");
        let body = serde_json::json!({
            "username": username,
            "password": password,
            "auth": {
                "type": "m.login.registration_token",
                "token": registration_token
            }
        });

        let http_client = reqwest::Client::new();
        let resp = http_client
            .post(&reg_url)
            .json(&body)
            .send()
            .await
            .map_err(|e| to_js_err(format!("Registration request failed: {e}")))?;

        if !resp.status().is_success() {
            let err_body = resp.text().await.unwrap_or_else(|_| "unknown error".to_string());
            return Err(to_js_err(format!("Registration failed: {err_body}")));
        }

        client
            .matrix_auth()
            .login_username(username, password)
            .initial_device_display_name("mxdx")
            .await
            .map_err(to_js_err)?;

        client
            .sync_once(SyncSettings::default().timeout(Duration::from_secs(5)))
            .await
            .map_err(to_js_err)?;

        Ok(WasmMatrixClient { client })
    }

    /// Login to a Matrix server.
    #[wasm_bindgen(js_name = "login")]
    pub async fn login(
        server_name: &str,
        username: &str,
        password: &str,
    ) -> Result<WasmMatrixClient, JsValue> {
        let builder = Client::builder();
        let client = if server_name.contains("://") {
            builder.homeserver_url(server_name)
        } else {
            builder.server_name_or_homeserver_url(server_name)
        }
        .build()
        .await
        .map_err(to_js_err)?;

        client
            .matrix_auth()
            .login_username(username, password)
            .initial_device_display_name("mxdx")
            .await
            .map_err(to_js_err)?;

        client
            .sync_once(SyncSettings::default().timeout(Duration::from_secs(5)))
            .await
            .map_err(to_js_err)?;

        Ok(WasmMatrixClient { client })
    }

    #[wasm_bindgen(js_name = "isLoggedIn")]
    pub fn is_logged_in(&self) -> bool {
        self.client.user_id().is_some()
    }

    #[wasm_bindgen(js_name = "userId")]
    pub fn user_id(&self) -> Option<String> {
        self.client.user_id().map(|u| u.to_string())
    }

    #[wasm_bindgen(js_name = "syncOnce")]
    pub async fn sync_once(&self) -> Result<(), JsValue> {
        self.client
            .sync_once(SyncSettings::default().timeout(Duration::from_secs(1)))
            .await
            .map_err(to_js_err)?;
        Ok(())
    }

    /// Create a launcher space with exec, status, and logs child rooms.
    /// Returns JSON: { space_id, exec_room_id, status_room_id, logs_room_id }
    #[wasm_bindgen(js_name = "createLauncherSpace")]
    pub async fn create_launcher_space(&self, launcher_id: &str) -> Result<JsValue, JsValue> {
        let server_name = self.client.user_id()
            .ok_or_else(|| to_js_err("Not logged in"))?
            .server_name()
            .to_string();

        // Create space room
        let mut creation_content = CreationContent::new();
        creation_content.room_type = Some(RoomType::Space);

        let space_topic = InitialStateEvent::new(
            EmptyStateKey,
            RoomTopicEventContent::new(format!("org.mxdx.launcher.space:{launcher_id}")),
        );

        let mut space_request = CreateRoomRequest::new();
        space_request.name = Some(format!("mxdx: {launcher_id}"));
        space_request.creation_content = Some(
            matrix_sdk::ruma::serde::Raw::new(&creation_content)
                .map_err(|e| to_js_err(format!("Failed to serialize creation content: {e}")))?
        );
        space_request.initial_state = vec![space_topic.to_raw_any()];

        let space = self.client.create_room(space_request).await.map_err(to_js_err)?;
        let space_id = space.room_id().to_string();

        // Create exec room (encrypted)
        let exec_room_id = self.create_named_encrypted_room(
            &format!("mxdx: {launcher_id} — exec"),
            &format!("org.mxdx.launcher.exec:{launcher_id}"),
        ).await?;

        // Create status room (unencrypted)
        let status_room_id = self.create_named_room(
            &format!("mxdx: {launcher_id} — status"),
            &format!("org.mxdx.launcher.status:{launcher_id}"),
        ).await?;

        // Create logs room (unencrypted)
        let logs_room_id = self.create_named_room(
            &format!("mxdx: {launcher_id} — logs"),
            &format!("org.mxdx.launcher.logs:{launcher_id}"),
        ).await?;

        // Link child rooms to space
        let via = serde_json::json!({ "via": [server_name] });
        for child_id in [&exec_room_id, &status_room_id, &logs_room_id] {
            let room = self.client.get_room(space.room_id())
                .ok_or_else(|| to_js_err("Space room not found"))?;
            room.send_state_event_raw("m.space.child", child_id, via.clone())
                .await
                .map_err(to_js_err)?;
        }

        let topology = LauncherTopology {
            space_id,
            exec_room_id,
            status_room_id,
            logs_room_id,
        };
        serde_wasm_bindgen::to_value(&topology).map_err(to_js_err)
    }

    /// Find an existing launcher space by scanning joined rooms for matching topics.
    /// Returns JSON topology or null.
    #[wasm_bindgen(js_name = "findLauncherSpace")]
    pub async fn find_launcher_space(&self, launcher_id: &str) -> Result<JsValue, JsValue> {
        self.sync_once().await?;

        let expected_space = format!("org.mxdx.launcher.space:{launcher_id}");
        let expected_exec = format!("org.mxdx.launcher.exec:{launcher_id}");
        let expected_status = format!("org.mxdx.launcher.status:{launcher_id}");
        let expected_logs = format!("org.mxdx.launcher.logs:{launcher_id}");

        let mut space_id = None;
        let mut exec_room_id = None;
        let mut status_room_id = None;
        let mut logs_room_id = None;

        for room in self.client.joined_rooms() {
            let topic = room.topic().unwrap_or_default();
            let rid = room.room_id().to_string();

            if topic == expected_space {
                space_id = Some(rid);
            } else if topic == expected_exec {
                exec_room_id = Some(rid);
            } else if topic == expected_status {
                status_room_id = Some(rid);
            } else if topic == expected_logs {
                logs_room_id = Some(rid);
            }
        }

        match (space_id, exec_room_id, status_room_id, logs_room_id) {
            (Some(s), Some(e), Some(st), Some(l)) => {
                let topology = LauncherTopology {
                    space_id: s,
                    exec_room_id: e,
                    status_room_id: st,
                    logs_room_id: l,
                };
                serde_wasm_bindgen::to_value(&topology).map_err(to_js_err)
            }
            _ => Ok(JsValue::NULL),
        }
    }

    /// Find or create a launcher space (idempotent).
    #[wasm_bindgen(js_name = "getOrCreateLauncherSpace")]
    pub async fn get_or_create_launcher_space(&self, launcher_id: &str) -> Result<JsValue, JsValue> {
        let existing = self.find_launcher_space(launcher_id).await?;
        if !existing.is_null() {
            return Ok(existing);
        }
        self.create_launcher_space(launcher_id).await
    }

    /// Send a custom event to a room.
    #[wasm_bindgen(js_name = "sendEvent")]
    pub async fn send_event(
        &self,
        room_id: &str,
        event_type: &str,
        content_json: &str,
    ) -> Result<(), JsValue> {
        let rid = <&matrix_sdk::ruma::RoomId>::try_from(room_id).map_err(to_js_err)?;
        let room = self.client.get_room(rid)
            .ok_or_else(|| to_js_err(format!("Room not found: {room_id}")))?;
        let content: serde_json::Value = serde_json::from_str(content_json).map_err(to_js_err)?;
        room.send_raw(event_type, content).await.map_err(to_js_err)?;
        Ok(())
    }

    /// Send a state event to a room.
    #[wasm_bindgen(js_name = "sendStateEvent")]
    pub async fn send_state_event(
        &self,
        room_id: &str,
        event_type: &str,
        state_key: &str,
        content_json: &str,
    ) -> Result<(), JsValue> {
        let rid = <&matrix_sdk::ruma::RoomId>::try_from(room_id).map_err(to_js_err)?;
        let room = self.client.get_room(rid)
            .ok_or_else(|| to_js_err(format!("Room not found: {room_id}")))?;
        let content: serde_json::Value = serde_json::from_str(content_json).map_err(to_js_err)?;
        room.send_state_event_raw(event_type, state_key, content).await.map_err(to_js_err)?;
        Ok(())
    }

    /// Sync and collect events from a room. Returns JSON array of events.
    #[wasm_bindgen(js_name = "collectRoomEvents")]
    pub async fn collect_room_events(
        &self,
        room_id: &str,
        timeout_secs: u32,
    ) -> Result<JsValue, JsValue> {
        let rid = <&matrix_sdk::ruma::RoomId>::try_from(room_id).map_err(to_js_err)?;
        let timeout = Duration::from_secs(timeout_secs as u64);
        let deadline = web_time::Instant::now() + timeout;

        while web_time::Instant::now() < deadline {
            self.sync_once().await?;

            if let Some(room) = self.client.get_room(rid) {
                let messages = room.messages(MessagesOptions::backward()).await.map_err(to_js_err)?;
                let mut collected: Vec<serde_json::Value> = Vec::new();
                for event in &messages.chunk {
                    if let Ok(json) = serde_json::to_value(event.raw().json()) {
                        let event_type = json.get("type").and_then(|t| t.as_str());
                        if event_type != Some("m.room.encrypted")
                            && event_type != Some("m.room.encryption")
                            && event_type != Some("m.room.member")
                        {
                            collected.push(json);
                        }
                    }
                }
                if !collected.is_empty() {
                    return serde_wasm_bindgen::to_value(&collected).map_err(to_js_err);
                }
            }
        }

        serde_wasm_bindgen::to_value(&Vec::<serde_json::Value>::new()).map_err(to_js_err)
    }
}

// Private helpers
impl WasmMatrixClient {
    async fn create_named_encrypted_room(&self, name: &str, topic: &str) -> Result<String, JsValue> {
        let encryption_event = InitialStateEvent::new(
            EmptyStateKey,
            RoomEncryptionEventContent::with_recommended_defaults(),
        );
        let topic_event = InitialStateEvent::new(
            EmptyStateKey,
            RoomTopicEventContent::new(topic.to_string()),
        );

        let mut request = CreateRoomRequest::new();
        request.name = Some(name.to_string());
        request.initial_state = vec![encryption_event.to_raw_any(), topic_event.to_raw_any()];

        let response = self.client.create_room(request).await.map_err(to_js_err)?;
        Ok(response.room_id().to_string())
    }

    async fn create_named_room(&self, name: &str, topic: &str) -> Result<String, JsValue> {
        let topic_event = InitialStateEvent::new(
            EmptyStateKey,
            RoomTopicEventContent::new(topic.to_string()),
        );

        let mut request = CreateRoomRequest::new();
        request.name = Some(name.to_string());
        request.initial_state = vec![topic_event.to_raw_any()];

        let response = self.client.create_room(request).await.map_err(to_js_err)?;
        Ok(response.room_id().to_string())
    }
}
