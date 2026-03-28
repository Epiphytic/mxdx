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

Unify into a single session model where every command execution — fire-and-forget, long-running, or interactive — follows the same lifecycle. Output always flows to Matrix threads (ground truth). WebRTC DataChannels transparently accelerate interactive sessions with application-level E2EE. A client can disconnect and reconnect to any session.

## Crate Layout

### Rust Crates

| Crate | Purpose |
|---|---|
| `mxdx-worker` | Unified execution engine. Spawns processes in tmux, manages sessions, streams output to Matrix threads, WebRTC DataChannels for interactive sessions, maintains process table via state events, cleans up history past retention window. |
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
  → If interactive: WebRTC DataChannel established (see WebRTC Acceleration Layer)
      → Output streams via DataChannel (low latency) AND Matrix thread (ground truth)
      → --no-room-output suppresses stdout/stderr in thread, but session
        start/end/exit-code events and heartbeats are ALWAYS posted
  → If client disconnects:
      → Output continues to Matrix thread automatically
      → tmux session preserves full scrollback on worker
  → Client reconnects:
      → Finds session via state event
      → Tails thread and/or re-establishes WebRTC DataChannel
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

| Mode | Matrix Thread | WebRTC DataChannel | tmux |
|---|---|---|---|
| Default | Full stdout/stderr + heartbeats | If interactive | Always |
| `--no-room-output` | Start/end/exit-code + heartbeats only | If interactive | Always |

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
- `mxdx attach <uuid>` reads active state, re-establishes WebRTC DataChannel if interactive, else tails thread

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

**`interactive` field:** This replaces the old `p2p_stream` field from `fabric::TaskEvent`. The semantic change is intentional: `p2p_stream` described a transport mechanism, `interactive` describes the session mode (PTY allocation). WebRTC is automatically initiated for interactive sessions (see WebRTC Acceleration Layer section).

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
| `identity` | Device key management via OS keychain. Stable device ID across restarts. |
| `trust` | Trust store management. Cross-signing, trust anchor, trust list propagation. Invitation/task filtering by trusted device IDs. |
| `webrtc` | WebRTC DataChannel management. App-level E2EE key derivation, payload encryption, ICE state monitoring, automatic failover to Matrix thread. Signaling via thread metadata + to-device messages. |
| `config` | TOML config loading (defaults.toml + worker.toml). CLI argument merging. |
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
| `attach` | Attach to active session. WebRTC DataChannel for interactive, thread-tailing for non-interactive. Falls back gracefully. |
| `ls` | Read session state events from worker room. Format as process table. Filter active/completed/all. |
| `logs` | Fetch full thread history for a session. |
| `reconnect` | On startup, check for active sessions this client previously started. Offer to reattach. |
| `identity` | Device key management via OS keychain. Stable device ID across restarts. |
| `trust` | Trust store, cross-signing ceremony initiation, trust list exchange. `mxdx trust` subcommands. |
| `config` | TOML config loading (defaults.toml + client.toml). CLI argument merging. |
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

mxdx attach <uuid>                   reconnect to session (WebRTC if interactive, else tail thread)
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
- Handle WebRTC — that's client↔worker direct
- Store history — session state lives in worker rooms

## Configuration

### File Layout

All configuration lives in `$HOME/.mxdx/`:

```
~/.mxdx/
├── defaults.toml          # Shared defaults (applies to all modes)
├── client.toml            # Client-specific config
├── worker.toml            # Worker-specific config
├── coordinator.toml       # Coordinator-specific config
```

**Precedence:** CLI arguments > mode-specific TOML > defaults.toml.

### `defaults.toml` (shared)

```toml
# Matrix account(s) — used by all modes unless overridden
[[accounts]]
user_id = "@alice:example.com"
homeserver = "https://example.com"

# Optional: multiple accounts for multi-homeserver redundancy
[[accounts]]
user_id = "@alice:backup.example.com"
homeserver = "https://backup.example.com"

# Trust settings (apply to all modes)
[trust]
# "auto" = automatically trust verified device IDs from trusted identities
# "manual" = require manual approval for each new device ID
cross_signing_mode = "auto"
```

### `worker.toml`

```toml
# Override or extend accounts from defaults.toml
# If omitted, inherits from defaults.toml

# Worker room naming: defaults to {hostname}.{username}.{account}
# e.g., "prod-web-01.deploy.alice" — unless overridden here or by coordinator
room_name = ""  # empty = use default naming

# Trust anchor — the Matrix identity trusted at bootstrap
# In single-host mode, this is typically the client user
# In fleet mode, this is typically the coordinator
trust_anchor = "@alice:example.com"

# Session retention
history_retention = "90d"

# Capabilities (auto-detected + manual additions)
[capabilities]
extra = ["docker", "gpu"]  # added to auto-detected capabilities

# Telemetry refresh interval
telemetry_refresh_seconds = 300
```

### `client.toml`

```toml
# Default target worker (for single-host mode, direct connection)
default_worker_room = ""  # empty = discover via room list

# Default coordinator (for fleet mode)
coordinator_room = ""

# Session defaults
[session]
timeout_seconds = 0        # 0 = no timeout
heartbeat_interval = 30
interactive = false
no_room_output = false
```

### `coordinator.toml`

```toml
# Coordinator room
room = ""  # empty = auto-create

# Capability room prefix
capability_room_prefix = "workers"

# Failure policy defaults
[failure]
default_on_timeout = "escalate"
default_on_heartbeat_miss = "escalate"
```

### CLI Override

Every configuration value has a corresponding CLI flag. Examples:

```
mxdx worker start --trust-anchor @coordinator:example.com
mxdx worker start --history-retention 30d
mxdx worker start --cross-signing-mode manual
mxdx run --timeout 300 --no-room-output echo hello
mxdx client --coordinator-room '!abc:example.com' run echo hello
```

## Identity & Key Management

### Device Identity

Each (host, OS user, Matrix account) tuple gets exactly one Matrix device ID. The device's keys (Ed25519 signing key, Curve25519 identity key) are stored in the OS keychain (e.g., libsecret on Linux, Keychain on macOS, Credential Manager on Windows). This ensures:

- Devices are created only once per (host, user, account) — no device proliferation from restarts
- Keys survive process restarts without re-login
- Keys are encrypted at rest by the OS keychain

**Keychain entry naming:** `mxdx/{user_id}/{device_id}` — contains the serialized crypto store or a reference to it.

### Room Naming (Worker)

Worker rooms are named by default using the pattern: `{hostname}.{username}.{matrix_account_localpart}`

Examples:
- `prod-web-01.deploy.alice` — host "prod-web-01", OS user "deploy", Matrix user "@alice:example.com"
- `dev-laptop.liam.liamhelmer` — host "dev-laptop", OS user "liam", Matrix user "@liamhelmer:matrix.org"

This allows multiple workers on the same Matrix account (different hosts or OS users) without room name collisions. The room name can be overridden in `worker.toml` or by the coordinator.

## Trust Model

### Overview

The trust model is built on Matrix cross-signing. Each mxdx device (worker, client, coordinator) has a device ID tied to a Matrix account. Trust is established through cross-signing between devices, with trust lists stored in the OS keychain.

### Trust Store

Each device maintains a trust store in the OS keychain containing:
- Its own device keys
- A list of trusted device IDs (cross-signed)
- The trust anchor identity (Matrix user ID)

**Keychain entry:** `mxdx/{user_id}/trust-store` — contains the list of trusted device IDs and their signing keys.

### Trust Topology

**Single-host mode:**
```
Client (trust anchor)
  ↓ cross-signs
Worker
```
- Client's Matrix identity is the worker's trust anchor
- Worker trusts all verified device IDs belonging to the client's Matrix identity at bootstrap
- Client creates/joins the worker room directly

**Fleet/coordinator mode:**
```
Coordinator (trust anchor)
  ↓ cross-signs          ↓ cross-signs
Worker A               Worker B
  ↑ cross-signs (via coordinator's trust list)
Client
```
- Coordinator's Matrix identity is the workers' trust anchor
- Workers trust all verified device IDs belonging to the coordinator's Matrix identity at bootstrap
- Coordinator creates capability rooms, invites workers
- Workers accept invitations only from trusted device IDs
- During client↔coordinator cross-signing, the coordinator sends its trust list to the client

### Cross-Signing Modes

| Mode | Behavior | Use Case |
|---|---|---|
| `auto` (default) | Automatically trust all verified device IDs belonging to trusted Matrix identities. New devices from trusted identities are cross-signed without interaction. | Most deployments — low friction, relies on Matrix identity verification. |
| `manual` | Each new device ID requires explicit CLI approval before cross-signing. | High-security environments where every device must be individually vetted. |

Configurable per-mode in `defaults.toml`, `worker.toml`, `client.toml`, or `coordinator.toml`. CLI override: `--cross-signing-mode manual`.

### Bootstrap Flow

**Worker bootstrap (single-host):**
```
1. Worker starts for the first time
2. Reads trust_anchor from worker.toml (e.g., @alice:example.com)
3. Logs into Matrix, creates device ID, stores keys in OS keychain
4. Fetches verified device IDs for @alice:example.com
5. Cross-signs all verified devices (auto mode) or prompts for each (manual mode)
6. Creates worker room (using default naming or config override)
7. Ready to accept tasks from trusted devices
```

**Worker bootstrap (fleet mode):**
```
1. Worker starts for the first time
2. Reads trust_anchor from worker.toml (e.g., @coordinator:fleet.example.com)
3. Logs into Matrix, creates device ID, stores keys in OS keychain
4. Fetches verified device IDs for @coordinator:fleet.example.com
5. Cross-signs coordinator's devices (auto or manual)
6. Waits for coordinator to invite it to capability rooms
7. Accepts invitations from trusted device IDs only
8. Ready to accept tasks
```

### Cross-Signing Ceremony

When a worker or coordinator cross-signs with a client, there is a manual approval step:

```
1. Client initiates: mxdx trust add --device <worker_device_id>
   (or: worker initiates: mxdx worker trust add --device <client_device_id>)
2. Both sides confirm the device fingerprint (displayed on CLI)
3. Cross-signing keys are exchanged
4. The initiator (client or coordinator) sends its trust list to the worker
5. Worker automatically cross-signs all devices in the received list
   (in auto mode — in manual mode, each requires approval)
```

**Trust list propagation is one-directional:** The initiator sends its list to the worker. The worker does NOT automatically send its list back. A client can manually pull the worker's trust list via `mxdx trust pull --from <worker_device_id>`, but this is not the default.

### Room Trust Enforcement

- A worker room must be created by a trusted device ID (the worker itself, or a trusted coordinator)
- Workers reject task events from untrusted device IDs
- Workers reject room invitations from untrusted device IDs
- All room events are E2EE — untrusted devices cannot read room content even if they somehow join

### Trust CLI Commands

```
mxdx trust list                          list trusted device IDs
mxdx trust add --device <device_id>      initiate cross-signing with a device
mxdx trust remove --device <device_id>   revoke trust for a device
mxdx trust pull --from <device_id>       pull trust list from a trusted device
mxdx trust anchor                        show current trust anchor identity
mxdx trust anchor set <user_id>          set trust anchor (requires restart)
```

## Migration Path

### Phase 1: Unified Types, Events, and Identity
- Extend `mxdx-types` with unified `org.mxdx.session.*` events
- Deprecate `org.mxdx.fabric.*` and `org.mxdx.command` event types
- Add combined capability+telemetry state event type (`org.mxdx.worker.info`)
- Implement TOML configuration loading (defaults.toml + mode-specific files)
- Implement OS keychain integration for device keys and trust store
- Implement trust model: trust anchor, cross-signing, trust list propagation

### Phase 2: Build `mxdx-worker`
New crate pulling from:
- `mxdx-fabric`: process spawning, heartbeat batching, capability advertisement
- `mxdx-launcher`: tmux management, PTY allocation, arg sanitization, telemetry collection

Add: session state events, retention cleanup, output routing with `--no-room-output`, start/result events, heartbeat/output separation.

Add: WebRTC DataChannel support for interactive sessions with app-level E2EE, automatic failover to Matrix threads on disconnect.

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
- Trust bootstrap: worker trusts anchor identity's verified devices on first start
- Cross-signing ceremony: client and worker exchange fingerprints, trust established
- Trust list propagation: worker receives and cross-signs initiator's trust list
- Manual mode: worker prompts for approval on each new device ID
- Room invitation rejection: worker refuses invitation from untrusted device
- Config loading: CLI args override TOML, mode-specific overrides defaults

### E2E Tests (Tuwunel instances + beta server credentials)
- Full flow: client → worker → process → output → client (both npm and native binary paths)
- Interactive session: WebRTC DataChannel with PTY, send input, receive output via DataChannel
- WebRTC failover: disconnect DataChannel mid-session, verify output continues on Matrix thread
- WebRTC reconnection: re-establish DataChannel, verify output resumes with no duplicates (seq dedup)
- WebRTC upgrade: non-interactive session upgraded to interactive via `mxdx attach -i`
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
- Device keys stored in OS keychain, not on filesystem
- Worker rejects task events from untrusted device IDs
- Worker rejects room invitations from untrusted device IDs
- Cross-signing ceremony requires fingerprint confirmation
- Manual cross-signing mode blocks automatic trust propagation
- Trust list propagation is one-directional (initiator→worker only)
- Device identity is stable across restarts (no device proliferation)
- WebRTC signaling: no cryptographic material in thread events (SDP/keys only in to-device messages)
- WebRTC app-level E2EE: TURN relay cannot read DataChannel payloads
- WebRTC ephemeral keys: fresh key pair per connection (no key reuse across reconnections)

## WebRTC Acceleration Layer

### Overview

WebRTC DataChannels provide low-latency P2P communication for interactive sessions. Matrix threads remain the ground truth for all output. WebRTC is a transparent acceleration layer — sessions work identically without it, just with higher latency.

Interactive sessions (`interactive: true`) automatically trigger WebRTC setup. Non-interactive sessions use Matrix threads only, but can be upgraded to interactive via `mxdx attach -i <uuid>`. Future enhancement: heuristic-based auto-upgrade for high-throughput non-interactive output.

### Encryption (Two Layers)

1. **DTLS** — WebRTC's built-in transport encryption. Protects P2P traffic. For TURN-relayed traffic, DTLS terminates at each hop (relay can see plaintext at the DTLS level).
2. **Application-level E2EE** — Using existing Matrix device keys (Curve25519) to derive a shared secret between the two devices via ephemeral key exchange. All DataChannel payloads encrypted with this key before being sent. TURN relay sees only ciphertext. This layer is always active, regardless of whether traffic is P2P or relayed.

This double encryption ensures the E2EE guarantee holds regardless of network topology, consistent with the project's strict encryption requirements.

### Signaling (Split Model)

Signaling is split between room thread events and to-device messages to prevent cryptographic material from being exposed in the room:

**Thread events (metadata only, auditable):**
- Announce *that* a WebRTC connection is being established
- Contain only session UUID, device IDs, and timestamp
- No cryptographic material (no SDP, no DTLS fingerprints, no keys)

**To-device messages (private, encrypted by Olm):**
- Full SDP offer/answer (with DTLS fingerprints)
- ICE candidates (trickle ICE)
- Application-level E2EE ephemeral public keys

This split ensures the room log shows when WebRTC connections were established between which devices (auditability), while the actual cryptographic handshake remains private between the two devices.

### ICE Configuration

Default STUN servers are public (for NAT traversal). TURN servers are user-configured, with credentials typically provided by the matrix-hosting auth infrastructure.

```toml
# In defaults.toml, worker.toml, client.toml, or coordinator.toml
[webrtc]
stun_servers = ["stun:stun.l.google.com:19302", "stun:stun.mozilla.com"]

[[webrtc.turn_servers]]
url = "turn:turn.example.com:3478"
# Credentials provided by matrix-hosting auth endpoint
auth_endpoint = "https://hosting.example.com/turn/credentials"
```

Overridable via CLI flags (e.g., `--stun-server`, `--turn-server`, `--turn-auth-endpoint`).

### Connection Lifecycle

**Initiation (automatic for interactive sessions):**

```
Client sends TaskEvent with interactive: true
  → Worker claims session, starts process in tmux with PTY
  → Worker posts org.mxdx.session.webrtc.offer to thread
    (metadata only: session_uuid, worker_device_id, timestamp)
  → Worker sends full SDP offer + app-level E2EE ephemeral key via to-device message
  → Client receives to-device message
  → Client posts org.mxdx.session.webrtc.answer to thread (metadata only)
  → Client sends SDP answer + E2EE ephemeral key via to-device message
  → ICE candidates exchanged via to-device messages
  → DataChannel established, app-level E2EE active
  → Output streams via DataChannel (low latency) AND Matrix thread (ground truth)
```

**Failover (automatic on disconnect):**

```
WebRTC ICE state → disconnected/failed
  → Worker continues posting output to Matrix thread (already happening)
  → Worker stops sending on DataChannel
  → Client detects disconnect, falls back to tailing Matrix thread
  → Output continuity maintained via seq field on output events (no duplicates)
```

**Reconnection (automatic):**

```
Client detects network recovery or runs `mxdx attach -i <uuid>`
  → New signaling round: thread metadata event + to-device key exchange
  → New DataChannel established with fresh E2EE ephemeral keys
  → Output switches back to DataChannel
  → Client merges thread + DataChannel output using seq numbers (no duplicates)
```

**Upgrade (non-interactive → interactive):**

```
$ mxdx attach -i <uuid>
  → Client posts webrtc.offer to thread (metadata)
  → To-device key exchange with worker
  → DataChannel established
  → tmux session already exists (all sessions use tmux)
  → Client now has full PTY access via DataChannel
```

### Thread Events (Metadata Only)

```
org.mxdx.session.webrtc.offer
├── session_uuid: String
├── device_id: String              // offering device
├── timestamp: u64

org.mxdx.session.webrtc.answer
├── session_uuid: String
├── device_id: String              // answering device
├── timestamp: u64
```

No cryptographic material. These exist solely for auditability.

### To-Device Messages

```
org.mxdx.webrtc.sdp
├── session_uuid: String
├── type: "offer" | "answer"
├── sdp: String                    // full SDP with DTLS fingerprints
├── e2ee_public_key: String        // Curve25519 ephemeral key for app-level E2EE

org.mxdx.webrtc.ice
├── session_uuid: String
├── candidate: String              // ICE candidate
├── sdp_mid: String
├── sdp_mline_index: u32
```

These are already encrypted by Matrix's to-device encryption (Olm). The app-level E2EE key negotiation piggybacks on the SDP exchange.

### Worker Module

The worker's `webrtc` module handles:
- DataChannel creation and management
- App-level E2EE key derivation from ephemeral Curve25519 exchange
- Payload encryption/decryption on DataChannel
- ICE state monitoring and automatic failover to Matrix thread
- Signaling event posting (thread metadata) and to-device message handling

### Client Module

The client's `attach` module handles:
- WebRTC connection initiation (for interactive sessions and `mxdx attach -i`)
- App-level E2EE key derivation
- Payload encryption/decryption on DataChannel
- Output merging: deduplicates thread + DataChannel output using seq numbers
- Automatic reconnection on network recovery
- Graceful fallback to thread-only when WebRTC is unavailable

## Future Work

### Heuristic-Based WebRTC Auto-Upgrade
Non-interactive sessions could automatically upgrade to WebRTC when output rate exceeds a threshold (e.g., high-throughput log streaming). This would be opt-in via configuration.

### Event Versioning
The current design uses unversioned event types (`org.mxdx.session.task`). If schema evolution requires breaking changes, a versioning scheme (e.g., `org.mxdx.session.task.v2`) will be introduced. For the initial implementation, backward compatibility is handled by translating old `org.mxdx.fabric.*` events at the worker boundary.
