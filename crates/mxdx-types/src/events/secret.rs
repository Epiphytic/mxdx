#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;

    #[test]
    fn secret_request_round_trips_json() {
        let req = SecretRequestEvent {
            request_id: "req-001".into(),
            scope: "github.token".into(),
            ttl_seconds: 3600,
            reason: "CI deployment".into(),
        };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: SecretRequestEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.request_id, "req-001");
        assert_eq!(parsed.scope, "github.token");
        assert_eq!(parsed.ttl_seconds, 3600);
        assert_eq!(parsed.reason, "CI deployment");
    }

    #[test]
    fn secret_response_granted() {
        let resp = SecretResponseEvent {
            request_id: "req-001".into(),
            granted: true,
            value: Some("ghp_secret_token_value".into()),
            error: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: SecretResponseEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.request_id, "req-001");
        assert!(parsed.granted);
        assert_eq!(parsed.value, Some("ghp_secret_token_value".into()));
        assert!(parsed.error.is_none());
    }

    #[test]
    fn secret_response_denied() {
        let resp = SecretResponseEvent {
            request_id: "req-002".into(),
            granted: false,
            value: None,
            error: Some("Unauthorized scope".into()),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: SecretResponseEvent = serde_json::from_str(&json).unwrap();
        assert!(!parsed.granted);
        assert!(parsed.value.is_none());
        assert_eq!(parsed.error, Some("Unauthorized scope".into()));
    }

    #[test]
    fn secret_request_rejects_missing_fields() {
        let json = r#"{"request_id":"req-001"}"#;
        let result: Result<SecretRequestEvent, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn secret_response_rejects_missing_fields() {
        let json = r#"{"request_id":"req-001"}"#;
        let result: Result<SecretResponseEvent, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }
}
