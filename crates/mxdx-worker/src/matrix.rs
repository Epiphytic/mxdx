use anyhow::Result;
use serde::{de::DeserializeOwned, Serialize};

/// Abstraction over Matrix room operations for the worker.
/// This trait allows testing with mocks without requiring a real Matrix server.
pub trait WorkerRoomOps: Send + Sync {
    /// Create or find the worker's room (E2EE, named per config)
    fn get_or_create_room(
        &self,
        room_name: &str,
    ) -> impl std::future::Future<Output = Result<String>> + Send;

    /// Post threaded event to a session's thread
    fn post_to_thread(
        &self,
        room_id: &str,
        thread_root: &str,
        event_type: &str,
        content: serde_json::Value,
    ) -> impl std::future::Future<Output = Result<String>> + Send;

    /// Write state event (used for claims, session tracking, worker info)
    fn write_state(
        &self,
        room_id: &str,
        event_type: &str,
        state_key: &str,
        content: serde_json::Value,
    ) -> impl std::future::Future<Output = Result<()>> + Send;

    /// Read state event
    fn read_state(
        &self,
        room_id: &str,
        event_type: &str,
        state_key: &str,
    ) -> impl std::future::Future<Output = Result<Option<serde_json::Value>>> + Send;

    /// Remove state event (post empty content)
    fn remove_state(
        &self,
        room_id: &str,
        event_type: &str,
        state_key: &str,
    ) -> impl std::future::Future<Output = Result<()>> + Send;
}

/// Concrete implementation wrapping mxdx-matrix client.
///
/// Holds the room_id for the worker's Matrix room. The actual Matrix SDK
/// integration (via `mxdx-matrix::MatrixClient`) will be wired up in Task 2.13.
pub struct WorkerRoom {
    room_id: String,
}

impl WorkerRoom {
    pub fn new(room_id: String) -> Self {
        Self { room_id }
    }

    pub fn room_id(&self) -> &str {
        &self.room_id
    }
}

/// Helper to serialize a typed event into a JSON Value for posting.
pub fn serialize_event<T: Serialize>(event: &T) -> Result<serde_json::Value> {
    Ok(serde_json::to_value(event)?)
}

/// Helper to deserialize a JSON Value into a typed event.
pub fn deserialize_event<T: DeserializeOwned>(value: &serde_json::Value) -> Result<T> {
    Ok(serde_json::from_value(value.clone())?)
}

/// Incoming event types that the worker needs to handle from sync.
#[derive(Debug, Clone)]
pub enum IncomingEvent {
    /// A new task submission
    TaskSubmission {
        event_id: String,
        content: serde_json::Value,
    },
    /// Client sending input to an active session
    SessionInput {
        session_uuid: String,
        content: serde_json::Value,
    },
    /// Client sending a signal to an active session
    SessionSignal {
        session_uuid: String,
        content: serde_json::Value,
    },
    /// Client resizing a session terminal
    SessionResize {
        session_uuid: String,
        content: serde_json::Value,
    },
    /// Client cancelling a session
    SessionCancel {
        session_uuid: String,
        content: serde_json::Value,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use mxdx_types::events::session::SessionTask;

    #[test]
    fn serialize_session_task_to_json_value() {
        let task = SessionTask {
            uuid: "test-uuid-1234".to_string(),
            sender_id: "@worker:example.com".to_string(),
            bin: "/bin/echo".to_string(),
            args: vec!["hello".to_string()],
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
        };

        let value = serialize_event(&task).expect("serialization should succeed");
        assert_eq!(value["uuid"], "test-uuid-1234");
        assert_eq!(value["bin"], "/bin/echo");
        assert_eq!(value["args"], serde_json::json!(["hello"]));
        assert_eq!(value["timeout_seconds"], 60);
    }

    #[test]
    fn deserialize_json_value_to_session_task() {
        let json = serde_json::json!({
            "uuid": "test-uuid-5678",
            "sender_id": "@client:example.com",
            "bin": "/usr/bin/ls",
            "args": ["-la"],
            "interactive": false,
            "no_room_output": false,
            "heartbeat_interval_seconds": 30,
        });

        let task: SessionTask = deserialize_event(&json).expect("deserialization should succeed");
        assert_eq!(task.uuid, "test-uuid-5678");
        assert_eq!(task.bin, "/usr/bin/ls");
        assert_eq!(task.args, vec!["-la"]);
        assert!(!task.interactive);
        assert!(task.env.is_none());
        assert!(task.timeout_seconds.is_none());
    }

    #[test]
    fn incoming_event_variants_construct_and_match() {
        let events = vec![
            IncomingEvent::TaskSubmission {
                event_id: "$evt1".to_string(),
                content: serde_json::json!({"bin": "/bin/sh"}),
            },
            IncomingEvent::SessionInput {
                session_uuid: "uuid-1".to_string(),
                content: serde_json::json!({"data": "ls\n"}),
            },
            IncomingEvent::SessionSignal {
                session_uuid: "uuid-2".to_string(),
                content: serde_json::json!({"signal": 15}),
            },
            IncomingEvent::SessionResize {
                session_uuid: "uuid-3".to_string(),
                content: serde_json::json!({"cols": 120, "rows": 40}),
            },
            IncomingEvent::SessionCancel {
                session_uuid: "uuid-4".to_string(),
                content: serde_json::json!({"reason": "user request"}),
            },
        ];

        let mut task_count = 0;
        let mut input_count = 0;
        let mut signal_count = 0;
        let mut resize_count = 0;
        let mut cancel_count = 0;

        for event in &events {
            match event {
                IncomingEvent::TaskSubmission { event_id, .. } => {
                    assert_eq!(event_id, "$evt1");
                    task_count += 1;
                }
                IncomingEvent::SessionInput { session_uuid, .. } => {
                    assert_eq!(session_uuid, "uuid-1");
                    input_count += 1;
                }
                IncomingEvent::SessionSignal { session_uuid, .. } => {
                    assert_eq!(session_uuid, "uuid-2");
                    signal_count += 1;
                }
                IncomingEvent::SessionResize { session_uuid, .. } => {
                    assert_eq!(session_uuid, "uuid-3");
                    resize_count += 1;
                }
                IncomingEvent::SessionCancel { session_uuid, .. } => {
                    assert_eq!(session_uuid, "uuid-4");
                    cancel_count += 1;
                }
            }
        }

        assert_eq!(task_count, 1);
        assert_eq!(input_count, 1);
        assert_eq!(signal_count, 1);
        assert_eq!(resize_count, 1);
        assert_eq!(cancel_count, 1);
    }

    #[test]
    fn worker_room_stores_and_returns_room_id() {
        let room = WorkerRoom::new("!abc123:example.com".to_string());
        assert_eq!(room.room_id(), "!abc123:example.com");

        let room2 = WorkerRoom::new("!xyz789:matrix.org".to_string());
        assert_eq!(room2.room_id(), "!xyz789:matrix.org");
    }
}
