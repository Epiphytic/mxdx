//! Backward compatibility layer for translating legacy `org.mxdx.fabric.task`
//! events into the new `SessionTask` format.
//!
//! This module allows pre-migration clients that still emit `TaskEvent` payloads
//! to work with workers that expect `SessionTask`. It will be removed when all
//! clients have migrated to the unified session architecture.

use mxdx_types::events::fabric::TaskEvent;
use mxdx_types::events::session::SessionTask;

/// Translate a legacy `TaskEvent` (`org.mxdx.fabric.task`) to the new `SessionTask` format.
///
/// Field mapping:
/// - `uuid`, `sender_id`, `plan` — copied directly
/// - `required_capabilities` — copied directly
/// - `payload` — if the JSON value contains a `"cmd"` string, it becomes `bin`
///   with args split on whitespace; otherwise `bin` is empty and `args` is empty
/// - `p2p_stream` — maps to `interactive`
/// - `timeout_seconds` — wrapped in `Some()`
/// - `heartbeat_interval_seconds` — copied directly
/// - `on_timeout`, `on_heartbeat_miss` — wrapped in `Some()`
/// - `routing_mode` — wrapped in `Some()`
/// - `no_room_output` — defaults to `false` (legacy events had no equivalent)
/// - `env`, `cwd` — `None` (legacy events had no equivalent)
pub fn translate_legacy_task(fabric_task: &TaskEvent) -> SessionTask {
    let (bin, args) = extract_bin_args(&fabric_task.payload);

    SessionTask {
        uuid: fabric_task.uuid.clone(),
        sender_id: fabric_task.sender_id.clone(),
        bin,
        args,
        env: None,
        cwd: None,
        interactive: fabric_task.p2p_stream,
        no_room_output: false,
        timeout_seconds: Some(fabric_task.timeout_seconds),
        heartbeat_interval_seconds: fabric_task.heartbeat_interval_seconds,
        plan: fabric_task.plan.clone(),
        required_capabilities: fabric_task.required_capabilities.clone(),
        routing_mode: Some(fabric_task.routing_mode.clone()),
        on_timeout: Some(fabric_task.on_timeout.clone()),
        on_heartbeat_miss: Some(fabric_task.on_heartbeat_miss.clone()),
    }
}

/// Extract `bin` and `args` from a legacy task payload.
///
/// Legacy tasks stored the command in the payload JSON under a `"cmd"` key
/// as a single string (e.g. `{"cmd": "cargo build --release"}`).
/// We split on whitespace: the first token is `bin`, the rest are `args`.
///
/// If the payload has no `"cmd"` key or is null, returns empty bin and args.
fn extract_bin_args(payload: &serde_json::Value) -> (String, Vec<String>) {
    if let Some(cmd) = payload.get("cmd").and_then(|v| v.as_str()) {
        let mut parts = cmd.split_whitespace();
        let bin = parts.next().unwrap_or_default().to_string();
        let args: Vec<String> = parts.map(|s| s.to_string()).collect();
        (bin, args)
    } else {
        (String::new(), Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mxdx_types::events::fabric::{FailurePolicy, RoutingMode, TaskEvent};

    fn basic_task() -> TaskEvent {
        TaskEvent {
            uuid: "task-basic-001".into(),
            sender_id: "@alice:example.com".into(),
            required_capabilities: vec!["linux".into()],
            estimated_cycles: None,
            timeout_seconds: 60,
            heartbeat_interval_seconds: 10,
            on_timeout: FailurePolicy::Escalate,
            on_heartbeat_miss: FailurePolicy::Abandon,
            routing_mode: RoutingMode::Auto,
            p2p_stream: false,
            payload: serde_json::json!({"cmd": "echo hello"}),
            plan: None,
        }
    }

    #[test]
    fn translate_basic_legacy_task() {
        let legacy = basic_task();
        let session = translate_legacy_task(&legacy);

        assert_eq!(session.uuid, "task-basic-001");
        assert_eq!(session.sender_id, "@alice:example.com");
        assert_eq!(session.bin, "echo");
        assert_eq!(session.args, vec!["hello"]);
        assert!(!session.interactive);
        assert!(!session.no_room_output);
        assert_eq!(session.env, None);
        assert_eq!(session.cwd, None);
        assert_eq!(session.required_capabilities, vec!["linux"]);
        assert_eq!(session.routing_mode, Some(RoutingMode::Auto));
    }

    #[test]
    fn translate_task_with_payload_timeout_heartbeat() {
        let legacy = TaskEvent {
            uuid: "task-full-002".into(),
            sender_id: "@bob:example.com".into(),
            required_capabilities: vec!["rust".into(), "arm64".into()],
            estimated_cycles: Some(5000),
            timeout_seconds: 3600,
            heartbeat_interval_seconds: 30,
            on_timeout: FailurePolicy::Respawn { max_retries: 3 },
            on_heartbeat_miss: FailurePolicy::RespawnWithContext,
            routing_mode: RoutingMode::Brokered,
            p2p_stream: false,
            payload: serde_json::json!({"cmd": "cargo build --release --target aarch64-unknown-linux-gnu"}),
            plan: None,
        };

        let session = translate_legacy_task(&legacy);

        assert_eq!(session.bin, "cargo");
        assert_eq!(
            session.args,
            vec!["build", "--release", "--target", "aarch64-unknown-linux-gnu"]
        );
        assert_eq!(session.timeout_seconds, Some(3600));
        assert_eq!(session.heartbeat_interval_seconds, 30);
        assert_eq!(
            session.on_timeout,
            Some(FailurePolicy::Respawn { max_retries: 3 })
        );
        assert_eq!(
            session.on_heartbeat_miss,
            Some(FailurePolicy::RespawnWithContext)
        );
        assert_eq!(session.routing_mode, Some(RoutingMode::Brokered));
    }

    #[test]
    fn translate_task_p2p_stream_maps_to_interactive() {
        let legacy = TaskEvent {
            p2p_stream: true,
            ..basic_task()
        };

        let session = translate_legacy_task(&legacy);
        assert!(session.interactive);
    }

    #[test]
    fn translate_task_with_plan_field() {
        let legacy = TaskEvent {
            plan: Some("Deploy the v2 release to staging".into()),
            ..basic_task()
        };

        let session = translate_legacy_task(&legacy);
        assert_eq!(
            session.plan,
            Some("Deploy the v2 release to staging".into())
        );
    }

    #[test]
    fn translate_task_null_payload_gives_empty_bin() {
        let legacy = TaskEvent {
            payload: serde_json::Value::Null,
            ..basic_task()
        };

        let session = translate_legacy_task(&legacy);
        assert_eq!(session.bin, "");
        assert!(session.args.is_empty());
    }

    #[test]
    fn translate_task_payload_without_cmd_gives_empty_bin() {
        let legacy = TaskEvent {
            payload: serde_json::json!({"data": "some other format"}),
            ..basic_task()
        };

        let session = translate_legacy_task(&legacy);
        assert_eq!(session.bin, "");
        assert!(session.args.is_empty());
    }
}
