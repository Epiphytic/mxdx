use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::fabric::{FailurePolicy, RoutingMode};

/// Thread root event — a client submits a task for execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionTask {
    pub uuid: String,
    pub sender_id: String,
    pub bin: String,
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Option<HashMap<String, String>>,
    pub cwd: Option<String>,
    #[serde(default)]
    pub interactive: bool,
    #[serde(default)]
    pub no_room_output: bool,
    pub timeout_seconds: Option<u64>,
    #[serde(default = "default_heartbeat_interval")]
    pub heartbeat_interval_seconds: u64,
    pub plan: Option<String>,
    #[serde(default)]
    pub required_capabilities: Vec<String>,
    pub routing_mode: Option<RoutingMode>,
    pub on_timeout: Option<FailurePolicy>,
    pub on_heartbeat_miss: Option<FailurePolicy>,
}

fn default_heartbeat_interval() -> u64 {
    30
}

/// Worker start event — worker has begun executing the task.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionStart {
    pub session_uuid: String,
    pub worker_id: String,
    pub tmux_session: Option<String>,
    pub pid: Option<u32>,
    pub started_at: u64,
}

/// Batched output event — stdout/stderr from the process.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionOutput {
    pub session_uuid: String,
    pub worker_id: String,
    pub stream: OutputStream,
    pub data: String,
    pub seq: u64,
    pub timestamp: u64,
}

/// Liveness heartbeat from the worker.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionHeartbeat {
    pub session_uuid: String,
    pub worker_id: String,
    pub timestamp: u64,
    pub progress: Option<String>,
}

/// Completion event — the process has exited.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionResult {
    pub session_uuid: String,
    pub worker_id: String,
    pub status: SessionStatus,
    pub exit_code: Option<i32>,
    pub duration_seconds: u64,
    pub tail: Option<String>,
}

/// Client stdin input to an interactive session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionInput {
    pub session_uuid: String,
    pub data: String,
}

/// Client signal to the worker process.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionSignal {
    pub session_uuid: String,
    pub signal: String,
}

/// Client terminal resize event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionResize {
    pub session_uuid: String,
    pub cols: u16,
    pub rows: u16,
}

/// Client cancellation request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionCancel {
    pub session_uuid: String,
    pub reason: Option<String>,
    pub grace_seconds: Option<u64>,
}

/// State key: session/{uuid}/active
/// Represents an active running session as a room state event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ActiveSessionState {
    pub bin: String,
    pub args: Vec<String>,
    pub pid: Option<u32>,
    pub start_time: u64,
    pub client_id: String,
    pub interactive: bool,
    pub worker_id: String,
}

/// State key: session/{uuid}/completed
/// Represents a completed session as a room state event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompletedSessionState {
    pub exit_code: Option<i32>,
    pub duration_seconds: u64,
    pub completion_time: u64,
}

/// Output stream type.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum OutputStream {
    Stdout,
    Stderr,
}

/// Session completion status.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Success,
    Failed,
    Timeout,
    Cancelled,
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- SessionTask tests ---

    #[test]
    fn session_task_roundtrip() {
        let task = SessionTask {
            uuid: "abc-123".into(),
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
            required_capabilities: vec![],
            routing_mode: None,
            on_timeout: None,
            on_heartbeat_miss: None,
        };
        let json = serde_json::to_string(&task).unwrap();
        let back: SessionTask = serde_json::from_str(&json).unwrap();
        assert_eq!(back.uuid, "abc-123");
        assert_eq!(back.bin, "echo");
        assert!(!back.interactive);
    }

    #[test]
    fn session_task_snake_case_fields() {
        let task = SessionTask {
            uuid: "t-1".into(),
            sender_id: "@bob:example.com".into(),
            bin: "ls".into(),
            args: vec![],
            env: None,
            cwd: Some("/tmp".into()),
            interactive: true,
            no_room_output: true,
            timeout_seconds: Some(60),
            heartbeat_interval_seconds: 10,
            plan: Some("list files".into()),
            required_capabilities: vec!["linux".into()],
            routing_mode: Some(RoutingMode::Direct),
            on_timeout: Some(FailurePolicy::Escalate),
            on_heartbeat_miss: Some(FailurePolicy::Abandon),
        };
        let json = serde_json::to_string(&task).unwrap();
        assert!(json.contains("sender_id"));
        assert!(json.contains("no_room_output"));
        assert!(json.contains("timeout_seconds"));
        assert!(json.contains("heartbeat_interval_seconds"));
        assert!(json.contains("required_capabilities"));
        assert!(json.contains("routing_mode"));
        assert!(json.contains("on_timeout"));
        assert!(json.contains("on_heartbeat_miss"));
    }

    #[test]
    fn session_task_default_heartbeat() {
        let json = r#"{
            "uuid": "t-2",
            "sender_id": "@alice:example.com",
            "bin": "echo",
            "args": ["hi"],
            "cwd": null,
            "timeout_seconds": null,
            "plan": null,
            "routing_mode": null,
            "on_timeout": null,
            "on_heartbeat_miss": null
        }"#;
        let task: SessionTask = serde_json::from_str(json).unwrap();
        assert_eq!(task.heartbeat_interval_seconds, 30);
        assert!(!task.interactive);
        assert!(!task.no_room_output);
        assert!(task.required_capabilities.is_empty());
    }

    // --- SessionStart tests ---

    #[test]
    fn session_start_roundtrip() {
        let evt = SessionStart {
            session_uuid: "s-1".into(),
            worker_id: "@worker:example.com".into(),
            tmux_session: Some("tmux-abc".into()),
            pid: Some(12345),
            started_at: 1742572800,
        };
        let json = serde_json::to_string(&evt).unwrap();
        let back: SessionStart = serde_json::from_str(&json).unwrap();
        assert_eq!(back.session_uuid, "s-1");
        assert_eq!(back.worker_id, "@worker:example.com");
        assert_eq!(back.tmux_session, Some("tmux-abc".into()));
        assert_eq!(back.pid, Some(12345));
        assert_eq!(back.started_at, 1742572800);
    }

    #[test]
    fn session_start_snake_case_fields() {
        let evt = SessionStart {
            session_uuid: "s-2".into(),
            worker_id: "@w:example.com".into(),
            tmux_session: None,
            pid: None,
            started_at: 0,
        };
        let json = serde_json::to_string(&evt).unwrap();
        assert!(json.contains("session_uuid"));
        assert!(json.contains("worker_id"));
        assert!(json.contains("tmux_session"));
        assert!(json.contains("started_at"));
    }

    // --- SessionOutput tests ---

    #[test]
    fn session_output_roundtrip() {
        let evt = SessionOutput {
            session_uuid: "s-1".into(),
            worker_id: "@worker:example.com".into(),
            stream: OutputStream::Stdout,
            data: "Hello, world!\n".into(),
            seq: 1,
            timestamp: 1742572801,
        };
        let json = serde_json::to_string(&evt).unwrap();
        let back: SessionOutput = serde_json::from_str(&json).unwrap();
        assert_eq!(back.session_uuid, "s-1");
        assert_eq!(back.stream, OutputStream::Stdout);
        assert_eq!(back.data, "Hello, world!\n");
        assert_eq!(back.seq, 1);
    }

    #[test]
    fn session_output_stderr() {
        let evt = SessionOutput {
            session_uuid: "s-1".into(),
            worker_id: "@worker:example.com".into(),
            stream: OutputStream::Stderr,
            data: "error: something failed\n".into(),
            seq: 2,
            timestamp: 1742572802,
        };
        let json = serde_json::to_string(&evt).unwrap();
        assert!(json.contains(r#""stderr""#));
        let back: SessionOutput = serde_json::from_str(&json).unwrap();
        assert_eq!(back.stream, OutputStream::Stderr);
    }

    // --- SessionHeartbeat tests ---

    #[test]
    fn session_heartbeat_roundtrip() {
        let evt = SessionHeartbeat {
            session_uuid: "s-1".into(),
            worker_id: "@worker:example.com".into(),
            timestamp: 1742572830,
            progress: Some("50% complete".into()),
        };
        let json = serde_json::to_string(&evt).unwrap();
        let back: SessionHeartbeat = serde_json::from_str(&json).unwrap();
        assert_eq!(back.session_uuid, "s-1");
        assert_eq!(back.progress, Some("50% complete".into()));
        assert_eq!(back.timestamp, 1742572830);
    }

    #[test]
    fn session_heartbeat_no_progress() {
        let evt = SessionHeartbeat {
            session_uuid: "s-1".into(),
            worker_id: "@worker:example.com".into(),
            timestamp: 1742572860,
            progress: None,
        };
        let json = serde_json::to_string(&evt).unwrap();
        let back: SessionHeartbeat = serde_json::from_str(&json).unwrap();
        assert_eq!(back.progress, None);
    }

    // --- SessionResult tests ---

    #[test]
    fn session_result_success_roundtrip() {
        let evt = SessionResult {
            session_uuid: "s-1".into(),
            worker_id: "@worker:example.com".into(),
            status: SessionStatus::Success,
            exit_code: Some(0),
            duration_seconds: 120,
            tail: Some("Build complete".into()),
        };
        let json = serde_json::to_string(&evt).unwrap();
        let back: SessionResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.status, SessionStatus::Success);
        assert_eq!(back.exit_code, Some(0));
        assert_eq!(back.duration_seconds, 120);
        assert_eq!(back.tail, Some("Build complete".into()));
    }

    #[test]
    fn session_result_failed() {
        let evt = SessionResult {
            session_uuid: "s-2".into(),
            worker_id: "@worker:example.com".into(),
            status: SessionStatus::Failed,
            exit_code: Some(1),
            duration_seconds: 5,
            tail: Some("compilation error".into()),
        };
        let json = serde_json::to_string(&evt).unwrap();
        let back: SessionResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.status, SessionStatus::Failed);
        assert_eq!(back.exit_code, Some(1));
    }

    #[test]
    fn session_result_timeout() {
        let evt = SessionResult {
            session_uuid: "s-3".into(),
            worker_id: "@worker:example.com".into(),
            status: SessionStatus::Timeout,
            exit_code: None,
            duration_seconds: 3600,
            tail: None,
        };
        let json = serde_json::to_string(&evt).unwrap();
        let back: SessionResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.status, SessionStatus::Timeout);
        assert_eq!(back.exit_code, None);
    }

    #[test]
    fn session_result_snake_case_fields() {
        let evt = SessionResult {
            session_uuid: "s-4".into(),
            worker_id: "@w:example.com".into(),
            status: SessionStatus::Cancelled,
            exit_code: None,
            duration_seconds: 0,
            tail: None,
        };
        let json = serde_json::to_string(&evt).unwrap();
        assert!(json.contains("session_uuid"));
        assert!(json.contains("worker_id"));
        assert!(json.contains("exit_code"));
        assert!(json.contains("duration_seconds"));
    }

    // --- SessionInput tests ---

    #[test]
    fn session_input_roundtrip() {
        let evt = SessionInput {
            session_uuid: "s-1".into(),
            data: "ls -la\n".into(),
        };
        let json = serde_json::to_string(&evt).unwrap();
        let back: SessionInput = serde_json::from_str(&json).unwrap();
        assert_eq!(back.session_uuid, "s-1");
        assert_eq!(back.data, "ls -la\n");
    }

    // --- SessionSignal tests ---

    #[test]
    fn session_signal_roundtrip() {
        let evt = SessionSignal {
            session_uuid: "s-1".into(),
            signal: "SIGTERM".into(),
        };
        let json = serde_json::to_string(&evt).unwrap();
        let back: SessionSignal = serde_json::from_str(&json).unwrap();
        assert_eq!(back.session_uuid, "s-1");
        assert_eq!(back.signal, "SIGTERM");
    }

    // --- SessionResize tests ---

    #[test]
    fn session_resize_roundtrip() {
        let evt = SessionResize {
            session_uuid: "s-1".into(),
            cols: 120,
            rows: 40,
        };
        let json = serde_json::to_string(&evt).unwrap();
        let back: SessionResize = serde_json::from_str(&json).unwrap();
        assert_eq!(back.session_uuid, "s-1");
        assert_eq!(back.cols, 120);
        assert_eq!(back.rows, 40);
    }

    // --- SessionCancel tests ---

    #[test]
    fn session_cancel_roundtrip() {
        let evt = SessionCancel {
            session_uuid: "s-1".into(),
            reason: Some("user requested".into()),
            grace_seconds: Some(5),
        };
        let json = serde_json::to_string(&evt).unwrap();
        let back: SessionCancel = serde_json::from_str(&json).unwrap();
        assert_eq!(back.session_uuid, "s-1");
        assert_eq!(back.reason, Some("user requested".into()));
        assert_eq!(back.grace_seconds, Some(5));
    }

    #[test]
    fn session_cancel_no_reason() {
        let evt = SessionCancel {
            session_uuid: "s-2".into(),
            reason: None,
            grace_seconds: None,
        };
        let json = serde_json::to_string(&evt).unwrap();
        let back: SessionCancel = serde_json::from_str(&json).unwrap();
        assert_eq!(back.reason, None);
        assert_eq!(back.grace_seconds, None);
    }

    // --- Enum serialization tests ---

    #[test]
    fn output_stream_serializes_snake_case() {
        let json = serde_json::to_string(&OutputStream::Stdout).unwrap();
        assert_eq!(json, r#""stdout""#);
        let json = serde_json::to_string(&OutputStream::Stderr).unwrap();
        assert_eq!(json, r#""stderr""#);
    }

    #[test]
    fn output_stream_rejects_unknown() {
        let result: Result<OutputStream, _> = serde_json::from_str(r#""stdin""#);
        assert!(result.is_err());
    }

    #[test]
    fn session_status_serializes_snake_case() {
        assert_eq!(serde_json::to_string(&SessionStatus::Success).unwrap(), r#""success""#);
        assert_eq!(serde_json::to_string(&SessionStatus::Failed).unwrap(), r#""failed""#);
        assert_eq!(serde_json::to_string(&SessionStatus::Timeout).unwrap(), r#""timeout""#);
        assert_eq!(serde_json::to_string(&SessionStatus::Cancelled).unwrap(), r#""cancelled""#);
    }

    #[test]
    fn session_status_rejects_unknown() {
        let result: Result<SessionStatus, _> = serde_json::from_str(r#""exploded""#);
        assert!(result.is_err());
    }

    // --- ActiveSessionState tests ---

    #[test]
    fn active_session_state_roundtrip() {
        let state = ActiveSessionState {
            bin: "claude".into(),
            args: vec!["--model".into(), "opus".into()],
            pid: Some(42567),
            start_time: 1742572800,
            client_id: "@alice:example.com".into(),
            interactive: true,
            worker_id: "@bel-worker:ca1-beta.mxdx.dev".into(),
        };
        let json = serde_json::to_string(&state).unwrap();
        let back: ActiveSessionState = serde_json::from_str(&json).unwrap();
        assert_eq!(back.bin, "claude");
        assert_eq!(back.args, vec!["--model", "opus"]);
        assert_eq!(back.pid, Some(42567));
        assert_eq!(back.start_time, 1742572800);
        assert_eq!(back.client_id, "@alice:example.com");
        assert!(back.interactive);
        assert_eq!(back.worker_id, "@bel-worker:ca1-beta.mxdx.dev");
    }

    #[test]
    fn active_session_state_snake_case_fields() {
        let state = ActiveSessionState {
            bin: "echo".into(),
            args: vec![],
            pid: None,
            start_time: 0,
            client_id: "@c:example.com".into(),
            interactive: false,
            worker_id: "@w:example.com".into(),
        };
        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("start_time"), "expected snake_case in: {json}");
        assert!(json.contains("client_id"), "expected snake_case in: {json}");
        assert!(json.contains("worker_id"), "expected snake_case in: {json}");
    }

    // --- CompletedSessionState tests ---

    #[test]
    fn completed_session_state_roundtrip() {
        let state = CompletedSessionState {
            exit_code: Some(0),
            duration_seconds: 120,
            completion_time: 1742572920,
        };
        let json = serde_json::to_string(&state).unwrap();
        let back: CompletedSessionState = serde_json::from_str(&json).unwrap();
        assert_eq!(back.exit_code, Some(0));
        assert_eq!(back.duration_seconds, 120);
        assert_eq!(back.completion_time, 1742572920);
    }

    #[test]
    fn completed_session_state_no_exit_code() {
        let state = CompletedSessionState {
            exit_code: None,
            duration_seconds: 3600,
            completion_time: 1742576400,
        };
        let json = serde_json::to_string(&state).unwrap();
        let back: CompletedSessionState = serde_json::from_str(&json).unwrap();
        assert_eq!(back.exit_code, None);
        assert_eq!(back.duration_seconds, 3600);
    }

    #[test]
    fn completed_session_state_snake_case_fields() {
        let state = CompletedSessionState {
            exit_code: Some(1),
            duration_seconds: 5,
            completion_time: 1742572805,
        };
        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("exit_code"), "expected snake_case in: {json}");
        assert!(json.contains("duration_seconds"), "expected snake_case in: {json}");
        assert!(json.contains("completion_time"), "expected snake_case in: {json}");
    }
}
