use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;

use crate::protocol::{Request, Response, ErrorResponse, RequestId};
use crate::protocol::error;
use crate::protocol::methods::*;
use super::sessions::SessionTracker;

pub type NotificationSink = tokio::sync::mpsc::UnboundedSender<String>;

pub struct Handler {
    pub sessions: Arc<Mutex<SessionTracker>>,
    pub started_at: Instant,
    pub profile_name: String,
}

impl Handler {
    pub fn new(profile_name: &str) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(SessionTracker::new())),
            started_at: Instant::now(),
            profile_name: profile_name.to_string(),
        }
    }

    pub async fn handle_request(&self, request: &Request, _sink: &NotificationSink) -> String {
        match request.method.as_str() {
            "daemon.status" => self.handle_daemon_status(&request.id).await,
            "daemon.shutdown" => self.handle_daemon_shutdown(&request.id).await,
            _ => {
                serde_json::to_string(&ErrorResponse::new(
                    request.id.clone(),
                    error::METHOD_NOT_FOUND,
                    format!("unknown method: {}", request.method),
                ))
                .unwrap_or_default()
            }
        }
    }

    async fn handle_daemon_status(&self, id: &RequestId) -> String {
        let sessions = self.sessions.lock().await;
        let result = DaemonStatusResult {
            uptime_seconds: self.started_at.elapsed().as_secs(),
            profile: self.profile_name.clone(),
            connected_clients: 0,
            active_sessions: sessions.active_count() as u32,
            transports: vec![],
            matrix_status: "connected".into(),
            accounts: vec![],
        };
        serde_json::to_string(&Response::new(id.clone(), serde_json::to_value(result).unwrap()))
            .unwrap_or_default()
    }

    async fn handle_daemon_shutdown(&self, id: &RequestId) -> String {
        serde_json::to_string(&Response::new(
            id.clone(),
            serde_json::json!({"status": "shutting_down"}),
        ))
        .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Request;

    #[tokio::test]
    async fn daemon_status_returns_uptime() {
        let handler = Handler::new("default");
        let (sink, _rx) = tokio::sync::mpsc::unbounded_channel();
        let req = Request::new(1i64, "daemon.status", None);
        let resp = handler.handle_request(&req, &sink).await;
        let parsed: Response = serde_json::from_str(&resp).unwrap();
        let result: DaemonStatusResult = serde_json::from_value(parsed.result).unwrap();
        assert_eq!(result.profile, "default");
    }

    #[tokio::test]
    async fn unknown_method_returns_error() {
        let handler = Handler::new("default");
        let (sink, _rx) = tokio::sync::mpsc::unbounded_channel();
        let req = Request::new(1i64, "nonexistent.method", None);
        let resp = handler.handle_request(&req, &sink).await;
        let parsed: ErrorResponse = serde_json::from_str(&resp).unwrap();
        assert_eq!(parsed.error.code, error::METHOD_NOT_FOUND);
    }
}
