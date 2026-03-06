use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LauncherIdentityEvent {
    pub launcher_id: String,
    pub accounts: Vec<String>,
    pub primary: String,
    pub capabilities: Vec<String>,
    pub version: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;

    #[test]
    fn launcher_identity_event_round_trips_json() {
        let evt = LauncherIdentityEvent {
            launcher_id: "belthanior".into(),
            accounts: vec![
                "@launcher-belthanior:hs1.mxdx.dev".into(),
                "@launcher-belthanior:hs2.mxdx.dev".into(),
            ],
            primary: "@launcher-belthanior:hs1.mxdx.dev".into(),
            capabilities: vec!["exec".into(), "terminal".into(), "telemetry".into()],
            version: "0.1.0".into(),
        };
        let json = serde_json::to_string(&evt).unwrap();
        let parsed: LauncherIdentityEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.launcher_id, "belthanior");
        assert_eq!(parsed.accounts.len(), 2);
        assert_eq!(parsed.primary, "@launcher-belthanior:hs1.mxdx.dev");
        assert_eq!(parsed.capabilities, vec!["exec", "terminal", "telemetry"]);
        assert_eq!(parsed.version, "0.1.0");
    }

    #[test]
    fn launcher_identity_event_rejects_missing_fields() {
        let json = r#"{"launcher_id":"x"}"#;
        let result: Result<LauncherIdentityEvent, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn launcher_identity_event_rejects_invalid_json() {
        let json = r#"{"launcher_id":123,"accounts":"not-an-array"}"#;
        let result: Result<LauncherIdentityEvent, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }
}
