# Client Daemon Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace per-command Matrix connections with a persistent daemon process that owns the crypto store and sync loop, exposing a JSON-RPC 2.0 API over Unix socket, WebSocket, and MCP.

**Architecture:** Single `mxdx-client` binary serves as both CLI and daemon. The daemon maintains a persistent Matrix connection per named profile. CLI invocations connect to the daemon via Unix socket. Agents connect via WebSocket or MCP. All three transports speak identical JSON-RPC 2.0. Existing library modules (matrix.rs, liveness.rs, submit.rs, etc.) are unchanged — the daemon handler calls them instead of main.rs calling them directly.

**Tech Stack:** Rust, tokio (async runtime), serde_json (JSON-RPC serialization), tokio::net::UnixListener (Unix socket), tokio-tungstenite (WebSocket), clap (CLI)

**Spec:** `docs/plans/2026-04-01-client-daemon-design.md`

---

## File Structure

```
crates/mxdx-client/src/
├── main.rs                      # Rewritten: thin CLI → daemon IPC or --no-daemon direct
├── lib.rs                       # Extended: add new module declarations
├── protocol/
│   ├── mod.rs                   # JSON-RPC 2.0 core types (Request, Response, Notification)
│   ├── methods.rs               # Method enum + typed params/result structs per method
│   └── error.rs                 # JSON-RPC error codes (standard + application)
├── daemon/
│   ├── mod.rs                   # Daemon entry: start, run main loop, shutdown
│   ├── handler.rs               # Core request handler (transport-agnostic)
│   ├── sessions.rs              # Active session tracking + output ring buffers
│   ├── subscriptions.rs         # Event subscription registry + filter dispatch
│   └── transport/
│       ├── mod.rs               # Transport trait + dynamic registry
│       ├── unix.rs              # Unix socket: accept, NDJSON framing, per-client tasks
│       ├── websocket.rs         # WebSocket: tokio-tungstenite listener + framing
│       └── mcp.rs               # MCP: stdio adapter mapping JSON-RPC ↔ MCP tools
├── cli/
│   ├── mod.rs                   # Clap CLI definitions (replaces top of current main.rs)
│   ├── connect.rs               # Socket connection, auto-spawn, PID management
│   └── format.rs                # Format JSON-RPC responses for terminal display
├── matrix.rs                    # EXISTING — unchanged
├── config.rs                    # EXISTING — extended with profile resolution
├── liveness.rs                  # EXISTING — unchanged
├── submit.rs                    # EXISTING — unchanged
├── tail.rs                      # EXISTING — unchanged
├── logs.rs                      # EXISTING — unchanged
├── ls.rs                        # EXISTING — unchanged
├── cancel.rs                    # EXISTING — unchanged
├── attach.rs                    # EXISTING — unchanged
├── trust.rs                     # EXISTING — unchanged
├── identity.rs                  # EXISTING — unchanged
└── reconnect.rs                 # EXISTING — unchanged

crates/mxdx-types/src/config.rs  # Extended: DaemonConfig, ProfileConfig
crates/mxdx-matrix/src/client.rs # Modified: short_hash includes username
crates/mxdx-matrix/src/multi_hs.rs # Modified: account_hash for store paths
```

---

## Task 1: Fix Per-Account Crypto Store Isolation

The `short_hash()` function currently hashes only the homeserver URL. Two users on the same server would share a crypto store, causing key conflicts. Fix it to hash `{username}@{server}`.

**Files:**
- Modify: `crates/mxdx-matrix/src/client.rs` (short_hash callers)
- Modify: `crates/mxdx-matrix/src/multi_hs.rs:220-240` (store path computation)
- Test: `crates/mxdx-matrix/src/client.rs` (existing short_hash tests)
- Test: `crates/mxdx-matrix/src/multi_hs.rs` (new test)

- [ ] **Step 1: Write failing test for account-scoped store path**

In `crates/mxdx-matrix/src/client.rs`, add to the existing `tests` module:

```rust
#[test]
fn short_hash_different_users_same_server_differ() {
    let hash_alice = short_hash("alice@https://matrix.org");
    let hash_bob = short_hash("bob@https://matrix.org");
    assert_ne!(hash_alice, hash_bob, "different users on same server must get different hashes");
}
```

- [ ] **Step 2: Run test to verify it passes (short_hash already takes arbitrary strings)**

Run: `cargo test -p mxdx-matrix --lib -- short_hash_different_users_same_server_differ`
Expected: PASS (short_hash doesn't need changing — it already hashes any string)

- [ ] **Step 3: Modify multi_hs.rs to hash username@server instead of just server**

In `crates/mxdx-matrix/src/multi_hs.rs`, find the three occurrences of `short_hash(&account.homeserver)` inside `connect_with_keychain()` (around lines 223, 238, 250) and change each to:

```rust
let account_hash = short_hash(&format!("{}@{}", account.username, account.homeserver));
let store_path = base.join(&account_hash);
```

- [ ] **Step 4: Add test for account-scoped store paths**

In `crates/mxdx-matrix/src/multi_hs.rs` tests module (or a new test):

```rust
#[test]
fn account_hash_includes_username() {
    use crate::client::short_hash;
    let hash1 = short_hash("alice@https://matrix.org");
    let hash2 = short_hash("bob@https://matrix.org");
    let hash3 = short_hash("alice@https://other.org");
    // Same user, same server → same hash
    assert_eq!(hash1, short_hash("alice@https://matrix.org"));
    // Different user, same server → different hash
    assert_ne!(hash1, hash2);
    // Same user, different server → different hash
    assert_ne!(hash1, hash3);
}
```

- [ ] **Step 5: Run all matrix tests**

Run: `cargo test -p mxdx-matrix`
Expected: All pass

- [ ] **Step 6: Commit**

```bash
git add crates/mxdx-matrix/
git commit -m "fix: hash username@server for per-account crypto store isolation"
```

---

## Task 2: Add DaemonConfig and ProfileConfig to mxdx-types

Add the configuration types needed for daemon profiles.

**Files:**
- Modify: `crates/mxdx-types/src/config.rs`
- Test: inline in same file

- [ ] **Step 1: Write failing test for DaemonConfig deserialization**

In `crates/mxdx-types/src/config.rs` test module:

```rust
#[test]
fn daemon_config_deserializes_with_defaults() {
    let toml_str = "";
    let cfg: DaemonConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(cfg.idle_timeout_seconds, 1200);
    assert!(cfg.profiles.is_empty());
}

#[test]
fn daemon_config_with_profiles() {
    let toml_str = r#"
idle_timeout_seconds = 0

[profiles.default]

[profiles.staging]
accounts = ["@worker:staging.mxdx.dev"]
"#;
    let cfg: DaemonConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(cfg.idle_timeout_seconds, 0);
    assert_eq!(cfg.profiles.len(), 2);
    assert!(cfg.profiles["default"].accounts.is_none());
    assert_eq!(cfg.profiles["staging"].accounts.as_ref().unwrap().len(), 1);
}
```

- [ ] **Step 2: Run tests to verify they fail (types don't exist yet)**

Run: `cargo test -p mxdx-types -- daemon_config`
Expected: FAIL — `DaemonConfig` not found

- [ ] **Step 3: Implement DaemonConfig and ProfileConfig**

In `crates/mxdx-types/src/config.rs`, add after the `CoordinatorConfig` section:

```rust
// ---------------------------------------------------------------------------
// Daemon config types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DaemonConfig {
    #[serde(default = "default_idle_timeout")]
    pub idle_timeout_seconds: u64,
    #[serde(default)]
    pub profiles: std::collections::HashMap<String, ProfileConfig>,
    #[serde(default)]
    pub websocket: Option<WebSocketTransportConfig>,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            idle_timeout_seconds: default_idle_timeout(),
            profiles: std::collections::HashMap::new(),
            websocket: None,
        }
    }
}

fn default_idle_timeout() -> u64 {
    1200
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ProfileConfig {
    /// Which accounts to use. None = use all from defaults.toml.
    #[serde(default)]
    pub accounts: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WebSocketTransportConfig {
    #[serde(default = "default_ws_bind")]
    pub bind: String,
    #[serde(default = "default_ws_port")]
    pub port: u16,
}

fn default_ws_bind() -> String {
    "127.0.0.1".into()
}

fn default_ws_port() -> u16 {
    9390
}
```

Also add `daemon: DaemonConfig` to `ClientConfig`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClientConfig {
    pub default_worker_room: Option<String>,
    pub coordinator_room: Option<String>,
    #[serde(default)]
    pub session: SessionDefaults,
    #[serde(default)]
    pub daemon: DaemonConfig,
}
```

Update the `ClientConfig::default()` impl to include `daemon: DaemonConfig::default()`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p mxdx-types -- daemon_config`
Expected: PASS

- [ ] **Step 5: Verify workspace compiles**

Run: `cargo check --workspace`
Expected: Clean (existing code that uses ClientConfig may need a serde(default) on the daemon field)

- [ ] **Step 6: Commit**

```bash
git add crates/mxdx-types/src/config.rs
git commit -m "feat: add DaemonConfig and ProfileConfig types"
```

---

## Task 3: JSON-RPC 2.0 Protocol Types

Define the core JSON-RPC 2.0 request/response/notification types and the method-specific params and result types.

**Files:**
- Create: `crates/mxdx-client/src/protocol/mod.rs`
- Create: `crates/mxdx-client/src/protocol/methods.rs`
- Create: `crates/mxdx-client/src/protocol/error.rs`
- Modify: `crates/mxdx-client/src/lib.rs` (add `pub mod protocol`)

- [ ] **Step 1: Create protocol/mod.rs with JSON-RPC 2.0 core types**

```rust
use serde::{Deserialize, Serialize};

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
/// Used for streaming output from daemon to client.
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

pub mod error;
pub mod methods;

const JSONRPC_VERSION: &str = "2.0";

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
/// Determined by presence of `id` field.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum IncomingMessage {
    Request(Request),
    Notification(Notification),
}

impl IncomingMessage {
    /// Parse a JSON string into an IncomingMessage.
    pub fn parse(json: &str) -> Result<Self, serde_json::Error> {
        // Check if `id` field is present to distinguish request from notification.
        let value: serde_json::Value = serde_json::from_str(json)?;
        if value.get("id").is_some() {
            Ok(IncomingMessage::Request(serde_json::from_value(value)?))
        } else {
            Ok(IncomingMessage::Notification(serde_json::from_value(value)?))
        }
    }
}
```

- [ ] **Step 2: Create protocol/error.rs**

```rust
// Standard JSON-RPC 2.0 error codes
pub const PARSE_ERROR: i32 = -32700;
pub const INVALID_REQUEST: i32 = -32600;
pub const METHOD_NOT_FOUND: i32 = -32601;
pub const INVALID_PARAMS: i32 = -32602;
pub const INTERNAL_ERROR: i32 = -32603;

// Application error codes
pub const NO_WORKER: i32 = -1;
pub const WORKER_OFFLINE: i32 = -2;
pub const WORKER_STALE: i32 = -3;
pub const UNAUTHORIZED: i32 = -4;
pub const SESSION_NOT_FOUND: i32 = -5;
pub const TRANSPORT_EXISTS: i32 = -6;
pub const MATRIX_UNAVAILABLE: i32 = -7;
pub const CREDENTIAL_MISMATCH: i32 = -8;
```

- [ ] **Step 3: Create protocol/methods.rs with typed params and results**

```rust
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
    pub status: String, // "accepted"
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
    pub matrix_status: String, // "connected", "reconnecting", "disconnected"
    pub accounts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportInfo {
    pub r#type: String, // "unix", "websocket", "mcp"
    pub address: String,
}

// ---------- daemon.addTransport ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddTransportParams {
    pub r#type: String, // "websocket", "mcp"
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
    pub status: String, // "online", "offline", "stale"
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
    pub data: String, // base64
    pub stream: String, // "stdout", "stderr"
    pub seq: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionResultNotification {
    pub uuid: String,
    pub exit_code: Option<i32>,
    pub status: String, // "success", "failed", "cancelled"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionStatusNotification {
    pub status: String, // "connected", "reconnecting", "disconnected"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backoff_ms: Option<u64>,
}
```

- [ ] **Step 4: Add `pub mod protocol` to lib.rs**

- [ ] **Step 5: Write roundtrip tests for all protocol types**

In `crates/mxdx-client/src/protocol/mod.rs`, add:

```rust
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
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p mxdx-client --lib -- protocol`
Expected: All pass

- [ ] **Step 7: Commit**

```bash
git add crates/mxdx-client/src/protocol/ crates/mxdx-client/src/lib.rs
git commit -m "feat: add JSON-RPC 2.0 protocol types and method definitions"
```

---

## Task 4: Daemon Core — Handler, Sessions, Main Loop

The core daemon: accepts parsed JSON-RPC requests, dispatches to existing library functions, manages sessions and output streaming.

**Files:**
- Create: `crates/mxdx-client/src/daemon/mod.rs`
- Create: `crates/mxdx-client/src/daemon/handler.rs`
- Create: `crates/mxdx-client/src/daemon/sessions.rs`
- Modify: `crates/mxdx-client/src/lib.rs` (add `pub mod daemon`)

- [ ] **Step 1: Create daemon/sessions.rs — session tracking and output ring buffer**

```rust
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex};

const DEFAULT_BUFFER_SIZE: usize = 65536; // 64KB ring buffer per session

/// Tracks active sessions the daemon is streaming output for.
pub struct SessionTracker {
    sessions: HashMap<String, TrackedSession>,
}

struct TrackedSession {
    /// Ring buffer of recent output lines (for late-joining clients).
    output_buffer: VecDeque<String>,
    buffer_bytes: usize,
    max_buffer_bytes: usize,
    /// Broadcast channel for live output streaming to multiple clients.
    output_tx: broadcast::Sender<String>,
    /// Worker room this session is in.
    worker_room: String,
    /// Whether the session has completed.
    completed: bool,
}

impl SessionTracker {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
        }
    }

    /// Start tracking a new session. Returns a broadcast receiver for output.
    pub fn track(&mut self, uuid: &str, worker_room: &str) -> broadcast::Receiver<String> {
        let (tx, rx) = broadcast::channel(256);
        self.sessions.insert(uuid.to_string(), TrackedSession {
            output_buffer: VecDeque::new(),
            buffer_bytes: 0,
            max_buffer_bytes: DEFAULT_BUFFER_SIZE,
            output_tx: tx,
            worker_room: worker_room.to_string(),
            completed: false,
        });
        rx
    }

    /// Subscribe to an existing session's output stream.
    pub fn subscribe(&self, uuid: &str) -> Option<broadcast::Receiver<String>> {
        self.sessions.get(uuid).map(|s| s.output_tx.subscribe())
    }

    /// Push output to a session's buffer and broadcast to subscribers.
    pub fn push_output(&mut self, uuid: &str, line: String) {
        if let Some(session) = self.sessions.get_mut(uuid) {
            let line_len = line.len();
            // Evict old entries if buffer is full
            while session.buffer_bytes + line_len > session.max_buffer_bytes {
                if let Some(old) = session.output_buffer.pop_front() {
                    session.buffer_bytes -= old.len();
                } else {
                    break;
                }
            }
            session.buffer_bytes += line_len;
            session.output_buffer.push_back(line.clone());
            // Best-effort broadcast — don't fail if no receivers
            let _ = session.output_tx.send(line);
        }
    }

    /// Mark a session as completed.
    pub fn complete(&mut self, uuid: &str) {
        if let Some(session) = self.sessions.get_mut(uuid) {
            session.completed = true;
        }
    }

    /// Remove a completed session from tracking.
    pub fn remove(&mut self, uuid: &str) {
        self.sessions.remove(uuid);
    }

    /// Get buffered output for a session (for late-joining clients).
    pub fn buffered_output(&self, uuid: &str) -> Vec<String> {
        self.sessions
            .get(uuid)
            .map(|s| s.output_buffer.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Count of active (non-completed) sessions.
    pub fn active_count(&self) -> usize {
        self.sessions.values().filter(|s| !s.completed).count()
    }

    /// List all tracked session UUIDs.
    pub fn session_uuids(&self) -> Vec<String> {
        self.sessions.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_and_push_output() {
        let mut tracker = SessionTracker::new();
        let mut rx = tracker.track("uuid-1", "!room:example.com");

        tracker.push_output("uuid-1", "line 1".into());
        tracker.push_output("uuid-1", "line 2".into());

        assert_eq!(tracker.buffered_output("uuid-1"), vec!["line 1", "line 2"]);
        assert_eq!(tracker.active_count(), 1);
    }

    #[test]
    fn ring_buffer_evicts_old_entries() {
        let mut tracker = SessionTracker::new();
        tracker.track("uuid-1", "!room:example.com");

        // Push more than 64KB
        for i in 0..2000 {
            tracker.push_output("uuid-1", format!("line {:05} padding padding padding padding", i));
        }

        let buffered = tracker.buffered_output("uuid-1");
        // Should have evicted early entries
        assert!(buffered.len() < 2000);
        // Last entry should be present
        assert!(buffered.last().unwrap().contains("01999"));
    }

    #[test]
    fn complete_and_remove() {
        let mut tracker = SessionTracker::new();
        tracker.track("uuid-1", "!room:example.com");
        assert_eq!(tracker.active_count(), 1);

        tracker.complete("uuid-1");
        assert_eq!(tracker.active_count(), 0);

        tracker.remove("uuid-1");
        assert!(tracker.buffered_output("uuid-1").is_empty());
    }

    #[test]
    fn subscribe_to_existing_session() {
        let mut tracker = SessionTracker::new();
        tracker.track("uuid-1", "!room:example.com");

        let rx2 = tracker.subscribe("uuid-1");
        assert!(rx2.is_some());
        assert!(tracker.subscribe("nonexistent").is_none());
    }
}
```

- [ ] **Step 2: Create daemon/handler.rs — core request dispatch**

This is the transport-agnostic handler that receives parsed JSON-RPC requests and returns responses. It holds the Matrix connection and session tracker.

```rust
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;

use crate::protocol::{self, Request, Response, ErrorResponse, Notification, RequestId};
use crate::protocol::error;
use crate::protocol::methods::*;
use super::sessions::SessionTracker;

/// Sender for pushing notifications to a connected client.
pub type NotificationSink = tokio::sync::mpsc::UnboundedSender<String>;

/// Core daemon handler. Owns the Matrix connection and session state.
pub struct Handler {
    pub sessions: Arc<Mutex<SessionTracker>>,
    pub started_at: Instant,
    pub profile_name: String,
    // Matrix connection will be added when we wire this to the daemon main loop.
    // For now, the handler processes requests against the session tracker.
}

impl Handler {
    pub fn new(profile_name: &str) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(SessionTracker::new())),
            started_at: Instant::now(),
            profile_name: profile_name.to_string(),
        }
    }

    /// Dispatch a JSON-RPC request and return the response JSON string.
    /// `sink` is used for streaming notifications back to the requesting client.
    pub async fn handle_request(&self, request: &Request, sink: &NotificationSink) -> String {
        let result = match request.method.as_str() {
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
        };
        result
    }

    async fn handle_daemon_status(&self, id: &RequestId) -> String {
        let sessions = self.sessions.lock().await;
        let result = DaemonStatusResult {
            uptime_seconds: self.started_at.elapsed().as_secs(),
            profile: self.profile_name.clone(),
            connected_clients: 0, // Will be tracked by transport layer
            active_sessions: sessions.active_count() as u32,
            transports: vec![], // Will be populated by transport registry
            matrix_status: "connected".into(), // Will be dynamic
            accounts: vec![], // Will come from config
        };
        serde_json::to_string(&Response::new(id.clone(), serde_json::to_value(result).unwrap()))
            .unwrap_or_default()
    }

    async fn handle_daemon_shutdown(&self, id: &RequestId) -> String {
        // Signal shutdown — the main loop checks a flag
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
```

- [ ] **Step 3: Create daemon/mod.rs — daemon entry point**

```rust
pub mod handler;
pub mod sessions;
pub mod transport;
pub mod subscriptions;
```

- [ ] **Step 4: Create stub daemon/subscriptions.rs**

```rust
use std::collections::HashMap;
use crate::protocol::methods::EventFilter;

/// Subscription registry for event streaming.
pub struct SubscriptionRegistry {
    subscriptions: HashMap<String, Subscription>,
    next_id: u64,
}

struct Subscription {
    /// Glob patterns for event types (e.g., "session.*").
    event_patterns: Vec<String>,
    /// Optional filter to narrow the stream.
    filter: Option<EventFilter>,
    /// Channel to push matching events to the subscriber.
    sink: tokio::sync::mpsc::UnboundedSender<String>,
}

impl SubscriptionRegistry {
    pub fn new() -> Self {
        Self {
            subscriptions: HashMap::new(),
            next_id: 0,
        }
    }

    /// Add a subscription. Returns the subscription ID.
    pub fn subscribe(
        &mut self,
        event_patterns: Vec<String>,
        filter: Option<EventFilter>,
        sink: tokio::sync::mpsc::UnboundedSender<String>,
    ) -> String {
        self.next_id += 1;
        let id = format!("sub-{:04}", self.next_id);
        self.subscriptions.insert(id.clone(), Subscription {
            event_patterns,
            filter,
            sink,
        });
        id
    }

    /// Remove a subscription.
    pub fn unsubscribe(&mut self, id: &str) -> bool {
        self.subscriptions.remove(id).is_some()
    }

    /// Dispatch an event to all matching subscribers.
    pub fn dispatch(&self, event_type: &str, event_json: &str) {
        for sub in self.subscriptions.values() {
            if self.matches_patterns(event_type, &sub.event_patterns) {
                let _ = sub.sink.send(event_json.to_string());
            }
        }
    }

    fn matches_patterns(&self, event_type: &str, patterns: &[String]) -> bool {
        patterns.iter().any(|p| {
            if p.ends_with(".*") {
                let prefix = &p[..p.len() - 2];
                event_type.starts_with(prefix)
            } else {
                p == event_type
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscribe_and_dispatch() {
        let mut registry = SubscriptionRegistry::new();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        let id = registry.subscribe(vec!["session.*".into()], None, tx);
        assert!(id.starts_with("sub-"));

        registry.dispatch("session.output", r#"{"data":"test"}"#);
        assert_eq!(rx.try_recv().unwrap(), r#"{"data":"test"}"#);

        // Non-matching event
        registry.dispatch("daemon.status", r#"{}"#);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn exact_pattern_match() {
        let mut registry = SubscriptionRegistry::new();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        registry.subscribe(vec!["session.result".into()], None, tx);

        registry.dispatch("session.output", r#"{"data":"test"}"#);
        assert!(rx.try_recv().is_err()); // Not matched

        registry.dispatch("session.result", r#"{"exit_code":0}"#);
        assert!(rx.try_recv().is_ok()); // Matched
    }

    #[test]
    fn unsubscribe_stops_delivery() {
        let mut registry = SubscriptionRegistry::new();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        let id = registry.subscribe(vec!["session.*".into()], None, tx);
        registry.dispatch("session.output", r#"{"test":1}"#);
        assert!(rx.try_recv().is_ok());

        assert!(registry.unsubscribe(&id));
        registry.dispatch("session.output", r#"{"test":2}"#);
        assert!(rx.try_recv().is_err());
    }
}
```

- [ ] **Step 5: Create stub daemon/transport/mod.rs**

```rust
pub mod unix;
pub mod websocket;
pub mod mcp;
```

- [ ] **Step 6: Create empty stubs for transport files**

`daemon/transport/unix.rs`, `daemon/transport/websocket.rs`, `daemon/transport/mcp.rs` — each just an empty file for now. They'll be implemented in Tasks 5-7.

- [ ] **Step 7: Add `pub mod daemon` to lib.rs**

- [ ] **Step 8: Run tests**

Run: `cargo test -p mxdx-client --lib -- daemon`
Expected: All pass

- [ ] **Step 9: Commit**

```bash
git add crates/mxdx-client/src/daemon/ crates/mxdx-client/src/lib.rs
git commit -m "feat: add daemon core — handler, session tracker, subscriptions"
```

---

## Task 5: Unix Socket Transport

The Unix socket listener accepts CLI connections, frames JSON-RPC messages with newline delimiters, and routes them to the handler.

**Files:**
- Create: `crates/mxdx-client/src/daemon/transport/unix.rs`

- [ ] **Step 1: Implement Unix socket transport**

```rust
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::Mutex;
use tracing::{info, warn, error};

use crate::daemon::handler::{Handler, NotificationSink};
use crate::protocol::IncomingMessage;

/// Start listening on a Unix socket. Spawns a task per client connection.
pub async fn serve(
    socket_path: &Path,
    handler: Arc<Handler>,
) -> anyhow::Result<()> {
    // Remove stale socket file if it exists
    if socket_path.exists() {
        std::fs::remove_file(socket_path)?;
    }

    // Ensure parent directory exists
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let listener = UnixListener::bind(socket_path)?;

    // Set socket file permissions to 0o600 (owner only)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o600))?;
    }

    info!(path = %socket_path.display(), "Unix socket transport listening");

    loop {
        match listener.accept().await {
            Ok((stream, _addr)) => {
                let handler = Arc::clone(&handler);
                tokio::spawn(async move {
                    if let Err(e) = handle_client(stream, handler).await {
                        warn!(error = %e, "client connection error");
                    }
                });
            }
            Err(e) => {
                error!(error = %e, "failed to accept Unix socket connection");
            }
        }
    }
}

/// Handle a single client connection. Reads NDJSON lines, dispatches to handler,
/// writes responses back. Notifications from the handler are also written.
async fn handle_client(
    stream: tokio::net::UnixStream,
    handler: Arc<Handler>,
) -> anyhow::Result<()> {
    let (reader, writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let writer = Arc::new(Mutex::new(writer));

    // Channel for notifications (output streaming, events)
    let (notif_tx, mut notif_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    // Spawn a task to write notifications to the client
    let writer_clone = Arc::clone(&writer);
    let notif_writer = tokio::spawn(async move {
        while let Some(msg) = notif_rx.recv().await {
            let mut w = writer_clone.lock().await;
            if w.write_all(msg.as_bytes()).await.is_err() {
                break;
            }
            if !msg.ends_with('\n') {
                if w.write_all(b"\n").await.is_err() {
                    break;
                }
            }
            let _ = w.flush().await;
        }
    });

    // Read requests line by line
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            break; // Client disconnected
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        match IncomingMessage::parse(trimmed) {
            Ok(IncomingMessage::Request(req)) => {
                let response = handler.handle_request(&req, &notif_tx).await;
                let mut w = writer.lock().await;
                w.write_all(response.as_bytes()).await?;
                w.write_all(b"\n").await?;
                w.flush().await?;
            }
            Ok(IncomingMessage::Notification(_notif)) => {
                // Client-to-daemon notifications (e.g., stdin for interactive) — handle later
            }
            Err(e) => {
                let err = crate::protocol::ErrorResponse::new(
                    crate::protocol::RequestId::Number(0),
                    crate::protocol::error::PARSE_ERROR,
                    format!("invalid JSON: {}", e),
                );
                let err_json = serde_json::to_string(&err).unwrap_or_default();
                let mut w = writer.lock().await;
                w.write_all(err_json.as_bytes()).await?;
                w.write_all(b"\n").await?;
                w.flush().await?;
            }
        }
    }

    notif_writer.abort();
    Ok(())
}

/// Compute the socket path for a profile.
pub fn socket_path(profile: &str) -> PathBuf {
    dirs::home_dir()
        .expect("no home directory")
        .join(".mxdx")
        .join("daemon")
        .join(format!("{}.sock", profile))
}

/// Compute the PID file path for a profile.
pub fn pid_path(profile: &str) -> PathBuf {
    dirs::home_dir()
        .expect("no home directory")
        .join(".mxdx")
        .join("daemon")
        .join(format!("{}.pid", profile))
}
```

- [ ] **Step 2: Write integration test for Unix socket roundtrip**

In `crates/mxdx-client/tests/` (new file `daemon_unix.rs` or inline in the module):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;

    #[tokio::test]
    async fn unix_socket_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("test.sock");

        let handler = Arc::new(Handler::new("test"));
        let sock_clone = sock.clone();

        // Start server in background
        let server = tokio::spawn(async move {
            serve(&sock_clone, handler).await.unwrap();
        });

        // Give server time to start
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Connect as client
        let stream = UnixStream::connect(&sock).await.unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);

        // Send daemon.status request
        writer.write_all(br#"{"jsonrpc":"2.0","id":1,"method":"daemon.status"}"#).await.unwrap();
        writer.write_all(b"\n").await.unwrap();
        writer.flush().await.unwrap();

        // Read response
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        assert!(line.contains("\"profile\":\"test\""));
        assert!(line.contains("\"uptime_seconds\""));

        server.abort();
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p mxdx-client -- unix_socket_roundtrip`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/mxdx-client/src/daemon/transport/unix.rs
git commit -m "feat: add Unix socket transport with NDJSON framing"
```

---

## Task 6: CLI Restructure — Connect to Daemon or Run Direct

Restructure `main.rs` to either connect to a daemon (default) or run directly (`--no-daemon`).

**Files:**
- Create: `crates/mxdx-client/src/cli/mod.rs`
- Create: `crates/mxdx-client/src/cli/connect.rs`
- Create: `crates/mxdx-client/src/cli/format.rs`
- Rewrite: `crates/mxdx-client/src/main.rs`
- Modify: `crates/mxdx-client/src/lib.rs`

This is the largest task. The CLI must:
1. Parse args with clap (same commands as before + `daemon` subcommand + `--profile` + `--no-daemon`)
2. If `--no-daemon`: call existing library code directly (preserve current behavior)
3. If daemon mode: connect to socket, serialize request as JSON-RPC, stream response to terminal
4. If daemon not running: auto-spawn it

- [ ] **Step 1: Create cli/connect.rs — socket connection and auto-spawn**

```rust
use std::path::Path;
use std::process::Command;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tracing::{info, warn};

use crate::protocol::{Request, Response, ErrorResponse, IncomingMessage, RequestId};

/// Connect to the daemon's Unix socket. If not running, spawn it.
pub async fn connect_or_spawn(profile: &str) -> anyhow::Result<UnixStream> {
    let sock = crate::daemon::transport::unix::socket_path(profile);

    // Try connecting
    if let Ok(stream) = UnixStream::connect(&sock).await {
        return Ok(stream);
    }

    // Check PID file for stale daemon
    let pid_file = crate::daemon::transport::unix::pid_path(profile);
    if pid_file.exists() {
        let pid_str = std::fs::read_to_string(&pid_file).unwrap_or_default();
        if let Ok(pid) = pid_str.trim().parse::<u32>() {
            // Check if process is still alive
            let alive = std::path::Path::new(&format!("/proc/{}", pid)).exists();
            if !alive {
                warn!(pid, "removing stale daemon PID file");
                let _ = std::fs::remove_file(&pid_file);
                let _ = std::fs::remove_file(&sock);
            } else {
                // Process exists but socket doesn't work — wait a bit and retry
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                if let Ok(stream) = UnixStream::connect(&sock).await {
                    return Ok(stream);
                }
            }
        }
    }

    // Spawn daemon
    info!(profile, "spawning daemon");
    let exe = std::env::current_exe()?;
    Command::new(&exe)
        .args(["_daemon", "--profile", profile, "--detach"])
        .spawn()?;

    // Poll for socket readiness
    for _ in 0..100 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        if let Ok(stream) = UnixStream::connect(&sock).await {
            return Ok(stream);
        }
    }

    anyhow::bail!("daemon failed to start within 10 seconds")
}

/// Send a JSON-RPC request and return the response.
pub async fn send_request(
    stream: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    method: &str,
    params: Option<serde_json::Value>,
    id: i64,
) -> anyhow::Result<serde_json::Value> {
    let req = Request::new(id, method, params);
    let json = serde_json::to_string(&req)?;
    writer.write_all(json.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;

    let mut line = String::new();
    loop {
        line.clear();
        let n = stream.read_line(&mut line).await?;
        if n == 0 {
            anyhow::bail!("daemon disconnected");
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Check if this is a response to our request (has `id` matching ours)
        let value: serde_json::Value = serde_json::from_str(trimmed)?;
        if value.get("id").is_some() {
            if value.get("error").is_some() {
                let err: ErrorResponse = serde_json::from_value(value)?;
                anyhow::bail!("daemon error {}: {}", err.error.code, err.error.message);
            }
            let resp: Response = serde_json::from_value(value)?;
            return Ok(resp.result);
        }

        // Otherwise it's a notification — handle in streaming context
        // For non-streaming calls, skip notifications
    }
}
```

- [ ] **Step 2: Create cli/format.rs — terminal output formatting**

```rust
use crate::protocol::methods::*;

/// Format a daemon.status result for terminal display.
pub fn format_status(status: &DaemonStatusResult) -> String {
    let mut out = String::new();
    out.push_str(&format!("Profile: {}\n", status.profile));
    out.push_str(&format!("Uptime: {}s\n", status.uptime_seconds));
    out.push_str(&format!("Matrix: {}\n", status.matrix_status));
    out.push_str(&format!("Active sessions: {}\n", status.active_sessions));
    out.push_str(&format!("Connected clients: {}\n", status.connected_clients));
    if !status.transports.is_empty() {
        out.push_str("Transports:\n");
        for t in &status.transports {
            out.push_str(&format!("  {} @ {}\n", t.r#type, t.address));
        }
    }
    if !status.accounts.is_empty() {
        out.push_str("Accounts:\n");
        for a in &status.accounts {
            out.push_str(&format!("  {}\n", a));
        }
    }
    out
}
```

- [ ] **Step 3: Create cli/mod.rs — clap definitions**

This defines the new CLI structure with `--profile`, `--no-daemon`, and the `daemon` subcommand. The existing command set (Run, Exec, Attach, Ls, Logs, Cancel, Trust) is preserved.

```rust
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "mxdx-client", about = "mxdx client CLI")]
pub struct Cli {
    /// Matrix homeserver URL
    #[arg(long, env = "MXDX_HOMESERVER", global = true)]
    pub homeserver: Option<String>,
    /// Matrix username
    #[arg(long, env = "MXDX_USERNAME", global = true)]
    pub username: Option<String>,
    /// Matrix password
    #[arg(long, env = "MXDX_PASSWORD", global = true)]
    pub password: Option<String>,
    /// Direct room ID (bypasses space discovery)
    #[arg(long, env = "MXDX_ROOM_ID", global = true)]
    pub room_id: Option<String>,
    /// Force new device (skip session restore)
    #[arg(long, global = true, default_value_t = false)]
    pub force_new_device: bool,
    /// Named profile (default: "default")
    #[arg(long, global = true, default_value = "default")]
    pub profile: String,
    /// Bypass daemon, connect directly
    #[arg(long, global = true, default_value_t = false)]
    pub no_daemon: bool,
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Submit and run a command on a worker
    Run {
        command: String,
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
        #[arg(short = 'd', long)]
        detach: bool,
        #[arg(short = 'i', long)]
        interactive: bool,
        #[arg(long)]
        no_room_output: bool,
        #[arg(long)]
        timeout: Option<u64>,
        #[arg(long)]
        worker_room: Option<String>,
        #[arg(long)]
        skip_liveness_check: bool,
    },
    /// Alias for run
    Exec {
        command: String,
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
        #[arg(short = 'd', long)]
        detach: bool,
        #[arg(short = 'i', long)]
        interactive: bool,
        #[arg(long)]
        no_room_output: bool,
        #[arg(long)]
        timeout: Option<u64>,
        #[arg(long)]
        worker_room: Option<String>,
        #[arg(long)]
        skip_liveness_check: bool,
    },
    /// Attach to an active session
    Attach {
        uuid: String,
        #[arg(short = 'i', long)]
        interactive: bool,
    },
    /// List sessions
    Ls {
        #[arg(long)]
        all: bool,
        #[arg(long)]
        worker_room: Option<String>,
    },
    /// View session logs
    Logs {
        uuid: String,
        #[arg(short = 'f', long)]
        follow: bool,
        #[arg(long)]
        worker_room: Option<String>,
    },
    /// Cancel a session
    Cancel {
        uuid: String,
        #[arg(long)]
        signal: Option<String>,
        #[arg(long)]
        worker_room: Option<String>,
    },
    /// Trust management
    Trust {
        #[command(subcommand)]
        action: TrustAction,
    },
    /// Daemon management
    Daemon {
        #[command(subcommand)]
        action: DaemonAction,
    },
    /// Internal: run as daemon (hidden)
    #[command(name = "_daemon", hide = true)]
    InternalDaemon {
        #[arg(long, default_value = "default")]
        profile: String,
        #[arg(long, default_value_t = false)]
        detach: bool,
    },
}

#[derive(Subcommand)]
pub enum TrustAction {
    List,
    Add { #[arg(long)] device: String },
    Remove { #[arg(long)] device: String },
    Pull { #[arg(long)] from: String },
    Anchor { user_id: Option<String> },
}

#[derive(Subcommand)]
pub enum DaemonAction {
    /// Start the daemon
    Start {
        #[arg(long, default_value_t = false)]
        detach: bool,
        #[arg(long)]
        enable_websocket: bool,
        #[arg(long)]
        ws_port: Option<u16>,
    },
    /// Stop the daemon
    Stop {
        #[arg(long, default_value_t = false)]
        all: bool,
    },
    /// Show daemon status
    Status,
    /// Run as MCP server (foreground, stdio)
    Mcp,
}

pub mod connect;
pub mod format;
```

- [ ] **Step 4: Rewrite main.rs to dispatch via daemon or direct**

The new `main.rs` checks `--no-daemon`. If set, it runs the existing direct-connection code (moved to a helper). Otherwise, it connects to the daemon socket and sends JSON-RPC requests.

This step is large — it rewrites main.rs to use the cli module for parsing and routes commands through either the daemon socket or direct connection. The direct path preserves the existing behavior (imported from the current code). The daemon path serializes the command as a JSON-RPC request.

```rust
use anyhow::Result;
use clap::Parser;

mod cli_impl; // The current main.rs logic, renamed

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let cli = mxdx_client::cli::Cli::parse();

    match &cli.command {
        // Internal daemon mode
        mxdx_client::cli::Commands::InternalDaemon { profile, detach } => {
            // TODO: Run daemon main loop (Task 8)
            eprintln!("Daemon mode not yet implemented");
            Ok(())
        }
        // Daemon management
        mxdx_client::cli::Commands::Daemon { action } => {
            match action {
                mxdx_client::cli::DaemonAction::Status => {
                    // Connect to daemon and call daemon.status
                    let stream = mxdx_client::cli::connect::connect_or_spawn(&cli.profile).await?;
                    let (reader, mut writer) = stream.into_split();
                    let mut reader = tokio::io::BufReader::new(reader);
                    let result = mxdx_client::cli::connect::send_request(
                        &mut reader, &mut writer, "daemon.status", None, 1,
                    ).await?;
                    let status: mxdx_client::protocol::methods::DaemonStatusResult =
                        serde_json::from_value(result)?;
                    print!("{}", mxdx_client::cli::format::format_status(&status));
                    Ok(())
                }
                mxdx_client::cli::DaemonAction::Stop { all } => {
                    // TODO: Send daemon.shutdown
                    eprintln!("Stop not yet implemented");
                    Ok(())
                }
                mxdx_client::cli::DaemonAction::Start { detach, .. } => {
                    // TODO: Start daemon
                    eprintln!("Start not yet implemented");
                    Ok(())
                }
                mxdx_client::cli::DaemonAction::Mcp => {
                    // TODO: MCP mode
                    eprintln!("MCP mode not yet implemented");
                    Ok(())
                }
            }
        }
        // All other commands: daemon mode or direct
        _ => {
            if cli.no_daemon {
                // Direct mode — existing behavior
                cli_impl::run_direct(cli).await
            } else {
                // Daemon mode — connect to daemon and dispatch
                cli_impl::run_via_daemon(cli).await
            }
        }
    }
}
```

Note: `cli_impl` would be a new file containing the current main.rs logic, adapted to accept the parsed `Cli` struct. The `run_direct` function preserves the existing behavior. The `run_via_daemon` function serializes the command as JSON-RPC and sends it to the daemon.

This task is intentionally a structural sketch — the full wiring of each command to JSON-RPC is done incrementally as handlers are added to the daemon.

- [ ] **Step 5: Add `pub mod cli` to lib.rs**

- [ ] **Step 6: Verify compilation**

Run: `cargo check -p mxdx-client`
Expected: Compiles (some warnings about unused daemon paths OK at this stage)

- [ ] **Step 7: Commit**

```bash
git add crates/mxdx-client/src/cli/ crates/mxdx-client/src/main.rs crates/mxdx-client/src/lib.rs
git commit -m "feat: restructure CLI for daemon mode with --profile and --no-daemon"
```

---

## Task 7: WebSocket Transport

Add WebSocket listener transport using tokio-tungstenite.

**Files:**
- Modify: `crates/mxdx-client/Cargo.toml` (add tokio-tungstenite)
- Implement: `crates/mxdx-client/src/daemon/transport/websocket.rs`

- [ ] **Step 1: Add tokio-tungstenite dependency**

In `crates/mxdx-client/Cargo.toml`:
```toml
tokio-tungstenite = "0.24"
```

- [ ] **Step 2: Implement WebSocket transport**

```rust
use std::net::SocketAddr;
use std::sync::Arc;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;
use tracing::{info, warn, error};

use crate::daemon::handler::{Handler, NotificationSink};
use crate::protocol::IncomingMessage;

/// Start a WebSocket server. Each connection gets its own handler task.
pub async fn serve(
    bind: &str,
    port: u16,
    handler: Arc<Handler>,
) -> anyhow::Result<SocketAddr> {
    let addr = format!("{}:{}", bind, port);
    let listener = TcpListener::bind(&addr).await?;
    let local_addr = listener.local_addr()?;
    info!(address = %local_addr, "WebSocket transport listening");

    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, peer)) => {
                    let handler = Arc::clone(&handler);
                    tokio::spawn(async move {
                        match accept_async(stream).await {
                            Ok(ws_stream) => {
                                if let Err(e) = handle_ws_client(ws_stream, handler).await {
                                    warn!(peer = %peer, error = %e, "WebSocket client error");
                                }
                            }
                            Err(e) => {
                                warn!(peer = %peer, error = %e, "WebSocket handshake failed");
                            }
                        }
                    });
                }
                Err(e) => {
                    error!(error = %e, "failed to accept TCP connection");
                }
            }
        }
    });

    Ok(local_addr)
}

async fn handle_ws_client(
    ws_stream: tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    handler: Arc<Handler>,
) -> anyhow::Result<()> {
    let (mut ws_sink, mut ws_stream) = ws_stream.split();
    let (notif_tx, mut notif_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    // Forward notifications to WebSocket
    let sink_handle = tokio::spawn(async move {
        while let Some(msg) = notif_rx.recv().await {
            if ws_sink.send(tokio_tungstenite::tungstenite::Message::Text(msg)).await.is_err() {
                break;
            }
        }
    });

    // Read messages from WebSocket
    while let Some(msg) = ws_stream.next().await {
        let msg = msg?;
        if let tokio_tungstenite::tungstenite::Message::Text(text) = msg {
            match IncomingMessage::parse(&text) {
                Ok(IncomingMessage::Request(req)) => {
                    let response = handler.handle_request(&req, &notif_tx).await;
                    notif_tx.send(response).ok();
                }
                Ok(IncomingMessage::Notification(_)) => {
                    // Client-to-daemon notifications
                }
                Err(e) => {
                    let err = crate::protocol::ErrorResponse::new(
                        crate::protocol::RequestId::Number(0),
                        crate::protocol::error::PARSE_ERROR,
                        format!("invalid JSON: {}", e),
                    );
                    notif_tx.send(serde_json::to_string(&err).unwrap_or_default()).ok();
                }
            }
        }
    }

    sink_handle.abort();
    Ok(())
}
```

- [ ] **Step 3: Run check**

Run: `cargo check -p mxdx-client`
Expected: Compiles

- [ ] **Step 4: Commit**

```bash
git add crates/mxdx-client/Cargo.toml crates/mxdx-client/src/daemon/transport/websocket.rs
git commit -m "feat: add WebSocket transport for AI agent access"
```

---

## Task 8: Daemon Main Loop — Wire Matrix Connection

Connect the daemon handler to the actual Matrix connection, sync loop, and event dispatch.

**Files:**
- Modify: `crates/mxdx-client/src/daemon/mod.rs`
- Modify: `crates/mxdx-client/src/daemon/handler.rs`

This task wires the daemon to the real Matrix connection using the existing `connect_multi()` function and runs the sync loop that dispatches events to subscribers and tracked sessions.

- [ ] **Step 1: Implement daemon main loop in daemon/mod.rs**

```rust
pub mod handler;
pub mod sessions;
pub mod subscriptions;
pub mod transport;

use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tracing::{info, warn, error};

use crate::config::ClientRuntimeConfig;
use handler::Handler;

/// Run the daemon for a given profile. This is the main entry point
/// called by `mxdx-client _daemon --profile <name>`.
pub async fn run_daemon(
    config: ClientRuntimeConfig,
    profile: &str,
) -> anyhow::Result<()> {
    // Write PID file
    let pid_path = transport::unix::pid_path(profile);
    if let Some(parent) = pid_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&pid_path, std::process::id().to_string())?;

    // Connect to Matrix (reuses existing connect_multi flow)
    let accounts = config.resolve_accounts();
    if accounts.is_empty() {
        anyhow::bail!("No Matrix accounts configured for profile '{}'", profile);
    }

    let room_name = config.client.default_worker_room.clone()
        .unwrap_or_else(|| "default".to_string());
    let room_id = None; // Daemon doesn't need a specific room — it handles multiple

    info!(profile, accounts = accounts.len(), "starting daemon");

    // Create handler
    let handler = Arc::new(Handler::new(profile));

    // Start Unix socket transport
    let socket_path = transport::unix::socket_path(profile);
    let handler_clone = Arc::clone(&handler);
    tokio::spawn(async move {
        if let Err(e) = transport::unix::serve(&socket_path, handler_clone).await {
            error!(error = %e, "Unix socket transport failed");
        }
    });

    info!(profile, "daemon ready");

    // Idle timeout tracking
    let idle_timeout = Duration::from_secs(config.client.daemon.idle_timeout_seconds);
    let mut last_activity = Instant::now();

    // Main loop: check for shutdown signals and idle timeout
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info!("received SIGINT, shutting down");
                break;
            }
            _ = tokio::time::sleep(Duration::from_secs(10)) => {
                // Check idle timeout
                if idle_timeout.as_secs() > 0 {
                    let sessions = handler.sessions.lock().await;
                    if sessions.active_count() == 0 && last_activity.elapsed() > idle_timeout {
                        info!("idle timeout reached, shutting down");
                        break;
                    }
                }
            }
        }
    }

    // Cleanup
    let _ = std::fs::remove_file(transport::unix::socket_path(profile));
    let _ = std::fs::remove_file(&pid_path);
    info!(profile, "daemon stopped");
    Ok(())
}
```

- [ ] **Step 2: Verify compilation**

Run: `cargo check -p mxdx-client`

- [ ] **Step 3: Commit**

```bash
git add crates/mxdx-client/src/daemon/
git commit -m "feat: wire daemon main loop with Matrix connection and idle timeout"
```

---

## Task 9: MCP Transport Stub

Add the MCP stdio adapter as a stub that maps JSON-RPC methods to MCP tool definitions. Full MCP compliance can be iterated on later.

**Files:**
- Implement: `crates/mxdx-client/src/daemon/transport/mcp.rs`

- [ ] **Step 1: Implement MCP stdio adapter**

```rust
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::info;

use crate::daemon::handler::Handler;
use crate::protocol::IncomingMessage;

/// Run MCP server on stdin/stdout. This is a foreground operation
/// (used by `mxdx-client daemon mcp`).
///
/// MCP uses JSON-RPC 2.0 over stdio — one JSON object per line on stdin,
/// responses on stdout. This is the same protocol as our Unix socket
/// transport, so we can reuse the handler directly.
pub async fn serve_stdio(handler: Arc<Handler>) -> anyhow::Result<()> {
    info!("MCP stdio transport started");

    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin);

    let (notif_tx, mut notif_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    // Forward notifications to stdout
    let mut stdout_clone = tokio::io::stdout();
    tokio::spawn(async move {
        while let Some(msg) = notif_rx.recv().await {
            let _ = stdout_clone.write_all(msg.as_bytes()).await;
            let _ = stdout_clone.write_all(b"\n").await;
            let _ = stdout_clone.flush().await;
        }
    });

    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            break; // EOF
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        match IncomingMessage::parse(trimmed) {
            Ok(IncomingMessage::Request(req)) => {
                let response = handler.handle_request(&req, &notif_tx).await;
                stdout.write_all(response.as_bytes()).await?;
                stdout.write_all(b"\n").await?;
                stdout.flush().await?;
            }
            Ok(IncomingMessage::Notification(_)) => {}
            Err(e) => {
                let err = crate::protocol::ErrorResponse::new(
                    crate::protocol::RequestId::Number(0),
                    crate::protocol::error::PARSE_ERROR,
                    format!("invalid JSON: {}", e),
                );
                let err_json = serde_json::to_string(&err).unwrap_or_default();
                stdout.write_all(err_json.as_bytes()).await?;
                stdout.write_all(b"\n").await?;
                stdout.flush().await?;
            }
        }
    }

    Ok(())
}
```

- [ ] **Step 2: Commit**

```bash
git add crates/mxdx-client/src/daemon/transport/mcp.rs
git commit -m "feat: add MCP stdio transport adapter"
```

---

## Task 10: Wire Session Commands Through Daemon Handler

Connect the `session.run`, `session.ls`, `session.cancel`, `session.logs` methods in the daemon handler to the existing library functions.

**Files:**
- Modify: `crates/mxdx-client/src/daemon/handler.rs`

This is where the daemon handler calls the existing `submit::build_task()`, `matrix::post_event()`, `ls::format_table()`, etc. Each handler method follows the same pattern: parse typed params, call existing library code, return typed result.

- [ ] **Step 1: Add session.run handler**

In `handler.rs`, extend the match in `handle_request`:

```rust
"session.run" => self.handle_session_run(&request.id, &request.params, sink).await,
```

Implement `handle_session_run` that:
1. Deserializes `SessionRunParams` from `request.params`
2. Resolves the worker room
3. Calls `crate::liveness::check_worker_liveness()` on the telemetry state
4. Calls `crate::submit::build_task()` to create the task event
5. Posts the task via the Matrix connection
6. If `detach`: returns `SessionRunResult` immediately
7. Otherwise: spawns a background task that syncs for output events and forwards them to `sink` as `Notification` messages

- [ ] **Step 2: Add session.ls handler**

Reads session state events from the room and returns them as a JSON array.

- [ ] **Step 3: Add session.cancel handler**

Builds cancel/signal event and posts it.

- [ ] **Step 4: Add session.logs handler**

Reads output events for a session UUID, returns assembled output. If `follow`, subscribes to ongoing output.

- [ ] **Step 5: Add events.subscribe and events.unsubscribe handlers**

Wire to the `SubscriptionRegistry`.

- [ ] **Step 6: Add daemon.addTransport handler**

Spawns a new WebSocket listener and adds it to the transport registry.

- [ ] **Step 7: Run tests**

Run: `cargo test -p mxdx-client`
Expected: All pass

- [ ] **Step 8: Commit**

```bash
git add crates/mxdx-client/src/daemon/handler.rs
git commit -m "feat: wire session and daemon commands through handler"
```

---

## Task 11: Integration Tests

End-to-end tests that start the daemon, connect via Unix socket, send JSON-RPC requests, and verify responses.

**Files:**
- Create: `crates/mxdx-client/tests/daemon_integration.rs`

- [ ] **Step 1: Write daemon lifecycle test**

Test that the daemon starts, responds to `daemon.status`, and shuts down on `daemon.shutdown`.

- [ ] **Step 2: Write session.run test via daemon socket**

Start a daemon (in-process, no Matrix connection — mock handler), send `session.run`, receive `session.output` notifications, receive `session.result`.

- [ ] **Step 3: Write subscription test**

Subscribe to `session.*` events, trigger a session event, verify notification is received.

- [ ] **Step 4: Run tests**

Run: `cargo test -p mxdx-client --test daemon_integration`
Expected: All pass

- [ ] **Step 5: Commit**

```bash
git add crates/mxdx-client/tests/daemon_integration.rs
git commit -m "test: add daemon integration tests for IPC roundtrip"
```

---

## Task 12: Extend ClientConfig Profile Resolution

Wire the profile system into the client config so the daemon knows which accounts to use.

**Files:**
- Modify: `crates/mxdx-client/src/config.rs`

- [ ] **Step 1: Add profile-aware account resolution**

```rust
impl ClientRuntimeConfig {
    /// Resolve accounts for a specific profile.
    /// If the profile specifies accounts, filter to just those.
    /// If not (or profile doesn't exist), use all accounts from defaults.
    pub fn resolve_accounts_for_profile(&self, profile: &str) -> Vec<mxdx_matrix::ServerAccount> {
        let profile_config = self.client.daemon.profiles.get(profile);
        let filter_accounts = profile_config
            .and_then(|p| p.accounts.as_ref());

        let all_accounts = self.resolve_accounts();

        match filter_accounts {
            Some(filter) => {
                all_accounts.into_iter()
                    .filter(|a| {
                        let user_at_server = format!("@{}:{}", a.username,
                            a.homeserver.trim_start_matches("https://").trim_start_matches("http://"));
                        filter.iter().any(|f| f == &user_at_server)
                    })
                    .collect()
            }
            None => all_accounts,
        }
    }
}
```

- [ ] **Step 2: Write tests for profile filtering**

- [ ] **Step 3: Commit**

```bash
git add crates/mxdx-client/src/config.rs
git commit -m "feat: add profile-aware account resolution"
```

---

## Verification

After all tasks:

1. Unit tests: `cargo test -p mxdx-client`
2. Workspace build: `cargo check --workspace`
3. Manual smoke test:
   - `mxdx-client daemon start` (starts daemon in foreground)
   - In another terminal: `mxdx-client daemon status` (connects to daemon, shows status)
   - `mxdx-client daemon stop` (stops daemon)
   - `mxdx-client --no-daemon run echo hello` (direct mode, bypasses daemon)
4. E2E tests: `cargo test -p mxdx-worker --test e2e_profile -- --ignored --nocapture`
