//! End-to-end tests for the mxdx-client crate using a real Tuwunel Matrix homeserver.
//!
//! These tests validate that the client's public API (submit, tail, ls, logs, cancel,
//! reconnect) works correctly when events flow through a real Matrix room with E2EE,
//! threaded events, and state events.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use mxdx_matrix::{MatrixClient, OwnedRoomId};
use mxdx_test_helpers::tuwunel::TuwunelInstance;
use mxdx_types::events::session::{
    ActiveSessionState, OutputStream, SessionOutput, SESSION_CANCEL, SESSION_OUTPUT, SESSION_TASK,
};

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
async fn setup_encrypted_pair(base_url: &str) -> (MatrixClient, MatrixClient, OwnedRoomId) {
    let client_mc =
        MatrixClient::register_and_connect(base_url, "client-user", "pass123", "mxdx-test-token")
            .await
            .unwrap();
    let worker_mc =
        MatrixClient::register_and_connect(base_url, "worker-user", "pass123", "mxdx-test-token")
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

    // Exchange E2EE keys -- 4 sync rounds
    for _ in 0..4 {
        client_mc.sync_once().await.unwrap();
        worker_mc.sync_once().await.unwrap();
    }

    (client_mc, worker_mc, room_id)
}

// ---------------------------------------------------------------------------
// Test 1: Client submits task, worker sees it
// ---------------------------------------------------------------------------

/// Client uses submit::build_task() to create a SessionTask, sends it to the room,
/// and the worker syncs and receives the task with matching uuid, bin, and args.
#[tokio::test]
async fn client_submit_worker_receives_e2e() {
    let mut hs = TuwunelInstance::start().await.unwrap();
    let base_url = format!("http://127.0.0.1:{}", hs.port);
    let (client_mc, worker_mc, room_id) = setup_encrypted_pair(&base_url).await;

    // Client builds a task using the public API
    let task = mxdx_client::submit::build_task(
        "echo",
        &["hello".to_string(), "world".to_string()],
        false,
        false,
        Some(60),
        30,
        &client_mc.user_id().to_string(),
    );

    let task_uuid = task.uuid.clone();
    let task_json = serde_json::to_value(&task).unwrap();

    // Client sends the task as thread root
    client_mc
        .send_event(
            &room_id,
            serde_json::json!({
                "type": SESSION_TASK,
                "content": task_json,
            }),
        )
        .await
        .unwrap();

    // Worker syncs and receives the task
    worker_mc.sync_once().await.unwrap();
    let events = worker_mc
        .sync_and_collect_events(&room_id, Duration::from_secs(5))
        .await
        .unwrap();

    let found_task = events.iter().find(|e| {
        e.get("content")
            .and_then(|c| c.get("uuid"))
            .and_then(|u| u.as_str())
            == Some(&task_uuid)
    });
    assert!(
        found_task.is_some(),
        "worker should see the task event, got: {:?}",
        events
    );

    let content = &found_task.unwrap()["content"];
    assert_eq!(content["bin"], "echo");
    assert_eq!(content["args"], serde_json::json!(["hello", "world"]));
    assert_eq!(content["interactive"], false);
    assert_eq!(content["heartbeat_interval_seconds"], 30);

    hs.stop().await;
}

// ---------------------------------------------------------------------------
// Test 2: Client tails worker output
// ---------------------------------------------------------------------------

/// Worker sends SessionOutput (base64 encoded) as a threaded event.
/// Client syncs, collects events, and uses tail::decode_output() / tail::format_output()
/// to verify the decoded output matches the original data.
#[tokio::test]
async fn client_tail_output_e2e() {
    let mut hs = TuwunelInstance::start().await.unwrap();
    let base_url = format!("http://127.0.0.1:{}", hs.port);
    let (client_mc, worker_mc, room_id) = setup_encrypted_pair(&base_url).await;

    // Client submits a task (thread root)
    let task = mxdx_client::submit::build_task(
        "echo",
        &["tail-test".to_string()],
        false,
        false,
        None,
        30,
        &client_mc.user_id().to_string(),
    );
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

    // Worker posts SessionOutput as threaded event
    let original_data = "hello from tail test\n";
    let output = SessionOutput {
        session_uuid: task.uuid.clone(),
        worker_id: worker_mc.user_id().to_string(),
        stream: OutputStream::Stdout,
        data: BASE64.encode(original_data.as_bytes()),
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

    // Client syncs and collects events
    client_mc.sync_once().await.unwrap();
    let events = client_mc
        .sync_and_collect_events(&room_id, Duration::from_secs(5))
        .await
        .unwrap();

    // Find the output event and use tail module to decode it
    let output_event = events
        .iter()
        .find(|e| {
            e.get("content")
                .and_then(|c| c.get("stream"))
                .is_some()
        })
        .expect("client should see the output event");

    let received_output: SessionOutput =
        serde_json::from_value(output_event["content"].clone()).unwrap();

    // Use the client tail module to decode
    let decoded = mxdx_client::tail::decode_output(&received_output).unwrap();
    assert_eq!(decoded, original_data.as_bytes());

    let formatted = mxdx_client::tail::format_output(&received_output).unwrap();
    assert_eq!(formatted, original_data);

    hs.stop().await;
}

// ---------------------------------------------------------------------------
// Test 3: Client reads session list from state events
// ---------------------------------------------------------------------------

/// Worker writes 2 ActiveSessionState events. Client reads state events and uses
/// ls::from_active() to build entries, then ls::format_table() to render.
#[tokio::test]
async fn client_ls_state_events_e2e() {
    let mut hs = TuwunelInstance::start().await.unwrap();
    let base_url = format!("http://127.0.0.1:{}", hs.port);
    let (client_mc, worker_mc, room_id) = setup_encrypted_pair(&base_url).await;

    // Worker writes 2 ActiveSessionState events
    let active_1 = ActiveSessionState {
        bin: "echo".into(),
        args: vec!["hello".into()],
        pid: Some(1001),
        start_time: now_secs(),
        client_id: client_mc.user_id().to_string(),
        interactive: false,
        worker_id: worker_mc.user_id().to_string(),
    };
    let active_2 = ActiveSessionState {
        bin: "ls".into(),
        args: vec!["-la".into()],
        pid: Some(1002),
        start_time: now_secs(),
        client_id: client_mc.user_id().to_string(),
        interactive: true,
        worker_id: worker_mc.user_id().to_string(),
    };

    worker_mc
        .send_state_event(
            &room_id,
            "org.mxdx.session.active",
            "session/ls-test-001/active",
            serde_json::to_value(&active_1).unwrap(),
        )
        .await
        .unwrap();
    worker_mc
        .send_state_event(
            &room_id,
            "org.mxdx.session.active",
            "session/ls-test-002/active",
            serde_json::to_value(&active_2).unwrap(),
        )
        .await
        .unwrap();

    // Client reads state events
    client_mc.sync_once().await.unwrap();
    let state_1 = client_mc
        .get_room_state_event(
            &room_id,
            "org.mxdx.session.active",
            "session/ls-test-001/active",
        )
        .await
        .unwrap();
    let state_2 = client_mc
        .get_room_state_event(
            &room_id,
            "org.mxdx.session.active",
            "session/ls-test-002/active",
        )
        .await
        .unwrap();

    // Deserialize and use ls module
    let parsed_1: ActiveSessionState = serde_json::from_value(state_1).unwrap();
    let parsed_2: ActiveSessionState = serde_json::from_value(state_2).unwrap();

    let entries = vec![
        mxdx_client::ls::from_active("ls-test-001".into(), &parsed_1),
        mxdx_client::ls::from_active("ls-test-002".into(), &parsed_2),
    ];

    let table = mxdx_client::ls::format_table(&entries);
    assert!(table.contains("UUID"), "table should have header");
    assert!(table.contains("ls-test-001"), "table should show session 1");
    assert!(table.contains("ls-test-002"), "table should show session 2");
    assert!(table.contains("active"), "table should show active status");

    hs.stop().await;
}

// ---------------------------------------------------------------------------
// Test 4: Client reads logs from thread
// ---------------------------------------------------------------------------

/// Worker posts 3 SessionOutput events (seq 0, 1, 2) as threaded events.
/// Client collects all thread events and uses logs::reassemble_output() to
/// combine them. Verifies reassembled output matches original data in order.
#[tokio::test]
async fn client_logs_reassemble_e2e() {
    let mut hs = TuwunelInstance::start().await.unwrap();
    let base_url = format!("http://127.0.0.1:{}", hs.port);
    let (client_mc, worker_mc, room_id) = setup_encrypted_pair(&base_url).await;

    // Client submits a task (thread root)
    let task = mxdx_client::submit::build_task(
        "seq",
        &["3".to_string()],
        false,
        false,
        None,
        30,
        &client_mc.user_id().to_string(),
    );
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

    // Worker posts 3 output events (deliberately out of order to test reassembly)
    let chunks = vec![
        ("line2\n", 1u64),
        ("line1\n", 0u64),
        ("line3\n", 2u64),
    ];
    for (data, seq) in &chunks {
        let output = SessionOutput {
            session_uuid: task.uuid.clone(),
            worker_id: worker_mc.user_id().to_string(),
            stream: OutputStream::Stdout,
            data: BASE64.encode(data.as_bytes()),
            seq: *seq,
            timestamp: now_secs() + seq,
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
    }

    // Client syncs and collects output events
    client_mc.sync_once().await.unwrap();
    let events = client_mc
        .sync_and_collect_events(&room_id, Duration::from_secs(5))
        .await
        .unwrap();

    // Collect all output events and parse them
    let output_events: Vec<SessionOutput> = events
        .iter()
        .filter_map(|e| {
            let content = e.get("content")?;
            if content.get("stream").is_some() {
                serde_json::from_value(content.clone()).ok()
            } else {
                None
            }
        })
        .collect();

    assert_eq!(
        output_events.len(),
        3,
        "should have 3 output events, got: {}",
        output_events.len()
    );

    // Use the logs module to reassemble
    let reassembled = mxdx_client::logs::reassemble_output_string(output_events).unwrap();
    assert_eq!(
        reassembled, "line1\nline2\nline3\n",
        "logs::reassemble_output should order by seq"
    );

    hs.stop().await;
}

// ---------------------------------------------------------------------------
// Test 5: Client sends cancel, worker receives
// ---------------------------------------------------------------------------

/// Client submits a task, then uses cancel::build_cancel() to send a cancel event
/// as a threaded event. Worker syncs and sees the cancel with matching session_uuid.
#[tokio::test]
async fn client_cancel_worker_receives_e2e() {
    let mut hs = TuwunelInstance::start().await.unwrap();
    let base_url = format!("http://127.0.0.1:{}", hs.port);
    let (client_mc, worker_mc, room_id) = setup_encrypted_pair(&base_url).await;

    // Client submits task
    let task = mxdx_client::submit::build_task(
        "sleep",
        &["300".to_string()],
        false,
        false,
        Some(600),
        30,
        &client_mc.user_id().to_string(),
    );
    let task_uuid = task.uuid.clone();
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

    // Worker syncs to see the task
    worker_mc.sync_once().await.unwrap();
    worker_mc
        .sync_and_collect_events(&room_id, Duration::from_secs(5))
        .await
        .unwrap();

    // Client builds and sends cancel using the cancel module
    let cancel = mxdx_client::cancel::build_cancel(
        &task_uuid,
        Some("user requested abort".into()),
        Some(5),
    );
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
            == Some(&task_uuid)
            && e.get("content")
                .and_then(|c| c.get("reason"))
                .and_then(|r| r.as_str())
                == Some("user requested abort")
    });
    assert!(
        found_cancel,
        "worker should see the cancel event with session_uuid={}, got: {:?}",
        task_uuid, events
    );

    hs.stop().await;
}

// ---------------------------------------------------------------------------
// Test 6: Reconnect finds client's own sessions
// ---------------------------------------------------------------------------

/// Worker writes 2 ActiveSessionState events -- one with client's user_id as
/// client_id, one with a different user. Client reads state, uses
/// reconnect::find_reconnectable_sessions() and verifies only its own session is found.
#[tokio::test]
async fn client_reconnect_finds_own_sessions_e2e() {
    let mut hs = TuwunelInstance::start().await.unwrap();
    let base_url = format!("http://127.0.0.1:{}", hs.port);
    let (client_mc, worker_mc, room_id) = setup_encrypted_pair(&base_url).await;

    let client_id = client_mc.user_id().to_string();
    let worker_id = worker_mc.user_id().to_string();

    // Worker writes session owned by client
    let my_session = ActiveSessionState {
        bin: "echo".into(),
        args: vec!["mine".into()],
        pid: Some(2001),
        start_time: now_secs(),
        client_id: client_id.clone(),
        interactive: false,
        worker_id: worker_id.clone(),
    };

    // Worker writes session owned by someone else
    let other_session = ActiveSessionState {
        bin: "ls".into(),
        args: vec![],
        pid: Some(2002),
        start_time: now_secs(),
        client_id: "@someone-else:test.localhost".into(),
        interactive: false,
        worker_id: worker_id.clone(),
    };

    worker_mc
        .send_state_event(
            &room_id,
            "org.mxdx.session.active",
            "session/reconnect-mine/active",
            serde_json::to_value(&my_session).unwrap(),
        )
        .await
        .unwrap();
    worker_mc
        .send_state_event(
            &room_id,
            "org.mxdx.session.active",
            "session/reconnect-other/active",
            serde_json::to_value(&other_session).unwrap(),
        )
        .await
        .unwrap();

    // Client reads state events
    client_mc.sync_once().await.unwrap();
    let state_mine = client_mc
        .get_room_state_event(
            &room_id,
            "org.mxdx.session.active",
            "session/reconnect-mine/active",
        )
        .await
        .unwrap();
    let state_other = client_mc
        .get_room_state_event(
            &room_id,
            "org.mxdx.session.active",
            "session/reconnect-other/active",
        )
        .await
        .unwrap();

    let parsed_mine: ActiveSessionState = serde_json::from_value(state_mine).unwrap();
    let parsed_other: ActiveSessionState = serde_json::from_value(state_other).unwrap();

    // Use reconnect module to filter
    let all_sessions = vec![
        ("reconnect-mine".to_string(), parsed_mine),
        ("reconnect-other".to_string(), parsed_other),
    ];

    let reconnectable =
        mxdx_client::reconnect::find_reconnectable_sessions(&all_sessions, &client_id);

    assert_eq!(
        reconnectable.len(),
        1,
        "should find exactly 1 reconnectable session, got: {}",
        reconnectable.len()
    );
    assert_eq!(reconnectable[0].0, "reconnect-mine");
    assert_eq!(reconnectable[0].1.bin, "echo");
    assert_eq!(reconnectable[0].1.client_id, client_id);

    // Verify the format function works
    let formatted = mxdx_client::reconnect::format_reconnectable(&reconnectable);
    assert!(
        formatted.contains("reconnect-mine"),
        "formatted output should contain the session UUID"
    );
    assert!(
        formatted.contains("echo"),
        "formatted output should contain the command"
    );

    hs.stop().await;
}
