# Unified Session Architecture Design

**Date:** 2026-03-26
**Status:** Draft
**Supersedes:** `mxdx-fabric` design (2026-03-21), launcher-client runtime design (2026-03-06)

## Problem

The codebase has two overlapping execution systems:

- **mxdx-fabric:** Generic process execution via Matrix with coordinator routing, heartbeat monitoring, capability-based worker selection, and threaded output.
- **mxdx-launcher:** Fleet management with tmux-based terminal sessions, PTY bridging, telemetry, and interactive shell access.

The only real difference is whether the client stays connected interactively or not. Both spawn processes on remote hosts and route output over Matrix. This duplication means two sets of event types, two output routing strategies, two session models, and two codebases doing fundamentally the same thing.

## Solution

Unify into a single session model where every command execution — fire-and-forget, long-running, or interactive — follows the same lifecycle. Output always flows to Matrix threads (ground truth). A low-latency acceleration layer (specified separately — see Future Work) can transparently accelerate interactive sessions. A client can disconnect and reconnect to any session.

## Crate Layout

### Rust Crates

| Crate | Purpose |
|---|---|
| `mxdx-worker` | Unified execution engine. Spawns processes in tmux, manages sessions, streams output to Matrix threads, maintains process table via state events, cleans up history past retention window. Future: acceleration layer for interactive sessions. |
| `mxdx-client` | CLI for submitting commands, tailing output threads, attaching to interactive sessions, listing active/historical commands, reconnecting to disconnected sessions. |
| `mxdx-coordinator` | Optional fleet routing. Capability-based task routing to worker rooms, failure policies (timeout/retry/escalate), claim arbitration. Not required for single-host use. |
| `mxdx-types` | Shared event types — unified session events, capability schemas. Extended from existing crate. |
| `mxdx-matrix` | Matrix protocol abstraction with thread helpers and state event utilities. Extended from existing crate. |
| `mxdx-core-wasm` | WASM build target exposing worker and client functionality for npm packages. Extended from existing crate. |

### npm Packages

| Package | Role |
|---|---|
| `@mxdx/core` | WASM bindings + JS OS layer (process spawning, PTY, filesystem) |
| `@mxdx/launcher` | Worker binary via Node.js — `npx @mxdx/launcher start` |
| `@mxdx/client` | Client CLI via Node.js — `npx @mxdx/client exec ...` |
| `@mxdx/web-console` | Browser SPA (Vite + xterm.js) |
| `@mxdx/cli` | Dispatcher — `npx mxdx <subcommand>` |

### Dual Distribution

- **npm (WASM+JS):** Universal install path. `npx @mxdx/launcher start` works anywhere Node.js runs. WASM handles Matrix/E2EE, JS handles OS-level operations (process spawning, PTY, filesystem). No cross-compilation needed.
- **Native Rust binaries (Linux):** Same CLI interface, same behavior, better performance. Built from the same Rust crates. Users shouldn't notice the difference except speed.

## Session Model

### Core Concept

Every command execution is a **session**. A session is created when a client submits a TaskEvent to the worker's room. The TaskEvent becomes the thread root. All output, heartbeats, results, and client input are posted as threaded replies. The thread is the complete, immutable log of the session.

### Session Lifecycle

```
Client posts TaskEvent → Worker claims (state event: session/{uuid}/active)
  → Worker posts StartEvent to thread (worker_id, tmux_session, started_at)
  → tmux session created (always, even non-interactive)
  → Process spawns inside tmux
  → Output streams to Matrix thread (batched output events)
  → Heartbeat events posted periodically (liveness, independent of output)
  → If interactive + acceleration layer available:
      → Output also streams via acceleration layer (low latency)
      → --no-room-output suppresses stdout/stderr in thread, but session
        start/end/exit-code events and heartbeats are ALWAYS posted
  → If client disconnects:
      → Output continues to Matrix thread automatically
      → tmux session preserves full scrollback on worker
  → Client reconnects:
      → Finds session via state event
      → Tails thread and/or re-establishes acceleration layer
  → If cancel event received:
      → Worker sends SIGTERM, waits grace_seconds (default 10)
      → If still alive: SIGKILL
      → ResultEvent posted with status: "cancelled"
      → State event updated to session/{uuid}/completed
  → Process exits (normal or after cancel):
      → ResultEvent posted to thread (exit code, duration)
      → State event updated to session/{uuid}/completed
  → After retention period (default 90 days):
      → Completed state event removed by worker cleanup
      → Thread content remains in Matrix history
```

### Output Modes

| Mode | Matrix Thread | Acceleration Layer | tmux |
|---|---|---|---|
| Default | Full stdout/stderr + heartbeats | If interactive & available | Always |
| `--no-room-output` | Start/end/exit-code + heartbeats only | If interactive & available | Always |

With `--no-room-output`, all session metadata (command, who, when, exit code, duration) and liveness heartbeats are always recorded in the thread. Only stdout/stderr content is suppressed. Output is still fully available via tmux scrollback on the worker, accessible through reconnection.

### State Events as Process Table

| State Key | Content | Purpose |
|---|---|---|
| `session/{uuid}/active` | bin, args, PID, start time, client ID, interactive flag, worker_id | Running session (also serves as claim — first writer wins) |
| `session/{uuid}/completed` | exit code, duration, completion time | Finished session |
| `worker/{id}/info` | See Worker Info Schema below | Combined capability + telemetry advertisement |

**Note:** Slash separators in state keys are technically valid per the Matrix spec. The existing fabric code already uses `task/{uuid}/claim` and `task/{uuid}/stream` as precedent. This is a known interoperability consideration if targeting homeservers beyond Tuwunel — if issues arise, switch to dot separators (e.g., `session.{uuid}.active`).

- `mxdx ls` reads all `session/*/active` state events
- `mxdx ls --all` includes `session/*/completed` within retention window
- `mxdx logs <uuid>` fetches the thread from the task event
- `mxdx attach <uuid>` reads active state, re-establishes acceleration layer if interactive, else tails thread

**Session state events double as claim events.** When a worker writes `session/{uuid}/active` with its `worker_id`, that constitutes the claim. The worker reads back the state event to confirm it won (last-write-wins). No separate claim event type is needed.

### Retention

The worker runs a periodic sweep (hourly) removing `session/*/completed` state events older than the configured retention window. Default: 90 days, configurable via `--history-retention 90d`. Thread content remains in Matrix room history — only the state event index is cleaned up. This means completed sessions older than the retention window are invisible to `mxdx ls --all` but their threads are still accessible if you know the event ID. This is intentional — Matrix history is the long-term archive, state events are the queryable index.

### Worker Info Schema

The `worker/{id}/info` state event combines capability advertisement and host telemetry into a single event. This merges the existing `CapabilityAdvertisement` from fabric with the launcher's telemetry system.

```
org.mxdx.worker.info (state event, state_key: worker/{id})
├── worker_id: String
├── host: String
├── os: String                          // e.g., "linux"
├── arch: String                        // e.g., "x86_64", "aarch64"
├── cpu_count: u32
├── memory_total_mb: u64
├── disk_available_mb: u64
├── tools: Vec<WorkerTool>
│   ├── name: String                    // binary name, e.g., "node"
│   ├── version: Option<String>         // e.g., "22.22.0"
│   ├── description: String
│   ├── healthy: bool                   // last health check passed
│   └── input_schema: InputSchema
│       ├── type: "object"
│       ├── properties: Map<String, SchemaProperty>
│       └── required: Vec<String>
├── capabilities: Vec<String>           // e.g., ["linux", "x86_64", "node", "rust"]
├── updated_at: u64                     // unix timestamp of last refresh
```

The coordinator uses `capabilities` for routing and `tools` for detailed capability matching. Clients use the telemetry fields for `mxdx ls` display. The worker refreshes this event periodically (e.g., every 5 minutes) and on capability changes.

## Event Schema

### Worker → Thread Events

```
org.mxdx.session.start
├── session_uuid: String
├── worker_id: String
├── tmux_session: String               // tmux session name for recovery
├── pid: Option<u32>                   // OS process ID (if available)
├── started_at: u64                    // unix timestamp

org.mxdx.session.output
├── session_uuid: String
├── worker_id: String
├── stream: "stdout" | "stderr"
├── data: String                       // base64 encoded, batched
├── seq: u64                           // sequence number for ordering
├── timestamp: u64

org.mxdx.session.heartbeat
├── session_uuid: String
├── worker_id: String
├── timestamp: u64
├── progress: Option<String>           // human-readable status (optional)

org.mxdx.session.result
├── session_uuid: String
├── worker_id: String
├── status: "success" | "failed" | "timeout" | "cancelled"
├── exit_code: Option<i32>
├── duration_seconds: u64
├── tail: Option<String>               // last 50 lines for quick display
```

**Heartbeat vs output separation:** Heartbeats are liveness signals, always posted regardless of `--no-room-output`. Output events carry actual stdout/stderr data and are suppressed by `--no-room-output`. This prevents the coordinator from falsely detecting heartbeat misses on quiet processes or processes running with suppressed output.

### Client → Thread Events

```
org.mxdx.session.input
├── session_uuid: String
├── data: String                       // base64 encoded stdin

org.mxdx.session.signal
├── session_uuid: String
├── signal: "SIGINT" | "SIGTERM" | "SIGKILL" | ...

org.mxdx.session.resize
├── session_uuid: String
├── cols: u32
├── rows: u32

org.mxdx.session.cancel
├── session_uuid: String
├── reason: Option<String>
├── grace_seconds: Option<u64>         // default: 10
```

**Cancel behavior:** When a worker receives a `cancel` event, it sends SIGTERM to the process, waits `grace_seconds` (default 10), then sends SIGKILL if the process is still alive. The worker then posts a `result` event with `status: "cancelled"` and updates the state event to `session/{uuid}/completed`. The `signal` event is for sending a specific signal without the graceful shutdown sequence.

**CLI dispatch:** `mxdx cancel <uuid>` sends an `org.mxdx.session.cancel` event (graceful shutdown). `mxdx cancel <uuid> --signal SIGKILL` sends an `org.mxdx.session.signal` event directly (immediate, no grace period).

### Task Submission Event (Thread Root)

```
org.mxdx.session.task
├── uuid: String
├── sender_id: String
├── bin: String                        // binary path or name (NOT a shell expression)
├── args: Vec<String>
├── env: Option<Map<String, String>>
├── cwd: Option<String>
├── interactive: bool                  // request PTY allocation
├── no_room_output: bool               // suppress stdout/stderr in thread
├── timeout_seconds: Option<u64>       // None = no timeout
├── heartbeat_interval_seconds: u64    // how often worker sends heartbeats (default: 30)
├── plan: Option<String>               // execution plan context (used by RespawnWithContext)
├── required_capabilities: Vec<String> // for coordinator routing (optional, empty = any worker)
├── routing_mode: Option<RoutingMode>  // "direct" | "brokered" | "auto" (coordinator only)
├── on_timeout: Option<FailurePolicy>  // coordinator only (default: Escalate)
├── on_heartbeat_miss: Option<FailurePolicy>  // coordinator only (default: Escalate)
```

**`bin` field (not `command`):** This field is a binary path or name, never a shell expression. The worker resolves it via PATH lookup or absolute path. This removes shell injection as a concern at the protocol level.

**Sanitization rules (enforced by worker `executor` module):**
- `bin`: Must be a single token. No shell metacharacters (`|`, `&`, `;`, `` ` ``, `$`, `(`, `)`, `>`, `<`). Resolved via PATH or absolute path only.
- `args`: Each arg is passed as a discrete argv element (no shell expansion). Validated for no null bytes.
- `cwd`: Must be an absolute path. No `..` traversal allowed. Must exist on the worker filesystem.
- `env`: Keys must be valid environment variable names (`[A-Z_][A-Z0-9_]*`). Values are arbitrary strings.

**`interactive` field:** This replaces the old `p2p_stream` field from `fabric::TaskEvent`. The semantic change is intentional: `p2p_stream` described a transport mechanism, `interactive` describes the session mode (PTY allocation). The acceleration layer is orthogonal — it is used when available for interactive sessions but is not implied by this flag alone.

**Coordinator-only fields:** `routing_mode`, `on_timeout`, and `on_heartbeat_miss` are only meaningful when a coordinator is in the path. For direct client→worker communication, these are ignored. They are `Option` types so single-host users don't need to specify them.

## Worker Internal Architecture

### Modules

| Module | Responsibility |
|---|---|
| `session` | Session lifecycle — create, track, complete, cleanup. Owns state event process table. Claim via state event write. |
| `executor` | Process spawning inside tmux — child process with optional PTY allocation. Arg sanitization (see rules above). |
| `output` | Output routing — multiplexes stdout/stderr to Matrix thread (batched output events). Respects `no_room_output`. |
| `heartbeat` | Periodic liveness heartbeat posting. Independent of output. Always active regardless of `no_room_output`. |
| `tmux` | tmux session management — every process runs inside tmux. Scrollback persistence and terminal state across disconnections. |
| `telemetry` | Host info + capability advertisement — single state event with OS/arch/resources and available tools/binaries. Periodic refresh. |
| `retention` | Periodic sweep of completed session state events past retention window. |
| `matrix` | Room setup, thread posting, state event read/write. Built on `mxdx-matrix`. |

### Why tmux Always

Even non-interactive commands run inside tmux:
- Output is always recoverable from the worker side
- A non-interactive session can be "upgraded" to interactive mid-flight via `mxdx attach -i <uuid>`
- Worker restarts can reconnect to surviving tmux sessions

## Client Internal Architecture

### Modules

| Module | Responsibility |
|---|---|
| `submit` | Build and post TaskEvent to worker/coordinator room. Return session UUID. |
| `tail` | Follow a session's Matrix thread in real-time. Render stdout/stderr with stream markers. |
| `attach` | Attach to active session. Initially thread-tailing only; future acceleration layer support. Falls back gracefully. |
| `ls` | Read session state events from worker room. Format as process table. Filter active/completed/all. |
| `logs` | Fetch full thread history for a session. |
| `reconnect` | On startup, check for active sessions this client previously started. Offer to reattach. |
| `matrix` | Auth, sync, room discovery. Built on `mxdx-matrix` / WASM. |

### JS Layer (npm packages)

- CLI arg parsing in JS
- WASM for Matrix auth, E2EE, event parsing, thread operations
- JS for OS-level operations: process spawning (Node.js child_process), PTY (node-pty), terminal rendering

## CLI Interface

Identical for native `mxdx` binary and `npx @mxdx/cli`:

```
mxdx run <command> [args...]         fire-and-forget, output tailed until done
mxdx run -d <command> [args...]      detached, returns session UUID immediately
mxdx run -i <command> [args...]      interactive (PTY)
mxdx run --no-room-output ...        suppress stdout/stderr in Matrix thread
mxdx run --timeout 300 ...           5 minute timeout

mxdx exec <command> [args...]        alias for `mxdx run` (backward compat)

mxdx attach <uuid>                   reconnect to session (tail thread, future: acceleration)
mxdx ls                              list active sessions on target worker
mxdx ls --all                        include completed (within retention)
mxdx logs <uuid>                     fetch full thread output
mxdx logs <uuid> --follow            tail thread in real-time
mxdx cancel <uuid>                   send cancel event to session
mxdx cancel <uuid> --signal SIGKILL  send specific signal

mxdx worker start                    start worker (launcher mode)
mxdx coordinator start               start optional fleet coordinator
```

**Note:** `mxdx exec` is retained as an alias for `mxdx run` for backward compatibility with the existing client CLI.

## Coordinator Architecture

The coordinator is an optional, separately deployed service for multi-host fleet management. Single-host users never need it.

### Modules

| Module | Responsibility |
|---|---|
| `router` | Match TaskEvent `required_capabilities` against worker capability/telemetry state events. Select target worker room. |
| `watchlist` | Track in-flight tasks. Monitor heartbeats, detect timeouts. Uses `heartbeat_interval_seconds` from task event (triggers miss detection at `2 * heartbeat_interval_seconds`). |
| `failure` | Apply failure policies — escalate, respawn, respawn-with-context, abandon. From current fabric. `RespawnWithContext` uses the `plan` field from the task event to append failure context. |
| `claim` | Arbitrate claim races when multiple workers match. Last-write-wins via state events. |
| `index` | Capability index — maps capability sets to worker rooms. Dynamic room creation on first match. |

### Routing Flow

```
Client posts TaskEvent to coordinator room
  → router: find workers matching required_capabilities
  → If one worker: route directly to worker room
  → If multiple: post to shared capability room, workers race to claim
  → watchlist: track task, start heartbeat monitoring
  → On result: remove from watchlist, relay result to client thread
  → On timeout: apply on_timeout policy (default: Escalate)
  → On heartbeat miss (2x interval): apply on_heartbeat_miss policy (default: Escalate)
```

### What the Coordinator Does NOT Do

- Execute anything — it only routes
- Manage sessions — that's the worker's job
- Handle acceleration layer — that's client↔worker direct
- Store history — session state lives in worker rooms

## Migration Path

### Phase 1: Unified Types and Events
- Extend `mxdx-types` with unified `org.mxdx.session.*` events
- Deprecate `org.mxdx.fabric.*` and `org.mxdx.command` event types
- Add combined capability+telemetry state event type (`org.mxdx.worker.info`)

### Phase 2: Build `mxdx-worker`
New crate pulling from:
- `mxdx-fabric`: process spawning, heartbeat batching, capability advertisement
- `mxdx-launcher`: tmux management, PTY allocation, arg sanitization, telemetry collection

Add: session state events, retention cleanup, output routing with `--no-room-output`, start/result events, heartbeat/output separation.

Note: Acceleration layer (low-latency transport) is deferred to a future design. Phase 2 implements Matrix-thread-only output. The existing P2P Unix socket approach from fabric is retained as a same-host optimization.

### Phase 3: Build `mxdx-client`
New Rust crate pulling from:
- `mxdx-fabric`: SenderClient (task submission, result polling)
- `packages/client`: exec flow, shell attach, session listing

Add: thread tailing, reconnection, `ls`/`logs`/`attach`/`cancel` commands. WASM target for npm via `mxdx-core-wasm`.

### Phase 4: Refactor `mxdx-coordinator`
- Rename/refactor `mxdx-fabric` into `mxdx-coordinator`
- Strip out ProcessWorker and SenderClient (now in own crates)
- Keep: routing, watchlist, failure policies, capability index, claim arbitration
- Update to use new `org.mxdx.session.*` events and separated heartbeat/output

### Phase 5: Update npm Packages
- `@mxdx/core`: WASM bindings from `mxdx-core-wasm` (which builds from `mxdx-worker` + `mxdx-client` crate code)
- `@mxdx/launcher`: thin JS shell — process spawning via Node.js, Matrix via WASM
- `@mxdx/client`: thin JS shell — CLI parsing in JS, Matrix via WASM
- `@mxdx/cli`: dispatcher updated with new subcommands (`run`, `attach`, `ls`, `logs`, `cancel`)

### Phase 6: Deprecate Old Crates
- `mxdx-fabric` → absorbed into `mxdx-coordinator` + `mxdx-worker`
- `mxdx-launcher` → absorbed into `mxdx-worker`
- Old event types marked deprecated; worker understands both old and new events during transition

### Backward Compatibility
Workers running new code handle `org.mxdx.fabric.task` events from old clients during the transition period, translating them internally to the unified session model.

## Testing Strategy

### Unit Tests (per crate)
- `mxdx-worker`: session lifecycle, output batching, heartbeat posting, retention cleanup, tmux management, arg sanitization
- `mxdx-client`: task submission, thread parsing, session discovery, CLI arg handling
- `mxdx-coordinator`: capability matching, claim arbitration, failure policy application, watchlist timeout detection (using `heartbeat_interval_seconds`)
- `mxdx-types`: event serialization/deserialization roundtrips (including new session events)

### Integration Tests (Matrix required)
- Worker claims session, posts start event, spawns process, posts output to thread, completes with result event
- Client submits task, tails thread, receives start + output + result events
- Client disconnects mid-session, reconnects, resumes tailing
- `--no-room-output`: verify only start/heartbeat/result events in thread, no output content
- Heartbeat posted even during quiet periods (no output) and with `--no-room-output`
- Retention cleanup removes completed sessions past window
- Bidirectional: client posts input/signal/cancel to thread, worker receives and applies

### E2E Tests (Tuwunel instances + beta server credentials)
- Full flow: client → worker → process → output → client (both npm and native binary paths)
- Interactive session: attach with PTY, send input, receive output, disconnect, reattach via thread
- Coordinator routing: two workers with different capabilities, coordinator routes to correct one
- Fleet scenario: `mxdx ls` shows sessions across workers, `mxdx logs` fetches from correct thread
- Backward compatibility: old-format `org.mxdx.fabric.task` events handled by new worker
- Beta server test credentials from repo used for real-server validation

### Security Tests
- All session output E2EE encrypted
- Session state events use MSC4362 encrypted state
- Arg sanitization prevents command injection (shell metacharacters in bin, null bytes in args, traversal in cwd)
- `--no-room-output` doesn't leak content to Matrix
- `env` field validated for proper key format

## Future Work

### Low-Latency Acceleration Layer
A separate design document will specify a low-latency transport for interactive sessions. This will cover:
- Protocol selection (WebRTC DataChannels, custom WebSocket relay, or other)
- E2EE guarantees (end-to-end encryption between client and worker, not just to relay)
- Server provisioning and discovery
- Credential distribution
- Integration with the session model (transparent fallback to Matrix threads)

Until the acceleration layer is specified and implemented, interactive sessions operate via Matrix threads with the existing P2P Unix socket approach available as a same-host optimization.

### Event Versioning
The current design uses unversioned event types (`org.mxdx.session.task`). If schema evolution requires breaking changes, a versioning scheme (e.g., `org.mxdx.session.task.v2`) will be introduced. For the initial implementation, backward compatibility is handled by translating old `org.mxdx.fabric.*` events at the worker boundary.
