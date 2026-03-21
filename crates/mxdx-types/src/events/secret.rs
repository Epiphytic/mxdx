use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SecretRequestEvent {
    pub request_id: String,
    pub scope: String,
    pub ttl_seconds: u64,
    pub reason: String,
    /// One-time age x25519 public key for double encryption (mxdx-adr2).
    /// The coordinator encrypts the secret to this key so that even if the
    /// Megolm session is compromised, the plaintext remains protected.
    pub ephemeral_public_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SecretResponseEvent {
    pub request_id: String,
    pub granted: bool,
    /// When granted, contains the secret value encrypted with the requester's
    /// ephemeral age public key (base64-encoded ciphertext).
    pub encrypted_value: Option<String>,
    pub error: Option<String>,
}

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
            ephemeral_public_key: "age1testpublickey".into(),
        };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: SecretRequestEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.request_id, "req-001");
        assert_eq!(parsed.scope, "github.token");
        assert_eq!(parsed.ttl_seconds, 3600);
        assert_eq!(parsed.reason, "CI deployment");
        assert_eq!(parsed.ephemeral_public_key, "age1testpublickey");
    }

    #[test]
    fn secret_response_granted() {
        let resp = SecretResponseEvent {
            request_id: "req-001".into(),
            granted: true,
            encrypted_value: Some("age-encrypted-ciphertext-base64".into()),
            error: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: SecretResponseEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.request_id, "req-001");
        assert!(parsed.granted);
        assert_eq!(
            parsed.encrypted_value,
            Some("age-encrypted-ciphertext-base64".into())
        );
        assert!(parsed.error.is_none());
    }

    #[test]
    fn secret_response_denied() {
        let resp = SecretResponseEvent {
            request_id: "req-002".into(),
            granted: false,
            encrypted_value: None,
            error: Some("Unauthorized scope".into()),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: SecretResponseEvent = serde_json::from_str(&json).unwrap();
        assert!(!parsed.granted);
        assert!(parsed.encrypted_value.is_none());
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
