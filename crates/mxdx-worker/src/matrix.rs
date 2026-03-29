use std::time::Duration;

use anyhow::Result;
use mxdx_matrix::{MatrixClient, MultiHsClient};
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

/// Live Matrix-backed implementation of `WorkerRoomOps`.
/// Wraps an `mxdx_matrix::MultiHsClient` for multi-homeserver failover
/// and a pre-resolved room ID.
pub struct MatrixWorkerRoom {
    multi: MultiHsClient,
    room_id: mxdx_matrix::OwnedRoomId,
}

impl MatrixWorkerRoom {
    pub fn new(multi: MultiHsClient, room_id: mxdx_matrix::OwnedRoomId) -> Self {
        Self { multi, room_id }
    }

    /// Construct from a single `MatrixClient` (backward compat / testing).
    pub fn from_single_client(client: MatrixClient, room_id: mxdx_matrix::OwnedRoomId) -> Self {
        let server = "single".to_string();
        let multi = MultiHsClient::from_clients(vec![(server, client, 0.0)], None);
        Self { multi, room_id }
    }

    pub fn room_id(&self) -> &mxdx_matrix::RoomId {
        &self.room_id
    }

    /// Access the preferred (active) `MatrixClient` for operations not
    /// yet wrapped by `MultiHsClient` (e.g., state reads).
    pub fn client(&self) -> &MatrixClient {
        self.multi.preferred()
    }

    /// Access the `MultiHsClient` mutably (for send operations with failover).
    pub fn multi(&mut self) -> &mut MultiHsClient {
        &mut self.multi
    }

    /// Number of connected homeservers.
    pub fn server_count(&self) -> usize {
        self.multi.server_count()
    }

    /// Sync and parse incoming events into `IncomingEvent` variants.
    /// This is not part of the `WorkerRoomOps` trait because it is
    /// specific to the live Matrix implementation.
    pub async fn sync_events(&mut self, timeout: Duration) -> Result<Vec<IncomingEvent>> {
        let raw_events = self
            .multi
            .sync_and_collect_events(&self.room_id, timeout)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        let mut parsed = Vec::new();
        for event in raw_events {
            let event_type = event
                .get("type")
                .and_then(|t| t.as_str())
                .unwrap_or("");
            let content = event.get("content").cloned().unwrap_or(serde_json::json!({}));

            match event_type {
                "org.mxdx.session.task" => {
                    let event_id = event
                        .get("event_id")
                        .and_then(|e| e.as_str())
                        .unwrap_or("")
                        .to_string();
                    parsed.push(IncomingEvent::TaskSubmission { event_id, content });
                }
                "org.mxdx.session.input" => {
                    let session_uuid = content
                        .get("session_uuid")
                        .and_then(|s| s.as_str())
                        .unwrap_or("")
                        .to_string();
                    parsed.push(IncomingEvent::SessionInput {
                        session_uuid,
                        content,
                    });
                }
                "org.mxdx.session.signal" => {
                    let session_uuid = content
                        .get("session_uuid")
                        .and_then(|s| s.as_str())
                        .unwrap_or("")
                        .to_string();
                    parsed.push(IncomingEvent::SessionSignal {
                        session_uuid,
                        content,
                    });
                }
                "org.mxdx.session.resize" => {
                    let session_uuid = content
                        .get("session_uuid")
                        .and_then(|s| s.as_str())
                        .unwrap_or("")
                        .to_string();
                    parsed.push(IncomingEvent::SessionResize {
                        session_uuid,
                        content,
                    });
                }
                "org.mxdx.session.cancel" => {
                    let session_uuid = content
                        .get("session_uuid")
                        .and_then(|s| s.as_str())
                        .unwrap_or("")
                        .to_string();
                    parsed.push(IncomingEvent::SessionCancel {
                        session_uuid,
                        content,
                    });
                }
                _ => {
                    // Unknown event types are silently ignored
                }
            }
        }

        Ok(parsed)
    }
}

impl WorkerRoomOps for MatrixWorkerRoom {
    fn get_or_create_room(
        &self,
        _room_name: &str,
    ) -> impl std::future::Future<Output = Result<String>> + Send {
        // Room is already set up at construction time
        let room_id = self.room_id.to_string();
        async move { Ok(room_id) }
    }

    fn post_to_thread(
        &self,
        room_id: &str,
        thread_root: &str,
        event_type: &str,
        content: serde_json::Value,
    ) -> impl std::future::Future<Output = Result<String>> + Send {
        let rid_str = room_id.to_string();
        let thread_root = thread_root.to_string();
        let event_type = event_type.to_string();
        let client = self.multi.preferred();
        async move {
            let rid = <&mxdx_matrix::RoomId>::try_from(rid_str.as_str())
                .map_err(|e| anyhow::anyhow!("Invalid room ID: {e}"))?;
            client
                .send_threaded_event(rid, &event_type, &thread_root, content)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))
        }
    }

    fn write_state(
        &self,
        room_id: &str,
        event_type: &str,
        state_key: &str,
        content: serde_json::Value,
    ) -> impl std::future::Future<Output = Result<()>> + Send {
        let rid_str = room_id.to_string();
        let event_type = event_type.to_string();
        let state_key = state_key.to_string();
        let client = self.multi.preferred();
        async move {
            let rid = <&mxdx_matrix::RoomId>::try_from(rid_str.as_str())
                .map_err(|e| anyhow::anyhow!("Invalid room ID: {e}"))?;
            client
                .send_state_event(rid, &event_type, &state_key, content)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))
        }
    }

    fn read_state(
        &self,
        room_id: &str,
        event_type: &str,
        state_key: &str,
    ) -> impl std::future::Future<Output = Result<Option<serde_json::Value>>> + Send {
        let rid_str = room_id.to_string();
        let event_type = event_type.to_string();
        let state_key = state_key.to_string();
        let client = self.multi.preferred();
        async move {
            let rid = <&mxdx_matrix::RoomId>::try_from(rid_str.as_str())
                .map_err(|e| anyhow::anyhow!("Invalid room ID: {e}"))?;
            match client
                .get_room_state_event(rid, &event_type, &state_key)
                .await
            {
                Ok(value) => Ok(Some(value)),
                Err(_) => Ok(None),
            }
        }
    }

    fn remove_state(
        &self,
        room_id: &str,
        event_type: &str,
        state_key: &str,
    ) -> impl std::future::Future<Output = Result<()>> + Send {
        let rid_str = room_id.to_string();
        let event_type = event_type.to_string();
        let state_key = state_key.to_string();
        let client = self.multi.preferred();
        async move {
            let rid = <&mxdx_matrix::RoomId>::try_from(rid_str.as_str())
                .map_err(|e| anyhow::anyhow!("Invalid room ID: {e}"))?;
            client
                .send_state_event(rid, &event_type, &state_key, serde_json::json!({}))
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))
        }
    }
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

    fn _assert_implements_trait<T: WorkerRoomOps>() {}

    #[test]
    fn matrix_worker_room_implements_worker_room_ops() {
        _assert_implements_trait::<MatrixWorkerRoom>();
    }
}
