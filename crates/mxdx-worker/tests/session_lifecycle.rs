use std::time::{SystemTime, UNIX_EPOCH};

use mxdx_types::events::session::{
    CompletedSessionState, OutputStream, SessionStatus, SessionTask,
};
use mxdx_worker::heartbeat::HeartbeatPoster;
use mxdx_worker::output::OutputRouter;
use mxdx_worker::retention::RetentionSweeper;
use mxdx_worker::session::{SessionManager, SessionState};
use mxdx_worker::tmux::TmuxSession;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_test_task(uuid: &str, bin: &str, args: &[&str]) -> SessionTask {
    SessionTask {
        uuid: uuid.into(),
        sender_id: "@alice:example.com".into(),
        bin: bin.into(),
        args: args.iter().map(|s| s.to_string()).collect(),
        env: None,
        cwd: None,
        interactive: false,
        no_room_output: false,
        timeout_seconds: None,
        heartbeat_interval_seconds: 30,
        plan: None,
        required_capabilities: vec![],
        routing_mode: None,
        on_timeout: None,
        on_heartbeat_miss: None,
    }
}

fn make_tmux_session(name: &str) -> TmuxSession {
    TmuxSession {
        session_name: name.to_string(),
        socket_path: std::path::PathBuf::from(format!("/tmp/mxdx-tmux/mxdx-{name}")),
        exit_notify_path: std::path::PathBuf::from(format!("/tmp/mxdx-tmux/mxdx-{name}.notify")),
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

// ---------------------------------------------------------------------------
// Test 1: Worker claims session, gets correct ActiveSessionState
// ---------------------------------------------------------------------------

#[test]
fn claim_creates_active_session_state() {
    let mut mgr = SessionManager::new("worker-1".into());
    let task = make_test_task("sess-1", "echo", &["hello"]);
    let active = mgr.claim(task).unwrap();

    assert_eq!(active.bin, "echo");
    assert_eq!(active.args, vec!["hello"]);
    assert_eq!(active.worker_id, "worker-1");
    assert!(!active.interactive);
    assert!(active.pid.is_none());
    assert!(active.start_time > 0);
}

// ---------------------------------------------------------------------------
// Test 2: Full lifecycle -- claim -> running -> complete
// ---------------------------------------------------------------------------

#[test]
fn full_session_lifecycle() {
    let mut mgr = SessionManager::new("worker-1".into());
    let task = make_test_task("sess-2", "ls", &["-la"]);

    // Claim
    let active = mgr.claim(task).unwrap();
    assert_eq!(active.bin, "ls");
    assert_eq!(mgr.get("sess-2").unwrap().state, SessionState::Claimed);

    // Mark running (using a synthetic TmuxSession -- no real tmux process)
    let tmux = make_tmux_session("sess-2");
    mgr.mark_running("sess-2", Some(42), tmux).unwrap();
    assert_eq!(mgr.get("sess-2").unwrap().state, SessionState::Running);
    assert_eq!(mgr.get("sess-2").unwrap().pid, Some(42));

    // Complete with Success
    let completed = mgr
        .complete("sess-2", SessionStatus::Success, Some(0))
        .unwrap();
    assert_eq!(mgr.get("sess-2").unwrap().state, SessionState::Completed);
    assert_eq!(completed.exit_code, Some(0));
    assert!(completed.completion_time > 0);
    // Duration should be near-zero since claim and complete happen instantly in tests
    assert!(completed.duration_seconds <= 1);
}

// ---------------------------------------------------------------------------
// Test 3: no_room_output suppresses output but heartbeats still work
// ---------------------------------------------------------------------------

#[test]
fn no_room_output_suppresses_output_not_heartbeats() {
    let output = OutputRouter::new(true); // suppressed
    let heartbeat = HeartbeatPoster::new(30);

    let event = output.create_output_event("sess-1", "worker-1", OutputStream::Stdout, b"data");
    assert!(event.is_none(), "output should be suppressed");

    let hb = heartbeat.create_heartbeat("sess-1", "worker-1", None);
    assert_eq!(hb.session_uuid, "sess-1", "heartbeat should still fire");
    assert_eq!(hb.worker_id, "worker-1");
}

// ---------------------------------------------------------------------------
// Test 4: Heartbeat posted even during quiet periods
// ---------------------------------------------------------------------------

#[test]
fn heartbeat_always_active() {
    let hb = HeartbeatPoster::new(10);
    let event = hb.create_heartbeat("sess-1", "worker-1", Some("50%".into()));

    assert_eq!(event.session_uuid, "sess-1");
    assert_eq!(event.worker_id, "worker-1");
    assert_eq!(event.progress, Some("50%".into()));
    assert!(event.timestamp > 0);
}

// ---------------------------------------------------------------------------
// Test 5: Retention sweep identifies expired sessions
// ---------------------------------------------------------------------------

#[test]
fn retention_sweep_removes_expired_sessions() {
    let sweeper = RetentionSweeper::new(30); // 30 day retention
    let now = now_secs();

    let sessions = vec![
        (
            "session/recent/completed".into(),
            CompletedSessionState {
                exit_code: Some(0),
                duration_seconds: 10,
                completion_time: now - 5 * 24 * 60 * 60, // 5 days ago -- fresh
            },
        ),
        (
            "session/old/completed".into(),
            CompletedSessionState {
                exit_code: Some(1),
                duration_seconds: 120,
                completion_time: now - 60 * 24 * 60 * 60, // 60 days ago -- expired
            },
        ),
    ];

    let expired = sweeper.find_expired(&sessions);
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0], "session/old/completed");
}
