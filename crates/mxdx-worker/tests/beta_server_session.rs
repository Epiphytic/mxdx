//! Beta server E2E tests for the unified session architecture.
//!
//! These tests validate session lifecycle, state events, and worker info
//! against the real beta Matrix servers using test credentials.
//!
//! All tests are `#[ignore]` by default since they require `test-credentials.toml`.
//!
//! ## Setup
//!
//! Create `test-credentials.toml` in the repo root (gitignored):
//!
//! ```toml
//! [server]
//! url = "https://ca1-beta.mxdx.dev"
//!
//! [server2]
//! url = "https://ca2-beta.mxdx.dev"
//!
//! [account1]
//! username = "e2e_account1"
//! password = "mxdx-e2e-test-2026!"
//!
//! [account2]
//! username = "e2e_account2"
//! password = "mxdx-e2e-test-2026!"
//! ```
//!
//! Run with: `cargo test -p mxdx-worker --test beta_server_session -- --ignored`

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mxdx_matrix::{MatrixClient, RoomId};
use mxdx_types::events::session::{ActiveSessionState, CompletedSessionState};
use mxdx_types::events::worker_info::WorkerInfo;

// ---------------------------------------------------------------------------
// Credential loading
// ---------------------------------------------------------------------------

struct TestCredentials {
    server_url: String,
    username1: String,
    password1: String,
    username2: String,
    password2: String,
}

fn load_test_credentials() -> Option<TestCredentials> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()?
        .parent()?
        .join("test-credentials.toml");

    if !path.exists() {
        return None;
    }

    let content = std::fs::read_to_string(&path).ok()?;
    let config: toml::Value = content.parse().ok()?;

    Some(TestCredentials {
        server_url: config["server"]["url"].as_str()?.to_string(),
        username1: config["account1"]["username"].as_str()?.to_string(),
        password1: config["account1"]["password"].as_str()?.to_string(),
        username2: config["account2"]["username"].as_str()?.to_string(),
        password2: config["account2"]["password"].as_str()?.to_string(),
    })
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Find an existing room with the given topic, or return None.
async fn find_room_by_topic(client: &MatrixClient, topic: &str) -> Option<String> {
    // Sync first to ensure we have joined rooms
    client.sync_once().await.ok()?;

    for room in client.inner().joined_rooms() {
        if let Some(t) = room.topic() {
            if t == topic {
                return Some(room.room_id().to_string());
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Test 1: Login both accounts
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires test-credentials.toml"]
async fn beta_login_both_accounts() {
    let creds = load_test_credentials().expect("test-credentials.toml required");

    eprintln!("[1/2] Logging in account1 ({})...", creds.username1);
    let client1 = MatrixClient::login_and_connect(&creds.server_url, &creds.username1, &creds.password1)
        .await
        .expect("Account 1 login failed");
    assert!(client1.is_logged_in());
    assert!(client1.crypto_enabled().await);
    eprintln!("  Account 1 logged in, E2EE enabled");

    tokio::time::sleep(Duration::from_secs(2)).await;

    eprintln!("[2/2] Logging in account2 ({})...", creds.username2);
    let client2 = MatrixClient::login_and_connect(&creds.server_url, &creds.username2, &creds.password2)
        .await
        .expect("Account 2 login failed");
    assert!(client2.is_logged_in());
    assert!(client2.crypto_enabled().await);
    eprintln!("  Account 2 logged in, E2EE enabled");
}

// ---------------------------------------------------------------------------
// Test 2: Full unified session lifecycle on beta server
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires test-credentials.toml"]
async fn beta_unified_session_lifecycle() {
    let creds = load_test_credentials().expect("test-credentials.toml required");

    // Login both accounts
    eprintln!("[1/8] Logging in client (account1)...");
    let mut client = MatrixClient::login_and_connect(&creds.server_url, &creds.username1, &creds.password1)
        .await
        .expect("Client login failed");
    client.set_room_creation_delay(Some(Duration::from_secs(3)));
    client.set_room_creation_timeout(Duration::from_secs(120));

    tokio::time::sleep(Duration::from_secs(2)).await;

    eprintln!("[2/8] Logging in worker (account2)...");
    let _worker = MatrixClient::login_and_connect(&creds.server_url, &creds.username2, &creds.password2)
        .await
        .expect("Worker login failed");

    tokio::time::sleep(Duration::from_secs(2)).await;

    // Find or create a stable test room
    let topic = "org.mxdx.e2e.unified-session-lifecycle";
    eprintln!("[3/8] Finding or creating room with topic '{topic}'...");
    let room_id_str = match find_room_by_topic(&client, topic).await {
        Some(id) => {
            eprintln!("  Reusing existing room: {id}");
            id
        }
        None => {
            eprintln!("  Creating new encrypted room...");
            let rid = client
                .create_encrypted_room(&[])
                .await
                .expect("Room creation failed");

            tokio::time::sleep(Duration::from_secs(3)).await;

            // Set the topic so we can find it next time
            client
                .send_state_event(
                    &rid,
                    "m.room.topic",
                    "",
                    serde_json::json!({ "topic": topic }),
                )
                .await
                .expect("Failed to set room topic");

            eprintln!("  Created room: {rid}");
            rid.to_string()
        }
    };

    let room_id = <&RoomId>::try_from(room_id_str.as_str())
        .expect("Invalid room ID");

    tokio::time::sleep(Duration::from_secs(3)).await;
    client.sync_once().await.expect("Sync failed");

    // Submit a task (client -> room)
    let session_uuid = format!("beta-sess-{}", uuid::Uuid::new_v4());
    eprintln!("[4/8] Submitting task {session_uuid}...");
    let task_content = serde_json::json!({
        "type": "org.mxdx.session.task",
        "content": {
            "uuid": session_uuid,
            "sender_id": format!("@{}:{}", creds.username1, "ca1-beta.mxdx.dev"),
            "bin": "echo",
            "args": ["hello-beta"],
            "interactive": false,
            "no_room_output": false,
            "heartbeat_interval_seconds": 30,
            "required_capabilities": [],
        }
    });
    let task_event_id = client
        .send_event(room_id, task_content)
        .await
        .expect("Failed to send task event");
    eprintln!("  Task event sent: {task_event_id}");

    tokio::time::sleep(Duration::from_secs(3)).await;

    // Worker claims by posting ActiveSessionState
    eprintln!("[5/8] Posting ActiveSessionState (worker claims task)...");
    let active_state = ActiveSessionState {
        bin: "echo".into(),
        args: vec!["hello-beta".into()],
        pid: Some(12345),
        start_time: now_secs(),
        client_id: format!("@{}:{}", creds.username1, "ca1-beta.mxdx.dev"),
        interactive: false,
        worker_id: format!("@{}:{}", creds.username2, "ca1-beta.mxdx.dev"),
    };
    let state_key = format!("session/{session_uuid}/active");
    client
        .send_state_event(
            room_id,
            "org.mxdx.session.active",
            &state_key,
            serde_json::to_value(&active_state).expect("serialize active state"),
        )
        .await
        .expect("Failed to send active session state");
    eprintln!("  ActiveSessionState posted");

    tokio::time::sleep(Duration::from_secs(3)).await;

    // Send a heartbeat as a threaded reply
    eprintln!("[6/8] Sending heartbeat (threaded reply)...");
    let heartbeat_content = serde_json::json!({
        "session_uuid": session_uuid,
        "worker_id": format!("@{}:{}", creds.username2, "ca1-beta.mxdx.dev"),
        "timestamp": now_secs(),
        "progress": "50%",
    });
    client
        .send_threaded_event(
            room_id,
            "org.mxdx.session.heartbeat",
            &task_event_id,
            heartbeat_content,
        )
        .await
        .expect("Failed to send heartbeat");
    eprintln!("  Heartbeat sent");

    tokio::time::sleep(Duration::from_secs(3)).await;

    // Post CompletedSessionState
    eprintln!("[7/8] Posting CompletedSessionState...");
    let completed_state = CompletedSessionState {
        exit_code: Some(0),
        duration_seconds: 5,
        completion_time: now_secs(),
    };
    let completed_key = format!("session/{session_uuid}/completed");
    client
        .send_state_event(
            room_id,
            "org.mxdx.session.completed",
            &completed_key,
            serde_json::to_value(&completed_state).expect("serialize completed state"),
        )
        .await
        .expect("Failed to send completed session state");
    eprintln!("  CompletedSessionState posted");

    tokio::time::sleep(Duration::from_secs(3)).await;

    // Verify we can read the state events back
    eprintln!("[8/8] Verifying state events readable...");
    let active_readback = client
        .get_room_state_event(room_id, "org.mxdx.session.active", &state_key)
        .await
        .expect("Failed to read active session state");
    assert_eq!(active_readback["bin"], "echo");
    assert_eq!(active_readback["worker_id"], format!("@{}:{}", creds.username2, "ca1-beta.mxdx.dev"));
    eprintln!("  ActiveSessionState verified");

    let completed_readback = client
        .get_room_state_event(room_id, "org.mxdx.session.completed", &completed_key)
        .await
        .expect("Failed to read completed session state");
    assert_eq!(completed_readback["exit_code"], 0);
    assert_eq!(completed_readback["duration_seconds"], 5);
    eprintln!("  CompletedSessionState verified");

    eprintln!("[ok] Full unified session lifecycle passed on beta server");
}

// ---------------------------------------------------------------------------
// Test 3: State events as process table on beta
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires test-credentials.toml"]
async fn beta_session_state_events() {
    let creds = load_test_credentials().expect("test-credentials.toml required");

    eprintln!("[1/6] Logging in...");
    let mut client = MatrixClient::login_and_connect(&creds.server_url, &creds.username1, &creds.password1)
        .await
        .expect("Login failed");
    client.set_room_creation_delay(Some(Duration::from_secs(3)));
    client.set_room_creation_timeout(Duration::from_secs(120));

    tokio::time::sleep(Duration::from_secs(2)).await;

    // Find or create a stable test room
    let topic = "org.mxdx.e2e.state-events-test";
    eprintln!("[2/6] Finding or creating room with topic '{topic}'...");
    let room_id_str = match find_room_by_topic(&client, topic).await {
        Some(id) => {
            eprintln!("  Reusing existing room: {id}");
            id
        }
        None => {
            let rid = client
                .create_encrypted_room(&[])
                .await
                .expect("Room creation failed");

            tokio::time::sleep(Duration::from_secs(3)).await;

            client
                .send_state_event(
                    &rid,
                    "m.room.topic",
                    "",
                    serde_json::json!({ "topic": topic }),
                )
                .await
                .expect("Failed to set room topic");

            eprintln!("  Created room: {rid}");
            rid.to_string()
        }
    };

    let room_id = <&RoomId>::try_from(room_id_str.as_str())
        .expect("Invalid room ID");

    tokio::time::sleep(Duration::from_secs(3)).await;
    client.sync_once().await.expect("Sync failed");

    // Write ActiveSessionState
    let session_uuid = format!("beta-state-{}", uuid::Uuid::new_v4());
    let state_key = format!("session/{session_uuid}/active");
    eprintln!("[3/6] Writing ActiveSessionState for {session_uuid}...");
    let active = ActiveSessionState {
        bin: "cargo".into(),
        args: vec!["test".into(), "--release".into()],
        pid: Some(99999),
        start_time: now_secs(),
        client_id: format!("@{}:{}", creds.username1, "ca1-beta.mxdx.dev"),
        interactive: false,
        worker_id: format!("@{}:{}", creds.username1, "ca1-beta.mxdx.dev"),
    };
    client
        .send_state_event(
            room_id,
            "org.mxdx.session.active",
            &state_key,
            serde_json::to_value(&active).expect("serialize"),
        )
        .await
        .expect("Failed to write ActiveSessionState");

    tokio::time::sleep(Duration::from_secs(3)).await;

    // Read it back
    eprintln!("[4/6] Reading ActiveSessionState back...");
    let readback = client
        .get_room_state_event(room_id, "org.mxdx.session.active", &state_key)
        .await
        .expect("Failed to read ActiveSessionState");
    let parsed: ActiveSessionState =
        serde_json::from_value(readback).expect("Failed to parse ActiveSessionState");
    assert_eq!(parsed.bin, "cargo");
    assert_eq!(parsed.args, vec!["test", "--release"]);
    assert_eq!(parsed.pid, Some(99999));
    assert!(parsed.interactive == false);
    eprintln!("  ActiveSessionState verified: bin={}, pid={:?}", parsed.bin, parsed.pid);

    tokio::time::sleep(Duration::from_secs(3)).await;

    // Write CompletedSessionState
    let completed_key = format!("session/{session_uuid}/completed");
    eprintln!("[5/6] Writing CompletedSessionState...");
    let completed = CompletedSessionState {
        exit_code: Some(0),
        duration_seconds: 42,
        completion_time: now_secs(),
    };
    client
        .send_state_event(
            room_id,
            "org.mxdx.session.completed",
            &completed_key,
            serde_json::to_value(&completed).expect("serialize"),
        )
        .await
        .expect("Failed to write CompletedSessionState");

    tokio::time::sleep(Duration::from_secs(3)).await;

    // Read it back
    eprintln!("[6/6] Reading CompletedSessionState back...");
    let readback = client
        .get_room_state_event(room_id, "org.mxdx.session.completed", &completed_key)
        .await
        .expect("Failed to read CompletedSessionState");
    let parsed: CompletedSessionState =
        serde_json::from_value(readback).expect("Failed to parse CompletedSessionState");
    assert_eq!(parsed.exit_code, Some(0));
    assert_eq!(parsed.duration_seconds, 42);
    eprintln!("  CompletedSessionState verified: exit_code={:?}, duration={}s", parsed.exit_code, parsed.duration_seconds);

    eprintln!("[ok] Process table state events pattern works on beta server");
}

// ---------------------------------------------------------------------------
// Test 4: WorkerInfo state event on beta
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires test-credentials.toml"]
async fn beta_worker_info_state_event() {
    let creds = load_test_credentials().expect("test-credentials.toml required");

    eprintln!("[1/4] Logging in...");
    let mut client = MatrixClient::login_and_connect(&creds.server_url, &creds.username1, &creds.password1)
        .await
        .expect("Login failed");
    client.set_room_creation_delay(Some(Duration::from_secs(3)));
    client.set_room_creation_timeout(Duration::from_secs(120));

    tokio::time::sleep(Duration::from_secs(2)).await;

    // Find or create a stable test room
    let topic = "org.mxdx.e2e.worker-info-test";
    eprintln!("[2/4] Finding or creating room with topic '{topic}'...");
    let room_id_str = match find_room_by_topic(&client, topic).await {
        Some(id) => {
            eprintln!("  Reusing existing room: {id}");
            id
        }
        None => {
            let rid = client
                .create_encrypted_room(&[])
                .await
                .expect("Room creation failed");

            tokio::time::sleep(Duration::from_secs(3)).await;

            client
                .send_state_event(
                    &rid,
                    "m.room.topic",
                    "",
                    serde_json::json!({ "topic": topic }),
                )
                .await
                .expect("Failed to set room topic");

            eprintln!("  Created room: {rid}");
            rid.to_string()
        }
    };

    let room_id = <&RoomId>::try_from(room_id_str.as_str())
        .expect("Invalid room ID");

    tokio::time::sleep(Duration::from_secs(3)).await;
    client.sync_once().await.expect("Sync failed");

    // Post WorkerInfo state event
    let worker_id = format!("@{}:{}", creds.username1, "ca1-beta.mxdx.dev");
    eprintln!("[3/4] Posting WorkerInfo state event for {worker_id}...");
    let worker_info = WorkerInfo {
        worker_id: worker_id.clone(),
        host: "beta-test-host".into(),
        os: "linux".into(),
        arch: "x86_64".into(),
        cpu_count: 8,
        memory_total_mb: 16384,
        disk_available_mb: 51200,
        tools: vec![],
        capabilities: vec!["linux".into(), "x86_64".into()],
        updated_at: now_secs(),
    };
    let state_key = format!("worker/{}", creds.username1);
    client
        .send_state_event(
            room_id,
            "org.mxdx.worker.info",
            &state_key,
            serde_json::to_value(&worker_info).expect("serialize WorkerInfo"),
        )
        .await
        .expect("Failed to send WorkerInfo state event");
    eprintln!("  WorkerInfo posted");

    tokio::time::sleep(Duration::from_secs(3)).await;

    // Read it back and verify
    eprintln!("[4/4] Reading WorkerInfo back...");
    let readback = client
        .get_room_state_event(room_id, "org.mxdx.worker.info", &state_key)
        .await
        .expect("Failed to read WorkerInfo state event");
    let parsed: WorkerInfo =
        serde_json::from_value(readback).expect("Failed to parse WorkerInfo");

    assert_eq!(parsed.worker_id, worker_id);
    assert_eq!(parsed.host, "beta-test-host");
    assert_eq!(parsed.os, "linux");
    assert_eq!(parsed.arch, "x86_64");
    assert_eq!(parsed.cpu_count, 8);
    assert_eq!(parsed.memory_total_mb, 16384);
    assert_eq!(parsed.disk_available_mb, 51200);
    assert_eq!(parsed.capabilities, vec!["linux", "x86_64"]);
    eprintln!(
        "  WorkerInfo verified: host={}, cpus={}, mem={}MB",
        parsed.host, parsed.cpu_count, parsed.memory_total_mb
    );

    eprintln!("[ok] WorkerInfo state event pattern works on beta server");
}
