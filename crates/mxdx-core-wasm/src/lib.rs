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
                history_visibility::{HistoryVisibility, RoomHistoryVisibilityEventContent},
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
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

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

/// Delete IndexedDB databases by name prefix. Clears stale crypto stores
/// when the device_id from a previous session conflicts with a fresh login.
async fn delete_indexeddb_store(name: &str) {
    let global = js_sys::global();
    let idb = js_sys::Reflect::get(&global, &"indexedDB".into())
        .ok()
        .and_then(|v| v.dyn_into::<web_sys::IdbFactory>().ok());

    if let Some(factory) = idb {
        for suffix in ["", "::matrix-sdk-crypto", "::matrix-sdk-state"] {
            let db_name = format!("{name}{suffix}");
            match factory.delete_database(&db_name) {
                Ok(req) => {
                    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
                        let resolve_clone = resolve.clone();
                        let cb = wasm_bindgen::closure::Closure::once_into_js(move || {
                            let _ = resolve_clone.call0(&JsValue::NULL);
                        });
                        req.set_onsuccess(Some(cb.unchecked_ref()));
                        let resolve_clone2 = resolve.clone();
                        let cb2 = wasm_bindgen::closure::Closure::once_into_js(move || {
                            let _ = resolve_clone2.call0(&JsValue::NULL);
                        });
                        req.set_onerror(Some(cb2.unchecked_ref()));
                    });
                    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
                    web_sys::console::log_1(&format!("[mxdx] Deleted IndexedDB: {db_name}").into());
                }
                Err(e) => {
                    web_sys::console::warn_1(
                        &format!("[mxdx] Failed to delete IndexedDB {db_name}: {:?}", e).into(),
                    );
                }
            }
        }
    }
}

#[wasm_bindgen]
pub struct WasmMatrixClient {
    client: Client,
    store_name: String,
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
        let store_name = format!(
            "mxdx_{}_{}",
            username,
            homeserver_url.replace([':', '/', '.'], "_")
        );
        let client = Client::builder()
            .homeserver_url(homeserver_url)
            .indexeddb_store(&store_name, None)
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
            let err_body = resp
                .text()
                .await
                .unwrap_or_else(|_| "unknown error".to_string());
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

        Ok(WasmMatrixClient { client, store_name })
    }

    /// Login to a Matrix server.
    /// Always clears any existing IndexedDB crypto store first — a fresh login
    /// creates a new device, so any prior crypto state is stale by definition.
    /// Session restore (restoreSession) is the path that preserves crypto state.
    #[wasm_bindgen(js_name = "login")]
    pub async fn login(
        server_name: &str,
        username: &str,
        password: &str,
    ) -> Result<WasmMatrixClient, JsValue> {
        let store_name = format!(
            "mxdx_{}_{}",
            username,
            server_name.replace([':', '/', '.'], "_")
        );

        // Fresh login = new device. Any existing crypto store is stale
        // (previous device may have been deleted, keys are invalid).
        // Clear it unconditionally to prevent sync hangs.
        delete_indexeddb_store(&store_name).await;

        let builder = Client::builder().indexeddb_store(&store_name, None);
        let builder = if server_name.contains("://") {
            builder.homeserver_url(server_name)
        } else {
            builder.server_name_or_homeserver_url(server_name)
        };

        let client = builder.build().await.map_err(to_js_err)?;

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

        Ok(WasmMatrixClient { client, store_name })
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
        let room = self
            .client
            .get_room(rid)
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
        self.client
            .invited_rooms()
            .iter()
            .map(|r| r.room_id().to_string())
            .collect()
    }

    /// Export the current session as JSON for persistence.
    /// Returns JSON: { user_id, device_id, access_token, homeserver_url }
    /// Store this in the OS keyring — never write it to a config file.
    #[wasm_bindgen(js_name = "exportSession")]
    pub fn export_session(&self) -> Result<String, JsValue> {
        let session = self
            .client
            .matrix_auth()
            .session()
            .ok_or_else(|| to_js_err("No active session to export"))?;

        let data = serde_json::json!({
            "user_id": session.meta.user_id.to_string(),
            "device_id": session.meta.device_id.to_string(),
            "access_token": session.tokens.access_token,
            "homeserver_url": self.client.homeserver().to_string(),
            "store_name": self.store_name,
        });
        serde_json::to_string(&data).map_err(to_js_err)
    }

    /// Restore a previously exported session without logging in again.
    /// Reuses the same device_id, avoiding rate limits and preserving cross-signing.
    /// The session_json should be the output of exportSession().
    #[wasm_bindgen(js_name = "restoreSession")]
    pub async fn restore_session(session_json: &str) -> Result<WasmMatrixClient, JsValue> {
        let parsed: serde_json::Value = serde_json::from_str(session_json).map_err(to_js_err)?;

        let homeserver_url = parsed["homeserver_url"]
            .as_str()
            .ok_or_else(|| to_js_err("Missing homeserver_url in session data"))?;
        let user_id = parsed["user_id"]
            .as_str()
            .ok_or_else(|| to_js_err("Missing user_id in session data"))?;
        let device_id = parsed["device_id"]
            .as_str()
            .ok_or_else(|| to_js_err("Missing device_id in session data"))?;
        let access_token = parsed["access_token"]
            .as_str()
            .ok_or_else(|| to_js_err("Missing access_token in session data"))?;

        // Use stored store_name if available (ensures same IndexedDB as login),
        // fall back to old format for sessions exported before this fix
        let store_name = parsed["store_name"]
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                format!(
                    "mxdx_{}_{}",
                    user_id,
                    homeserver_url.replace([':', '/', '.'], "_")
                )
            });
        let client = Client::builder()
            .homeserver_url(homeserver_url)
            .indexeddb_store(&store_name, None)
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

        Ok(WasmMatrixClient { client, store_name })
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
                let uiaa_info = e.as_uiaa_response().ok_or_else(|| {
                    to_js_err(format!("Cross-signing bootstrap failed (not UIA): {e}"))
                })?;

                // Extract UIA session from the 401 response
                let session = uiaa_info.session.clone();

                let user_id = self
                    .client
                    .user_id()
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
        let user_id: OwnedUserId = user_id_str
            .try_into()
            .map_err(|e| to_js_err(format!("Invalid user ID '{user_id_str}': {e}")))?;

        let encryption = self.client.encryption();

        let identity = encryption.get_user_identity(&user_id).await
            .map_err(|e| to_js_err(format!("Failed to get user identity: {e}")))?
            .ok_or_else(|| to_js_err(format!("No identity found for {user_id_str} — they may not have bootstrapped cross-signing")))?;

        identity
            .verify()
            .await
            .map_err(|e| to_js_err(format!("Failed to verify {user_id_str}: {e}")))?;

        Ok(())
    }

    /// Verify our own user identity (marks it as locally verified).
    /// This is needed before verifying other users — our own identity must
    /// be verified first.
    #[wasm_bindgen(js_name = "verifyOwnIdentity")]
    pub async fn verify_own_identity(&self) -> Result<(), JsValue> {
        let user_id = self
            .client
            .user_id()
            .ok_or_else(|| to_js_err("Not logged in"))?
            .to_owned();

        let encryption = self.client.encryption();

        let identity = encryption
            .get_user_identity(&user_id)
            .await
            .map_err(|e| to_js_err(format!("Failed to get own identity: {e}")))?
            .ok_or_else(|| to_js_err("No identity found — bootstrap cross-signing first"))?;

        identity
            .verify()
            .await
            .map_err(|e| to_js_err(format!("Failed to verify own identity: {e}")))?;

        Ok(())
    }

    /// Check if a user's identity is verified from our perspective.
    #[wasm_bindgen(js_name = "isUserVerified")]
    pub async fn is_user_verified(&self, user_id_str: &str) -> Result<bool, JsValue> {
        let user_id: OwnedUserId = user_id_str
            .try_into()
            .map_err(|e| to_js_err(format!("Invalid user ID '{user_id_str}': {e}")))?;

        let identity = self
            .client
            .encryption()
            .get_user_identity(&user_id)
            .await
            .map_err(|e| to_js_err(format!("Failed to get user identity: {e}")))?;

        Ok(identity.map(|i| i.is_verified()).unwrap_or(false))
    }

    /// Create a launcher space with exec and logs child rooms (both E2EE + MSC4362).
    /// Returns JSON: { space_id, exec_room_id, logs_room_id }
    #[wasm_bindgen(js_name = "createLauncherSpace")]
    pub async fn create_launcher_space(&self, launcher_id: &str) -> Result<JsValue, JsValue> {
        let server_name = self
            .client
            .user_id()
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
                .map_err(|e| to_js_err(format!("Failed to serialize creation content: {e}")))?,
        );
        space_request.initial_state = vec![space_topic.to_raw_any()];

        let space = self
            .client
            .create_room(space_request)
            .await
            .map_err(to_js_err)?;
        let space_id = space.room_id().to_string();

        // Create exec room (E2EE + MSC4362)
        let exec_room_id = self
            .create_named_encrypted_room(
                &format!("mxdx: {launcher_id} — exec"),
                &format!("org.mxdx.launcher.exec:{launcher_id}"),
            )
            .await?;

        // Create logs room (E2EE + MSC4362)
        let logs_room_id = self
            .create_named_encrypted_room(
                &format!("mxdx: {launcher_id} — logs"),
                &format!("org.mxdx.launcher.logs:{launcher_id}"),
            )
            .await?;

        // Link child rooms to space
        let via = serde_json::json!({ "via": [server_name] });
        for child_id in [&exec_room_id, &logs_room_id] {
            let room = self
                .client
                .get_room(space.room_id())
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
    pub async fn get_or_create_launcher_space(
        &self,
        launcher_id: &str,
    ) -> Result<JsValue, JsValue> {
        let existing = self.find_launcher_space(launcher_id).await?;
        if !existing.is_null() {
            return Ok(existing);
        }
        self.create_launcher_space(launcher_id).await
    }

    /// List all launcher spaces by scanning joined rooms for matching topic patterns.
    /// Returns JSON string: array of { space_id, exec_room_id, logs_room_id, launcher_id }.
    /// Reads from local cache — call syncOnce() before this if you need fresh data.
    #[wasm_bindgen(js_name = "listLauncherSpaces")]
    pub async fn list_launcher_spaces(&self) -> Result<String, JsValue> {
        let space_prefix = "org.mxdx.launcher.space:";
        let exec_prefix = "org.mxdx.launcher.exec:";
        let logs_prefix = "org.mxdx.launcher.logs:";

        // Collect all rooms by topic prefix
        let mut spaces: Vec<(String, String)> = Vec::new(); // (launcher_id, room_id)
        let mut exec_rooms: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        let mut logs_rooms: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();

        for room in self.client.joined_rooms() {
            let topic = room.topic().unwrap_or_default();
            let rid = room.room_id().to_string();

            if let Some(id) = topic.strip_prefix(space_prefix) {
                spaces.push((id.to_string(), rid));
            } else if let Some(id) = topic.strip_prefix(exec_prefix) {
                exec_rooms.insert(id.to_string(), rid);
            } else if let Some(id) = topic.strip_prefix(logs_prefix) {
                logs_rooms.insert(id.to_string(), rid);
            }
        }

        let mut result: Vec<serde_json::Value> = Vec::new();
        for (launcher_id, space_id) in &spaces {
            if let (Some(exec_id), Some(logs_id)) =
                (exec_rooms.get(launcher_id), logs_rooms.get(launcher_id))
            {
                result.push(serde_json::json!({
                    "launcher_id": launcher_id,
                    "space_id": space_id,
                    "exec_room_id": exec_id,
                    "logs_room_id": logs_id,
                }));
            }
        }

        serde_json::to_string(&result).map_err(to_js_err)
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
        let room = self
            .client
            .get_room(rid)
            .ok_or_else(|| to_js_err(format!("Room not found: {room_id}")))?;
        let content: serde_json::Value = serde_json::from_str(content_json).map_err(to_js_err)?;
        room.send_raw(event_type, content)
            .await
            .map_err(to_js_err)?;
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
        let room = self
            .client
            .get_room(rid)
            .ok_or_else(|| to_js_err(format!("Room not found: {room_id}")))?;
        let content: serde_json::Value = serde_json::from_str(content_json).map_err(to_js_err)?;
        room.send_state_event_raw(event_type, state_key, content)
            .await
            .map_err(to_js_err)?;
        Ok(())
    }

    /// Read events from a room's local cache without syncing.
    /// Use this for batch reads after a single syncOnce() call.
    /// Returns JSON string of event array (excluding m.room.encrypted, m.room.encryption, m.room.member).
    #[wasm_bindgen(js_name = "readRoomEvents")]
    pub async fn read_room_events(&self, room_id: &str) -> Result<String, JsValue> {
        let rid = <&matrix_sdk::ruma::RoomId>::try_from(room_id).map_err(to_js_err)?;

        if let Some(room) = self.client.get_room(rid) {
            let messages = room
                .messages(MessagesOptions::backward())
                .await
                .map_err(to_js_err)?;
            let mut collected: Vec<serde_json::Value> = Vec::new();
            for event in &messages.chunk {
                if let Ok(json) =
                    serde_json::from_str::<serde_json::Value>(event.raw().json().get())
                {
                    let event_type = json.get("type").and_then(|t| t.as_str());
                    if event_type != Some("m.room.encrypted")
                        && event_type != Some("m.room.encryption")
                        && event_type != Some("m.room.member")
                    {
                        collected.push(json);
                    }
                }
            }
            return serde_json::to_string(&collected).map_err(to_js_err);
        }

        Ok("[]".to_string())
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
                let messages = room
                    .messages(MessagesOptions::backward())
                    .await
                    .map_err(to_js_err)?;
                let mut collected: Vec<serde_json::Value> = Vec::new();
                let mut encrypted_count: u32 = 0;
                for event in &messages.chunk {
                    if let Ok(json) =
                        serde_json::from_str::<serde_json::Value>(event.raw().json().get())
                    {
                        let event_type = json.get("type").and_then(|t| t.as_str());
                        if event_type == Some("m.room.encrypted") {
                            encrypted_count += 1;
                        } else if event_type != Some("m.room.encryption")
                            && event_type != Some("m.room.member")
                        {
                            collected.push(json);
                        }
                    }
                }
                if encrypted_count > 0 {
                    web_sys::console::warn_1(&format!(
                        "[mxdx] {} encrypted event(s) in room {} could not be decrypted (missing Megolm keys)",
                        encrypted_count, room_id
                    ).into());
                }
                if !collected.is_empty() {
                    return serde_json::to_string(&collected).map_err(to_js_err);
                }
            }
        }

        Ok("[]".to_string())
    }

    /// Create a direct message room with E2EE and history_visibility: joined.
    /// Used for interactive terminal sessions — only participants who join see messages.
    #[wasm_bindgen(js_name = "createDmRoom")]
    pub async fn create_dm_room(&self, user_id: &str) -> Result<String, JsValue> {
        let uid: OwnedUserId = user_id
            .try_into()
            .map_err(|e| to_js_err(format!("Invalid user ID '{user_id}': {e}")))?;

        let encryption_event = InitialStateEvent::new(
            EmptyStateKey,
            RoomEncryptionEventContent::with_recommended_defaults(),
        );
        let history_event = InitialStateEvent::new(
            EmptyStateKey,
            RoomHistoryVisibilityEventContent::new(HistoryVisibility::Joined),
        );

        let mut request = CreateRoomRequest::new();
        request.is_direct = true;
        request.invite = vec![uid];
        request.initial_state = vec![encryption_event.to_raw_any(), history_event.to_raw_any()];

        let response = self.client.create_room(request).await.map_err(to_js_err)?;
        Ok(response.room_id().to_string())
    }

    /// Create a room with configurable options (topic, invites, preset).
    /// Always adds E2EE and history_visibility: joined.
    /// config_json: { "invite": ["@user:server"], "topic": "...", "preset": "trusted_private_chat", "is_direct": false }
    #[wasm_bindgen(js_name = "createRoom")]
    pub async fn create_room(&self, config_json: &str) -> Result<String, JsValue> {
        #[derive(Deserialize)]
        struct RoomConfig {
            #[serde(default)]
            invite: Vec<String>,
            #[serde(default)]
            topic: Option<String>,
            #[serde(default)]
            preset: Option<String>,
            #[serde(default)]
            is_direct: bool,
        }

        let config: RoomConfig = serde_json::from_str(config_json)
            .map_err(|e| to_js_err(format!("Invalid room config: {e}")))?;

        let encryption_event = InitialStateEvent::new(
            EmptyStateKey,
            RoomEncryptionEventContent::with_recommended_defaults(),
        );
        let history_event = InitialStateEvent::new(
            EmptyStateKey,
            RoomHistoryVisibilityEventContent::new(HistoryVisibility::Joined),
        );

        let mut initial_state = vec![encryption_event.to_raw_any(), history_event.to_raw_any()];

        // Add topic as initial state if provided
        if let Some(ref topic) = config.topic {
            let topic_event =
                InitialStateEvent::new(EmptyStateKey, RoomTopicEventContent::new(topic.clone()));
            initial_state.push(topic_event.to_raw_any());
        }

        let mut request = CreateRoomRequest::new();
        request.is_direct = config.is_direct;
        request.invite = config
            .invite
            .iter()
            .filter_map(|u| u.as_str().try_into().ok())
            .collect();
        request.initial_state = initial_state;

        // Handle preset
        if let Some(ref preset) = config.preset {
            use matrix_sdk::ruma::api::client::room::create_room::v3::RoomPreset;
            request.preset = match preset.as_str() {
                "trusted_private_chat" => Some(RoomPreset::TrustedPrivateChat),
                "private_chat" => Some(RoomPreset::PrivateChat),
                "public_chat" => Some(RoomPreset::PublicChat),
                _ => None,
            };
        }

        let response = self.client.create_room(request).await.map_err(to_js_err)?;
        Ok(response.room_id().to_string())
    }

    /// Search existing room history for events of a given type.
    /// Returns a JSON array of matching events (newest first), without affecting seen-event tracking.
    /// Use this for one-time lookups (e.g., finding ICE candidates that arrived before polling started).
    #[wasm_bindgen(js_name = "findRoomEvents")]
    pub async fn find_room_events(
        &self,
        room_id: &str,
        event_type: &str,
        limit: u32,
    ) -> Result<String, JsValue> {
        // Sync first to ensure we have the latest events
        self.sync_once().await?;

        let rid = <&matrix_sdk::ruma::RoomId>::try_from(room_id).map_err(to_js_err)?;
        if let Some(room) = self.client.get_room(rid) {
            let messages = room
                .messages(MessagesOptions::backward())
                .await
                .map_err(to_js_err)?;
            let mut results = Vec::new();
            for event in &messages.chunk {
                if let Ok(json) =
                    serde_json::from_str::<serde_json::Value>(event.raw().json().get())
                {
                    let etype = json.get("type").and_then(|t| t.as_str());
                    if etype == Some(event_type) {
                        results.push(json);
                        if results.len() >= limit as usize {
                            break;
                        }
                    }
                }
            }
            return serde_json::to_string(&results).map_err(to_js_err);
        }
        Ok("[]".to_string())
    }

    /// Sync and wait for a specific event type in a room.
    /// Returns event content as JSON string, or "null" if timeout.
    #[wasm_bindgen(js_name = "onRoomEvent")]
    pub async fn on_room_event(
        &self,
        room_id: &str,
        event_type: &str,
        timeout_secs: u32,
    ) -> Result<String, JsValue> {
        let rid = <&matrix_sdk::ruma::RoomId>::try_from(room_id).map_err(to_js_err)?;
        let timeout = Duration::from_secs(timeout_secs as u64);
        let deadline = web_time::Instant::now() + timeout;
        let mut seen_event_ids: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        // Collect already-seen event IDs on first pass
        if let Some(room) = self.client.get_room(rid) {
            if let Ok(messages) = room.messages(MessagesOptions::backward()).await {
                for event in &messages.chunk {
                    if let Ok(json) =
                        serde_json::from_str::<serde_json::Value>(event.raw().json().get())
                    {
                        if let Some(eid) = json.get("event_id").and_then(|e| e.as_str()) {
                            seen_event_ids.insert(eid.to_string());
                        }
                    }
                }
            }
        }

        while web_time::Instant::now() < deadline {
            self.sync_once().await?;

            if let Some(room) = self.client.get_room(rid) {
                let messages = room
                    .messages(MessagesOptions::backward())
                    .await
                    .map_err(to_js_err)?;
                let mut encrypted_count: u32 = 0;
                for event in &messages.chunk {
                    if let Ok(json) =
                        serde_json::from_str::<serde_json::Value>(event.raw().json().get())
                    {
                        let etype = json.get("type").and_then(|t| t.as_str());
                        let eid = json.get("event_id").and_then(|e| e.as_str()).unwrap_or("");

                        if etype == Some("m.room.encrypted") && !seen_event_ids.contains(eid) {
                            encrypted_count += 1;
                        }

                        if etype == Some(event_type) && !seen_event_ids.contains(eid) {
                            return serde_json::to_string(&json).map_err(to_js_err);
                        }
                    }
                }
                if encrypted_count > 0 {
                    web_sys::console::warn_1(
                        &format!(
                            "[mxdx] {} undecryptable event(s) in room {} while waiting for '{}'",
                            encrypted_count, room_id, event_type
                        )
                        .into(),
                    );
                }
            }
        }

        Ok("null".to_string())
    }
}

// Private helpers
impl WasmMatrixClient {
    async fn create_named_encrypted_room(
        &self,
        name: &str,
        topic: &str,
    ) -> Result<String, JsValue> {
        let encryption_event = InitialStateEvent::new(
            EmptyStateKey,
            RoomEncryptionEventContent::with_recommended_defaults().with_encrypted_state(),
        );
        let topic_event =
            InitialStateEvent::new(EmptyStateKey, RoomTopicEventContent::new(topic.to_string()));

        let mut request = CreateRoomRequest::new();
        request.name = Some(name.to_string());
        request.initial_state = vec![encryption_event.to_raw_any(), topic_event.to_raw_any()];

        let response = self.client.create_room(request).await.map_err(to_js_err)?;
        Ok(response.room_id().to_string())
    }
}

// === Unified Session Types ===

/// Create a SessionTask JSON string from parameters.
/// `timeout_seconds_js` accepts a JS number or null/undefined for None.
#[wasm_bindgen]
pub fn create_session_task(
    bin: &str,
    args: JsValue,
    interactive: bool,
    no_room_output: bool,
    timeout_seconds_js: JsValue,
    heartbeat_interval_seconds: u64,
    sender_id: &str,
) -> Result<String, JsValue> {
    let args_vec: Vec<String> = serde_wasm_bindgen::from_value(args).map_err(to_js_err)?;
    let timeout_seconds: Option<u64> = if timeout_seconds_js.is_null() || timeout_seconds_js.is_undefined() {
        None
    } else {
        Some(
            timeout_seconds_js
                .as_f64()
                .ok_or_else(|| to_js_err("timeout_seconds must be a number or null"))? as u64,
        )
    };
    let task = mxdx_types::events::session::SessionTask {
        uuid: uuid::Uuid::new_v4().to_string(),
        sender_id: sender_id.to_string(),
        bin: bin.to_string(),
        args: args_vec,
        env: None,
        cwd: None,
        interactive,
        no_room_output,
        timeout_seconds,
        heartbeat_interval_seconds,
        plan: None,
        required_capabilities: vec![],
        routing_mode: None,
        on_timeout: None,
        on_heartbeat_miss: None,
    };
    serde_json::to_string(&task).map_err(to_js_err)
}

/// Parse a SessionResult JSON string and return it (for JS consumption).
#[wasm_bindgen]
pub fn parse_session_result(json: &str) -> Result<String, JsValue> {
    let result: mxdx_types::events::session::SessionResult =
        serde_json::from_str(json).map_err(to_js_err)?;
    serde_json::to_string(&result).map_err(to_js_err)
}

/// Parse an ActiveSessionState JSON string.
#[wasm_bindgen]
pub fn parse_active_session(json: &str) -> Result<String, JsValue> {
    let state: mxdx_types::events::session::ActiveSessionState =
        serde_json::from_str(json).map_err(to_js_err)?;
    serde_json::to_string(&state).map_err(to_js_err)
}

/// Parse a CompletedSessionState JSON string.
#[wasm_bindgen]
pub fn parse_completed_session(json: &str) -> Result<String, JsValue> {
    let state: mxdx_types::events::session::CompletedSessionState =
        serde_json::from_str(json).map_err(to_js_err)?;
    serde_json::to_string(&state).map_err(to_js_err)
}

/// Parse a WorkerInfo JSON string.
#[wasm_bindgen]
pub fn parse_worker_info(json: &str) -> Result<String, JsValue> {
    let info: mxdx_types::events::worker_info::WorkerInfo =
        serde_json::from_str(json).map_err(to_js_err)?;
    serde_json::to_string(&info).map_err(to_js_err)
}

/// Get session event type constants as JSON.
#[wasm_bindgen]
pub fn session_event_types() -> String {
    serde_json::json!({
        "SESSION_TASK": mxdx_types::events::session::SESSION_TASK,
        "SESSION_START": mxdx_types::events::session::SESSION_START,
        "SESSION_OUTPUT": mxdx_types::events::session::SESSION_OUTPUT,
        "SESSION_HEARTBEAT": mxdx_types::events::session::SESSION_HEARTBEAT,
        "SESSION_RESULT": mxdx_types::events::session::SESSION_RESULT,
        "SESSION_INPUT": mxdx_types::events::session::SESSION_INPUT,
        "SESSION_SIGNAL": mxdx_types::events::session::SESSION_SIGNAL,
        "SESSION_RESIZE": mxdx_types::events::session::SESSION_RESIZE,
        "SESSION_CANCEL": mxdx_types::events::session::SESSION_CANCEL,
        "WORKER_INFO": mxdx_types::events::worker_info::WORKER_INFO,
    })
    .to_string()
}
