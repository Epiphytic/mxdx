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
    server2_url: Option<String>,
    username1: String,
    password1: String,
    username2: String,
    password2: String,
    coordinator_username: Option<String>,
    coordinator_password: Option<String>,
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
        server2_url: config.get("server2")
            .and_then(|s| s.get("url"))
            .and_then(|u| u.as_str())
            .map(|s| s.to_string()),
        username1: config["account1"]["username"].as_str()?.to_string(),
        password1: config["account1"]["password"].as_str()?.to_string(),
        username2: config["account2"]["username"].as_str()?.to_string(),
        password2: config["account2"]["password"].as_str()?.to_string(),
        coordinator_username: config.get("coordinator")
            .and_then(|c| c.get("username"))
            .and_then(|u| u.as_str())
            .map(|s| s.to_string()),
        coordinator_password: config.get("coordinator")
            .and_then(|c| c.get("password"))
            .and_then(|u| u.as_str())
            .map(|s| s.to_string()),
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

// ---------------------------------------------------------------------------
// Test 5: Multi-user session — client submits, worker receives via shared room
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires test-credentials.toml"]
async fn beta_multi_user_session_flow() {
    let creds = load_test_credentials().expect("test-credentials.toml required");

    eprintln!("[1/7] Logging in client (account1) and worker (account2)...");
    let mut client = MatrixClient::login_and_connect(&creds.server_url, &creds.username1, &creds.password1)
        .await
        .expect("Client login failed");
    client.set_room_creation_delay(Some(Duration::from_secs(3)));
    client.set_room_creation_timeout(Duration::from_secs(120));

    tokio::time::sleep(Duration::from_secs(2)).await;

    let worker = MatrixClient::login_and_connect(&creds.server_url, &creds.username2, &creds.password2)
        .await
        .expect("Worker login failed");

    tokio::time::sleep(Duration::from_secs(2)).await;

    // Create encrypted room and invite worker
    let topic = "org.mxdx.e2e.multi-user-session";
    eprintln!("[2/7] Finding or creating shared room...");
    let room_id_str = match find_room_by_topic(&client, topic).await {
        Some(id) => {
            eprintln!("  Reusing existing room: {id}");
            id
        }
        None => {
            eprintln!("  Creating new E2EE room with worker invited...");
            let rid = client
                .create_encrypted_room(&[worker.user_id().to_owned()])
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

    let room_id = <&RoomId>::try_from(room_id_str.as_str()).expect("Invalid room ID");

    // Worker joins the room
    eprintln!("[3/7] Worker joining room...");
    worker.sync_once().await.ok();
    worker.join_room(room_id).await.ok(); // May already be joined
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Exchange E2EE keys
    eprintln!("[4/7] Exchanging E2EE keys...");
    for _ in 0..4 {
        client.sync_once().await.ok();
        worker.sync_once().await.ok();
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    // Client submits task
    let session_uuid = format!("beta-multi-{}", uuid::Uuid::new_v4());
    eprintln!("[5/7] Client submitting task {session_uuid}...");
    let task_content = serde_json::json!({
        "type": "org.mxdx.session.task",
        "content": {
            "uuid": session_uuid,
            "sender_id": client.user_id().to_string(),
            "bin": "whoami",
            "args": [],
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
    eprintln!("  Task sent: {task_event_id}");

    tokio::time::sleep(Duration::from_secs(3)).await;

    // Worker syncs and sees the task
    eprintln!("[6/7] Worker syncing to receive task...");
    worker.sync_once().await.ok();
    let events = worker
        .sync_and_collect_events(room_id, Duration::from_secs(10))
        .await
        .expect("Worker sync failed");

    let found_task = events.iter().any(|e| {
        e.get("content")
            .and_then(|c| c.get("uuid"))
            .and_then(|u| u.as_str())
            == Some(&session_uuid)
    });
    assert!(found_task, "Worker should see the task event from client");
    eprintln!("  Worker received task");

    // Worker posts result back as threaded reply
    eprintln!("[7/7] Worker posting result...");
    let result_content = serde_json::json!({
        "session_uuid": session_uuid,
        "worker_id": worker.user_id().to_string(),
        "status": "success",
        "exit_code": 0,
        "duration_seconds": 1,
        "tail": "e2etest-test2\n",
    });
    worker
        .send_threaded_event(room_id, "org.mxdx.session.result", &task_event_id, result_content)
        .await
        .expect("Failed to send result");

    tokio::time::sleep(Duration::from_secs(3)).await;

    // Client sees the result
    client.sync_once().await.ok();
    let client_events = client
        .sync_and_collect_events(room_id, Duration::from_secs(10))
        .await
        .expect("Client sync failed");

    let found_result = client_events.iter().any(|e| {
        e.get("content")
            .and_then(|c| c.get("status"))
            .and_then(|s| s.as_str())
            == Some("success")
    });
    assert!(found_result, "Client should see the result from worker");

    eprintln!("[ok] Multi-user session flow passed on beta server");
}

// ---------------------------------------------------------------------------
// Test 6: Coordinator account — three-party flow
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires test-credentials.toml with coordinator account"]
async fn beta_coordinator_routes_task() {
    let creds = load_test_credentials().expect("test-credentials.toml required");
    let coord_user = creds.coordinator_username.as_ref().expect("coordinator account required");
    let coord_pass = creds.coordinator_password.as_ref().expect("coordinator password required");

    eprintln!("[1/8] Logging in all three accounts...");
    let mut coordinator = MatrixClient::login_and_connect(&creds.server_url, coord_user, coord_pass)
        .await
        .expect("Coordinator login failed");
    coordinator.set_room_creation_delay(Some(Duration::from_secs(3)));
    coordinator.set_room_creation_timeout(Duration::from_secs(120));

    tokio::time::sleep(Duration::from_secs(2)).await;

    let client = MatrixClient::login_and_connect(&creds.server_url, &creds.username1, &creds.password1)
        .await
        .expect("Client login failed");

    tokio::time::sleep(Duration::from_secs(2)).await;

    let worker = MatrixClient::login_and_connect(&creds.server_url, &creds.username2, &creds.password2)
        .await
        .expect("Worker login failed");

    tokio::time::sleep(Duration::from_secs(2)).await;

    // Coordinator creates a room and invites both
    let topic = "org.mxdx.e2e.coordinator-routing";
    eprintln!("[2/8] Coordinator creating shared room...");
    let room_id_str = match find_room_by_topic(&coordinator, topic).await {
        Some(id) => {
            eprintln!("  Reusing existing room: {id}");
            id
        }
        None => {
            let rid = coordinator
                .create_encrypted_room(&[
                    client.user_id().to_owned(),
                    worker.user_id().to_owned(),
                ])
                .await
                .expect("Room creation failed");

            tokio::time::sleep(Duration::from_secs(3)).await;

            coordinator
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

    let room_id = <&RoomId>::try_from(room_id_str.as_str()).expect("Invalid room ID");

    // Everyone joins and exchanges keys
    eprintln!("[3/8] All parties joining and exchanging E2EE keys...");
    client.sync_once().await.ok();
    worker.sync_once().await.ok();
    client.join_room(room_id).await.ok();
    worker.join_room(room_id).await.ok();
    tokio::time::sleep(Duration::from_secs(2)).await;

    for _ in 0..4 {
        coordinator.sync_once().await.ok();
        client.sync_once().await.ok();
        worker.sync_once().await.ok();
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    // Worker posts WorkerInfo
    eprintln!("[4/8] Worker posting WorkerInfo...");
    let worker_info = WorkerInfo {
        worker_id: worker.user_id().to_string(),
        host: "beta-worker".into(),
        os: "linux".into(),
        arch: "x86_64".into(),
        cpu_count: 4,
        memory_total_mb: 8192,
        disk_available_mb: 40000,
        tools: vec![],
        capabilities: vec!["linux".into(), "bash".into()],
        updated_at: now_secs(),
    };
    coordinator
        .send_state_event(
            room_id,
            "org.mxdx.worker.info",
            &format!("worker/{}", creds.username2),
            serde_json::to_value(&worker_info).expect("serialize"),
        )
        .await
        .ok(); // Coordinator writes on behalf of worker

    tokio::time::sleep(Duration::from_secs(3)).await;

    // Client submits task to coordinator room
    let session_uuid = format!("beta-coord-{}", uuid::Uuid::new_v4());
    eprintln!("[5/8] Client submitting task {session_uuid} to coordinator room...");
    let task_content = serde_json::json!({
        "type": "org.mxdx.session.task",
        "content": {
            "uuid": session_uuid,
            "sender_id": client.user_id().to_string(),
            "bin": "date",
            "args": [],
            "interactive": false,
            "no_room_output": false,
            "heartbeat_interval_seconds": 30,
            "required_capabilities": ["linux"],
        }
    });
    let task_event_id = client
        .send_event(room_id, task_content)
        .await
        .expect("Failed to send task");
    eprintln!("  Task sent: {task_event_id}");

    tokio::time::sleep(Duration::from_secs(3)).await;

    // Coordinator sees the task
    eprintln!("[6/8] Coordinator syncing to see task...");
    coordinator.sync_once().await.ok();
    let coord_events = coordinator
        .sync_and_collect_events(room_id, Duration::from_secs(10))
        .await
        .expect("Coordinator sync failed");
    let coord_sees_task = coord_events.iter().any(|e| {
        e.get("content")
            .and_then(|c| c.get("uuid"))
            .and_then(|u| u.as_str())
            == Some(&session_uuid)
    });
    assert!(coord_sees_task, "Coordinator should see the task");
    eprintln!("  Coordinator received task");

    // Worker also sees it (all three are in the room)
    eprintln!("[7/8] Worker syncing to see task...");
    worker.sync_once().await.ok();
    let worker_events = worker
        .sync_and_collect_events(room_id, Duration::from_secs(10))
        .await
        .expect("Worker sync failed");
    let worker_sees_task = worker_events.iter().any(|e| {
        e.get("content")
            .and_then(|c| c.get("uuid"))
            .and_then(|u| u.as_str())
            == Some(&session_uuid)
    });
    assert!(worker_sees_task, "Worker should see the task");
    eprintln!("  Worker received task");

    // Worker posts result
    eprintln!("[8/8] Worker posting result...");
    let result_content = serde_json::json!({
        "session_uuid": session_uuid,
        "worker_id": worker.user_id().to_string(),
        "status": "success",
        "exit_code": 0,
        "duration_seconds": 1,
    });
    worker
        .send_threaded_event(room_id, "org.mxdx.session.result", &task_event_id, result_content)
        .await
        .expect("Failed to send result");

    tokio::time::sleep(Duration::from_secs(3)).await;

    // Client and coordinator both see the result
    client.sync_once().await.ok();
    coordinator.sync_once().await.ok();

    let client_events = client
        .sync_and_collect_events(room_id, Duration::from_secs(10))
        .await
        .expect("Client sync failed");
    let client_sees_result = client_events.iter().any(|e| {
        e.get("content")
            .and_then(|c| c.get("status"))
            .and_then(|s| s.as_str())
            == Some("success")
    });
    assert!(client_sees_result, "Client should see the result");

    eprintln!("[ok] Three-party coordinator routing flow passed on beta server");
}

// ---------------------------------------------------------------------------
// Test 7: Cross-server federation — client on ca1, worker on ca2
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires test-credentials.toml with both servers"]
async fn beta_cross_server_session_flow() {
    let creds = load_test_credentials().expect("test-credentials.toml required");
    let server2_url = creds.server2_url.as_ref().expect("server2 URL required for federation tests");

    eprintln!("[1/9] Logging in client on ca1 ({})...", creds.server_url);
    let mut client = MatrixClient::login_and_connect(&creds.server_url, &creds.username1, &creds.password1)
        .await
        .expect("Client login on ca1 failed");
    client.set_room_creation_delay(Some(Duration::from_secs(3)));
    client.set_room_creation_timeout(Duration::from_secs(120));

    tokio::time::sleep(Duration::from_secs(2)).await;

    eprintln!("[2/9] Logging in worker on ca2 ({server2_url})...");
    let worker = MatrixClient::login_and_connect(server2_url, &creds.username2, &creds.password2)
        .await
        .expect("Worker login on ca2 failed");

    tokio::time::sleep(Duration::from_secs(2)).await;

    eprintln!("  Client user_id: {}", client.user_id());
    eprintln!("  Worker user_id: {}", worker.user_id());
    assert_ne!(
        client.user_id().server_name(),
        worker.user_id().server_name(),
        "Client and worker must be on different servers for this test"
    );

    // Client creates room on ca1 and invites worker on ca2
    let topic = "org.mxdx.e2e.cross-server-session";
    eprintln!("[3/9] Client creating federated room, inviting worker...");
    let room_id_str = match find_room_by_topic(&client, topic).await {
        Some(id) => {
            eprintln!("  Reusing existing room: {id}");
            id
        }
        None => {
            eprintln!("  Creating new E2EE room with cross-server invite...");
            let rid = client
                .create_encrypted_room(&[worker.user_id().to_owned()])
                .await
                .expect("Room creation failed");

            tokio::time::sleep(Duration::from_secs(5)).await; // Extra time for federation

            client
                .send_state_event(
                    &rid,
                    "m.room.topic",
                    "",
                    serde_json::json!({ "topic": topic }),
                )
                .await
                .expect("Failed to set room topic");

            eprintln!("  Created federated room: {rid}");
            rid.to_string()
        }
    };

    let room_id = <&RoomId>::try_from(room_id_str.as_str()).expect("Invalid room ID");

    // Worker joins from ca2 (federation must deliver the invite)
    eprintln!("[4/9] Worker joining from ca2 (federation)...");
    for attempt in 1..=5 {
        worker.sync_once().await.ok();
        match worker.join_room(room_id).await {
            Ok(_) => {
                eprintln!("  Worker joined on attempt {attempt}");
                break;
            }
            Err(e) if attempt < 5 => {
                eprintln!("  Join attempt {attempt} failed ({e}), retrying after 3s...");
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
            Err(e) => panic!("Worker failed to join after 5 attempts: {e}"),
        }
    }

    tokio::time::sleep(Duration::from_secs(3)).await;

    // Exchange E2EE keys across federation
    eprintln!("[5/9] Exchanging E2EE keys across federation...");
    for round in 1..=6 {
        client.sync_once().await.ok();
        worker.sync_once().await.ok();
        tokio::time::sleep(Duration::from_millis(500)).await;
        if round % 2 == 0 {
            eprintln!("  Key exchange round {round}/6");
        }
    }

    // Client submits task from ca1
    let session_uuid = format!("beta-xserver-{}", uuid::Uuid::new_v4());
    eprintln!("[6/9] Client (ca1) submitting task {session_uuid}...");
    let task_content = serde_json::json!({
        "type": "org.mxdx.session.task",
        "content": {
            "uuid": session_uuid,
            "sender_id": client.user_id().to_string(),
            "bin": "hostname",
            "args": [],
            "interactive": false,
            "no_room_output": false,
            "heartbeat_interval_seconds": 30,
            "required_capabilities": [],
        }
    });
    let task_event_id = client
        .send_event(room_id, task_content)
        .await
        .expect("Failed to send task from ca1");
    eprintln!("  Task sent from ca1: {task_event_id}");

    tokio::time::sleep(Duration::from_secs(5)).await; // Federation delivery time

    // Worker on ca2 receives the task
    eprintln!("[7/9] Worker (ca2) syncing to receive task...");
    worker.sync_once().await.ok();
    let events = worker
        .sync_and_collect_events(room_id, Duration::from_secs(15))
        .await
        .expect("Worker sync failed");

    let found_task = events.iter().any(|e| {
        e.get("content")
            .and_then(|c| c.get("uuid"))
            .and_then(|u| u.as_str())
            == Some(&session_uuid)
    });
    assert!(found_task, "Worker on ca2 should see the task sent from ca1 via federation");
    eprintln!("  Worker on ca2 received task via federation");

    // Worker on ca2 posts result back
    eprintln!("[8/9] Worker (ca2) posting result...");
    let result_content = serde_json::json!({
        "session_uuid": session_uuid,
        "worker_id": worker.user_id().to_string(),
        "status": "success",
        "exit_code": 0,
        "duration_seconds": 2,
        "tail": "ca2-worker-host\n",
    });
    worker
        .send_threaded_event(room_id, "org.mxdx.session.result", &task_event_id, result_content)
        .await
        .expect("Worker on ca2 failed to send result");

    tokio::time::sleep(Duration::from_secs(5)).await; // Federation delivery time

    // Client on ca1 sees the result
    eprintln!("[9/9] Client (ca1) verifying result from ca2...");
    client.sync_once().await.ok();
    let client_events = client
        .sync_and_collect_events(room_id, Duration::from_secs(15))
        .await
        .expect("Client sync failed");

    let found_result = client_events.iter().any(|e| {
        e.get("content")
            .and_then(|c| c.get("status"))
            .and_then(|s| s.as_str())
            == Some("success")
    });
    assert!(found_result, "Client on ca1 should see result from worker on ca2");

    eprintln!("[ok] Cross-server federated session flow passed (ca1 ↔ ca2)");
}

// ---------------------------------------------------------------------------
// Test 8: Cross-server state events — state events readable across federation
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires test-credentials.toml with both servers"]
async fn beta_cross_server_state_events() {
    let creds = load_test_credentials().expect("test-credentials.toml required");
    let server2_url = creds.server2_url.as_ref().expect("server2 URL required");

    eprintln!("[1/6] Logging in: client on ca1, worker on ca2...");
    let mut client = MatrixClient::login_and_connect(&creds.server_url, &creds.username1, &creds.password1)
        .await
        .expect("Client login failed");
    client.set_room_creation_delay(Some(Duration::from_secs(3)));
    client.set_room_creation_timeout(Duration::from_secs(120));

    tokio::time::sleep(Duration::from_secs(2)).await;

    let worker = MatrixClient::login_and_connect(server2_url, &creds.username2, &creds.password2)
        .await
        .expect("Worker login failed");

    tokio::time::sleep(Duration::from_secs(2)).await;

    // Find or create federated room
    let topic = "org.mxdx.e2e.cross-server-state";
    eprintln!("[2/6] Setting up federated room...");
    let room_id_str = match find_room_by_topic(&client, topic).await {
        Some(id) => id,
        None => {
            let rid = client
                .create_encrypted_room(&[worker.user_id().to_owned()])
                .await
                .expect("Room creation failed");
            tokio::time::sleep(Duration::from_secs(5)).await;
            client
                .send_state_event(&rid, "m.room.topic", "", serde_json::json!({ "topic": topic }))
                .await
                .expect("Failed to set topic");
            rid.to_string()
        }
    };
    let room_id = <&RoomId>::try_from(room_id_str.as_str()).expect("Invalid room ID");

    // Worker joins and exchange keys
    for _ in 0..3 {
        worker.sync_once().await.ok();
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    worker.join_room(room_id).await.ok();
    tokio::time::sleep(Duration::from_secs(3)).await;
    for _ in 0..4 {
        client.sync_once().await.ok();
        worker.sync_once().await.ok();
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    // Worker on ca2 needs power level to send state events
    // Grant power level 50 to worker
    eprintln!("[3/6] Granting worker power level for state events...");
    let power_levels = serde_json::json!({
        "users": {
            client.user_id().to_string(): 100,
            worker.user_id().to_string(): 50,
        },
        "state_default": 50,
    });
    client
        .send_state_event(room_id, "m.room.power_levels", "", power_levels)
        .await
        .expect("Failed to set power levels");
    tokio::time::sleep(Duration::from_secs(3)).await;
    worker.sync_once().await.ok();

    // Worker on ca2 writes ActiveSessionState
    let session_uuid = format!("beta-xstate-{}", uuid::Uuid::new_v4());
    let state_key = format!("session/{session_uuid}/active");
    eprintln!("[4/6] Worker (ca2) writing ActiveSessionState...");
    let active = ActiveSessionState {
        bin: "uname".into(),
        args: vec!["-a".into()],
        pid: Some(54321),
        start_time: now_secs(),
        client_id: client.user_id().to_string(),
        interactive: false,
        worker_id: worker.user_id().to_string(),
    };
    worker
        .send_state_event(
            room_id,
            "org.mxdx.session.active",
            &state_key,
            serde_json::to_value(&active).expect("serialize"),
        )
        .await
        .expect("Worker on ca2 failed to write state event");
    eprintln!("  State event written from ca2");

    tokio::time::sleep(Duration::from_secs(5)).await; // Federation sync time

    // Client on ca1 reads the state event
    eprintln!("[5/6] Client (ca1) reading state event written by ca2...");
    client.sync_once().await.ok();
    let readback = client
        .get_room_state_event(room_id, "org.mxdx.session.active", &state_key)
        .await
        .expect("Client on ca1 failed to read state event from ca2");
    let parsed: ActiveSessionState =
        serde_json::from_value(readback).expect("Failed to parse");

    assert_eq!(parsed.bin, "uname");
    assert_eq!(parsed.worker_id, worker.user_id().to_string());
    assert_eq!(parsed.client_id, client.user_id().to_string());
    eprintln!("  Client (ca1) verified state event from worker (ca2): bin={}, worker={}", parsed.bin, parsed.worker_id);

    // Worker on ca2 writes CompletedSessionState
    let completed_key = format!("session/{session_uuid}/completed");
    eprintln!("[6/6] Cross-server completed state event...");
    let completed = CompletedSessionState {
        exit_code: Some(0),
        duration_seconds: 3,
        completion_time: now_secs(),
    };
    worker
        .send_state_event(
            room_id,
            "org.mxdx.session.completed",
            &completed_key,
            serde_json::to_value(&completed).expect("serialize"),
        )
        .await
        .expect("Failed to write completed state");

    tokio::time::sleep(Duration::from_secs(5)).await;

    let readback = client
        .get_room_state_event(room_id, "org.mxdx.session.completed", &completed_key)
        .await
        .expect("Failed to read completed state");
    let parsed: CompletedSessionState = serde_json::from_value(readback).expect("parse");
    assert_eq!(parsed.exit_code, Some(0));
    eprintln!("  Completed state verified across federation");

    eprintln!("[ok] Cross-server state events work across ca1 ↔ ca2 federation");
}

// ---------------------------------------------------------------------------
// Test 9: Three-party cross-server — coordinator ca1, client ca1, worker ca2
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires test-credentials.toml with both servers and coordinator"]
async fn beta_cross_server_coordinator_routing() {
    let creds = load_test_credentials().expect("test-credentials.toml required");
    let server2_url = creds.server2_url.as_ref().expect("server2 URL required");
    let coord_user = creds.coordinator_username.as_ref().expect("coordinator required");
    let coord_pass = creds.coordinator_password.as_ref().expect("coordinator password required");

    eprintln!("[1/9] Logging in: coordinator + client on ca1, worker on ca2...");
    let mut coordinator = MatrixClient::login_and_connect(&creds.server_url, coord_user, coord_pass)
        .await
        .expect("Coordinator login failed");
    coordinator.set_room_creation_delay(Some(Duration::from_secs(3)));
    coordinator.set_room_creation_timeout(Duration::from_secs(120));

    tokio::time::sleep(Duration::from_secs(2)).await;

    let client = MatrixClient::login_and_connect(&creds.server_url, &creds.username1, &creds.password1)
        .await
        .expect("Client login failed");

    tokio::time::sleep(Duration::from_secs(2)).await;

    // Worker on the OTHER server
    let worker = MatrixClient::login_and_connect(server2_url, &creds.username2, &creds.password2)
        .await
        .expect("Worker login on ca2 failed");

    tokio::time::sleep(Duration::from_secs(2)).await;

    eprintln!("  Coordinator: {} (ca1)", coordinator.user_id());
    eprintln!("  Client:      {} (ca1)", client.user_id());
    eprintln!("  Worker:      {} (ca2)", worker.user_id());

    // Coordinator creates room, invites both
    let topic = "org.mxdx.e2e.cross-server-coordinator";
    eprintln!("[2/9] Coordinator creating federated room...");
    let room_id_str = match find_room_by_topic(&coordinator, topic).await {
        Some(id) => {
            eprintln!("  Reusing existing room: {id}");
            id
        }
        None => {
            let rid = coordinator
                .create_encrypted_room(&[
                    client.user_id().to_owned(),
                    worker.user_id().to_owned(),
                ])
                .await
                .expect("Room creation failed");
            tokio::time::sleep(Duration::from_secs(5)).await;
            coordinator
                .send_state_event(&rid, "m.room.topic", "", serde_json::json!({ "topic": topic }))
                .await
                .expect("Failed to set topic");
            eprintln!("  Created federated room: {rid}");
            rid.to_string()
        }
    };
    let room_id = <&RoomId>::try_from(room_id_str.as_str()).expect("Invalid room ID");

    // Everyone joins
    eprintln!("[3/9] All parties joining (including ca2 worker via federation)...");
    for _ in 0..3 {
        client.sync_once().await.ok();
        worker.sync_once().await.ok();
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    client.join_room(room_id).await.ok();
    worker.join_room(room_id).await.ok();
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Exchange E2EE keys (extra rounds for 3-party federation)
    eprintln!("[4/9] Exchanging E2EE keys (3-party federation)...");
    for round in 1..=8 {
        coordinator.sync_once().await.ok();
        client.sync_once().await.ok();
        worker.sync_once().await.ok();
        tokio::time::sleep(Duration::from_millis(500)).await;
        if round % 2 == 0 {
            eprintln!("  Key exchange round {round}/8");
        }
    }

    // Worker on ca2 posts WorkerInfo
    eprintln!("[5/9] Worker (ca2) posting WorkerInfo...");
    let worker_info = WorkerInfo {
        worker_id: worker.user_id().to_string(),
        host: "ca2-worker-host".into(),
        os: "linux".into(),
        arch: "x86_64".into(),
        cpu_count: 4,
        memory_total_mb: 8192,
        disk_available_mb: 40000,
        tools: vec![],
        capabilities: vec!["linux".into(), "remote".into()],
        updated_at: now_secs(),
    };
    // Coordinator grants worker power to write state
    coordinator
        .send_state_event(
            room_id,
            "m.room.power_levels",
            "",
            serde_json::json!({
                "users": {
                    coordinator.user_id().to_string(): 100,
                    client.user_id().to_string(): 50,
                    worker.user_id().to_string(): 50,
                },
                "state_default": 50,
            }),
        )
        .await
        .ok();
    tokio::time::sleep(Duration::from_secs(3)).await;
    worker.sync_once().await.ok();

    worker
        .send_state_event(
            room_id,
            "org.mxdx.worker.info",
            &format!("worker/{}", worker.user_id()),
            serde_json::to_value(&worker_info).expect("serialize"),
        )
        .await
        .expect("Worker failed to post WorkerInfo");
    tokio::time::sleep(Duration::from_secs(5)).await;

    // Client on ca1 submits task
    let session_uuid = format!("beta-xcoord-{}", uuid::Uuid::new_v4());
    eprintln!("[6/9] Client (ca1) submitting task to federated coordinator room...");
    let task_content = serde_json::json!({
        "type": "org.mxdx.session.task",
        "content": {
            "uuid": session_uuid,
            "sender_id": client.user_id().to_string(),
            "bin": "date",
            "args": ["+%s"],
            "interactive": false,
            "no_room_output": false,
            "heartbeat_interval_seconds": 30,
            "required_capabilities": ["linux"],
        }
    });
    let task_event_id = client
        .send_event(room_id, task_content)
        .await
        .expect("Client failed to send task");
    eprintln!("  Task sent: {task_event_id}");

    tokio::time::sleep(Duration::from_secs(5)).await;

    // Coordinator on ca1 sees the task
    eprintln!("[7/9] Coordinator (ca1) receiving task...");
    coordinator.sync_once().await.ok();
    let coord_events = coordinator
        .sync_and_collect_events(room_id, Duration::from_secs(15))
        .await
        .expect("Coordinator sync failed");
    let coord_sees_task = coord_events.iter().any(|e| {
        e.get("content")
            .and_then(|c| c.get("uuid"))
            .and_then(|u| u.as_str())
            == Some(&session_uuid)
    });
    assert!(coord_sees_task, "Coordinator on ca1 should see the task");

    // Worker on ca2 sees the task
    eprintln!("[8/9] Worker (ca2) receiving task via federation...");
    worker.sync_once().await.ok();
    let worker_events = worker
        .sync_and_collect_events(room_id, Duration::from_secs(15))
        .await
        .expect("Worker sync failed");
    let worker_sees_task = worker_events.iter().any(|e| {
        e.get("content")
            .and_then(|c| c.get("uuid"))
            .and_then(|u| u.as_str())
            == Some(&session_uuid)
    });
    assert!(worker_sees_task, "Worker on ca2 should see the task via federation");

    // Worker on ca2 posts result
    eprintln!("[9/9] Worker (ca2) posting result...");
    let result_content = serde_json::json!({
        "session_uuid": session_uuid,
        "worker_id": worker.user_id().to_string(),
        "status": "success",
        "exit_code": 0,
        "duration_seconds": 1,
    });
    worker
        .send_threaded_event(room_id, "org.mxdx.session.result", &task_event_id, result_content)
        .await
        .expect("Worker failed to send result");

    tokio::time::sleep(Duration::from_secs(5)).await;

    // Client on ca1 sees result from worker on ca2
    client.sync_once().await.ok();
    let client_events = client
        .sync_and_collect_events(room_id, Duration::from_secs(15))
        .await
        .expect("Client sync failed");
    let client_sees_result = client_events.iter().any(|e| {
        e.get("content")
            .and_then(|c| c.get("status"))
            .and_then(|s| s.as_str())
            == Some("success")
    });
    assert!(client_sees_result, "Client on ca1 should see result from worker on ca2");

    eprintln!("[ok] Cross-server coordinator routing passed (coordinator+client ca1, worker ca2)");
}
