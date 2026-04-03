use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Event type constants for worker state room
// ---------------------------------------------------------------------------

pub const WORKER_STATE_CONFIG: &str = "org.mxdx.worker.config";
pub const WORKER_STATE_IDENTITY: &str = "org.mxdx.worker.identity";
pub const WORKER_STATE_ROOM: &str = "org.mxdx.worker.room";
pub const WORKER_STATE_SESSION: &str = "org.mxdx.worker.session";
pub const WORKER_STATE_TOPOLOGY: &str = "org.mxdx.worker.topology";
pub const WORKER_STATE_ROOM_POINTER: &str = "org.mxdx.worker.state_room";
pub const WORKER_STATE_TRUSTED_CLIENT: &str = "org.mxdx.worker.trusted_client";
pub const WORKER_STATE_TRUSTED_COORDINATOR: &str = "org.mxdx.worker.trusted_coordinator";

// ---------------------------------------------------------------------------
// Data structs
// ---------------------------------------------------------------------------

/// Configuration for the worker state room itself.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkerStateConfig {
    pub room_name: String,
    pub trust_anchor: Option<String>,
    pub capabilities: Vec<String>,
    pub created_at: u64,
}

/// Identity of the worker (device, host, OS user).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkerStateIdentity {
    pub device_id: String,
    pub host: String,
    pub os_user: String,
}

/// A session tracked in the state room.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StateRoomSession {
    pub uuid: String,
    pub bin: String,
    pub args: Vec<String>,
    pub tmux_session: Option<String>,
    pub started_at: u64,
    pub thread_root: String,
    pub exec_room_id: String,
    pub state: String,
}

/// A room entry tracked in the state room (exec, logs, DMs, etc.).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StateRoomEntry {
    pub room_id: String,
    pub room_name: Option<String>,
    pub space_id: Option<String>,
    pub role: String,
    pub joined_at: u64,
}

/// A trusted entity (client or coordinator).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrustedEntity {
    pub user_id: String,
    pub verified_at: u64,
    pub verified_by_device: String,
}

/// Topology pointers for the worker's space and child rooms.
/// Two-room topology: exec (encrypted, all client interaction) + logs (worker operational logs).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StateRoomTopology {
    pub space_id: String,
    pub exec_room_id: String,
    pub logs_room_id: String,
}

/// Content written to exec room to advertise the worker's state room.
/// State key: `{device_id}`
/// Event type: `org.mxdx.worker.state_room`
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StateRoomPointer {
    pub room_id: String,
    pub device_id: String,
    pub hostname: String,
    pub os_user: String,
}

/// Content written by coordinator to assign a room to a worker.
/// Stored in the worker's state room with event type `org.mxdx.worker.room`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CoordinatorRoomAssignment {
    pub room_id: String,
    pub room_name: Option<String>,
    pub assigned_by: String,
    pub assigned_at: u64,
    pub role: String,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_type_constants() {
        assert_eq!(WORKER_STATE_CONFIG, "org.mxdx.worker.config");
        assert_eq!(WORKER_STATE_IDENTITY, "org.mxdx.worker.identity");
        assert_eq!(WORKER_STATE_ROOM, "org.mxdx.worker.room");
        assert_eq!(WORKER_STATE_SESSION, "org.mxdx.worker.session");
        assert_eq!(WORKER_STATE_TOPOLOGY, "org.mxdx.worker.topology");
        assert_eq!(WORKER_STATE_ROOM_POINTER, "org.mxdx.worker.state_room");
        assert_eq!(WORKER_STATE_TRUSTED_CLIENT, "org.mxdx.worker.trusted_client");
        assert_eq!(
            WORKER_STATE_TRUSTED_COORDINATOR,
            "org.mxdx.worker.trusted_coordinator"
        );
    }

    #[test]
    fn worker_state_config_roundtrip() {
        let config = WorkerStateConfig {
            room_name: "mxdx-state-node01.deploy.worker".into(),
            trust_anchor: Some("@coordinator:example.com".into()),
            capabilities: vec!["linux".into(), "gpu".into()],
            created_at: 1742572800,
        };
        let json = serde_json::to_string(&config).unwrap();
        let back: WorkerStateConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config, back);
    }

    #[test]
    fn worker_state_config_no_trust_anchor() {
        let config = WorkerStateConfig {
            room_name: "test-room".into(),
            trust_anchor: None,
            capabilities: vec![],
            created_at: 0,
        };
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("\"trust_anchor\":null"));
        let back: WorkerStateConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config, back);
    }

    #[test]
    fn worker_state_identity_roundtrip() {
        let identity = WorkerStateIdentity {
            device_id: "ABCDEF123".into(),
            host: "node-01".into(),
            os_user: "deploy".into(),
        };
        let json = serde_json::to_string(&identity).unwrap();
        let back: WorkerStateIdentity = serde_json::from_str(&json).unwrap();
        assert_eq!(identity, back);
    }

    #[test]
    fn state_room_session_roundtrip() {
        let session = StateRoomSession {
            uuid: "550e8400-e29b-41d4-a716-446655440000".into(),
            bin: "/usr/bin/bash".into(),
            args: vec!["-l".into()],
            tmux_session: Some("mxdx-550e8400".into()),
            started_at: 1742572800,
            thread_root: "$event123:example.com".into(),
            exec_room_id: "!room123:example.com".into(),
            state: "running".into(),
        };
        let json = serde_json::to_string(&session).unwrap();
        let back: StateRoomSession = serde_json::from_str(&json).unwrap();
        assert_eq!(session, back);
    }

    #[test]
    fn state_room_session_no_tmux() {
        let session = StateRoomSession {
            uuid: "test-uuid".into(),
            bin: "cat".into(),
            args: vec![],
            tmux_session: None,
            started_at: 0,
            thread_root: "$root".into(),
            exec_room_id: "!room".into(),
            state: "pending".into(),
        };
        let json = serde_json::to_string(&session).unwrap();
        assert!(json.contains("\"tmux_session\":null"));
        let back: StateRoomSession = serde_json::from_str(&json).unwrap();
        assert_eq!(session, back);
    }

    #[test]
    fn state_room_entry_roundtrip() {
        let entry = StateRoomEntry {
            room_id: "!abc123:example.com".into(),
            room_name: Some("exec room".into()),
            space_id: Some("!space:example.com".into()),
            role: "exec".into(),
            joined_at: 1742572800,
        };
        let json = serde_json::to_string(&entry).unwrap();
        let back: StateRoomEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry, back);
    }

    #[test]
    fn state_room_entry_optional_fields() {
        let entry = StateRoomEntry {
            room_id: "!room:example.com".into(),
            room_name: None,
            space_id: None,
            role: "logs".into(),
            joined_at: 0,
        };
        let json = serde_json::to_string(&entry).unwrap();
        let back: StateRoomEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry, back);
    }

    #[test]
    fn trusted_entity_roundtrip() {
        let entity = TrustedEntity {
            user_id: "@admin:example.com".into(),
            verified_at: 1742572800,
            verified_by_device: "DEVICEABC".into(),
        };
        let json = serde_json::to_string(&entity).unwrap();
        let back: TrustedEntity = serde_json::from_str(&json).unwrap();
        assert_eq!(entity, back);
    }

    #[test]
    fn state_room_topology_roundtrip() {
        let topology = StateRoomTopology {
            space_id: "!space:example.com".into(),
            exec_room_id: "!exec:example.com".into(),
            logs_room_id: "!logs:example.com".into(),
        };
        let json = serde_json::to_string(&topology).unwrap();
        let back: StateRoomTopology = serde_json::from_str(&json).unwrap();
        assert_eq!(topology, back);
    }

    #[test]
    fn state_room_pointer_roundtrip() {
        let pointer = StateRoomPointer {
            room_id: "!state:example.com".into(),
            device_id: "ABCDEF123".into(),
            hostname: "node-01.prod".into(),
            os_user: "deploy".into(),
        };
        let json = serde_json::to_string(&pointer).unwrap();
        let back: StateRoomPointer = serde_json::from_str(&json).unwrap();
        assert_eq!(pointer, back);
    }

    #[test]
    fn coordinator_room_assignment_roundtrip() {
        let assignment = CoordinatorRoomAssignment {
            room_id: "!exec:example.com".into(),
            room_name: Some("prod-exec".into()),
            assigned_by: "@coordinator:example.com".into(),
            assigned_at: 1742572800,
            role: "exec".into(),
        };
        let json = serde_json::to_string(&assignment).unwrap();
        let back: CoordinatorRoomAssignment = serde_json::from_str(&json).unwrap();
        assert_eq!(assignment, back);
    }

    #[test]
    fn coordinator_room_assignment_no_name() {
        let assignment = CoordinatorRoomAssignment {
            room_id: "!room:example.com".into(),
            room_name: None,
            assigned_by: "@admin:example.com".into(),
            assigned_at: 0,
            role: "logs".into(),
        };
        let json = serde_json::to_string(&assignment).unwrap();
        assert!(json.contains("\"room_name\":null"));
        let back: CoordinatorRoomAssignment = serde_json::from_str(&json).unwrap();
        assert_eq!(assignment, back);
    }

    #[test]
    fn snake_case_serialization() {
        let config = WorkerStateConfig {
            room_name: "test".into(),
            trust_anchor: None,
            capabilities: vec![],
            created_at: 0,
        };
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("room_name"));
        assert!(json.contains("trust_anchor"));
        assert!(json.contains("created_at"));
        assert!(!json.contains("roomName"));
        assert!(!json.contains("trustAnchor"));
        assert!(!json.contains("createdAt"));

        let session = StateRoomSession {
            uuid: "x".into(),
            bin: "x".into(),
            args: vec![],
            tmux_session: None,
            started_at: 0,
            thread_root: "x".into(),
            exec_room_id: "x".into(),
            state: "x".into(),
        };
        let json = serde_json::to_string(&session).unwrap();
        assert!(json.contains("tmux_session"));
        assert!(json.contains("started_at"));
        assert!(json.contains("thread_root"));
        assert!(json.contains("exec_room_id"));
    }
}
