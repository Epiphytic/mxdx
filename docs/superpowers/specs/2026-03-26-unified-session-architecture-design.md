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

Unify into a single session model where every command execution — fire-and-forget, long-running, or interactive — follows the same lifecycle. Output always flows to Matrix threads (ground truth). TURN provides transparent low-latency acceleration for interactive sessions. A client can disconnect and reconnect to any session.

## Crate Layout

### Rust Crates

| Crate | Purpose |
|---|---|
| `mxdx-worker` | Unified execution engine. Spawns processes in tmux, manages sessions, streams output to Matrix threads, handles TURN acceleration, maintains process table via state events, cleans up history past retention window. |
| `mxdx-client` | CLI for submitting commands, tailing output threads, attaching to interactive sessions (TURN), listing active/historical commands, reconnecting to disconnected sessions. |
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
  → tmux session created (always, even non-interactive)
  → Process spawns inside tmux
  → Output streams to Matrix thread (batched heartbeat events)
  → If interactive: TURN connection established
      → Output streams via TURN (low latency) AND Matrix thread (ground truth)
      → --no-room-output suppresses stdout/stderr in thread, but session
        start/end/exit-code events are ALWAYS posted
  → If client disconnects from TURN:
      → Output continues to Matrix thread automatically
      → tmux session preserves full scrollback on worker
  → Client reconnects:
      → Finds session via state event
      → Tails thread and/or re-establishes TURN
  → Process exits:
      → ResultEvent posted to thread (exit code, duration)
      → State event updated to session/{uuid}/completed
  → After retention period (default 90 days):
      → Completed state event removed by worker cleanup
      → Thread content remains in Matrix history
```

### Output Modes

| Mode | Matrix Thread | TURN | tmux |
|---|---|---|---|
| Default | Full stdout/stderr | If interactive | Always |
| `--no-room-output` | Session start/end/exit-code only | If interactive | Always |

With `--no-room-output`, all session metadata (command, who, when, exit code, duration) is always recorded in the thread. Only stdout/stderr content is suppressed. Output is still fully available via tmux scrollback on the worker, accessible through TURN reconnection.

### State Events as Process Table

| State Key | Content | Purpose |
|---|---|---|
| `session/{uuid}/active` | command, PID, start time, client ID, interactive flag | Running session |
| `session/{uuid}/completed` | exit code, duration, completion time | Finished session |
| `worker/{id}/info` | OS, arch, CPU, memory, disk, available tools/binaries, versions | Combined capability + telemetry advertisement |

- `mxdx ls` reads all `session/*/active` state events
- `mxdx ls --all` includes `session/*/completed` within retention window
- `mxdx logs <uuid>` fetches the thread from the task event
- `mxdx attach <uuid>` reads active state, re-establishes TURN if interactive, else tails thread

### Retention

The worker runs a periodic sweep (hourly) removing `session/*/completed` state events older than the configured retention window. Default: 90 days, configurable via `--history-retention 90d`. Thread content remains in Matrix room history — only the state event index is cleaned up.

## Event Schema

### Worker → Thread Events

```
org.mxdx.session.heartbeat
├── session_uuid: String
├── worker_id: String
├── stream: "stdout" | "stderr"
├── data: String                    // base64 encoded, batched
├── seq: u64                        // sequence number for ordering
├── timestamp: u64

org.mxdx.session.result
├── session_uuid: String
├── worker_id: String
├── status: "success" | "failed" | "timeout" | "cancelled"
├── exit_code: Option<i32>
├── duration_seconds: u64
├── tail: Option<String>            // last 50 lines for quick display
```

### Client → Thread Events

```
org.mxdx.session.input
├── session_uuid: String
├── data: String                    // base64 encoded stdin

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
```

### Task Submission Event (Thread Root)

```
org.mxdx.session.task
├── uuid: String
├── sender_id: String
├── command: String
├── args: Vec<String>
├── env: Option<Map<String, String>>
├── cwd: Option<String>
├── interactive: bool               // request PTY + TURN
├── no_room_output: bool            // suppress stdout/stderr in thread
├── timeout_seconds: Option<u64>    // None = no timeout
├── required_capabilities: Vec<String>  // for coordinator routing
├── routing_mode: "direct" | "brokered" | "auto"  // coordinator only
├── failure_policy: Option<FailurePolicy>          // coordinator only
```

## Worker Internal Architecture

### Modules

| Module | Responsibility |
|---|---|
| `session` | Session lifecycle — create, track, complete, cleanup. Owns state event process table. |
| `executor` | Process spawning inside tmux — child process with optional PTY allocation. Arg sanitization. |
| `output` | Output routing — multiplexes stdout/stderr to Matrix thread (batched) and/or TURN stream. Respects `no_room_output`. |
| `turn` | TURN connection management — establish, teardown, reconnect. Transparent to executor. |
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
| `attach` | Establish TURN connection to active session's tmux. Falls back to `tail` if TURN unavailable. |
| `ls` | Read session state events from worker room. Format as process table. Filter active/completed/all. |
| `logs` | Fetch full thread history for a session. |
| `reconnect` | On startup, check for active sessions this client previously started. Offer to reattach. |
| `matrix` | Auth, sync, room discovery. Built on `mxdx-matrix` / WASM. |

### JS Layer (npm packages)

- CLI arg parsing in JS
- WASM for Matrix auth, E2EE, event parsing, thread operations
- JS for OS-level operations: TURN client (WebRTC/websocket), process spawning (Node.js child_process), PTY (node-pty), terminal rendering

## CLI Interface

Identical for native `mxdx` binary and `npx @mxdx/cli`:

```
mxdx run <command> [args...]         fire-and-forget, output tailed until done
mxdx run -d <command> [args...]      detached, returns session UUID immediately
mxdx run -i <command> [args...]      interactive (PTY + TURN)
mxdx run --no-room-output ...        suppress stdout/stderr in Matrix thread
mxdx run --timeout 300 ...           5 minute timeout

mxdx attach <uuid>                   reconnect to session (TURN if interactive, else tail)
mxdx ls                              list active sessions on target worker
mxdx ls --all                        include completed (within retention)
mxdx logs <uuid>                     fetch full thread output
mxdx logs <uuid> --follow            tail thread in real-time

mxdx worker start                    start worker (launcher mode)
mxdx coordinator start               start optional fleet coordinator
```

## Coordinator Architecture

The coordinator is an optional, separately deployed service for multi-host fleet management. Single-host users never need it.

### Modules

| Module | Responsibility |
|---|---|
| `router` | Match TaskEvent `required_capabilities` against worker capability/telemetry state events. Select target worker room. |
| `watchlist` | Track in-flight tasks. Monitor heartbeats, detect timeouts. |
| `failure` | Apply failure policies — escalate, respawn, respawn-with-context, abandon. From current fabric. |
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
  → On timeout: failure policy kicks in
```

### What the Coordinator Does NOT Do

- Execute anything — it only routes
- Manage sessions — that's the worker's job
- Handle TURN — that's client↔worker direct
- Store history — session state lives in worker rooms

## Migration Path

### Phase 1: Unified Types and Events
- Extend `mxdx-types` with unified `org.mxdx.session.*` events
- Deprecate `org.mxdx.fabric.*` and `org.mxdx.command` event types
- Add combined capability+telemetry state event type

### Phase 2: Build `mxdx-worker`
New crate pulling from:
- `mxdx-fabric`: process spawning, heartbeat batching, capability advertisement, P2P streaming
- `mxdx-launcher`: tmux management, PTY allocation, arg sanitization, telemetry collection

Add: session state events, retention cleanup, output routing with `--no-room-output`, TURN integration.

### Phase 3: Build `mxdx-client`
New Rust crate pulling from:
- `mxdx-fabric`: SenderClient (task submission, result polling)
- `packages/client`: exec flow, shell attach, session listing

Add: thread tailing, TURN attach, reconnection, `ls`/`logs`/`attach` commands. WASM target for npm.

### Phase 4: Refactor `mxdx-coordinator`
- Rename/refactor `mxdx-fabric` into `mxdx-coordinator`
- Strip out ProcessWorker and SenderClient (now in own crates)
- Keep: routing, watchlist, failure policies, capability index, claim arbitration

### Phase 5: Update npm Packages
- `@mxdx/core`: WASM bindings from `mxdx-worker` + `mxdx-client`
- `@mxdx/launcher`: thin JS shell — process spawning via Node.js, Matrix via WASM
- `@mxdx/client`: thin JS shell — CLI parsing in JS, Matrix via WASM
- `@mxdx/cli`: dispatcher unchanged

### Phase 6: Deprecate Old Crates
- `mxdx-fabric` → absorbed into `mxdx-coordinator` + `mxdx-worker`
- `mxdx-launcher` → absorbed into `mxdx-worker`
- Old event types marked deprecated; worker understands both old and new events during transition

### Backward Compatibility
Workers running new code handle `org.mxdx.fabric.task` events from old clients during the transition period, translating them internally to the unified session model.

## Testing Strategy

### Unit Tests (per crate)
- `mxdx-worker`: session lifecycle, output batching, retention cleanup, tmux management, arg sanitization
- `mxdx-client`: task submission, thread parsing, session discovery, CLI arg handling
- `mxdx-coordinator`: capability matching, claim arbitration, failure policy application, watchlist timeout detection
- `mxdx-types`: event serialization/deserialization roundtrips

### Integration Tests (Matrix required)
- Worker claims session, spawns process, posts output to thread, completes with result event
- Client submits task, tails thread, receives result
- Client disconnects mid-session, reconnects, resumes tailing
- `--no-room-output`: verify only start/end events in thread, no stdout content
- TURN establishment and fallback to thread-only on TURN failure
- Retention cleanup removes completed sessions past window
- Bidirectional: client posts input/signal to thread, worker receives and applies

### E2E Tests (Tuwunel instances + beta server credentials)
- Full flow: client → worker → process → output → client (both npm and native binary paths)
- Interactive session: attach with PTY, send input, receive output via TURN, disconnect, reattach
- Coordinator routing: two workers with different capabilities, coordinator routes to correct one
- Fleet scenario: `mxdx ls` shows sessions across workers, `mxdx logs` fetches from correct thread
- Backward compatibility: old-format `org.mxdx.fabric.task` events handled by new worker
- Beta server test credentials from repo used for real-server validation

### Security Tests
- All session output E2EE encrypted
- Session state events use MSC4362 encrypted state
- TURN traffic encrypted
- Arg sanitization prevents command injection
- `--no-room-output` doesn't leak content to Matrix
