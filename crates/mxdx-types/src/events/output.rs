#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;

    #[test]
    fn output_event_round_trips_json() {
        let out = OutputEvent {
            uuid: "test-1".into(),
            stream: OutputStream::Stdout,
            data: "aGVsbG8=".into(),
            encoding: "raw+base64".into(),
            seq: 0,
        };
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains(r#""stream":"stdout"#));
        let parsed: OutputEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.stream, OutputStream::Stdout);
        assert_eq!(parsed.uuid, out.uuid);
        assert_eq!(parsed.data, out.data);
        assert_eq!(parsed.seq, out.seq);
    }

    #[test]
    fn output_event_supports_stderr() {
        let out = OutputEvent {
            uuid: "test-1".into(),
            stream: OutputStream::Stderr,
            data: "ZXJyb3I=".into(),
            encoding: "raw+base64".into(),
            seq: 1,
        };
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains(r#""stream":"stderr"#));
    }

    #[test]
    fn output_event_rejects_invalid_stream() {
        let json = r#"{"uuid":"x","stream":"stdwhat","data":"","encoding":"raw","seq":0}"#;
        let result: Result<OutputEvent, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }
}
