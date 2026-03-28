# Unified Session Architecture Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Unify mxdx-fabric and mxdx-launcher into a single session model where every command execution — fire-and-forget, long-running, or interactive — follows the same lifecycle with Matrix threads as ground truth and optional WebRTC acceleration.

**Architecture:** Three new Rust crates (mxdx-worker, mxdx-client, mxdx-coordinator) replace the existing mxdx-fabric + mxdx-launcher split. Shared event types in mxdx-types, config/identity/trust as shared modules. npm packages remain the primary distribution (WASM+JS), with native Rust binaries as a performance option.

**Tech Stack:** Rust (tokio, matrix-sdk 0.16, serde, age, webrtc), WASM (wasm-bindgen, wasm-pack), Node.js 22, TypeScript/JS (CLI shells), TOML config, OS keychain (keytar/libsecret).

**Spec:** `docs/superpowers/specs/2026-03-26-unified-session-architecture-design.md`

---

## File Structure Map

### New/Modified Files by Phase

**Phase 1 — Unified Types, Config, Identity**
```
crates/mxdx-types/src/events/
├── mod.rs                          (MODIFY — add session, worker_info, webrtc modules)
├── session.rs                      (CREATE — unified session event types)
├── worker_info.rs                  (CREATE — merged capability + telemetry)
└── webrtc.rs                       (CREATE — signaling events + to-device messages)
crates/mxdx-types/src/
├── config.rs                       (CREATE — shared config types + TOML loading)
├── identity.rs                     (CREATE — device identity types, keychain trait)
├── trust.rs                        (CREATE — trust store types + logic)
└── lib.rs                          (MODIFY — add config, identity, trust modules)
```

**Phase 2 — mxdx-worker**
```
crates/mxdx-worker/
├── Cargo.toml                      (CREATE)
├── src/
│   ├── lib.rs                      (CREATE — module exports)
│   ├── main.rs                     (CREATE — binary entry point)
│   ├── config.rs                   (CREATE — worker config loading)
│   ├── identity.rs                 (CREATE — keychain integration)
│   ├── trust.rs                    (CREATE — trust enforcement)
│   ├── matrix.rs                   (CREATE — room setup, thread posting)
│   ├── session.rs                  (CREATE — session lifecycle + state machine)
│   ├── executor.rs                 (CREATE — adapted from mxdx-launcher::executor)
│   ├── tmux.rs                     (CREATE — tmux session management)
│   ├── output.rs                   (CREATE — output routing + batching)
│   ├── heartbeat.rs                (CREATE — periodic heartbeat posting)
│   ├── telemetry.rs                (CREATE — host info + capability advertisement)
│   ├── retention.rs                (CREATE — completed session cleanup)
│   └── webrtc.rs                   (CREATE — DataChannel + app-level E2EE)
└── tests/
    ├── session_lifecycle.rs         (CREATE)
    ├── executor_test.rs             (CREATE)
    └── integration.rs               (CREATE)
```

**Phase 3 — mxdx-client**
```
crates/mxdx-client/
├── Cargo.toml                      (CREATE)
├── src/
│   ├── lib.rs                      (CREATE — module exports)
│   ├── main.rs                     (CREATE — binary entry point)
│   ├── config.rs                   (CREATE — client config loading)
│   ├── identity.rs                 (CREATE — keychain integration)
│   ├── trust.rs                    (CREATE — trust commands)
│   ├── matrix.rs                   (CREATE — auth, sync, room discovery)
│   ├── submit.rs                   (CREATE — task submission)
│   ├── tail.rs                     (CREATE — thread tailing)
│   ├── attach.rs                   (CREATE — session attach + WebRTC)
│   ├── ls.rs                       (CREATE — session listing from state events)
│   ├── logs.rs                     (CREATE — thread history fetch)
│   ├── cancel.rs                   (CREATE — cancel + signal)
│   └── reconnect.rs                (CREATE — session reconnection)
└── tests/
    ├── submit_test.rs               (CREATE)
    └── integration.rs               (CREATE)
```

**Phase 4 — mxdx-coordinator refactor**
```
crates/mxdx-coordinator/            (CREATE — renamed/refactored from mxdx-fabric)
├── Cargo.toml
├── src/
│   ├── lib.rs
│   ├── main.rs
│   ├── config.rs
│   ├── router.rs                   (FROM coordinator.rs routing logic)
│   ├── watchlist.rs                (FROM coordinator.rs watch logic)
│   ├── failure.rs                  (FROM failure.rs)
│   ├── claim.rs                    (FROM claim.rs)
│   └── index.rs                    (FROM capability_index.rs)
```

**Phase 5 — npm packages**
```
crates/mxdx-core-wasm/src/lib.rs    (MODIFY — add worker + client WASM bindings)
packages/core/index.js               (MODIFY — export new bindings)
packages/core/src/session-client.js  (CREATE — unified session client wrapper)
packages/launcher/src/runtime.js     (MODIFY — use unified session model)
packages/client/src/                 (MODIFY — all commands use unified model)
packages/mxdx/bin/mxdx.js           (MODIFY — add run/attach/ls/logs/cancel)
```

**Phase 6 — Deprecation + E2E**
```
crates/mxdx-fabric/                  (DEPRECATE — mark for removal)
crates/mxdx-fabric-cli/              (DEPRECATE)
crates/mxdx-launcher/                (DEPRECATE)
packages/e2e-tests/                  (MODIFY — add unified session E2E tests)
```

---

## Phase 1: Unified Types, Config, Identity

### Task 1.1: Session Event Types

**Files:**
- Create: `crates/mxdx-types/src/events/session.rs`
- Modify: `crates/mxdx-types/src/events/mod.rs`

- [ ] **Step 1: Write failing test for SessionTask serialization**

```rust
// In session.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_task_roundtrip() {
        let task = SessionTask {
            uuid: "abc-123".into(),
            sender_id: "@alice:example.com".into(),
            bin: "echo".into(),
            args: vec!["hello".into()],
            env: None,
            cwd: None,
            interactive: false,
            no_room_output: false,
            timeout_seconds: None,
            heartbeat_interval_seconds: 30,
            plan: None,
            required_capabilities: vec![],
            routing_mode: None,
            on_timeout: None,
            on_heartbeat_miss: None,
        };
        let json = serde_json::to_string(&task).unwrap();
        let back: SessionTask = serde_json::from_str(&json).unwrap();
        assert_eq!(back.uuid, "abc-123");
        assert_eq!(back.bin, "echo");
        assert!(!back.interactive);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p mxdx-types session_task_roundtrip`
Expected: FAIL — `SessionTask` not defined

- [ ] **Step 3: Implement all session event structs**

Define these structs with `#[derive(Debug, Clone, Serialize, Deserialize)]`:
- `SessionTask` — thread root (uuid, sender_id, bin, args, env, cwd, interactive, no_room_output, timeout_seconds, heartbeat_interval_seconds, plan, required_capabilities, routing_mode, on_timeout, on_heartbeat_miss)
- `SessionStart` — worker start event (session_uuid, worker_id, tmux_session, pid, started_at)
- `SessionOutput` — batched output (session_uuid, worker_id, stream: OutputStream, data, seq, timestamp)
- `SessionHeartbeat` — liveness (session_uuid, worker_id, timestamp, progress)
- `SessionResult` — completion (session_uuid, worker_id, status: SessionStatus, exit_code, duration_seconds, tail)
- `SessionInput` — client stdin (session_uuid, data)
- `SessionSignal` — client signal (session_uuid, signal)
- `SessionResize` — client resize (session_uuid, cols, rows)
- `SessionCancel` — client cancel (session_uuid, reason, grace_seconds)

Enums:
- `OutputStream` — Stdout, Stderr
- `SessionStatus` — Success, Failed, Timeout, Cancelled

Reuse existing `FailurePolicy` and `RoutingMode` from `events::fabric`.

- [ ] **Step 4: Add module to mod.rs**

```rust
pub mod session;
```

- [ ] **Step 5: Run all session serialization tests**

Run: `cargo test -p mxdx-types session`
Expected: All pass

- [ ] **Step 6: Add tests for all event types (start, output, heartbeat, result, input, signal, resize, cancel)**

Each event gets a roundtrip serialization test + a test for snake_case field names in JSON.

- [ ] **Step 7: Run tests and commit**

Run: `cargo test -p mxdx-types`
```bash
git add crates/mxdx-types/src/events/session.rs crates/mxdx-types/src/events/mod.rs
git commit -m "feat(types): add unified session event types (org.mxdx.session.*)"
```

### Task 1.2: Worker Info State Event

**Files:**
- Create: `crates/mxdx-types/src/events/worker_info.rs`
- Modify: `crates/mxdx-types/src/events/mod.rs`

- [ ] **Step 1: Write failing test for WorkerInfo serialization**

Test that `WorkerInfo` serializes with all fields including nested `tools: Vec<WorkerTool>` and flat `capabilities: Vec<String>`.

- [ ] **Step 2: Implement WorkerInfo struct**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerInfo {
    pub worker_id: String,
    pub host: String,
    pub os: String,
    pub arch: String,
    pub cpu_count: u32,
    pub memory_total_mb: u64,
    pub disk_available_mb: u64,
    pub tools: Vec<WorkerTool>,       // reuse from events::capability
    pub capabilities: Vec<String>,
    pub updated_at: u64,
}
```

Reuse `WorkerTool` and `InputSchema` from `events::capability` — they're already defined.

- [ ] **Step 3: Run tests and commit**

Run: `cargo test -p mxdx-types worker_info`
```bash
git add crates/mxdx-types/src/events/worker_info.rs crates/mxdx-types/src/events/mod.rs
git commit -m "feat(types): add WorkerInfo state event (merged capability + telemetry)"
```

### Task 1.3: WebRTC Event Types

**Files:**
- Create: `crates/mxdx-types/src/events/webrtc.rs`
- Modify: `crates/mxdx-types/src/events/mod.rs`

- [ ] **Step 1: Write failing tests for WebRTC event serialization**

- [ ] **Step 2: Implement WebRTC event types**

Thread events (metadata only, no crypto material):
- `WebRtcOffer` — session_uuid, device_id, timestamp
- `WebRtcAnswer` — session_uuid, device_id, timestamp

To-device messages (private, Olm-encrypted):
- `WebRtcSdp` — session_uuid, sdp_type ("offer"|"answer"), sdp, e2ee_public_key
- `WebRtcIce` — session_uuid, candidate, sdp_mid, sdp_mline_index

- [ ] **Step 3: Run tests and commit**

```bash
git add crates/mxdx-types/src/events/webrtc.rs crates/mxdx-types/src/events/mod.rs
git commit -m "feat(types): add WebRTC signaling event types (split model)"
```

### Task 1.4: Session State Event Types

**Files:**
- Modify: `crates/mxdx-types/src/events/session.rs`

- [ ] **Step 1: Write failing test for ActiveSession state**

- [ ] **Step 2: Implement state event content types**

```rust
/// State key: session/{uuid}/active
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveSessionState {
    pub bin: String,
    pub args: Vec<String>,
    pub pid: Option<u32>,
    pub start_time: u64,
    pub client_id: String,
    pub interactive: bool,
    pub worker_id: String,
}

/// State key: session/{uuid}/completed
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletedSessionState {
    pub exit_code: Option<i32>,
    pub duration_seconds: u64,
    pub completion_time: u64,
}
```

- [ ] **Step 3: Run tests and commit**

```bash
git commit -m "feat(types): add session state event types (active/completed)"
```

### Task 1.5: Shared Configuration Types

**Files:**
- Create: `crates/mxdx-types/src/config.rs`
- Modify: `crates/mxdx-types/Cargo.toml` — add `toml` dependency
- Modify: `crates/mxdx-types/src/lib.rs` — add `config` module

- [ ] **Step 1: Write failing test for config deserialization from TOML**

Test parsing a `defaults.toml` string with `[[accounts]]` array, `[trust]` section, and `[webrtc]` section (including `stun_servers` and `[[webrtc.turn_servers]]`).

- [ ] **Step 2: Implement shared config types**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountConfig {
    pub user_id: String,
    pub homeserver: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustConfig {
    #[serde(default = "default_cross_signing_mode")]
    pub cross_signing_mode: CrossSigningMode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrossSigningMode {
    Auto,
    Manual,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebRtcConfig {
    #[serde(default = "default_stun_servers")]
    pub stun_servers: Vec<String>,
    #[serde(default)]
    pub turn_servers: Vec<TurnServerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnServerConfig {
    pub url: String,
    pub auth_endpoint: Option<String>,
    pub username: Option<String>,
    pub credential: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefaultsConfig {
    #[serde(default)]
    pub accounts: Vec<AccountConfig>,
    #[serde(default)]
    pub trust: TrustConfig,
    #[serde(default)]
    pub webrtc: WebRtcConfig,
}
```

Also add mode-specific config types:
- `WorkerConfig` — room_name, trust_anchor, history_retention, capabilities.extra, telemetry_refresh_seconds
- `ClientConfig` — default_worker_room, coordinator_room, session defaults (timeout_seconds, heartbeat_interval, interactive, no_room_output)
- `CoordinatorConfig` — room, capability_room_prefix, failure defaults (default_on_timeout: "escalate", default_on_heartbeat_miss: "escalate")

- [ ] **Step 3: Implement config loading utility with merge**

```rust
/// Load config from $HOME/.mxdx/{filename}, returns default if file doesn't exist
pub fn load_config<T: DeserializeOwned + Default>(filename: &str) -> Result<T>

/// Merge defaults.toml with mode-specific TOML (mode-specific overrides defaults)
pub fn load_merged_config<D: DeserializeOwned + Default, M: DeserializeOwned + Default>(
    defaults_file: &str,
    mode_file: &str,
) -> Result<(D, M)>
```

- [ ] **Step 4: Test three-level precedence (CLI > mode-specific > defaults)**

Test that field-level overrides from CLI args take precedence over mode-specific TOML, which takes precedence over defaults.toml. Test WebRTC config parsing including STUN servers and TURN server arrays.

- [ ] **Step 5: Run tests and commit**

```bash
git add crates/mxdx-types/src/config.rs crates/mxdx-types/src/lib.rs crates/mxdx-types/Cargo.toml
git commit -m "feat(types): add shared TOML configuration types with load utility"
```

### Task 1.6: Identity Types and Keychain Trait

**Files:**
- Create: `crates/mxdx-types/src/identity.rs`
- Modify: `crates/mxdx-types/src/lib.rs`
- Reference: `crates/mxdx-secrets/` — existing age-based encryption crate. The new `KeychainBackend` trait defined here supersedes direct use of `mxdx-secrets::SecretStore` for device identity storage. `mxdx-secrets` continues to handle coordinator-injected secrets (ADR-0008); the keychain trait handles device keys and trust stores.

- [ ] **Step 1: Write failing test for DeviceIdentity serialization**

- [ ] **Step 2: Implement identity types**

```rust
/// Represents a (host, os_user, matrix_account) device identity
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceIdentity {
    pub device_id: String,
    pub user_id: String,
    pub host: String,
    pub os_user: String,
}

/// Keychain entry naming: mxdx/{user_id}/{device_id}
pub fn keychain_key(user_id: &str, device_id: &str) -> String {
    format!("mxdx/{user_id}/{device_id}")
}

/// Keychain entry for trust store: mxdx/{user_id}/trust-store
pub fn trust_store_key(user_id: &str) -> String {
    format!("mxdx/{user_id}/trust-store")
}

/// Abstract keychain backend (OS keychain or file-based fallback)
pub trait KeychainBackend: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>>;
    fn set(&self, key: &str, value: &[u8]) -> Result<()>;
    fn delete(&self, key: &str) -> Result<()>;
}
```

- [ ] **Step 3: Run tests and commit**

```bash
git add crates/mxdx-types/src/identity.rs crates/mxdx-types/src/lib.rs
git commit -m "feat(types): add device identity types and keychain trait"
```

### Task 1.7: Trust Store Types and Logic

**Files:**
- Create: `crates/mxdx-types/src/trust.rs`
- Modify: `crates/mxdx-types/src/lib.rs`

- [ ] **Step 1: Write failing tests for trust store operations**

Test: add device to trust store, check if trusted, remove device, trust anchor validation.

- [ ] **Step 2: Implement trust store**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustStore {
    /// The Matrix identity this device trusts as anchor
    pub trust_anchor: String,
    /// Trusted device IDs with their signing keys
    pub trusted_devices: HashMap<String, TrustedDevice>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustedDevice {
    pub device_id: String,
    pub user_id: String,
    pub ed25519_key: String,
    pub cross_signed_at: u64,
}

impl TrustStore {
    pub fn new(trust_anchor: String) -> Self;
    pub fn is_trusted(&self, device_id: &str) -> bool;
    pub fn add_device(&mut self, device: TrustedDevice);
    pub fn remove_device(&mut self, device_id: &str);
    pub fn trusted_device_ids(&self) -> Vec<&str>;
    pub fn merge_trust_list(&mut self, devices: Vec<TrustedDevice>, mode: CrossSigningMode);
}
```

- [ ] **Step 3: Test trust list propagation (one-directional merge)**

Test that `merge_trust_list` in `Auto` mode adds all devices, and in `Manual` mode adds none (requires explicit `add_device` calls).

- [ ] **Step 4: Run tests and commit**

```bash
git add crates/mxdx-types/src/trust.rs crates/mxdx-types/src/lib.rs
git commit -m "feat(types): add trust store with cross-signing mode support"
```

### Task 1.8: Event Type Constants

**Files:**
- Modify: `crates/mxdx-types/src/events/session.rs`

- [ ] **Step 1: Add event type string constants**

```rust
pub const SESSION_TASK: &str = "org.mxdx.session.task";
pub const SESSION_START: &str = "org.mxdx.session.start";
pub const SESSION_OUTPUT: &str = "org.mxdx.session.output";
pub const SESSION_HEARTBEAT: &str = "org.mxdx.session.heartbeat";
pub const SESSION_RESULT: &str = "org.mxdx.session.result";
pub const SESSION_INPUT: &str = "org.mxdx.session.input";
pub const SESSION_SIGNAL: &str = "org.mxdx.session.signal";
pub const SESSION_RESIZE: &str = "org.mxdx.session.resize";
pub const SESSION_CANCEL: &str = "org.mxdx.session.cancel";
pub const WORKER_INFO: &str = "org.mxdx.worker.info";
pub const WEBRTC_OFFER: &str = "org.mxdx.session.webrtc.offer";
pub const WEBRTC_ANSWER: &str = "org.mxdx.session.webrtc.answer";
pub const WEBRTC_SDP: &str = "org.mxdx.webrtc.sdp";
pub const WEBRTC_ICE: &str = "org.mxdx.webrtc.ice";
```

- [ ] **Step 2: Commit**

```bash
git commit -m "feat(types): add event type string constants for unified session model"
```

---

## Phase 2: Build mxdx-worker

### Task 2.1: Scaffold Worker Crate

**Files:**
- Create: `crates/mxdx-worker/Cargo.toml`
- Create: `crates/mxdx-worker/src/lib.rs`
- Create: `crates/mxdx-worker/src/main.rs`
- Modify: `Cargo.toml` (workspace members)

- [ ] **Step 1: Create Cargo.toml**

```toml
[package]
name = "mxdx-worker"
version = "1.1.0"
edition = "2021"

[[bin]]
name = "mxdx-worker"
path = "src/main.rs"

[lib]
name = "mxdx_worker"
path = "src/lib.rs"

[dependencies]
mxdx-types = { path = "../mxdx-types" }
mxdx-matrix = { path = "../mxdx-matrix" }
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"
anyhow = "1"
tracing = "0.1"
tracing-subscriber = "0.3"
uuid = { version = "1", features = ["v4"] }
hostname = "0.4"
sysinfo = "0.32"
base64 = "0.22"

[dev-dependencies]
mxdx-test-helpers = { path = "../tests/helpers" }
```

- [ ] **Step 2: Create minimal lib.rs and main.rs**

```rust
// lib.rs
pub mod config;

// main.rs
fn main() {
    println!("mxdx-worker");
}
```

- [ ] **Step 3: Add to workspace and verify it builds**

Run: `cargo build -p mxdx-worker`

- [ ] **Step 4: Commit**

```bash
git add crates/mxdx-worker/ Cargo.toml
git commit -m "feat(worker): scaffold mxdx-worker crate"
```

### Task 2.2: Worker Config Module

**Files:**
- Create: `crates/mxdx-worker/src/config.rs`

- [ ] **Step 1: Write failing test for worker config loading**

Test loading from a TOML string with trust_anchor, history_retention, capabilities.extra fields.

- [ ] **Step 2: Implement WorkerRuntimeConfig**

Wraps `mxdx_types::config::WorkerConfig` with CLI override merge.
```rust
pub struct WorkerRuntimeConfig {
    pub defaults: DefaultsConfig,
    pub worker: WorkerConfig,
    pub resolved_room_name: String,  // computed from hostname.username.account
}

impl WorkerRuntimeConfig {
    pub fn load() -> Result<Self>;
    pub fn with_cli_overrides(self, args: &WorkerArgs) -> Self;
}
```

- [ ] **Step 3: Test default room naming**

Test: `{hostname}.{username}.{localpart}` generation from config.

- [ ] **Step 4: Run tests and commit**

```bash
git commit -m "feat(worker): add config module with TOML loading and CLI overrides"
```

### Task 2.3: Worker Executor Module

**Files:**
- Create: `crates/mxdx-worker/src/executor.rs`
- Reference: `crates/mxdx-launcher/src/executor.rs`

- [ ] **Step 1: Write failing tests for bin validation (no shell metacharacters)**

Test: `validate_bin("echo")` → Ok, `validate_bin("echo; rm -rf /")` → Err

- [ ] **Step 2: Migrate and adapt executor from mxdx-launcher**

Key adaptations:
- `validate_bin(bin: &str)` — single token, no shell metacharacters
- `validate_args(args: &[String])` — no null bytes
- `validate_cwd(cwd: &str)` — absolute path, no `..` traversal, must exist
- `validate_env(env: &HashMap<String, String>)` — key format `[A-Z_][A-Z0-9_]*`
- `ValidatedCommand` struct — validated bin, args, env, cwd

Port the injection checks from `mxdx-launcher/src/executor.rs:validate_command()`.

- [ ] **Step 3: Test all sanitization rules from spec**

Tests for: shell metacharacters in bin, null bytes in args, `..` traversal in cwd, invalid env key names.

- [ ] **Step 4: Run tests and commit**

```bash
git commit -m "feat(worker): add executor module with arg sanitization (from launcher)"
```

### Task 2.4: Worker tmux Module

**Files:**
- Create: `crates/mxdx-worker/src/tmux.rs`
- Reference: `packages/launcher/src/pty-bridge.js`

- [ ] **Step 1: Write failing test for tmux session creation**

- [ ] **Step 2: Implement tmux module**

```rust
pub struct TmuxSession {
    pub session_name: String,
    pub socket_path: PathBuf,
}

impl TmuxSession {
    /// Create a new tmux session running the given command
    pub async fn create(bin: &str, args: &[String], cwd: &str, env: &HashMap<String, String>) -> Result<Self>;
    /// Create an interactive tmux session with PTY
    pub async fn create_interactive(shell: &str) -> Result<Self>;
    /// Send data to the tmux session's stdin
    pub async fn send_keys(&self, data: &[u8]) -> Result<()>;
    /// Resize the tmux window
    pub async fn resize(&self, cols: u32, rows: u32) -> Result<()>;
    /// Capture current scrollback
    pub async fn capture_pane(&self) -> Result<String>;
    /// List existing mxdx sessions
    pub async fn list() -> Result<Vec<String>>;
    /// Kill the session
    pub async fn kill(&self) -> Result<()>;
    /// Check if session is still alive
    pub async fn is_alive(&self) -> Result<bool>;
}
```

Uses a dedicated tmux socket (not user's tmux) — pattern from `packages/launcher/src/pty-bridge.js`.

- [ ] **Step 3: Run tests and commit**

```bash
git commit -m "feat(worker): add tmux session management module"
```

### Task 2.5: Worker Session Module

**Files:**
- Create: `crates/mxdx-worker/src/session.rs`

- [ ] **Step 1: Write failing test for session state machine transitions**

Test: Created → Active (claim) → Running (start) → Completed (result)

- [ ] **Step 2: Implement session lifecycle**

```rust
pub enum SessionState {
    Claimed,     // state event written
    Running,     // process started, start event posted
    Completed,   // result event posted
}

pub struct Session {
    pub uuid: String,
    pub task: SessionTask,
    pub state: SessionState,
    pub tmux: TmuxSession,
    pub started_at: u64,
    pub worker_id: String,
}

pub struct SessionManager {
    sessions: HashMap<String, Session>,
}

impl SessionManager {
    /// Claim a session by writing state event (first writer wins)
    pub async fn claim(&mut self, task: SessionTask, client: &MatrixClient, room_id: &RoomId) -> Result<Session>;
    /// Transition to running after process start
    pub fn mark_running(&mut self, uuid: &str, pid: Option<u32>) -> Result<()>;
    /// Complete a session
    pub async fn complete(&mut self, uuid: &str, status: SessionStatus, exit_code: Option<i32>, client: &MatrixClient, room_id: &RoomId) -> Result<()>;
    /// List active sessions
    pub fn active_sessions(&self) -> Vec<&Session>;
    /// Get session by UUID
    pub fn get(&self, uuid: &str) -> Option<&Session>;
}
```

- [ ] **Step 3: Test claim-as-state-event pattern**

Test: write `session/{uuid}/active` state event, read it back, confirm worker_id matches.

- [ ] **Step 4: Run tests and commit**

```bash
git commit -m "feat(worker): add session lifecycle manager with state event claims"
```

### Task 2.6: Worker Output Module

**Files:**
- Create: `crates/mxdx-worker/src/output.rs`

- [ ] **Step 1: Write failing test for output batching**

Test: feed multiple small writes, verify they batch into a single output event within the batch window.

- [ ] **Step 2: Implement output router**

```rust
pub struct OutputRouter {
    batch_window_ms: u64,      // default 200ms (from launcher's BatchedTerminalSender)
    batch_size_bytes: usize,   // max bytes per event (4KB)
    seq: AtomicU64,
    no_room_output: bool,
}

impl OutputRouter {
    /// Route output to Matrix thread (batched)
    pub async fn post_output(&self, session_uuid: &str, stream: OutputStream, data: &[u8], client: &MatrixClient, room_id: &RoomId, thread_root: &EventId) -> Result<()>;
}
```

Respects `no_room_output` — when set, suppresses stdout/stderr but session metadata events still post.

- [ ] **Step 3: Test no_room_output suppression**

- [ ] **Step 4: Run tests and commit**

```bash
git commit -m "feat(worker): add output routing module with batching and suppression"
```

### Task 2.7: Worker Heartbeat Module

**Files:**
- Create: `crates/mxdx-worker/src/heartbeat.rs`

- [ ] **Step 1: Write failing test for heartbeat posting**

- [ ] **Step 2: Implement heartbeat poster**

```rust
pub struct HeartbeatPoster {
    interval_seconds: u64,
}

impl HeartbeatPoster {
    /// Spawn a background task that posts heartbeats at the configured interval
    pub fn spawn(self, session_uuid: String, worker_id: String, client: Arc<MatrixClient>, room_id: OwnedRoomId, thread_root: OwnedEventId) -> JoinHandle<()>;
}
```

Always active regardless of `no_room_output`. Independent of output events.

- [ ] **Step 3: Run tests and commit**

```bash
git commit -m "feat(worker): add heartbeat posting module (always active)"
```

### Task 2.8: Worker Telemetry Module

**Files:**
- Create: `crates/mxdx-worker/src/telemetry.rs`

- [ ] **Step 1: Write failing test for WorkerInfo generation**

- [ ] **Step 2: Implement telemetry collector**

Uses `sysinfo` crate for host metrics. Builds `WorkerInfo` state event. Probes available binaries for `tools` list (pattern from `process_worker.rs:probe_bin_version()`).

```rust
pub struct TelemetryCollector {
    worker_id: String,
    refresh_seconds: u64,
    extra_capabilities: Vec<String>,
}

impl TelemetryCollector {
    pub fn collect_info(&self) -> Result<WorkerInfo>;
    pub fn spawn_refresh(self, client: Arc<MatrixClient>, room_id: OwnedRoomId) -> JoinHandle<()>;
}
```

- [ ] **Step 3: Run tests and commit**

```bash
git commit -m "feat(worker): add telemetry collector (merged capability + host info)"
```

### Task 2.9: Worker Retention Module

**Files:**
- Create: `crates/mxdx-worker/src/retention.rs`

- [ ] **Step 1: Write failing test for retention sweep**

Test: two completed sessions — one within retention window, one expired. Sweep should remove only the expired one.

- [ ] **Step 2: Implement retention sweep**

```rust
pub struct RetentionSweeper {
    retention_days: u64,   // default 90
}

impl RetentionSweeper {
    /// Sweep completed session state events older than retention window
    pub async fn sweep(&self, client: &MatrixClient, room_id: &RoomId) -> Result<u64>;
    /// Spawn hourly sweep task
    pub fn spawn_periodic(self, client: Arc<MatrixClient>, room_id: OwnedRoomId) -> JoinHandle<()>;
}
```

Removes `session/*/completed` state events (by posting empty content to same state key). Thread content remains in Matrix history.

- [ ] **Step 3: Run tests and commit**

```bash
git commit -m "feat(worker): add retention sweeper for completed session cleanup"
```

### Task 2.10: Worker Matrix Module

**Files:**
- Create: `crates/mxdx-worker/src/matrix.rs`
- Reference: `crates/mxdx-matrix/src/rooms.rs`, `crates/mxdx-fabric/src/worker.rs`

- [ ] **Step 1: Write failing test for worker room creation**

- [ ] **Step 2: Implement worker room operations**

```rust
pub struct WorkerRoom {
    pub room_id: OwnedRoomId,
    pub client: Arc<MatrixClient>,
}

impl WorkerRoom {
    /// Create or find the worker's room (E2EE, named per config)
    pub async fn get_or_create(client: &MatrixClient, room_name: &str) -> Result<Self>;
    /// Post threaded event to a session's thread
    pub async fn post_to_thread(&self, thread_root: &EventId, event_type: &str, content: impl Serialize) -> Result<OwnedEventId>;
    /// Write state event (used for claims, session tracking, worker info)
    pub async fn write_state(&self, event_type: &str, state_key: &str, content: impl Serialize) -> Result<()>;
    /// Read state event
    pub async fn read_state<T: DeserializeOwned>(&self, event_type: &str, state_key: &str) -> Result<Option<T>>;
    /// Remove state event (post empty content)
    pub async fn remove_state(&self, event_type: &str, state_key: &str) -> Result<()>;
    /// Listen for incoming events (task submissions, client input/signal/cancel)
    pub async fn sync_and_dispatch(&self) -> Result<Vec<IncomingEvent>>;
}
```

- [ ] **Step 3: Run tests and commit**

```bash
git commit -m "feat(worker): add Matrix room operations module"
```

### Task 2.11: Worker Identity and Trust Modules

**Files:**
- Create: `crates/mxdx-worker/src/identity.rs`
- Create: `crates/mxdx-worker/src/trust.rs`

- [ ] **Step 1: Write failing test for device identity persistence**

- [ ] **Step 2: Implement identity module**

Wraps `KeychainBackend` for device key storage. Ensures stable device ID across restarts.

- [ ] **Step 3: Implement trust module**

Wraps `TrustStore` with Matrix cross-signing operations. Filters incoming events by trusted device IDs. Rejects room invitations from untrusted devices.

- [ ] **Step 4: Run tests and commit**

```bash
git commit -m "feat(worker): add identity and trust modules with keychain integration"
```

### Task 2.12: Worker WebRTC Stub

**Files:**
- Create: `crates/mxdx-worker/src/webrtc.rs`

- [ ] **Step 1: Create stub WebRTC module**

Create a minimal module with the public interface that the main loop will call. Actual WebRTC implementation is in Phase 6 (Task 6.5). For now, the functions log warnings and return errors indicating WebRTC is not yet implemented.

```rust
pub struct WebRtcManager;

impl WebRtcManager {
    pub fn new() -> Self { Self }
    /// Initiate WebRTC offer for interactive session (stub — returns not-implemented)
    pub async fn initiate_offer(&self, _session_uuid: &str) -> Result<()> {
        tracing::warn!("WebRTC not yet implemented, interactive sessions will use Matrix thread only");
        Err(anyhow::anyhow!("WebRTC not implemented"))
    }
    /// Handle incoming WebRTC answer (stub)
    pub async fn handle_answer(&self, _session_uuid: &str) -> Result<()> {
        Err(anyhow::anyhow!("WebRTC not implemented"))
    }
}
```

- [ ] **Step 2: Add to lib.rs and commit**

```bash
git commit -m "feat(worker): add WebRTC stub module (implemented in Phase 6)"
```

### Task 2.13: Worker Main Loop

**Files:**
- Modify: `crates/mxdx-worker/src/lib.rs`
- Modify: `crates/mxdx-worker/src/main.rs`

- [ ] **Step 1: Wire all modules into the worker main loop**

```rust
// lib.rs — public API
pub async fn run_worker(config: WorkerRuntimeConfig) -> Result<()> {
    // 1. Load identity from keychain (or create new)
    // 2. Login to Matrix
    // 3. Bootstrap trust (cross-sign anchor's devices)
    // 4. Get or create worker room
    // 5. Post initial WorkerInfo state event
    // 6. Spawn telemetry refresh task
    // 7. Spawn retention sweep task
    // 8. Enter main event loop:
    //    - On SessionTask: validate trust, claim, spawn executor + tmux + output + heartbeat
    //      - If interactive: attempt WebRTC (graceful fallback to thread-only if stub returns error)
    //    - On SessionInput: route to tmux session
    //    - On SessionSignal: send signal to process
    //    - On SessionResize: resize tmux
    //    - On SessionCancel: SIGTERM → grace → SIGKILL → result
}
```

- [ ] **Step 2: Implement CLI argument parsing in main.rs**

Use clap or manual arg parsing for: `mxdx-worker start [--trust-anchor ...] [--history-retention ...] [--cross-signing-mode ...]`

- [ ] **Step 3: Integration test — worker starts and posts WorkerInfo**

Uses `mxdx-test-helpers::TuwunelInstance` — start worker, verify WorkerInfo state event appears in room.

- [ ] **Step 4: Run tests and commit**

```bash
git commit -m "feat(worker): wire main loop with full session lifecycle"
```

### Task 2.14: Worker Integration Tests

**Files:**
- Create: `crates/mxdx-worker/tests/session_lifecycle.rs`
- Create: `crates/mxdx-worker/tests/integration.rs`

- [ ] **Step 1: Test — worker claims session, posts start event, posts output, posts result**

- [ ] **Step 2: Test — cancel event triggers SIGTERM → grace → SIGKILL → cancelled result**

- [ ] **Step 3: Test — no_room_output suppresses stdout/stderr but heartbeats still post**

- [ ] **Step 4: Test — heartbeat posted even during quiet periods (no output)**

- [ ] **Step 5: Test — retention sweep removes expired completed sessions**

- [ ] **Step 6: Test — worker rejects task from untrusted device**

- [ ] **Step 7: Test — worker rejects room invitation from untrusted device**

- [ ] **Step 8: Test — trust bootstrap: worker trusts anchor identity's verified devices on first start**

- [ ] **Step 9: Test — cross-signing ceremony: client and worker exchange fingerprints**

- [ ] **Step 10: Test — trust list propagation: worker receives and cross-signs initiator's trust list**

- [ ] **Step 11: Test — manual cross-signing mode: worker requires approval for each new device**

- [ ] **Step 12: Test — config loading: CLI args override TOML, mode-specific overrides defaults**

- [ ] **Step 13: Run all tests and commit**

```bash
git commit -m "test(worker): add integration tests for session lifecycle and trust"
```

---

## Phase 3: Build mxdx-client

### Task 3.1: Scaffold Client Crate

**Files:**
- Create: `crates/mxdx-client/Cargo.toml`
- Create: `crates/mxdx-client/src/lib.rs`
- Create: `crates/mxdx-client/src/main.rs`
- Modify: `Cargo.toml` (workspace)

Same pattern as Task 2.1. Binary name: `mxdx-client`.

- [ ] **Step 1: Create crate, add to workspace, verify build**
- [ ] **Step 2: Commit**

```bash
git commit -m "feat(client): scaffold mxdx-client crate"
```

### Task 3.2: Client Config Module

**Files:**
- Create: `crates/mxdx-client/src/config.rs`

- [ ] **Step 1: Implement ClientRuntimeConfig (TOML + CLI overrides)**

Loads `defaults.toml` + `client.toml`. Fields: default_worker_room, coordinator_room, session defaults (timeout, heartbeat_interval, interactive, no_room_output).

- [ ] **Step 2: Run tests and commit**

```bash
git commit -m "feat(client): add config module"
```

### Task 3.3: Client Matrix Module

**Files:**
- Create: `crates/mxdx-client/src/matrix.rs`

- [ ] **Step 1: Write failing test for room discovery**

- [ ] **Step 2: Implement client matrix module**

```rust
pub struct ClientMatrix {
    client: Arc<MatrixClient>,
}

impl ClientMatrix {
    /// Login and connect (password or session restore)
    pub async fn connect(config: &ClientRuntimeConfig) -> Result<Self>;
    /// Discover worker room (from config or room list scan)
    pub async fn find_worker_room(&self, config: &ClientRuntimeConfig) -> Result<OwnedRoomId>;
    /// Discover coordinator room (from config)
    pub async fn find_coordinator_room(&self, config: &ClientRuntimeConfig) -> Result<Option<OwnedRoomId>>;
    /// Post event to room thread
    pub async fn post_to_thread(&self, room_id: &RoomId, thread_root: &EventId, event_type: &str, content: impl Serialize) -> Result<OwnedEventId>;
    /// Read state events matching a prefix
    pub async fn read_state_events<T: DeserializeOwned>(&self, room_id: &RoomId, event_type: &str) -> Result<Vec<(String, T)>>;
    /// Sync and collect events
    pub async fn sync_events(&self) -> Result<Vec<IncomingEvent>>;
}
```

- [ ] **Step 3: Run tests and commit**

```bash
git commit -m "feat(client): add Matrix auth/sync/room discovery module"
```

### Task 3.4: Client Identity and Trust Modules

**Files:**
- Create: `crates/mxdx-client/src/identity.rs`
- Create: `crates/mxdx-client/src/trust.rs`

- [ ] **Step 1: Write failing test for client device identity persistence**

- [ ] **Step 2: Implement identity module**

Same pattern as worker Task 2.11 — wraps `KeychainBackend` for device key storage.

- [ ] **Step 3: Implement trust module with CLI-facing operations**

```rust
pub struct ClientTrust {
    store: TrustStore,
    keychain: Box<dyn KeychainBackend>,
}

impl ClientTrust {
    /// List trusted device IDs (mxdx trust list)
    pub fn list_trusted(&self) -> Vec<&TrustedDevice>;
    /// Initiate cross-signing with a device (mxdx trust add --device)
    pub async fn add_device(&mut self, device_id: &str, client: &MatrixClient) -> Result<()>;
    /// Revoke trust for a device (mxdx trust remove --device)
    pub fn remove_device(&mut self, device_id: &str) -> Result<()>;
    /// Pull trust list from a trusted device (mxdx trust pull --from)
    pub async fn pull_trust_list(&mut self, device_id: &str, client: &MatrixClient) -> Result<()>;
    /// Show current trust anchor (mxdx trust anchor)
    pub fn trust_anchor(&self) -> &str;
    /// Set trust anchor (mxdx trust anchor set)
    pub fn set_trust_anchor(&mut self, user_id: &str) -> Result<()>;
}
```

- [ ] **Step 4: Run tests and commit**

```bash
git commit -m "feat(client): add identity and trust modules with CLI operations"
```

### Task 3.5: Client Submit Module

**Files:**
- Create: `crates/mxdx-client/src/submit.rs`
- Reference: `crates/mxdx-fabric/src/sender.rs`

- [ ] **Step 1: Write failing test for task submission**

- [ ] **Step 2: Implement submit module**

```rust
/// Submit a task and return the session UUID and thread root event ID
pub async fn submit_task(client: &MatrixClient, room_id: &RoomId, task: SessionTask) -> Result<(String, OwnedEventId)>;
```

Posts `SessionTask` to worker/coordinator room. Returns the session UUID and event ID (thread root). In detached mode (`-d`), the caller prints the UUID and exits immediately. In default mode, the caller passes the event ID to `tail_session`.

- [ ] **Step 3: Run tests and commit**

```bash
git commit -m "feat(client): add task submission module"
```

### Task 3.6: Client Tail Module

**Files:**
- Create: `crates/mxdx-client/src/tail.rs`

- [ ] **Step 1: Write failing test for thread tailing**

- [ ] **Step 2: Implement thread tailer**

Follows a session's thread in real-time. Renders `SessionOutput` events (base64 decode, apply stream markers). Stops on `SessionResult` event.

```rust
pub async fn tail_session(client: &MatrixClient, room_id: &RoomId, thread_root: &EventId, follow: bool) -> Result<()>;
```

- [ ] **Step 3: Run tests and commit**

```bash
git commit -m "feat(client): add thread tailing module"
```

### Task 3.7: Client LS Module

**Files:**
- Create: `crates/mxdx-client/src/ls.rs`

- [ ] **Step 1: Write failing test for session listing**

- [ ] **Step 2: Implement session listing**

Reads `session/*/active` and optionally `session/*/completed` state events. Formats as a process table.

```rust
pub async fn list_sessions(client: &MatrixClient, room_id: &RoomId, include_completed: bool) -> Result<Vec<SessionEntry>>;
```

- [ ] **Step 3: Run tests and commit**

```bash
git commit -m "feat(client): add session listing (ls) module"
```

### Task 3.8: Client Logs Module

**Files:**
- Create: `crates/mxdx-client/src/logs.rs`

- [ ] **Step 1: Implement thread history fetcher**

Fetches all events in a session's thread, filters to `SessionOutput` events, concatenates and decodes data.

- [ ] **Step 2: Run tests and commit**

```bash
git commit -m "feat(client): add logs module (thread history fetch)"
```

### Task 3.9: Client Cancel Module

**Files:**
- Create: `crates/mxdx-client/src/cancel.rs`

- [ ] **Step 1: Implement cancel and signal**

`cancel <uuid>` → posts `SessionCancel` event. `cancel <uuid> --signal SIGKILL` → posts `SessionSignal` event.

- [ ] **Step 2: Run tests and commit**

```bash
git commit -m "feat(client): add cancel/signal module"
```

### Task 3.10: Client Attach Module

**Files:**
- Create: `crates/mxdx-client/src/attach.rs`

- [ ] **Step 1: Implement attach**

Reads active session state. For interactive: initiates WebRTC DataChannel (deferred to Phase 2 WebRTC task). For non-interactive: tails thread.

```rust
pub async fn attach_session(client: &MatrixClient, room_id: &RoomId, uuid: &str, force_interactive: bool) -> Result<()>;
```

- [ ] **Step 2: Run tests and commit**

```bash
git commit -m "feat(client): add session attach module"
```

### Task 3.11: Client Reconnect Module

**Files:**
- Create: `crates/mxdx-client/src/reconnect.rs`

- [ ] **Step 1: Implement reconnection**

On startup, check for active sessions this client previously started (by scanning `session/*/active` state events for matching client_id). Offer to reattach.

- [ ] **Step 2: Run tests and commit**

```bash
git commit -m "feat(client): add session reconnection module"
```

### Task 3.12: Client CLI and Main Loop

**Files:**
- Modify: `crates/mxdx-client/src/main.rs`

- [ ] **Step 1: Implement CLI parsing**

Subcommands matching the spec:
```
mxdx-client run <command> [args...]    (with -d, -i, --no-room-output, --timeout flags)
mxdx-client exec <command> [args...]   (alias for run)
mxdx-client attach <uuid>             (with -i flag)
mxdx-client ls                        (with --all flag)
mxdx-client logs <uuid>               (with --follow flag)
mxdx-client cancel <uuid>             (with --signal flag)
mxdx-client trust list|add|remove|pull|anchor
```

- [ ] **Step 2: Wire CLI to modules**

Key behaviors:
- `run` (default): submit task → tail thread → exit on result
- `run -d`: submit task → print session UUID → exit immediately (detached)
- `run -i`: submit task with `interactive: true` → attach with WebRTC
- `exec`: alias for `run` (backward compat)

- [ ] **Step 3: Test `exec` alias works identically to `run`**

- [ ] **Step 4: Run tests and commit**

```bash
git commit -m "feat(client): implement CLI with all subcommands"
```

### Task 3.13: Client Integration Tests

**Files:**
- Create: `crates/mxdx-client/tests/integration.rs`

- [ ] **Step 1: Test — submit task, tail thread, receive output + result**
- [ ] **Step 2: Test — ls shows active session, shows completed after finish**
- [ ] **Step 3: Test — cancel sends cancel event, worker posts cancelled result**
- [ ] **Step 4: Test — disconnect mid-session, reconnect, resume tailing**
- [ ] **Step 5: Run tests and commit**

```bash
git commit -m "test(client): add integration tests"
```

---

## Phase 4: Refactor mxdx-coordinator

### Task 4.1: Scaffold mxdx-coordinator Crate

**Files:**
- Create: `crates/mxdx-coordinator/Cargo.toml`
- Create: `crates/mxdx-coordinator/src/lib.rs`
- Create: `crates/mxdx-coordinator/src/main.rs`
- Modify: `Cargo.toml` (workspace members)
- Reference: `crates/mxdx-fabric/`

- [ ] **Step 1: Create Cargo.toml with dependencies**

Dependencies: mxdx-types, mxdx-matrix, tokio, serde, serde_json, anyhow, tracing, uuid, hostname.

- [ ] **Step 2: Create minimal lib.rs and main.rs**

- [ ] **Step 3: Add to workspace and verify build**

Run: `cargo build -p mxdx-coordinator`

- [ ] **Step 4: Commit**

```bash
git commit -m "feat(coordinator): scaffold mxdx-coordinator crate"
```

### Task 4.2: Coordinator Config Module

**Files:**
- Create: `crates/mxdx-coordinator/src/config.rs`

- [ ] **Step 1: Write failing test for coordinator config loading**

- [ ] **Step 2: Implement CoordinatorRuntimeConfig**

Loads `defaults.toml` + `coordinator.toml`. Fields: room, capability_room_prefix, failure defaults.

- [ ] **Step 3: Run tests and commit**

```bash
git commit -m "feat(coordinator): add config module"
```

### Task 4.3: Coordinator Router Module

**Files:**
- Create: `crates/mxdx-coordinator/src/router.rs`
- Reference: `crates/mxdx-fabric/src/coordinator.rs` (routing logic only)

- [ ] **Step 1: Write failing test for capability-based routing**

- [ ] **Step 2: Migrate routing logic from coordinator.rs**

Strip out ProcessWorker references. Update to use `org.mxdx.session.*` events and `WorkerInfo` state events for capability matching. Routing modes: Direct, Brokered, Auto.

- [ ] **Step 3: Run tests and commit**

```bash
git commit -m "feat(coordinator): add router module (from fabric routing logic)"
```

### Task 4.4: Coordinator Watchlist Module

**Files:**
- Create: `crates/mxdx-coordinator/src/watchlist.rs`
- Reference: `crates/mxdx-fabric/src/coordinator.rs` (watch logic)

- [ ] **Step 1: Write failing test for heartbeat miss detection**

- [ ] **Step 2: Migrate watchlist logic**

Update heartbeat monitoring to use `SessionHeartbeat` events. Miss detection triggers at `2 * heartbeat_interval_seconds` from the task event.

- [ ] **Step 3: Run tests and commit**

```bash
git commit -m "feat(coordinator): add watchlist module with heartbeat monitoring"
```

### Task 4.5: Coordinator Failure and Claim Modules

**Files:**
- Create: `crates/mxdx-coordinator/src/failure.rs`
- Create: `crates/mxdx-coordinator/src/claim.rs`
- Reference: `crates/mxdx-fabric/src/failure.rs`, `crates/mxdx-fabric/src/claim.rs`

- [ ] **Step 1: Migrate failure policies**

Reuse logic from mxdx-fabric. Update event references to `SessionTask`/`SessionResult`. `RespawnWithContext` uses the `plan` field.

- [ ] **Step 2: Migrate claim arbitration**

Update to use session state events for claims (last-write-wins via `session/{uuid}/active`).

- [ ] **Step 3: Run tests and commit**

```bash
git commit -m "feat(coordinator): add failure policy and claim arbitration modules"
```

### Task 4.6: Coordinator Capability Index Module

**Files:**
- Create: `crates/mxdx-coordinator/src/index.rs`
- Reference: `crates/mxdx-fabric/src/capability_index.rs`

- [ ] **Step 1: Migrate capability index**

Update room naming and capability matching to use `WorkerInfo` state events instead of `CapabilityEvent`.

- [ ] **Step 2: Run tests and commit**

```bash
git commit -m "feat(coordinator): add capability index module"
```

### Task 4.7: Coordinator Main Loop and CLI

**Files:**
- Modify: `crates/mxdx-coordinator/src/lib.rs`
- Modify: `crates/mxdx-coordinator/src/main.rs`

- [ ] **Step 1: Wire all modules into coordinator main loop**

- [ ] **Step 2: Implement CLI parsing**

```
mxdx-coordinator start [--room ...] [--capability-room-prefix ...] [--default-on-timeout ...]
```

- [ ] **Step 3: Run tests and commit**

```bash
git commit -m "feat(coordinator): wire main loop and CLI"
```

### Task 4.8: Coordinator Integration Tests

**Files:**
- Create: `crates/mxdx-coordinator/tests/integration.rs`
- Reference: `crates/mxdx-fabric/tests/e2e_fabric.rs`

- [ ] **Step 1: Port happy path test from e2e_fabric.rs**

Update to use new event types. Test: client → coordinator → worker routing.

- [ ] **Step 2: Test failure policy application (timeout, heartbeat miss)**

- [ ] **Step 3: Test multi-worker capability routing**

- [ ] **Step 4: Run tests and commit**

```bash
git commit -m "test(coordinator): add integration tests (ported from fabric)"
```

---

## Phase 5: Update npm Packages

### Task 5.1: Update WASM Bindings

**Files:**
- Modify: `crates/mxdx-core-wasm/src/lib.rs`

- [ ] **Step 1: Add WASM bindings for worker session types**

Expose: `SessionTask`, `SessionResult`, `ActiveSessionState`, `CompletedSessionState`, `WorkerInfo` — serializable to/from JS via `serde_wasm_bindgen` or JSON string pattern.

- [ ] **Step 2: Add WASM bindings for client operations**

Expose: `submit_task`, `list_sessions`, `tail_session`, `cancel_session` as async WASM functions.

- [ ] **Step 3: Build and test WASM**

```bash
wasm-pack build crates/mxdx-core-wasm --target nodejs --out-dir ../../packages/core/wasm
wasm-pack build crates/mxdx-core-wasm --target web --out-dir ../../packages/web-console/wasm
```

- [ ] **Step 4: Commit**

```bash
git commit -m "feat(wasm): add unified session WASM bindings"
```

### Task 5.2: Update @mxdx/core

**Files:**
- Create: `packages/core/src/session-client.js`
- Modify: `packages/core/index.js`

- [ ] **Step 1: Create session-client.js**

JS wrapper around WASM session operations. Handles process spawning (Node.js child_process), PTY (node-pty) on the JS side while Matrix/E2EE is in WASM.

- [ ] **Step 2: Export from index.js**

- [ ] **Step 3: Run smoke tests**

```bash
node -e "const { SessionClient } = require('@mxdx/core'); console.log('ok');"
```

- [ ] **Step 4: Commit**

```bash
git commit -m "feat(core): add unified session client JS wrapper"
```

### Task 5.3: Update @mxdx/launcher

**Files:**
- Modify: `packages/launcher/src/runtime.js`
- Modify: `packages/launcher/src/config.js`

- [ ] **Step 1: Update LauncherRuntime to use unified session model**

Replace fabric-style heartbeat/result events with `SessionStart`, `SessionOutput`, `SessionHeartbeat`, `SessionResult`. Use session state events for process table.

- [ ] **Step 2: Update config to match new TOML schema**

Read from `~/.mxdx/worker.toml` instead of `~/.config/mxdx/launcher.toml`.

- [ ] **Step 3: Run existing tests (should still pass)**

- [ ] **Step 4: Commit**

```bash
git commit -m "feat(launcher): update to unified session model"
```

### Task 5.4: Update @mxdx/client

**Files:**
- Modify: `packages/client/src/exec.js`
- Modify: `packages/client/src/config.js`
- Create: `packages/client/src/run.js`
- Create: `packages/client/src/ls.js`
- Create: `packages/client/src/logs.js`
- Create: `packages/client/src/attach.js`
- Create: `packages/client/src/cancel.js`

- [ ] **Step 1: Create new command modules**

Each module wraps WASM operations with JS-side I/O (terminal rendering, stdin piping).

- [ ] **Step 2: Update exec.js as alias for run.js**

- [ ] **Step 3: Update config.js for new TOML schema**

- [ ] **Step 4: Commit**

```bash
git commit -m "feat(client): add run/ls/logs/attach/cancel commands"
```

### Task 5.5: Create @mxdx/coordinator npm Package

**Files:**
- Create: `packages/coordinator/package.json`
- Create: `packages/coordinator/bin/mxdx-coordinator.js`
- Create: `packages/coordinator/src/runtime.js`
- Modify: `package.json` (add workspace member)

- [ ] **Step 1: Scaffold package**

Thin JS shell around WASM coordinator logic. Pattern matches @mxdx/launcher structure.

```json
{
  "name": "@mxdx/coordinator",
  "version": "1.1.0",
  "bin": { "mxdx-coordinator": "bin/mxdx-coordinator.js" },
  "dependencies": { "@mxdx/core": "^1.1.0" }
}
```

- [ ] **Step 2: Create bin entry point and runtime**

`bin/mxdx-coordinator.js` — CLI arg parsing, delegates to WASM for Matrix/routing logic.

- [ ] **Step 3: Smoke test**

```bash
node packages/coordinator/bin/mxdx-coordinator.js --help
```

- [ ] **Step 4: Commit**

```bash
git commit -m "feat(coordinator): create @mxdx/coordinator npm package"
```

### Task 5.6: Update @mxdx/cli Dispatcher

**Files:**
- Modify: `packages/mxdx/bin/mxdx.js`

- [ ] **Step 1: Add new subcommands**

Add to SUBCOMMANDS map:
- `run` → `@mxdx/client/bin/mxdx-client.js` (with `run` subcommand)
- `exec` → `@mxdx/client/bin/mxdx-client.js` (with `exec` subcommand, alias for run)
- `attach` → `@mxdx/client/bin/mxdx-client.js` (with `attach` subcommand)
- `ls` → `@mxdx/client/bin/mxdx-client.js` (with `ls` subcommand)
- `logs` → `@mxdx/client/bin/mxdx-client.js` (with `logs` subcommand)
- `cancel` → `@mxdx/client/bin/mxdx-client.js` (with `cancel` subcommand)
- `worker` → `@mxdx/launcher/bin/mxdx-launcher.js` (worker mode)
- `coordinator` → `@mxdx/coordinator/bin/mxdx-coordinator.js` (created in Task 5.5)

Update HELP text to match spec's CLI interface.

- [ ] **Step 2: Smoke test**

```bash
node packages/mxdx/bin/mxdx.js --help
node packages/mxdx/bin/mxdx.js run --help
```

- [ ] **Step 3: Commit**

```bash
git commit -m "feat(cli): add run/attach/ls/logs/cancel/worker/coordinator subcommands"
```

### Task 5.7: Native Binary Build Verification

**Files:**
- No new files — verification only

- [ ] **Step 1: Build all three native binaries**

```bash
cargo build --release -p mxdx-worker -p mxdx-client -p mxdx-coordinator
```

- [ ] **Step 2: Smoke test native binaries**

```bash
./target/release/mxdx-worker --help
./target/release/mxdx-client --help
./target/release/mxdx-coordinator --help
```

- [ ] **Step 3: Verify CLI interface matches npm version**

Same subcommands, same flags, same behavior.

- [ ] **Step 4: Commit any fixes**

```bash
git commit -m "chore: verify native binary builds for worker/client/coordinator"
```

---

## Phase 6: Deprecation, Backward Compatibility, and E2E Tests

### Task 6.1: Backward Compatibility Layer

**Files:**
- Modify: `crates/mxdx-worker/src/matrix.rs` or create `crates/mxdx-worker/src/compat.rs`

- [ ] **Step 1: Add old event translation**

Worker recognizes `org.mxdx.fabric.task` events from old clients and translates to `SessionTask` internally.

```rust
pub fn translate_legacy_task(fabric_task: &TaskEvent) -> SessionTask;
```

- [ ] **Step 2: Test legacy event handling**

- [ ] **Step 3: Commit**

```bash
git commit -m "feat(worker): add backward compatibility for org.mxdx.fabric.task events"
```

### Task 6.2: Deprecate Old Crates

**Files:**
- Modify: `crates/mxdx-fabric/src/lib.rs`
- Modify: `crates/mxdx-launcher/src/lib.rs`

- [ ] **Step 1: Add deprecation notices**

```rust
#![deprecated(note = "Use mxdx-worker and mxdx-coordinator instead")]
```

- [ ] **Step 2: Update workspace Cargo.toml comments**

- [ ] **Step 3: Commit**

```bash
git commit -m "chore: deprecate mxdx-fabric and mxdx-launcher crates"
```

### Task 6.3: E2E Tests — Full Session Flow

**Files:**
- Create: `packages/e2e-tests/tests/unified-session.test.js` (or Rust test)

- [ ] **Step 1: Test — full flow via npm path: client → worker → process → output → client**

- [ ] **Step 2: Test — full flow via native binary path: same flow using Rust binaries**

- [ ] **Step 3: Test — interactive session: WebRTC DataChannel with PTY, send input, receive output**

- [ ] **Step 4: Test — WebRTC failover: disconnect DataChannel mid-session, verify output continues on thread**

- [ ] **Step 5: Test — WebRTC reconnection: re-establish DataChannel, verify no duplicates (seq dedup)**

- [ ] **Step 6: Test — WebRTC upgrade: non-interactive → interactive via `mxdx attach -i`**

- [ ] **Step 7: Test — client disconnect → reconnect → resume tailing**

- [ ] **Step 8: Test — `mxdx ls` shows sessions, `mxdx logs` fetches correct thread**

- [ ] **Step 9: Test — coordinator routes to correct worker based on capabilities (two workers)**

- [ ] **Step 10: Test — fleet scenario: `mxdx ls` shows sessions across workers**

- [ ] **Step 11: Test — backward compat — old `org.mxdx.fabric.task` handled by new worker**

- [ ] **Step 12: Test — beta server credentials from `test-credentials.toml` for real-server validation**

- [ ] **Step 13: Commit**

```bash
git commit -m "test(e2e): add unified session architecture end-to-end tests"
```

### Task 6.4: Security Tests

**Files:**
- Create: `packages/e2e-tests/tests/unified-session-security.test.js`

- [ ] **Step 1: Test — all output E2EE encrypted**
- [ ] **Step 2: Test — session state events use MSC4362 encrypted state**
- [ ] **Step 3: Test — arg sanitization prevents command injection (shell metacharacters in bin, null bytes in args, traversal in cwd)**
- [ ] **Step 4: Test — `env` field validated for proper key format (`[A-Z_][A-Z0-9_]*`)**
- [ ] **Step 5: Test — no_room_output doesn't leak content**
- [ ] **Step 6: Test — device keys stored in OS keychain, not on filesystem**
- [ ] **Step 7: Test — worker rejects task from untrusted device**
- [ ] **Step 8: Test — worker rejects invitation from untrusted device**
- [ ] **Step 9: Test — cross-signing ceremony requires fingerprint confirmation**
- [ ] **Step 10: Test — manual cross-signing mode blocks automatic trust propagation**
- [ ] **Step 11: Test — trust list propagation is one-directional (initiator→worker only)**
- [ ] **Step 12: Test — device identity stable across restarts (no device proliferation)**
- [ ] **Step 13: Test — WebRTC signaling has no crypto material in thread events**
- [ ] **Step 14: Test — WebRTC app-level E2EE: TURN relay cannot read DataChannel payloads**
- [ ] **Step 15: Test — WebRTC ephemeral keys: fresh key pair per connection (no key reuse)**
- [ ] **Step 16: Commit**

```bash
git commit -m "test(security): add unified session security tests"
```

### Task 6.5: WebRTC Integration (Worker + Client)

> **Note:** This task connects the worker's webrtc module (Task 2.12 stub) and client's attach module (Task 3.10). It requires a WebRTC library in Rust (e.g., `webrtc-rs`) or delegation to JS layer.

**Files:**
- Create: `crates/mxdx-worker/src/webrtc.rs` (if not created in Phase 2)
- Modify: `crates/mxdx-client/src/attach.rs`

- [ ] **Step 1: Implement WebRTC DataChannel creation (worker side)**

Split signaling: post metadata-only events to thread, send SDP/ICE/keys via to-device messages.

- [ ] **Step 2: Implement WebRTC connection (client side)**

Handle offer/answer exchange. Derive app-level E2EE key from Curve25519 ephemeral exchange.

- [ ] **Step 3: Implement automatic failover**

On ICE disconnect: worker continues Matrix thread, client falls back to tailing. On reconnect: fresh key exchange, new DataChannel.

- [ ] **Step 4: E2E test — interactive session over WebRTC**

- [ ] **Step 5: E2E test — failover and reconnection**

- [ ] **Step 6: Commit**

```bash
git commit -m "feat: add WebRTC DataChannel acceleration with app-level E2EE"
```

---

## Dependency Graph

```
Phase 1 (types/config/identity/trust)
  ├── Phase 2 (worker) ──────────┐
  │                               ├── Phase 5 (npm packages)
  ├── Phase 3 (client) ──────────┤     │
  │                               │     ├── Phase 6 (E2E + deprecation)
  └── Phase 4 (coordinator) ─────┘     │
                                        └── Done
```

Phases 2, 3, 4 can proceed in parallel after Phase 1 completes.
Phase 5 depends on 2+3+4.
Phase 6 depends on 5.

---

## Key Patterns and References

### Testing with Tuwunel

All integration tests use `TuwunelInstance` from `crates/tests/helpers/`. Pattern:

```rust
let server = TuwunelInstance::start().await?;
let client = server.register_user("worker-user").await?;
// ... create rooms, post events, assert on state
server.stop().await;
```

Beta server credentials in `test-credentials.toml` for real-server validation.

### WASM Build Commands

After ANY Rust changes that affect WASM:
```bash
wasm-pack build crates/mxdx-core-wasm --target nodejs --out-dir ../../packages/core/wasm
wasm-pack build crates/mxdx-core-wasm --target web --out-dir ../../packages/web-console/wasm
rm -f packages/core/wasm/.gitignore packages/web-console/wasm/.gitignore packages/web-console/wasm/package.json
echo '{"type":"commonjs"}' > packages/core/wasm/package.json
```

### Event Posting Pattern (from mxdx-fabric/worker.rs)

```rust
let content = serde_json::to_value(&event)?;
let raw = Raw::from_json(serde_json::to_string(&content)?);
client.send_threaded_event(&room_id, &thread_root, event_type, raw).await?;
```

### State Event Pattern

```rust
// Write
client.send_state_event(&room_id, event_type, state_key, content).await?;
// Read
let state: Option<T> = client.get_room_state_event(&room_id, event_type, state_key).await?;
```

### serde_wasm_bindgen Caveat

`serde_wasm_bindgen::to_value` does NOT work for `serde_json::Value` — returns `{}`. Return JSON strings from WASM and `JSON.parse()` in JS.
