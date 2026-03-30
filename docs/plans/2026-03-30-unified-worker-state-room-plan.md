# Unified Worker: State Room + Client Integration + Interactive Features + Benchmarks

## Context

**Worker and launcher are the same thing** — just aliases. The end goal is a single unified worker codebase: core logic in Rust, exposed via WASM, consumed by both the Rust binary and the npm package. JS is used only for platform-specific capabilities that WASM can't handle (xterm.js, local keystores, networking, process/PTY management, file I/O).

Four pieces of work merged into a single cohesive plan:

1. **Worker State Room**: Store all worker operational state in a private E2EE Matrix room. Implemented once in Rust, exposed via WASM.
2. **Client Integration**: Wire npm client commands (`run`, `ls`, `logs`, `cancel`) to use unified session events via WASM bindings.
3. **Worker Convergence**: Migrate the npm worker's session/room management to consume Rust `WorkerStateRoom` via WASM, replacing JS-only state tracking.
4. **Interactive Features + Benchmarks**: Interactive terminal sessions, TURN relay, and comprehensive benchmarks across all transports.

**Architecture principles**:
- Worker = launcher (aliases in both ecosystems)
- Core logic in Rust → WASM → consumed by npm. JS only for platform I/O (PTY, networking, xterm, keystores)
- Room name has automatic defaults from hostname + username — never required, even on first run
- Config hierarchy: TOML files (`~/.mxdx/worker.toml`, `defaults.toml`) → state room → keychain. Disk persistence only as fallback.

**Current state**: Rust binary has Matrix connectivity, E2EE, session restore, keychain, batched sender. npm worker has P2P/terminal handling but uses old event formats and JS-based state tracking (`sessions.json`, `session-rooms.json`, `#sessionRegistry`). The WASM crate exposes Matrix operations but not session management or state room logic.

---

## Test Strategy

**Regular suite** (beta infrastructure, pre-existing accounts from `test-credentials.toml`):
- `cargo test -p mxdx-worker --test e2e_binary_beta -- --ignored` — Rust binary E2E
- `cargo test -p mxdx-worker --test e2e_profile -- --ignored` — profiling benchmarks
- `npm test` in `packages/e2e-tests/` — npm E2E (requires WASM build)

**Local dev only** (ephemeral tuwunel, separately triggered, not part of regular suite):
- `cargo test -p mxdx-worker --test e2e_binary -- --ignored` — infrastructure/provisioning tests

**Blocking rules**: Any E2E failure or >10% profiling regression blocks advancement.

---

## Architecture: Write-Confirm Pattern + State Room Rules

### Write-Confirm Pattern (multi-homeserver)

When updating state, the worker writes to its **primary homeserver** first and waits for confirmation (event ID returned), then replicates to secondary homeservers. On startup, the worker reads from the **most recently written** state room as the starting point. This matches the client's existing pattern for in-flight requests.

```
Write flow:  primary.send_state_event() → confirmed → secondary.send_state_event()
Read flow:   load from last-confirmed homeserver → apply as starting state
```

### Room Naming (deterministic, idempotent, zero-config)

**All room names** (state room AND exec room) derive defaults from `{hostname}.{os_username}.{matrix_localpart}`. This means:
- `--room-name` is **never required**, even on first run — automatic defaults just work
- `--room-name` only needed to override the default (e.g., multiple workers on same host)
- Same host + user + account always finds the same rooms
- Idempotent across restarts

State room alias: `#mxdx-state-{hostname}.{os_user}.{localpart}:{server}`
State room topic: `org.mxdx.worker.state:{hostname}.{os_user}.{localpart}`
Exec room default: derives from same pattern via `config.rs:compute_room_name()`

### Room Trust Validation

On discovery, the worker **must reject** a state room that:
1. Was NOT created by the worker's own account or a trusted coordinator (check `m.room.create` event's `creator` field)
2. Is NOT encrypted (no `m.room.encryption` state event)

If either check fails, the worker logs an error and refuses to use the room. This prevents an attacker from pre-creating a room with the expected name/alias to inject malicious state.

### State Contents

The state room stores:
- **Running tasks**: UUID, command, args, tmux session, start time, thread root, exec room
- **Trusted clients**: Matrix user IDs that have been cross-signed / authorized
- **Trusted coordinators**: Matrix user IDs authorized to manage this worker
- **Tracked rooms**: Room IDs the worker monitors for events (exec rooms, spaces)
- **Worker config**: room name, trust anchor, capabilities
- **Worker identity**: device ID, host, os_user

---

## Architecture: Unified Worker via WASM

```
┌─────────────────────────────────────────────┐
│  Rust Core (single source of truth)          │
│  ┌──────────────┐  ┌───────────────────┐    │
│  │ WorkerState  │  │ SessionManager    │    │
│  │ Room         │  │ (state machine)   │    │
│  ├──────────────┤  ├───────────────────┤    │
│  │ create/find  │  │ claim/run/complete│    │
│  │ validate     │  │ write to state rm │    │
│  │ read/write   │  │ interactive I/O   │    │
│  │ write-confirm│  │ session mux       │    │
│  └──────────────┘  └───────────────────┘    │
│         │                    │               │
│         ▼                    ▼               │
│  ┌──────────────────────────────────────┐   │
│  │ mxdx-core-wasm (WASM bindings)      │   │
│  │ getOrCreateStateRoom()              │   │
│  │ writeSession() / readSessions()     │   │
│  │ writeRoom() / readRooms()           │   │
│  │ claimTask() / completeTask()        │   │
│  │ submitTask() / tailThread()         │   │
│  └──────────────────────────────────────┘   │
└─────────────────┬───────────────────────────┘
                  │
      ┌───────────┼───────────┐
      ▼           ▼           ▼
  Rust Binary   npm Worker    npm Client
  (mxdx-worker) (JS: PTY,     (thin shell
   = launcher)   networking,    over WASM)
                 xterm, etc.)
```

**JS layer handles only what WASM can't**: PTY/process management, networking (WebRTC), xterm.js rendering, OS keystore access, file I/O. Everything else in WASM.

The npm worker's `runtime.js` will progressively delegate to WASM:
- Phase 3: State room CRUD (replace `sessions.json` + `session-rooms.json`)
- Phase 4: Session lifecycle events (replace `#sessionRegistry` management)
- Phase 5: Interactive session mux (replace `SessionMux` class)

---

## Phase 1: State Room Infrastructure (Rust + types)

### 1a: Event types — `crates/mxdx-types/src/events/state_room.rs` (NEW)

```rust
pub const WORKER_STATE_CONFIG: &str = "org.mxdx.worker.config";
pub const WORKER_STATE_IDENTITY: &str = "org.mxdx.worker.identity";
pub const WORKER_STATE_ROOM: &str = "org.mxdx.worker.room";
pub const WORKER_STATE_SESSION: &str = "org.mxdx.worker.session";
pub const WORKER_STATE_TOPOLOGY: &str = "org.mxdx.worker.topology";
pub const WORKER_STATE_ROOM_POINTER: &str = "org.mxdx.worker.state_room";
pub const WORKER_STATE_TRUSTED_CLIENT: &str = "org.mxdx.worker.trusted_client";
pub const WORKER_STATE_TRUSTED_COORDINATOR: &str = "org.mxdx.worker.trusted_coordinator";
```

Data structs:
- `WorkerStateConfig { room_name, trust_anchor, capabilities, created_at }`
- `StateRoomSession { uuid, bin, args, tmux_session, started_at, thread_root, exec_room_id, state }`
- `StateRoomEntry { room_id, room_name, space_id, role, joined_at }`
- `TrustedEntity { user_id, verified_at, verified_by_device }`

All `Serialize`/`Deserialize`, shared between Rust and npm via WASM.

- Modify: `crates/mxdx-types/src/events/mod.rs` — add `pub mod state_room;`

### 1b: Room creation & discovery — `crates/mxdx-matrix/src/rooms.rs` (MODIFY)

- `create_worker_state_room(hostname, os_user, localpart)`:
  - Room alias: `#mxdx-state-{hostname}.{os_user}.{localpart}:{server}`
  - Topic: `org.mxdx.worker.state:{hostname}.{os_user}.{localpart}`
  - E2EE enabled, `HistoryVisibility::Joined`
  - Returns `OwnedRoomId`

- `find_worker_state_room(hostname, os_user, localpart)`:
  - Try alias lookup first (fast, O(1))
  - Fall back to topic scan if alias not found

- `validate_state_room(room_id, own_user_id, trusted_coordinators)`:
  - Check `m.room.create` → `creator` is own account or trusted coordinator
  - Check `m.room.encryption` state event exists
  - Returns `Result<(), StateRoomRejected>`

### 1c: Bulk state read — `crates/mxdx-matrix/src/client.rs` (MODIFY)

- `get_all_state_events_of_type(room_id, event_type) -> Vec<(state_key, Value)>`
- Uses `GET /_matrix/client/v3/rooms/{roomId}/state`, filters by type

### 1d: Keychain key — `crates/mxdx-types/src/identity.rs` (MODIFY)

- `state_room_key(user_id)` → `mxdx/{user_id}/state-room-id`

**Tests**: Create state room, validate trust. Reject room created by untrusted user. Reject unencrypted room. Alias-based discovery. Bulk state read.

---

## Phase 2: WorkerStateRoom Module (Rust)

### `crates/mxdx-worker/src/state_room.rs` (NEW)

```rust
pub struct WorkerStateRoom { room_id: OwnedRoomId }

impl WorkerStateRoom {
    /// Find or create + validate the state room.
    pub async fn get_or_create(client, hostname, os_user, localpart, keychain, trusted_coords) -> Result<Self>;

    // Config
    pub async fn write_config / read_config;

    // Sessions (replaces sessions.json)
    pub async fn write_session / remove_session / read_sessions;

    // Rooms being tracked
    pub async fn write_room / read_rooms;

    // Trust
    pub async fn write_trusted_client / read_trusted_clients;
    pub async fn write_trusted_coordinator / read_trusted_coordinators;

    // Topology
    pub async fn write_topology / read_topology;

    // Multi-homeserver write-confirm
    pub async fn write_state_confirmed(&self, primary, secondaries, event_type, state_key, content);

    // Coordinator discovery
    pub async fn advertise_in_exec_room(client, exec_room_id, device_id);
}
```

`get_or_create()` flow:
1. Check keychain for `mxdx/{user_id}/state-room-id`
2. If found → validate trust (creator + encryption check) → use
3. If not → try alias `#mxdx-state-{hostname}.{os_user}.{localpart}:{server}`
4. If not → topic scan
5. If not → create new E2EE room with alias
6. Cache room ID in keychain

`write_state_confirmed()`: writes to primary, waits for event ID, then replicates to secondaries. On startup, reads from whichever server has the most recent state.

Session state keys: `{device_id}/{uuid}` (supports multiple workers per account).

**Tests**: Full CRUD. Trust validation (reject untrusted creator, reject unencrypted). Write-confirm pattern. `get_or_create` idempotency via alias.

---

## Phase 3: Wire State Room + WASM Bindings (Rust + npm together)

This phase wires the state room into both ecosystems simultaneously. Core logic is in Rust, WASM bindings make it available to npm.

### 3a: WASM bindings for WorkerStateRoom — `crates/mxdx-core-wasm/src/lib.rs` (MODIFY)

Expose state room operations to npm via WASM:
```rust
// New WASM methods on WasmMatrixClient:
pub fn get_or_create_state_room(hostname, os_user, localpart) -> Result<String>  // room_id
pub fn write_state_room_config(room_id, config_json) -> Result<()>
pub fn read_state_room_config(room_id) -> Result<Option<String>>
pub fn write_session(room_id, device_id, uuid, session_json) -> Result<()>
pub fn remove_session(room_id, device_id, uuid) -> Result<()>
pub fn read_sessions(room_id) -> Result<String>  // JSON array
pub fn write_room(room_id, entry_json) -> Result<()>
pub fn read_rooms(room_id) -> Result<String>  // JSON array
pub fn write_trusted_entity(room_id, entity_type, user_id, entity_json) -> Result<()>
pub fn read_trusted_entities(room_id, entity_type) -> Result<String>
pub fn write_topology(room_id, topology_json) -> Result<()>
pub fn read_topology(room_id) -> Result<Option<String>>
```

These all delegate to the Rust `WorkerStateRoom` from Phase 2 — no separate JS implementation.

### 3b: Wire state room into Rust worker startup — `crates/mxdx-worker/src/lib.rs` (MODIFY)

```
1. Load identity, connect to Matrix                        (unchanged)
2. Get or create state room                                (NEW)
3. Read config from state room                             (NEW)
   - If config.room_name exists AND --room-name not given: use stored
   - If --room-name given: update config in state room
4. Get or create launcher space                            (uses state room config)
5. Write topology to state room                            (NEW)
6. Post WorkerInfo + advertise state room in exec room     (MODIFIED)
7. Recover sessions from state room                        (REPLACES disk recovery)
8. Main sync loop                                          (unchanged)
```

### 3c: Wire state room into npm worker — `packages/launcher/src/runtime.js` (MODIFY)

Replace disk-based state with WASM state room calls:
- Replace `#saveSessionsFile()` / `#loadSessionsFile()` → WASM `writeSession()` / `readSessions()`
- Replace `#sessionRooms` Map + `session-rooms.json` → WASM `writeRoom()` / `readRooms()`
- On startup: `getOrCreateStateRoom()` via WASM, recover sessions from state room
- On session claim: `writeSession()` to state room
- On session complete: `removeSession()` from state room

### 3d: Room name defaults from hostname+user — `crates/mxdx-worker/src/config.rs` (MODIFY)

`resolved_room_name` always has an automatic default derived from `{hostname}.{os_username}.{matrix_localpart}`. `--room-name` is an optional override, never required. On startup:
1. If `--room-name` passed → use it, write to state room config
2. If state room has stored config → use it
3. Otherwise → compute default from hostname + OS user + Matrix localpart

Applies to both Rust binary and npm worker identically.

### 3e: Replace disk session persistence with state room (both ecosystems simultaneously)

- Remove `crates/mxdx-worker/src/session_persist.rs` (disk-based `sessions.json`)
- Remove `#saveSessionsFile()`, `#loadSessionsFile()`, `sessions.json`, `session-rooms.json` from npm worker
- State room is sole source of truth for session/room state in both ecosystems
- **Config files remain**: `~/.mxdx/worker.toml`, `defaults.toml` etc. are still the config layer. Only ad-hoc disk state files (`sessions.json`) are removed.

**Tests**:
- Rust worker starts without `--room-name` on any run (defaults from hostname+user)
- npm worker starts without `--room-name` on any run (same defaults via WASM)
- Session CRUD flows through state room in both ecosystems
- No `sessions.json` created by either ecosystem
- Kill worker, restart — sessions recovered from state room

---

## Phase 4: Unified Session Events (Rust + npm together)

Wire npm client commands AND worker event handlers to use unified session events. All session management logic uses the same Rust types exposed via WASM.

### 4a: Session lifecycle WASM bindings — `crates/mxdx-core-wasm/src/lib.rs` (MODIFY)

```rust
// Task submission (for client)
pub fn create_session_task(bin, args, interactive, timeout) -> Result<String>  // JSON
pub fn submit_task(room_id, task_json) -> Result<String>  // event_id (thread root)
pub fn tail_session_thread(room_id, thread_root, timeout_ms) -> Result<String>  // yields events

// Task claiming (for worker/launcher)
pub fn claim_task(room_id, task_event_id, device_id) -> Result<String>  // SessionStart JSON
pub fn post_session_output(room_id, thread_root, output_bytes) -> Result<()>
pub fn post_session_result(room_id, thread_root, exit_code) -> Result<()>
pub fn post_session_heartbeat(room_id, thread_root) -> Result<()>
```

### 4b: npm client commands — `packages/client/src/` (MODIFY run.js, ls.js, logs.js, cancel.js)

These become thin wrappers over WASM:
- `run.js`: `createSessionTask()` → `submitTask()` → `tailSessionThread()` or print UUID for `--detach`
- `ls.js`: Read `org.mxdx.session.active` state events via `readRoomEvents()`
- `logs.js`: Fetch thread history via `findRoomEvents()`, decode output
- `cancel.js`: `sendEvent()` with `SESSION_CANCEL` type

### 4c: npm worker unified event handlers — `packages/launcher/src/runtime.js` (MODIFY)

Add handlers for unified session events (alongside old `org.mxdx.command` for backward compat):
- `SESSION_TASK` → claim task via WASM `claimTask()`, start tmux, post `SESSION_START`
- `SESSION_CANCEL` / `SESSION_SIGNAL` → forward to tmux process
- `SESSION_INPUT` → write to PTY stdin
- `SESSION_RESIZE` → resize PTY

The worker's `#processCommand()` translates old `org.mxdx.command` events to `SessionTask` internally, so the processing pipeline converges.

**Tests**:
- npm client `run` → Rust worker processes → output returns (cross-ecosystem)
- npm client `run` → npm worker processes → output returns (same ecosystem)
- `ls`, `logs`, `cancel` all work against both worker types
- Build WASM first: `wasm-pack build crates/mxdx-core-wasm --target nodejs --out-dir ../../packages/core/wasm`

---

## Phase 5: Interactive Features (Rust core + WASM, unified)

Interactive terminal sessions allow a client to attach to a running shell on the worker, piping stdin/stdout through Matrix DM rooms. All session mux logic lives in Rust, exposed via WASM.

### 5a: Interactive session mux in Rust — `crates/mxdx-worker/src/session_mux.rs` (NEW)

Port the npm worker's `SessionMux` concept to Rust:
- Routes incoming DM events to correct session by `session_id` field
- Manages multiple sessions sharing a single DM room
- Handles `SESSION_INPUT` → PTY stdin, PTY stdout → `SESSION_OUTPUT`
- Handles `SESSION_RESIZE` → PTY resize, `SESSION_SIGNAL` → process signal

This replaces the JS `SessionMux` class in `runtime.js` — the npm worker will call this via WASM.

### 5b: WASM bindings for interactive sessions — `crates/mxdx-core-wasm/src/lib.rs` (MODIFY)

```rust
// Interactive session management (for worker/launcher)
pub fn create_interactive_session(room_id, client_user_id, task_json) -> Result<String>  // DM room_id
pub fn route_dm_event(session_mux_state, event_json) -> Result<String>  // routing action
pub fn post_terminal_output(room_id, session_id, output_bytes) -> Result<()>

// Interactive session client-side (for attach command)
pub fn find_session_dm_room(exec_room_id, uuid) -> Result<Option<String>>  // DM room_id
pub fn send_terminal_input(dm_room_id, session_id, input_bytes) -> Result<()>
pub fn send_terminal_resize(dm_room_id, session_id, cols, rows) -> Result<()>
```

### 5c: Worker interactive handling — `crates/mxdx-worker/src/lib.rs` (MODIFY)

When `SessionTask.interactive == true`:
1. Create E2EE DM room with the client user (`HistoryVisibility::Joined`)
2. Create tmux interactive session (shell)
3. Post `SessionStart` with DM `room_id` for terminal I/O
4. Register with session mux for DM event routing
5. Stream tmux PTY output to DM via `BatchedSender`

### 5d: Client `attach` command — `crates/mxdx-client/src/main.rs` + `crates/mxdx-client/src/attach.rs` (MODIFY)

- `mxdx-client attach <uuid>` — find DM room from `SessionStart` event, pipe stdin/stdout
- Terminal raw mode, `SIGWINCH` → resize events, `Ctrl-]` to detach
- `mxdx-client run --interactive /bin/sh` — shorthand for run + auto-attach

### 5e: npm interactive via WASM — `packages/client/src/attach.js` + `packages/worker/src/runtime.js` (MODIFY)

- npm client `attach` uses same WASM bindings (`findSessionDmRoom`, `sendTerminalInput`, etc.)
- npm worker replaces JS `SessionMux` with WASM `routeDmEvent()` calls
- PTY bridge and `TerminalSocket` compression remain in JS (platform-specific I/O)
- WASM handles session routing, event creation, state management; JS handles PTY I/O and networking

### 5f: TURN relay support — `crates/mxdx-worker/src/webrtc.rs` (MODIFY)

- Basic TURN relay config for interactive sessions
- Worker reads TURN credentials from state room (E2EE)
- Graceful fallback when TURN unavailable (Matrix-only I/O)

**Tests**:
- E2E: `mxdx-client run --interactive /bin/sh`, send `echo hello\n`, read `hello`, send `exit\n`
- E2E: attach/detach cycle
- E2E: npm client interactive → Rust worker (cross-ecosystem)
- E2E: npm client interactive → npm worker via WASM session mux

---

## Phase 6: Benchmarks (beta infrastructure, all transports)

### 8a: Move tuwunel tests to local-dev

**`crates/mxdx-worker/tests/e2e_binary.rs`**:
- Change `#[ignore]` messages to `"local-dev: requires tuwunel binary"`
- These are for validating infrastructure/provisioning, not regular testing
- Document: "run with `--include-ignored` for local dev setup validation"

### 8b: Rust binary benchmarks — `crates/mxdx-worker/tests/e2e_profile.rs` (MODIFY)

Profiling against beta with pre-existing accounts from `test-credentials.toml`.
- Warm-up run (untimed) establishes sessions, then measured runs use session restore
- 10 iterations per workload, report min/max/p50/p95/p99/mean

**Workloads** (same command on all transports):

| Workload | Command | Measures |
|---|---|---|
| echo | `/bin/echo hello world` | session setup + round-trip latency |
| exit-code | `/bin/false` | exit code propagation latency |
| md5sum | 10k line md5 | output throughput |
| ping-30s | `ping -c 30` | streaming latency |
| ping-5min | `ping -c 300` | sustained streaming |

**Transports**: SSH, mxdx-local (single server), mxdx-federated (ca1 ↔ ca2)

**TURN variants**: For mxdx-local and mxdx-federated, run with and without TURN relay to measure TURN overhead.

### 8c: npm benchmarks — `packages/e2e-tests/bench_npm.test.js` (NEW)

Same workloads via npm client, reads same `test-credentials.toml`:
- npm client → Rust worker (cross-ecosystem)
- npm client → npm worker (same ecosystem)

### 8d: Interactive session benchmarks — `tests/e2e/bench_interactive.rs` (NEW)

- Keystroke round-trip latency: send character → receive echo (Rust binary, npm, SSH)
- Interactive throughput: send large input → receive complete output
- Compare across: SSH interactive, mxdx Rust interactive, mxdx npm interactive

### 8e: Comparative report — auto-generated markdown table

```markdown
| Transport            | Echo (p50) | Lifecycle (p50) | Throughput | Interactive RTT (p50) |
|----------------------|-----------|-----------------|------------|----------------------|
| SSH localhost        | X ms      | N/A             | Y ops/s    | Z ms                 |
| mxdx Rust local     | X ms      | X ms            | Y ops/s    | Z ms                 |
| mxdx Rust local+TURN| X ms      | X ms            | Y ops/s    | Z ms                 |
| mxdx npm local      | X ms      | X ms            | Y ops/s    | Z ms                 |
| mxdx Rust federated | X ms      | X ms            | Y ops/s    | Z ms                 |
| mxdx Rust fed+TURN  | X ms      | X ms            | Y ops/s    | Z ms                 |
| npm→Rust worker     | X ms      | X ms            | Y ops/s    | Z ms                 |
| Rust→npm worker   | X ms      | X ms            | Y ops/s    | Z ms                 |
```

Save as JSON to `docs/benchmarks/` with timestamps for regression tracking.

### 8f: Reproducible benchmark runner — `docs/benchmarks/README.md` (NEW)

Document how any user with `test-credentials.toml` and the binaries can reproduce all benchmarks.

**Blocking rule**: >10% regression on any workload blocks advancement.

---

## Phase 7: Coordinator Integration

- Worker writes `org.mxdx.worker.state_room` pointer to exec room
- Coordinator reads pointer, optionally joins state room
- Coordinator can write `org.mxdx.worker.room` events to assign rooms to workers
- Coordinator reads `org.mxdx.worker.session` events for ground-truth process state

---

## Execution Order

```
Phase 1 (types + room infra) → Phase 2 (WorkerStateRoom Rust module)
    → Phase 3 (WASM bindings + wire into BOTH ecosystems + remove disk state)
    → Phase 4 (unified session events — client commands + worker handlers)
    → Phase 5 (interactive features — Rust mux + WASM + both ecosystems)
    → Phase 6 (benchmarks — all transports, all ecosystems)
    → Phase 7 (coordinator integration)
```

**Key principle**: Every phase touches Rust + WASM + npm together. No phase is "Rust only" or "npm only" — changes move together to keep codebases unified.

**Parallel opportunities:**
- Phase 7 can run in parallel with Phase 5/6
- Within Phase 3: Rust wiring and npm wiring can proceed in parallel once WASM bindings are done

---

## Files Changed

| File | Change | Phase |
|---|---|---|
| `crates/mxdx-types/src/events/state_room.rs` | **NEW** | 1 |
| `crates/mxdx-types/src/events/mod.rs` | MODIFY | 1 |
| `crates/mxdx-types/src/identity.rs` | MODIFY | 1 |
| `crates/mxdx-matrix/src/rooms.rs` | MODIFY | 1 |
| `crates/mxdx-matrix/src/client.rs` | MODIFY | 1 |
| `crates/mxdx-worker/src/state_room.rs` | **NEW** | 2 |
| `crates/mxdx-core-wasm/src/lib.rs` | MODIFY | 3, 4, 5 |
| `crates/mxdx-worker/src/lib.rs` | MODIFY | 3, 5 |
| `crates/mxdx-worker/src/config.rs` | MODIFY | 3 |
| `packages/launcher/src/runtime.js` | MODIFY | 3, 4, 5 |
| `crates/mxdx-worker/src/session_persist.rs` | **REMOVE** | 3 |
| `packages/client/src/run.js` | MODIFY | 4 |
| `packages/client/src/ls.js` | MODIFY | 4 |
| `packages/client/src/logs.js` | MODIFY | 4 |
| `packages/client/src/cancel.js` | MODIFY | 4 |
| `crates/mxdx-worker/src/session_mux.rs` | **NEW** | 5 |
| `crates/mxdx-client/src/attach.rs` | MODIFY | 5 |
| `crates/mxdx-client/src/main.rs` | MODIFY | 5 |
| `crates/mxdx-worker/src/webrtc.rs` | MODIFY | 5 |
| `packages/client/src/attach.js` | MODIFY | 5 |
| `crates/mxdx-worker/tests/e2e_profile.rs` | MODIFY | 6 |
| `crates/mxdx-worker/tests/e2e_binary.rs` | MODIFY (ignore tags) | 6 |
| `packages/e2e-tests/bench_npm.test.js` | **NEW** | 6 |
| `tests/e2e/bench_interactive.rs` | **NEW** | 6 |
| `docs/benchmarks/README.md` | **NEW** | 6 |

---

## Security

- State room is **E2EE** — Megolm encrypted, single-member
- Coordinator access opt-in via trust anchor invitation
- State room ID cached in keychain (encrypted at rest)
- Session state events: commands/args only, NO credentials
- Interactive DM rooms: E2EE, `HistoryVisibility::Joined`, no unencrypted terminal data
- TURN credentials: stored in state room (E2EE) or keychain, never in config files
- All Matrix communication remains E2EE — no bypass
- **No separate JS crypto or state logic** — all security-sensitive operations in Rust, auditable in one place

---

## Verification

Per phase: unit tests + beta E2E suite + profiling (>10% regression blocks).

Final:
1. Both Rust worker and npm worker use same WASM `WorkerStateRoom` — verify identical state room contents
2. `--room-name` never required — automatic defaults from hostname+user work on every run
3. Kill worker, restart — sessions recovered from state room (no disk files)
4. Cross-ecosystem: npm client → Rust worker works
5. Cross-ecosystem: Rust client → npm worker works
6. Interactive: `mxdx-client run --interactive /bin/sh` works (Rust binary + npm)
7. Interactive: `mxdx-client attach <uuid>` works (Rust binary + npm)
8. npm worker's `SessionMux` replaced by WASM calls — session routing logic lives in Rust
9. Benchmark table: SSH vs mxdx-local vs mxdx-federated vs npm vs interactive, with/without TURN
10. Security review: E2EE on state room + DM rooms, no credentials in state events, TURN creds secured, all crypto in Rust
