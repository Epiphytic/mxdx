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

pub fn determine_attach_mode(interactive: bool, webrtc_available: bool) -> AttachMode {
    if interactive && webrtc_available {
        AttachMode::Interactive
    } else {
        AttachMode::TailThread
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let target = AttachTarget {
            session_uuid: "uuid-1".into(),
            room_id: "!room:example.com".into(),
            thread_root: "$event1".into(),
            interactive: true,
            worker_id: "@worker:example.com".into(),
        };
        assert_eq!(target.session_uuid, "uuid-1");
        assert_eq!(target.room_id, "!room:example.com");
        assert_eq!(target.thread_root, "$event1");
        assert!(target.interactive);
        assert_eq!(target.worker_id, "@worker:example.com");
    }
}
