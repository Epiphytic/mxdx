# mxdx-fabric: Agent Task Fabric Design

**Date:** 2026-03-21
**Status:** Draft
**Author:** Bel (belthanior)

---

## Summary

`mxdx-fabric` is a new crate in the mxdx workspace that turns a Matrix homeserver into an agent task coordination fabric. It provides:

- **Capability-based routing** — workers advertise what they can do; tasks declare what they need; the coordinator matchmakes
- **Decentralized claim racing** — first available worker claims a task via Matrix state events; no central scheduler
- **Coordinator as backstop** — monitors for missed tasks, duplicate claims, and heartbeat failures; only intervenes when things go wrong
- **P2P data streams** — high-volume or low-latency streams negotiate direct connections (building on the existing P2P transport design); control plane stays in Matrix

This design builds directly on existing mxdx primitives:
- `mxdx-matrix` — client, room management, E2EE
- `mxdx-types` — event types (extended here)
- `mxdx-secrets` — double-encrypted credential injection
- P2P transport plan (`2026-03-10-p2p-transport-design.md`)

---

## Motivation

The immediate problem: spawning jcode subagents from an LLM (Bel) is fragile. The LLM's context accumulates all subagent output, making long sessions drift. There's no way to:
- Supervise a subagent without polling
- Recover from a crash without manual intervention
- Route tasks to the right worker automatically
- Get async progress updates without blocking

The general problem: agent-to-agent coordination has no standard fabric. Existing solutions (BullMQ, Temporal, LangGraph) are either dumb (no LLM reasoning in failure handling) or not agent-native.

mxdx already has the right primitives. This is the coordination layer on top.

---

## Architecture

### Roles

**Sender** — an LLM agent (e.g., Bel/OpenClaw) that posts task events to Matrix. Doesn't need to know which worker will handle it or where it's running.

**Worker** — a process that can execute tasks (jcode, bash, girt pipeline, etc.). Advertises its capabilities as a Matrix state event. Watches capability rooms for matching tasks, claims them, executes, reports back.

**Coordinator** — a persistent mxdx bot that watches all task rooms. Handles routing (inviting tasks to the right room), monitors heartbeats, enforces failure policies. Does **not** relay data — only manages lifecycle.

### Room topology

```
#fabric-coordinator:belthanior     ← sender posts tasks here; coordinator handles routing
#workers.rust.linux:belthanior     ← workers with rust+linux capability join here
#workers.bash.linux:belthanior     ← general bash workers
#workers.girt:belthanior           ← GIRT pipeline workers
```

Rooms are named by capability namespace. Workers join the rooms matching their capabilities at startup and stay joined.

### Task lifecycle

```
1. Sender posts TaskEvent → #fabric-coordinator
2. Coordinator reads required_capabilities, routing_mode
   - direct mode:  invites sender to matching capability room
   - brokered mode: coordinator posts TaskEvent to room on sender's behalf
3. First available worker in the room posts ClaimEvent (Matrix state event)
4. Other workers see the claim, back off
5. Worker executes; posts HeartbeatEvent periodically to the room
6. Worker posts TaskResultEvent on completion
7. In direct mode: sender was in the room, sees the result directly
   In brokered mode: coordinator forwards result to sender's DM room
8. Coordinator cleans up (removes sender from room if no more pending tasks)

Backstop monitoring (coordinator watches throughout):
- Task unclaimed after claim_timeout_s → re-post or escalate
- Duplicate claims → signal loser to back off
- Heartbeat gap > heartbeat_deadline_s → declare dead, apply failure policy
- Worker crash without result → re-queue or escalate
```

### Routing modes

| Mode | When to use | How it works |
|------|-------------|--------------|
| `direct` | Short tasks (< 30s), latency-sensitive | Sender joins worker room directly; sees all worker chatter |
| `brokered` | Long tasks, cleaner isolation | Coordinator proxies; sender only sees its own task events |
| `auto` (default) | Let the coordinator decide | Uses `timeout_seconds < 30` → direct, else → brokered |

The latency cost of brokered mode is ~50-100ms (one extra Matrix event hop on a local homeserver). For tasks over 30s, this is irrelevant.

### P2P data streams

For high-volume output (jcode stdout, GIRT pipeline logs) or tight heartbeat intervals (< 5s):

1. Worker posts P2P offer to the task room (reusing `m.call.invite` from the existing P2P transport)
2. Sender/coordinator accepts
3. Raw stream flows over the direct socket (Unix domain socket on localhost; WebRTC data channel cross-host)
4. Milestones, heartbeats, and task result still go to Matrix

This mirrors the existing P2P transport design exactly. No new P2P primitives needed — just wire `mxdx-fabric` to use the existing P2P path when task metadata requests it.

**Threshold for P2P negotiation** (configurable):
- `heartbeat_interval_s < 5` → P2P
- `estimated_output == "stream"` → P2P
- Otherwise → Matrix only

---

## New Types (`mxdx-types` additions)

### `TaskEvent` — sender → coordinator room

```rust
pub struct TaskEvent {
    pub uuid: String,
    pub required_capabilities: Vec<String>,   // ["rust", "linux", "arm64", "ram:3gb"]
    pub estimated_cycles: Option<u64>,         // rough CPU budget for backpressure
    pub timeout_seconds: u64,                  // task deadline
    pub heartbeat_interval_seconds: u64,       // expected heartbeat cadence
    pub on_timeout: FailurePolicy,
    pub on_heartbeat_miss: FailurePolicy,
    pub routing_mode: RoutingMode,
    pub p2p_stream: bool,                      // request P2P for output stream
    pub payload: serde_json::Value,            // task-specific content (command, prompt, etc.)
    pub plan: Option<String>,                  // why this task — used by coordinator for recovery reasoning
}

pub enum FailurePolicy {
    Escalate,                    // notify sender, wait for human input
    Respawn { max_retries: u8 }, // re-queue with same params
    RespawnWithContext,          // re-queue; coordinator includes failure context in new TaskEvent
    Abandon,                     // mark failed, notify sender
}

pub enum RoutingMode {
    Direct,
    Brokered,
    Auto,
}
```

### `CapabilityEvent` — worker → capability room (state event)

```rust
pub struct CapabilityEvent {
    pub worker_id: String,               // @jcode-worker:belthanior
    pub capabilities: Vec<String>,       // ["rust", "linux", "arm64"]
    pub available_gas: u64,              // remaining CPU budget
    pub reserved_gas: u64,               // gas already committed to claimed tasks
    pub max_concurrent_tasks: u8,
    pub current_task_count: u8,
}
```

Workers update this state event when they claim or complete tasks.

### `ClaimEvent` — worker → task room (state event, key: task/{uuid}/claim)

```rust
pub struct ClaimEvent {
    pub task_uuid: String,
    pub worker_id: String,
    pub claimed_at: u64,          // unix timestamp
}
```

Matrix state events are last-write-wins per key — the homeserver serializes concurrent writes. Workers read back the state event after posting to confirm they won the race. Losers see a different `worker_id` and back off silently.

### `HeartbeatEvent` — worker → task room

```rust
pub struct HeartbeatEvent {
    pub task_uuid: String,
    pub worker_id: String,
    pub progress: Option<String>,   // optional human-readable progress note
    pub gas_consumed: u64,          // cycles used so far
    pub timestamp: u64,
}
```

### `TaskResultEvent` — worker → task room

```rust
pub struct TaskResultEvent {
    pub task_uuid: String,
    pub worker_id: String,
    pub status: TaskStatus,
    pub output: Option<serde_json::Value>,
    pub error: Option<String>,
    pub gas_consumed: u64,
    pub duration_seconds: u64,
}

pub enum TaskStatus {
    Success,
    Failed,
    Timeout,
    Cancelled,
}
```

---

## `mxdx-fabric` Crate Structure

```
crates/mxdx-fabric/
  src/
    lib.rs
    coordinator.rs      # coordinator bot: routing, monitoring, failure policy
    worker.rs           # worker client: capability advertisement, task claiming
    sender.rs           # sender client: task posting, result waiting
    capability_index.rs # coordinator's in-memory index of capability rooms
    failure.rs          # failure policy execution
    claim.rs            # claim race logic + back-off
  tests/
    e2e_fabric.rs       # integration tests against local Tuwunel
```

### Coordinator loop (simplified)

```rust
// coordinator.rs
loop {
    let event = matrix_client.next_event().await;
    match event {
        TaskEvent(task) => {
            let room = capability_index.find_room(&task.required_capabilities)?;
            match task.routing_mode {
                Direct => matrix_client.invite(task.sender, room).await?,
                Brokered => matrix_client.post_to_room(room, &task).await?,
                Auto => { /* decide based on timeout_seconds */ }
            }
            watchlist.insert(task.uuid, WatchEntry {
                task,
                claimed_at: None,
                last_heartbeat: Instant::now(),
            });
        }
        ClaimEvent(claim) => {
            watchlist.get_mut(&claim.task_uuid)?.claimed_at = Some(Instant::now());
        }
        HeartbeatEvent(hb) => {
            watchlist.get_mut(&hb.task_uuid)?.last_heartbeat = Instant::now();
        }
        TaskResultEvent(result) => {
            watchlist.remove(&result.task_uuid);
            if brokered_mode { forward_result_to_sender(result).await? }
        }
    }

    // periodic backstop checks
    for (uuid, entry) in &watchlist {
        if entry.claimed_at.is_none() && entry.elapsed() > claim_timeout {
            apply_failure_policy(&entry.task.on_timeout, entry).await?;
        }
        if entry.claimed_at.is_some() && entry.heartbeat_overdue() {
            apply_failure_policy(&entry.task.on_heartbeat_miss, entry).await?;
        }
    }
}
```

---

## Capability Matching

Workers advertise capabilities as a flat string list: `["rust", "linux", "arm64", "ram:8gb"]`.

Tasks declare required capabilities: `["rust", "linux"]`.

Matching rule: task's required capabilities must be a **subset** of the worker's advertised capabilities. Room membership encodes this — workers only join rooms matching their capabilities, so any worker in the room can handle any task posted there.

Room naming convention: `#workers.{cap1}.{cap2}:homeserver` (sorted, lowercase). Coordinator maintains the index at startup by scanning room state events.

**Gas / CPU budget:**

`estimated_cycles` is a hint, not a hard limit. Workers advertise `available_gas`. Coordinator (or workers) can skip tasks if `estimated_cycles > worker.available_gas`. This is a backpressure mechanism, not a scheduler — keep it simple, don't over-engineer.

---

## Failure Policy Execution

When the coordinator detects a failure (missed claim, missed heartbeat, worker crash):

### `Escalate`
Post a `CoordinatorAlertEvent` to the sender's DM room:
```
Task {uuid} appears stalled (no heartbeat for {n}s).
Plan: {task.plan}
Last progress: {last_heartbeat.progress}
Options: [respawn] [abandon] [wait]
```
Sender (LLM) reads the alert and decides. This is the human-in-the-loop path.

### `Respawn { max_retries }`
Re-post the original `TaskEvent` to the coordinator room with `retries_remaining: n-1`. If `retries_remaining == 0`, fall back to `Escalate`.

### `RespawnWithContext`
Same as `Respawn`, but prepend the failure context to `task.plan`:
```
Previous attempt failed after {duration}s with: {error}
Last progress: {progress}
Retrying from context. Original plan: {original_plan}
```
Useful for jcode — the re-spawned worker has context on what was tried.

### `Abandon`
Mark task as failed in room state, notify sender, done.

---

## Integration with jcode

jcode doesn't speak Matrix natively. The worker adapter wraps it:

```rust
// worker.rs — jcode adapter
async fn execute_task(task: &TaskEvent, matrix_room: &RoomId) {
    // post claim
    claim_task(task.uuid, matrix_room).await?;

    // spawn jcode
    let mut child = Command::new("jcode")
        .args(["--provider", "claude", "--ndjson", "run", &task.payload["prompt"]])
        .stdout(Stdio::piped())
        .spawn()?;

    // if P2P requested: negotiate socket, stream stdout over it
    // else: batch stdout and post as HeartbeatEvents to Matrix

    let batcher = OutputBatcher::new(4096, Duration::from_secs(30));
    while let Some(line) = child.stdout.read_line().await {
        batcher.push(line);
        if let Some(batch) = batcher.tick() {
            post_heartbeat(task.uuid, batch.as_str(), matrix_room).await?;
        }
    }

    let status = child.wait().await?;
    post_result(task.uuid, status, matrix_room).await?;
}
```

The `OutputBatcher` already exists in `mxdx-launcher/terminal/batcher.rs` — reuse it.

---

## What This Is NOT

- **Not a scheduler.** No central queue, no priority lanes, no fair-share. Workers race, first wins.
- **Not a message bus.** Point-to-point task assignment, not pub/sub fan-out.
- **Not a workflow engine.** No DAGs, no step dependencies, no conditional branching. Tasks are atomic units. Compose them at the LLM layer, not here.
- **Not multi-tenant.** Trust model assumes all agents in a room are controlled by the same operator. Federation is for connecting your own boxes, not untrusted third parties.

---

## Open Questions

1. **Coordinator identity** — should the coordinator be a dedicated Matrix account (`@coordinator:belthanior`) or a capability of the launcher itself? Dedicated account is cleaner isolation; embedded is simpler ops.

2. **Room creation** — who creates capability rooms? Coordinator on first worker registration, or pre-provisioned? Pre-provisioned is simpler; dynamic creation supports ad-hoc capability namespaces.

3. **Gas accounting** — is per-task `estimated_cycles` worth the complexity? Could start with just `max_concurrent_tasks` on workers and add gas later.

4. **Sender client** — does Bel (OpenClaw) need a native `mxdx-fabric` client, or does it talk to the coordinator via the existing mxdx `CommandEvent` mechanism? The latter is less coupling.

5. **Cross-host** — the P2P path uses Unix sockets on localhost. For cross-host workers, this becomes WebRTC. Worth designing now or defer until needed?

---

## Phased Build Plan

### Phase 1 — Types + coordinator skeleton (1-2 days)
- Add `TaskEvent`, `CapabilityEvent`, `ClaimEvent`, `HeartbeatEvent`, `TaskResultEvent` to `mxdx-types`
- Coordinator loop: routing + watchlist (no failure policy yet)
- Basic integration test: sender posts task, worker claims, coordinator routes

### Phase 2 — Worker client + jcode adapter (1-2 days)
- Worker capability advertisement
- Claim race + back-off
- jcode adapter: wrap CLI, post heartbeats, post result
- Integration test: full task lifecycle with real jcode

### Phase 3 — Failure policies (1 day)
- `Escalate`, `Respawn`, `RespawnWithContext`, `Abandon`
- Coordinator heartbeat monitoring
- Integration test: simulate worker crash, verify respawn

### Phase 4 — P2P stream integration (1-2 days)
- Wire fabric task metadata to existing P2P transport
- Unix socket path negotiation on localhost
- Integration test: high-volume jcode output over P2P

### Phase 5 — Sender client for OpenClaw (1 day)
- OpenClaw skill or plugin to post tasks and receive results
- Replaces current jcode subagent spawning pattern

Total: ~6-8 days of focused work.

---

## Relation to Existing mxdx Work

The fabric does not replace or modify existing mxdx features. It adds a coordination layer on top:

| Existing | Fabric use |
|----------|-----------|
| `mxdx-matrix` MatrixClient | Used directly by coordinator and workers |
| `mxdx-launcher` TerminalSession | Workers can use it for PTY-based tasks |
| `mxdx-launcher` OutputBatcher | Reused in jcode adapter |
| `mxdx-secrets` SecretCoordinator | Workers request credentials via existing double-encryption protocol |
| P2P transport | Workers negotiate P2P for streaming tasks |
| `CommandEvent` | Existing mechanism; fabric adds higher-level `TaskEvent` on top |

The security issues identified in the audit (SEC-03 RCE via unvalidated command, SEC-01 env injection) MUST be fixed before deploying the fabric. The fabric expands the attack surface for these issues — more agents posting `CommandEvent`s means more opportunities for injection. Fix the allowlist validation first.
