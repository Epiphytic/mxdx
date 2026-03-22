use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskEvent {
    pub uuid: String,
    pub sender_id: String,
    pub required_capabilities: Vec<String>,
    pub estimated_cycles: Option<u64>,
    pub timeout_seconds: u64,
    pub heartbeat_interval_seconds: u64,
    pub on_timeout: FailurePolicy,
    pub on_heartbeat_miss: FailurePolicy,
    pub routing_mode: RoutingMode,
    pub p2p_stream: bool,
    pub payload: serde_json::Value,
    pub plan: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CapabilityEvent {
    pub worker_id: String,
    pub capabilities: Vec<String>,
    pub max_concurrent_tasks: u8,
    pub current_task_count: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClaimEvent {
    pub task_uuid: String,
    pub worker_id: String,
    pub claimed_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HeartbeatEvent {
    pub task_uuid: String,
    pub worker_id: String,
    pub progress: Option<String>,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskResultEvent {
    pub task_uuid: String,
    pub worker_id: String,
    pub status: TaskStatus,
    pub output: Option<serde_json::Value>,
    pub error: Option<String>,
    pub duration_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum FailurePolicy {
    Escalate,
    Respawn { max_retries: u8 },
    RespawnWithContext,
    Abandon,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RoutingMode {
    Direct,
    Brokered,
    Auto,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Success,
    Failed,
    Timeout,
    Cancelled,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;

    #[test]
    fn task_event_round_trips_json() {
        let evt = TaskEvent {
            uuid: "550e8400-e29b-41d4-a716-446655440000".into(),
            sender_id: "@alice:example.com".into(),
            required_capabilities: vec!["rust".into(), "linux".into()],
            estimated_cycles: Some(1000),
            timeout_seconds: 3600,
            heartbeat_interval_seconds: 30,
            on_timeout: FailurePolicy::Escalate,
            on_heartbeat_miss: FailurePolicy::Respawn { max_retries: 3 },
            routing_mode: RoutingMode::Auto,
            p2p_stream: false,
            payload: serde_json::json!({"cmd": "cargo build"}),
            plan: Some("Build the workspace".into()),
        };
        let json = serde_json::to_string(&evt).unwrap();
        let parsed: TaskEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.uuid, evt.uuid);
        assert_eq!(parsed.sender_id, "@alice:example.com");
        assert_eq!(parsed.required_capabilities, evt.required_capabilities);
        assert_eq!(parsed.estimated_cycles, Some(1000));
        assert_eq!(parsed.timeout_seconds, 3600);
        assert_eq!(parsed.heartbeat_interval_seconds, 30);
        assert_eq!(parsed.on_timeout, FailurePolicy::Escalate);
        assert_eq!(
            parsed.on_heartbeat_miss,
            FailurePolicy::Respawn { max_retries: 3 }
        );
        assert_eq!(parsed.routing_mode, RoutingMode::Auto);
        assert!(!parsed.p2p_stream);
        assert_eq!(parsed.plan, Some("Build the workspace".into()));
    }

    #[test]
    fn task_event_optional_fields_null() {
        let evt = TaskEvent {
            uuid: "task-2".into(),
            sender_id: "@bob:example.com".into(),
            required_capabilities: vec![],
            estimated_cycles: None,
            timeout_seconds: 60,
            heartbeat_interval_seconds: 10,
            on_timeout: FailurePolicy::Abandon,
            on_heartbeat_miss: FailurePolicy::RespawnWithContext,
            routing_mode: RoutingMode::Direct,
            p2p_stream: true,
            payload: serde_json::Value::Null,
            plan: None,
        };
        let json = serde_json::to_string(&evt).unwrap();
        let parsed: TaskEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.estimated_cycles, None);
        assert_eq!(parsed.plan, None);
        assert!(parsed.p2p_stream);
    }

    #[test]
    fn capability_event_round_trips_json() {
        let evt = CapabilityEvent {
            worker_id: "@jcode-worker:belthanior".into(),
            capabilities: vec!["rust".into(), "linux".into(), "arm64".into()],
            max_concurrent_tasks: 4,
            current_task_count: 1,
        };
        let json = serde_json::to_string(&evt).unwrap();
        let parsed: CapabilityEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.worker_id, evt.worker_id);
        assert_eq!(parsed.capabilities, evt.capabilities);
        assert_eq!(parsed.max_concurrent_tasks, 4);
        assert_eq!(parsed.current_task_count, 1);
    }

    #[test]
    fn claim_event_round_trips_json() {
        let evt = ClaimEvent {
            task_uuid: "550e8400-e29b-41d4-a716-446655440000".into(),
            worker_id: "@worker-01:belthanior".into(),
            claimed_at: 1742572800,
        };
        let json = serde_json::to_string(&evt).unwrap();
        let parsed: ClaimEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.task_uuid, evt.task_uuid);
        assert_eq!(parsed.worker_id, evt.worker_id);
        assert_eq!(parsed.claimed_at, 1742572800);
    }

    #[test]
    fn heartbeat_event_round_trips_json() {
        let evt = HeartbeatEvent {
            task_uuid: "task-abc".into(),
            worker_id: "@worker-01:belthanior".into(),
            progress: Some("50% complete".into()),
            timestamp: 1742572830,
        };
        let json = serde_json::to_string(&evt).unwrap();
        let parsed: HeartbeatEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.task_uuid, evt.task_uuid);
        assert_eq!(parsed.worker_id, evt.worker_id);
        assert_eq!(parsed.progress, Some("50% complete".into()));
        assert_eq!(parsed.timestamp, 1742572830);
    }

    #[test]
    fn heartbeat_event_no_progress() {
        let evt = HeartbeatEvent {
            task_uuid: "task-abc".into(),
            worker_id: "@worker-01:belthanior".into(),
            progress: None,
            timestamp: 1742572860,
        };
        let json = serde_json::to_string(&evt).unwrap();
        let parsed: HeartbeatEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.progress, None);
    }

    #[test]
    fn task_result_event_round_trips_json() {
        let evt = TaskResultEvent {
            task_uuid: "task-abc".into(),
            worker_id: "@worker-01:belthanior".into(),
            status: TaskStatus::Success,
            output: Some(serde_json::json!({"artifacts": ["build/output.wasm"]})),
            error: None,
            duration_seconds: 120,
        };
        let json = serde_json::to_string(&evt).unwrap();
        let parsed: TaskResultEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.task_uuid, evt.task_uuid);
        assert_eq!(parsed.worker_id, evt.worker_id);
        assert_eq!(parsed.status, TaskStatus::Success);
        assert!(parsed.output.is_some());
        assert_eq!(parsed.error, None);
        assert_eq!(parsed.duration_seconds, 120);
    }

    #[test]
    fn task_result_event_failed_with_error() {
        let evt = TaskResultEvent {
            task_uuid: "task-xyz".into(),
            worker_id: "@worker-02:belthanior".into(),
            status: TaskStatus::Failed,
            output: None,
            error: Some("compilation error: missing crate".into()),
            duration_seconds: 5,
        };
        let json = serde_json::to_string(&evt).unwrap();
        let parsed: TaskResultEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.status, TaskStatus::Failed);
        assert_eq!(
            parsed.error,
            Some("compilation error: missing crate".into())
        );
        assert_eq!(parsed.output, None);
    }

    #[test]
    fn failure_policy_serializes_snake_case() {
        let json = serde_json::to_string(&FailurePolicy::Escalate).unwrap();
        assert_eq!(json, r#""escalate""#);

        let json = serde_json::to_string(&FailurePolicy::Abandon).unwrap();
        assert_eq!(json, r#""abandon""#);

        let json = serde_json::to_string(&FailurePolicy::RespawnWithContext).unwrap();
        assert_eq!(json, r#""respawn_with_context""#);

        let json = serde_json::to_string(&FailurePolicy::Respawn { max_retries: 5 }).unwrap();
        assert!(json.contains("respawn"));
        assert!(json.contains("max_retries"));
        let parsed: FailurePolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, FailurePolicy::Respawn { max_retries: 5 });
    }

    #[test]
    fn failure_policy_rejects_unknown_variant() {
        let json = r#""explode""#;
        let result: Result<FailurePolicy, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn routing_mode_serializes_snake_case() {
        let json = serde_json::to_string(&RoutingMode::Direct).unwrap();
        assert_eq!(json, r#""direct""#);

        let json = serde_json::to_string(&RoutingMode::Brokered).unwrap();
        assert_eq!(json, r#""brokered""#);

        let json = serde_json::to_string(&RoutingMode::Auto).unwrap();
        assert_eq!(json, r#""auto""#);
    }

    #[test]
    fn routing_mode_rejects_unknown_variant() {
        let json = r#""teleport""#;
        let result: Result<RoutingMode, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn task_status_serializes_snake_case() {
        let json = serde_json::to_string(&TaskStatus::Success).unwrap();
        assert_eq!(json, r#""success""#);

        let json = serde_json::to_string(&TaskStatus::Failed).unwrap();
        assert_eq!(json, r#""failed""#);

        let json = serde_json::to_string(&TaskStatus::Timeout).unwrap();
        assert_eq!(json, r#""timeout""#);

        let json = serde_json::to_string(&TaskStatus::Cancelled).unwrap();
        assert_eq!(json, r#""cancelled""#);
    }

    #[test]
    fn task_status_rejects_unknown_variant() {
        let json = r#""exploded""#;
        let result: Result<TaskStatus, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn task_event_no_callback_field() {
        let evt = TaskEvent {
            uuid: "task-no-cb".into(),
            sender_id: "@bob:example.com".into(),
            required_capabilities: vec![],
            estimated_cycles: None,
            timeout_seconds: 60,
            heartbeat_interval_seconds: 10,
            on_timeout: FailurePolicy::Escalate,
            on_heartbeat_miss: FailurePolicy::Escalate,
            routing_mode: RoutingMode::Auto,
            p2p_stream: false,
            payload: serde_json::Value::Null,
            plan: None,
        };
        let json = serde_json::to_string(&evt).unwrap();
        assert!(
            !json.contains("callback"),
            "callback field should not exist in serialized TaskEvent"
        );
    }

    #[test]
    fn task_result_event_no_callback_field() {
        let result = TaskResultEvent {
            task_uuid: "task-no-cb".into(),
            worker_id: "@worker:example.com".into(),
            status: TaskStatus::Failed,
            output: None,
            error: Some("oops".into()),
            duration_seconds: 1,
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(
            !json.contains("callback"),
            "callback field should not exist in serialized TaskResultEvent"
        );
    }
}
