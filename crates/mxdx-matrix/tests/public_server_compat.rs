//! Public Matrix server compatibility tests.
//!
//! These tests verify that mxdx works against public Matrix homeservers
//! (e.g., matrix.org) rather than just local Tuwunel instances.
//!
//! All tests are `#[ignore]` by default since they require real credentials.
//!
//! Required environment variables:
//!   MXDX_PUBLIC_HS_URL    — homeserver URL (e.g., https://matrix-client.matrix.org)
//!   MXDX_PUBLIC_USERNAME  — existing account username
//!   MXDX_PUBLIC_PASSWORD  — account password
//!
//! Run with: cargo test -p mxdx-matrix --test public_server_compat -- --ignored

use std::time::Duration;

use matrix_sdk::{
    config::SyncSettings,
    room::MessagesOptions,
    ruma::{
        api::client::room::create_room::v3::Request as CreateRoomRequest,
        events::{
            room::encryption::RoomEncryptionEventContent, EmptyStateKey, InitialStateEvent,
        },
    },
    Client,
};
use serde_json::Value;

/// Read credentials from environment variables. Panics with a helpful
/// message if any are missing.
fn credentials() -> (String, String, String) {
    let hs_url = std::env::var("MXDX_PUBLIC_HS_URL")
        .expect("Set MXDX_PUBLIC_HS_URL (e.g., https://matrix-client.matrix.org)");
    let username = std::env::var("MXDX_PUBLIC_USERNAME")
        .expect("Set MXDX_PUBLIC_USERNAME to an existing account username");
    let password = std::env::var("MXDX_PUBLIC_PASSWORD")
        .expect("Set MXDX_PUBLIC_PASSWORD to the account password");
    (hs_url, username, password)
}

/// Build a matrix-sdk Client with sqlite store, login, and return it.
async fn connect() -> Client {
    let (hs_url, username, password) = credentials();
    let store_dir = tempfile::TempDir::new().unwrap();

    let client = Client::builder()
        .homeserver_url(&hs_url)
        .sqlite_store(store_dir.path(), None)
        .build()
        .await
        .expect("Failed to build client");

    client
        .matrix_auth()
        .login_username(&username, &password)
        .initial_device_display_name("mxdx-compat-test")
        .await
        .expect("Login failed — check credentials");

    // Leak the TempDir so it survives for the duration of the test.
    // Tests are short-lived, so this is acceptable.
    std::mem::forget(store_dir);

    client
}

/// Delete a room by leaving it (best-effort cleanup).
async fn cleanup_room(client: &Client, room_id: &matrix_sdk::ruma::RoomId) {
    if let Some(room) = client.get_room(room_id) {
        let _ = room.leave().await;
        // On matrix.org, rooms are garbage-collected after all members leave.
        // We can also try to forget the room.
        let _ = client.get_room(room_id).map(|r| {
            tokio::spawn(async move {
                let _ = r.forget().await;
            })
        });
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore]
async fn login_with_password() {
    let (hs_url, username, password) = credentials();
    let store_dir = tempfile::TempDir::new().unwrap();

    let client = Client::builder()
        .homeserver_url(&hs_url)
        .sqlite_store(store_dir.path(), None)
        .build()
        .await
        .expect("Failed to build client");

    let response = client
        .matrix_auth()
        .login_username(&username, &password)
        .initial_device_display_name("mxdx-compat-test-login")
        .await;

    assert!(response.is_ok(), "Login should succeed: {:?}", response.err());
    assert!(client.user_id().is_some(), "user_id should be set after login");
}

#[tokio::test]
#[ignore]
async fn e2ee_crypto_is_enabled() {
    let client = connect().await;
    let key = client.encryption().ed25519_key().await;
    assert!(key.is_some(), "E2EE ed25519 key should be available after login with sqlite store");
}

#[tokio::test]
#[ignore]
async fn create_encrypted_room() {
    let client = connect().await;

    let encryption_event = InitialStateEvent::new(
        EmptyStateKey,
        RoomEncryptionEventContent::with_recommended_defaults(),
    );

    let mut request = CreateRoomRequest::new();
    request.initial_state = vec![encryption_event.to_raw_any()];

    let response = client.create_room(request).await;
    assert!(response.is_ok(), "Room creation should succeed: {:?}", response.err());

    let room_id = response.unwrap().room_id().to_owned();
    assert!(!room_id.as_str().is_empty());

    cleanup_room(&client, &room_id).await;
}

#[tokio::test]
#[ignore]
async fn send_custom_event_in_encrypted_room() {
    let client = connect().await;

    // Create encrypted room
    let encryption_event = InitialStateEvent::new(
        EmptyStateKey,
        RoomEncryptionEventContent::with_recommended_defaults(),
    );
    let mut request = CreateRoomRequest::new();
    request.initial_state = vec![encryption_event.to_raw_any()];
    let room_id = client.create_room(request).await.unwrap().room_id().to_owned();

    // Initial sync so we know about the room
    client
        .sync_once(SyncSettings::default().timeout(Duration::from_secs(5)))
        .await
        .expect("Initial sync should succeed");

    let room = client.get_room(&room_id).expect("Room should be in client state");

    // Send a custom org.mxdx.command event
    let content = serde_json::json!({
        "uuid": "compat-test-001",
        "action": "exec",
        "cmd": "echo",
        "args": ["hello"],
        "env": {},
        "timeout_seconds": 10
    });

    let send_result = room.send_raw("org.mxdx.command", content).await;
    assert!(
        send_result.is_ok(),
        "Sending custom event should succeed: {:?}",
        send_result.err()
    );

    cleanup_room(&client, &room_id).await;
}

#[tokio::test]
#[ignore]
async fn send_custom_state_event() {
    let client = connect().await;

    let mut request = CreateRoomRequest::new();
    let encryption_event = InitialStateEvent::new(
        EmptyStateKey,
        RoomEncryptionEventContent::with_recommended_defaults(),
    );
    request.initial_state = vec![encryption_event.to_raw_any()];
    let room_id = client.create_room(request).await.unwrap().room_id().to_owned();

    client
        .sync_once(SyncSettings::default().timeout(Duration::from_secs(5)))
        .await
        .unwrap();

    let room = client.get_room(&room_id).expect("Room should exist");

    // Send a custom state event (like org.mxdx.launcher.status)
    let content = serde_json::json!({
        "status": "online",
        "version": "0.1.0"
    });

    let result = room
        .send_state_event_raw("org.mxdx.launcher.status", "", content)
        .await;
    assert!(
        result.is_ok(),
        "Sending custom state event should succeed: {:?}",
        result.err()
    );

    cleanup_room(&client, &room_id).await;
}

#[tokio::test]
#[ignore]
async fn sync_and_receive_own_custom_events() {
    let client = connect().await;

    // Create encrypted room
    let encryption_event = InitialStateEvent::new(
        EmptyStateKey,
        RoomEncryptionEventContent::with_recommended_defaults(),
    );
    let mut request = CreateRoomRequest::new();
    request.initial_state = vec![encryption_event.to_raw_any()];
    let room_id = client.create_room(request).await.unwrap().room_id().to_owned();

    // Sync to pick up room
    client
        .sync_once(SyncSettings::default().timeout(Duration::from_secs(5)))
        .await
        .unwrap();

    let room = client.get_room(&room_id).expect("Room should exist");

    // Send a custom event
    let test_uuid = format!("compat-recv-{}", uuid::Uuid::new_v4());
    let content = serde_json::json!({
        "uuid": test_uuid,
        "action": "exec",
        "cmd": "echo",
        "args": ["test"],
        "env": {},
    });
    room.send_raw("org.mxdx.command", content).await.unwrap();

    // Sync again to pick up the event
    client
        .sync_once(SyncSettings::default().timeout(Duration::from_secs(5)))
        .await
        .unwrap();

    // Use Room::messages() to retrieve decrypted events
    let messages = room.messages(MessagesOptions::backward()).await;
    assert!(
        messages.is_ok(),
        "Room::messages() should succeed: {:?}",
        messages.err()
    );

    let messages = messages.unwrap();
    let found = messages.chunk.iter().any(|event| {
        if let Ok(json) = serde_json::to_value(event.raw().json()) {
            json.get("content")
                .and_then(|c| c.get("uuid"))
                .and_then(|u| u.as_str())
                == Some(&test_uuid)
        } else {
            false
        }
    });

    assert!(
        found,
        "Should find the custom event via Room::messages() after sync"
    );

    cleanup_room(&client, &room_id).await;
}

#[tokio::test]
#[ignore]
async fn create_room_with_custom_initial_state() {
    let client = connect().await;

    let encryption_event = InitialStateEvent::new(
        EmptyStateKey,
        RoomEncryptionEventContent::with_recommended_defaults(),
    );

    // Create room with custom initial state events (similar to what
    // create_terminal_session_dm does with history_visibility)
    use matrix_sdk::ruma::events::room::history_visibility::{
        HistoryVisibility, RoomHistoryVisibilityEventContent,
    };
    let history_event = InitialStateEvent::new(
        EmptyStateKey,
        RoomHistoryVisibilityEventContent::new(HistoryVisibility::Joined),
    );

    let mut request = CreateRoomRequest::new();
    request.is_direct = true;
    request.initial_state = vec![
        encryption_event.to_raw_any(),
        history_event.to_raw_any(),
    ];

    let response = client.create_room(request).await;
    assert!(
        response.is_ok(),
        "Room creation with custom initial state should succeed: {:?}",
        response.err()
    );

    let room_id = response.unwrap().room_id().to_owned();

    // Verify state via REST API
    let homeserver = client.homeserver();
    let access_token = client.access_token().unwrap();
    let http_client = reqwest::Client::new();

    let url = format!(
        "{}_matrix/client/v3/rooms/{}/state/m.room.history_visibility",
        homeserver,
        room_id
    );
    let resp = http_client
        .get(&url)
        .bearer_auth(&access_token)
        .send()
        .await
        .unwrap();
    let body: Value = resp.json().await.unwrap();
    assert_eq!(
        body["history_visibility"], "joined",
        "history_visibility should be 'joined'"
    );

    cleanup_room(&client, &room_id).await;
}

#[tokio::test]
#[ignore]
async fn create_space_with_child_rooms() {
    use matrix_sdk::ruma::{
        api::client::room::create_room::v3::CreationContent, room::RoomType,
    };

    let client = connect().await;
    let user_id = client.user_id().unwrap();
    let server_name = user_id.server_name().to_string();

    // Create a space
    let mut creation_content = CreationContent::new();
    creation_content.room_type = Some(RoomType::Space);

    let mut space_request = CreateRoomRequest::new();
    space_request.name = Some("mxdx-compat-space".to_string());
    space_request.creation_content = Some(
        matrix_sdk::ruma::serde::Raw::new(&creation_content).expect("serialize creation_content"),
    );

    let space_response = client.create_room(space_request).await;
    assert!(
        space_response.is_ok(),
        "Space creation should succeed: {:?}",
        space_response.err()
    );
    let space_id = space_response.unwrap().room_id().to_owned();

    // Create a child room
    let mut child_request = CreateRoomRequest::new();
    child_request.name = Some("mxdx-compat-child".to_string());
    let child_id = client
        .create_room(child_request)
        .await
        .unwrap()
        .room_id()
        .to_owned();

    // Sync to pick up rooms
    client
        .sync_once(SyncSettings::default().timeout(Duration::from_secs(5)))
        .await
        .unwrap();

    // Link child to space via m.space.child state event
    let space_room = client.get_room(&space_id).expect("Space should exist");
    let via = serde_json::json!({ "via": [server_name] });
    let link_result = space_room
        .send_state_event_raw("m.space.child", child_id.as_str(), via)
        .await;
    assert!(
        link_result.is_ok(),
        "Linking child to space via m.space.child should succeed: {:?}",
        link_result.err()
    );

    cleanup_room(&client, &space_id).await;
    cleanup_room(&client, &child_id).await;
}

#[tokio::test]
#[ignore]
async fn tombstone_room() {
    let client = connect().await;

    // Create two rooms
    let mut req1 = CreateRoomRequest::new();
    req1.name = Some("mxdx-compat-old".to_string());
    let old_room_id = client.create_room(req1).await.unwrap().room_id().to_owned();

    let mut req2 = CreateRoomRequest::new();
    req2.name = Some("mxdx-compat-new".to_string());
    let new_room_id = client.create_room(req2).await.unwrap().room_id().to_owned();

    client
        .sync_once(SyncSettings::default().timeout(Duration::from_secs(5)))
        .await
        .unwrap();

    let room = client.get_room(&old_room_id).expect("Old room should exist");

    // Send tombstone state event
    let content = serde_json::json!({
        "body": "This room has been replaced",
        "replacement_room": new_room_id.to_string(),
    });
    let result = room
        .send_state_event_raw("m.room.tombstone", "", content)
        .await;
    assert!(
        result.is_ok(),
        "Tombstone state event should succeed: {:?}",
        result.err()
    );

    cleanup_room(&client, &old_room_id).await;
    cleanup_room(&client, &new_room_id).await;
}
