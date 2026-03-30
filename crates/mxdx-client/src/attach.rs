/// Session attachment info — what the client needs to attach to a session
#[derive(Debug, Clone)]
pub struct AttachTarget {
    pub session_uuid: String,
    pub room_id: String,
    pub thread_root: String,
    pub interactive: bool,
    pub worker_id: String,
    /// DM room ID for interactive terminal I/O (from SessionStart event).
    pub dm_room_id: Option<String>,
}

/// Determine attach mode based on session state and flags
#[derive(Debug, Clone, PartialEq)]
pub enum AttachMode {
    /// Tail the thread (non-interactive or WebRTC unavailable)
    TailThread,
    /// Interactive mode via DM room (terminal I/O over Matrix E2EE)
    InteractiveDm,
    /// Interactive mode via WebRTC DataChannel (future)
    Interactive,
}

/// WebRTC connection state for attached sessions
#[derive(Debug, Clone, PartialEq)]
pub enum WebRtcState {
    /// Not using WebRTC
    Disabled,
    /// Negotiating (offer sent, waiting for answer)
    Negotiating,
    /// Connected via DataChannel
    Connected,
    /// Disconnected, falling back to thread
    Disconnected,
}

pub fn determine_attach_mode(interactive: bool, webrtc_available: bool, has_dm_room: bool) -> AttachMode {
    if interactive && webrtc_available {
        AttachMode::Interactive
    } else if interactive && has_dm_room {
        AttachMode::InteractiveDm
    } else {
        AttachMode::TailThread
    }
}

/// Full attach context with WebRTC state tracking
pub struct AttachContext {
    pub target: AttachTarget,
    pub mode: AttachMode,
    pub webrtc_state: WebRtcState,
}

impl AttachContext {
    pub fn new(target: AttachTarget, force_interactive: bool, webrtc_available: bool) -> Self {
        let has_dm = target.dm_room_id.is_some();
        let mode = determine_attach_mode(
            force_interactive && target.interactive,
            webrtc_available,
            has_dm,
        );
        let webrtc_state = match mode {
            AttachMode::Interactive => WebRtcState::Negotiating,
            AttachMode::InteractiveDm | AttachMode::TailThread => WebRtcState::Disabled,
        };
        Self {
            target,
            mode,
            webrtc_state,
        }
    }

    /// Handle WebRTC disconnection — fall back to DM or thread tailing
    pub fn handle_disconnect(&mut self) {
        self.webrtc_state = WebRtcState::Disconnected;
        if self.target.dm_room_id.is_some() {
            self.mode = AttachMode::InteractiveDm;
        } else {
            self.mode = AttachMode::TailThread;
        }
    }

    /// Handle WebRTC reconnection
    pub fn handle_reconnect(&mut self) {
        self.webrtc_state = WebRtcState::Connected;
        self.mode = AttachMode::Interactive;
    }
}

/// Find the DM room ID for an interactive session by scanning SessionStart events.
/// Returns the dm_room_id from the first matching SessionStart event, or None.
pub fn find_dm_room_from_start_event(
    events: &[serde_json::Value],
    session_uuid: &str,
) -> Option<String> {
    for event in events {
        let event_type = event.get("type").and_then(|t| t.as_str());
        if event_type != Some("org.mxdx.session.start") {
            continue;
        }
        let content = event.get("content").unwrap_or(event);
        let uuid = content
            .get("session_uuid")
            .and_then(|v| v.as_str());
        if uuid == Some(session_uuid) {
            return content
                .get("dm_room_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_target(interactive: bool, dm_room_id: Option<String>) -> AttachTarget {
        AttachTarget {
            session_uuid: "uuid-1".into(),
            room_id: "!room:example.com".into(),
            thread_root: "$event1".into(),
            interactive,
            worker_id: "@worker:example.com".into(),
            dm_room_id,
        }
    }

    #[test]
    fn interactive_with_webrtc_returns_interactive() {
        assert_eq!(
            determine_attach_mode(true, true, false),
            AttachMode::Interactive
        );
    }

    #[test]
    fn interactive_with_dm_returns_dm() {
        assert_eq!(
            determine_attach_mode(true, false, true),
            AttachMode::InteractiveDm
        );
    }

    #[test]
    fn interactive_without_webrtc_or_dm_returns_tail() {
        assert_eq!(
            determine_attach_mode(true, false, false),
            AttachMode::TailThread
        );
    }

    #[test]
    fn non_interactive_with_webrtc_returns_tail() {
        assert_eq!(
            determine_attach_mode(false, true, false),
            AttachMode::TailThread
        );
    }

    #[test]
    fn non_interactive_without_webrtc_returns_tail() {
        assert_eq!(
            determine_attach_mode(false, false, false),
            AttachMode::TailThread
        );
    }

    #[test]
    fn attach_target_fields() {
        let target = make_target(true, Some("!dm:example.com".into()));
        assert_eq!(target.session_uuid, "uuid-1");
        assert_eq!(target.room_id, "!room:example.com");
        assert_eq!(target.thread_root, "$event1");
        assert!(target.interactive);
        assert_eq!(target.worker_id, "@worker:example.com");
        assert_eq!(target.dm_room_id, Some("!dm:example.com".into()));
    }

    #[test]
    fn context_interactive_webrtc_available_negotiating() {
        let ctx = AttachContext::new(make_target(true, None), true, true);
        assert_eq!(ctx.mode, AttachMode::Interactive);
        assert_eq!(ctx.webrtc_state, WebRtcState::Negotiating);
    }

    #[test]
    fn context_interactive_dm_available() {
        let ctx = AttachContext::new(make_target(true, Some("!dm:ex.com".into())), true, false);
        assert_eq!(ctx.mode, AttachMode::InteractiveDm);
        assert_eq!(ctx.webrtc_state, WebRtcState::Disabled);
    }

    #[test]
    fn context_interactive_webrtc_unavailable_disabled() {
        let ctx = AttachContext::new(make_target(true, None), true, false);
        assert_eq!(ctx.mode, AttachMode::TailThread);
        assert_eq!(ctx.webrtc_state, WebRtcState::Disabled);
    }

    #[test]
    fn context_non_interactive_webrtc_available_disabled() {
        let ctx = AttachContext::new(make_target(false, None), true, true);
        assert_eq!(ctx.mode, AttachMode::TailThread);
        assert_eq!(ctx.webrtc_state, WebRtcState::Disabled);
    }

    #[test]
    fn handle_disconnect_transitions_to_dm_if_available() {
        let mut ctx = AttachContext::new(make_target(true, Some("!dm:ex.com".into())), true, true);
        assert_eq!(ctx.webrtc_state, WebRtcState::Negotiating);
        ctx.handle_disconnect();
        assert_eq!(ctx.webrtc_state, WebRtcState::Disconnected);
        assert_eq!(ctx.mode, AttachMode::InteractiveDm);
    }

    #[test]
    fn handle_disconnect_transitions_to_tail_if_no_dm() {
        let mut ctx = AttachContext::new(make_target(true, None), true, true);
        ctx.handle_disconnect();
        assert_eq!(ctx.webrtc_state, WebRtcState::Disconnected);
        assert_eq!(ctx.mode, AttachMode::TailThread);
    }

    #[test]
    fn handle_reconnect_transitions_to_interactive() {
        let mut ctx = AttachContext::new(make_target(true, None), true, true);
        ctx.handle_disconnect();
        ctx.handle_reconnect();
        assert_eq!(ctx.webrtc_state, WebRtcState::Connected);
        assert_eq!(ctx.mode, AttachMode::Interactive);
    }

    #[test]
    fn find_dm_room_from_events_found() {
        let events = vec![serde_json::json!({
            "type": "org.mxdx.session.start",
            "content": {
                "session_uuid": "sess-1",
                "worker_id": "@w:ex.com",
                "started_at": 0,
                "dm_room_id": "!dm123:ex.com"
            }
        })];
        assert_eq!(
            find_dm_room_from_start_event(&events, "sess-1"),
            Some("!dm123:ex.com".to_string())
        );
    }

    #[test]
    fn find_dm_room_from_events_not_found() {
        let events = vec![serde_json::json!({
            "type": "org.mxdx.session.start",
            "content": {
                "session_uuid": "sess-other",
                "worker_id": "@w:ex.com",
                "started_at": 0,
            }
        })];
        assert_eq!(find_dm_room_from_start_event(&events, "sess-1"), None);
    }

    #[test]
    fn find_dm_room_no_dm_field() {
        let events = vec![serde_json::json!({
            "type": "org.mxdx.session.start",
            "content": {
                "session_uuid": "sess-1",
                "worker_id": "@w:ex.com",
                "started_at": 0,
            }
        })];
        // No dm_room_id field — non-interactive session
        assert_eq!(find_dm_room_from_start_event(&events, "sess-1"), None);
    }
}
