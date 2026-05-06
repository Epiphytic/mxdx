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
            EmptyStateKey, InitialStateEvent, StateEventType,
        },
        serde::Raw,
        OwnedUserId,
    },
    Client,
};

/// Custom field key inside `m.room.create` content identifying the launcher
/// this room belongs to. `m.room.create` is NEVER encrypted per Matrix spec —
/// it's the foundational state event that establishes the room — so this
/// field is the one place we can put discovery metadata that survives
/// MSC4362 encrypted state events.
const MXDX_LAUNCHER_ID_KEY: &str = "org.mxdx.launcher_id";

/// Custom field key inside `m.room.create` content identifying the role of
/// this room within the launcher topology (`space`, `exec`, or `logs`).
const MXDX_ROLE_KEY: &str = "org.mxdx.role";

/// Build a `Raw<CreationContent>` that embeds the mxdx discovery fields in
/// the room's `m.room.create` event. See the Rust worker's
/// `mxdx_creation_content_raw` helper in `mxdx-matrix/src/rooms.rs` for the
/// matching logic — both sides of the ecosystem must produce and consume
/// the same custom keys.
fn mxdx_creation_content_raw(
    launcher_id: &str,
    role: &str,
    is_space: bool,
) -> Raw<CreationContent> {
    let mut obj = serde_json::Map::new();
    if is_space {
        obj.insert(
            "type".to_string(),
            serde_json::Value::String("m.space".to_string()),
        );
    }
    obj.insert(
        MXDX_LAUNCHER_ID_KEY.to_string(),
        serde_json::Value::String(launcher_id.to_string()),
    );
    obj.insert(
        MXDX_ROLE_KEY.to_string(),
        serde_json::Value::String(role.to_string()),
    );
    let json_string = serde_json::to_string(&serde_json::Value::Object(obj))
        .expect("mxdx creation content serializes");
    Raw::from_json_string(json_string).expect("mxdx creation content is valid JSON")
}
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
///
/// Internal only — serialized as a JSON string via `serde_json`.
/// Never use `serde_wasm_bindgen::to_value` for this type: it returns
/// empty `{}` for nested `serde_json::Value` and arbitrary serde shapes
/// (project memory). Callers in JS must `JSON.parse(<await>)` the
/// returned string.
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
    ///
    /// Returns a JSON string `{ space_id, exec_room_id, logs_room_id }`.
    /// JS callers MUST `JSON.parse(...)` the returned value — the WASM layer
    /// returns a string, not a JS object, because `serde_wasm_bindgen::to_value`
    /// produces empty `{}` for `serde_json::Value`-shaped types (project memory).
    #[wasm_bindgen(js_name = "createLauncherSpace")]
    pub async fn create_launcher_space(&self, launcher_id: &str) -> Result<JsValue, JsValue> {
        let server_name = self
            .client
            .user_id()
            .ok_or_else(|| to_js_err("Not logged in"))?
            .server_name()
            .to_string();

        // Space room. The launcher_id + role marker are embedded in
        // `m.room.create` content (never encrypted by Matrix spec) so
        // discovery works even when every other state event — including
        // `m.space.child` linking the exec/logs rooms below — is encrypted
        // via MSC4362.
        //
        // Security (CLAUDE.md): every Matrix event in this project MUST be
        // E2EE on the wire, including state events like `m.space.child`.
        // The space room therefore needs:
        //   1. `m.room.encryption` (algorithm `m.megolm.v1.aes-sha2`) so
        //      timeline events are encrypted by Megolm.
        //   2. `with_encrypted_state()` (MSC4362
        //      `experimental-encrypted-state-events`) so state events such
        //      as `m.space.child` are encrypted on the wire.
        // Both are set the same way `create_named_encrypted_mxdx_room`
        // sets them for the exec/logs rooms.
        let space_encryption = InitialStateEvent::new(
            EmptyStateKey,
            RoomEncryptionEventContent::with_recommended_defaults().with_encrypted_state(),
        );
        let space_topic = InitialStateEvent::new(
            EmptyStateKey,
            RoomTopicEventContent::new(format!("org.mxdx.launcher.space:{launcher_id}")),
        );
        let mut space_request = CreateRoomRequest::new();
        space_request.name = Some(format!("mxdx: {launcher_id}"));
        space_request.creation_content =
            Some(mxdx_creation_content_raw(launcher_id, "space", true));
        space_request.initial_state =
            vec![space_encryption.to_raw_any(), space_topic.to_raw_any()];

        let space = self
            .client
            .create_room(space_request)
            .await
            .map_err(to_js_err)?;
        let space_id = space.room_id().to_string();

        // Create exec room (E2EE + MSC4362 + mxdx discovery fields)
        let exec_room_id = self
            .create_named_encrypted_mxdx_room(
                &format!("mxdx: {launcher_id} — exec"),
                &format!("org.mxdx.launcher.exec:{launcher_id}"),
                launcher_id,
                "exec",
            )
            .await?;

        // Create logs room (E2EE + MSC4362 + mxdx discovery fields)
        let logs_room_id = self
            .create_named_encrypted_mxdx_room(
                &format!("mxdx: {launcher_id} — logs"),
                &format!("org.mxdx.launcher.logs:{launcher_id}"),
                launcher_id,
                "logs",
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
        // Return a JSON string. JS must JSON.parse() it. See LauncherTopology
        // doc comment — serde_wasm_bindgen::to_value drops nested serde_json::Value
        // shapes silently.
        serde_json::to_string(&topology)
            .map_err(to_js_err)
            .map(|s| JsValue::from_str(&s))
    }

    /// Find an existing launcher space by scanning joined rooms for matching
    /// mxdx discovery metadata in their `m.room.create` content. That event
    /// is never encrypted per Matrix spec, so this works even when every
    /// other state event is MSC4362-encrypted and the client has not yet
    /// received the room keys needed to decrypt the topic.
    ///
    /// Returns a JSON-string topology `{space_id, exec_room_id, logs_room_id}`
    /// or JS `null` if no matching space exists.
    /// JS callers MUST `JSON.parse(...)` non-null returns.
    #[wasm_bindgen(js_name = "findLauncherSpace")]
    pub async fn find_launcher_space(&self, launcher_id: &str) -> Result<JsValue, JsValue> {
        self.sync_once().await?;

        let mut space_id: Option<String> = None;
        let mut exec_room_id: Option<String> = None;
        let mut logs_room_id: Option<String> = None;

        for room in self.client.joined_rooms() {
            let rid = room.room_id().to_string();

            // Read m.room.create (never encrypted). Parse the raw JSON so we
            // can access custom `org.mxdx.*` fields that the typed ruma
            // struct doesn't expose.
            let raw = match room
                .get_state_event(StateEventType::RoomCreate, "")
                .await
            {
                Ok(Some(raw)) => raw,
                _ => continue,
            };
            let json_str = match &raw {
                matrix_sdk::deserialized_responses::RawAnySyncOrStrippedState::Sync(r) => {
                    r.json().get().to_string()
                }
                matrix_sdk::deserialized_responses::RawAnySyncOrStrippedState::Stripped(r) => {
                    r.json().get().to_string()
                }
            };
            let value: serde_json::Value = match serde_json::from_str(&json_str) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let content = value.get("content").unwrap_or(&value);
            let lid = content.get(MXDX_LAUNCHER_ID_KEY).and_then(|v| v.as_str());
            if lid != Some(launcher_id) {
                continue;
            }
            let role = content.get(MXDX_ROLE_KEY).and_then(|v| v.as_str());
            match role {
                Some("space") if space_id.is_none() => space_id = Some(rid),
                Some("exec") if exec_room_id.is_none() => exec_room_id = Some(rid),
                Some("logs") if logs_room_id.is_none() => logs_room_id = Some(rid),
                _ => {}
            }
        }

        // Exec room is the minimum requirement; space and logs are optional
        match exec_room_id {
            Some(e) => {
                let topology = LauncherTopology {
                    space_id: space_id.unwrap_or_else(|| e.clone()),
                    exec_room_id: e.clone(),
                    logs_room_id: logs_room_id.unwrap_or_else(|| e),
                };
                // Return a JSON string. JS must JSON.parse() it. See
                // LauncherTopology doc comment.
                serde_json::to_string(&topology)
                    .map_err(to_js_err)
                    .map(|s| JsValue::from_str(&s))
            }
            None => Ok(JsValue::NULL),
        }
    }

    /// Find or create a launcher space (idempotent).
    ///
    /// Returns a JSON string `{ space_id, exec_room_id, logs_room_id }`.
    /// JS callers MUST `JSON.parse(...)` the returned value (see
    /// `LauncherTopology` doc comment).
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
    ///
    /// Uses the mxdx discovery fields in `m.room.create` (never encrypted)
    /// rather than reading the encrypted `m.room.topic` from state cache.
    #[wasm_bindgen(js_name = "listLauncherSpaces")]
    pub async fn list_launcher_spaces(&self) -> Result<String, JsValue> {
        let mut spaces: Vec<(String, String)> = Vec::new();
        let mut exec_rooms: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        let mut logs_rooms: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();

        for room in self.client.joined_rooms() {
            let rid = room.room_id().to_string();
            let raw = match room
                .get_state_event(StateEventType::RoomCreate, "")
                .await
            {
                Ok(Some(raw)) => raw,
                _ => continue,
            };
            let json_str = match &raw {
                matrix_sdk::deserialized_responses::RawAnySyncOrStrippedState::Sync(r) => {
                    r.json().get().to_string()
                }
                matrix_sdk::deserialized_responses::RawAnySyncOrStrippedState::Stripped(r) => {
                    r.json().get().to_string()
                }
            };
            let value: serde_json::Value = match serde_json::from_str(&json_str) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let content = value.get("content").unwrap_or(&value);
            let launcher_id = match content.get(MXDX_LAUNCHER_ID_KEY).and_then(|v| v.as_str()) {
                Some(s) => s.to_string(),
                None => continue,
            };
            let role = content.get(MXDX_ROLE_KEY).and_then(|v| v.as_str());
            match role {
                Some("space") => spaces.push((launcher_id, rid)),
                Some("exec") => {
                    exec_rooms.insert(launcher_id, rid);
                }
                Some("logs") => {
                    logs_rooms.insert(launcher_id, rid);
                }
                _ => {}
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
    /// MSC4362: state events MUST be encrypted (CLAUDE.md hard rule).
    #[wasm_bindgen(js_name = "createDmRoom")]
    pub async fn create_dm_room(&self, user_id: &str) -> Result<String, JsValue> {
        let uid: OwnedUserId = user_id
            .try_into()
            .map_err(|e| to_js_err(format!("Invalid user ID '{user_id}': {e}")))?;

        let encryption_event = InitialStateEvent::new(
            EmptyStateKey,
            RoomEncryptionEventContent::with_recommended_defaults().with_encrypted_state(),
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
    /// MSC4362: state events MUST be encrypted (CLAUDE.md hard rule).
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
            RoomEncryptionEventContent::with_recommended_defaults().with_encrypted_state(),
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

    // -----------------------------------------------------------------------
    // Interactive session methods
    // -----------------------------------------------------------------------

    /// Create an E2EE DM room for an interactive session.
    /// Returns the room_id as a string. The room has E2EE enabled and
    /// history_visibility: joined so only participants who join see messages.
    /// This is a thin wrapper around createDmRoom.
    #[wasm_bindgen(js_name = "createInteractiveSessionRoom")]
    pub async fn create_interactive_session_room(
        &self,
        client_user_id: &str,
    ) -> Result<String, JsValue> {
        self.create_dm_room(client_user_id).await
    }

    /// Send terminal input to a DM room for a specific session.
    #[wasm_bindgen(js_name = "sendTerminalInput")]
    pub async fn send_terminal_input(
        &self,
        dm_room_id: &str,
        session_id: &str,
        data: &str,
    ) -> Result<(), JsValue> {
        let content = serde_json::json!({
            "session_uuid": session_id,
            "data": data,
        });
        self.send_event(
            dm_room_id,
            "org.mxdx.session.input",
            &serde_json::to_string(&content).map_err(to_js_err)?,
        )
        .await
    }

    /// Send terminal resize to a DM room for a specific session.
    #[wasm_bindgen(js_name = "sendTerminalResize")]
    pub async fn send_terminal_resize(
        &self,
        dm_room_id: &str,
        session_id: &str,
        cols: u16,
        rows: u16,
    ) -> Result<(), JsValue> {
        let content = serde_json::json!({
            "session_uuid": session_id,
            "cols": cols,
            "rows": rows,
        });
        self.send_event(
            dm_room_id,
            "org.mxdx.session.resize",
            &serde_json::to_string(&content).map_err(to_js_err)?,
        )
        .await
    }

    /// Post terminal output to a DM room for a specific session.
    #[wasm_bindgen(js_name = "postTerminalOutput")]
    pub async fn post_terminal_output(
        &self,
        dm_room_id: &str,
        session_id: &str,
        data: &str,
        seq: u32,
    ) -> Result<(), JsValue> {
        let content = serde_json::json!({
            "session_uuid": session_id,
            "data": data,
            "seq": seq,
        });
        self.send_event(
            dm_room_id,
            "org.mxdx.session.output",
            &serde_json::to_string(&content).map_err(to_js_err)?,
        )
        .await
    }
}

// ---------------------------------------------------------------------------
// State Room WASM bindings
// ---------------------------------------------------------------------------
//
// These methods implement state room operations directly on `WasmMatrixClient`
// using `matrix-sdk` (not `mxdx-worker::state_room`) because the worker crate
// pulls in sqlite/tokio which are incompatible with WASM.

#[wasm_bindgen]
impl WasmMatrixClient {
    /// Get or create a worker state room. Returns the room ID as a string.
    /// Uses alias lookup first, falls back to topic scan, then creates a new
    /// E2EE room with a deterministic alias.
    #[wasm_bindgen(js_name = "getOrCreateStateRoom")]
    pub async fn get_or_create_state_room(
        &self,
        hostname: &str,
        os_user: &str,
        localpart: &str,
    ) -> Result<String, JsValue> {
        let server_name = self
            .client
            .user_id()
            .ok_or_else(|| to_js_err("Not logged in"))?
            .server_name()
            .to_string();

        let alias_localpart = format!("mxdx-state-{hostname}.{os_user}.{localpart}");
        let alias = format!("#{alias_localpart}:{server_name}");

        // Step 1: Try alias resolution via REST API
        let homeserver = self.client.homeserver().to_string();
        let encoded_alias = js_sys::encode_uri_component(&alias);
        let url = format!(
            "{}/_matrix/client/v3/directory/room/{}",
            homeserver.trim_end_matches('/'),
            encoded_alias
        );

        let http_client = reqwest::Client::new();
        let resp = http_client.get(&url).send().await;
        if let Ok(resp) = resp {
            if resp.status().is_success() {
                if let Ok(body) = resp.json::<serde_json::Value>().await {
                    if let Some(room_id) = body.get("room_id").and_then(|v| v.as_str()) {
                        // Ensure we've joined and synced this room
                        if let Err(e) = self.client.join_room_by_id(
                            <&matrix_sdk::ruma::RoomId>::try_from(room_id).map_err(to_js_err)?,
                        ).await {
                            web_sys::console::warn_1(
                                &format!("[mxdx] join state room warning: {e}").into(),
                            );
                        }
                        return Ok(room_id.to_string());
                    }
                }
            }
        }

        // Step 2: Fall back to topic scan
        self.sync_once().await?;
        let expected_topic =
            format!("org.mxdx.worker.state:{hostname}.{os_user}.{localpart}");
        for room in self.client.joined_rooms() {
            if room.topic().unwrap_or_default() == expected_topic {
                return Ok(room.room_id().to_string());
            }
        }

        // Step 3: Create new E2EE state room with alias
        // MSC4362: state events MUST be encrypted (CLAUDE.md hard rule).
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

        let response = self.client.create_room(request).await.map_err(to_js_err)?;
        Ok(response.room_id().to_string())
    }

    /// Write config to state room.
    #[wasm_bindgen(js_name = "writeStateRoomConfig")]
    pub async fn write_state_room_config(
        &self,
        room_id: &str,
        config_json: &str,
    ) -> Result<(), JsValue> {
        self.send_state_event(
            room_id,
            mxdx_types::events::state_room::WORKER_STATE_CONFIG,
            "",
            config_json,
        )
        .await
    }

    /// Read config from state room. Returns JSON string or null.
    #[wasm_bindgen(js_name = "readStateRoomConfig")]
    pub async fn read_state_room_config(&self, room_id: &str) -> Result<JsValue, JsValue> {
        self.read_single_state_event(
            room_id,
            mxdx_types::events::state_room::WORKER_STATE_CONFIG,
            "",
        )
        .await
    }

    /// Write a session entry. State key: {device_id}/{uuid}
    #[wasm_bindgen(js_name = "writeSession")]
    pub async fn write_session(
        &self,
        room_id: &str,
        device_id: &str,
        uuid: &str,
        session_json: &str,
    ) -> Result<(), JsValue> {
        let state_key = format!("{device_id}/{uuid}");
        self.send_state_event(
            room_id,
            mxdx_types::events::state_room::WORKER_STATE_SESSION,
            &state_key,
            session_json,
        )
        .await
    }

    /// Remove a session entry (writes empty content). State key: {device_id}/{uuid}
    #[wasm_bindgen(js_name = "removeSession")]
    pub async fn remove_session(
        &self,
        room_id: &str,
        device_id: &str,
        uuid: &str,
    ) -> Result<(), JsValue> {
        let state_key = format!("{device_id}/{uuid}");
        self.send_state_event(
            room_id,
            mxdx_types::events::state_room::WORKER_STATE_SESSION,
            &state_key,
            "{}",
        )
        .await
    }

    /// Read all sessions from state room. Returns JSON array string.
    #[wasm_bindgen(js_name = "readSessions")]
    pub async fn read_sessions(&self, room_id: &str) -> Result<String, JsValue> {
        self.read_all_state_events_of_type(
            room_id,
            mxdx_types::events::state_room::WORKER_STATE_SESSION,
        )
        .await
    }

    /// Write a tracked room entry. State key: room_id_key
    #[wasm_bindgen(js_name = "writeRoom")]
    pub async fn write_room(
        &self,
        room_id: &str,
        room_id_key: &str,
        entry_json: &str,
    ) -> Result<(), JsValue> {
        self.send_state_event(
            room_id,
            mxdx_types::events::state_room::WORKER_STATE_ROOM,
            room_id_key,
            entry_json,
        )
        .await
    }

    /// Read all tracked rooms. Returns JSON array string.
    #[wasm_bindgen(js_name = "readRooms")]
    pub async fn read_rooms(&self, room_id: &str) -> Result<String, JsValue> {
        self.read_all_state_events_of_type(
            room_id,
            mxdx_types::events::state_room::WORKER_STATE_ROOM,
        )
        .await
    }

    /// Write a trusted entity. entity_type is "client" or "coordinator".
    #[wasm_bindgen(js_name = "writeTrustedEntity")]
    pub async fn write_trusted_entity(
        &self,
        room_id: &str,
        entity_type: &str,
        user_id: &str,
        entity_json: &str,
    ) -> Result<(), JsValue> {
        let event_type = match entity_type {
            "client" => mxdx_types::events::state_room::WORKER_STATE_TRUSTED_CLIENT,
            "coordinator" => mxdx_types::events::state_room::WORKER_STATE_TRUSTED_COORDINATOR,
            _ => return Err(to_js_err(format!("Unknown entity type: {entity_type}"))),
        };
        self.send_state_event(room_id, event_type, user_id, entity_json)
            .await
    }

    /// Read trusted entities. Returns JSON array string.
    #[wasm_bindgen(js_name = "readTrustedEntities")]
    pub async fn read_trusted_entities(
        &self,
        room_id: &str,
        entity_type: &str,
    ) -> Result<String, JsValue> {
        let event_type = match entity_type {
            "client" => mxdx_types::events::state_room::WORKER_STATE_TRUSTED_CLIENT,
            "coordinator" => mxdx_types::events::state_room::WORKER_STATE_TRUSTED_COORDINATOR,
            _ => return Err(to_js_err(format!("Unknown entity type: {entity_type}"))),
        };
        self.read_all_state_events_of_type(room_id, event_type)
            .await
    }

    /// Write topology to state room.
    #[wasm_bindgen(js_name = "writeTopology")]
    pub async fn write_topology(
        &self,
        room_id: &str,
        topology_json: &str,
    ) -> Result<(), JsValue> {
        self.send_state_event(
            room_id,
            mxdx_types::events::state_room::WORKER_STATE_TOPOLOGY,
            "",
            topology_json,
        )
        .await
    }

    /// Read topology from state room. Returns JSON string or null.
    #[wasm_bindgen(js_name = "readTopology")]
    pub async fn read_topology(&self, room_id: &str) -> Result<JsValue, JsValue> {
        self.read_single_state_event(
            room_id,
            mxdx_types::events::state_room::WORKER_STATE_TOPOLOGY,
            "",
        )
        .await
    }

    /// Get state room event type constants as JSON.
    #[wasm_bindgen(js_name = "stateRoomEventTypes")]
    pub fn state_room_event_types() -> String {
        serde_json::json!({
            "WORKER_STATE_CONFIG": mxdx_types::events::state_room::WORKER_STATE_CONFIG,
            "WORKER_STATE_IDENTITY": mxdx_types::events::state_room::WORKER_STATE_IDENTITY,
            "WORKER_STATE_ROOM": mxdx_types::events::state_room::WORKER_STATE_ROOM,
            "WORKER_STATE_SESSION": mxdx_types::events::state_room::WORKER_STATE_SESSION,
            "WORKER_STATE_TOPOLOGY": mxdx_types::events::state_room::WORKER_STATE_TOPOLOGY,
            "WORKER_STATE_ROOM_POINTER": mxdx_types::events::state_room::WORKER_STATE_ROOM_POINTER,
            "WORKER_STATE_TRUSTED_CLIENT": mxdx_types::events::state_room::WORKER_STATE_TRUSTED_CLIENT,
            "WORKER_STATE_TRUSTED_COORDINATOR": mxdx_types::events::state_room::WORKER_STATE_TRUSTED_COORDINATOR,
        })
        .to_string()
    }
}

// Private helpers for state room operations
impl WasmMatrixClient {
    /// Read a single state event by type and state key.
    /// Returns JsValue::NULL if not found, or a JSON string JsValue if found.
    async fn read_single_state_event(
        &self,
        room_id: &str,
        event_type: &str,
        state_key: &str,
    ) -> Result<JsValue, JsValue> {
        let homeserver = self.client.homeserver().to_string();
        let token = self
            .client
            .matrix_auth()
            .session()
            .map(|s| s.tokens.access_token.clone())
            .ok_or_else(|| to_js_err("No active session"))?;

        let url = format!(
            "{}/_matrix/client/v3/rooms/{}/state/{}/{}",
            homeserver.trim_end_matches('/'),
            js_sys::encode_uri_component(room_id),
            js_sys::encode_uri_component(event_type),
            js_sys::encode_uri_component(state_key),
        );

        let http_client = reqwest::Client::new();
        let resp = http_client
            .get(&url)
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await
            .map_err(|e| to_js_err(format!("State event fetch failed: {e}")))?;

        if !resp.status().is_success() {
            return Ok(JsValue::NULL);
        }

        let body = resp.text().await.map_err(to_js_err)?;

        // Check for errcode or empty content
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&body) {
            if val.get("errcode").is_some() {
                return Ok(JsValue::NULL);
            }
            if val.is_object() && val.as_object().map(|o| o.is_empty()).unwrap_or(false) {
                return Ok(JsValue::NULL);
            }
        }

        Ok(JsValue::from_str(&body))
    }

    /// Read all state events of a given type from a room.
    /// Returns a JSON array string, filtering out empty content (removed entries).
    async fn read_all_state_events_of_type(
        &self,
        room_id: &str,
        event_type: &str,
    ) -> Result<String, JsValue> {
        let homeserver = self.client.homeserver().to_string();
        let token = self
            .client
            .matrix_auth()
            .session()
            .map(|s| s.tokens.access_token.clone())
            .ok_or_else(|| to_js_err("No active session"))?;

        // GET /_matrix/client/v3/rooms/{roomId}/state returns all state events
        let url = format!(
            "{}/_matrix/client/v3/rooms/{}/state",
            homeserver.trim_end_matches('/'),
            js_sys::encode_uri_component(room_id),
        );

        let http_client = reqwest::Client::new();
        let resp = http_client
            .get(&url)
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await
            .map_err(|e| to_js_err(format!("State fetch failed: {e}")))?;

        if !resp.status().is_success() {
            return Ok("[]".to_string());
        }

        let all_events: Vec<serde_json::Value> =
            resp.json().await.map_err(to_js_err)?;

        let mut results: Vec<serde_json::Value> = Vec::new();
        for event in all_events {
            let etype = event.get("type").and_then(|t| t.as_str()).unwrap_or("");
            if etype != event_type {
                continue;
            }
            if let Some(content) = event.get("content") {
                // Skip empty content (removed entries)
                if content.is_object()
                    && content.as_object().map(|o| o.is_empty()).unwrap_or(false)
                {
                    continue;
                }
                if content.is_null() {
                    continue;
                }
                results.push(content.clone());
            }
        }

        serde_json::to_string(&results).map_err(to_js_err)
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

    /// Create an E2EE+MSC4362 room that also carries the mxdx launcher_id
    /// and role in its `m.room.create` content. These fields remain readable
    /// via plain REST even when every other state event is encrypted, and
    /// form the discovery mechanism the Rust worker uses.
    async fn create_named_encrypted_mxdx_room(
        &self,
        name: &str,
        topic: &str,
        launcher_id: &str,
        role: &str,
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
        request.creation_content =
            Some(mxdx_creation_content_raw(launcher_id, role, false));

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

// ===========================================================================
// P2P surface (Phase 8 — T-81)
//
// Re-exports P2PCrypto (AES-256-GCM), signaling event helpers, and TURN
// credential fetching from mxdx-p2p via wasm_bindgen. The browser-side
// WebRtcChannel (web-sys) is used by P2PTransport internally but not
// directly exported — JS callers use the higher-level P2PTransport API.
//
// Design note: mxdx-p2p types use Rust patterns (Result, Bytes, async-trait)
// that don't map 1:1 to JS. These wasm_bindgen wrappers return JSON strings
// following the project convention: "return JSON strings from WASM and
// JSON.parse() in JS instead" (from MEMORY.md). This avoids the
// serde_wasm_bindgen::to_value failure on serde_json::Value.
// ===========================================================================

/// WASM wrapper around `mxdx_p2p::crypto::P2PCrypto`.
///
/// Exposes key generation, encrypt, and decrypt — wire-compatible with the
/// npm `P2PCrypto` class in `packages/core/p2p-crypto.js`.
#[wasm_bindgen]
pub struct P2PCrypto {
    inner: mxdx_p2p::crypto::P2PCrypto,
}

#[wasm_bindgen]
impl P2PCrypto {
    /// Generate a new random AES-256-GCM session key.
    /// Returns the base64-encoded key string for embedding in signaling events.
    #[wasm_bindgen(js_name = "generate")]
    pub fn generate() -> Result<P2PCryptoWithKey, JsValue> {
        let (crypto, sealed) = mxdx_p2p::crypto::P2PCrypto::generate();
        let key_b64 = sealed.to_base64();
        Ok(P2PCryptoWithKey {
            crypto: P2PCrypto { inner: crypto },
            key: key_b64,
        })
    }

    /// Create a P2PCrypto instance from a base64-encoded session key.
    #[wasm_bindgen(js_name = "fromKey")]
    pub fn from_key(base64_key: &str) -> Result<P2PCrypto, JsValue> {
        let sealed = mxdx_p2p::crypto::SealedKey::from_base64(base64_key)
            .map_err(|e| to_js_err(format!("invalid session key: {e}")))?;
        let crypto = mxdx_p2p::crypto::P2PCrypto::from_sealed(sealed);
        Ok(P2PCrypto { inner: crypto })
    }

    /// Encrypt a plaintext string. Returns JSON: `{"c":"<base64>","iv":"<base64>"}`.
    #[wasm_bindgen]
    pub fn encrypt(&self, plaintext: &str) -> Result<String, JsValue> {
        let frame = self
            .inner
            .encrypt(plaintext.as_bytes())
            .map_err(|e| to_js_err(format!("encrypt: {e}")))?;
        serde_json::to_string(&frame).map_err(|e| to_js_err(format!("serialize: {e}")))
    }

    /// Decrypt a JSON frame string. Returns the plaintext string, or throws on failure.
    #[wasm_bindgen]
    pub fn decrypt(&self, ciphertext_json: &str) -> Result<String, JsValue> {
        let frame: mxdx_p2p::crypto::EncryptedFrame =
            serde_json::from_str(ciphertext_json)
                .map_err(|e| to_js_err(format!("parse frame: {e}")))?;
        let plaintext = self
            .inner
            .decrypt(&frame)
            .map_err(|e| to_js_err(format!("decrypt: {e}")))?;
        String::from_utf8(plaintext)
            .map_err(|e| to_js_err(format!("utf8: {e}")))
    }
}

/// Result of `P2PCrypto.generate()` — contains both the crypto instance and
/// the base64-encoded key for signaling. JS destructures this.
#[wasm_bindgen]
pub struct P2PCryptoWithKey {
    crypto: P2PCrypto,
    key: String,
}

#[wasm_bindgen]
impl P2PCryptoWithKey {
    /// Get the P2PCrypto instance.
    #[wasm_bindgen(getter)]
    pub fn crypto(self) -> P2PCrypto {
        self.crypto
    }

    /// Get the base64-encoded session key for embedding in m.call.invite.
    #[wasm_bindgen(getter)]
    pub fn key(&self) -> String {
        self.key.clone()
    }
}

/// Generate a random AES-256-GCM session key, returning just the base64 string.
/// Convenience wrapper matching `generateSessionKey()` in p2p-crypto.js.
#[wasm_bindgen(js_name = "generateSessionKey")]
pub fn generate_session_key() -> String {
    let (_crypto, sealed) = mxdx_p2p::crypto::P2PCrypto::generate();
    sealed.to_base64()
}

/// Create a P2PCrypto instance from a base64 key. Convenience wrapper matching
/// `createP2PCrypto()` in p2p-crypto.js.
#[wasm_bindgen(js_name = "createP2PCrypto")]
pub fn create_p2p_crypto(base64_key: &str) -> Result<P2PCrypto, JsValue> {
    P2PCrypto::from_key(base64_key)
}

/// Fetch TURN credentials from a Matrix homeserver.
/// Returns JSON string of the TURN response, or "null" if unavailable.
///
/// Uses the browser's `fetch()` via reqwest's wasm backend. Mirrors the
/// security checks in `packages/core/turn-credentials.js`: only https or
/// loopback http, graceful fallback to null.
#[wasm_bindgen(js_name = "fetchTurnCredentials")]
pub async fn fetch_turn_credentials(
    homeserver_url: &str,
    access_token: &str,
) -> Result<String, JsValue> {
    // Validate URL scheme — only https or loopback http allowed.
    // Use reqwest::Url which is re-exported from the `url` crate.
    let parsed = reqwest::Url::parse(homeserver_url)
        .map_err(|e| to_js_err(format!("invalid homeserver URL: {e}")))?;

    let loopback = ["localhost", "127.0.0.1", "::1", "[::1]"];
    match parsed.scheme() {
        "https" => {}
        "http" if loopback.contains(&parsed.host_str().unwrap_or("")) => {}
        _ => return Ok("null".to_string()),
    }

    let url = format!(
        "{}/_matrix/client/v3/voip/turnServer",
        homeserver_url.trim_end_matches('/')
    );

    let client = reqwest::Client::new();

    let resp = match client
        .get(&url)
        .header("Authorization", format!("Bearer {access_token}"))
        .send()
        .await
    {
        Ok(r) => r,
        Err(_) => return Ok("null".to_string()),
    };

    if !resp.status().is_success() {
        return Ok("null".to_string());
    }

    match resp.text().await {
        Ok(body) => {
            // Validate it has uris
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&body) {
                if val.get("uris").and_then(|u| u.as_array()).map(|a| a.is_empty()).unwrap_or(true) {
                    return Ok("null".to_string());
                }
            }
            Ok(body)
        }
        Err(_) => Ok("null".to_string()),
    }
}

/// Convert a TURN credentials JSON response to RTCPeerConnection iceServers format.
/// Returns JSON: `[{"urls":[...],"username":"...","credential":"..."}]`
/// Mirrors `turnToIceServers()` in `packages/core/turn-credentials.js`.
#[wasm_bindgen(js_name = "turnToIceServers")]
pub fn turn_to_ice_servers(turn_response_json: &str) -> Result<String, JsValue> {
    let val: serde_json::Value = serde_json::from_str(turn_response_json)
        .map_err(|e| to_js_err(format!("parse TURN response: {e}")))?;
    let uris = val.get("uris").and_then(|u| u.as_array());
    if uris.map(|a| a.is_empty()).unwrap_or(true) {
        return Ok("[]".to_string());
    }
    let result = serde_json::json!([{
        "urls": val["uris"],
        "username": val["username"],
        "credential": val["password"],
    }]);
    serde_json::to_string(&result).map_err(|e| to_js_err(format!("serialize: {e}")))
}

// ── Batched terminal sender ──────────────────────────────────────────────────

use flate2::{write::ZlibEncoder, Compression};
use std::io::Write as _;

const COMPRESSION_THRESHOLD: usize = 32;
// Zlib bomb protection — 1MB max decompressed size, matching JS MAX_DECOMPRESSED_SIZE.
// JS equivalent: packages/launcher/src/runtime.js MAX_DECOMPRESSED_SIZE = 1048576
const MAX_DECOMPRESSED_SIZE: usize = 1024 * 1024;

/// Compress data for a terminal.data Matrix event payload.
///
/// Returns `(encoded_base64, encoding_str)` where `encoding_str` is either
/// `"zlib+base64"` (for payloads >= 32 bytes) or `"base64"` (for smaller payloads).
///
/// Rust equivalent: packages/core/batched-sender.js::defaultCompress
fn compress_terminal_data(data: &[u8]) -> (String, &'static str) {
    if data.len() < COMPRESSION_THRESHOLD {
        return (base64_encode(data), "base64");
    }
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    if encoder.write_all(data).is_ok() {
        if let Ok(compressed) = encoder.finish() {
            return (base64_encode(&compressed), "zlib+base64");
        }
    }
    // Fallback to uncompressed if zlib fails
    (base64_encode(data), "base64")
}

fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = Vec::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as usize;
        let b1 = chunk.get(1).copied().unwrap_or(0) as usize;
        let b2 = chunk.get(2).copied().unwrap_or(0) as usize;
        let n = (b0 << 16) | (b1 << 8) | b2;
        result.push(CHARS[(n >> 18) & 63]);
        result.push(CHARS[(n >> 12) & 63]);
        result.push(if chunk.len() > 1 { CHARS[(n >> 6) & 63] } else { b'=' });
        result.push(if chunk.len() > 2 { CHARS[n & 63] } else { b'=' });
    }
    String::from_utf8(result).expect("base64 is always valid utf8")
}

fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    const TABLE: [i8; 256] = {
        let mut t = [-1i8; 256];
        let chars = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut i = 0usize;
        while i < 64 {
            t[chars[i] as usize] = i as i8;
            i += 1;
        }
        t
    };
    let s = s.trim_end_matches('=');
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let bytes = s.as_bytes();
    let mut i = 0;
    while i + 3 < bytes.len() {
        let (a, b, c, d) = (TABLE[bytes[i] as usize], TABLE[bytes[i+1] as usize], TABLE[bytes[i+2] as usize], TABLE[bytes[i+3] as usize]);
        if a < 0 || b < 0 || c < 0 || d < 0 { return Err("invalid base64".to_string()); }
        let n = ((a as u32) << 18) | ((b as u32) << 12) | ((c as u32) << 6) | (d as u32);
        out.push((n >> 16) as u8);
        out.push((n >> 8) as u8);
        out.push(n as u8);
        i += 4;
    }
    match bytes.len() - i {
        2 => {
            let (a, b) = (TABLE[bytes[i] as usize], TABLE[bytes[i+1] as usize]);
            if a < 0 || b < 0 { return Err("invalid base64".to_string()); }
            let n = ((a as u32) << 2) | ((b as u32) >> 4);
            out.push(n as u8);
        }
        3 => {
            let (a, b, c) = (TABLE[bytes[i] as usize], TABLE[bytes[i+1] as usize], TABLE[bytes[i+2] as usize]);
            if a < 0 || b < 0 || c < 0 { return Err("invalid base64".to_string()); }
            let n = ((a as u32) << 10) | ((b as u32) << 4) | ((c as u32) >> 2);
            out.push((n >> 8) as u8);
            out.push(n as u8);
        }
        _ => {}
    }
    Ok(out)
}

/// Process incoming terminal input data from a Matrix terminal.data event.
///
/// Accepts the base64-encoded data string and encoding type from the event content.
/// Returns the decoded raw bytes as a Uint8Array, applying zlib decompression
/// when encoding is "zlib+base64". Enforces 1MB decompression limit (zlib bomb protection).
///
/// Rust equivalent: packages/launcher/src/runtime.js::SessionMux.#processInput
#[wasm_bindgen(js_name = "processTerminalInput")]
pub fn process_terminal_input(data_b64: &str, encoding: &str) -> Result<Box<[u8]>, JsValue> {
    let raw = base64_decode(data_b64)
        .map_err(|e| to_js_err(format!("base64 decode: {e}")))?;

    if encoding == "zlib+base64" {
        use flate2::read::ZlibDecoder;
        use std::io::Read;
        let decoder = ZlibDecoder::new(raw.as_slice());
        let mut decompressed = Vec::new();
        // Read with limit to prevent zlib bomb — stop at MAX_DECOMPRESSED_SIZE + 1
        let mut limited = decoder.take((MAX_DECOMPRESSED_SIZE + 1) as u64);
        limited.read_to_end(&mut decompressed)
            .map_err(|e| to_js_err(format!("zlib decompress: {e}")))?;
        if decompressed.len() > MAX_DECOMPRESSED_SIZE {
            return Err(to_js_err("decompressed data exceeds 1MB limit"));
        }
        Ok(decompressed.into_boxed_slice())
    } else {
        Ok(raw.into_boxed_slice())
    }
}

/// Compress terminal PTY data for a Matrix terminal.data event.
///
/// Accepts raw byte data and returns a JSON string:
/// `{"data": "<base64>", "encoding": "zlib+base64"|"base64"}`.
///
/// Payloads >= 32 bytes are zlib-compressed. Smaller payloads are sent as plain base64.
///
/// Rust equivalent: packages/core/batched-sender.js::defaultCompress
#[wasm_bindgen(js_name = "compressTerminalData")]
pub fn compress_terminal_data_wasm(data: &[u8]) -> Result<String, JsValue> {
    let (encoded, encoding) = compress_terminal_data(data);
    serde_json::to_string(&serde_json::json!({ "data": encoded, "encoding": encoding }))
        .map_err(|e| to_js_err(format!("serialize: {e}")))
}

/// WASM-side batched terminal sender — full Rust replacement for
/// `packages/core/batched-sender.js::BatchedSender`.
///
/// Manages a buffer of raw PTY byte chunks, concatenates and compresses them,
/// emits a ready-to-send payload as a JSON string, and absorbs Matrix
/// `M_LIMIT_EXCEEDED` / 429 rate-limit errors with retry+coalesce semantics
/// identical to the JS implementation it replaces.
///
/// ## Threading model
///
/// JS owns the actual Matrix `sendEvent` call (the E2EE send path stays in
/// `WasmMatrixClient`) and the wall-clock timing (`setTimeout`). WASM owns:
///   - PTY chunk buffering
///   - zlib + base64 compression
///   - `seq` numbering
///   - 429 retry-with-coalesce: when the previous send was rate-limited, the
///     unsent payload is kept and re-emitted with any newly-buffered chunks
///     concatenated, mirroring `BatchedSender.#drain`'s coalescing behavior.
///
/// ## JS driver contract
///
/// 1. `push(bytes)` — for every PTY chunk.
/// 2. After a `batchMs` window, call `takePayload()`:
///    - returns `null` if nothing to send;
///    - returns a JSON string `{"data","encoding","seq","session_id"?}` to send.
/// 3. JS calls `await client.sendEvent(roomId(), eventType(), payload)`.
///    On success: call `markSent()`.
///    On `429` / `M_LIMIT_EXCEEDED`: call `markRateLimited()`, then
///    `WasmBatchedSender.parseRetryAfterMs(errString)` to get the wait
///    duration, `await new Promise(r => setTimeout(r, ms))`, and loop back
///    to step 2. Newly-buffered data will coalesce into the retry.
///    On other error: call `markError()` and surface to caller (same
///    drop-and-report behavior as the JS `onError` callback).
/// 4. `flushFinal()` — drain on shutdown.
///
/// ## Why a state-machine API and not async sleep
///
/// The project's WASM target does not currently depend on `gloo-timers`;
/// adding it pulls a futures crate into the WASM binary for what is
/// strictly a JS-side concern (the runtime already manages timers for
/// every other reason). Returning structured state lets JS own timing
/// while WASM owns the security-critical compression + sequencing. This
/// is the "structured retry-action" pattern called out in the P0-2
/// migration plan as the no-`gloo-timers` fallback.
///
/// Rust equivalent of: packages/core/batched-sender.js::BatchedSender
#[wasm_bindgen]
pub struct WasmBatchedSender {
    room_id: String,
    event_type: String,
    session_id: Option<String>,
    seq: u32,
    /// New raw PTY chunks awaiting compression.
    buffer: Vec<Vec<u8>>,
    /// Last payload returned to JS that has not yet been confirmed sent.
    /// On `markSent`, cleared. On `markRateLimited`, retained — next
    /// `takePayload()` will recompress it together with any newly-buffered
    /// chunks (coalesce).
    in_flight: Option<InFlightPayload>,
    /// True when the last attempt was rate-limited; used to drive an
    /// `onBuffering(true)` notification from JS.
    rate_limited: bool,
}

/// In-flight payload bookkeeping. Stores the *raw* bytes (pre-compression)
/// so a retry can re-coalesce + recompress them with new data without
/// re-decoding the compressed form.
struct InFlightPayload {
    raw: Vec<u8>,
    seq: u32,
}

#[wasm_bindgen]
impl WasmBatchedSender {
    /// Create a new WasmBatchedSender.
    ///
    /// # Arguments
    /// - `room_id`: Matrix room ID for the terminal session
    /// - `event_type`: Matrix event type (default: `org.mxdx.terminal.data`)
    /// - `session_id`: Optional session ID for room multiplexing
    #[wasm_bindgen(constructor)]
    pub fn new(room_id: &str, event_type: Option<String>, session_id: Option<String>) -> Self {
        WasmBatchedSender {
            room_id: room_id.to_string(),
            event_type: event_type.unwrap_or_else(|| "org.mxdx.terminal.data".to_string()),
            session_id,
            seq: 0,
            buffer: Vec::new(),
            in_flight: None,
            rate_limited: false,
        }
    }

    /// Push raw PTY bytes into the buffer.
    #[wasm_bindgen]
    pub fn push(&mut self, data: &[u8]) {
        self.buffer.push(data.to_vec());
    }

    /// Take the next payload to send.
    ///
    /// Returns:
    ///   - `null` if there is nothing to send (no buffered data and no in-flight retry).
    ///   - JSON string `{"data","encoding","seq","session_id"?}` otherwise.
    ///
    /// After a successful Matrix send, JS MUST call `markSent()`.
    /// After a 429 / `M_LIMIT_EXCEEDED`, JS MUST call `markRateLimited()`.
    /// On any other error, JS MUST call `markError()`.
    /// Failure to call any of those three keeps the payload in-flight; a
    /// follow-up `takePayload()` will return the same payload, plus any
    /// new buffered data coalesced in.
    #[wasm_bindgen(js_name = "takePayload")]
    pub fn take_payload(&mut self) -> Result<JsValue, JsValue> {
        // Combine: in-flight raw (if any, from prior 429) + everything in buffer.
        let mut combined: Vec<u8> = Vec::new();
        let chosen_seq;

        match self.in_flight.take() {
            Some(prev) => {
                combined.extend_from_slice(&prev.raw);
                for chunk in self.buffer.drain(..) {
                    combined.extend_from_slice(&chunk);
                }
                // Reuse the previous seq on retry — this matches the JS
                // implementation, which keeps the same seq when coalescing.
                // (Strictly the JS does `lastSeq = q[q.length-1].seq` after
                // coalescing, which for a 1-element queue is the original.)
                chosen_seq = prev.seq;
            }
            None => {
                if self.buffer.is_empty() {
                    return Ok(JsValue::NULL);
                }
                for chunk in self.buffer.drain(..) {
                    combined.extend_from_slice(&chunk);
                }
                chosen_seq = self.seq;
                self.seq += 1;
            }
        }

        if combined.is_empty() {
            return Ok(JsValue::NULL);
        }

        // Stash the raw bytes as the new in-flight; if JS reports rate-limit,
        // the next takePayload will coalesce with new data using these bytes.
        self.in_flight = Some(InFlightPayload {
            raw: combined.clone(),
            seq: chosen_seq,
        });

        let (encoded, encoding) = compress_terminal_data(&combined);
        let mut payload = serde_json::json!({
            "data": encoded,
            "encoding": encoding,
            "seq": chosen_seq,
        });
        if let Some(sid) = &self.session_id {
            payload["session_id"] = serde_json::Value::String(sid.clone());
        }
        let json = serde_json::to_string(&payload)
            .map_err(|e| to_js_err(format!("serialize payload: {e}")))?;
        Ok(JsValue::from_str(&json))
    }

    /// Confirm the last payload from `takePayload()` was sent successfully.
    /// Clears the in-flight state and the rate-limited flag.
    #[wasm_bindgen(js_name = "markSent")]
    pub fn mark_sent(&mut self) {
        self.in_flight = None;
        self.rate_limited = false;
    }

    /// Mark the last payload as rate-limited.
    /// The in-flight payload is retained so the next `takePayload()` will
    /// re-emit it (with any newly-buffered data coalesced in).
    /// Sets `isRateLimited()` to true; JS uses this to drive its
    /// `onBuffering(true)` notification.
    #[wasm_bindgen(js_name = "markRateLimited")]
    pub fn mark_rate_limited(&mut self) {
        self.rate_limited = true;
        // in_flight is intentionally retained.
    }

    /// Mark the last payload as having failed with a non-retryable error.
    /// Drops the in-flight payload and clears the rate-limited flag.
    /// JS callers should report this to their `onError` callback.
    #[wasm_bindgen(js_name = "markError")]
    pub fn mark_error(&mut self) {
        self.in_flight = None;
        self.rate_limited = false;
    }

    /// True if the most recent send attempt was rate-limited.
    /// Used by the JS thin wrapper to drive `onBuffering(true)` exactly once.
    #[wasm_bindgen(getter, js_name = "isRateLimited")]
    pub fn is_rate_limited(&self) -> bool {
        self.rate_limited
    }

    /// Parse a Matrix error string for `retry_after_ms` and return that value
    /// plus a 100ms safety margin (matching the JS implementation). Returns
    /// 2000 (the JS fallback) if no `retry_after_ms` field is found.
    ///
    /// This is exposed as a static (associated) function so JS can call it
    /// without holding a sender instance.
    #[wasm_bindgen(js_name = "parseRetryAfterMs")]
    pub fn parse_retry_after_ms(err: &str) -> u32 {
        // Mirror the regex `/retry_after_ms["\s:]+(\d+)/` from
        // packages/core/batched-sender.js. We do a manual scan rather than
        // pulling in `regex` (which inflates the WASM binary).
        let needle = "retry_after_ms";
        let bytes = err.as_bytes();
        let n = needle.len();
        if bytes.len() < n {
            return 2000;
        }
        let mut i = 0;
        while i + n <= bytes.len() {
            if &bytes[i..i + n] == needle.as_bytes() {
                // skip needle, then any of `"`, whitespace, `:`
                let mut j = i + n;
                while j < bytes.len() {
                    let c = bytes[j];
                    if c == b'"' || c == b':' || c == b' ' || c == b'\t' || c == b'\n' || c == b'\r' {
                        j += 1;
                    } else {
                        break;
                    }
                }
                let start = j;
                while j < bytes.len() && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                if j > start {
                    let num_str = &err[start..j];
                    if let Ok(n) = num_str.parse::<u32>() {
                        return n.saturating_add(100);
                    }
                }
                return 2000;
            }
            i += 1;
        }
        2000
    }

    /// Flush the buffer immediately (single-shot, ignores 429 retry semantics).
    ///
    /// **Prefer the `takePayload` / `markSent` / `markRateLimited` /
    /// `markError` cycle for the launcher hot path** — it implements the
    /// full BatchedSender state machine. This method is preserved for
    /// callers (and tests) that just want a single compressed payload
    /// without rate-limit handling.
    ///
    /// Returns `null` (JS null) if the buffer is empty.
    /// Returns a JSON string `{"data","encoding","seq","session_id"?}` on success.
    /// The room_id and event_type are available via `roomId()` and `eventType()` getters.
    #[wasm_bindgen]
    pub fn flush(&mut self) -> Result<JsValue, JsValue> {
        if self.buffer.is_empty() {
            return Ok(JsValue::NULL);
        }
        let combined: Vec<u8> = self.buffer.iter().flatten().copied().collect();
        self.buffer.clear();
        let seq = self.seq;
        self.seq += 1;

        let (encoded, encoding) = compress_terminal_data(&combined);
        let mut payload = serde_json::json!({
            "data": encoded,
            "encoding": encoding,
            "seq": seq,
        });
        if let Some(sid) = &self.session_id {
            payload["session_id"] = serde_json::Value::String(sid.clone());
        }
        let json = serde_json::to_string(&payload)
            .map_err(|e| to_js_err(format!("serialize payload: {e}")))?;
        Ok(JsValue::from_str(&json))
    }

    /// Room ID this sender targets.
    #[wasm_bindgen(getter, js_name = "roomId")]
    pub fn room_id(&self) -> String {
        self.room_id.clone()
    }

    /// Event type this sender emits.
    #[wasm_bindgen(getter, js_name = "eventType")]
    pub fn event_type(&self) -> String {
        self.event_type.clone()
    }

    /// Current sequence number (next seq that will be used on flush).
    #[wasm_bindgen(getter)]
    pub fn seq(&self) -> u32 {
        self.seq
    }

    /// Number of buffered byte chunks awaiting flush.
    #[wasm_bindgen(getter, js_name = "bufferLength")]
    pub fn buffer_length(&self) -> usize {
        self.buffer.len()
    }

    /// True iff a payload is currently in-flight (returned by `takePayload`
    /// but not yet acknowledged via `markSent`/`markError`).
    #[wasm_bindgen(getter, js_name = "hasInFlight")]
    pub fn has_in_flight(&self) -> bool {
        self.in_flight.is_some()
    }

    /// Number of pending raw bytes (in_flight + buffered).
    /// Used by tests and shutdown logic to decide whether to keep draining.
    #[wasm_bindgen(getter, js_name = "pendingBytes")]
    pub fn pending_bytes(&self) -> usize {
        let in_flight = self.in_flight.as_ref().map(|p| p.raw.len()).unwrap_or(0);
        let buffered: usize = self.buffer.iter().map(|c| c.len()).sum();
        in_flight + buffered
    }
}

// ── Telemetry payload construction (T-4.4) ───────────────────────────────────
//
// ADR docs/adr/2026-04-29-rust-npm-binary-parity.md req 13, 14, 15
// JS equivalent: packages/launcher/src/runtime.js::LauncherRuntime.#postTelemetry

/// Build a telemetry payload JSON string from OS-supplied values.
///
/// OS metric collection (os.hostname, os.cpus, etc.) MUST remain in JS.
/// This function constructs and serialises the payload — the Matrix send call
/// stays in JS via `WasmMatrixClient.sendStateEvent`.
///
/// # Arguments
/// - `level`: `"full"` or `"summary"` (default `"full"` when empty/null)
/// - `hostname`, `platform`, `arch`: always included
/// - `cpus`, `total_memory_mb`, `free_memory_mb`, `uptime_secs`: full-level only
/// - `tmux_available`, `tmux_version`: tmux probe results from JS
/// - `session_persistence`: computed in JS (`useTmux` policy + tmux probe)
/// - `p2p_enabled`: whether P2P is enabled
/// - `p2p_internal_ips_json`: JSON array string of internal IPs, or empty string
/// - `preferred_server`, `preferred_identity`, `accounts_json`, `server_health_json`:
///   multi-homeserver fields; empty strings omit them
/// - `status`: `"online"` or `"offline"`
/// - `heartbeat_interval_ms`: telemetry interval in milliseconds
#[wasm_bindgen(js_name = "buildTelemetryPayload")]
pub fn build_telemetry_payload(
    level: &str,
    hostname: &str,
    platform: &str,
    arch: &str,
    cpus: u32,
    total_memory_mb: u32,
    free_memory_mb: u32,
    uptime_secs: u32,
    tmux_available: bool,
    tmux_version: &str,
    session_persistence: bool,
    p2p_enabled: bool,
    p2p_internal_ips_json: &str,
    preferred_server: &str,
    preferred_identity: &str,
    accounts_json: &str,
    server_health_json: &str,
    status: &str,
    heartbeat_interval_ms: u32,
) -> Result<String, JsValue> {
    let effective_level = if level.is_empty() || level == "full" { "full" } else { level };
    let mut payload = serde_json::Map::new();

    payload.insert("timestamp".to_string(), serde_json::Value::String(
        js_sys::Date::new_0().to_iso_string().as_string().unwrap_or_default(),
    ));
    payload.insert("heartbeat_interval_ms".to_string(), serde_json::Value::Number(heartbeat_interval_ms.into()));
    payload.insert("hostname".to_string(), serde_json::Value::String(hostname.to_string()));
    payload.insert("platform".to_string(), serde_json::Value::String(platform.to_string()));
    payload.insert("arch".to_string(), serde_json::Value::String(arch.to_string()));

    if effective_level == "full" {
        payload.insert("cpus".to_string(), serde_json::Value::Number(cpus.into()));
        payload.insert("total_memory_mb".to_string(), serde_json::Value::Number(total_memory_mb.into()));
        payload.insert("free_memory_mb".to_string(), serde_json::Value::Number(free_memory_mb.into()));
        payload.insert("uptime_secs".to_string(), serde_json::Value::Number(uptime_secs.into()));
    }

    payload.insert("tmux_available".to_string(), serde_json::Value::Bool(tmux_available));
    if !tmux_version.is_empty() {
        payload.insert("tmux_version".to_string(), serde_json::Value::String(tmux_version.to_string()));
    }
    payload.insert("session_persistence".to_string(), serde_json::Value::Bool(session_persistence));

    let mut p2p_obj = serde_json::Map::new();
    p2p_obj.insert("enabled".to_string(), serde_json::Value::Bool(p2p_enabled));
    if !p2p_internal_ips_json.is_empty() {
        let ips: serde_json::Value = serde_json::from_str(p2p_internal_ips_json)
            .map_err(|e| to_js_err(format!("invalid p2p_internal_ips_json: {e}")))?;
        p2p_obj.insert("internal_ips".to_string(), ips);
    }
    payload.insert("p2p".to_string(), serde_json::Value::Object(p2p_obj));

    if !preferred_server.is_empty() {
        payload.insert("preferred_server".to_string(), serde_json::Value::String(preferred_server.to_string()));
        payload.insert("preferred_identity".to_string(), serde_json::Value::String(preferred_identity.to_string()));
    }
    if !accounts_json.is_empty() {
        let accounts: serde_json::Value = serde_json::from_str(accounts_json)
            .map_err(|e| to_js_err(format!("invalid accounts_json: {e}")))?;
        payload.insert("accounts".to_string(), accounts);
    }
    if !server_health_json.is_empty() {
        let health: serde_json::Value = serde_json::from_str(server_health_json)
            .map_err(|e| to_js_err(format!("invalid server_health_json: {e}")))?;
        payload.insert("server_health".to_string(), health);
    }

    payload.insert("status".to_string(), serde_json::Value::String(status.to_string()));

    serde_json::to_string(&serde_json::Value::Object(payload))
        .map_err(|e| to_js_err(format!("serialize telemetry payload: {e}")))
}

// ── P2P session transport state machine (T-4.4) ─────────────────────────────
//
// ADR docs/adr/2026-04-29-rust-npm-binary-parity.md req 13, 15
// JS equivalent: packages/launcher/src/runtime.js::LauncherRuntime.#setupSessionTransport,
//   #releaseRoomTransport, #attemptP2PConnection (state tracking portions)
//
// NodeWebRTCChannel and P2PSignaling remain JS-side (OS-bound native addon).
// This struct tracks connection state, rate limits, refCounts, and attempt IDs.

use std::collections::HashMap;

#[derive(Default)]
struct P2PTransportEntry {
    ref_count: u32,
    last_attempt_ms: f64,
    current_attempt_id: u32,
    settled: bool,
    batch_ms: u32,
}

/// Tracks P2P transport state across rooms.
///
/// One instance manages all room connections for a launcher session.
/// JS creates and holds the actual NodeWebRTCChannel / P2PTransport objects;
/// this struct owns only the coordination state.
#[wasm_bindgen]
pub struct SessionTransportManager {
    rooms: HashMap<String, P2PTransportEntry>,
    p2p_rate_limit_ms: f64,
}

#[wasm_bindgen]
impl SessionTransportManager {
    /// Create a new manager.
    ///
    /// `p2p_rate_limit_ms`: minimum milliseconds between P2P attempts per room.
    /// Pass a negative value to use the default (15_000 ms).
    #[wasm_bindgen(constructor)]
    pub fn new(p2p_rate_limit_ms: f64) -> Self {
        SessionTransportManager {
            rooms: HashMap::new(),
            p2p_rate_limit_ms: if p2p_rate_limit_ms < 0.0 { 15_000.0 } else { p2p_rate_limit_ms },
        }
    }

    /// Register a new transport for a room; returns the initial refCount (1).
    /// If the room already has a transport, increments refCount and returns it.
    #[wasm_bindgen(js_name = "addTransport")]
    pub fn add_transport(&mut self, room_id: &str, batch_ms: u32) -> u32 {
        let entry = self.rooms.entry(room_id.to_string()).or_insert_with(P2PTransportEntry::default);
        if entry.ref_count == 0 {
            entry.batch_ms = batch_ms;
            entry.settled = false;
            entry.last_attempt_ms = 0.0;
            entry.current_attempt_id = 0;
        }
        entry.ref_count += 1;
        entry.ref_count
    }

    /// Decrement refCount for a room; returns true if the transport should be closed
    /// (refCount has reached 0). Returns false if the room is unknown.
    #[wasm_bindgen(js_name = "releaseTransport")]
    pub fn release_transport(&mut self, room_id: &str) -> bool {
        let entry = match self.rooms.get_mut(room_id) {
            Some(e) => e,
            None => return false,
        };
        if entry.ref_count > 0 {
            entry.ref_count -= 1;
        }
        if entry.ref_count == 0 {
            self.rooms.remove(room_id);
            true
        } else {
            false
        }
    }

    /// Returns true if a P2P attempt for `room_id` is allowed (rate limit not exceeded).
    /// Also returns true if no previous attempt has been recorded.
    #[wasm_bindgen(js_name = "shouldAttemptP2P")]
    pub fn should_attempt_p2p(&self, room_id: &str) -> bool {
        match self.rooms.get(room_id) {
            None => false,
            Some(entry) => {
                let now_ms = js_sys::Date::now();
                now_ms - entry.last_attempt_ms >= self.p2p_rate_limit_ms
            }
        }
    }

    /// Record the start of a P2P attempt; returns the new attempt ID.
    /// Resets `settled` for this room.
    #[wasm_bindgen(js_name = "beginP2PAttempt")]
    pub fn begin_p2p_attempt(&mut self, room_id: &str) -> u32 {
        let entry = match self.rooms.get_mut(room_id) {
            Some(e) => e,
            None => return 0,
        };
        entry.last_attempt_ms = js_sys::Date::now();
        entry.current_attempt_id += 1;
        entry.settled = false;
        entry.current_attempt_id
    }

    /// Reset the rate limit for a room (allows immediate retry).
    /// Used when a new session joins an existing room.
    #[wasm_bindgen(js_name = "resetRateLimit")]
    pub fn reset_rate_limit(&mut self, room_id: &str) {
        if let Some(entry) = self.rooms.get_mut(room_id) {
            entry.last_attempt_ms = 0.0;
        }
    }

    /// Returns true if the given attempt ID is no longer the current attempt
    /// (a newer attempt has started). Used by async P2P paths to self-cancel.
    #[wasm_bindgen(js_name = "isAttemptStale")]
    pub fn is_attempt_stale(&self, room_id: &str, attempt_id: u32) -> bool {
        match self.rooms.get(room_id) {
            None => true,
            Some(entry) => entry.current_attempt_id != attempt_id,
        }
    }

    /// Mark a room's P2P connection as settled (data channel opened).
    /// Returns false if the room is unknown (e.g. already released).
    #[wasm_bindgen(js_name = "markSettled")]
    pub fn mark_settled(&mut self, room_id: &str) -> bool {
        match self.rooms.get_mut(room_id) {
            None => false,
            Some(entry) => {
                entry.settled = true;
                true
            }
        }
    }

    /// Returns true if the room's P2P connection is already settled.
    #[wasm_bindgen(js_name = "isSettled")]
    pub fn is_settled(&self, room_id: &str) -> bool {
        self.rooms.get(room_id).map_or(false, |e| e.settled)
    }

    /// Returns the current batch_ms for a room (adjusted on P2P status change).
    #[wasm_bindgen(js_name = "batchMs")]
    pub fn batch_ms(&self, room_id: &str) -> u32 {
        self.rooms.get(room_id).map_or(200, |e| e.batch_ms)
    }

    /// Update batch_ms for a room (called when P2P status changes).
    #[wasm_bindgen(js_name = "setBatchMs")]
    pub fn set_batch_ms(&mut self, room_id: &str, ms: u32) {
        if let Some(entry) = self.rooms.get_mut(room_id) {
            entry.batch_ms = ms;
        }
    }

    /// Returns the number of rooms currently tracked.
    #[wasm_bindgen(getter, js_name = "roomCount")]
    pub fn room_count(&self) -> usize {
        self.rooms.len()
    }
}

// ── Session lifecycle + command dispatch state machine (T-4.5) ───────────────
//
// ADR docs/adr/2026-04-29-rust-npm-binary-parity.md req 13, 14, 15, 16
// JS equivalent: packages/launcher/src/runtime.js::LauncherRuntime (core logic)
//
// WasmSessionManager: pure-Rust session state + command routing.
// JS thin shell: OS-bound I/O (PTY, subprocess, Matrix client, timers).
// Dispatch model: JS calls processCommands(events_json) -> SendActions JSON;
// JS executes the returned send actions against the Matrix client.

const SESSION_EVENT_TASK: &str = "org.mxdx.session.task";
const SESSION_EVENT_START: &str = "org.mxdx.session.start";
const SESSION_EVENT_OUTPUT: &str = "org.mxdx.session.output";
const SESSION_EVENT_RESULT: &str = "org.mxdx.session.result";
const SESSION_EVENT_CANCEL: &str = "org.mxdx.session.cancel";
const SESSION_EVENT_SIGNAL: &str = "org.mxdx.session.signal";
const SESSION_EVENT_ACTIVE: &str = "org.mxdx.session.active";
const SESSION_EVENT_COMPLETED: &str = "org.mxdx.session.completed";

/// A "send action" returned by WasmSessionManager.processCommands().
/// JS executes each action against the Matrix client.
#[derive(Serialize)]
#[serde(tag = "kind")]
enum SendAction {
    /// Regular encrypted room event.
    #[serde(rename = "send_event")]
    SendEvent {
        room_id: String,
        event_type: String,
        content: serde_json::Value,
    },
    /// Encrypted state event (MSC4362 path in JS).
    #[serde(rename = "send_state_event")]
    SendStateEvent {
        room_id: String,
        event_type: String,
        state_key: String,
        content: serde_json::Value,
    },
    /// Spawn a new PTY session (JS OS-bound operation).
    #[serde(rename = "spawn_pty")]
    SpawnPty {
        session_id: String,
        request_id: String,
        command: String,
        args: Vec<String>,
        cols: u16,
        rows: u16,
        cwd: String,
        env: serde_json::Value,
        dm_room_id: String,
        batch_ms: u32,
        persistent: bool,
    },
    /// Execute a command via subprocess (JS OS-bound operation).
    #[serde(rename = "exec_command")]
    ExecCommand {
        request_id: String,
        uuid: String,
        command: String,
        args: Vec<String>,
        cwd: String,
        timeout_ms: u64,
        exec_room_id: String,
    },
    /// Kill a PTY session.
    #[serde(rename = "kill_pty")]
    KillPty {
        session_id: String,
        signal: String,
    },
    /// Write session metadata to state room (JS handles Matrix write).
    #[serde(rename = "write_session")]
    WriteSession {
        state_room_id: String,
        device_id: String,
        session_id: String,
        content: serde_json::Value,
    },
    /// Remove session metadata from state room.
    #[serde(rename = "remove_session")]
    RemoveSession {
        state_room_id: String,
        device_id: String,
        session_id: String,
    },
}

/// Lightweight session record (security: MUST NOT expose sender or dmRoomId as public API).
#[derive(Clone, Serialize, Deserialize)]
struct SessionRecord {
    session_id: String,
    tmux_name: Option<String>,
    dm_room_id: String,
    /// Matrix user ID of the session requester — kept internal, not returned to JS callers.
    #[serde(skip_serializing)]
    sender: String,
    persistent: bool,
    created_at: String,
    alive: bool,
}

/// Configuration for WasmSessionManager.
#[derive(Deserialize)]
struct WasmSessionConfig {
    allowed_commands: Vec<String>,
    allowed_cwd: Vec<String>,
    max_sessions: u32,
    username: String,
    #[serde(default)]
    use_tmux: String,
    #[serde(default = "default_batch_ms")]
    batch_ms: u32,
}

fn default_batch_ms() -> u32 { 200 }

/// Pure-Rust session state machine for the launcher.
///
/// Manages session registry, command routing, and authorization.
/// All OS-bound I/O is expressed as `SendAction` return values for JS to execute.
#[wasm_bindgen]
pub struct WasmSessionManager {
    config: WasmSessionConfig,
    exec_room_id: String,
    state_room_id: String,
    user_id: String,
    device_id: String,
    processed_events: std::collections::HashSet<String>,
    sessions: HashMap<String, SessionRecord>,
    /// Map "username:clientUserId" -> dmRoomId
    session_rooms: HashMap<String, String>,
    active_sessions: u32,
}

#[wasm_bindgen]
impl WasmSessionManager {
    /// Create a new WasmSessionManager from a JSON config string.
    ///
    /// `config_json`: serialized WasmSessionConfig
    /// `exec_room_id`: exec room for session events
    /// `state_room_id`: state room for persistence
    /// `user_id`: Matrix user ID of the launcher
    /// `device_id`: Matrix device ID of the launcher
    #[wasm_bindgen(constructor)]
    pub fn new(
        config_json: &str,
        exec_room_id: &str,
        state_room_id: &str,
        user_id: &str,
        device_id: &str,
    ) -> Result<WasmSessionManager, JsValue> {
        let config: WasmSessionConfig = serde_json::from_str(config_json)
            .map_err(|e| to_js_err(format!("WasmSessionManager config parse error: {e}")))?;
        Ok(WasmSessionManager {
            config,
            exec_room_id: exec_room_id.to_string(),
            state_room_id: state_room_id.to_string(),
            user_id: user_id.to_string(),
            device_id: device_id.to_string(),
            processed_events: std::collections::HashSet::new(),
            sessions: HashMap::new(),
            session_rooms: HashMap::new(),
            active_sessions: 0,
        })
    }

    /// Process a batch of Matrix events from the exec room.
    ///
    /// `events_json`: JSON array of room events (from `collectRoomEvents`)
    ///
    /// Returns a JSON array of `SendAction` objects for JS to execute.
    /// JS executes each action against the Matrix client and OS APIs.
    #[wasm_bindgen(js_name = "processCommands")]
    pub fn process_commands(&mut self, events_json: &str) -> Result<String, JsValue> {
        let events: Vec<serde_json::Value> = serde_json::from_str(events_json)
            .map_err(|e| to_js_err(format!("processCommands: invalid events JSON: {e}")))?;

        let mut actions: Vec<SendAction> = Vec::new();

        for event in &events {
            let event_type = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
            let event_id = event.get("event_id").and_then(|v| v.as_str()).unwrap_or("");
            if event_id.is_empty() { continue; }
            if self.processed_events.contains(event_id) { continue; }
            self.processed_events.insert(event_id.to_string());

            let content = event.get("content").cloned().unwrap_or(serde_json::Value::Object(Default::default()));
            let sender = event.get("sender").and_then(|v| v.as_str()).unwrap_or("");

            match event_type {
                SESSION_EVENT_TASK => {
                    if sender == self.user_id { continue; }
                    self.handle_session_task(&content, event_id, sender, &mut actions);
                }
                SESSION_EVENT_CANCEL => {
                    if sender == self.user_id { continue; }
                    self.handle_session_cancel(&content, &mut actions);
                }
                SESSION_EVENT_SIGNAL => {
                    if sender == self.user_id { continue; }
                    self.handle_session_signal(&content, &mut actions);
                }
                "org.mxdx.command" => {
                    if sender == self.user_id { continue; }
                    self.handle_legacy_command(&content, event_id, sender, &mut actions);
                }
                _ => {}
            }
        }

        serde_json::to_string(&actions)
            .map_err(|e| to_js_err(format!("processCommands: serialize actions: {e}")))
    }

    /// Called by JS when a PTY session exits.
    /// Returns a JSON array of SendActions to execute (state event cleanup).
    #[wasm_bindgen(js_name = "onPtyExit")]
    pub fn on_pty_exit(&mut self, session_id: &str, exit_code: i32) -> Result<String, JsValue> {
        let actions = self.cleanup_session(session_id, exit_code);
        serde_json::to_string(&actions)
            .map_err(|e| to_js_err(format!("onPtyExit: serialize actions: {e}")))
    }

    /// Called by JS when an exec command completes.
    /// Returns a JSON array of SendActions (SESSION_RESULT + state event).
    #[wasm_bindgen(js_name = "onCommandComplete")]
    pub fn on_command_complete(
        &mut self,
        uuid: &str,
        exec_room_id: &str,
        exit_code: i32,
        duration_seconds: u32,
        timed_out: bool,
        tail_json: &str,
        error_msg: &str,
    ) -> Result<String, JsValue> {
        let tail: Vec<String> = if tail_json.is_empty() {
            vec![]
        } else {
            serde_json::from_str(tail_json).unwrap_or_default()
        };

        let status = if exit_code == 0 && !timed_out { "success" } else { "failed" };
        let mut result_content = serde_json::json!({
            "session_uuid": uuid,
            "worker_id": self.user_id,
            "status": status,
            "exit_code": exit_code,
            "duration_seconds": duration_seconds,
            "tail": tail,
            "timed_out": timed_out,
        });
        if !error_msg.is_empty() {
            result_content["error"] = serde_json::Value::String(error_msg.to_string());
        }

        let mut actions = vec![
            SendAction::SendEvent {
                room_id: exec_room_id.to_string(),
                event_type: SESSION_EVENT_RESULT.to_string(),
                content: result_content,
            },
            // Clear active session state
            SendAction::SendStateEvent {
                room_id: exec_room_id.to_string(),
                event_type: SESSION_EVENT_ACTIVE.to_string(),
                state_key: format!("session/{uuid}"),
                content: serde_json::Value::Object(Default::default()),
            },
            // Write completed state
            SendAction::SendStateEvent {
                room_id: exec_room_id.to_string(),
                event_type: SESSION_EVENT_COMPLETED.to_string(),
                state_key: format!("session/{uuid}"),
                content: serde_json::json!({
                    "session_uuid": uuid,
                    "worker_id": self.user_id,
                    "status": status,
                    "exit_code": exit_code,
                    "duration_seconds": duration_seconds,
                }),
            },
        ];

        if self.active_sessions > 0 { self.active_sessions -= 1; }

        serde_json::to_string(&actions)
            .map_err(|e| to_js_err(format!("onCommandComplete: serialize: {e}")))
    }

    /// Report a session as started (called by JS after PTY spawn succeeds).
    /// Returns SendActions to emit SESSION_START + write active state.
    #[wasm_bindgen(js_name = "onSessionStarted")]
    pub fn on_session_started(
        &mut self,
        session_id: &str,
        _request_id: &str,
        dm_room_id: &str,
        tmux_name: &str,
        persistent: bool,
        _batch_ms: u32,
        sender: &str,
        started_at_secs: f64,
        bin: &str,
        args_json: &str,
    ) -> Result<String, JsValue> {
        let args: Vec<String> = serde_json::from_str(args_json).unwrap_or_default();
        let created_at = js_sys::Date::new_0().to_iso_string().as_string().unwrap_or_default();
        self.sessions.insert(session_id.to_string(), SessionRecord {
            session_id: session_id.to_string(),
            tmux_name: if tmux_name.is_empty() { None } else { Some(tmux_name.to_string()) },
            dm_room_id: dm_room_id.to_string(),
            sender: sender.to_string(),
            persistent,
            created_at: created_at.clone(),
            alive: true,
        });

        let actions = vec![
            SendAction::SendEvent {
                room_id: self.exec_room_id.clone(),
                event_type: SESSION_EVENT_START.to_string(),
                content: serde_json::json!({
                    "session_uuid": session_id,
                    "worker_id": self.user_id,
                    "tmux_session": if tmux_name.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(tmux_name.to_string()) },
                    "pid": serde_json::Value::Null,
                    "started_at": started_at_secs as u64,
                }),
            },
            SendAction::SendStateEvent {
                room_id: self.exec_room_id.clone(),
                event_type: SESSION_EVENT_ACTIVE.to_string(),
                state_key: format!("session/{session_id}"),
                content: serde_json::json!({
                    "session_uuid": session_id,
                    "worker_id": self.user_id,
                    "bin": bin,
                    "args": args,
                    "sender_id": sender,
                    "started_at": started_at_secs as u64,
                }),
            },
            SendAction::WriteSession {
                state_room_id: self.state_room_id.clone(),
                device_id: self.device_id.clone(),
                session_id: session_id.to_string(),
                content: serde_json::json!({
                    "uuid": session_id,
                    "tmuxName": tmux_name,
                    "dmRoomId": dm_room_id,
                    "sender": sender,
                    "persistent": persistent,
                    "createdAt": created_at,
                    "state": "running",
                }),
            },
        ];

        serde_json::to_string(&actions)
            .map_err(|e| to_js_err(format!("onSessionStarted: serialize: {e}")))
    }

    /// Register a recovered (existing tmux) session from state room.
    #[wasm_bindgen(js_name = "recoverSession")]
    pub fn recover_session(
        &mut self,
        session_id: &str,
        tmux_name: &str,
        dm_room_id: &str,
        sender: &str,
        persistent: bool,
        created_at: &str,
    ) {
        self.sessions.insert(session_id.to_string(), SessionRecord {
            session_id: session_id.to_string(),
            tmux_name: if tmux_name.is_empty() { None } else { Some(tmux_name.to_string()) },
            dm_room_id: dm_room_id.to_string(),
            sender: sender.to_string(),
            persistent,
            created_at: created_at.to_string(),
            alive: true,
        });
        if !dm_room_id.is_empty() && !sender.is_empty() {
            let key = self.session_room_key(sender);
            self.session_rooms.entry(key).or_insert_with(|| dm_room_id.to_string());
        }
    }

    /// Register a session room mapping (called when a DM room is created/loaded).
    #[wasm_bindgen(js_name = "registerSessionRoom")]
    pub fn register_session_room(&mut self, room_key: &str, room_id: &str) {
        self.session_rooms.insert(room_key.to_string(), room_id.to_string());
    }

    /// Get the DM room ID for a client user, or empty string if not found.
    #[wasm_bindgen(js_name = "getSessionRoomId")]
    pub fn get_session_room_id(&self, client_user_id: &str) -> String {
        let key = self.session_room_key(client_user_id);
        self.session_rooms.get(&key).cloned().unwrap_or_default()
    }

    /// Get the room key for a client user.
    #[wasm_bindgen(js_name = "sessionRoomKey")]
    pub fn session_room_key_pub(&self, client_user_id: &str) -> String {
        self.session_room_key(client_user_id)
    }

    /// Returns a JSON array of public session info (session_id, persistent, tmux_name, alive).
    /// DOES NOT return sender IDs or dmRoomIds (security constraint per T-4.5).
    #[wasm_bindgen(js_name = "listSessions")]
    pub fn list_sessions(&self) -> Result<String, JsValue> {
        let sessions: Vec<serde_json::Value> = self.sessions.values().map(|s| {
            serde_json::json!({
                "session_id": s.session_id,
                "persistent": s.persistent,
                "tmux_name": s.tmux_name,
                "alive": s.alive,
                "created_at": s.created_at,
            })
        }).collect();
        serde_json::to_string(&sessions)
            .map_err(|e| to_js_err(format!("listSessions: serialize: {e}")))
    }

    /// Returns the DM room ID for a session (needed for transport setup in JS).
    #[wasm_bindgen(js_name = "sessionDmRoomId")]
    pub fn session_dm_room_id(&self, session_id: &str) -> String {
        self.sessions.get(session_id).map(|s| s.dm_room_id.clone()).unwrap_or_default()
    }

    /// Returns the sender (client user ID) for a session.
    #[wasm_bindgen(js_name = "sessionSender")]
    pub fn session_sender(&self, session_id: &str) -> String {
        self.sessions.get(session_id).map(|s| s.sender.clone()).unwrap_or_default()
    }

    /// Returns the tmux name for a session, or empty string.
    #[wasm_bindgen(js_name = "sessionTmuxName")]
    pub fn session_tmux_name(&self, session_id: &str) -> String {
        self.sessions.get(session_id).and_then(|s| s.tmux_name.clone()).unwrap_or_default()
    }

    /// Check authorization for a command against the allowlist.
    #[wasm_bindgen(js_name = "isCommandAllowed")]
    pub fn is_command_allowed(&self, command: &str) -> bool {
        if self.config.allowed_commands.is_empty() { return false; }
        self.config.allowed_commands.iter().any(|c| c == command)
    }

    /// Check authorization for a cwd against the allowlist.
    #[wasm_bindgen(js_name = "isCwdAllowed")]
    pub fn is_cwd_allowed(&self, cwd: &str) -> bool {
        self.config.allowed_cwd.iter().any(|allowed| cwd.starts_with(allowed.as_str()))
    }

    /// Number of currently active sessions.
    #[wasm_bindgen(getter, js_name = "activeSessions")]
    pub fn active_sessions(&self) -> u32 {
        self.active_sessions
    }

    /// Maximum allowed sessions.
    #[wasm_bindgen(getter, js_name = "maxSessions")]
    pub fn max_sessions(&self) -> u32 {
        self.config.max_sessions
    }

    /// Increment active session count (called by JS before spawning a PTY or exec).
    #[wasm_bindgen(js_name = "incrementActiveSessions")]
    pub fn increment_active_sessions(&mut self) {
        self.active_sessions += 1;
    }

    /// Decrement active session count (called by JS on session end / exec complete).
    #[wasm_bindgen(js_name = "decrementActiveSessions")]
    pub fn decrement_active_sessions(&mut self) {
        if self.active_sessions > 0 { self.active_sessions -= 1; }
    }

    /// Mark a session as not alive (PTY exited).
    #[wasm_bindgen(js_name = "markSessionDead")]
    pub fn mark_session_dead(&mut self, session_id: &str) {
        if let Some(s) = self.sessions.get_mut(session_id) {
            s.alive = false;
        }
    }

    /// Remove a session record entirely.
    #[wasm_bindgen(js_name = "removeSession")]
    pub fn remove_session(&mut self, session_id: &str) {
        self.sessions.remove(session_id);
    }

    /// Returns the default batch_ms from config.
    #[wasm_bindgen(getter, js_name = "defaultBatchMs")]
    pub fn default_batch_ms(&self) -> u32 {
        self.config.batch_ms
    }

    // ── Internal helpers ──────────────────────────────────────────────────

    fn session_room_key(&self, client_user_id: &str) -> String {
        format!("{}:{}", self.config.username, client_user_id)
    }

    fn handle_session_task(
        &mut self,
        task: &serde_json::Value,
        event_id: &str,
        sender: &str,
        actions: &mut Vec<SendAction>,
    ) {
        let uuid = task.get("uuid").and_then(|v| v.as_str()).unwrap_or(event_id);
        let bin = task.get("bin").or_else(|| task.get("command"))
            .and_then(|v| v.as_str()).unwrap_or("");
        let args: Vec<String> = task.get("args")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();
        let cwd = task.get("cwd").and_then(|v| v.as_str()).unwrap_or("/tmp");
        let interactive = task.get("interactive").and_then(|v| v.as_bool()).unwrap_or(false);
        let timeout_seconds = task.get("timeout_seconds").and_then(|v| v.as_u64());
        let exec_room_id = self.exec_room_id.clone();

        if interactive {
            // Route to interactive handler (spawn_pty)
            let cols = task.get("cols").and_then(|v| v.as_u64()).unwrap_or(80) as u16;
            let rows = task.get("rows").and_then(|v| v.as_u64()).unwrap_or(24) as u16;
            self.handle_interactive_request(uuid, sender, bin, &args, cwd, cols, rows,
                serde_json::Value::Object(Default::default()), self.config.batch_ms, actions);
            return;
        }

        // Validate
        if !self.is_command_allowed(bin) {
            actions.push(SendAction::SendEvent {
                room_id: exec_room_id.clone(),
                event_type: SESSION_EVENT_RESULT.to_string(),
                content: serde_json::json!({
                    "session_uuid": uuid,
                    "worker_id": self.user_id,
                    "status": "failed",
                    "exit_code": 1,
                    "error": format!("Command '{}' is not allowed", bin),
                    "duration_seconds": 0,
                    "tail": [],
                }),
            });
            return;
        }
        if !self.is_cwd_allowed(cwd) {
            actions.push(SendAction::SendEvent {
                room_id: exec_room_id.clone(),
                event_type: SESSION_EVENT_RESULT.to_string(),
                content: serde_json::json!({
                    "session_uuid": uuid,
                    "worker_id": self.user_id,
                    "status": "failed",
                    "exit_code": 1,
                    "error": format!("Working directory '{}' is not allowed", cwd),
                    "duration_seconds": 0,
                    "tail": [],
                }),
            });
            return;
        }
        if self.active_sessions >= self.config.max_sessions {
            actions.push(SendAction::SendEvent {
                room_id: exec_room_id.clone(),
                event_type: SESSION_EVENT_RESULT.to_string(),
                content: serde_json::json!({
                    "session_uuid": uuid,
                    "worker_id": self.user_id,
                    "status": "failed",
                    "exit_code": 1,
                    "error": format!("Session limit reached ({} max)", self.config.max_sessions),
                    "duration_seconds": 0,
                    "tail": [],
                }),
            });
            return;
        }

        self.active_sessions += 1;
        let timeout_ms = timeout_seconds.unwrap_or(30) * 1000;
        let started_at_secs = (js_sys::Date::now() / 1000.0) as u64;

        // SESSION_START event
        actions.push(SendAction::SendEvent {
            room_id: exec_room_id.clone(),
            event_type: SESSION_EVENT_START.to_string(),
            content: serde_json::json!({
                "session_uuid": uuid,
                "worker_id": self.user_id,
                "tmux_session": serde_json::Value::Null,
                "pid": serde_json::Value::Null,
                "started_at": started_at_secs,
            }),
        });
        // ACTIVE state event
        actions.push(SendAction::SendStateEvent {
            room_id: exec_room_id.clone(),
            event_type: SESSION_EVENT_ACTIVE.to_string(),
            state_key: format!("session/{uuid}"),
            content: serde_json::json!({
                "session_uuid": uuid,
                "worker_id": self.user_id,
                "bin": bin,
                "args": args,
                "sender_id": sender,
                "started_at": started_at_secs,
            }),
        });
        // Exec command action (JS runs the subprocess)
        actions.push(SendAction::ExecCommand {
            request_id: uuid.to_string(),
            uuid: uuid.to_string(),
            command: bin.to_string(),
            args,
            cwd: cwd.to_string(),
            timeout_ms,
            exec_room_id,
        });
    }

    fn handle_session_cancel(&mut self, content: &serde_json::Value, actions: &mut Vec<SendAction>) {
        let uuid = content.get("session_uuid").and_then(|v| v.as_str()).unwrap_or("");
        let _grace_seconds = content.get("grace_seconds").and_then(|v| v.as_u64()).unwrap_or(5);
        if uuid.is_empty() { return; }

        if self.sessions.contains_key(uuid) {
            actions.push(SendAction::KillPty {
                session_id: uuid.to_string(),
                signal: "SIGTERM".to_string(),
            });
        }
    }

    fn handle_session_signal(&mut self, content: &serde_json::Value, actions: &mut Vec<SendAction>) {
        let uuid = content.get("session_uuid").and_then(|v| v.as_str()).unwrap_or("");
        let signal = content.get("signal").and_then(|v| v.as_str()).unwrap_or("");
        if uuid.is_empty() || signal.is_empty() { return; }

        if self.sessions.contains_key(uuid) {
            actions.push(SendAction::KillPty {
                session_id: uuid.to_string(),
                signal: signal.to_string(),
            });
        }
    }

    fn handle_legacy_command(
        &mut self,
        content: &serde_json::Value,
        event_id: &str,
        sender: &str,
        actions: &mut Vec<SendAction>,
    ) {
        let action = content.get("action").and_then(|v| v.as_str()).unwrap_or("");
        let command = content.get("command").and_then(|v| v.as_str()).unwrap_or("");
        let args: Vec<String> = content.get("args")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();
        let cwd = content.get("cwd").and_then(|v| v.as_str()).unwrap_or("/tmp");
        let request_id = content.get("request_id").and_then(|v| v.as_str()).unwrap_or(event_id);
        let exec_room_id = self.exec_room_id.clone();

        match action {
            "interactive" => {
                let cols = content.get("cols").and_then(|v| v.as_u64()).unwrap_or(80) as u16;
                let rows = content.get("rows").and_then(|v| v.as_u64()).unwrap_or(24) as u16;
                let env = content.get("env").cloned().unwrap_or(serde_json::Value::Object(Default::default()));
                let client_batch_ms = content.get("batch_ms").and_then(|v| v.as_u64()).unwrap_or(200) as u32;
                let negotiated_batch_ms = std::cmp::max(client_batch_ms, self.config.batch_ms);
                self.handle_interactive_request(request_id, sender, command, &args, cwd,
                    cols, rows, env, negotiated_batch_ms, actions);
            }
            "list_sessions" => {
                // dm_room_id intentionally omitted — it's private session metadata
                let sessions = self.sessions.values().map(|s| serde_json::json!({
                    "session_id": s.session_id,
                    "persistent": s.persistent,
                    "tmux_name": s.tmux_name,
                    "alive": s.alive,
                    "created_at": s.created_at,
                })).collect::<Vec<_>>();
                actions.push(SendAction::SendEvent {
                    room_id: exec_room_id,
                    event_type: "org.mxdx.terminal.sessions".to_string(),
                    content: serde_json::json!({ "request_id": request_id, "sessions": sessions }),
                });
            }
            "reconnect" => {
                let session_id = content.get("session_id").and_then(|v| v.as_str()).unwrap_or("");
                let cols = content.get("cols").and_then(|v| v.as_u64()).unwrap_or(80) as u16;
                let rows = content.get("rows").and_then(|v| v.as_u64()).unwrap_or(24) as u16;
                self.handle_reconnect_request(session_id, request_id, sender, cols, rows, actions);
            }
            _ => {
                // Non-interactive command execution
                if !self.is_command_allowed(command) {
                    actions.push(SendAction::SendEvent {
                        room_id: exec_room_id.clone(),
                        event_type: "org.mxdx.result".to_string(),
                        content: serde_json::json!({
                            "request_id": request_id,
                            "exit_code": 1,
                            "error": format!("Command '{}' is not allowed", command),
                        }),
                    });
                    return;
                }
                if !self.is_cwd_allowed(cwd) {
                    actions.push(SendAction::SendEvent {
                        room_id: exec_room_id.clone(),
                        event_type: "org.mxdx.result".to_string(),
                        content: serde_json::json!({
                            "request_id": request_id,
                            "exit_code": 1,
                            "error": format!("Working directory '{}' is not allowed", cwd),
                        }),
                    });
                    return;
                }
                if self.active_sessions >= self.config.max_sessions {
                    actions.push(SendAction::SendEvent {
                        room_id: exec_room_id.clone(),
                        event_type: "org.mxdx.result".to_string(),
                        content: serde_json::json!({
                            "request_id": request_id,
                            "exit_code": 1,
                            "error": format!("Session limit reached ({} max)", self.config.max_sessions),
                        }),
                    });
                    return;
                }
                self.active_sessions += 1;
                actions.push(SendAction::ExecCommand {
                    request_id: request_id.to_string(),
                    uuid: request_id.to_string(),
                    command: command.to_string(),
                    args,
                    cwd: cwd.to_string(),
                    timeout_ms: 30_000,
                    exec_room_id,
                });
            }
        }
    }

    fn handle_interactive_request(
        &mut self,
        request_id: &str,
        sender: &str,
        command: &str,
        args: &[String],
        cwd: &str,
        cols: u16,
        rows: u16,
        env: serde_json::Value,
        batch_ms: u32,
        actions: &mut Vec<SendAction>,
    ) {
        let exec_room_id = self.exec_room_id.clone();

        if !command.is_empty() && !self.is_command_allowed(command) {
            actions.push(SendAction::SendEvent {
                room_id: exec_room_id,
                event_type: "org.mxdx.terminal.session".to_string(),
                content: serde_json::json!({ "request_id": request_id, "status": "rejected", "room_id": serde_json::Value::Null }),
            });
            return;
        }
        if !self.is_cwd_allowed(cwd) {
            actions.push(SendAction::SendEvent {
                room_id: exec_room_id,
                event_type: "org.mxdx.terminal.session".to_string(),
                content: serde_json::json!({ "request_id": request_id, "status": "rejected", "room_id": serde_json::Value::Null }),
            });
            return;
        }
        if self.active_sessions >= self.config.max_sessions {
            actions.push(SendAction::SendEvent {
                room_id: exec_room_id,
                event_type: "org.mxdx.terminal.session".to_string(),
                content: serde_json::json!({ "request_id": request_id, "status": "rejected", "room_id": serde_json::Value::Null }),
            });
            return;
        }
        if sender.is_empty() {
            actions.push(SendAction::SendEvent {
                room_id: exec_room_id,
                event_type: "org.mxdx.terminal.session".to_string(),
                content: serde_json::json!({ "request_id": request_id, "status": "rejected", "room_id": serde_json::Value::Null }),
            });
            return;
        }

        self.active_sessions += 1;
        let dm_room_id = self.session_rooms.get(&self.session_room_key(sender)).cloned().unwrap_or_default();

        actions.push(SendAction::SpawnPty {
            session_id: request_id.to_string(),
            request_id: request_id.to_string(),
            command: command.to_string(),
            args: args.to_vec(),
            cols,
            rows,
            cwd: cwd.to_string(),
            env,
            dm_room_id,
            batch_ms,
            persistent: self.config.use_tmux != "never",
        });
    }

    fn handle_reconnect_request(
        &mut self,
        session_id: &str,
        request_id: &str,
        sender: &str,
        cols: u16,
        rows: u16,
        actions: &mut Vec<SendAction>,
    ) {
        let exec_room_id = self.exec_room_id.clone();
        let entry = self.sessions.get(session_id);
        match entry {
            None => {
                actions.push(SendAction::SendEvent {
                    room_id: exec_room_id,
                    event_type: "org.mxdx.terminal.session".to_string(),
                    content: serde_json::json!({ "request_id": request_id, "status": "expired", "room_id": serde_json::Value::Null }),
                });
            }
            Some(entry) if !entry.persistent => {
                actions.push(SendAction::SendEvent {
                    room_id: exec_room_id,
                    event_type: "org.mxdx.terminal.session".to_string(),
                    content: serde_json::json!({ "request_id": request_id, "status": "expired", "room_id": serde_json::Value::Null }),
                });
            }
            Some(entry) if entry.sender != sender => {
                actions.push(SendAction::SendEvent {
                    room_id: exec_room_id,
                    event_type: "org.mxdx.terminal.session".to_string(),
                    content: serde_json::json!({ "request_id": request_id, "status": "rejected", "room_id": serde_json::Value::Null }),
                });
            }
            Some(entry) => {
                let dm_room_id = entry.dm_room_id.clone();
                let _tmux_name = entry.tmux_name.clone();
                self.active_sessions += 1;
                actions.push(SendAction::SendEvent {
                    room_id: exec_room_id.clone(),
                    event_type: "org.mxdx.terminal.session".to_string(),
                    content: serde_json::json!({
                        "request_id": request_id,
                        "status": "reconnected",
                        "room_id": dm_room_id,
                        "session_id": session_id,
                        "persistent": true,
                    }),
                });
                actions.push(SendAction::SpawnPty {
                    session_id: session_id.to_string(),
                    request_id: request_id.to_string(),
                    command: "bash".to_string(),
                    args: vec![],
                    cols,
                    rows,
                    cwd: "/tmp".to_string(),
                    env: serde_json::Value::Object(Default::default()),
                    dm_room_id,
                    batch_ms: self.config.batch_ms,
                    persistent: true,
                });
            }
        }
    }

    fn cleanup_session(&mut self, session_id: &str, _exit_code: i32) -> Vec<SendAction> {
        let entry = match self.sessions.get(session_id) {
            Some(e) => e.clone(),
            None => return vec![],
        };

        let mut actions = vec![];
        if !entry.persistent {
            self.sessions.remove(session_id);
            actions.push(SendAction::RemoveSession {
                state_room_id: self.state_room_id.clone(),
                device_id: self.device_id.clone(),
                session_id: session_id.to_string(),
            });
        }

        if self.active_sessions > 0 { self.active_sessions -= 1; }
        actions
    }
}
