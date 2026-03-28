use anyhow::Result;
use serde::{de::DeserializeOwned, Serialize};

/// Abstraction over Matrix room operations for the client.
/// This trait allows testing with mocks without requiring a real Matrix server.
pub trait ClientRoomOps: Send + Sync {
    /// Find a room by name or alias
    fn find_room(
        &self,
        room_name: &str,
    ) -> impl std::future::Future<Output = Result<Option<String>>> + Send;

    /// Post an event to a room
    fn post_event(
        &self,
        room_id: &str,
        event_type: &str,
        content: serde_json::Value,
    ) -> impl std::future::Future<Output = Result<String>> + Send;

    /// Post a threaded event to a session's thread
    fn post_to_thread(
        &self,
        room_id: &str,
        thread_root: &str,
        event_type: &str,
        content: serde_json::Value,
    ) -> impl std::future::Future<Output = Result<String>> + Send;

    /// Read state events of a given type from a room
    fn read_state_events(
        &self,
        room_id: &str,
        event_type: &str,
    ) -> impl std::future::Future<Output = Result<Vec<(String, serde_json::Value)>>> + Send;

    /// Sync and return incoming client-relevant events
    fn sync_events(
        &self,
    ) -> impl std::future::Future<Output = Result<Vec<IncomingClientEvent>>> + Send;
}

/// Client-side incoming events from Matrix sync.
#[derive(Debug, Clone)]
pub enum IncomingClientEvent {
    /// Worker has started a session
    SessionStart {
        session_uuid: String,
        content: serde_json::Value,
    },
    /// Worker is sending session output
    SessionOutput {
        session_uuid: String,
        content: serde_json::Value,
    },
    /// Worker heartbeat for an active session
    SessionHeartbeat {
        session_uuid: String,
        content: serde_json::Value,
    },
    /// Worker is reporting final session result
    SessionResult {
        session_uuid: String,
        content: serde_json::Value,
    },
}

/// Concrete holder for a client's room reference.
///
/// Holds the room_id for the target worker room. The actual Matrix SDK
/// integration (via `mxdx-matrix::MatrixClient`) will be wired up later.
pub struct ClientRoom {
    room_id: String,
}

impl ClientRoom {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_event_produces_json() {
        let data = serde_json::json!({"bin": "/bin/echo", "args": ["hello"]});
        let value = serialize_event(&data).expect("serialization should succeed");
        assert_eq!(value["bin"], "/bin/echo");
        assert_eq!(value["args"], serde_json::json!(["hello"]));
    }

    #[test]
    fn deserialize_event_from_json() {
        let json = serde_json::json!({
            "uuid": "test-uuid-5678",
            "bin": "/usr/bin/ls",
        });

        let result: serde_json::Value = deserialize_event(&json).expect("deserialization should succeed");
        assert_eq!(result["uuid"], "test-uuid-5678");
        assert_eq!(result["bin"], "/usr/bin/ls");
    }

    #[test]
    fn incoming_client_event_variants_construct_and_match() {
        let events = vec![
            IncomingClientEvent::SessionStart {
                session_uuid: "uuid-1".to_string(),
                content: serde_json::json!({"status": "started"}),
            },
            IncomingClientEvent::SessionOutput {
                session_uuid: "uuid-2".to_string(),
                content: serde_json::json!({"data": "output line"}),
            },
            IncomingClientEvent::SessionHeartbeat {
                session_uuid: "uuid-3".to_string(),
                content: serde_json::json!({"ts": 1700000000}),
            },
            IncomingClientEvent::SessionResult {
                session_uuid: "uuid-4".to_string(),
                content: serde_json::json!({"exit_code": 0}),
            },
        ];

        let mut start_count = 0;
        let mut output_count = 0;
        let mut heartbeat_count = 0;
        let mut result_count = 0;

        for event in &events {
            match event {
                IncomingClientEvent::SessionStart { session_uuid, .. } => {
                    assert_eq!(session_uuid, "uuid-1");
                    start_count += 1;
                }
                IncomingClientEvent::SessionOutput { session_uuid, .. } => {
                    assert_eq!(session_uuid, "uuid-2");
                    output_count += 1;
                }
                IncomingClientEvent::SessionHeartbeat { session_uuid, .. } => {
                    assert_eq!(session_uuid, "uuid-3");
                    heartbeat_count += 1;
                }
                IncomingClientEvent::SessionResult { session_uuid, .. } => {
                    assert_eq!(session_uuid, "uuid-4");
                    result_count += 1;
                }
            }
        }

        assert_eq!(start_count, 1);
        assert_eq!(output_count, 1);
        assert_eq!(heartbeat_count, 1);
        assert_eq!(result_count, 1);
    }

    #[test]
    fn client_room_stores_and_returns_room_id() {
        let room = ClientRoom::new("!abc123:example.com".to_string());
        assert_eq!(room.room_id(), "!abc123:example.com");

        let room2 = ClientRoom::new("!xyz789:matrix.org".to_string());
        assert_eq!(room2.room_id(), "!xyz789:matrix.org");
    }
}
