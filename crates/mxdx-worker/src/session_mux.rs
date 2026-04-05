use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Routes incoming DM events to the correct session by session_id field.
/// Manages multiple sessions sharing a single DM room.
pub struct SessionMux {
    /// Maps session_id -> SessionMuxEntry
    sessions: HashMap<String, SessionMuxEntry>,
    /// Maps room_id -> list of session_ids in that room
    room_sessions: HashMap<String, Vec<String>>,
}

/// A registered session within the mux.
pub struct SessionMuxEntry {
    pub session_id: String,
    pub dm_room_id: String,
    pub client_user_id: String,
}

/// Actions returned by route_event.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "action")]
pub enum MuxAction {
    /// Forward input data to session's PTY
    ForwardInput { session_id: String, data: String },
    /// Resize the session's PTY
    Resize {
        session_id: String,
        cols: u16,
        rows: u16,
    },
    /// Forward a signal to the session's process
    Signal { session_id: String, signal: String },
    /// Event doesn't match any registered session
    NoMatch,
}

impl SessionMux {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            room_sessions: HashMap::new(),
        }
    }

    /// Register a session for routing.
    pub fn add_session(&mut self, session_id: &str, dm_room_id: &str, client_user_id: &str) {
        let entry = SessionMuxEntry {
            session_id: session_id.to_string(),
            dm_room_id: dm_room_id.to_string(),
            client_user_id: client_user_id.to_string(),
        };
        self.sessions.insert(session_id.to_string(), entry);
        self.room_sessions
            .entry(dm_room_id.to_string())
            .or_default()
            .push(session_id.to_string());
    }

    /// Unregister a session.
    pub fn remove_session(&mut self, session_id: &str) {
        if let Some(entry) = self.sessions.remove(session_id) {
            if let Some(ids) = self.room_sessions.get_mut(&entry.dm_room_id) {
                ids.retain(|id| id != session_id);
                if ids.is_empty() {
                    self.room_sessions.remove(&entry.dm_room_id);
                }
            }
        }
    }

    /// Route an incoming event to the appropriate action.
    /// Parses the event's session_uuid field and event type to determine
    /// what action to take.
    pub fn route_event(&self, event_type: &str, content_json: &str) -> MuxAction {
        let content: serde_json::Value = match serde_json::from_str(content_json) {
            Ok(v) => v,
            Err(_) => return MuxAction::NoMatch,
        };

        let session_id = match content.get("session_uuid").and_then(|v| v.as_str()) {
            Some(id) => id.to_string(),
            None => return MuxAction::NoMatch,
        };

        if !self.sessions.contains_key(&session_id) {
            return MuxAction::NoMatch;
        }

        match event_type {
            "org.mxdx.session.input" => {
                let data = content
                    .get("data")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                MuxAction::ForwardInput { session_id, data }
            }
            "org.mxdx.session.resize" => {
                let cols = content
                    .get("cols")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(80) as u16;
                let rows = content
                    .get("rows")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(24) as u16;
                MuxAction::Resize {
                    session_id,
                    cols,
                    rows,
                }
            }
            "org.mxdx.session.signal" => {
                let signal = content
                    .get("signal")
                    .and_then(|v| v.as_str())
                    .unwrap_or("SIGTERM")
                    .to_string();
                MuxAction::Signal { session_id, signal }
            }
            _ => MuxAction::NoMatch,
        }
    }

    /// Get sessions for a DM room.
    pub fn sessions_in_room(&self, room_id: &str) -> Vec<&str> {
        self.room_sessions
            .get(room_id)
            .map(|ids| ids.iter().map(|s| s.as_str()).collect())
            .unwrap_or_default()
    }

    /// Check if a session exists.
    pub fn has_session(&self, session_id: &str) -> bool {
        self.sessions.contains_key(session_id)
    }

    /// Get the number of registered sessions.
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }
}

impl Default for SessionMux {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_mux_is_empty() {
        let mux = SessionMux::new();
        assert_eq!(mux.session_count(), 0);
        assert!(!mux.has_session("anything"));
    }

    #[test]
    fn add_and_has_session() {
        let mut mux = SessionMux::new();
        mux.add_session("sess-1", "!dm1:example.com", "@client:example.com");
        assert!(mux.has_session("sess-1"));
        assert!(!mux.has_session("sess-2"));
        assert_eq!(mux.session_count(), 1);
    }

    #[test]
    fn remove_session() {
        let mut mux = SessionMux::new();
        mux.add_session("sess-1", "!dm1:example.com", "@client:example.com");
        assert!(mux.has_session("sess-1"));

        mux.remove_session("sess-1");
        assert!(!mux.has_session("sess-1"));
        assert_eq!(mux.session_count(), 0);
        assert!(mux.sessions_in_room("!dm1:example.com").is_empty());
    }

    #[test]
    fn remove_nonexistent_session_is_noop() {
        let mut mux = SessionMux::new();
        mux.remove_session("nonexistent"); // should not panic
        assert_eq!(mux.session_count(), 0);
    }

    #[test]
    fn sessions_in_room() {
        let mut mux = SessionMux::new();
        mux.add_session("sess-1", "!dm1:example.com", "@client:example.com");
        mux.add_session("sess-2", "!dm1:example.com", "@client:example.com");
        mux.add_session("sess-3", "!dm2:example.com", "@other:example.com");

        let room1 = mux.sessions_in_room("!dm1:example.com");
        assert_eq!(room1.len(), 2);
        assert!(room1.contains(&"sess-1"));
        assert!(room1.contains(&"sess-2"));

        let room2 = mux.sessions_in_room("!dm2:example.com");
        assert_eq!(room2.len(), 1);
        assert!(room2.contains(&"sess-3"));

        assert!(mux.sessions_in_room("!unknown:example.com").is_empty());
    }

    #[test]
    fn route_input_event() {
        let mut mux = SessionMux::new();
        mux.add_session("sess-1", "!dm1:example.com", "@client:example.com");

        let content = r#"{"session_uuid": "sess-1", "data": "ls -la\n"}"#;
        let action = mux.route_event("org.mxdx.session.input", content);
        assert_eq!(
            action,
            MuxAction::ForwardInput {
                session_id: "sess-1".into(),
                data: "ls -la\n".into(),
            }
        );
    }

    #[test]
    fn route_resize_event() {
        let mut mux = SessionMux::new();
        mux.add_session("sess-1", "!dm1:example.com", "@client:example.com");

        let content = r#"{"session_uuid": "sess-1", "cols": 120, "rows": 40}"#;
        let action = mux.route_event("org.mxdx.session.resize", content);
        assert_eq!(
            action,
            MuxAction::Resize {
                session_id: "sess-1".into(),
                cols: 120,
                rows: 40,
            }
        );
    }

    #[test]
    fn route_resize_defaults() {
        let mut mux = SessionMux::new();
        mux.add_session("sess-1", "!dm1:example.com", "@client:example.com");

        let content = r#"{"session_uuid": "sess-1"}"#;
        let action = mux.route_event("org.mxdx.session.resize", content);
        assert_eq!(
            action,
            MuxAction::Resize {
                session_id: "sess-1".into(),
                cols: 80,
                rows: 24,
            }
        );
    }

    #[test]
    fn route_signal_event() {
        let mut mux = SessionMux::new();
        mux.add_session("sess-1", "!dm1:example.com", "@client:example.com");

        let content = r#"{"session_uuid": "sess-1", "signal": "SIGINT"}"#;
        let action = mux.route_event("org.mxdx.session.signal", content);
        assert_eq!(
            action,
            MuxAction::Signal {
                session_id: "sess-1".into(),
                signal: "SIGINT".into(),
            }
        );
    }

    #[test]
    fn route_signal_default() {
        let mut mux = SessionMux::new();
        mux.add_session("sess-1", "!dm1:example.com", "@client:example.com");

        let content = r#"{"session_uuid": "sess-1"}"#;
        let action = mux.route_event("org.mxdx.session.signal", content);
        assert_eq!(
            action,
            MuxAction::Signal {
                session_id: "sess-1".into(),
                signal: "SIGTERM".into(),
            }
        );
    }

    #[test]
    fn route_unknown_session_returns_no_match() {
        let mux = SessionMux::new();
        let content = r#"{"session_uuid": "unknown-sess", "data": "hello"}"#;
        let action = mux.route_event("org.mxdx.session.input", content);
        assert_eq!(action, MuxAction::NoMatch);
    }

    #[test]
    fn route_unknown_event_type_returns_no_match() {
        let mut mux = SessionMux::new();
        mux.add_session("sess-1", "!dm1:example.com", "@client:example.com");

        let content = r#"{"session_uuid": "sess-1"}"#;
        let action = mux.route_event("org.mxdx.session.unknown", content);
        assert_eq!(action, MuxAction::NoMatch);
    }

    #[test]
    fn route_invalid_json_returns_no_match() {
        let mut mux = SessionMux::new();
        mux.add_session("sess-1", "!dm1:example.com", "@client:example.com");

        let action = mux.route_event("org.mxdx.session.input", "not valid json");
        assert_eq!(action, MuxAction::NoMatch);
    }

    #[test]
    fn route_missing_session_uuid_returns_no_match() {
        let mut mux = SessionMux::new();
        mux.add_session("sess-1", "!dm1:example.com", "@client:example.com");

        let content = r#"{"data": "hello"}"#;
        let action = mux.route_event("org.mxdx.session.input", content);
        assert_eq!(action, MuxAction::NoMatch);
    }

    #[test]
    fn multiple_sessions_route_independently() {
        let mut mux = SessionMux::new();
        mux.add_session("sess-1", "!dm1:example.com", "@client1:example.com");
        mux.add_session("sess-2", "!dm1:example.com", "@client2:example.com");

        let content1 = r#"{"session_uuid": "sess-1", "data": "input1"}"#;
        let content2 = r#"{"session_uuid": "sess-2", "data": "input2"}"#;

        assert_eq!(
            mux.route_event("org.mxdx.session.input", content1),
            MuxAction::ForwardInput {
                session_id: "sess-1".into(),
                data: "input1".into(),
            }
        );
        assert_eq!(
            mux.route_event("org.mxdx.session.input", content2),
            MuxAction::ForwardInput {
                session_id: "sess-2".into(),
                data: "input2".into(),
            }
        );
    }

    #[test]
    fn remove_one_session_from_shared_room() {
        let mut mux = SessionMux::new();
        mux.add_session("sess-1", "!dm1:example.com", "@client:example.com");
        mux.add_session("sess-2", "!dm1:example.com", "@client:example.com");

        mux.remove_session("sess-1");
        assert!(!mux.has_session("sess-1"));
        assert!(mux.has_session("sess-2"));

        let room = mux.sessions_in_room("!dm1:example.com");
        assert_eq!(room.len(), 1);
        assert!(room.contains(&"sess-2"));
    }

    #[test]
    fn default_impl() {
        let mux = SessionMux::default();
        assert_eq!(mux.session_count(), 0);
    }
}
