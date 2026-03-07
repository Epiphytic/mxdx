use wasm_bindgen::prelude::*;
use matrix_sdk::{
    config::SyncSettings,
    room::MessagesOptions,
    ruma::{
        api::client::{
            room::create_room::v3::{CreationContent, Request as CreateRoomRequest},
            uiaa,
        },
        events::{
            room::{
                encryption::RoomEncryptionEventContent,
                topic::RoomTopicEventContent,
            },
            EmptyStateKey, InitialStateEvent,
        },
        room::RoomType,
        OwnedUserId,
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

    /// Invite a user to a room.
    #[wasm_bindgen(js_name = "inviteUser")]
    pub async fn invite_user(&self, room_id: &str, user_id: &str) -> Result<(), JsValue> {
        let rid = <&matrix_sdk::ruma::RoomId>::try_from(room_id).map_err(to_js_err)?;
        let uid = <&matrix_sdk::ruma::UserId>::try_from(user_id).map_err(to_js_err)?;
        let room = self.client.get_room(rid)
            .ok_or_else(|| to_js_err(format!("Room not found: {room_id}")))?;
        room.invite_user_by_id(uid).await.map_err(to_js_err)?;
        Ok(())
    }

    /// Accept a pending room invitation.
    #[wasm_bindgen(js_name = "joinRoom")]
    pub async fn join_room(&self, room_id: &str) -> Result<(), JsValue> {
        let rid = <&matrix_sdk::ruma::RoomId>::try_from(room_id).map_err(to_js_err)?;
        self.client.join_room_by_id(rid).await.map_err(to_js_err)?;
        Ok(())
    }

    /// Get list of invited room IDs (pending invitations).
    #[wasm_bindgen(js_name = "invitedRoomIds")]
    pub fn invited_room_ids(&self) -> Vec<String> {
        self.client.invited_rooms().iter().map(|r| r.room_id().to_string()).collect()
    }

    /// Export the current session as JSON for persistence.
    /// Returns JSON: { user_id, device_id, access_token, homeserver_url }
    /// Store this in the OS keyring — never write it to a config file.
    #[wasm_bindgen(js_name = "exportSession")]
    pub fn export_session(&self) -> Result<String, JsValue> {
        let session = self.client.matrix_auth().session()
            .ok_or_else(|| to_js_err("No active session to export"))?;

        let data = serde_json::json!({
            "user_id": session.meta.user_id.to_string(),
            "device_id": session.meta.device_id.to_string(),
            "access_token": session.tokens.access_token,
            "homeserver_url": self.client.homeserver().to_string(),
        });
        serde_json::to_string(&data).map_err(to_js_err)
    }

    /// Restore a previously exported session without logging in again.
    /// Reuses the same device_id, avoiding rate limits and preserving cross-signing.
    /// The session_json should be the output of exportSession().
    #[wasm_bindgen(js_name = "restoreSession")]
    pub async fn restore_session(session_json: &str) -> Result<WasmMatrixClient, JsValue> {
        let parsed: serde_json::Value = serde_json::from_str(session_json).map_err(to_js_err)?;

        let homeserver_url = parsed["homeserver_url"].as_str()
            .ok_or_else(|| to_js_err("Missing homeserver_url in session data"))?;
        let user_id = parsed["user_id"].as_str()
            .ok_or_else(|| to_js_err("Missing user_id in session data"))?;
        let device_id = parsed["device_id"].as_str()
            .ok_or_else(|| to_js_err("Missing device_id in session data"))?;
        let access_token = parsed["access_token"].as_str()
            .ok_or_else(|| to_js_err("Missing access_token in session data"))?;

        let client = Client::builder()
            .homeserver_url(homeserver_url)
            .build()
            .await
            .map_err(to_js_err)?;

        let session = matrix_sdk::authentication::matrix::MatrixSession {
            meta: matrix_sdk::SessionMeta {
                user_id: user_id.try_into().map_err(to_js_err)?,
                device_id: device_id.into(),
            },
            tokens: matrix_sdk::authentication::SessionTokens {
                access_token: access_token.to_string(),
                refresh_token: None,
            },
        };

        client.restore_session(session).await.map_err(to_js_err)?;

        // Sync to re-establish crypto state
        client
            .sync_once(SyncSettings::default().timeout(Duration::from_secs(5)))
            .await
            .map_err(to_js_err)?;

        Ok(WasmMatrixClient { client })
    }

    /// Bootstrap cross-signing for this device.
    /// Generates cross-signing keys and uploads them. Handles the two-step UIA
    /// flow by capturing the session ID from the 401 response and including it
    /// in the password auth retry.
    #[wasm_bindgen(js_name = "bootstrapCrossSigning")]
    pub async fn bootstrap_cross_signing(&self, password: &str) -> Result<(), JsValue> {
        let encryption = self.client.encryption();

        // Try without auth first (UIA grace period right after login)
        match encryption.bootstrap_cross_signing(None).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                let uiaa_info = e.as_uiaa_response()
                    .ok_or_else(|| to_js_err(format!("Cross-signing bootstrap failed (not UIA): {e}")))?;

                // Extract UIA session from the 401 response
                let session = uiaa_info.session.clone();

                let user_id = self.client.user_id()
                    .ok_or_else(|| to_js_err("Not logged in"))?;

                let mut password_auth = uiaa::Password::new(
                    uiaa::UserIdentifier::UserIdOrLocalpart(user_id.localpart().to_owned()),
                    password.to_owned(),
                );
                password_auth.session = session;

                encryption
                    .bootstrap_cross_signing(Some(uiaa::AuthData::Password(password_auth)))
                    .await
                    .map_err(|e| to_js_err(format!("Cross-signing UIA auth failed: {e}")))?;
            }
        }

        Ok(())
    }

    /// Bootstrap cross-signing only if not already set up.
    /// No-op if keys exist and private parts are in the local crypto store.
    /// Falls back to full bootstrap if private keys are missing (e.g. after
    /// session restore with ephemeral crypto store).
    #[wasm_bindgen(js_name = "bootstrapCrossSigningIfNeeded")]
    pub async fn bootstrap_cross_signing_if_needed(&self, password: &str) -> Result<(), JsValue> {
        let encryption = self.client.encryption();

        match encryption.bootstrap_cross_signing_if_needed(None).await {
            Ok(()) => return Ok(()),
            Err(_) => {
                // Either UIA required or private keys missing locally —
                // fall through to full bootstrap
            }
        }

        self.bootstrap_cross_signing(password).await
    }

    /// Get the device ID of the current session.
    #[wasm_bindgen(js_name = "deviceId")]
    pub fn device_id(&self) -> Option<String> {
        self.client.device_id().map(|d| d.to_string())
    }

    /// Verify another user's identity by signing their master key with our
    /// user-signing key. Both users must have bootstrapped cross-signing first.
    /// This is a one-way operation — the other user must also call this to
    /// verify us back.
    #[wasm_bindgen(js_name = "verifyUser")]
    pub async fn verify_user(&self, user_id_str: &str) -> Result<(), JsValue> {
        let user_id: OwnedUserId = user_id_str.try_into()
            .map_err(|e| to_js_err(format!("Invalid user ID '{user_id_str}': {e}")))?;

        let encryption = self.client.encryption();

        let identity = encryption.get_user_identity(&user_id).await
            .map_err(|e| to_js_err(format!("Failed to get user identity: {e}")))?
            .ok_or_else(|| to_js_err(format!("No identity found for {user_id_str} — they may not have bootstrapped cross-signing")))?;

        identity.verify().await
            .map_err(|e| to_js_err(format!("Failed to verify {user_id_str}: {e}")))?;

        Ok(())
    }

    /// Verify our own user identity (marks it as locally verified).
    /// This is needed before verifying other users — our own identity must
    /// be verified first.
    #[wasm_bindgen(js_name = "verifyOwnIdentity")]
    pub async fn verify_own_identity(&self) -> Result<(), JsValue> {
        let user_id = self.client.user_id()
            .ok_or_else(|| to_js_err("Not logged in"))?
            .to_owned();

        let encryption = self.client.encryption();

        let identity = encryption.get_user_identity(&user_id).await
            .map_err(|e| to_js_err(format!("Failed to get own identity: {e}")))?
            .ok_or_else(|| to_js_err("No identity found — bootstrap cross-signing first"))?;

        identity.verify().await
            .map_err(|e| to_js_err(format!("Failed to verify own identity: {e}")))?;

        Ok(())
    }

    /// Check if a user's identity is verified from our perspective.
    #[wasm_bindgen(js_name = "isUserVerified")]
    pub async fn is_user_verified(&self, user_id_str: &str) -> Result<bool, JsValue> {
        let user_id: OwnedUserId = user_id_str.try_into()
            .map_err(|e| to_js_err(format!("Invalid user ID '{user_id_str}': {e}")))?;

        let identity = self.client.encryption().get_user_identity(&user_id).await
            .map_err(|e| to_js_err(format!("Failed to get user identity: {e}")))?;

        Ok(identity.map(|i| i.is_verified()).unwrap_or(false))
    }

    /// Create a launcher space with exec and logs child rooms (both E2EE + MSC4362).
    /// Returns JSON: { space_id, exec_room_id, logs_room_id }
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

        // Create exec room (E2EE + MSC4362)
        let exec_room_id = self.create_named_encrypted_room(
            &format!("mxdx: {launcher_id} — exec"),
            &format!("org.mxdx.launcher.exec:{launcher_id}"),
        ).await?;

        // Create logs room (E2EE + MSC4362)
        let logs_room_id = self.create_named_encrypted_room(
            &format!("mxdx: {launcher_id} — logs"),
            &format!("org.mxdx.launcher.logs:{launcher_id}"),
        ).await?;

        // Link child rooms to space
        let via = serde_json::json!({ "via": [server_name] });
        for child_id in [&exec_room_id, &logs_room_id] {
            let room = self.client.get_room(space.room_id())
                .ok_or_else(|| to_js_err("Space room not found"))?;
            room.send_state_event_raw("m.space.child", child_id, via.clone())
                .await
                .map_err(to_js_err)?;
        }

        let topology = LauncherTopology {
            space_id,
            exec_room_id,
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
        let expected_logs = format!("org.mxdx.launcher.logs:{launcher_id}");

        let mut space_id = None;
        let mut exec_room_id = None;
        let mut logs_room_id = None;

        for room in self.client.joined_rooms() {
            let topic = room.topic().unwrap_or_default();
            let rid = room.room_id().to_string();

            if topic == expected_space {
                space_id = Some(rid);
            } else if topic == expected_exec {
                exec_room_id = Some(rid);
            } else if topic == expected_logs {
                logs_room_id = Some(rid);
            }
        }

        match (space_id, exec_room_id, logs_room_id) {
            (Some(s), Some(e), Some(l)) => {
                let topology = LauncherTopology {
                    space_id: s,
                    exec_room_id: e,
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

    /// Sync and collect events from a room. Returns JSON string of event array.
    #[wasm_bindgen(js_name = "collectRoomEvents")]
    pub async fn collect_room_events(
        &self,
        room_id: &str,
        timeout_secs: u32,
    ) -> Result<String, JsValue> {
        let rid = <&matrix_sdk::ruma::RoomId>::try_from(room_id).map_err(to_js_err)?;
        let timeout = Duration::from_secs(timeout_secs as u64);
        let deadline = web_time::Instant::now() + timeout;

        while web_time::Instant::now() < deadline {
            self.sync_once().await?;

            if let Some(room) = self.client.get_room(rid) {
                let messages = room.messages(MessagesOptions::backward()).await.map_err(to_js_err)?;
                let mut collected: Vec<serde_json::Value> = Vec::new();
                for event in &messages.chunk {
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(event.raw().json().get()) {
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
                    return serde_json::to_string(&collected).map_err(to_js_err);
                }
            }
        }

        Ok("[]".to_string())
    }
}

// Private helpers
impl WasmMatrixClient {
    async fn create_named_encrypted_room(&self, name: &str, topic: &str) -> Result<String, JsValue> {
        let encryption_event = InitialStateEvent::new(
            EmptyStateKey,
            RoomEncryptionEventContent::with_recommended_defaults().with_encrypted_state(),
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
}
