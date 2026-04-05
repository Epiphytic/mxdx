use serde::{Deserialize, Serialize};

pub mod error;
pub mod methods;

const JSONRPC_VERSION: &str = "2.0";

/// JSON-RPC 2.0 request (has `id`, expects response).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub jsonrpc: String,
    pub id: RequestId,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

/// JSON-RPC 2.0 successful response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub jsonrpc: String,
    pub id: RequestId,
    pub result: serde_json::Value,
}

/// JSON-RPC 2.0 error response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub jsonrpc: String,
    pub id: RequestId,
    pub error: RpcError,
}

/// JSON-RPC 2.0 notification (no `id`, no response expected).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// Request ID — can be a number or string per JSON-RPC 2.0 spec.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum RequestId {
    Number(i64),
    String(String),
}

impl Request {
    pub fn new(id: impl Into<RequestId>, method: &str, params: Option<serde_json::Value>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.into(),
            id: id.into(),
            method: method.into(),
            params,
        }
    }
}

impl Response {
    pub fn new(id: RequestId, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.into(),
            id,
            result,
        }
    }
}

impl ErrorResponse {
    pub fn new(id: RequestId, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.into(),
            id,
            error: RpcError {
                code,
                message: message.into(),
                data: None,
            },
        }
    }
}

impl Notification {
    pub fn new(method: &str, params: Option<serde_json::Value>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.into(),
            method: method.into(),
            params,
        }
    }
}

impl From<i64> for RequestId {
    fn from(n: i64) -> Self {
        RequestId::Number(n)
    }
}

impl From<String> for RequestId {
    fn from(s: String) -> Self {
        RequestId::String(s)
    }
}

/// Incoming message: either a request or a notification.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum IncomingMessage {
    Request(Request),
    Notification(Notification),
}

impl IncomingMessage {
    /// Parse a JSON string into an IncomingMessage.
    pub fn parse(json: &str) -> Result<Self, serde_json::Error> {
        let value: serde_json::Value = serde_json::from_str(json)?;
        if value.get("id").is_some() {
            Ok(IncomingMessage::Request(serde_json::from_value(value)?))
        } else {
            Ok(IncomingMessage::Notification(serde_json::from_value(value)?))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_roundtrip() {
        let req = Request::new(1i64, "session.run", Some(serde_json::json!({"bin": "echo"})));
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"id\":1"));
        let parsed: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.method, "session.run");
    }

    #[test]
    fn response_roundtrip() {
        let resp = Response::new(RequestId::Number(1), serde_json::json!({"uuid": "abc"}));
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: Response = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, RequestId::Number(1));
    }

    #[test]
    fn error_response_roundtrip() {
        let err = ErrorResponse::new(RequestId::Number(1), error::NO_WORKER, "no worker found");
        let json = serde_json::to_string(&err).unwrap();
        let parsed: ErrorResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.error.code, -1);
        assert_eq!(parsed.error.message, "no worker found");
    }

    #[test]
    fn notification_has_no_id() {
        let notif = Notification::new("session.output", Some(serde_json::json!({"data": "abc"})));
        let json = serde_json::to_string(&notif).unwrap();
        assert!(!json.contains("\"id\""));
    }

    #[test]
    fn incoming_message_distinguishes_request_and_notification() {
        let req_json = r#"{"jsonrpc":"2.0","id":1,"method":"session.run","params":{"bin":"ls"}}"#;
        let notif_json = r#"{"jsonrpc":"2.0","method":"session.output","params":{"data":"abc"}}"#;

        assert!(matches!(IncomingMessage::parse(req_json).unwrap(), IncomingMessage::Request(_)));
        assert!(matches!(IncomingMessage::parse(notif_json).unwrap(), IncomingMessage::Notification(_)));
    }

    #[test]
    fn request_id_number_and_string() {
        let num: RequestId = 42i64.into();
        let str_id: RequestId = "req-1".to_string().into();
        assert_eq!(num, RequestId::Number(42));
        assert_eq!(str_id, RequestId::String("req-1".into()));
    }
}
