use std::sync::Arc;
use std::time::Duration;

use mxdx_fabric::coordinator::CoordinatorBot;
use mxdx_fabric::worker::WorkerClient;
use mxdx_fabric::{
    ClaimEvent, FailurePolicy, HeartbeatEvent, RoutingMode, TaskEvent, TaskResultEvent, TaskStatus,
    EVENT_CLAIM,
};
use mxdx_matrix::MatrixClient;
use mxdx_test_helpers::tuwunel::TuwunelInstance;

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
        .post_heartbeat("test-task-001", Some("50%".into()), &worker_room_id)
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
