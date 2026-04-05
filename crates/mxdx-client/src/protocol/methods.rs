use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---------- session.run ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRunParams {
    pub bin: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<HashMap<String, String>>,
    #[serde(default)]
    pub interactive: bool,
    #[serde(default)]
    pub no_room_output: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heartbeat_interval: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_room: Option<String>,
    #[serde(default)]
    pub detach: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRunResult {
    pub uuid: String,
    pub status: String,
}

// ---------- session.cancel / session.signal ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionCancelParams {
    pub uuid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_room: Option<String>,
}

// ---------- session.ls ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionLsParams {
    #[serde(default)]
    pub all: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_room: Option<String>,
}

// ---------- session.logs ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionLogsParams {
    pub uuid: String,
    #[serde(default)]
    pub follow: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_room: Option<String>,
}

// ---------- session.attach ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionAttachParams {
    pub uuid: String,
    #[serde(default)]
    pub interactive: bool,
}

// ---------- events.subscribe ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventsSubscribeParams {
    pub events: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<EventFilter>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EventFilter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uuid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_room: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventsSubscribeResult {
    pub subscription_id: String,
}

// ---------- events.unsubscribe ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventsUnsubscribeParams {
    pub subscription_id: String,
}

// ---------- daemon.status ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonStatusResult {
    pub uptime_seconds: u64,
    pub profile: String,
    pub connected_clients: u32,
    pub active_sessions: u32,
    pub transports: Vec<TransportInfo>,
    pub matrix_status: String,
    pub accounts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportInfo {
    pub r#type: String,
    pub address: String,
}

// ---------- daemon.addTransport ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddTransportParams {
    pub r#type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddTransportResult {
    pub address: String,
}

// ---------- daemon.removeTransport ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoveTransportParams {
    pub r#type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
}

// ---------- worker.list ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerInfo {
    pub worker_id: String,
    pub room_id: String,
    pub status: String,
    pub capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerListResult {
    pub workers: Vec<WorkerInfo>,
}

// ---------- Streaming notifications ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionOutputNotification {
    pub uuid: String,
    pub data: String,
    pub stream: String,
    pub seq: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionResultNotification {
    pub uuid: String,
    pub exit_code: Option<i32>,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionStatusNotification {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backoff_ms: Option<u64>,
}
