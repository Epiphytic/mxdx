#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;
    use std::collections::HashMap;

    // TerminalDataEvent tests

    #[test]
    fn terminal_data_event_round_trips_json() {
        let evt = TerminalDataEvent {
            data: "dGVzdA==".into(),
            encoding: "raw+base64".into(),
            seq: 42,
        };
        let json = serde_json::to_string(&evt).unwrap();
        let parsed: TerminalDataEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.data, "dGVzdA==");
        assert_eq!(parsed.encoding, "raw+base64");
        assert_eq!(parsed.seq, 42);
    }

    #[test]
    fn seq_field_is_u64_and_handles_large_values() {
        let event = TerminalDataEvent {
            data: "dGVzdA==".into(),
            encoding: "raw+base64".into(),
            seq: u64::MAX,
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: TerminalDataEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.seq, u64::MAX);
    }

    #[test]
    fn terminal_data_event_rejects_missing_fields() {
        let json = r#"{"data":"dGVzdA=="}"#;
        let result: Result<TerminalDataEvent, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    // TerminalResizeEvent tests

    #[test]
    fn terminal_resize_event_round_trips_json() {
        let evt = TerminalResizeEvent {
            cols: 120,
            rows: 40,
        };
        let json = serde_json::to_string(&evt).unwrap();
        let parsed: TerminalResizeEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.cols, 120);
        assert_eq!(parsed.rows, 40);
    }

    #[test]
    fn terminal_resize_event_rejects_missing_fields() {
        let json = r#"{"cols":80}"#;
        let result: Result<TerminalResizeEvent, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    // TerminalSessionRequestEvent tests

    #[test]
    fn terminal_session_request_round_trips_json() {
        let evt = TerminalSessionRequestEvent {
            uuid: "sess-001".into(),
            command: "/bin/bash".into(),
            env: [("TERM".into(), "xterm-256color".into())].into(),
            cols: 80,
            rows: 24,
        };
        let json = serde_json::to_string(&evt).unwrap();
        let parsed: TerminalSessionRequestEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.uuid, "sess-001");
        assert_eq!(parsed.command, "/bin/bash");
        assert_eq!(parsed.env.get("TERM").unwrap(), "xterm-256color");
        assert_eq!(parsed.cols, 80);
        assert_eq!(parsed.rows, 24);
    }

    #[test]
    fn terminal_session_request_rejects_missing_fields() {
        let json = r#"{"uuid":"x"}"#;
        let result: Result<TerminalSessionRequestEvent, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    // TerminalSessionResponseEvent tests

    #[test]
    fn terminal_session_response_round_trips_json() {
        let evt = TerminalSessionResponseEvent {
            uuid: "sess-001".into(),
            status: "created".into(),
            room_id: Some("!abc:example.com".into()),
        };
        let json = serde_json::to_string(&evt).unwrap();
        let parsed: TerminalSessionResponseEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.uuid, "sess-001");
        assert_eq!(parsed.status, "created");
        assert_eq!(parsed.room_id, Some("!abc:example.com".into()));
    }

    #[test]
    fn terminal_session_response_without_room_id() {
        let evt = TerminalSessionResponseEvent {
            uuid: "sess-002".into(),
            status: "error".into(),
            room_id: None,
        };
        let json = serde_json::to_string(&evt).unwrap();
        let parsed: TerminalSessionResponseEvent = serde_json::from_str(&json).unwrap();
        assert!(parsed.room_id.is_none());
    }

    #[test]
    fn terminal_session_response_rejects_missing_fields() {
        let json = r#"{"uuid":"x"}"#;
        let result: Result<TerminalSessionResponseEvent, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    // TerminalRetransmitEvent tests

    #[test]
    fn terminal_retransmit_event_round_trips_json() {
        let evt = TerminalRetransmitEvent {
            from_seq: 100,
            to_seq: 200,
        };
        let json = serde_json::to_string(&evt).unwrap();
        let parsed: TerminalRetransmitEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.from_seq, 100);
        assert_eq!(parsed.to_seq, 200);
    }

    #[test]
    fn terminal_retransmit_event_handles_large_seq() {
        let evt = TerminalRetransmitEvent {
            from_seq: u64::MAX - 1,
            to_seq: u64::MAX,
        };
        let json = serde_json::to_string(&evt).unwrap();
        let parsed: TerminalRetransmitEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.from_seq, u64::MAX - 1);
        assert_eq!(parsed.to_seq, u64::MAX);
    }

    #[test]
    fn terminal_retransmit_event_rejects_missing_fields() {
        let json = r#"{"from_seq":0}"#;
        let result: Result<TerminalRetransmitEvent, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }
}
