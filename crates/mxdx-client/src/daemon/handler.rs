use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use tokio::sync::Mutex;

use crate::matrix::MatrixClientRoom;
use crate::protocol::{Request, Response, ErrorResponse, RequestId};
use crate::protocol::error;
use crate::protocol::methods::*;
use super::sessions::SessionTracker;
use super::subscriptions::SubscriptionRegistry;

pub type NotificationSink = tokio::sync::mpsc::UnboundedSender<String>;

pub struct Handler {
    pub sessions: Arc<Mutex<SessionTracker>>,
    pub subscriptions: Arc<Mutex<SubscriptionRegistry>>,
    pub started_at: Instant,
    pub profile_name: String,
    /// Epoch millis of last client activity, for idle timeout tracking.
    pub last_activity_ms: AtomicU64,
    /// Shared Matrix connection — None until connected.
    pub matrix: Arc<Mutex<Option<MatrixClientRoom>>>,
}

impl Handler {
    pub fn new(profile_name: &str) -> Self {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        Self {
            sessions: Arc::new(Mutex::new(SessionTracker::new())),
            subscriptions: Arc::new(Mutex::new(SubscriptionRegistry::new())),
            started_at: Instant::now(),
            profile_name: profile_name.to_string(),
            last_activity_ms: AtomicU64::new(now_ms),
            matrix: Arc::new(Mutex::new(None)),
        }
    }

    /// Record client activity (resets idle timeout).
    pub fn touch(&self) {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        self.last_activity_ms.store(now_ms, Ordering::Relaxed);
    }

    /// Seconds since last client activity.
    pub fn idle_seconds(&self) -> u64 {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let last = self.last_activity_ms.load(Ordering::Relaxed);
        (now_ms.saturating_sub(last)) / 1000
    }

    /// Store a live Matrix connection for use by session commands.
    pub async fn set_matrix(&self, room: MatrixClientRoom) {
        let mut mx = self.matrix.lock().await;
        *mx = Some(room);
    }

    pub async fn handle_request(&self, request: &Request, sink: &NotificationSink) -> String {
        self.touch();
        match request.method.as_str() {
            "daemon.status" => self.handle_daemon_status(&request.id).await,
            "daemon.shutdown" => self.handle_daemon_shutdown(&request.id).await,
            "session.run" => self.handle_session_run(&request.id, &request.params, sink).await,
            "session.cancel" | "session.signal" => {
                self.handle_session_cancel(&request.id, &request.params).await
            }
            "session.ls" => self.handle_session_ls(&request.id, &request.params).await,
            "session.logs" => self.handle_session_logs(&request.id, &request.params).await,
            "session.attach" => self.handle_session_attach(&request.id, &request.params).await,
            "events.subscribe" => {
                self.handle_events_subscribe(&request.id, &request.params, sink).await
            }
            "events.unsubscribe" => {
                self.handle_events_unsubscribe(&request.id, &request.params).await
            }
            "daemon.addTransport" => {
                self.handle_add_transport(&request.id, &request.params).await
            }
            "daemon.removeTransport" => {
                self.handle_remove_transport(&request.id, &request.params).await
            }
            "worker.list" => self.handle_worker_list(&request.id).await,
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
        let mx_guard = self.matrix.lock().await;
        let matrix_status = if mx_guard.is_some() { "connected" } else { "disconnected" };
        let result = DaemonStatusResult {
            uptime_seconds: self.started_at.elapsed().as_secs(),
            profile: self.profile_name.clone(),
            connected_clients: 0,
            active_sessions: sessions.active_count() as u32,
            transports: vec![],
            matrix_status: matrix_status.into(),
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

    async fn handle_session_run(
        &self,
        id: &RequestId,
        params: &Option<serde_json::Value>,
        sink: &NotificationSink,
    ) -> String {
        let params = match parse_params::<SessionRunParams>(id, params) {
            Ok(p) => p,
            Err(e) => return e,
        };

        let mut mx_guard = self.matrix.lock().await;
        let mx_room = match mx_guard.as_mut() {
            Some(r) => r,
            None => {
                return serde_json::to_string(&ErrorResponse::new(
                    id.clone(),
                    error::MATRIX_UNAVAILABLE,
                    "Matrix connection not yet available",
                ))
                .unwrap_or_default();
            }
        };

        // Resolve worker room name (for session tracking)
        let worker_room_name = params.worker_room.clone()
            .unwrap_or_else(|| "default".to_string());

        // Build task event
        let sender_id = mx_room.user_id_string();
        let task = crate::submit::build_task(
            &params.bin,
            &params.args,
            params.interactive,
            params.no_room_output,
            params.timeout_seconds,
            params.heartbeat_interval.unwrap_or(30),
            &sender_id,
            params.cwd.as_deref(),
        );

        let task_uuid = task.uuid.clone();
        let task_content = match crate::matrix::serialize_event(&task) {
            Ok(c) => c,
            Err(e) => {
                return serde_json::to_string(&ErrorResponse::new(
                    id.clone(),
                    error::INTERNAL_ERROR,
                    format!("failed to serialize task: {e}"),
                ))
                .unwrap_or_default();
            }
        };

        // Submit task to Matrix room
        let event_id = match mx_room
            .post_event_mut(mxdx_types::events::session::SESSION_TASK, task_content)
            .await
        {
            Ok(eid) => eid,
            Err(e) => {
                return serde_json::to_string(&ErrorResponse::new(
                    id.clone(),
                    error::INTERNAL_ERROR,
                    format!("failed to submit task: {e}"),
                ))
                .unwrap_or_default();
            }
        };

        tracing::info!(uuid = %task_uuid, event_id = %event_id, "task submitted via daemon");

        // Track session
        let _rx = {
            let mut sessions = self.sessions.lock().await;
            sessions.track(&task_uuid, &worker_room_name)
        };

        let detach = params.detach;

        if !detach {
            // Non-detach: spawn background task to tail session output and
            // forward it as JSON-RPC notifications through the sink.
            let matrix_arc = Arc::clone(&self.matrix);
            let sessions_arc = Arc::clone(&self.sessions);
            let uuid_clone = task_uuid.clone();
            let sink_clone = sink.clone();

            tokio::spawn(async move {
                Self::tail_session(matrix_arc, sessions_arc, &uuid_clone, &sink_clone).await;
            });
        }

        let result = SessionRunResult {
            uuid: task_uuid,
            status: "submitted".into(),
        };
        serde_json::to_string(&Response::new(
            id.clone(),
            serde_json::to_value(result).unwrap(),
        ))
        .unwrap_or_default()
    }

    /// Background task: sync for session output events and forward them as
    /// JSON-RPC notifications through the client's notification sink.
    async fn tail_session(
        matrix: Arc<Mutex<Option<MatrixClientRoom>>>,
        sessions: Arc<Mutex<SessionTracker>>,
        uuid: &str,
        sink: &NotificationSink,
    ) {
        use crate::matrix::IncomingClientEvent;
        use mxdx_types::events::session::{SessionOutput, SessionResult};

        loop {
            // Acquire matrix lock briefly to sync, then release
            let events = {
                let mut mx_guard = matrix.lock().await;
                let mx_room = match mx_guard.as_mut() {
                    Some(r) => r,
                    None => {
                        tracing::warn!("Matrix connection lost during session tail");
                        break;
                    }
                };
                match mx_room.sync_events_mut().await {
                    Ok(evts) => evts,
                    Err(e) => {
                        tracing::warn!(error = %e, "sync failed during session tail");
                        continue;
                    }
                }
            };
            // Lock released here — other handlers can use the Matrix connection

            for event in events {
                match event {
                    IncomingClientEvent::SessionOutput {
                        session_uuid,
                        content,
                    } => {
                        if session_uuid != uuid {
                            continue;
                        }
                        if let Ok(output) =
                            crate::matrix::deserialize_event::<SessionOutput>(&content)
                        {
                            if let Ok(text) = crate::tail::format_output(&output) {
                                let notif = serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "method": "session.output",
                                    "params": {
                                        "uuid": uuid,
                                        "data": text,
                                    }
                                });
                                let _ = sink.send(serde_json::to_string(&notif).unwrap_or_default());

                                // Also buffer in session tracker
                                let mut s = sessions.lock().await;
                                s.push_output(uuid, text);
                            }
                        }
                    }
                    IncomingClientEvent::SessionResult {
                        session_uuid,
                        content,
                    } => {
                        if session_uuid != uuid {
                            continue;
                        }
                        if let Ok(result) =
                            crate::matrix::deserialize_event::<SessionResult>(&content)
                        {
                            let notif = serde_json::json!({
                                "jsonrpc": "2.0",
                                "method": "session.result",
                                "params": {
                                    "uuid": uuid,
                                    "exit_code": result.exit_code,
                                    "status": crate::tail::format_result(&result),
                                }
                            });
                            let _ = sink.send(serde_json::to_string(&notif).unwrap_or_default());

                            let mut s = sessions.lock().await;
                            s.complete(uuid);
                        }
                        return; // Session complete, stop tailing
                    }
                    _ => {}
                }
            }
        }
    }

    async fn handle_session_cancel(
        &self,
        id: &RequestId,
        params: &Option<serde_json::Value>,
    ) -> String {
        let params = match parse_params::<SessionCancelParams>(id, params) {
            Ok(p) => p,
            Err(e) => return e,
        };

        let mut mx_guard = self.matrix.lock().await;
        let mx_room = match mx_guard.as_mut() {
            Some(r) => r,
            None => {
                return serde_json::to_string(&ErrorResponse::new(
                    id.clone(),
                    error::MATRIX_UNAVAILABLE,
                    "Matrix connection not yet available",
                ))
                .unwrap_or_default();
            }
        };

        use mxdx_types::events::session::{SESSION_CANCEL, SESSION_SIGNAL};

        if let Some(ref sig) = params.signal {
            // Send signal event
            let event = crate::cancel::build_signal(&params.uuid, sig);
            let content = match crate::matrix::serialize_event(&event) {
                Ok(c) => c,
                Err(e) => {
                    return serde_json::to_string(&ErrorResponse::new(
                        id.clone(),
                        error::INTERNAL_ERROR,
                        format!("failed to serialize signal: {e}"),
                    ))
                    .unwrap_or_default();
                }
            };
            if let Err(e) = mx_room.post_event_mut(SESSION_SIGNAL, content).await {
                return serde_json::to_string(&ErrorResponse::new(
                    id.clone(),
                    error::INTERNAL_ERROR,
                    format!("failed to send signal: {e}"),
                ))
                .unwrap_or_default();
            }
        } else {
            // Send cancel event
            let event = crate::cancel::build_cancel(&params.uuid, None, None);
            let content = match crate::matrix::serialize_event(&event) {
                Ok(c) => c,
                Err(e) => {
                    return serde_json::to_string(&ErrorResponse::new(
                        id.clone(),
                        error::INTERNAL_ERROR,
                        format!("failed to serialize cancel: {e}"),
                    ))
                    .unwrap_or_default();
                }
            };
            if let Err(e) = mx_room.post_event_mut(SESSION_CANCEL, content).await {
                return serde_json::to_string(&ErrorResponse::new(
                    id.clone(),
                    error::INTERNAL_ERROR,
                    format!("failed to send cancel: {e}"),
                ))
                .unwrap_or_default();
            }
        }

        // Mark session as complete locally
        let mut sessions = self.sessions.lock().await;
        sessions.complete(&params.uuid);

        serde_json::to_string(&Response::new(
            id.clone(),
            serde_json::json!({"status": "cancelled", "uuid": params.uuid}),
        ))
        .unwrap_or_default()
    }

    async fn handle_session_ls(
        &self,
        id: &RequestId,
        _params: &Option<serde_json::Value>,
    ) -> String {
        // Return locally tracked sessions
        let sessions = self.sessions.lock().await;
        let uuids = sessions.session_uuids();
        serde_json::to_string(&Response::new(
            id.clone(),
            serde_json::json!({"sessions": uuids}),
        ))
        .unwrap_or_default()
    }

    async fn handle_session_logs(
        &self,
        id: &RequestId,
        params: &Option<serde_json::Value>,
    ) -> String {
        let params = match parse_params::<SessionLogsParams>(id, params) {
            Ok(p) => p,
            Err(e) => return e,
        };

        let sessions = self.sessions.lock().await;
        let output = sessions.buffered_output(&params.uuid);
        if output.is_empty() {
            return serde_json::to_string(&ErrorResponse::new(
                id.clone(),
                error::SESSION_NOT_FOUND,
                format!("session {} not found or has no output", params.uuid),
            ))
            .unwrap_or_default();
        }

        serde_json::to_string(&Response::new(
            id.clone(),
            serde_json::json!({"output": output}),
        ))
        .unwrap_or_default()
    }

    async fn handle_session_attach(
        &self,
        id: &RequestId,
        params: &Option<serde_json::Value>,
    ) -> String {
        let _params = match parse_params::<SessionAttachParams>(id, params) {
            Ok(p) => p,
            Err(e) => return e,
        };

        serde_json::to_string(&ErrorResponse::new(
            id.clone(),
            error::MATRIX_UNAVAILABLE,
            "Interactive attach not yet available in daemon mode",
        ))
        .unwrap_or_default()
    }

    async fn handle_events_subscribe(
        &self,
        id: &RequestId,
        params: &Option<serde_json::Value>,
        sink: &NotificationSink,
    ) -> String {
        let params = match parse_params::<EventsSubscribeParams>(id, params) {
            Ok(p) => p,
            Err(e) => return e,
        };

        let mut subs = self.subscriptions.lock().await;
        let sub_id = subs.subscribe(params.events, params.filter, sink.clone());
        let result = EventsSubscribeResult {
            subscription_id: sub_id,
        };
        serde_json::to_string(&Response::new(id.clone(), serde_json::to_value(result).unwrap()))
            .unwrap_or_default()
    }

    async fn handle_events_unsubscribe(
        &self,
        id: &RequestId,
        params: &Option<serde_json::Value>,
    ) -> String {
        let params = match parse_params::<EventsUnsubscribeParams>(id, params) {
            Ok(p) => p,
            Err(e) => return e,
        };

        let mut subs = self.subscriptions.lock().await;
        let removed = subs.unsubscribe(&params.subscription_id);
        serde_json::to_string(&Response::new(
            id.clone(),
            serde_json::json!({"removed": removed}),
        ))
        .unwrap_or_default()
    }

    async fn handle_add_transport(
        &self,
        id: &RequestId,
        params: &Option<serde_json::Value>,
    ) -> String {
        let _params = match parse_params::<AddTransportParams>(id, params) {
            Ok(p) => p,
            Err(e) => return e,
        };

        // Transport addition will be implemented when WebSocket is fully wired
        serde_json::to_string(&ErrorResponse::new(
            id.clone(),
            error::INTERNAL_ERROR,
            "Dynamic transport addition not yet implemented",
        ))
        .unwrap_or_default()
    }

    async fn handle_remove_transport(
        &self,
        id: &RequestId,
        params: &Option<serde_json::Value>,
    ) -> String {
        let _params = match parse_params::<RemoveTransportParams>(id, params) {
            Ok(p) => p,
            Err(e) => return e,
        };

        serde_json::to_string(&ErrorResponse::new(
            id.clone(),
            error::INTERNAL_ERROR,
            "Dynamic transport removal not yet implemented",
        ))
        .unwrap_or_default()
    }

    async fn handle_worker_list(&self, id: &RequestId) -> String {
        // Will be populated when Matrix connection is wired
        let result = WorkerListResult { workers: vec![] };
        serde_json::to_string(&Response::new(id.clone(), serde_json::to_value(result).unwrap()))
            .unwrap_or_default()
    }
}

/// Parse typed params from JSON-RPC params value.
/// Returns the parsed params or a JSON-RPC error string with the correct request ID.
fn parse_params<T: serde::de::DeserializeOwned>(
    id: &RequestId,
    params: &Option<serde_json::Value>,
) -> Result<T, String> {
    match params {
        Some(v) => serde_json::from_value(v.clone()).map_err(|e| {
            serde_json::to_string(&ErrorResponse::new(
                id.clone(),
                error::INVALID_PARAMS,
                format!("invalid params: {}", e),
            ))
            .unwrap_or_default()
        }),
        None => serde_json::from_value(serde_json::json!({})).map_err(|e| {
            serde_json::to_string(&ErrorResponse::new(
                id.clone(),
                error::INVALID_PARAMS,
                format!("params required: {}", e),
            ))
            .unwrap_or_default()
        }),
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

    #[tokio::test]
    async fn session_run_returns_matrix_unavailable() {
        let handler = Handler::new("default");
        let (sink, _rx) = tokio::sync::mpsc::unbounded_channel();
        let req = Request::new(1i64, "session.run", Some(serde_json::json!({"bin": "echo", "args": ["hello"]})));
        let resp = handler.handle_request(&req, &sink).await;
        let parsed: ErrorResponse = serde_json::from_str(&resp).unwrap();
        assert_eq!(parsed.error.code, error::MATRIX_UNAVAILABLE);
    }

    #[tokio::test]
    async fn session_ls_returns_tracked_sessions() {
        let handler = Handler::new("default");
        let (sink, _rx) = tokio::sync::mpsc::unbounded_channel();

        // Track a session
        {
            let mut sessions = handler.sessions.lock().await;
            sessions.track("uuid-1", "!room:example.com");
        }

        let req = Request::new(1i64, "session.ls", None);
        let resp = handler.handle_request(&req, &sink).await;
        let parsed: Response = serde_json::from_str(&resp).unwrap();
        let sessions: Vec<String> = serde_json::from_value(parsed.result["sessions"].clone()).unwrap();
        assert_eq!(sessions, vec!["uuid-1"]);
    }

    #[tokio::test]
    async fn session_logs_returns_buffered_output() {
        let handler = Handler::new("default");
        let (sink, _rx) = tokio::sync::mpsc::unbounded_channel();

        {
            let mut sessions = handler.sessions.lock().await;
            sessions.track("uuid-1", "!room:example.com");
            sessions.push_output("uuid-1", "hello world".into());
        }

        let req = Request::new(1i64, "session.logs", Some(serde_json::json!({"uuid": "uuid-1"})));
        let resp = handler.handle_request(&req, &sink).await;
        let parsed: Response = serde_json::from_str(&resp).unwrap();
        let output: Vec<String> = serde_json::from_value(parsed.result["output"].clone()).unwrap();
        assert_eq!(output, vec!["hello world"]);
    }

    #[tokio::test]
    async fn session_logs_not_found() {
        let handler = Handler::new("default");
        let (sink, _rx) = tokio::sync::mpsc::unbounded_channel();
        let req = Request::new(1i64, "session.logs", Some(serde_json::json!({"uuid": "nonexistent"})));
        let resp = handler.handle_request(&req, &sink).await;
        let parsed: ErrorResponse = serde_json::from_str(&resp).unwrap();
        assert_eq!(parsed.error.code, error::SESSION_NOT_FOUND);
    }

    #[tokio::test]
    async fn events_subscribe_and_unsubscribe() {
        let handler = Handler::new("default");
        let (sink, _rx) = tokio::sync::mpsc::unbounded_channel();

        // Subscribe
        let req = Request::new(1i64, "events.subscribe", Some(serde_json::json!({"events": ["session.*"]})));
        let resp = handler.handle_request(&req, &sink).await;
        let parsed: Response = serde_json::from_str(&resp).unwrap();
        let sub_id = parsed.result["subscription_id"].as_str().unwrap().to_string();
        assert!(sub_id.starts_with("sub-"));

        // Unsubscribe
        let req = Request::new(2i64, "events.unsubscribe", Some(serde_json::json!({"subscription_id": sub_id})));
        let resp = handler.handle_request(&req, &sink).await;
        let parsed: Response = serde_json::from_str(&resp).unwrap();
        assert_eq!(parsed.result["removed"], true);
    }

    #[tokio::test]
    async fn invalid_params_returns_error_with_correct_id() {
        let handler = Handler::new("default");
        let (sink, _rx) = tokio::sync::mpsc::unbounded_channel();
        // session.run requires `bin` field — use id 42 to verify it's echoed back
        let req = Request::new(42i64, "session.run", Some(serde_json::json!({})));
        let resp = handler.handle_request(&req, &sink).await;
        let parsed: ErrorResponse = serde_json::from_str(&resp).unwrap();
        assert_eq!(parsed.error.code, error::INVALID_PARAMS);
        assert_eq!(parsed.id, RequestId::Number(42));
    }

    #[tokio::test]
    async fn worker_list_returns_empty() {
        let handler = Handler::new("default");
        let (sink, _rx) = tokio::sync::mpsc::unbounded_channel();
        let req = Request::new(1i64, "worker.list", None);
        let resp = handler.handle_request(&req, &sink).await;
        let parsed: Response = serde_json::from_str(&resp).unwrap();
        let result: WorkerListResult = serde_json::from_value(parsed.result).unwrap();
        assert!(result.workers.is_empty());
    }
}
