use mxdx_types::events::session::{SessionCancel, SessionSignal};

/// Build a cancel event
pub fn build_cancel(session_uuid: &str, reason: Option<String>, grace_seconds: Option<u64>) -> SessionCancel {
    SessionCancel {
        session_uuid: session_uuid.to_string(),
        reason,
        grace_seconds,
    }
}

/// Build a signal event
pub fn build_signal(session_uuid: &str, signal: &str) -> SessionSignal {
    SessionSignal {
        session_uuid: session_uuid.to_string(),
        signal: signal.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_cancel_with_reason() {
        let cancel = build_cancel("uuid-1", Some("user requested".into()), Some(5));
        assert_eq!(cancel.session_uuid, "uuid-1");
        assert_eq!(cancel.reason, Some("user requested".into()));
        assert_eq!(cancel.grace_seconds, Some(5));
    }

    #[test]
    fn build_cancel_without_reason() {
        let cancel = build_cancel("uuid-2", None, None);
        assert_eq!(cancel.session_uuid, "uuid-2");
        assert_eq!(cancel.reason, None);
        assert_eq!(cancel.grace_seconds, None);
    }

    #[test]
    fn build_signal_creates_correct_event() {
        let signal = build_signal("uuid-3", "SIGTERM");
        assert_eq!(signal.session_uuid, "uuid-3");
        assert_eq!(signal.signal, "SIGTERM");
    }

    #[test]
    fn build_signal_sigkill() {
        let signal = build_signal("uuid-4", "SIGKILL");
        assert_eq!(signal.signal, "SIGKILL");
    }
}
