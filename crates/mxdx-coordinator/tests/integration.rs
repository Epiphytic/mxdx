use mxdx_coordinator::claim::ClaimTracker;
use mxdx_coordinator::config::CoordinatorRuntimeConfig;
use mxdx_coordinator::failure::{apply_policy, FailureAction};
use mxdx_coordinator::index::CapabilityIndex;
use mxdx_coordinator::router::Router;
use mxdx_coordinator::watchlist::{WatchAlert, WatchedSession, Watchlist};
use mxdx_types::config::{CoordinatorConfig, DefaultsConfig};
use mxdx_types::events::capability::WorkerTool;
use mxdx_types::events::fabric::FailurePolicy;
use mxdx_types::events::session::SessionTask;
use mxdx_types::events::worker_info::WorkerInfo;

// --- Helpers ---

fn make_worker_info(id: &str, capabilities: Vec<&str>, tools: Vec<WorkerTool>) -> WorkerInfo {
    WorkerInfo {
        worker_id: id.into(),
        host: "test-host".into(),
        os: "linux".into(),
        arch: "x86_64".into(),
        cpu_count: 4,
        memory_total_mb: 8192,
        disk_available_mb: 50000,
        tools,
        capabilities: capabilities.into_iter().map(String::from).collect(),
        updated_at: 1742572800,
    }
}

fn make_task(required_capabilities: Vec<&str>) -> SessionTask {
    SessionTask {
        uuid: "task-1".into(),
        sender_id: "@alice:example.com".into(),
        bin: "echo".into(),
        args: vec!["hello".into()],
        env: None,
        cwd: None,
        interactive: false,
        no_room_output: false,
        timeout_seconds: None,
        heartbeat_interval_seconds: 30,
        plan: None,
        required_capabilities: required_capabilities
            .into_iter()
            .map(String::from)
            .collect(),
        routing_mode: None,
        on_timeout: None,
        on_heartbeat_miss: None,
    }
}

fn make_watched_session(uuid: &str, started_at: u64, last_heartbeat: u64) -> WatchedSession {
    WatchedSession {
        session_uuid: uuid.into(),
        worker_id: "worker-1".into(),
        room_id: "!room:example.com".into(),
        started_at,
        last_heartbeat,
        heartbeat_interval_seconds: 30,
        timeout_seconds: None,
    }
}

// --- Router integration tests ---

#[test]
fn router_routes_to_correct_worker_based_on_capabilities() {
    let mut router = Router::new();
    router.update_worker(
        "!room1:example.com".into(),
        make_worker_info("w-1", vec!["linux"], vec![]),
    );
    router.update_worker(
        "!room2:example.com".into(),
        make_worker_info("w-2", vec!["linux", "gpu"], vec![]),
    );

    let task = make_task(vec!["gpu"]);
    let result = router.route(&task);
    assert!(result.is_some());
    assert_eq!(result.unwrap().info.worker_id, "w-2");
}

#[test]
fn router_returns_none_when_no_worker_matches() {
    let mut router = Router::new();
    router.update_worker(
        "!room1:example.com".into(),
        make_worker_info("w-1", vec!["linux"], vec![]),
    );

    let task = make_task(vec!["gpu", "arm64"]);
    assert!(router.route(&task).is_none());
}

// --- Watchlist integration tests ---

#[test]
fn watchlist_detects_heartbeat_miss() {
    let mut wl = Watchlist::new();
    wl.watch(make_watched_session("s-1", 900, 1000));

    // 61s since last heartbeat, exceeds 2x30=60
    let alerts = wl.check_at(1061);
    assert_eq!(alerts.len(), 1);
    assert!(matches!(
        &alerts[0],
        WatchAlert::HeartbeatMiss { session_uuid, .. } if session_uuid == "s-1"
    ));
}

#[test]
fn watchlist_detects_timeout() {
    let mut wl = Watchlist::new();
    let mut session = make_watched_session("s-1", 1000, 1500);
    session.timeout_seconds = Some(600);
    wl.watch(session);

    // 601s elapsed, exceeds 600s timeout
    let alerts = wl.check_at(1601);
    assert!(alerts.iter().any(|a| matches!(
        a,
        WatchAlert::Timeout { session_uuid, elapsed_seconds, .. }
        if session_uuid == "s-1" && *elapsed_seconds == 601
    )));
}

// --- Failure policy integration tests ---

#[test]
fn failure_policy_escalates_after_max_retries() {
    let task = make_task(vec![]);
    let action = apply_policy(
        &FailurePolicy::Respawn { max_retries: 2 },
        &task,
        "process crashed",
        2,
    );
    match action {
        FailureAction::Escalate { reason, .. } => {
            assert!(reason.contains("max retries (2) exceeded"));
        }
        _ => panic!("expected Escalate after max retries"),
    }
}

// --- Claim tracker integration tests ---

#[test]
fn claim_tracker_prevents_double_claims() {
    let mut tracker = ClaimTracker::new();
    assert!(tracker.record_claim("s-1", "worker-1"));
    assert!(!tracker.record_claim("s-1", "worker-2"));

    // Original claim preserved
    let claim = tracker.get_claim("s-1").unwrap();
    assert_eq!(claim.worker_id, "worker-1");
}

// --- Capability index integration tests ---

#[test]
fn capability_index_finds_intersection_of_capabilities() {
    let mut index = CapabilityIndex::new();
    index.update(make_worker_info(
        "w-1",
        vec!["linux", "gpu", "docker"],
        vec![],
    ));
    index.update(make_worker_info("w-2", vec!["linux", "gpu"], vec![]));
    index.update(make_worker_info("w-3", vec!["linux"], vec![]));

    let result = index.workers_with_all(&["linux".into(), "gpu".into(), "docker".into()]);
    assert_eq!(result.len(), 1);
    assert!(result.contains(&"w-1"));
}

// --- run_coordinator integration test ---

#[tokio::test]
async fn run_coordinator_returns_ok() {
    let config = CoordinatorRuntimeConfig::from_parts(
        DefaultsConfig::default(),
        CoordinatorConfig::default(),
    );
    let result = mxdx_coordinator::run_coordinator(config).await;
    assert!(result.is_ok());
}
