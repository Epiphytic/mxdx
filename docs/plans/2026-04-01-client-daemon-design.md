# Client Daemon Design

## Problem

Each `mxdx-client` CLI invocation creates a fresh Matrix connection, opens the SQLite crypto store, performs 3+ `sync_once()` calls, and tears everything down on exit. This causes:

1. **Crypto store corruption** — parallel CLI invocations race on SQLite schema migrations (`table "kv" already exists`)
2. **Slow startup** — session restore + sync takes 1-3 seconds per command
3. **Wasted resources** — repeated authentication, key exchange, and sync for every command
4. **No event streaming** — agents can't subscribe to events without maintaining their own connections

## Solution

A persistent daemon process per client profile that owns the Matrix connection, crypto store, and sync loop. CLI invocations connect to the daemon via Unix socket. Agents connect via WebSocket or MCP. The daemon multiplexes requests from multiple clients over a single Matrix connection.

## Architecture

```
┌─────────────┐  ┌─────────────┐  ┌─────────────┐
│ mxdx-client │  │  AI Agent   │  │ MCP Client  │
│   (CLI)     │  │ (WebSocket) │  │  (stdio)    │
└──────┬──────┘  └──────┬──────┘  └──────┬──────┘
       │                │                │
  Unix Socket     WebSocket:port     stdio
       │                │                │
       └────────┬───────┴────────┬───────┘
                │                │
         ┌──────┴────────────────┴──────┐
         │        Client Daemon         │
         │                              │
         │  Transport Adapters          │
         │  ┌──────┬──────────┬──────┐  │
         │  │ Unix │ WebSocket│ MCP  │  │
         │  └──┬───┴────┬─────┴──┬───┘  │
         │     └────────┼────────┘      │
         │        ┌─────┴─────┐         │
         │        │  Handler  │         │
         │        │  (core)   │         │
         │        └─────┬─────┘         │
         │        ┌─────┴─────┐         │
         │        │  Matrix   │         │
         │        │Connection │         │
         │        │ + Sync    │         │
         │        └───────────┘         │
         └──────────────────────────────┘
```

## Profiles and Identity

A **profile** is a named daemon configuration representing a set of Matrix accounts. Each profile gets its own daemon process, crypto store, and socket.

### File Layout

```
~/.mxdx/
├── defaults.toml          # all accounts
├── client.toml            # client config + profile definitions
└── daemon/
    ├── default.sock       # "default" profile daemon socket
    ├── default.pid        # PID file for stale detection
    ├── staging.sock       # "staging" profile daemon socket
    └── staging.pid
```

### Profile Configuration

In `client.toml`:

```toml
[daemon]
idle_timeout_seconds = 1200    # 0 = never auto-shutdown

[profiles.default]
# Uses all accounts from defaults.toml (omitting accounts = use all)

[profiles.staging]
accounts = ["@worker:staging.mxdx.dev"]

[profiles.prod-readonly]
accounts = ["@monitor:matrix.org"]
```

### CLI Behavior

- `mxdx-client run echo hello` — connects to `default` profile daemon
- `mxdx-client --profile staging run echo hello` — connects to `staging` daemon
- `mxdx-client --no-daemon run echo hello` — direct connection, no daemon
- `mxdx-client daemon start` — start default profile in foreground
- `mxdx-client daemon start --profile staging --detach` — start staging in background
- `mxdx-client daemon stop` — stop default daemon
- `mxdx-client daemon stop --all` — stop all daemons
- `mxdx-client daemon status` — list all running daemons with uptime, clients, sessions, transports
- `mxdx-client daemon mcp` — run MCP server on stdio (foreground, no daemon fork)

When no profiles are defined in `client.toml`, the `default` profile uses all accounts from `defaults.toml` plus any CLI-provided credentials. If the `default` daemon isn't running and you run a command, it auto-spawns.

**Credential mismatch:** If the CLI provides `--homeserver`/`--username` flags that don't match any account in the running daemon's profile, the daemon returns error code `-4` (Unauthorized). The user should either add that account to the profile's config, use `--profile` to target a different daemon, or use `--no-daemon` for a one-off direct connection.

## Protocol — JSON-RPC 2.0

All three transports speak the same JSON-RPC 2.0 protocol. Transport adapters handle framing; the core handler sees identical `Request` types regardless of source.

### Methods

**Task operations:**

| Method | Description |
|--------|-------------|
| `session.run` | Submit task, stream output + result |
| `session.cancel` | Cancel a running session |
| `session.signal` | Send signal to session |
| `session.attach` | Attach to interactive session |
| `session.ls` | List sessions |
| `session.logs` | Get/stream session output |

**Subscriptions:**

| Method | Description |
|--------|-------------|
| `events.subscribe` | Subscribe to event stream (with optional filters) |
| `events.unsubscribe` | Remove subscription |

**Daemon management:**

| Method | Description |
|--------|-------------|
| `daemon.status` | Uptime, clients, sessions, transports |
| `daemon.addTransport` | Add WebSocket/MCP listener at runtime |
| `daemon.removeTransport` | Remove a listener |
| `daemon.shutdown` | Graceful shutdown |

**Worker discovery:**

| Method | Description |
|--------|-------------|
| `worker.list` | List known workers + liveness status |
| `worker.capabilities` | Query worker capabilities |

### Streaming

JSON-RPC 2.0 distinguishes requests (have `id`, expect response) from notifications (no `id`, fire-and-forget). Streaming uses notifications from daemon to client:

```json
// CLI sends request
{"jsonrpc":"2.0","id":1,"method":"session.run","params":{"bin":"echo","args":["hello"]}}

// Daemon acknowledges with session UUID
{"jsonrpc":"2.0","id":1,"result":{"uuid":"abc-123","status":"accepted"}}

// Daemon streams output as notifications (no id)
{"jsonrpc":"2.0","method":"session.output","params":{"uuid":"abc-123","data":"aGVsbG8K","seq":0}}

// Daemon sends final result notification
{"jsonrpc":"2.0","method":"session.result","params":{"uuid":"abc-123","exit_code":0,"status":"success"}}
```

### Event Subscriptions

Agents subscribe to event streams with optional filters:

```json
// Subscribe to all session events
{"jsonrpc":"2.0","id":5,"method":"events.subscribe","params":{"events":["session.*"]}}
{"jsonrpc":"2.0","id":5,"result":{"subscription_id":"sub-001"}}

// Events pushed as notifications
{"jsonrpc":"2.0","method":"session.start","params":{"uuid":"xyz","worker_id":"...","bin":"ls"}}
{"jsonrpc":"2.0","method":"session.result","params":{"uuid":"xyz","exit_code":0}}

// Subscribe with filter (only output from one session)
{"jsonrpc":"2.0","id":6,"method":"events.subscribe","params":{"events":["session.output"],"filter":{"uuid":"abc-123"}}}
```

### Adding Transports at Runtime

```json
{"jsonrpc":"2.0","id":1,"method":"daemon.addTransport","params":{"type":"websocket","bind":"127.0.0.1:8765"}}
{"jsonrpc":"2.0","id":1,"result":{"address":"ws://127.0.0.1:8765"}}
```

### Error Codes

Standard JSON-RPC 2.0 error codes plus application-specific:

| Code | Meaning |
|------|---------|
| -32700 | Parse error |
| -32600 | Invalid request |
| -32601 | Method not found |
| -32602 | Invalid params |
| -32603 | Internal error |
| -1 | No worker found |
| -2 | Worker offline |
| -3 | Worker stale |
| -4 | Unauthorized |
| -5 | Session not found |
| -6 | Transport already exists |
| -7 | Matrix connection unavailable |

## Transport Adapters

### Unix Socket (default, always available)

- Path: `~/.mxdx/daemon/{profile}.sock`
- Framing: newline-delimited JSON-RPC (one JSON object per `\n`)
- Auto-spawned on first CLI invocation
- Used by: CLI, local scripts, local agents

### WebSocket (on-demand)

- Enabled via: `daemon.addTransport` call, `--enable-websocket` CLI flag, or `client.toml`
- Default bind: `127.0.0.1:{port}` (localhost only)
- Remote access: `--ws-bind 0.0.0.0:8765` (requires explicit opt-in)
- Framing: standard WebSocket text frames, each containing one JSON-RPC message
- Used by: AI agents, web UIs, remote tools

### MCP over stdio (on-demand)

- Enabled via: `mxdx-client daemon mcp` — runs MCP server on stdin/stdout (foreground, no daemon fork)
- Or: `daemon.addTransport` with `{"type":"mcp"}` to spawn an MCP subprocess
- Maps JSON-RPC methods to MCP tools: `session.run` becomes MCP tool `mxdx_run`
- Used by: Claude Code, Cursor, other MCP-compatible AI tools

## Daemon Lifecycle

### Auto-Start

1. CLI computes socket path: `~/.mxdx/daemon/{profile}.sock`
2. Try connecting — if successful, daemon is running, proceed
3. If connection fails, check PID file — if process is dead, remove stale `.sock` and `.pid`
4. Fork: `mxdx-client _daemon --profile {name} --detach`
5. Poll socket (100ms intervals, up to 10s) until daemon is ready
6. Connect and send request

### Idle Shutdown

- Timer starts when: no connected clients AND no active sessions
- Default: 1200 seconds (20 minutes)
- Timer resets on any new client connection
- `idle_timeout_seconds = 0` means never auto-shutdown
- On shutdown: post offline telemetry, close all transports, remove socket + PID file

### Error Handling

**Matrix connection drops:**
- Daemon reconnects with exponential backoff (1s → 30s), same as worker's `SyncBackoff`
- Connected clients receive notification: `{"method":"daemon.connectionStatus","params":{"status":"reconnecting","backoff_ms":4000}}`
- On reconnect: `{"method":"daemon.connectionStatus","params":{"status":"connected"}}`

**Client disconnects mid-stream:**
- Daemon keeps the session running (it's on the worker)
- Output buffered in a 64KB ring buffer per session
- Another CLI can `session.attach` or `session.logs --follow` to resume

**Daemon crashes:**
- Next CLI invocation detects stale socket, cleans up, spawns fresh daemon
- Worker sessions are unaffected — workers are independent processes
- Fresh daemon syncs room state and discovers active sessions

## Crate Structure

No new crates. Everything stays in `mxdx-client`:

```
crates/mxdx-client/src/
├── main.rs                 # CLI entry: parse args, connect to daemon or run direct
├── lib.rs                  # pub mod declarations
├── cli/
│   ├── mod.rs              # CLI arg definitions (clap)
│   ├── connect.rs          # Socket connection, auto-spawn, PID management
│   └── format.rs           # Format daemon responses for terminal output
├── daemon/
│   ├── mod.rs              # Daemon entry point (start, run main loop)
│   ├── handler.rs          # Core request handler (transport-agnostic)
│   ├── sessions.rs         # Active session tracking, output ring buffers
│   ├── subscriptions.rs    # Event subscription registry + filter dispatch
│   └── transport/
│       ├── mod.rs           # Transport trait + registry
│       ├── unix.rs          # Unix socket listener + NDJSON framing
│       ├── websocket.rs     # WebSocket listener + framing
│       └── mcp.rs           # MCP stdio adapter
├── protocol/
│   ├── mod.rs              # JSON-RPC 2.0 types (Request, Response, Notification)
│   ├── methods.rs          # Method enum, params/result types for each method
│   └── error.rs            # JSON-RPC error codes
├── matrix.rs               # Matrix connection (existing, used by daemon handler)
├── config.rs               # Config loading + profiles (existing, extended)
├── liveness.rs             # Worker liveness check (existing)
├── submit.rs               # Task building (existing)
├── tail.rs                 # Output formatting (existing)
├── logs.rs                 # Log reassembly (existing)
├── ls.rs                   # Session listing (existing)
├── cancel.rs               # Cancel building (existing)
└── attach.rs               # Attach logic (existing)
```

Existing modules are unchanged — they become library code called by `daemon/handler.rs` instead of directly by `main.rs`. With `--no-daemon`, `main.rs` calls the handler in-process (same code path, no socket).

## Method Parameters

### session.run

```json
{
  "bin": "echo",
  "args": ["hello", "world"],
  "cwd": "/tmp",
  "env": {"KEY": "value"},
  "interactive": false,
  "no_room_output": false,
  "timeout_seconds": 300,
  "heartbeat_interval": 30,
  "worker_room": "my-worker-room",
  "detach": false
}
```

All fields except `bin` are optional. When `detach` is true, the daemon returns the UUID immediately and does not stream output.

### session.ls

```json
{
  "all": false,
  "worker_room": "my-worker-room"
}
```

### session.logs

```json
{
  "uuid": "abc-123",
  "follow": false,
  "worker_room": "my-worker-room"
}
```

### session.cancel / session.signal

```json
{
  "uuid": "abc-123",
  "signal": "SIGTERM",
  "worker_room": "my-worker-room"
}
```

For `session.cancel`, `signal` is omitted. For `session.signal`, `signal` is required.

### events.subscribe

```json
{
  "events": ["session.*"],
  "filter": {
    "uuid": "abc-123",
    "worker_id": "device-xyz",
    "worker_room": "my-worker-room"
  }
}
```

`events` supports glob patterns: `session.*` matches `session.output`, `session.result`, etc. `filter` is optional; all fields within it are optional and ANDed together.

## Security

### Unix Socket

Protected by filesystem permissions. The socket is created with mode `0o600` (owner-only). No additional authentication needed — if you can connect, you're the same user.

### WebSocket

When bound to `127.0.0.1` (default), same-user protection applies via OS. When bound to `0.0.0.0` for remote access:

- Daemon generates a random bearer token on startup, written to `~/.mxdx/daemon/{profile}.token`
- WebSocket clients must send `Authorization: Bearer {token}` in the HTTP upgrade request
- Token file has mode `0o600` (owner-only)
- Remote access without a token is rejected

### MCP

MCP over stdio inherits the security of the parent process (the AI tool that spawned it). No additional auth needed.

## Crypto Store Isolation

Each (username, homeserver) pair gets its own SQLite crypto store directory and its own encryption passphrase in the OS keystore.

### Store Directory

```
~/.mxdx/crypto/{role}/{account_hash}/
```

Where `account_hash = short_hash("{username}@{homeserver}")`. This replaces the current `short_hash(homeserver)` which would collide if two users share the same server.

Example:
```
~/.mxdx/crypto/worker/a1b2c3d4/    ← hash of "alice@matrix.org"
~/.mxdx/crypto/worker/e5f6g7h8/    ← hash of "bob@matrix.org"
~/.mxdx/crypto/client/a1b2c3d4/    ← hash of "alice@matrix.org" (client role)
```

### Store Passphrase

Each account gets its own encryption passphrase stored in the OS keystore:

- Key: `mxdx:{username}@{normalized_server}:store_key`
- Value: random 32-byte hex string, generated on first use
- Used to encrypt all E2EE key material in that account's SQLite store

This is already the current passphrase key format — no change needed to the keychain keys, only to the directory hashing to include the username.
