/// Session attachment info — what the client needs to attach to a session
#[derive(Debug, Clone)]
pub struct AttachTarget {
    pub session_uuid: String,
    pub room_id: String,
    pub thread_root: String,
    pub interactive: bool,
    pub worker_id: String,
}

/// Determine attach mode based on session state and flags
#[derive(Debug, Clone, PartialEq)]
pub enum AttachMode {
    /// Tail the thread (non-interactive or WebRTC unavailable)
    TailThread,
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

pub fn determine_attach_mode(interactive: bool, webrtc_available: bool) -> AttachMode {
    if interactive && webrtc_available {
        AttachMode::Interactive
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
        let mode =
            determine_attach_mode(force_interactive && target.interactive, webrtc_available);
        let webrtc_state = match mode {
            AttachMode::Interactive => WebRtcState::Negotiating,
            AttachMode::TailThread => WebRtcState::Disabled,
        };
        Self {
            target,
            mode,
            webrtc_state,
        }
    }

    /// Handle WebRTC disconnection — fall back to thread tailing
    pub fn handle_disconnect(&mut self) {
        self.webrtc_state = WebRtcState::Disconnected;
        self.mode = AttachMode::TailThread;
    }

    /// Handle WebRTC reconnection
    pub fn handle_reconnect(&mut self) {
        self.webrtc_state = WebRtcState::Connected;
        self.mode = AttachMode::Interactive;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_target(interactive: bool) -> AttachTarget {
        AttachTarget {
            session_uuid: "uuid-1".into(),
            room_id: "!room:example.com".into(),
            thread_root: "$event1".into(),
            interactive,
            worker_id: "@worker:example.com".into(),
        }
    }

    #[test]
    fn interactive_with_webrtc_returns_interactive() {
        assert_eq!(
            determine_attach_mode(true, true),
            AttachMode::Interactive
        );
    }

    #[test]
    fn interactive_without_webrtc_returns_tail() {
        assert_eq!(
            determine_attach_mode(true, false),
            AttachMode::TailThread
        );
    }

    #[test]
    fn non_interactive_with_webrtc_returns_tail() {
        assert_eq!(
            determine_attach_mode(false, true),
            AttachMode::TailThread
        );
    }

    #[test]
    fn non_interactive_without_webrtc_returns_tail() {
        assert_eq!(
            determine_attach_mode(false, false),
            AttachMode::TailThread
        );
    }

    #[test]
    fn attach_target_fields() {
        let target = make_target(true);
        assert_eq!(target.session_uuid, "uuid-1");
        assert_eq!(target.room_id, "!room:example.com");
        assert_eq!(target.thread_root, "$event1");
        assert!(target.interactive);
        assert_eq!(target.worker_id, "@worker:example.com");
    }

    #[test]
    fn context_interactive_webrtc_available_negotiating() {
        let ctx = AttachContext::new(make_target(true), true, true);
        assert_eq!(ctx.mode, AttachMode::Interactive);
        assert_eq!(ctx.webrtc_state, WebRtcState::Negotiating);
    }

    #[test]
    fn context_interactive_webrtc_unavailable_disabled() {
        let ctx = AttachContext::new(make_target(true), true, false);
        assert_eq!(ctx.mode, AttachMode::TailThread);
        assert_eq!(ctx.webrtc_state, WebRtcState::Disabled);
    }

    #[test]
    fn context_non_interactive_webrtc_available_disabled() {
        let ctx = AttachContext::new(make_target(false), true, true);
        assert_eq!(ctx.mode, AttachMode::TailThread);
        assert_eq!(ctx.webrtc_state, WebRtcState::Disabled);
    }

    #[test]
    fn handle_disconnect_transitions_to_tail() {
        let mut ctx = AttachContext::new(make_target(true), true, true);
        assert_eq!(ctx.webrtc_state, WebRtcState::Negotiating);
        ctx.handle_disconnect();
        assert_eq!(ctx.webrtc_state, WebRtcState::Disconnected);
        assert_eq!(ctx.mode, AttachMode::TailThread);
    }

    #[test]
    fn handle_reconnect_transitions_to_interactive() {
        let mut ctx = AttachContext::new(make_target(true), true, true);
        ctx.handle_disconnect();
        ctx.handle_reconnect();
        assert_eq!(ctx.webrtc_state, WebRtcState::Connected);
        assert_eq!(ctx.mode, AttachMode::Interactive);
    }
}
