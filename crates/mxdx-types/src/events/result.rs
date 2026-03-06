use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResultEvent {
    pub uuid: String,
    pub status: ResultStatus,
    pub exit_code: Option<i32>,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ResultStatus {
    Exit,
    Error,
    Timeout,
    Killed,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;

    #[test]
    fn result_event_round_trips_json() {
        let evt = ResultEvent {
            uuid: "test-result-1".into(),
            status: ResultStatus::Exit,
            exit_code: Some(0),
            summary: Some("Build succeeded".into()),
        };
        let json = serde_json::to_string(&evt).unwrap();
        let parsed: ResultEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.uuid, evt.uuid);
        assert_eq!(parsed.status, ResultStatus::Exit);
        assert_eq!(parsed.exit_code, Some(0));
        assert_eq!(parsed.summary, Some("Build succeeded".into()));
    }

    #[test]
    fn result_event_timeout_status() {
        let evt = ResultEvent {
            uuid: "test-result-2".into(),
            status: ResultStatus::Timeout,
            exit_code: None,
            summary: Some("Command timed out after 3600s".into()),
        };
        let json = serde_json::to_string(&evt).unwrap();
        assert!(json.contains(r#""status":"timeout"#));
    }

    #[test]
    fn result_event_rejects_unknown_status() {
        let json = r#"{"uuid":"x","status":"exploded","exit_code":null,"summary":null}"#;
        let result: Result<ResultEvent, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }
}
