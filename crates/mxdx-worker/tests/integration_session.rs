//! Integration tests for the unified session event schema using a real Tuwunel Matrix homeserver.
//!
//! NOTE: These are INTEGRATION tests, not end-to-end tests. They exercise library code
//! directly (MatrixClient, session types) to validate that the event schema works correctly
//! through real Matrix protocol — encrypted rooms, threaded events, state events,
//! and E2EE key exchange. They do NOT spawn the compiled mxdx-worker/mxdx-client binaries.
//!
//! For true E2E tests that exercise the compiled binaries as subprocesses,
//! see `e2e_binary.rs` and `e2e_binary_beta.rs`.

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use mxdx_matrix::{MatrixClient, OwnedRoomId};
use mxdx_test_helpers::tuwunel::TuwunelInstance;
use mxdx_types::events::capability::{InputSchema, SchemaProperty, WorkerTool};
use mxdx_types::events::fabric::{FailurePolicy, RoutingMode, TaskEvent};
use mxdx_types::events::session::{
    ActiveSessionState, CompletedSessionState, OutputStream, SessionCancel, SessionHeartbeat,
    SessionOutput, SessionResult, SessionStart, SessionStatus, SessionTask, SESSION_CANCEL,
    SESSION_HEARTBEAT, SESSION_OUTPUT, SESSION_RESULT, SESSION_START, SESSION_TASK,
};
use mxdx_types::events::worker_info::{WorkerInfo, WORKER_INFO};
use mxdx_worker::compat::translate_legacy_task;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Register two users, create an encrypted room, join, and exchange E2EE keys.
/// Returns (client_mc, worker_mc, room_id).
async fn setup_encrypted_pair(
    base_url: &str,
) -> (MatrixClient, MatrixClient, OwnedRoomId) {
    let client_mc = MatrixClient::register_and_connect(base_url, "client-user", "pass123", "mxdx-test-token")
        .await
        .unwrap();
    let worker_mc = MatrixClient::register_and_connect(base_url, "worker-user", "pass123", "mxdx-test-token")
        .await
        .unwrap();

    let room_id = client_mc
        .create_encrypted_room(&[worker_mc.user_id().to_owned()])
        .await
        .unwrap();

    worker_mc.sync_once().await.unwrap();
    worker_mc.join_room(&room_id).await.unwrap();

    // Set power levels so the worker can send state events
    let power_levels = serde_json::json!({
        "users": {
            client_mc.user_id().to_string(): 100,
            worker_mc.user_id().to_string(): 50
        },
        "users_default": 0,
        "events_default": 0,
        "state_default": 50,
        "ban": 50,
        "kick": 50,
        "invite": 50,
        "redact": 50
    });
    client_mc
        .send_state_event(&room_id, "m.room.power_levels", "", power_levels)
        .await
        .unwrap();

    // Exchange E2EE keys — 4 sync rounds
    for _ in 0..4 {
        client_mc.sync_once().await.unwrap();
        worker_mc.sync_once().await.unwrap();
    }

    (client_mc, worker_mc, room_id)
}

fn make_session_task(uuid: &str, sender_id: &str) -> SessionTask {
    SessionTask {
        uuid: uuid.into(),
        sender_id: sender_id.into(),
        bin: "echo".into(),
        args: vec!["hello".into(), "world".into()],
        env: None,
        cwd: None,
        interactive: false,
        no_room_output: false,
        timeout_seconds: Some(60),
        heartbeat_interval_seconds: 30,
        plan: None,
        required_capabilities: vec![],
        routing_mode: None,
        on_timeout: None,
        on_heartbeat_miss: None,
    }
}

// ---------------------------------------------------------------------------
// Test 1: Full session lifecycle
// ---------------------------------------------------------------------------

/// Client submits a SessionTask, worker claims it, posts output, posts result.
/// Client observes all events in the thread.
#[tokio::test]
async fn full_session_lifecycle_e2e() {
    let mut hs = TuwunelInstance::start().await.unwrap();
    let base_url = format!("http://127.0.0.1:{}", hs.port);
    let (client_mc, worker_mc, room_id) = setup_encrypted_pair(&base_url).await;

    // Client submits SessionTask as thread root
    let task = make_session_task("e2e-session-001", &client_mc.user_id().to_string());
    let task_json = serde_json::to_value(&task).unwrap();
    let task_event_id = client_mc
        .send_event(
            &room_id,
            serde_json::json!({
                "type": SESSION_TASK,
                "content": task_json,
            }),
        )
        .await
        .unwrap();

    // Worker syncs and sees the task
    worker_mc.sync_once().await.unwrap();
    let events = worker_mc
        .sync_and_collect_events(&room_id, Duration::from_secs(5))
        .await
        .unwrap();
    let found_task = events.iter().any(|e| {
        e.get("content")
            .and_then(|c| c.get("uuid"))
            .and_then(|u| u.as_str())
            == Some("e2e-session-001")
    });
    assert!(
        found_task,
        "worker should see the task event, got: {:?}",
        events
    );

    // Worker writes ActiveSessionState as state event
    let active_state = ActiveSessionState {
        bin: "echo".into(),
        args: vec!["hello".into(), "world".into()],
        pid: Some(12345),
        start_time: now_secs(),
        client_id: client_mc.user_id().to_string(),
        interactive: false,
        worker_id: worker_mc.user_id().to_string(),
    };
    worker_mc
        .send_state_event(
            &room_id,
            "org.mxdx.session.active",
            "session/e2e-session-001/active",
            serde_json::to_value(&active_state).unwrap(),
        )
        .await
        .unwrap();

    // Worker posts SessionStart as threaded event
    let start_event = SessionStart {
        session_uuid: "e2e-session-001".into(),
        worker_id: worker_mc.user_id().to_string(),
        tmux_session: Some("mxdx-e2e-session-001".into()),
        pid: Some(12345),
        started_at: now_secs(),
    };
    worker_mc
        .send_threaded_event(
            &room_id,
            SESSION_START,
            &task_event_id,
            serde_json::to_value(&start_event).unwrap(),
        )
        .await
        .unwrap();

    // Worker posts SessionOutput as threaded event
    let output = SessionOutput {
        session_uuid: "e2e-session-001".into(),
        worker_id: worker_mc.user_id().to_string(),
        stream: OutputStream::Stdout,
        data: base64::engine::general_purpose::STANDARD.encode(b"hello world\n"),
        seq: 0,
        timestamp: now_secs(),
    };
    worker_mc
        .send_threaded_event(
            &room_id,
            SESSION_OUTPUT,
            &task_event_id,
            serde_json::to_value(&output).unwrap(),
        )
        .await
        .unwrap();

    // Worker posts SessionHeartbeat
    let heartbeat = SessionHeartbeat {
        session_uuid: "e2e-session-001".into(),
        worker_id: worker_mc.user_id().to_string(),
        timestamp: now_secs(),
        progress: Some("running".into()),
    };
    worker_mc
        .send_threaded_event(
            &room_id,
            SESSION_HEARTBEAT,
            &task_event_id,
            serde_json::to_value(&heartbeat).unwrap(),
        )
        .await
        .unwrap();

    // Worker posts SessionResult
    let result = SessionResult {
        session_uuid: "e2e-session-001".into(),
        worker_id: worker_mc.user_id().to_string(),
        status: SessionStatus::Success,
        exit_code: Some(0),
        duration_seconds: 1,
        tail: Some("hello world\n".into()),
    };
    worker_mc
        .send_threaded_event(
            &room_id,
            SESSION_RESULT,
            &task_event_id,
            serde_json::to_value(&result).unwrap(),
        )
        .await
        .unwrap();

    // Worker writes CompletedSessionState
    let completed_state = CompletedSessionState {
        exit_code: Some(0),
        duration_seconds: 1,
        completion_time: now_secs(),
    };
    worker_mc
        .send_state_event(
            &room_id,
            "org.mxdx.session.completed",
            "session/e2e-session-001/completed",
            serde_json::to_value(&completed_state).unwrap(),
        )
        .await
        .unwrap();

    // Client syncs and verifies events
    client_mc.sync_once().await.unwrap();
    let all_events = client_mc
        .sync_and_collect_events(&room_id, Duration::from_secs(5))
        .await
        .unwrap();

    let found_result = all_events.iter().any(|e| {
        e.get("content")
            .and_then(|c| c.get("status"))
            .and_then(|s| s.as_str())
            == Some("success")
    });
    assert!(
        found_result,
        "client should see the result event, got: {:?}",
        all_events
    );

    // Client reads state events to see session status
    let active = client_mc
        .get_room_state_event(
            &room_id,
            "org.mxdx.session.active",
            "session/e2e-session-001/active",
        )
        .await
        .unwrap();
    assert_eq!(active["bin"], "echo");
    assert_eq!(active["worker_id"], worker_mc.user_id().to_string());

    let completed = client_mc
        .get_room_state_event(
            &room_id,
            "org.mxdx.session.completed",
            "session/e2e-session-001/completed",
        )
        .await
        .unwrap();
    assert_eq!(completed["exit_code"], 0);

    hs.stop().await;
}

// ---------------------------------------------------------------------------
// Test 2: Worker info state event (telemetry)
// ---------------------------------------------------------------------------

/// Worker posts WorkerInfo as state event, client can read it.
#[tokio::test]
async fn worker_info_state_event_e2e() {
    let mut hs = TuwunelInstance::start().await.unwrap();
    let base_url = format!("http://127.0.0.1:{}", hs.port);
    let (client_mc, worker_mc, room_id) = setup_encrypted_pair(&base_url).await;

    let info = WorkerInfo {
        worker_id: worker_mc.user_id().to_string(),
        host: "test-host".into(),
        os: "linux".into(),
        arch: "x86_64".into(),
        cpu_count: 8,
        memory_total_mb: 16384,
        disk_available_mb: 50000,
        tools: vec![WorkerTool {
            name: "echo".into(),
            version: Some("1.0".into()),
            description: "Test tool".into(),
            healthy: true,
            input_schema: InputSchema {
                r#type: "object".into(),
                properties: HashMap::from([(
                    "prompt".into(),
                    SchemaProperty {
                        r#type: "string".into(),
                        description: "Task prompt".into(),
                    },
                )]),
                required: vec!["prompt".into()],
            },
        }],
        capabilities: vec!["linux".into(), "rust".into()],
        updated_at: now_secs(),
    };

    let state_key = format!("worker/{}", worker_mc.user_id());
    worker_mc
        .send_state_event(
            &room_id,
            WORKER_INFO,
            &state_key,
            serde_json::to_value(&info).unwrap(),
        )
        .await
        .unwrap();

    // Client reads the worker info
    client_mc.sync_once().await.unwrap();
    let read_info = client_mc
        .get_room_state_event(&room_id, WORKER_INFO, &state_key)
        .await
        .unwrap();

    assert_eq!(read_info["worker_id"], worker_mc.user_id().to_string());
    assert_eq!(read_info["host"], "test-host");
    assert_eq!(read_info["os"], "linux");
    assert_eq!(read_info["arch"], "x86_64");
    assert_eq!(read_info["cpu_count"], 8);
    assert_eq!(read_info["memory_total_mb"], 16384);
    assert_eq!(read_info["capabilities"], serde_json::json!(["linux", "rust"]));

    hs.stop().await;
}

// ---------------------------------------------------------------------------
// Test 3: SessionCancel flow
// ---------------------------------------------------------------------------

/// Client cancels a running session, worker sees cancel event.
#[tokio::test]
async fn session_cancel_flow_e2e() {
    let mut hs = TuwunelInstance::start().await.unwrap();
    let base_url = format!("http://127.0.0.1:{}", hs.port);
    let (client_mc, worker_mc, room_id) = setup_encrypted_pair(&base_url).await;

    // Client submits task
    let task = make_session_task("e2e-cancel-001", &client_mc.user_id().to_string());
    let task_event_id = client_mc
        .send_event(
            &room_id,
            serde_json::json!({
                "type": SESSION_TASK,
                "content": serde_json::to_value(&task).unwrap(),
            }),
        )
        .await
        .unwrap();

    // Worker syncs and starts
    worker_mc.sync_once().await.unwrap();
    let start_event = SessionStart {
        session_uuid: "e2e-cancel-001".into(),
        worker_id: worker_mc.user_id().to_string(),
        tmux_session: Some("mxdx-e2e-cancel-001".into()),
        pid: Some(99999),
        started_at: now_secs(),
    };
    worker_mc
        .send_threaded_event(
            &room_id,
            SESSION_START,
            &task_event_id,
            serde_json::to_value(&start_event).unwrap(),
        )
        .await
        .unwrap();

    // Client posts SessionCancel as threaded event
    let cancel = SessionCancel {
        session_uuid: "e2e-cancel-001".into(),
        reason: Some("user requested abort".into()),
        grace_seconds: Some(5),
    };
    client_mc
        .send_threaded_event(
            &room_id,
            SESSION_CANCEL,
            &task_event_id,
            serde_json::to_value(&cancel).unwrap(),
        )
        .await
        .unwrap();

    // Worker syncs and sees the cancel event
    worker_mc.sync_once().await.unwrap();
    let events = worker_mc
        .sync_and_collect_events(&room_id, Duration::from_secs(5))
        .await
        .unwrap();

    let found_cancel = events.iter().any(|e| {
        e.get("content")
            .and_then(|c| c.get("session_uuid"))
            .and_then(|u| u.as_str())
            == Some("e2e-cancel-001")
            && e.get("content")
                .and_then(|c| c.get("reason"))
                .and_then(|r| r.as_str())
                == Some("user requested abort")
    });
    assert!(
        found_cancel,
        "worker should see the cancel event, got: {:?}",
        events
    );

    // Worker posts cancelled result
    let result = SessionResult {
        session_uuid: "e2e-cancel-001".into(),
        worker_id: worker_mc.user_id().to_string(),
        status: SessionStatus::Cancelled,
        exit_code: None,
        duration_seconds: 2,
        tail: None,
    };
    worker_mc
        .send_threaded_event(
            &room_id,
            SESSION_RESULT,
            &task_event_id,
            serde_json::to_value(&result).unwrap(),
        )
        .await
        .unwrap();

    // Client sees cancelled result
    client_mc.sync_once().await.unwrap();
    let all_events = client_mc
        .sync_and_collect_events(&room_id, Duration::from_secs(5))
        .await
        .unwrap();
    let found_cancelled = all_events.iter().any(|e| {
        e.get("content")
            .and_then(|c| c.get("status"))
            .and_then(|s| s.as_str())
            == Some("cancelled")
    });
    assert!(
        found_cancelled,
        "client should see the cancelled result, got: {:?}",
        all_events
    );

    hs.stop().await;
}

// ---------------------------------------------------------------------------
// Test 4: Multiple concurrent sessions with state events as process table
// ---------------------------------------------------------------------------

/// Two sessions running simultaneously, both tracked via state events.
#[tokio::test]
async fn concurrent_sessions_state_table_e2e() {
    let mut hs = TuwunelInstance::start().await.unwrap();
    let base_url = format!("http://127.0.0.1:{}", hs.port);
    let (client_mc, worker_mc, room_id) = setup_encrypted_pair(&base_url).await;

    // Client submits two tasks
    let task_a = make_session_task("e2e-concurrent-a", &client_mc.user_id().to_string());
    let task_b = SessionTask {
        uuid: "e2e-concurrent-b".into(),
        bin: "ls".into(),
        args: vec!["-la".into()],
        ..make_session_task("e2e-concurrent-b", &client_mc.user_id().to_string())
    };

    client_mc
        .send_event(
            &room_id,
            serde_json::json!({
                "type": SESSION_TASK,
                "content": serde_json::to_value(&task_a).unwrap(),
            }),
        )
        .await
        .unwrap();

    client_mc
        .send_event(
            &room_id,
            serde_json::json!({
                "type": SESSION_TASK,
                "content": serde_json::to_value(&task_b).unwrap(),
            }),
        )
        .await
        .unwrap();

    // Worker claims both, writes active state for both
    let active_a = ActiveSessionState {
        bin: "echo".into(),
        args: vec!["hello".into(), "world".into()],
        pid: Some(1001),
        start_time: now_secs(),
        client_id: client_mc.user_id().to_string(),
        interactive: false,
        worker_id: worker_mc.user_id().to_string(),
    };
    let active_b = ActiveSessionState {
        bin: "ls".into(),
        args: vec!["-la".into()],
        pid: Some(1002),
        start_time: now_secs(),
        client_id: client_mc.user_id().to_string(),
        interactive: false,
        worker_id: worker_mc.user_id().to_string(),
    };

    worker_mc
        .send_state_event(
            &room_id,
            "org.mxdx.session.active",
            "session/e2e-concurrent-a/active",
            serde_json::to_value(&active_a).unwrap(),
        )
        .await
        .unwrap();
    worker_mc
        .send_state_event(
            &room_id,
            "org.mxdx.session.active",
            "session/e2e-concurrent-b/active",
            serde_json::to_value(&active_b).unwrap(),
        )
        .await
        .unwrap();

    // Client reads state events — sees two active sessions
    client_mc.sync_once().await.unwrap();
    let state_a = client_mc
        .get_room_state_event(
            &room_id,
            "org.mxdx.session.active",
            "session/e2e-concurrent-a/active",
        )
        .await
        .unwrap();
    let state_b = client_mc
        .get_room_state_event(
            &room_id,
            "org.mxdx.session.active",
            "session/e2e-concurrent-b/active",
        )
        .await
        .unwrap();
    assert_eq!(state_a["bin"], "echo");
    assert_eq!(state_a["pid"], 1001);
    assert_eq!(state_b["bin"], "ls");
    assert_eq!(state_b["pid"], 1002);

    // Worker completes first session
    let completed_a = CompletedSessionState {
        exit_code: Some(0),
        duration_seconds: 1,
        completion_time: now_secs(),
    };
    worker_mc
        .send_state_event(
            &room_id,
            "org.mxdx.session.completed",
            "session/e2e-concurrent-a/completed",
            serde_json::to_value(&completed_a).unwrap(),
        )
        .await
        .unwrap();

    // Client reads — first completed, second still active
    client_mc.sync_once().await.unwrap();
    let completed = client_mc
        .get_room_state_event(
            &room_id,
            "org.mxdx.session.completed",
            "session/e2e-concurrent-a/completed",
        )
        .await
        .unwrap();
    assert_eq!(completed["exit_code"], 0);

    // Second still active
    let still_active = client_mc
        .get_room_state_event(
            &room_id,
            "org.mxdx.session.active",
            "session/e2e-concurrent-b/active",
        )
        .await
        .unwrap();
    assert_eq!(still_active["bin"], "ls");

    // Worker completes second session
    let completed_b = CompletedSessionState {
        exit_code: Some(0),
        duration_seconds: 2,
        completion_time: now_secs(),
    };
    worker_mc
        .send_state_event(
            &room_id,
            "org.mxdx.session.completed",
            "session/e2e-concurrent-b/completed",
            serde_json::to_value(&completed_b).unwrap(),
        )
        .await
        .unwrap();

    client_mc.sync_once().await.unwrap();
    let completed_b_state = client_mc
        .get_room_state_event(
            &room_id,
            "org.mxdx.session.completed",
            "session/e2e-concurrent-b/completed",
        )
        .await
        .unwrap();
    assert_eq!(completed_b_state["exit_code"], 0);
    assert_eq!(completed_b_state["duration_seconds"], 2);

    hs.stop().await;
}

// ---------------------------------------------------------------------------
// Test 5: E2EE verification — events are encrypted on the wire
// ---------------------------------------------------------------------------

/// Verify that session events are E2EE encrypted.
/// We send a session event in an encrypted room, then verify the client
/// can read it back (proving decryption works). The room was created with
/// encryption enabled, so all events go through megolm.
#[tokio::test]
async fn session_events_are_encrypted_e2e() {
    let mut hs = TuwunelInstance::start().await.unwrap();
    let base_url = format!("http://127.0.0.1:{}", hs.port);
    let (client_mc, worker_mc, room_id) = setup_encrypted_pair(&base_url).await;

    // Send a session output event (contains sensitive data)
    let task = make_session_task("e2e-encrypted-001", &client_mc.user_id().to_string());
    let task_event_id = client_mc
        .send_event(
            &room_id,
            serde_json::json!({
                "type": SESSION_TASK,
                "content": serde_json::to_value(&task).unwrap(),
            }),
        )
        .await
        .unwrap();

    let output = SessionOutput {
        session_uuid: "e2e-encrypted-001".into(),
        worker_id: worker_mc.user_id().to_string(),
        stream: OutputStream::Stdout,
        data: base64::engine::general_purpose::STANDARD.encode(b"SECRET_DATA_12345"),
        seq: 0,
        timestamp: now_secs(),
    };
    worker_mc
        .send_threaded_event(
            &room_id,
            SESSION_OUTPUT,
            &task_event_id,
            serde_json::to_value(&output).unwrap(),
        )
        .await
        .unwrap();

    // Client syncs and uses the decrypting API to read the output
    client_mc.sync_once().await.unwrap();
    let events = client_mc
        .sync_and_collect_events(&room_id, Duration::from_secs(5))
        .await
        .unwrap();

    let found_output = events.iter().any(|e| {
        e.get("content")
            .and_then(|c| c.get("data"))
            .and_then(|d| d.as_str())
            .map(|d| {
                let decoded = base64::engine::general_purpose::STANDARD.decode(d).unwrap();
                decoded == b"SECRET_DATA_12345"
            })
            .unwrap_or(false)
    });
    assert!(
        found_output,
        "client should decrypt and see the output event data, got: {:?}",
        events
    );

    // Verify the room has encryption enabled (sanity check)
    let encryption_state = client_mc
        .get_room_state_event(&room_id, "m.room.encryption", "")
        .await;
    assert!(
        encryption_state.is_ok(),
        "room should have m.room.encryption state event"
    );

    hs.stop().await;
}

// ---------------------------------------------------------------------------
// Test 6: Interactive session flag propagates
// ---------------------------------------------------------------------------

/// Client submits interactive task, worker sees interactive=true.
#[tokio::test]
async fn interactive_session_flag_e2e() {
    let mut hs = TuwunelInstance::start().await.unwrap();
    let base_url = format!("http://127.0.0.1:{}", hs.port);
    let (client_mc, worker_mc, room_id) = setup_encrypted_pair(&base_url).await;

    let task = SessionTask {
        uuid: "e2e-interactive-001".into(),
        sender_id: client_mc.user_id().to_string(),
        bin: "bash".into(),
        args: vec![],
        env: None,
        cwd: Some("/home/user".into()),
        interactive: true,
        no_room_output: false,
        timeout_seconds: Some(300),
        heartbeat_interval_seconds: 15,
        plan: None,
        required_capabilities: vec!["linux".into()],
        routing_mode: None,
        on_timeout: None,
        on_heartbeat_miss: None,
    };

    client_mc
        .send_event(
            &room_id,
            serde_json::json!({
                "type": SESSION_TASK,
                "content": serde_json::to_value(&task).unwrap(),
            }),
        )
        .await
        .unwrap();

    // Worker syncs and sees the task with interactive=true
    worker_mc.sync_once().await.unwrap();
    let events = worker_mc
        .sync_and_collect_events(&room_id, Duration::from_secs(5))
        .await
        .unwrap();

    let task_event = events
        .iter()
        .find(|e| {
            e.get("content")
                .and_then(|c| c.get("uuid"))
                .and_then(|u| u.as_str())
                == Some("e2e-interactive-001")
        })
        .expect("worker should see the interactive task event");

    let content = &task_event["content"];
    assert_eq!(
        content["interactive"], true,
        "interactive flag should be true"
    );
    assert_eq!(content["bin"], "bash");
    assert_eq!(content["cwd"], "/home/user");
    assert_eq!(content["heartbeat_interval_seconds"], 15);
    assert_eq!(
        content["required_capabilities"],
        serde_json::json!(["linux"])
    );

    // Deserialize the content back to a SessionTask to verify full roundtrip
    let received_task: SessionTask =
        serde_json::from_value(content.clone()).unwrap();
    assert!(received_task.interactive);
    assert_eq!(received_task.cwd, Some("/home/user".into()));
    assert_eq!(received_task.heartbeat_interval_seconds, 15);

    hs.stop().await;
}

// ---------------------------------------------------------------------------
// Test 7: no_room_output suppression
// ---------------------------------------------------------------------------

/// With no_room_output=true, the worker posts result directly without output events.
/// Client gets result but no output events in the thread.
#[tokio::test]
async fn no_room_output_flag_e2e() {
    let mut hs = TuwunelInstance::start().await.unwrap();
    let base_url = format!("http://127.0.0.1:{}", hs.port);
    let (client_mc, worker_mc, room_id) = setup_encrypted_pair(&base_url).await;

    let task = SessionTask {
        uuid: "e2e-noro-001".into(),
        sender_id: client_mc.user_id().to_string(),
        bin: "echo".into(),
        args: vec!["suppressed".into()],
        env: None,
        cwd: None,
        interactive: false,
        no_room_output: true,
        timeout_seconds: Some(60),
        heartbeat_interval_seconds: 30,
        plan: None,
        required_capabilities: vec![],
        routing_mode: None,
        on_timeout: None,
        on_heartbeat_miss: None,
    };

    let task_event_id = client_mc
        .send_event(
            &room_id,
            serde_json::json!({
                "type": SESSION_TASK,
                "content": serde_json::to_value(&task).unwrap(),
            }),
        )
        .await
        .unwrap();

    // Worker syncs and sees the no_room_output flag
    worker_mc.sync_once().await.unwrap();
    let events = worker_mc
        .sync_and_collect_events(&room_id, Duration::from_secs(5))
        .await
        .unwrap();
    let task_content = events
        .iter()
        .find(|e| {
            e.get("content")
                .and_then(|c| c.get("uuid"))
                .and_then(|u| u.as_str())
                == Some("e2e-noro-001")
        })
        .expect("worker should see the task");
    assert_eq!(
        task_content["content"]["no_room_output"], true,
        "no_room_output flag should be true"
    );

    // Worker respects no_room_output — posts result only (no output events)
    let result = SessionResult {
        session_uuid: "e2e-noro-001".into(),
        worker_id: worker_mc.user_id().to_string(),
        status: SessionStatus::Success,
        exit_code: Some(0),
        duration_seconds: 1,
        tail: Some("suppressed\n".into()),
    };
    worker_mc
        .send_threaded_event(
            &room_id,
            SESSION_RESULT,
            &task_event_id,
            serde_json::to_value(&result).unwrap(),
        )
        .await
        .unwrap();

    // Client sees result but no output events
    client_mc.sync_once().await.unwrap();
    let all_events = client_mc
        .sync_and_collect_events(&room_id, Duration::from_secs(5))
        .await
        .unwrap();

    let has_result = all_events.iter().any(|e| {
        e.get("content")
            .and_then(|c| c.get("status"))
            .and_then(|s| s.as_str())
            == Some("success")
    });
    assert!(has_result, "client should see the result event");

    // Verify no output events were posted in the room
    let has_output = all_events.iter().any(|e| {
        e.get("content")
            .and_then(|c| c.get("stream"))
            .is_some()
    });
    assert!(
        !has_output,
        "no output events should exist when no_room_output is true, got: {:?}",
        all_events
    );

    hs.stop().await;
}

// ---------------------------------------------------------------------------
// Test 8: Backward compatibility — old fabric task events
// ---------------------------------------------------------------------------

/// Old org.mxdx.fabric.task events can be parsed and translated via compat module.
#[tokio::test]
async fn backward_compat_fabric_task_e2e() {
    let mut hs = TuwunelInstance::start().await.unwrap();
    let base_url = format!("http://127.0.0.1:{}", hs.port);
    let (client_mc, worker_mc, room_id) = setup_encrypted_pair(&base_url).await;

    // Client sends old-format TaskEvent
    let legacy_task = TaskEvent {
        uuid: "e2e-compat-001".into(),
        sender_id: client_mc.user_id().to_string(),
        required_capabilities: vec!["linux".into(), "rust".into()],
        estimated_cycles: None,
        timeout_seconds: 120,
        heartbeat_interval_seconds: 15,
        on_timeout: FailurePolicy::Escalate,
        on_heartbeat_miss: FailurePolicy::Abandon,
        routing_mode: RoutingMode::Auto,
        p2p_stream: true,
        payload: serde_json::json!({"cmd": "cargo build --release"}),
        plan: Some("Build the project".into()),
    };

    client_mc
        .send_event(
            &room_id,
            serde_json::json!({
                "type": "org.mxdx.fabric.task",
                "content": serde_json::to_value(&legacy_task).unwrap(),
            }),
        )
        .await
        .unwrap();

    // Worker syncs and receives the legacy event
    worker_mc.sync_once().await.unwrap();
    let events = worker_mc
        .sync_and_collect_events(&room_id, Duration::from_secs(5))
        .await
        .unwrap();

    let legacy_event = events
        .iter()
        .find(|e| {
            e.get("content")
                .and_then(|c| c.get("uuid"))
                .and_then(|u| u.as_str())
                == Some("e2e-compat-001")
        })
        .expect("worker should see the legacy task event");

    // Worker deserializes as TaskEvent and translates to SessionTask
    let received_legacy: TaskEvent =
        serde_json::from_value(legacy_event["content"].clone()).unwrap();
    assert_eq!(received_legacy.uuid, "e2e-compat-001");
    assert!(received_legacy.p2p_stream);

    let session_task = translate_legacy_task(&received_legacy);

    // Verify translation
    assert_eq!(session_task.uuid, "e2e-compat-001");
    assert_eq!(session_task.bin, "cargo");
    assert_eq!(session_task.args, vec!["build", "--release"]);
    assert!(session_task.interactive); // p2p_stream -> interactive
    assert!(!session_task.no_room_output); // default
    assert_eq!(session_task.timeout_seconds, Some(120));
    assert_eq!(session_task.heartbeat_interval_seconds, 15);
    assert_eq!(session_task.plan, Some("Build the project".into()));
    assert_eq!(
        session_task.required_capabilities,
        vec!["linux", "rust"]
    );
    assert_eq!(session_task.routing_mode, Some(RoutingMode::Auto));
    assert_eq!(session_task.on_timeout, Some(FailurePolicy::Escalate));
    assert_eq!(
        session_task.on_heartbeat_miss,
        Some(FailurePolicy::Abandon)
    );

    hs.stop().await;
}
