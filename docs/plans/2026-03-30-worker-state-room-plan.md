# Worker Private State Room — Implementation Plan

## Context

The mxdx worker currently stores operational state in two places: in-memory (`SessionManager`) and on disk (`~/.mxdx/sessions.json`). This means state is lost on machine migration, container recreation, or reinstall. Room names must be passed via `--room-name` every startup.

The fix: store all mutable worker state in a **private E2EE Matrix room** (the "state room"). The worker becomes stateless on disk except for crypto keys (needed for fast session/device startup). State survives across machines, the coordinator can observe/modify it, and `--room-name` is only needed on first run.

---

## Architecture

The worker creates a single-member E2EE room on first run. All operational state is stored as **Matrix state events** in this room — rooms the worker manages, active processes, configuration. The state room ID is cached in the keychain for fast lookup on subsequent starts.

```
┌─────────────────────────────────────────┐
│  Worker State Room (E2EE, single-member) │
│                                         │
│  State Events:                          │
│   org.mxdx.worker.config    ""          │  ← room_name, trust_anchor, capabilities
│   org.mxdx.worker.identity  ""          │  ← device_id, host, os_user
│   org.mxdx.worker.room      {room_id}   │  ← one per managed room
│   org.mxdx.worker.topology   {space_id}  │  ← LauncherTopology per space
│   org.mxdx.worker.session   {dev}/{uuid} │  ← replaces sessions.json
└─────────────────────────────────────────┘
         │
         │ pointer: org.mxdx.worker.state_room
         │ (state event in exec room, state_key: worker/{device_id})
         ▼
┌─────────────────────────────────────────┐
│  Exec Room (shared with clients/coord)  │
│  ← coordinator reads pointer to         │
│    discover worker's state room         │
└─────────────────────────────────────────┘
```

**Only crypto keys stay on disk**: session tokens, SQLite crypto store, device ID — all in keychain or `~/.mxdx/crypto/`. Everything else is in the state room.

---

## Phase 1: State Room Infrastructure

### 1a: Event types (`crates/mxdx-types/src/events/state_room.rs`) — NEW

```rust
pub const WORKER_STATE_CONFIG: &str = "org.mxdx.worker.config";
pub const WORKER_STATE_IDENTITY: &str = "org.mxdx.worker.identity";
pub const WORKER_STATE_ROOM: &str = "org.mxdx.worker.room";
pub const WORKER_STATE_SESSION: &str = "org.mxdx.worker.session";
pub const WORKER_STATE_TOPOLOGY: &str = "org.mxdx.worker.topology";
pub const WORKER_STATE_ROOM_POINTER: &str = "org.mxdx.worker.state_room";
```

Data structs: `WorkerStateConfig`, `StateRoomSession`, `StateRoomEntry`, all `Serialize`/`Deserialize`.

- Modify: `crates/mxdx-types/src/events/mod.rs` — add `pub mod state_room;`

### 1b: Room creation & discovery (`crates/mxdx-matrix/src/rooms.rs`) — MODIFY

Add `create_worker_state_room()`:
- Creates E2EE room with topic `org.mxdx.worker.state:{user_id}`
- `HistoryVisibility::Joined`, single-member
- Returns `OwnedRoomId`

Add `find_worker_state_room()`:
- Scans joined rooms for topic `org.mxdx.worker.state:{user_id}`
- Fallback discovery if keychain entry lost

### 1c: Bulk state read (`crates/mxdx-matrix/src/client.rs`) — MODIFY

Add `get_all_state_events_of_type(room_id, event_type) -> Vec<(state_key, Value)>`:
- Calls `GET /_matrix/client/v3/rooms/{roomId}/state`
- Filters by event type, returns all matching state_key → content pairs
- Needed to read all sessions, all rooms, etc.

### 1d: Keychain key (`crates/mxdx-types/src/identity.rs`) — MODIFY

Add `state_room_key(user_id)` returning `mxdx/{user_id}/state-room-id`.

**Tests**: Create state room, write config event, read it back. Find state room by topic. Bulk state read returns correct entries.

---

## Phase 2: WorkerStateRoom Module

### `crates/mxdx-worker/src/state_room.rs` — NEW

```rust
pub struct WorkerStateRoom {
    room_id: OwnedRoomId,
}

impl WorkerStateRoom {
    pub async fn get_or_create(client, user_id, keychain) -> Result<Self>;

    // Config
    pub async fn write_config(&self, client, config) -> Result<()>;
    pub async fn read_config(&self, client) -> Result<Option<WorkerStateConfig>>;

    // Sessions (replaces sessions.json)
    pub async fn write_session(&self, client, session) -> Result<()>;
    pub async fn remove_session(&self, client, uuid) -> Result<()>;
    pub async fn read_sessions(&self, client) -> Result<Vec<StateRoomSession>>;

    // Rooms
    pub async fn write_room(&self, client, entry) -> Result<()>;
    pub async fn read_rooms(&self, client) -> Result<Vec<StateRoomEntry>>;

    // Topology
    pub async fn write_topology(&self, client, topo, launcher_id) -> Result<()>;
    pub async fn read_topology(&self, client) -> Result<Option<LauncherTopology>>;

    // Coordinator discovery
    pub async fn advertise_in_exec_room(&self, client, exec_room_id, device_id) -> Result<()>;
}
```

`get_or_create()` flow:
1. Check keychain for `mxdx/{user_id}/state-room-id`
2. If found → use directly
3. If not → scan joined rooms by topic
4. If not → create new state room
5. Cache room ID in keychain

Session state keys use `{device_id}/{uuid}` to support multiple workers per account.

**Tests**: Full CRUD for config, sessions, rooms, topology. `get_or_create` with empty keychain creates room. Second call with cached keychain returns same room.

---

## Phase 3: Wire Into Worker Startup

### Modified startup flow (`crates/mxdx-worker/src/lib.rs`)

```
1. Load identity from keychain                        (unchanged)
2. Connect to Matrix                                  (unchanged)
3. ** Get or create state room **                     (NEW)
4. ** Read config from state room **                  (NEW)
   - If config.room_name exists AND --room-name not given: use stored name
   - If --room-name given: update config in state room
5. Get or create launcher space                       (unchanged, but uses state room config)
6. ** Write topology to state room **                 (NEW)
7. Post WorkerInfo + advertise state room in exec room (MODIFIED)
8. ** Recover sessions from state room **             (REPLACES disk recovery)
9. Enter main sync loop                               (unchanged)
```

### Modified session lifecycle

Replace `persist_active_sessions()` (which writes `sessions.json`) with `state_room.write_session()` / `state_room.remove_session()`. Called at:
- Session claimed (write)
- Session running (update)
- Session completed (remove)

### `connect()` returns topology

Currently `connect()` discards `LauncherTopology` after extracting `exec_room_id`. Change to return `(MatrixWorkerRoom, LauncherTopology)` so the caller can store it in the state room.

### Make `--room-name` optional

- `crates/mxdx-worker/src/config.rs` — `resolved_room_name` becomes `Option<String>`
- On subsequent runs: read from state room config
- If neither CLI nor state room has a room name: fail with clear error

**Tests**: Worker startup without `--room-name` reads from state room. Worker startup with `--room-name` updates state room. Session CRUD flows through state room instead of disk.

---

## Phase 4: Remove Disk Persistence

- **Remove** `crates/mxdx-worker/src/session_persist.rs`
- **Remove** all references to `sessions.json`
- State room is now the sole source of truth for worker state (except crypto keys)

**Tests**: Verify no `~/.mxdx/sessions.json` is created. Verify session recovery works from state room after simulated crash.

---

## Phase 5: Coordinator Integration

- **Exec room state event**: Worker writes `org.mxdx.worker.state_room` with state_key `worker/{device_id}` containing `{ state_room_id }`. Coordinator reads this to find the state room.
- **Coordinator joins**: Worker invites trust anchor to state room (optional, controlled by config).
- **Room assignment**: Coordinator writes `org.mxdx.worker.room` events to add rooms to a worker's managed set.

**Tests**: Coordinator discovers state room via exec room pointer. Coordinator reads session state from state room.

---

## Files Changed

| File | Change | Phase |
|---|---|---|
| `crates/mxdx-types/src/events/state_room.rs` | **NEW** — event types + data structs | 1 |
| `crates/mxdx-types/src/events/mod.rs` | MODIFY — export state_room | 1 |
| `crates/mxdx-types/src/identity.rs` | MODIFY — add `state_room_key()` | 1 |
| `crates/mxdx-matrix/src/rooms.rs` | MODIFY — create/find state room | 1 |
| `crates/mxdx-matrix/src/client.rs` | MODIFY — `get_all_state_events_of_type()` | 1 |
| `crates/mxdx-worker/src/state_room.rs` | **NEW** — WorkerStateRoom | 2 |
| `crates/mxdx-worker/src/lib.rs` | MODIFY — wire state room into startup + session lifecycle | 3 |
| `crates/mxdx-worker/src/config.rs` | MODIFY — optional room_name | 3 |
| `crates/mxdx-worker/src/session_persist.rs` | **REMOVE** | 4 |

---

## Security

- State room is **E2EE** — all state events encrypted via Megolm
- Single-member room — only the worker's account can read state
- Coordinator access is opt-in (trust anchor invitation)
- State room ID cached in keychain (encrypted at rest via OS keychain or AES-256-GCM file)
- Session state events contain command/args but NOT credentials (same as current `sessions.json`)
- State events are auditable — Matrix preserves history with timestamps

---

## Verification

1. `cargo test -p mxdx-worker -p mxdx-matrix -p mxdx-types` — all pass
2. `cargo test -p mxdx-worker --test e2e_binary -- --ignored` — all 6 E2E tests pass
3. `cargo test -p mxdx-worker --test e2e_profile -- --ignored` — profiling against beta
4. Worker starts without `--room-name` on second run (reads from state room)
5. Kill worker, restart — sessions recovered from state room, not disk
6. No `~/.mxdx/sessions.json` created anywhere
7. State room is E2EE (verify via `m.room.encryption` state event)
8. Security review: no credentials in state events, proper E2EE, keychain-stored room ID
