# mxdx-fabric Phases 3-5 Task Plan

**Date:** 2026-03-21
**Status:** In progress
**Depends on:** Phase 1 + 2 complete (commits 134f0c8 through a79e671)

---

## Phase 3 — Failure Policies

### Task 3.1 — FailurePolicy execution in coordinator

In `crates/mxdx-fabric/src/failure.rs` (new file):
- `FailureContext` struct: task: TaskEvent, reason: String, attempt: u8, last_progress: Option<String>
- `async fn apply_policy(policy: &FailurePolicy, ctx: FailureContext, matrix_client: &MatrixClient, sender_room_id: &RoomId) -> Result<Option<TaskEvent>>`
  - `Escalate`: post a plain-text message to sender_room_id: "⚠️ Task {uuid} stalled: {reason}. Plan: {plan}. Last progress: {last_progress}". Return None.
  - `Respawn { max_retries }`: if ctx.attempt < max_retries: return Some(re-queued TaskEvent with attempt+1 in payload metadata). Else fall back to Escalate.
  - `RespawnWithContext`: same as Respawn but prepend failure context to task.plan: "Previous attempt failed: {reason}. Last progress: {last_progress}. Original plan: {plan}"
  - `Abandon`: post "❌ Task {uuid} abandoned: {reason}" to sender_room_id. Return None.

Wire into `coordinator.rs` backstop check loop:
- Replace log WARN with actual policy execution
- On unclaimed timeout: apply `task.on_timeout`
- On heartbeat overdue: apply `task.on_heartbeat_miss`
- If policy returns Some(new_task): re-post to coordinator room

Add `pub mod failure` to lib.rs.

E2E test in `tests/e2e_fabric.rs`:
- `test_failure_policy_escalate`: Post task with on_timeout: Escalate, timeout: 5s, then don't claim it. Wait 10s. Assert coordinator posted an escalation message to sender room.
- `test_failure_policy_respawn`: Post task with on_timeout: Respawn{max_retries:1}, timeout: 5s. Don't claim. Wait 10s. Assert a NEW task event appears in the room (re-queued). Don't claim again. Wait. Assert escalation message (exhausted retries).

Commit: `feat(fabric): failure policy enforcement`

---

### Task 3.2 — Coordinator tick loop

The coordinator's backstop check currently only runs periodically inside the main sync loop. Make it reliable:

In `coordinator.rs`:
- Add `last_backstop_check: Instant` field
- After each sync_once(), if `last_backstop_check.elapsed() > Duration::from_secs(10)`: run backstop check, update timestamp
- Backstop check: iterate watchlist, detect stale entries, apply policies
- On policy returning Some(new_task): re-post to coordinator room via matrix_client.send_event()

This ensures the coordinator never skips a check due to busy event processing.

Commit: included in `feat(fabric): failure policy enforcement`

---

## Phase 4 — P2P Stream Integration

### Task 4.1 — Unix socket stream in ProcessWorker

For tasks where `task.p2p_stream == true`:

In `process_worker.rs`:
- When `task.p2p_stream && task.heartbeat_interval_seconds < 5`:
  - Create a Unix domain socket at `/tmp/mxdx-fabric-{task_uuid}.sock`
  - Post socket path to the task room as a state event (event_type: `org.mxdx.fabric.stream_offer`, state_key: `task/{uuid}/stream`, content: `{socket_path, worker_id}`)
  - Accept one connection on the socket (the sender connects)
  - Stream jcode stdout directly to the socket connection (raw bytes)
  - Milestone events (heartbeats) still go to Matrix
  - Clean up socket after task completes

In `sender.rs`:
- `async fn connect_stream(&self, task_uuid: &str, room_id: &RoomId, timeout: Duration) -> Result<Option<UnixStream>>`:
  - Polls room for `org.mxdx.fabric.stream_offer` state event with matching task_uuid
  - Connects to the socket path
  - Returns the UnixStream for caller to read

E2E test: `test_p2p_stream_unix_socket`
- Post task with p2p_stream: true, heartbeat_interval_seconds: 2
- Worker spawns `cat /dev/urandom | head -c 10000` as a fake high-volume process
- Sender connects to stream socket, reads all bytes
- Assert: received ~10000 bytes over socket
- Assert: task result posted to Matrix as Success

Commit: `feat(fabric): P2P Unix socket stream for high-volume tasks`

---

## Phase 5 — fabric CLI + jcode skill + OpenClaw integration

**No Python. All Rust + TypeScript.**

### Task 5.1 — `mxdx-fabric-cli` binary crate

Create `crates/mxdx-fabric-cli/` — a standalone Rust binary that both jcode and OpenClaw can shell out to.

Subcommands:
- `fabric post --homeserver URL --token TOKEN --coordinator-room ROOM_ID --capabilities rust,linux --prompt "..." --timeout 1800`
  Posts a TaskEvent, waits for result, prints result JSON to stdout. Exit 0 on success, 1 on failure/timeout.
- `fabric status --homeserver URL --token TOKEN --room ROOM_ID --task-uuid UUID`
  Prints current task status (claimed/running/done/failed).
- `fabric watch --homeserver URL --token TOKEN --room ROOM_ID --task-uuid UUID`
  Tails heartbeat messages live to stdout.

Config file support: `~/.config/mxdx-fabric/config.toml` for homeserver/token defaults.

Build and install to `/home/openclaw/.local/bin/fabric`.

Commit: `feat(fabric-cli): fabric CLI binary`

---

### Task 5.2 — jcode skill

Create `~/.jcode/skills/mxdx-fabric/SKILL.md` teaching jcode when and how to delegate subtasks via fabric:

```markdown
# mxdx-fabric skill

When to use: for long-running subtasks that can run in parallel or need specialized capabilities
(e.g. cargo build, running tests, WASM compilation, security scans).

How to delegate:
  fabric post --capabilities rust,linux --prompt "cargo test -p mxdx-fabric" --timeout 300

The fabric CLI posts the task to available workers and returns the result.
Use this instead of running long commands directly.
```

Commit: `feat(fabric-cli): jcode skill for fabric delegation`

---

### Task 5.3 — OpenClaw skill

Create `/home/openclaw/openclaw/skills/mxdx-fabric/SKILL.md` teaching Bel to delegate tasks via fabric:

```markdown
# mxdx-fabric skill

Use to delegate coding/build tasks to fabric workers instead of spawning jcode subagents directly.

## Usage

fabric post \
  --capabilities rust,linux \
  --prompt "Your task here" \
  --timeout 1800

Blocks until result. Result JSON printed to stdout.
```

Commit: `feat(fabric-cli): OpenClaw skill for fabric delegation`

---

### E2E test

Add `test_fabric_cli_post` to e2e_fabric.rs:
- Start TuwunelInstance, register coordinator + worker
- Start coordinator in background task
- Run `fabric post ...` as a subprocess with `--homeserver` pointing to TuwunelInstance URL
- Worker claims and completes task (using ProcessWorker with mock binary)
- Assert: fabric CLI subprocess exits 0, stdout contains valid result JSON
