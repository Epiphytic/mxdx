# Unified Session Wiring & Real E2E Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire up the Matrix integration layer in mxdx-worker and mxdx-client so the compiled binaries actually work end-to-end, then prove it with real E2E tests that spawn the binaries as subprocesses, and benchmark performance across Rust binaries, npm+WASM, and SSH.

**Architecture:** The scaffolding from the previous plan created all the types, helpers, and module structure. This plan fills in the integration layer: implementing `WorkerRoomOps` and `ClientRoomOps` using `mxdx_matrix::MatrixClient`, wiring the event loops in `run_worker()` and the client CLI commands, and building true E2E tests that exercise the compiled binaries exactly as a user would.

**Tech Stack:** Rust (tokio, matrix-sdk 0.16 via mxdx-matrix, clap), Node.js 22 (commander, @mxdx/core WASM), TOML config, TuwunelInstance for local tests, beta servers (ca1/ca2-beta.mxdx.dev) for federation tests.

**Test credentials:** `test-credentials.toml` (gitignored) with accounts on both ca1-beta.mxdx.dev and ca2-beta.mxdx.dev. Any user with this file can reproduce all tests.

**Critical rule from CLAUDE.md:** E2E tests MUST exercise compiled binaries as subprocesses. Tests that use library code directly are integration tests, not E2E tests.

---

## File Structure Map

### Phase A — Worker Matrix Integration
```
crates/mxdx-worker/src/
├── matrix.rs                   (MODIFY — implement WorkerRoomOps for MatrixClient)
├── lib.rs                      (MODIFY — real event loop in run_worker())
├── main.rs                     (MODIFY — add --homeserver, --username, --password flags)
└── config.rs                   (MODIFY — add account credentials to WorkerRuntimeConfig)
```

### Phase B — Client Matrix Integration
```
crates/mxdx-client/src/
├── matrix.rs                   (MODIFY — implement ClientRoomOps for MatrixClient)
├── main.rs                     (MODIFY — wire all CLI commands to Matrix)
├── lib.rs                      (MODIFY — export connect function)
└── config.rs                   (MODIFY — add account credentials to ClientRuntimeConfig)
```

### Phase C — Real E2E Tests (Binary Subprocess)
```
crates/mxdx-worker/tests/
├── e2e_binary.rs               (CREATE — true E2E tests spawning mxdx-worker binary)
├── e2e_session.rs              (RENAME concepts — these are integration tests)
├── beta_server_session.rs      (RENAME concepts — these are integration tests)
└── bench_latency.rs            (KEEP — already tests event schema latency)

crates/mxdx-client/tests/
├── e2e_binary.rs               (CREATE — true E2E tests spawning mxdx-client binary)
└── e2e_session.rs              (RENAME concepts — these are integration tests)

tests/
└── e2e/
    ├── mod.rs                  (CREATE — shared test harness)
    ├── full_lifecycle.rs       (CREATE — worker+client binary lifecycle)
    └── federation.rs           (CREATE — cross-server binary tests)
```

### Phase D — npm/WASM Integration
```
packages/client/src/
├── run.js                      (MODIFY — wire to Matrix via @mxdx/core)
├── ls.js                       (MODIFY — wire to Matrix)
├── logs.js                     (MODIFY — wire to Matrix)
├── cancel.js                   (MODIFY — wire to Matrix)
└── attach.js                   (MODIFY — wire to Matrix)

packages/launcher/src/
└── runtime.js                  (MODIFY — migrate to unified session events)
```

### Phase E — Performance Benchmarks
```
crates/mxdx-worker/tests/
└── bench_binary.rs             (CREATE — binary-level latency benchmarks)

tests/e2e/
└── bench_comparison.rs         (CREATE — Rust vs npm vs SSH comparison)

docs/benchmarks/
├── README.md                   (CREATE — how to reproduce benchmarks)
└── *.json                      (OUTPUT — benchmark results)
```

### Phase F — Interactive Features
```
crates/mxdx-worker/src/
├── lib.rs                      (MODIFY — interactive session handling in event loop)
└── webrtc.rs                   (MODIFY — basic TURN relay support)

crates/mxdx-client/src/
├── main.rs                     (MODIFY — attach command wiring)
└── attach.rs                   (MODIFY — terminal attach via Matrix events)

tests/e2e/
└── interactive.rs              (CREATE — interactive session E2E tests)
```

---

## Phase A: Worker Matrix Integration

The worker must: login to Matrix, find/create its room, sync for incoming events, claim tasks, execute commands via tmux, post output/heartbeats/results as threaded events, and update state events.

### Task A1: Add credentials to worker config and CLI

**Files:**
- Modify: `crates/mxdx-worker/src/config.rs`
- Modify: `crates/mxdx-worker/src/main.rs`

The worker needs homeserver URL, username, and password to log in. These come from config file or CLI flags.

- [ ] **Step 1: Write failing test — config loads account credentials**

```rust
// In config.rs tests
#[test]
fn config_includes_homeserver_and_credentials() {
    let toml_str = r#"
[defaults]
[[defaults.accounts]]
user_id = "@worker:example.com"
homeserver = "https://example.com"

[worker]
"#;
    // Parse and verify defaults.accounts[0].homeserver and user_id are accessible
    let defaults: DefaultsConfig = toml::from_str(&toml_str.replace("[defaults]\n", "").replace("[worker]\n", "")).unwrap();
    assert_eq!(defaults.accounts[0].homeserver, "https://example.com");
}
```

- [ ] **Step 2: Run test to verify it compiles and the config structure works**

Run: `cargo test -p mxdx-worker config::tests::config_includes_homeserver_and_credentials`

- [ ] **Step 3: Add --homeserver, --username, --password CLI flags to main.rs**

Add to the `Start` variant in `Commands`:
```rust
/// Matrix homeserver URL
#[arg(long, env = "MXDX_HOMESERVER")]
homeserver: Option<String>,

/// Matrix username
#[arg(long, env = "MXDX_USERNAME")]
username: Option<String>,

/// Matrix password
#[arg(long, env = "MXDX_PASSWORD")]
password: Option<String>,
```

Update `WorkerArgs` to include these fields. Update `WorkerRuntimeConfig` to resolve credentials from CLI > env > config file.

- [ ] **Step 4: Verify worker binary accepts new flags**

Run: `cargo build -p mxdx-worker && ./target/debug/mxdx-worker start --help`
Expected: Shows --homeserver, --username, --password flags

- [ ] **Step 5: Commit**

```bash
git add crates/mxdx-worker/src/config.rs crates/mxdx-worker/src/main.rs
git commit -m "feat(worker): add Matrix credential flags to CLI and config"
```

---

### Task A2: Implement WorkerRoomOps for MatrixClient

**Files:**
- Modify: `crates/mxdx-worker/src/matrix.rs`

This is the core integration: implementing the `WorkerRoomOps` trait using `mxdx_matrix::MatrixClient`.

- [ ] **Step 1: Write failing test — MatrixWorkerRoom implements WorkerRoomOps**

```rust
#[cfg(test)]
mod integration_tests {
    use super::*;

    // Compile-time check that MatrixWorkerRoom implements WorkerRoomOps
    fn _assert_implements_trait<T: WorkerRoomOps>() {}

    #[test]
    fn matrix_worker_room_implements_worker_room_ops() {
        _assert_implements_trait::<MatrixWorkerRoom>();
    }
}
```

- [ ] **Step 2: Run test to verify it fails (MatrixWorkerRoom doesn't exist yet)**

Run: `cargo test -p mxdx-worker matrix::integration_tests::matrix_worker_room_implements_worker_room_ops`
Expected: FAIL — `MatrixWorkerRoom` not found

- [ ] **Step 3: Implement MatrixWorkerRoom**

```rust
use mxdx_matrix::MatrixClient;

pub struct MatrixWorkerRoom {
    client: MatrixClient,
    room_id: mxdx_matrix::OwnedRoomId,
}

impl MatrixWorkerRoom {
    pub fn new(client: MatrixClient, room_id: mxdx_matrix::OwnedRoomId) -> Self {
        Self { client, room_id }
    }

    pub fn room_id(&self) -> &mxdx_matrix::RoomId {
        &self.room_id
    }

    pub fn client(&self) -> &MatrixClient {
        &self.client
    }
}

impl WorkerRoomOps for MatrixWorkerRoom {
    async fn get_or_create_room(&self, room_name: &str) -> Result<String> {
        // Use client.get_or_create_launcher_space() or create_encrypted_room()
        // For the unified session model, we use a single encrypted room
        // named after the worker's room_name config
        Ok(self.room_id.to_string())
    }

    async fn post_to_thread(
        &self,
        room_id: &str,
        thread_root: &str,
        event_type: &str,
        content: serde_json::Value,
    ) -> Result<String> {
        let rid = mxdx_matrix::RoomId::parse(room_id)?;
        self.client
            .send_threaded_event(&rid, event_type, thread_root, content)
            .await
            .map_err(Into::into)
    }

    async fn write_state(
        &self,
        room_id: &str,
        event_type: &str,
        state_key: &str,
        content: serde_json::Value,
    ) -> Result<()> {
        let rid = mxdx_matrix::RoomId::parse(room_id)?;
        self.client
            .send_state_event(&rid, event_type, state_key, content)
            .await
            .map_err(Into::into)
    }

    async fn read_state(
        &self,
        room_id: &str,
        event_type: &str,
        state_key: &str,
    ) -> Result<Option<serde_json::Value>> {
        let rid = mxdx_matrix::RoomId::parse(room_id)?;
        match self.client.get_room_state_event(&rid, event_type, state_key).await {
            Ok(val) => Ok(Some(val)),
            Err(_) => Ok(None),
        }
    }

    async fn remove_state(
        &self,
        room_id: &str,
        event_type: &str,
        state_key: &str,
    ) -> Result<()> {
        let rid = mxdx_matrix::RoomId::parse(room_id)?;
        self.client
            .send_state_event(&rid, event_type, state_key, serde_json::json!({}))
            .await
            .map_err(Into::into)
    }
}
```

- [ ] **Step 4: Run the compile-time trait check test**

Run: `cargo test -p mxdx-worker matrix::integration_tests::matrix_worker_room_implements_worker_room_ops`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/mxdx-worker/src/matrix.rs
git commit -m "feat(worker): implement WorkerRoomOps for MatrixClient"
```

---

### Task A3: Implement worker login and room setup

**Files:**
- Modify: `crates/mxdx-worker/src/lib.rs`

Add a `connect()` function that logs into Matrix and sets up the worker room.

- [ ] **Step 1: Write failing integration test — worker can login and create room**

```rust
// In crates/mxdx-worker/tests/integration.rs (or new file)
#[tokio::test]
#[ignore] // Requires TuwunelInstance
async fn worker_login_and_room_setup() {
    let tuwunel = mxdx_test_helpers::TuwunelInstance::start().await.unwrap();
    let base_url = format!("http://127.0.0.1:{}", tuwunel.port);

    let config = /* build WorkerRuntimeConfig with tuwunel credentials */;
    let room = mxdx_worker::connect(&config).await.unwrap();

    assert!(!room.room_id().is_empty());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p mxdx-worker --test integration worker_login_and_room_setup -- --ignored`
Expected: FAIL — `connect` function doesn't exist

- [ ] **Step 3: Implement connect() in lib.rs**

```rust
use matrix::MatrixWorkerRoom;

/// Connect to Matrix and set up the worker's room.
/// Returns a MatrixWorkerRoom ready for the event loop.
pub async fn connect(config: &WorkerRuntimeConfig) -> Result<MatrixWorkerRoom> {
    let account = config.defaults.accounts.first()
        .ok_or_else(|| anyhow::anyhow!("no account configured"))?;

    let client = mxdx_matrix::MatrixClient::login_and_connect(
        &account.homeserver,
        &config.credentials.username,
        &config.credentials.password,
    ).await?;

    tracing::info!(user_id = %client.user_id(), "logged in to Matrix");

    // Create or find the worker's encrypted room
    let topology = client.get_or_create_launcher_space(&config.resolved_room_name).await?;
    let room_id = topology.exec_room_id;

    tracing::info!(room_id = %room_id, "worker room ready");

    Ok(MatrixWorkerRoom::new(client, room_id))
}
```

- [ ] **Step 4: Run integration test**

Run: `cargo test -p mxdx-worker --test integration worker_login_and_room_setup -- --ignored`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/mxdx-worker/src/lib.rs crates/mxdx-worker/src/config.rs
git commit -m "feat(worker): implement Matrix login and room setup"
```

---

### Task A4: Implement worker event loop

**Files:**
- Modify: `crates/mxdx-worker/src/lib.rs`

This is the core: `run_worker()` must enter a sync loop, watch for incoming `SessionTask` events, claim tasks, execute commands, stream output, post heartbeats, and write results.

- [ ] **Step 1: Write failing integration test — worker processes a task end-to-end**

```rust
#[tokio::test]
#[ignore]
async fn worker_processes_echo_task() {
    let tuwunel = TuwunelInstance::start().await.unwrap();
    let base_url = format!("http://127.0.0.1:{}", tuwunel.port);

    // Register worker and client users
    let worker_mc = MatrixClient::register_and_connect(&base_url, "worker", "pass", "mxdx-test-token").await.unwrap();
    let client_mc = MatrixClient::register_and_connect(&base_url, "client", "pass", "mxdx-test-token").await.unwrap();

    // Setup encrypted room
    let room_id = client_mc.create_encrypted_room(&[worker_mc.user_id().to_owned()]).await.unwrap();
    worker_mc.sync_once().await.unwrap();
    worker_mc.join_room(&room_id).await.unwrap();
    // Exchange keys
    for _ in 0..4 {
        client_mc.sync_once().await.unwrap();
        worker_mc.sync_once().await.unwrap();
    }

    // Submit a task
    let task = SessionTask {
        uuid: uuid::Uuid::new_v4().to_string(),
        sender_id: client_mc.user_id().to_string(),
        bin: "/bin/echo".to_string(),
        args: vec!["hello".to_string(), "world".to_string()],
        env: None,
        cwd: None,
        interactive: false,
        no_room_output: false,
        timeout_seconds: Some(30),
        heartbeat_interval_seconds: 30,
        plan: None,
        required_capabilities: vec![],
        routing_mode: None,
        on_timeout: None,
        on_heartbeat_miss: None,
    };
    let task_event_id = client_mc.send_event(&room_id, serde_json::json!({
        "type": "org.mxdx.session.task",
        "content": serde_json::to_value(&task).unwrap(),
    })).await.unwrap();

    // Run worker event loop for one cycle
    // ... (use the library's process_incoming_events or similar)

    // Verify: active state event exists, output event posted, result event posted
    // Read state events and thread events
}
```

- [ ] **Step 2: Implement the event loop in run_worker()**

The event loop structure:

```rust
pub async fn run_worker(config: WorkerRuntimeConfig) -> Result<()> {
    // ... existing init code ...

    // Connect to Matrix
    let room = connect(&config).await?;
    let room_id_str = room.room_id().to_string();

    // Post WorkerInfo state event
    let info = telemetry.collect_info()?;
    room.write_state(
        &room_id_str,
        "org.mxdx.worker.info",
        &format!("worker/{}", identity.device_id()),
        serde_json::to_value(&info)?,
    ).await?;

    // Main sync loop
    loop {
        // Sync and collect events
        let events = room.client().sync_and_collect_events(
            room.room_id(),
            Duration::from_secs(30),
        ).await?;

        for event in events {
            let event_type = event.get("type").and_then(|t| t.as_str());
            match event_type {
                Some("org.mxdx.session.task") => {
                    let content = event.get("content").cloned().unwrap_or_default();
                    let task: SessionTask = serde_json::from_value(content)?;

                    // Validate command
                    let validated = executor::validate_command(
                        &task.bin,
                        &task.args,
                        task.env.as_ref(),
                        task.cwd.as_deref(),
                    )?;

                    // Claim session
                    let active_state = session_manager.claim(task.clone())?;
                    room.write_state(
                        &room_id_str,
                        "org.mxdx.session.active",
                        &format!("session/{}/active", task.uuid),
                        serde_json::to_value(&active_state)?,
                    ).await?;

                    // Post SessionStart
                    let event_id = event.get("event_id").and_then(|e| e.as_str()).unwrap();
                    let start = SessionStart { /* ... */ };
                    room.post_to_thread(
                        &room_id_str,
                        event_id,
                        "org.mxdx.session.start",
                        serde_json::to_value(&start)?,
                    ).await?;

                    // Execute via tmux
                    let tmux = TmuxSession::create(
                        &task.uuid, &validated.bin, &validated.args,
                        validated.cwd.as_deref(), &validated.env,
                    ).await?;
                    session_manager.mark_running(&task.uuid, None, tmux)?;

                    // Spawn output streaming task
                    // ... tokio::spawn for output capture + posting
                }
                // Handle cancel, input, resize, signal events
                _ => {}
            }
        }

        // Post heartbeats for active sessions
        // Check for completed sessions (tmux exited)
        // Run retention sweep
    }
}
```

- [ ] **Step 3: Implement output streaming as a spawned task**

For each running session, spawn a tokio task that:
1. Polls `tmux.capture_pane()` at the batch window interval
2. Diffs against previous capture to get new output
3. Posts `SessionOutput` events to the thread
4. Detects session completion (tmux is_alive() == false)
5. Posts `SessionResult` and `CompletedSessionState`

- [ ] **Step 4: Run integration test**

Run: `cargo test -p mxdx-worker --test integration worker_processes_echo_task -- --ignored`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/mxdx-worker/src/lib.rs
git commit -m "feat(worker): implement Matrix event loop with task processing"
```

---

### Task A5: Handle session completion and state transitions

**Files:**
- Modify: `crates/mxdx-worker/src/lib.rs`

When a tmux session exits, the worker must:
1. Capture final output
2. Post `SessionResult` to thread
3. Write `CompletedSessionState` state event
4. Remove `ActiveSessionState` state event

- [ ] **Step 1: Write failing test — completed session has result and state events**

Test that after a short command (`echo hello`) completes:
- Thread contains SessionResult with exit_code 0
- State event `session/{uuid}/completed` exists
- State event `session/{uuid}/active` is removed (empty content)

- [ ] **Step 2: Implement completion detection in the event loop**

Add a periodic check (every 500ms) for sessions where `tmux.is_alive()` returns false.
For each completed session:
```rust
// Capture final output
let final_output = tmux.capture_pane().await?;
// Post any remaining output
// ...

// Get exit status (from tmux exit code or process exit)
let completed = session_manager.complete(&uuid, SessionStatus::Success, Some(exit_code))?;

// Post SessionResult to thread
room.post_to_thread(&room_id_str, &thread_root, SESSION_RESULT, serde_json::to_value(&SessionResult {
    session_uuid: uuid.clone(),
    worker_id: identity.device_id().to_string(),
    status: SessionStatus::Success,
    exit_code: Some(exit_code),
    duration_seconds: completed.duration_seconds,
    error: None,
})?).await?;

// Write completed state, remove active state
room.write_state(&room_id_str, "org.mxdx.session.completed", &format!("session/{uuid}/completed"), serde_json::to_value(&completed)?).await?;
room.remove_state(&room_id_str, "org.mxdx.session.active", &format!("session/{uuid}/active")).await?;
```

- [ ] **Step 3: Run test**

Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/mxdx-worker/src/lib.rs
git commit -m "feat(worker): handle session completion with state transitions"
```

---

### Task A6: Handle cancel and signal events

**Files:**
- Modify: `crates/mxdx-worker/src/lib.rs`

- [ ] **Step 1: Write failing test — cancel kills running session**

- [ ] **Step 2: Add cancel/signal handling to event loop**

Match on `org.mxdx.session.cancel` events in the sync loop. Kill the tmux session and post SessionResult with `SessionStatus::Cancelled`.

- [ ] **Step 3: Run test**

- [ ] **Step 4: Commit**

```bash
git add crates/mxdx-worker/src/lib.rs
git commit -m "feat(worker): handle cancel and signal events"
```

---

## Phase B: Client Matrix Integration

The client CLI must: login to Matrix, find the worker room, submit tasks, tail output threads, read state events, cancel sessions.

### Task B1: Add credentials to client config and CLI

**Files:**
- Modify: `crates/mxdx-client/src/config.rs`
- Modify: `crates/mxdx-client/src/main.rs`

Same pattern as Task A1 — add --homeserver, --username, --password, --worker-room flags.

- [ ] **Step 1: Add credential fields to ClientRuntimeConfig**

- [ ] **Step 2: Add CLI flags with env var fallbacks**

```rust
/// Global options (before subcommand)
#[derive(Parser)]
struct Cli {
    #[arg(long, env = "MXDX_HOMESERVER", global = true)]
    homeserver: Option<String>,

    #[arg(long, env = "MXDX_USERNAME", global = true)]
    username: Option<String>,

    #[arg(long, env = "MXDX_PASSWORD", global = true)]
    password: Option<String>,

    #[command(subcommand)]
    command: Commands,
}
```

- [ ] **Step 3: Verify mxdx-client --help shows credential flags**

Run: `cargo build -p mxdx-client && ./target/debug/mxdx-client --help`

- [ ] **Step 4: Commit**

```bash
git add crates/mxdx-client/src/config.rs crates/mxdx-client/src/main.rs
git commit -m "feat(client): add Matrix credential flags to CLI and config"
```

---

### Task B2: Implement ClientRoomOps for MatrixClient

**Files:**
- Modify: `crates/mxdx-client/src/matrix.rs`

- [ ] **Step 1: Write compile-time trait check test**

- [ ] **Step 2: Implement MatrixClientRoom**

```rust
pub struct MatrixClientRoom {
    client: MatrixClient,
    room_id: OwnedRoomId,
}

impl ClientRoomOps for MatrixClientRoom {
    // Delegate to MatrixClient methods
}
```

- [ ] **Step 3: Add connect() function**

```rust
/// Connect to Matrix and find the target worker room.
pub async fn connect(
    homeserver: &str,
    username: &str,
    password: &str,
    worker_room: &str,
) -> Result<MatrixClientRoom> {
    let client = MatrixClient::login_and_connect(homeserver, username, password).await?;
    let topology = client.find_launcher_space(worker_room).await?
        .ok_or_else(|| anyhow::anyhow!("worker room '{}' not found", worker_room))?;
    Ok(MatrixClientRoom::new(client, topology.exec_room_id))
}
```

- [ ] **Step 4: Run tests**

- [ ] **Step 5: Commit**

```bash
git add crates/mxdx-client/src/matrix.rs
git commit -m "feat(client): implement ClientRoomOps for MatrixClient"
```

---

### Task B3: Wire `run` command — submit task and tail output

**Files:**
- Modify: `crates/mxdx-client/src/main.rs`

This is the most important client command. It must:
1. Connect to Matrix
2. Build SessionTask
3. Post task event to room
4. Enter sync loop watching thread for SessionOutput and SessionResult
5. Print output to stdout as it arrives
6. Exit with the session's exit code

- [ ] **Step 1: Write failing E2E test (binary subprocess)**

```rust
#[tokio::test]
#[ignore]
async fn client_run_echo_returns_output() {
    let tuwunel = TuwunelInstance::start().await.unwrap();
    // Start worker binary as subprocess
    let mut worker = Command::new(cargo_bin("mxdx-worker"))
        .args(["start", "--homeserver", &base_url, "--username", "worker", "--password", "pass"])
        .spawn().unwrap();

    // Wait for worker to be ready (poll WorkerInfo state event)
    tokio::time::sleep(Duration::from_secs(5)).await;

    // Run client binary
    let output = Command::new(cargo_bin("mxdx-client"))
        .args(["--homeserver", &base_url, "--username", "client", "--password", "pass",
               "run", "/bin/echo", "hello", "world"])
        .output().unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("hello world"));

    worker.kill().unwrap();
}
```

- [ ] **Step 2: Implement the `run` command flow in main.rs**

```rust
Commands::Run { command, args, detach, interactive, no_room_output, timeout, worker_room } => {
    // Connect to Matrix
    let room = crate::matrix::connect(
        &config.credentials.homeserver,
        &config.credentials.username,
        &config.credentials.password,
        &worker_room.unwrap_or(config.client.default_worker_room.clone()),
    ).await?;

    // Build task
    let task = submit::build_task(&command, &args, interactive, no_room_output, timeout, config.client.session.heartbeat_interval, &room.client().user_id().to_string());

    // Submit task to room
    let task_event_id = room.client().send_event(
        room.room_id(),
        serde_json::json!({
            "type": SESSION_TASK,
            "content": serde_json::to_value(&task)?,
        }),
    ).await?;

    if detach {
        println!("{}", task.uuid);
        return Ok(());
    }

    // Tail thread for output and result
    loop {
        let events = room.client().sync_and_collect_events(room.room_id(), Duration::from_secs(30)).await?;
        for event in events {
            let event_type = event.get("type").and_then(|t| t.as_str());
            let content = event.get("content");
            // Check if event is in our thread (relates_to.event_id == task_event_id)
            match event_type {
                Some(SESSION_OUTPUT) => {
                    // Decode and print
                    if let Some(c) = content {
                        let output: SessionOutput = serde_json::from_value(c.clone())?;
                        if output.session_uuid == task.uuid {
                            let decoded = tail::decode_output(&output.data)?;
                            print!("{}", decoded);
                        }
                    }
                }
                Some(SESSION_RESULT) => {
                    if let Some(c) = content {
                        let result: SessionResult = serde_json::from_value(c.clone())?;
                        if result.session_uuid == task.uuid {
                            std::process::exit(result.exit_code.unwrap_or(1));
                        }
                    }
                }
                _ => {}
            }
        }
    }
}
```

- [ ] **Step 3: Run E2E test**

Run: `cargo test --test e2e_binary client_run_echo_returns_output -- --ignored`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/mxdx-client/src/main.rs
git commit -m "feat(client): wire run command to Matrix with output tailing"
```

---

### Task B4: Wire `ls` command — read state events

**Files:**
- Modify: `crates/mxdx-client/src/main.rs`

- [ ] **Step 1: Write failing E2E test**

```rust
#[tokio::test]
#[ignore]
async fn client_ls_shows_active_sessions() {
    // Start worker, submit a long-running task, then run `mxdx-client ls`
    // Verify output contains the session UUID and status
}
```

- [ ] **Step 2: Implement ls command**

```rust
Commands::Ls { all } => {
    let room = connect(/* ... */).await?;
    room.client().sync_once().await?;

    // Read active state events
    let active_states = room.client().get_room_state(room.room_id(), "org.mxdx.session.active").await?;
    let entries: Vec<SessionEntry> = /* parse active states into SessionEntry via ls::from_active */;

    if all {
        let completed_states = room.client().get_room_state(room.room_id(), "org.mxdx.session.completed").await?;
        // Add completed entries
    }

    println!("{}", ls::format_table(&entries));
}
```

- [ ] **Step 3: Run test, commit**

---

### Task B5: Wire `logs` command — fetch thread history

**Files:**
- Modify: `crates/mxdx-client/src/main.rs`

- [ ] **Step 1: Write failing E2E test**

- [ ] **Step 2: Implement logs command**

Fetch all events in the session's thread, filter for SessionOutput, reassemble using `logs::reassemble_output()`, print.

- [ ] **Step 3: Run test, commit**

---

### Task B6: Wire `cancel` command — post cancel event

**Files:**
- Modify: `crates/mxdx-client/src/main.rs`

- [ ] **Step 1: Write failing E2E test**

Start a `sleep 300` session, run `mxdx-client cancel <uuid>`, verify session terminates.

- [ ] **Step 2: Implement cancel command**

Post `SessionCancel` event to thread, wait for `SessionResult` confirmation.

- [ ] **Step 3: Run test, commit**

---

## Phase C: Real E2E Tests (Binary Subprocess)

These tests spawn `mxdx-worker` and `mxdx-client` as actual subprocesses and verify behavior from the outside — exactly as a user would experience it.

### Task C1: Create E2E test harness

**Files:**
- Create: `tests/e2e/mod.rs`
- Create: `tests/e2e/helpers.rs`

- [ ] **Step 1: Create test harness with binary spawning helpers**

```rust
use std::process::{Command, Child, Stdio};
use std::time::Duration;

/// Path to compiled mxdx-worker binary
pub fn worker_bin() -> std::path::PathBuf {
    cargo_bin("mxdx-worker")
}

/// Path to compiled mxdx-client binary
pub fn client_bin() -> std::path::PathBuf {
    cargo_bin("mxdx-client")
}

/// Path to compiled binary (uses cargo's target directory)
fn cargo_bin(name: &str) -> std::path::PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop(); // Remove test binary name
    path.pop(); // Remove 'deps'
    path.push(name);
    path
}

/// Start a worker subprocess connected to a TuwunelInstance
pub fn start_worker(
    homeserver: &str,
    username: &str,
    password: &str,
    room_name: &str,
) -> Child {
    Command::new(worker_bin())
        .args(["start",
            "--homeserver", homeserver,
            "--username", username,
            "--password", password,
            "--room-name", room_name,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to start mxdx-worker")
}

/// Run a client command and capture output
pub fn run_client(
    homeserver: &str,
    username: &str,
    password: &str,
    args: &[&str],
) -> std::process::Output {
    Command::new(client_bin())
        .args(["--homeserver", homeserver,
               "--username", username,
               "--password", password])
        .args(args)
        .output()
        .expect("failed to run mxdx-client")
}

/// Wait for worker to be ready by polling for WorkerInfo state event
pub async fn wait_for_worker_ready(
    client: &mxdx_matrix::MatrixClient,
    room_id: &mxdx_matrix::RoomId,
    timeout: Duration,
) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        client.sync_once().await.ok();
        if let Ok(state) = client.get_room_state(room_id, "org.mxdx.worker.info").await {
            if !state.is_null() {
                return true;
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    false
}
```

- [ ] **Step 2: Commit**

```bash
git add tests/e2e/
git commit -m "test: create E2E test harness for binary subprocess testing"
```

---

### Task C2: E2E test — full lifecycle with local Tuwunel

**Files:**
- Create: `tests/e2e/full_lifecycle.rs`

- [ ] **Step 1: Write test — echo command lifecycle**

```rust
#[tokio::test]
#[ignore] // Requires tuwunel binary
async fn e2e_echo_command_lifecycle() {
    // 1. Start TuwunelInstance
    // 2. Register worker + client accounts
    // 3. Build binaries: cargo build -p mxdx-worker -p mxdx-client
    // 4. Start worker subprocess
    // 5. Wait for worker ready (WorkerInfo state event)
    // 6. Run client: mxdx-client run /bin/echo hello world
    // 7. Assert stdout contains "hello world"
    // 8. Assert exit code is 0
    // 9. Run client: mxdx-client ls
    // 10. Assert output shows completed session
    // 11. Kill worker
}
```

- [ ] **Step 2: Run test**

Run: `cargo build -p mxdx-worker -p mxdx-client && cargo test --test full_lifecycle e2e_echo -- --ignored`

If test fails: fix the binary, NOT the test. Iterate until passing.

- [ ] **Step 3: Write test — long-running command with cancel**

```rust
#[tokio::test]
#[ignore]
async fn e2e_cancel_running_session() {
    // 1. Start tuwunel + worker
    // 2. Client: run --detach sleep 300  → get UUID
    // 3. Client: ls → verify session is active
    // 4. Client: cancel <uuid>
    // 5. Client: ls → verify session is completed/cancelled
    // 6. Kill worker
}
```

- [ ] **Step 4: Write test — multiple concurrent sessions**

```rust
#[tokio::test]
#[ignore]
async fn e2e_concurrent_sessions() {
    // 1. Start tuwunel + worker
    // 2. Client: run --detach sleep 300
    // 3. Client: run --detach sleep 300
    // 4. Client: ls → verify 2 active sessions
    // 5. Cancel both
    // 6. Client: ls --all → verify 2 completed
}
```

- [ ] **Step 5: Write test — command with non-zero exit code**

```rust
#[tokio::test]
#[ignore]
async fn e2e_nonzero_exit_code() {
    // Client: run /bin/false
    // Assert exit code is 1
}
```

- [ ] **Step 6: Write test — command with stderr**

```rust
#[tokio::test]
#[ignore]
async fn e2e_stderr_output() {
    // Client: run /bin/sh -c "echo err >&2"
    // Assert stderr content is captured
}
```

- [ ] **Step 7: Commit**

```bash
git add tests/e2e/full_lifecycle.rs
git commit -m "test: add real E2E tests spawning worker/client binaries"
```

---

### Task C3: E2E tests — beta server (single + federated)

**Files:**
- Create: `tests/e2e/beta_server.rs`

These tests use the real beta servers from `test-credentials.toml`. They prove the binaries work against production-like Matrix infrastructure.

- [ ] **Step 1: Write test — single server lifecycle on ca1-beta**

```rust
#[tokio::test]
#[ignore] // Requires test-credentials.toml
async fn e2e_beta_single_server_echo() {
    let creds = load_test_credentials().expect("test-credentials.toml required");

    // Start worker binary pointing at ca1-beta.mxdx.dev
    let mut worker = start_worker(
        &creds.server.url,
        &creds.account1.username,
        &creds.account1.password,
        "e2e-test-room",
    );

    // Wait for ready
    // ...

    // Run client with account2
    let output = run_client(
        &creds.server.url,
        &creds.account2.username,
        &creds.account2.password,
        &["run", "/bin/echo", "beta", "test"],
    );

    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("beta test"));

    worker.kill().unwrap();
}
```

- [ ] **Step 2: Write test — cross-server federation**

```rust
#[tokio::test]
#[ignore]
async fn e2e_beta_federated_echo() {
    let creds = load_test_credentials().expect("test-credentials.toml required");

    // Worker on ca1-beta (account1)
    let mut worker = start_worker(
        &creds.server.url,  // ca1-beta
        &creds.account1.username,
        &creds.account1.password,
        "e2e-federation-test",
    );

    // Client on ca2-beta (account2) — different server!
    let output = run_client(
        &creds.server2.url,  // ca2-beta
        &creds.account2.username,
        &creds.account2.password,
        &["run", "--worker-room", "e2e-federation-test", "/bin/echo", "federated"],
    );

    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("federated"));

    worker.kill().unwrap();
}
```

- [ ] **Step 3: Commit**

```bash
git add tests/e2e/beta_server.rs
git commit -m "test: add beta server E2E tests (single + federated)"
```

---

### Task C4: Rename existing tests to reflect their true nature

**Files:**
- Modify: `crates/mxdx-worker/tests/e2e_session.rs` → rename to `integration_session.rs`
- Modify: `crates/mxdx-client/tests/e2e_session.rs` → rename to `integration_session.rs`

- [ ] **Step 1: Rename files and update test module names**

```bash
mv crates/mxdx-worker/tests/e2e_session.rs crates/mxdx-worker/tests/integration_session.rs
mv crates/mxdx-client/tests/e2e_session.rs crates/mxdx-client/tests/integration_session.rs
```

Add a comment at the top of each:
```rust
//! Integration tests for session event types through real Matrix servers.
//! These test the event schema and Matrix round-tripping, NOT the compiled binaries.
//! For true E2E tests that exercise the binaries, see tests/e2e/.
```

- [ ] **Step 2: Remove profiling from integration tests**

Move benchmark/profiling code out of integration tests — profiling only belongs on binary-level tests.

- [ ] **Step 3: Commit**

```bash
git add crates/mxdx-worker/tests/ crates/mxdx-client/tests/
git commit -m "refactor: rename e2e_session tests to integration_session (they test libraries, not binaries)"
```

---

## Phase D: npm/WASM Integration

Wire up the JS client commands to use the WASM bindings and Matrix connection from `@mxdx/core`.

### Task D1: Wire `run.js` to submit tasks via Matrix

**Files:**
- Modify: `packages/client/src/run.js`

- [ ] **Step 1: Implement run command using connectWithSession() and WASM**

```javascript
import { connectWithSession, createSessionTask, getSessionEventTypes } from '@mxdx/core';

export async function runCommand(opts) {
    const { client } = await connectWithSession(opts);
    const eventTypes = getSessionEventTypes();

    const task = createSessionTask({
        bin: opts.command,
        args: opts.args || [],
        interactive: opts.interactive || false,
        noRoomOutput: opts.noRoomOutput || false,
        timeoutSeconds: opts.timeout || null,
    });

    // Find worker room
    const room = await findWorkerRoom(client, opts.workerRoom);

    // Submit task
    const taskEventId = await client.sendEvent(room.execRoomId, eventTypes.SESSION_TASK, task);

    if (opts.detach) {
        console.log(task.uuid);
        return;
    }

    // Tail thread for output
    await tailSessionThread(client, room.execRoomId, taskEventId, task.uuid);
}
```

- [ ] **Step 2: Write E2E test using npm binary**

```bash
# Test that `npx mxdx run echo hello` produces correct output
# Against a local TuwunelInstance
```

- [ ] **Step 3: Commit**

---

### Task D2: Wire `ls.js`, `logs.js`, `cancel.js`

**Files:**
- Modify: `packages/client/src/ls.js`
- Modify: `packages/client/src/logs.js`
- Modify: `packages/client/src/cancel.js`

- [ ] **Step 1: Implement ls — read state events, format table**
- [ ] **Step 2: Implement logs — fetch thread history, reassemble**
- [ ] **Step 3: Implement cancel — post cancel event**
- [ ] **Step 4: Write E2E tests for each**
- [ ] **Step 5: Commit**

---

### Task D3: Migrate launcher to unified session events

**Files:**
- Modify: `packages/launcher/src/runtime.js`

The launcher currently listens for `org.mxdx.command` events. It needs to also handle `org.mxdx.session.task` events.

- [ ] **Step 1: Add session task handler alongside existing command handler**

Support both old `org.mxdx.command` (backward compat) and new `org.mxdx.session.task` format.

- [ ] **Step 2: Post session lifecycle events (start, output, heartbeat, result)**

Use the WASM helpers to create properly typed events.

- [ ] **Step 3: Write state events for session tracking**

- [ ] **Step 4: Test with both old client (org.mxdx.command) and new client (org.mxdx.session.task)**

- [ ] **Step 5: Commit**

---

## Phase E: Performance Benchmarks

Benchmark the actual binaries, not library code. Compare Rust binaries, npm+WASM, and SSH.

### Task E1: Binary-level latency benchmarks (local)

**Files:**
- Create: `tests/e2e/bench_binary.rs`

- [ ] **Step 1: Write benchmark — Rust binary echo latency**

Measure wall-clock time from `mxdx-client run /bin/echo test` to exit. Run 10 iterations, report min/max/p50/p95/p99.

```rust
#[tokio::test]
#[ignore]
async fn bench_rust_binary_echo_latency() {
    let tuwunel = TuwunelInstance::start().await.unwrap();
    // Register accounts, start worker
    // ...

    let mut latencies = vec![];
    for _ in 0..10 {
        let start = Instant::now();
        let output = run_client(&base_url, "client", "pass", &["run", "/bin/echo", "bench"]);
        let elapsed = start.elapsed();
        assert!(output.status.success());
        latencies.push(elapsed.as_millis() as f64);
    }

    let stats = compute_stats(&latencies);
    let report = BenchmarkReport {
        name: "rust-binary-echo-local",
        transport: "matrix-local",
        samples: latencies.len(),
        min_ms: stats.min,
        max_ms: stats.max,
        p50_ms: stats.p50,
        p95_ms: stats.p95,
        p99_ms: stats.p99,
        mean_ms: stats.mean,
    };

    save_benchmark_report(&report, "docs/benchmarks/");
    // Kill worker
}
```

- [ ] **Step 2: Write benchmark — Rust binary session lifecycle latency**

Measure: submit task → receive result (full lifecycle including tmux setup).

- [ ] **Step 3: Write benchmark — Rust binary ls latency**

Measure: `mxdx-client ls` wall-clock time.

- [ ] **Step 4: Commit**

```bash
git add tests/e2e/bench_binary.rs
git commit -m "test: add binary-level latency benchmarks (local)"
```

---

### Task E2: npm/WASM latency benchmarks

**Files:**
- Create: `tests/e2e/bench_npm.js` (or `packages/e2e-tests/bench_npm.test.js`)

- [ ] **Step 1: Write benchmark — npm client echo latency**

```javascript
// Measure: `npx mxdx run /bin/echo test` wall-clock time
// Same pattern as Rust benchmark
```

- [ ] **Step 2: Write benchmark — npm client ls latency**

- [ ] **Step 3: Commit**

---

### Task E3: SSH baseline benchmarks

**Files:**
- Modify: existing `bench_latency.rs` or create `tests/e2e/bench_ssh.rs`

- [ ] **Step 1: Write benchmark — SSH echo latency (binary-level comparison)**

```rust
// ssh -i ~/.ssh/id_ed25519_mxdx_test localhost echo test
// Measure wall-clock time for comparison with mxdx
```

- [ ] **Step 2: Commit**

---

### Task E4: Beta server benchmarks

**Files:**
- Create: `tests/e2e/bench_beta.rs`

- [ ] **Step 1: Benchmark — single server binary latency on ca1-beta**

Start worker binary on ca1-beta, run client binary, measure latency.

- [ ] **Step 2: Benchmark — federated binary latency (worker ca1, client ca2)**

- [ ] **Step 3: Write benchmark comparison report**

Auto-generate a markdown comparison table:
```markdown
| Transport | Echo Latency (p50) | Lifecycle (p50) | Throughput |
|-----------|-------------------|-----------------|------------|
| SSH localhost | X ms | N/A | Y ops/s |
| mxdx Rust local | X ms | X ms | Y ops/s |
| mxdx npm local | X ms | X ms | Y ops/s |
| mxdx Rust beta single | X ms | X ms | Y ops/s |
| mxdx Rust beta federated | X ms | X ms | Y ops/s |
```

- [ ] **Step 4: Commit**

```bash
git add tests/e2e/bench_beta.rs docs/benchmarks/
git commit -m "test: add beta server binary benchmarks with comparison report"
```

---

### Task E5: Create reproducible benchmark runner

**Files:**
- Create: `docs/benchmarks/README.md`

- [ ] **Step 1: Write benchmark documentation**

```markdown
# mxdx Performance Benchmarks

## Prerequisites
- Rust toolchain (cargo build)
- Node.js 22+ (npm)
- tmux installed
- SSH key at ~/.ssh/id_ed25519_mxdx_test (for SSH baseline)
- test-credentials.toml (for beta server benchmarks)

## Running All Benchmarks

### Local benchmarks (requires tuwunel binary)
cargo test --test bench_binary -- --ignored --nocapture

### Beta server benchmarks (requires test-credentials.toml)
cargo test --test bench_beta -- --ignored --nocapture

### npm benchmarks
npm test --workspace=packages/e2e-tests -- --grep bench

### SSH baseline
cargo test --test bench_ssh -- --ignored --nocapture

## Results
Results are saved as JSON to docs/benchmarks/ with timestamps.
Compare across runs to detect regressions.
```

- [ ] **Step 2: Commit**

---

## Phase F: Interactive Features

### Task F1: Implement interactive session in worker

**Files:**
- Modify: `crates/mxdx-worker/src/lib.rs`

When a `SessionTask` has `interactive: true`:
1. Create tmux interactive session (shell)
2. Create DM room with the client user
3. Post `SessionStart` with DM room_id for terminal I/O
4. Forward input events from DM to tmux
5. Stream tmux output to DM as terminal data events

- [ ] **Step 1: Write failing E2E test — interactive session**

```rust
#[tokio::test]
#[ignore]
async fn e2e_interactive_session() {
    // Start worker
    // Client: run --interactive /bin/sh
    // Send "echo hello\n" to stdin
    // Read "hello" from stdout
    // Send "exit\n"
    // Verify session completes
}
```

- [ ] **Step 2: Implement interactive task handling in event loop**

- [ ] **Step 3: Run test, iterate until passing**

- [ ] **Step 4: Commit**

---

### Task F2: Implement `attach` command in client

**Files:**
- Modify: `crates/mxdx-client/src/main.rs`
- Modify: `crates/mxdx-client/src/attach.rs`

- [ ] **Step 1: Write failing E2E test**

```rust
#[tokio::test]
#[ignore]
async fn e2e_attach_to_session() {
    // Start worker
    // Client: run --detach --interactive /bin/sh → UUID
    // Client: attach <uuid>
    // Send "echo attached\n"
    // Read "attached" from stdout
    // Ctrl-C to detach
}
```

- [ ] **Step 2: Implement attach — connect to DM room, pipe stdin/stdout**

- [ ] **Step 3: Run test, iterate**

- [ ] **Step 4: Commit**

---

### Task F3: Interactive session benchmarks

**Files:**
- Create: `tests/e2e/bench_interactive.rs`

- [ ] **Step 1: Benchmark — interactive keystroke latency**

Measure round-trip time: send character → receive echo.

- [ ] **Step 2: Benchmark — interactive throughput**

Measure: send large file content → receive complete output.

- [ ] **Step 3: Compare with SSH interactive latency**

- [ ] **Step 4: Commit**

---

## Execution Order and Dependencies

```
Phase A (Worker Matrix Integration)
  A1 → A2 → A3 → A4 → A5 → A6

Phase B (Client Matrix Integration) — depends on A3 (worker must be connectable)
  B1 → B2 → B3 → B4 → B5 → B6

Phase C (Real E2E Tests) — depends on A4 + B3 (both binaries must work)
  C1 → C2 → C3 → C4

Phase D (npm/WASM) — can start after B3 (same patterns as Rust client)
  D1 → D2 → D3

Phase E (Benchmarks) — depends on C2 (E2E tests prove binaries work)
  E1 → E2 → E3 → E4 → E5

Phase F (Interactive) — depends on A4 (event loop must handle interactive)
  F1 → F2 → F3
```

**Critical path:** A1 → A2 → A3 → A4 → A5 → B1 → B2 → B3 → C1 → C2 → E1

**Parallel opportunities:**
- B1 can start as soon as A2 is done (client config doesn't depend on worker event loop)
- C4 (rename tests) can happen anytime
- D1-D3 can proceed in parallel with E1-E5
- F1-F3 can proceed after A4 is stable

---

## Definition of Done

1. `mxdx-worker start` connects to Matrix, syncs, processes tasks, streams output, writes state events
2. `mxdx-client run` submits tasks and prints output in real-time, exits with correct code
3. `mxdx-client ls` shows active and completed sessions
4. `mxdx-client logs` replays session output
5. `mxdx-client cancel` terminates running sessions
6. All E2E tests spawn binaries as subprocesses — no library-level shortcuts
7. All E2E tests pass on local TuwunelInstance
8. Beta server E2E tests pass on ca1/ca2-beta.mxdx.dev (single + federated)
9. npm `run`, `ls`, `logs`, `cancel` commands work via WASM
10. npm launcher handles unified session events
11. Benchmark comparison table published: Rust binary vs npm+WASM vs SSH
12. Interactive sessions work (worker + client attach)
13. Security review: no encryption bypass, all events E2EE, credentials never logged
