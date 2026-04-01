use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use mxdx_types::events::session::{
    ActiveSessionState, CompletedSessionState, OutputStream, SessionOutput, SessionStatus,
};

// ── 1. Submit task, verify UUID and fields ──────────────────────────────

#[test]
fn submit_task_has_valid_uuid_and_fields() {
    let args = vec!["--flag".to_string(), "value".to_string()];
    let task = mxdx_client::submit::build_task(
        "my-bin", &args, true, false, Some(120), 15, "@sender:example.com",
    );

    assert!(!task.uuid.is_empty());
    // UUID should be parseable
    uuid::Uuid::parse_str(&task.uuid).expect("UUID should be valid");
    assert_eq!(task.bin, "my-bin");
    assert_eq!(task.args, vec!["--flag", "value"]);
    assert!(task.interactive);
    assert!(!task.no_room_output);
    assert_eq!(task.timeout_seconds, Some(120));
    assert_eq!(task.heartbeat_interval_seconds, 15);
    assert_eq!(task.sender_id, "@sender:example.com");
}

// ── 2. Build cancel event, verify fields ────────────────────────────────

#[test]
fn cancel_event_fields() {
    let cancel = mxdx_client::cancel::build_cancel(
        "session-uuid-1",
        Some("user requested".into()),
        Some(10),
    );
    assert_eq!(cancel.session_uuid, "session-uuid-1");
    assert_eq!(cancel.reason, Some("user requested".into()));
    assert_eq!(cancel.grace_seconds, Some(10));
}

#[test]
fn signal_event_fields() {
    let signal = mxdx_client::cancel::build_signal("session-uuid-2", "SIGTERM");
    assert_eq!(signal.session_uuid, "session-uuid-2");
    assert_eq!(signal.signal, "SIGTERM");
}

// ── 3. Tail decode output correctly ─────────────────────────────────────

#[test]
fn tail_decodes_base64_output() {
    let output = SessionOutput {
        session_uuid: "s-1".into(),
        worker_id: "@w:h".into(),
        stream: OutputStream::Stdout,
        data: BASE64.encode(b"hello from worker"),
        seq: 1,
        timestamp: 1000,
    };
    let decoded = mxdx_client::tail::format_output(&output).unwrap();
    assert_eq!(decoded, "hello from worker");
}

#[test]
fn tail_result_formatting() {
    use mxdx_types::events::session::SessionResult;
    let result = SessionResult {
        session_uuid: "s-1".into(),
        worker_id: "@w:h".into(),
        status: SessionStatus::Success,
        exit_code: Some(0),
        duration_seconds: 42,
        tail: None,
    };
    let formatted = mxdx_client::tail::format_result(&result);
    assert!(formatted.contains("s-1"));
    assert!(formatted.contains("success"));
    assert!(formatted.contains("42s"));
}

// ── 4. LS format table with mixed active/completed sessions ─────────────

#[test]
fn ls_format_table_mixed() {
    let active_state = ActiveSessionState {
        bin: "echo".into(),
        args: vec!["hello".into()],
        pid: Some(1234),
        start_time: 1742572800,
        client_id: "@alice:h".into(),
        interactive: false,
        worker_id: "@w1:h".into(),
    };
    let completed_state = CompletedSessionState {
        exit_code: Some(0),
        duration_seconds: 60,
        completion_time: 1742572860,
    };

    let entries = vec![
        mxdx_client::ls::from_active("uuid-active".into(), &active_state),
        mxdx_client::ls::from_completed("uuid-done".into(), &active_state, &completed_state),
    ];

    let table = mxdx_client::ls::format_table(&entries);
    assert!(table.contains("UUID"));
    assert!(table.contains("uuid-active"));
    assert!(table.contains("uuid-done"));
    assert!(table.contains("active"));
    assert!(table.contains("done"));
}

// ── 5. Logs reassemble multi-chunk output in order ──────────────────────

#[test]
fn logs_reassemble_multi_chunk_in_order() {
    let outputs = vec![
        SessionOutput {
            session_uuid: "s-1".into(),
            worker_id: "@w:h".into(),
            stream: OutputStream::Stdout,
            data: BASE64.encode(b"chunk3"),
            seq: 3,
            timestamp: 1003,
        },
        SessionOutput {
            session_uuid: "s-1".into(),
            worker_id: "@w:h".into(),
            stream: OutputStream::Stdout,
            data: BASE64.encode(b"chunk1"),
            seq: 1,
            timestamp: 1001,
        },
        SessionOutput {
            session_uuid: "s-1".into(),
            worker_id: "@w:h".into(),
            stream: OutputStream::Stdout,
            data: BASE64.encode(b"chunk2"),
            seq: 2,
            timestamp: 1002,
        },
    ];

    let result = mxdx_client::logs::reassemble_output_string(outputs).unwrap();
    assert_eq!(result, "chunk1chunk2chunk3");
}

// ── 6. Reconnect finds only client's own sessions ───────────────────────

#[test]
fn reconnect_finds_own_sessions_only() {
    let sessions = vec![
        (
            "uuid-mine-1".to_string(),
            ActiveSessionState {
                bin: "echo".into(),
                args: vec![],
                pid: Some(100),
                start_time: 1000,
                client_id: "@me:h".into(),
                interactive: false,
                worker_id: "@w1:h".into(),
            },
        ),
        (
            "uuid-other".to_string(),
            ActiveSessionState {
                bin: "ls".into(),
                args: vec![],
                pid: Some(200),
                start_time: 2000,
                client_id: "@other:h".into(),
                interactive: false,
                worker_id: "@w2:h".into(),
            },
        ),
        (
            "uuid-mine-2".to_string(),
            ActiveSessionState {
                bin: "cat".into(),
                args: vec!["file.txt".into()],
                pid: Some(300),
                start_time: 3000,
                client_id: "@me:h".into(),
                interactive: true,
                worker_id: "@w3:h".into(),
            },
        ),
    ];

    let reconnectable =
        mxdx_client::reconnect::find_reconnectable_sessions(&sessions, "@me:h");
    assert_eq!(reconnectable.len(), 2);
    assert_eq!(reconnectable[0].0, "uuid-mine-1");
    assert_eq!(reconnectable[1].0, "uuid-mine-2");

    let formatted = mxdx_client::reconnect::format_reconnectable(&reconnectable);
    assert!(formatted.contains("uuid-mine-1"));
    assert!(formatted.contains("uuid-mine-2"));
    assert!(formatted.contains("echo"));
    assert!(formatted.contains("cat file.txt"));
}

// ── 7. Attach determines correct mode ───────────────────────────────────

#[test]
fn attach_mode_interactive_with_webrtc() {
    use mxdx_client::attach::{determine_attach_mode, AttachMode};
    assert_eq!(determine_attach_mode(true, true, false), AttachMode::Interactive);
}

#[test]
fn attach_mode_interactive_without_webrtc() {
    use mxdx_client::attach::{determine_attach_mode, AttachMode};
    assert_eq!(determine_attach_mode(true, false, false), AttachMode::TailThread);
}

#[test]
fn attach_mode_non_interactive() {
    use mxdx_client::attach::{determine_attach_mode, AttachMode};
    assert_eq!(determine_attach_mode(false, true, false), AttachMode::TailThread);
    assert_eq!(determine_attach_mode(false, false, false), AttachMode::TailThread);
}
