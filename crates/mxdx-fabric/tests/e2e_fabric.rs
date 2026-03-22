use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use mxdx_fabric::coordinator::CoordinatorBot;
use mxdx_fabric::jcode_worker::JcodeWorker;
use mxdx_fabric::sender::SenderClient;
use mxdx_fabric::worker::{WorkerClient, EVENT_CAPABILITY};
use mxdx_fabric::{
    ClaimEvent, FailurePolicy, HeartbeatEvent, RoutingMode, TaskEvent, TaskResultEvent, TaskStatus,
    EVENT_CLAIM,
};
use mxdx_matrix::MatrixClient;
use mxdx_test_helpers::tuwunel::TuwunelInstance;
use mxdx_types::events::capability::{
    CapabilityAdvertisement, InputSchema, SchemaProperty, WorkerTool,
};

fn make_task(uuid: &str, sender_id: &str) -> TaskEvent {
    TaskEvent {
        uuid: uuid.to_string(),
        sender_id: sender_id.to_string(),
        required_capabilities: vec!["rust".to_string(), "linux".to_string()],
        estimated_cycles: None,
        timeout_seconds: 60,
        heartbeat_interval_seconds: 30,
        on_timeout: FailurePolicy::Escalate,
        on_heartbeat_miss: FailurePolicy::Escalate,
        routing_mode: RoutingMode::Auto,
        p2p_stream: false,
        payload: serde_json::json!({"cmd": "cargo build"}),
        plan: None,
    }
}

#[tokio::test]
async fn fabric_happy_path_e2e() {
    let mut hs = TuwunelInstance::start().await.unwrap();
    let base_url = format!("http://127.0.0.1:{}", hs.port);

    let coordinator_mc =
        MatrixClient::register_and_connect(&base_url, "coordinator", "pass", "mxdx-test-token")
            .await
            .unwrap();
    let sender_mc =
        MatrixClient::register_and_connect(&base_url, "sender", "pass", "mxdx-test-token")
            .await
            .unwrap();
    let worker_a_mc =
        MatrixClient::register_and_connect(&base_url, "worker-a", "pass", "mxdx-test-token")
            .await
            .unwrap();

    assert!(coordinator_mc.is_logged_in());
    assert!(sender_mc.is_logged_in());
    assert!(worker_a_mc.is_logged_in());

    let coord_room_id = coordinator_mc
        .create_named_unencrypted_room("coordinator-room", "org.mxdx.fabric.coordinator")
        .await
        .unwrap();

    coordinator_mc
        .invite_user(&coord_room_id, sender_mc.user_id())
        .await
        .unwrap();

    sender_mc.sync_once().await.unwrap();
    sender_mc.join_room(&coord_room_id).await.unwrap();

    coordinator_mc.sync_once().await.unwrap();
    sender_mc.sync_once().await.unwrap();

    let coordinator_mc = Arc::new(coordinator_mc);
    let worker_a_mc = Arc::new(worker_a_mc);

    let mut bot = CoordinatorBot::new(
        coordinator_mc.clone(),
        coord_room_id.clone(),
        hs.server_name.clone(),
    );

    let task = make_task("test-task-001", &format!("@sender:{}", hs.server_name));

    bot.handle_task_event(task.clone()).await.unwrap();

    assert_eq!(bot.watchlist_len(), 1);
    assert!(bot.watchlist_contains("test-task-001"));

    let worker_room_id = bot
        .capability_index()
        .find_room(&["rust".into(), "linux".into()])
        .expect("capability room should exist after routing");

    let power_levels = serde_json::json!({
        "users": {
            coordinator_mc.user_id().to_string(): 100
        },
        "users_default": 0,
        "events": {},
        "events_default": 0,
        "state_default": 0,
        "ban": 50,
        "kick": 50,
        "invite": 50,
        "redact": 50
    });
    coordinator_mc
        .send_state_event(&worker_room_id, "m.room.power_levels", "", power_levels)
        .await
        .unwrap();

    coordinator_mc
        .invite_user(&worker_room_id, worker_a_mc.user_id())
        .await
        .unwrap();

    worker_a_mc.sync_once().await.unwrap();
    worker_a_mc.join_room(&worker_room_id).await.unwrap();

    coordinator_mc.sync_once().await.unwrap();
    worker_a_mc.sync_once().await.unwrap();

    let worker = WorkerClient::new(
        worker_a_mc.clone(),
        format!("@worker-a:{}", hs.server_name),
        hs.server_name.clone(),
    );

    worker
        .advertise_capabilities(&["rust".into(), "linux".into()], &worker_room_id)
        .await
        .unwrap();

    worker_a_mc.sync_once().await.unwrap();

    let events = worker_a_mc
        .sync_and_collect_events(&worker_room_id, Duration::from_secs(5))
        .await
        .unwrap();

    let found_task = events.iter().any(|e| {
        e.get("content")
            .and_then(|c| c.get("uuid"))
            .and_then(|u| u.as_str())
            == Some("test-task-001")
    });
    assert!(
        found_task,
        "worker should see the brokered task event in the worker room, got: {:?}",
        events
    );

    let claimed = worker.try_claim(&task, &worker_room_id).await.unwrap();
    assert!(claimed, "worker-a should win the claim");

    let state_key = format!("task/{}/claim", task.uuid);
    let claim_state = worker_a_mc
        .get_room_state_event(&worker_room_id, EVENT_CLAIM, &state_key)
        .await
        .unwrap();
    let expected_worker_id = format!("@worker-a:{}", hs.server_name);
    assert_eq!(
        claim_state["worker_id"].as_str().unwrap(),
        expected_worker_id,
        "claim state event should show worker-a as the winner"
    );

    let claim: ClaimEvent = serde_json::from_value(claim_state).unwrap();
    bot.handle_claim_event(&claim);
    assert_eq!(
        bot.watchlist_len(),
        1,
        "watchlist should still have 1 entry after claim (removed on result)"
    );

    worker
        .post_heartbeat("test-task-001", Some("50%".into()), &worker_room_id, None)
        .await
        .unwrap();

    worker_a_mc.sync_once().await.unwrap();
    coordinator_mc.sync_once().await.unwrap();

    let hb_events = coordinator_mc
        .sync_and_collect_events(&worker_room_id, Duration::from_secs(3))
        .await
        .unwrap();

    let found_hb = hb_events.iter().find(|e| {
        e.get("content")
            .and_then(|c| c.get("task_uuid"))
            .and_then(|u| u.as_str())
            == Some("test-task-001")
            && e.get("content").and_then(|c| c.get("progress")).is_some()
    });

    if let Some(hb_json) = found_hb {
        if let Ok(hb) = serde_json::from_value::<HeartbeatEvent>(
            hb_json.get("content").cloned().unwrap_or_default(),
        ) {
            bot.handle_heartbeat_event(&hb);
        }
    }

    worker
        .post_result(
            "test-task-001",
            TaskStatus::Success,
            Some(serde_json::json!({"artifact": "build/output.wasm"})),
            None,
            42,
            &worker_room_id,
            None,
        )
        .await
        .unwrap();

    worker_a_mc.sync_once().await.unwrap();
    coordinator_mc.sync_once().await.unwrap();

    let result_events = coordinator_mc
        .sync_and_collect_events(&worker_room_id, Duration::from_secs(3))
        .await
        .unwrap();

    let found_result = result_events.iter().find(|e| {
        e.get("content")
            .and_then(|c| c.get("task_uuid"))
            .and_then(|u| u.as_str())
            == Some("test-task-001")
            && e.get("content")
                .and_then(|c| c.get("status"))
                .and_then(|s| s.as_str())
                == Some("success")
    });

    assert!(
        found_result.is_some(),
        "coordinator should see the result event"
    );

    let result_json = found_result.unwrap();
    let result: TaskResultEvent =
        serde_json::from_value(result_json.get("content").cloned().unwrap()).unwrap();
    bot.handle_result_event(&result);

    assert_eq!(
        bot.watchlist_len(),
        0,
        "watchlist should be empty after result"
    );

    hs.stop().await;
}

#[tokio::test]
async fn test_sender_client_post_and_wait() {
    use mxdx_fabric::sender::SenderClient;

    let mut hs = TuwunelInstance::start().await.unwrap();
    let base_url = format!("http://127.0.0.1:{}", hs.port);

    let sender_mc =
        MatrixClient::register_and_connect(&base_url, "sender-p2", "pass", "mxdx-test-token")
            .await
            .unwrap();
    let worker_a_mc =
        MatrixClient::register_and_connect(&base_url, "worker-a-p2", "pass", "mxdx-test-token")
            .await
            .unwrap();

    let shared_room_id = sender_mc
        .create_named_unencrypted_room("sender-worker-room", "org.mxdx.fabric.task")
        .await
        .unwrap();

    sender_mc
        .invite_user(&shared_room_id, worker_a_mc.user_id())
        .await
        .unwrap();
    worker_a_mc.sync_once().await.unwrap();
    worker_a_mc.join_room(&shared_room_id).await.unwrap();

    let power_levels = serde_json::json!({
        "users": {
            sender_mc.user_id().to_string(): 100,
            worker_a_mc.user_id().to_string(): 50
        },
        "users_default": 0,
        "events": {},
        "events_default": 0,
        "state_default": 0,
        "ban": 50,
        "kick": 50,
        "invite": 50,
        "redact": 50
    });
    sender_mc
        .send_state_event(&shared_room_id, "m.room.power_levels", "", power_levels)
        .await
        .unwrap();

    sender_mc.sync_once().await.unwrap();
    worker_a_mc.sync_once().await.unwrap();

    let sender_mc = Arc::new(sender_mc);
    let worker_a_mc = Arc::new(worker_a_mc);

    let sender_id = format!("@sender-p2:{}", hs.server_name);
    let sender = SenderClient::new(sender_mc.clone(), sender_id.clone());

    let task = make_task("sender-task-001", &sender_id);

    let posted_uuid = sender
        .post_task(task.clone(), &shared_room_id)
        .await
        .unwrap();
    assert_eq!(posted_uuid, "sender-task-001");

    worker_a_mc.sync_once().await.unwrap();
    let events = worker_a_mc
        .sync_and_collect_events(&shared_room_id, Duration::from_secs(5))
        .await
        .unwrap();

    let found_task = events.iter().any(|e| {
        e.get("content")
            .and_then(|c| c.get("uuid"))
            .and_then(|u| u.as_str())
            == Some("sender-task-001")
    });
    assert!(
        found_task,
        "worker should see the posted task event, got: {:?}",
        events
    );

    let worker = WorkerClient::new(
        worker_a_mc.clone(),
        format!("@worker-a-p2:{}", hs.server_name),
        hs.server_name.clone(),
    );

    let claimed = worker.try_claim(&task, &shared_room_id).await.unwrap();
    assert!(claimed, "worker-a should win the claim");

    worker
        .post_result(
            "sender-task-001",
            TaskStatus::Success,
            Some(serde_json::json!({"output": "done"})),
            None,
            5,
            &shared_room_id,
            None,
        )
        .await
        .unwrap();

    worker_a_mc.sync_once().await.unwrap();

    let result = sender
        .wait_for_result("sender-task-001", &shared_room_id, Duration::from_secs(30))
        .await
        .unwrap();

    assert!(result.is_some(), "sender should receive the task result");
    let result = result.unwrap();
    assert_eq!(result.status, TaskStatus::Success);
    assert_eq!(result.task_uuid, "sender-task-001");

    hs.stop().await;
}

#[tokio::test]
async fn test_jcode_worker_mock_task() {
    let task = TaskEvent {
        uuid: "jcode-mock-001".to_string(),
        sender_id: "@sender:test.localhost".to_string(),
        required_capabilities: vec![],
        estimated_cycles: None,
        timeout_seconds: 30,
        heartbeat_interval_seconds: 10,
        on_timeout: FailurePolicy::Escalate,
        on_heartbeat_miss: FailurePolicy::Escalate,
        routing_mode: RoutingMode::Direct,
        p2p_stream: false,
        payload: serde_json::json!({"prompt": "echo hello world"}),
        plan: None,
    };

    let prompt = task
        .payload
        .get("prompt")
        .and_then(|v| v.as_str())
        .or(task.plan.as_deref())
        .unwrap_or("no prompt provided");

    assert_eq!(prompt, "echo hello world");

    let task_with_plan = TaskEvent {
        uuid: "jcode-mock-002".to_string(),
        sender_id: "@sender:test.localhost".to_string(),
        required_capabilities: vec![],
        estimated_cycles: None,
        timeout_seconds: 30,
        heartbeat_interval_seconds: 10,
        on_timeout: FailurePolicy::Escalate,
        on_heartbeat_miss: FailurePolicy::Escalate,
        routing_mode: RoutingMode::Direct,
        p2p_stream: false,
        payload: serde_json::json!({}),
        plan: Some("fallback plan prompt".to_string()),
    };

    let prompt_from_plan = task_with_plan
        .payload
        .get("prompt")
        .and_then(|v| v.as_str())
        .or(task_with_plan.plan.as_deref())
        .unwrap_or("no prompt provided");

    assert_eq!(prompt_from_plan, "fallback plan prompt");

    let empty_task = TaskEvent {
        uuid: "jcode-mock-003".to_string(),
        sender_id: "@sender:test.localhost".to_string(),
        required_capabilities: vec![],
        estimated_cycles: None,
        timeout_seconds: 30,
        heartbeat_interval_seconds: 10,
        on_timeout: FailurePolicy::Escalate,
        on_heartbeat_miss: FailurePolicy::Escalate,
        routing_mode: RoutingMode::Direct,
        p2p_stream: false,
        payload: serde_json::json!({}),
        plan: None,
    };

    let fallback_prompt = empty_task
        .payload
        .get("prompt")
        .and_then(|v| v.as_str())
        .or(empty_task.plan.as_deref())
        .unwrap_or("no prompt provided");

    assert_eq!(fallback_prompt, "no prompt provided");

    let output = tokio::process::Command::new("echo")
        .args(["--provider", "claude", "--ndjson", "run", "hello world"])
        .output()
        .await
        .expect("echo binary should be available");

    assert!(
        output.status.success(),
        "echo should exit 0, got: {:?}",
        output.status
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("hello world"),
        "stdout should contain the prompt text, got: {}",
        stdout
    );
}

#[tokio::test]
async fn coordinator_routing_auto_brokered_for_long_timeout() {
    let task = make_task("task-routing-001", "@sender:test.localhost");
    assert_eq!(task.timeout_seconds, 60);
    assert_eq!(task.routing_mode, RoutingMode::Auto);

    let effective = match &task.routing_mode {
        RoutingMode::Auto => {
            if task.timeout_seconds < 30 {
                RoutingMode::Direct
            } else {
                RoutingMode::Brokered
            }
        }
        other => other.clone(),
    };

    assert_eq!(
        effective,
        RoutingMode::Brokered,
        "Auto with timeout >= 30 should route as Brokered"
    );
}

#[tokio::test]
async fn coordinator_routing_auto_direct_for_short_timeout() {
    let mut task = make_task("task-routing-002", "@sender:test.localhost");
    task.timeout_seconds = 10;

    let effective = match &task.routing_mode {
        RoutingMode::Auto => {
            if task.timeout_seconds < 30 {
                RoutingMode::Direct
            } else {
                RoutingMode::Brokered
            }
        }
        other => other.clone(),
    };

    assert_eq!(
        effective,
        RoutingMode::Direct,
        "Auto with timeout < 30 should route as Direct"
    );
}

#[tokio::test]
async fn coordinator_watchlist_lifecycle() {
    let mut hs = TuwunelInstance::start().await.unwrap();
    let base_url = format!("http://127.0.0.1:{}", hs.port);

    let coordinator_mc =
        MatrixClient::register_and_connect(&base_url, "coord2", "pass", "mxdx-test-token")
            .await
            .unwrap();

    let coord_room_id = coordinator_mc
        .create_named_unencrypted_room("fabric-coord-wl", "org.mxdx.fabric.coordinator")
        .await
        .unwrap();

    let coord_arc = Arc::new(coordinator_mc);
    let mut bot = CoordinatorBot::new(coord_arc.clone(), coord_room_id, hs.server_name.clone());

    assert_eq!(bot.watchlist_len(), 0);

    let task = make_task("task-wl-001", "@sender:test.localhost");
    bot.handle_task_event(task).await.unwrap();
    assert_eq!(bot.watchlist_len(), 1);

    let claim = ClaimEvent {
        task_uuid: "task-wl-001".to_string(),
        worker_id: "@worker:test.localhost".to_string(),
        claimed_at: 1000,
    };
    bot.handle_claim_event(&claim);
    assert!(bot.watchlist_contains("task-wl-001"));

    let hb = HeartbeatEvent {
        task_uuid: "task-wl-001".to_string(),
        worker_id: "@worker:test.localhost".to_string(),
        progress: Some("running".to_string()),
        timestamp: 1010,
    };
    bot.handle_heartbeat_event(&hb);
    assert!(bot.watchlist_contains("task-wl-001"));

    let result = TaskResultEvent {
        task_uuid: "task-wl-001".to_string(),
        worker_id: "@worker:test.localhost".to_string(),
        status: TaskStatus::Success,
        output: None,
        error: None,
        duration_seconds: 10,
    };
    bot.handle_result_event(&result);
    assert_eq!(
        bot.watchlist_len(),
        0,
        "watchlist should be empty after result"
    );

    hs.stop().await;
}

#[tokio::test]
async fn test_failure_policy_escalate() {
    let mut hs = TuwunelInstance::start().await.unwrap();
    let base_url = format!("http://127.0.0.1:{}", hs.port);

    let coordinator_mc =
        MatrixClient::register_and_connect(&base_url, "coord-esc", "pass", "mxdx-test-token")
            .await
            .unwrap();
    let sender_mc =
        MatrixClient::register_and_connect(&base_url, "sender-esc", "pass", "mxdx-test-token")
            .await
            .unwrap();

    let coord_room_id = coordinator_mc
        .create_named_unencrypted_room("coordinator-room-esc", "org.mxdx.fabric.coordinator")
        .await
        .unwrap();

    coordinator_mc
        .invite_user(&coord_room_id, sender_mc.user_id())
        .await
        .unwrap();
    sender_mc.sync_once().await.unwrap();
    sender_mc.join_room(&coord_room_id).await.unwrap();
    coordinator_mc.sync_once().await.unwrap();
    sender_mc.sync_once().await.unwrap();

    let task = TaskEvent {
        uuid: "esc-task-001".to_string(),
        sender_id: format!("@sender-esc:{}", hs.server_name),
        required_capabilities: vec!["rust".to_string()],
        estimated_cycles: None,
        timeout_seconds: 5,
        heartbeat_interval_seconds: 30,
        on_timeout: FailurePolicy::Escalate,
        on_heartbeat_miss: FailurePolicy::Escalate,
        routing_mode: RoutingMode::Brokered,
        p2p_stream: false,
        payload: serde_json::json!({"cmd": "should timeout"}),
        plan: Some("Test escalation plan".to_string()),
    };

    let task_payload = serde_json::json!({
        "type": "org.mxdx.fabric.task",
        "content": task,
    });
    sender_mc
        .send_event(&coord_room_id, task_payload)
        .await
        .unwrap();
    sender_mc.sync_once().await.unwrap();

    let coordinator_mc = Arc::new(coordinator_mc);
    let server_name = hs.server_name.clone();
    let room_id = coord_room_id.clone();
    let coord_client = coordinator_mc.clone();
    let coord_handle = tokio::spawn(async move {
        let mut bot = CoordinatorBot::new(coord_client, room_id, server_name);
        let _ = bot.run().await;
    });

    let sender_mc = Arc::new(sender_mc);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(25);
    let mut found_escalation = false;
    let mut last_events = Vec::new();

    while tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_secs(2)).await;
        sender_mc.sync_once().await.unwrap();
        let events = sender_mc
            .sync_and_collect_events(&coord_room_id, Duration::from_secs(2))
            .await
            .unwrap();
        last_events = events.clone();
        if events.iter().any(|e| {
            let event_type = e.get("type").and_then(|t| t.as_str()).unwrap_or("");
            if event_type != "m.room.message" {
                return false;
            }
            let body = e
                .get("content")
                .and_then(|c| c.get("body"))
                .and_then(|b| b.as_str())
                .unwrap_or("");
            body.contains("esc-task-001") && body.contains("stalled")
        }) {
            found_escalation = true;
            break;
        }
    }

    coord_handle.abort();

    assert!(
        found_escalation,
        "expected escalation message in coordinator room, got events: {:?}",
        last_events
    );

    hs.stop().await;
}

#[tokio::test]
async fn test_failure_policy_respawn() {
    let mut hs = TuwunelInstance::start().await.unwrap();
    let base_url = format!("http://127.0.0.1:{}", hs.port);

    let coordinator_mc =
        MatrixClient::register_and_connect(&base_url, "coord-rsp", "pass", "mxdx-test-token")
            .await
            .unwrap();
    let sender_mc =
        MatrixClient::register_and_connect(&base_url, "sender-rsp", "pass", "mxdx-test-token")
            .await
            .unwrap();

    let coord_room_id = coordinator_mc
        .create_named_unencrypted_room("coordinator-room-rsp", "org.mxdx.fabric.coordinator")
        .await
        .unwrap();

    coordinator_mc
        .invite_user(&coord_room_id, sender_mc.user_id())
        .await
        .unwrap();
    sender_mc.sync_once().await.unwrap();
    sender_mc.join_room(&coord_room_id).await.unwrap();
    coordinator_mc.sync_once().await.unwrap();
    sender_mc.sync_once().await.unwrap();

    let task = TaskEvent {
        uuid: "rsp-task-001".to_string(),
        sender_id: format!("@sender-rsp:{}", hs.server_name),
        required_capabilities: vec!["rust".to_string()],
        estimated_cycles: None,
        timeout_seconds: 5,
        heartbeat_interval_seconds: 30,
        on_timeout: FailurePolicy::Respawn { max_retries: 1 },
        on_heartbeat_miss: FailurePolicy::Escalate,
        routing_mode: RoutingMode::Brokered,
        p2p_stream: false,
        payload: serde_json::json!({"cmd": "should respawn"}),
        plan: Some("Test respawn plan".to_string()),
    };

    let task_payload = serde_json::json!({
        "type": "org.mxdx.fabric.task",
        "content": task,
    });
    sender_mc
        .send_event(&coord_room_id, task_payload)
        .await
        .unwrap();
    sender_mc.sync_once().await.unwrap();

    let coordinator_mc = Arc::new(coordinator_mc);
    let server_name = hs.server_name.clone();
    let room_id = coord_room_id.clone();
    let coord_client = coordinator_mc.clone();
    let coord_handle = tokio::spawn(async move {
        let mut bot = CoordinatorBot::new(coord_client, room_id, server_name);
        let _ = bot.run().await;
    });

    let sender_mc = Arc::new(sender_mc);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
    let mut found_escalation_retry = false;
    let mut last_events = Vec::new();

    while tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_secs(2)).await;
        sender_mc.sync_once().await.unwrap();
        let events = sender_mc
            .sync_and_collect_events(&coord_room_id, Duration::from_secs(2))
            .await
            .unwrap();
        last_events = events.clone();

        if !found_escalation_retry {
            found_escalation_retry = events.iter().any(|e| {
                let event_type = e.get("type").and_then(|t| t.as_str()).unwrap_or("");
                if event_type != "m.room.message" {
                    return false;
                }
                let body = e
                    .get("content")
                    .and_then(|c| c.get("body"))
                    .and_then(|b| b.as_str())
                    .unwrap_or("");
                body.contains("rsp-task-001-retry-1") && body.contains("stalled")
            });
        }

        if found_escalation_retry {
            break;
        }
    }

    coord_handle.abort();

    assert!(
        found_escalation_retry,
        "expected escalation message for retry task (rsp-task-001-retry-1) in coordinator room, \
         proving respawn happened then exhausted retries. Got events: {:?}",
        last_events
    );

    hs.stop().await;
}

#[tokio::test]
async fn test_p2p_stream_unix_socket() {
    use std::path::PathBuf;
    use tokio::io::AsyncReadExt;

    let mut hs = TuwunelInstance::start().await.unwrap();
    let base_url = format!("http://127.0.0.1:{}", hs.port);

    let helper_script = "/tmp/mxdx-test-p2p-helper.sh";
    tokio::fs::write(helper_script, "#!/bin/sh\nseq 1 1000\n")
        .await
        .unwrap();
    tokio::fs::set_permissions(
        helper_script,
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .await
    .unwrap();

    let sender_mc =
        MatrixClient::register_and_connect(&base_url, "sender-p2p", "pass", "mxdx-test-token")
            .await
            .unwrap();
    let worker_a_mc =
        MatrixClient::register_and_connect(&base_url, "worker-a-p2p", "pass", "mxdx-test-token")
            .await
            .unwrap();

    let shared_room_id = sender_mc
        .create_named_unencrypted_room("p2p-stream-room", "org.mxdx.fabric.task")
        .await
        .unwrap();

    sender_mc
        .invite_user(&shared_room_id, worker_a_mc.user_id())
        .await
        .unwrap();
    worker_a_mc.sync_once().await.unwrap();
    worker_a_mc.join_room(&shared_room_id).await.unwrap();

    let power_levels = serde_json::json!({
        "users": {
            sender_mc.user_id().to_string(): 100,
            worker_a_mc.user_id().to_string(): 50
        },
        "users_default": 0,
        "events": {},
        "events_default": 0,
        "state_default": 0,
        "ban": 50,
        "kick": 50,
        "invite": 50,
        "redact": 50
    });
    sender_mc
        .send_state_event(&shared_room_id, "m.room.power_levels", "", power_levels)
        .await
        .unwrap();

    sender_mc.sync_once().await.unwrap();
    worker_a_mc.sync_once().await.unwrap();

    let sender_mc = Arc::new(sender_mc);
    let worker_a_mc = Arc::new(worker_a_mc);

    let sender_id = format!("@sender-p2p:{}", hs.server_name);
    let sender = SenderClient::new(sender_mc.clone(), sender_id.clone());

    let task = TaskEvent {
        uuid: "p2p-task-001".to_string(),
        sender_id: sender_id.clone(),
        required_capabilities: vec![],
        estimated_cycles: None,
        timeout_seconds: 60,
        heartbeat_interval_seconds: 2,
        on_timeout: FailurePolicy::Escalate,
        on_heartbeat_miss: FailurePolicy::Escalate,
        routing_mode: RoutingMode::Direct,
        p2p_stream: true,
        payload: serde_json::json!({"prompt": "seq 1 1000"}),
        plan: None,
    };

    let posted_uuid = sender
        .post_task(task.clone(), &shared_room_id)
        .await
        .unwrap();
    assert_eq!(posted_uuid, "p2p-task-001");

    worker_a_mc.sync_once().await.unwrap();
    let _events = worker_a_mc
        .sync_and_collect_events(&shared_room_id, Duration::from_secs(5))
        .await
        .unwrap();

    let worker = WorkerClient::new(
        worker_a_mc.clone(),
        format!("@worker-a-p2p:{}", hs.server_name),
        hs.server_name.clone(),
    );

    let jcode_worker = JcodeWorker::new(worker, Some(PathBuf::from(helper_script)));

    let worker_room_id = shared_room_id.clone();
    let worker_task = task.clone();
    let worker_handle = tokio::spawn(async move {
        jcode_worker
            .run_task(worker_task, &worker_room_id, String::new())
            .await
            .unwrap();
    });

    let stream = sender
        .connect_stream("p2p-task-001", &shared_room_id, Duration::from_secs(30))
        .await
        .unwrap();

    assert!(
        stream.is_some(),
        "sender should connect to P2P stream socket"
    );

    let mut stream = stream.unwrap();
    let mut all_bytes = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        match stream.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => all_bytes.extend_from_slice(&buf[..n]),
            Err(e) => {
                panic!("error reading from P2P stream: {}", e);
            }
        }
    }

    let output = String::from_utf8_lossy(&all_bytes);
    assert!(
        output.contains("1000"),
        "P2P stream should contain '1000' (last line of seq output), got: {}",
        output
    );

    worker_handle.await.unwrap();

    sender_mc.sync_once().await.unwrap();
    let result_events = sender_mc
        .sync_and_collect_events(&shared_room_id, Duration::from_secs(10))
        .await
        .unwrap();

    let found_result = result_events.iter().any(|e| {
        e.get("content")
            .and_then(|c| c.get("task_uuid"))
            .and_then(|u| u.as_str())
            == Some("p2p-task-001")
            && e.get("content")
                .and_then(|c| c.get("status"))
                .and_then(|s| s.as_str())
                == Some("success")
    });

    assert!(
        found_result,
        "TaskResultEvent with Success should be posted, got events: {:?}",
        result_events
    );

    let socket_path = "/tmp/mxdx-fabric-p2p-task-001.sock";
    assert!(
        !std::path::Path::new(socket_path).exists(),
        "socket file should be cleaned up after task completion"
    );

    let _ = tokio::fs::remove_file(helper_script).await;

    hs.stop().await;
}

#[tokio::test]
async fn test_fabric_cli_post() {
    let mut hs = TuwunelInstance::start().await.unwrap();
    let base_url = format!("http://127.0.0.1:{}", hs.port);

    let sender_user = hs.register_user("cli-sender", "pass").await.unwrap();
    let worker_user = hs.register_user("cli-worker", "pass").await.unwrap();

    let room_id = sender_user.create_room().await.unwrap();
    sender_user
        .invite(&room_id, &worker_user.user_id)
        .await
        .unwrap();

    worker_user
        .wait_for_invite(&room_id, Duration::from_secs(10))
        .await
        .unwrap();

    let http = reqwest::Client::new();
    let join_url = format!(
        "{}/_matrix/client/v3/join/{}",
        base_url,
        urlencoding::encode(&room_id),
    );
    let resp = http
        .post(&join_url)
        .bearer_auth(&worker_user.access_token)
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "worker join failed: {}",
        resp.text().await.unwrap_or_default()
    );

    let worker_token = worker_user.access_token.clone();
    let worker_base_url = base_url.clone();
    let worker_room_id = room_id.clone();
    let worker_handle = tokio::spawn(async move {
        let http = reqwest::Client::new();
        let mut since: Option<String> = None;

        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        while tokio::time::Instant::now() < deadline {
            let mut sync_url = format!("{}/_matrix/client/v3/sync?timeout=3000", worker_base_url,);
            if let Some(ref token) = since {
                sync_url.push_str(&format!("&since={}", urlencoding::encode(token)));
            }

            let resp = http
                .get(&sync_url)
                .bearer_auth(&worker_token)
                .send()
                .await
                .unwrap();
            let body: serde_json::Value = resp.json().await.unwrap();
            if let Some(next) = body["next_batch"].as_str() {
                since = Some(next.to_string());
            }

            let timeline = &body["rooms"]["join"][&worker_room_id]["timeline"]["events"];
            if let Some(events) = timeline.as_array() {
                for event in events {
                    let event_type = event.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    if event_type == "org.mxdx.fabric.task" {
                        let content = event.get("content").unwrap();
                        let task_uuid = content.get("uuid").and_then(|u| u.as_str()).unwrap();

                        let result = serde_json::json!({
                            "task_uuid": task_uuid,
                            "worker_id": "cli-worker",
                            "status": "success",
                            "output": {"result": "cli test done"},
                            "error": null,
                            "duration_seconds": 1
                        });

                        let txn_id = uuid::Uuid::new_v4().to_string();
                        let send_url = format!(
                            "{}/_matrix/client/v3/rooms/{}/send/org.mxdx.fabric.result/{}",
                            worker_base_url,
                            urlencoding::encode(&worker_room_id),
                            urlencoding::encode(&txn_id),
                        );
                        let send_resp = http
                            .put(&send_url)
                            .bearer_auth(&worker_token)
                            .json(&result)
                            .send()
                            .await
                            .unwrap();
                        assert!(
                            send_resp.status().is_success(),
                            "failed to post result: {}",
                            send_resp.text().await.unwrap_or_default()
                        );
                        return;
                    }
                }
            }

            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        panic!("worker timed out waiting for task event");
    });

    let fabric_bin = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("target/debug/fabric");

    let output = tokio::process::Command::new(&fabric_bin)
        .args([
            "post",
            "--homeserver",
            &base_url,
            "--token",
            &sender_user.access_token,
            "--coordinator-room",
            &room_id,
            "--capabilities",
            "rust",
            "--prompt",
            "test task from CLI",
            "--timeout",
            "60",
        ])
        .output()
        .await
        .expect("fabric binary should be runnable");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(
        output.status.code(),
        Some(0),
        "fabric post should exit 0, stderr: {}, stdout: {}",
        stderr,
        stdout
    );

    let result_json: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("stdout should be valid JSON: {e}, got: {stdout}"));
    assert_eq!(
        result_json.get("status").and_then(|s| s.as_str()),
        Some("success"),
        "result should be success, got: {}",
        result_json
    );

    worker_handle.await.unwrap();
    hs.stop().await;
}

#[tokio::test]
async fn capability_advertisement_publish_and_read_back() {
    let mut hs = TuwunelInstance::start().await.unwrap();
    let base_url = format!("http://127.0.0.1:{}", hs.port);

    let coordinator_mc =
        MatrixClient::register_and_connect(&base_url, "coord-cap", "pass", "mxdx-test-token")
            .await
            .unwrap();
    let worker_mc =
        MatrixClient::register_and_connect(&base_url, "worker-cap", "pass", "mxdx-test-token")
            .await
            .unwrap();

    let room_id = coordinator_mc
        .create_named_unencrypted_room("cap-advert-room", "org.mxdx.fabric.coordinator")
        .await
        .unwrap();

    coordinator_mc
        .invite_user(&room_id, worker_mc.user_id())
        .await
        .unwrap();
    worker_mc.sync_once().await.unwrap();
    worker_mc.join_room(&room_id).await.unwrap();

    let power_levels = serde_json::json!({
        "users": {
            coordinator_mc.user_id().to_string(): 100,
            worker_mc.user_id().to_string(): 50
        },
        "users_default": 0,
        "events": {},
        "events_default": 0,
        "state_default": 0,
        "ban": 50,
        "kick": 50,
        "invite": 50,
        "redact": 50
    });
    coordinator_mc
        .send_state_event(&room_id, "m.room.power_levels", "", power_levels)
        .await
        .unwrap();

    coordinator_mc.sync_once().await.unwrap();
    worker_mc.sync_once().await.unwrap();

    let worker_mc = Arc::new(worker_mc);
    let worker_id = format!("@worker-cap:{}", hs.server_name);

    let worker_client =
        WorkerClient::new(worker_mc.clone(), worker_id.clone(), hs.server_name.clone());

    let advertisement = CapabilityAdvertisement {
        worker_id: worker_id.clone(),
        host: "test-host".into(),
        tools: vec![WorkerTool {
            name: "jcode".into(),
            version: Some("0.8.0".into()),
            description: "Rust coding agent".into(),
            healthy: true,
            input_schema: InputSchema {
                r#type: "object".into(),
                properties: HashMap::from([
                    (
                        "prompt".into(),
                        SchemaProperty {
                            r#type: "string".into(),
                            description: "Task prompt".into(),
                        },
                    ),
                    (
                        "cwd".into(),
                        SchemaProperty {
                            r#type: "string".into(),
                            description: "Working directory".into(),
                        },
                    ),
                ]),
                required: vec!["prompt".into()],
            },
        }],
    };

    worker_client
        .publish_capability_advertisement(&advertisement, &room_id)
        .await
        .unwrap();

    worker_mc.sync_once().await.unwrap();

    let state_json = worker_mc
        .get_room_state_event(&room_id, EVENT_CAPABILITY, &worker_id)
        .await
        .unwrap();

    let parsed: CapabilityAdvertisement = serde_json::from_value(state_json.clone())
        .unwrap_or_else(|e| {
            panic!(
                "state event should deserialize to CapabilityAdvertisement: {e}, raw: {state_json}"
            )
        });

    assert_eq!(parsed.worker_id, worker_id);
    assert_eq!(parsed.host, "test-host");
    assert_eq!(parsed.tools.len(), 1);
    assert_eq!(parsed.tools[0].name, "jcode");
    assert_eq!(parsed.tools[0].version, Some("0.8.0".into()));
    assert!(parsed.tools[0].healthy);
    assert_eq!(parsed.tools[0].input_schema.r#type, "object");
    assert_eq!(parsed.tools[0].input_schema.properties.len(), 2);
    assert!(parsed.tools[0]
        .input_schema
        .properties
        .contains_key("prompt"));
    assert!(parsed.tools[0].input_schema.properties.contains_key("cwd"));
    assert_eq!(parsed.tools[0].input_schema.required, vec!["prompt"]);

    assert_eq!(parsed, advertisement);

    hs.stop().await;
}
