# mxdx Management Console — Device & Server Management over Matrix

**Date:** 2026-03-04
**Status:** Draft
**Based on:** [mxdx Architecture](./mxdx-architecture.md) (2026-02-27)

---

## 1. Overview

The mxdx Management Console extends the mxdx agent orchestration framework with an interactive management interface. It adds a browser-based dashboard for fleet monitoring, host observability, and interactive terminal access (xterm.js) — all transported over E2EE Matrix rooms.

This document describes the **management console additions** to mxdx. It assumes the core mxdx architecture (Policy Agent, Secrets Coordinator, Launcher, Worker Agents, Tuwunel homeserver) as its foundation and does not duplicate that material.

### What This Adds to mxdx

| Capability | mxdx Core | This Design |
|:---|:---|:---|
| Command execution | Non-interactive, UUID-threaded | Interactive terminal sessions (xterm.js, tmux) |
| Host telemetry | Basic (CPU, memory, disk, GPU, IOPs) | Extended (network interfaces, routing tables, per-interface stats, devices, services, OTel) |
| User interface | None (Matrix rooms are the UI) | HTMX dashboard + PWA for Android |
| Browser E2EE | N/A | Full Matrix client in browser via `matrix-sdk-crypto-wasm` |
| Multi-homeserver launcher | N/A | Launcher registers on 2-3 federated homeservers with automatic failover |
| Packaging | Unspecified | WASI binary published to npm, zero-install via `npx` |

### Principles (inherited from mxdx)

- **No ambient credentials.** Launchers hold only their Matrix device keys.
- **Fail closed.** If the Policy Agent is down, launchers cannot access rooms or obtain credentials.
- **Ephemeral by default.** Worker agents are per-task and tombstoned after completion.
- **Auditable.** Every command, result, and secret request is a signed event in a Matrix room DAG.
- **Host-agnostic.** A launcher runs anywhere — `npx @mxdx/launcher` on any machine with Node.js.

---

## 2. Components

### 2.1 mxdx Core (existing)

These components are defined in the mxdx architecture document:

- **Tuwunel** — Matrix homeserver (Rust, RocksDB, built-in OIDC with rich claims)
- **Policy Agent** — Matrix appservice enforcing fail-closed access control over the `@agent-*:mxdx` namespace
- **Secrets Coordinator** — HSM-backed secret broker with dynamic (STS) and static (age-encrypted) secret support
- **Orchestrator** — Plans and delegates work, makes placement decisions based on host telemetry

### 2.2 Launcher (extended)

The mxdx Launcher is extended with:

- **Interactive terminal sessions** — Spawns tmux sessions and bridges PTY I/O to Matrix events
- **Multi-homeserver identity** — Registers on 2-3 federated homeservers for transport redundancy
- **Extended telemetry** — Network interfaces, routing tables, per-interface transport stats, services, devices
- **OpenTelemetry integration** — Optional OTel collector subprocess for specialized metrics
- **Admin provisioning** — Configured admin accounts are invited to the control room at power level 100

### 2.3 `mxdx-client` (JS/TS library, runs in browser)

A browser-native Matrix client using `matrix-sdk-crypto-wasm` for E2EE. Provides APIs for:

- Launcher discovery and status monitoring
- Session lifecycle management (terminal + non-interactive)
- Terminal I/O via a WebSocket-compatible API for xterm.js
- Host observability (reading `org.mxdx.host_telemetry` state events)
- Secret request forwarding (for browser-initiated workflows)

### 2.4 Web App (Rust + HTMX + PWA)

A lightweight, edge-deployable progressive web application. Runs separately from the Tuwunel homeservers — designed for Cloudflare Workers (Rust → WASM) or any edge runtime:

- **Management dashboard (HTMX + React islands)** — Fleet overview, per-host status, session management, observability panels. HTMX for the shell, React components for complex visualizations
- **Terminal view (xterm.js)** — Interactive terminal connected directly to the client library
- **PWA** — Installable on Android, offline app shell caching, push notifications for launcher status changes
- **Stateless** — The web service holds no Matrix credentials and never sees terminal traffic. All E2EE flows directly between browser and Tuwunel

### Architecture Diagram

```
┌────────────────── Browser / PWA ──────────────────┐
│                                                   │
│  ┌─ Management Dashboard (HTMX) ──────────────┐  │
│  │  Fleet overview                             │  │
│  │  Host status & observability                │  │
│  │  Session management                         │  │
│  │  Policy & audit views                       │  │
│  └─────────────────────────────────────────────┘  │
│                                                   │
│  ┌─ Terminal View ─────────────────────────────┐  │
│  │  xterm.js ←WS API→ mxdx-client        │  │
│  └─────────────────────────────────────────────┘  │
│                                                   │
│  mxdx-client (JS/TS + WASM)                 │
│  matrix-sdk-crypto-wasm (E2EE)                    │
└──────────┬────────────────────────────────────────┘
           │ E2EE Matrix events
     Tuwunel Homeservers (federated)
           │
           ├── Policy Agent (appservice, fail-closed)
           ├── Secrets Coordinator (HSM-backed)
           │
┌──────────┴────────────────────────────────────────┐
│  Launcher (Rust → WASI)                           │
│  Multi-homeserver Matrix client                   │
│       ↕                                           │
│  ┌─ Management ────────────────────────────────┐  │
│  │  Host telemetry collection                  │  │
│  │  Capability enforcement (policy-backed)     │  │
│  │  Session lifecycle                          │  │
│  └─────────────────────────────────────────────┘  │
│  ┌─ Command Execution ─────────────────────────┐  │
│  │  UUID-threaded commands (non-interactive)   │  │
│  │  Stdout/stderr streaming as threaded replies │  │
│  └─────────────────────────────────────────────┘  │
│  ┌─ Terminal ──────────────────────────────────┐  │
│  │  tmux sessions (interactive)                │  │
│  │    ├─ interactive shell                     │  │
│  │    ├─ long-running script                   │  │
│  │    └─ any allowed command                   │  │
│  └─────────────────────────────────────────────┘  │
│  ┌─ Telemetry (optional) ──────────────────────┐  │
│  │  OpenTelemetry collector subprocess         │  │
│  └─────────────────────────────────────────────┘  │
│  ┌─ Workers ───────────────────────────────────┐  │
│  │  Ephemeral @worker-{uuid}:mxdx agents │  │
│  │  Spawned per-task, tombstoned after         │  │
│  └─────────────────────────────────────────────┘  │
└───────────────────────────────────────────────────┘
```

---

## 3. Room Topology (extending mxdx)

Extends the mxdx room topology with terminal session rooms:

```
Space: mxdx Infrastructure
├── #policy:mxdx                          — Policy rules (state events)
├── #secrets-audit:mxdx                   — Audit log of all secret access
├── #orchestrator-control:mxdx            — Orchestrator commands/status
│
├── Space: Launchers
│   ├── Space: launcher-belthanior
│   │   ├── #launcher-belthanior-exec:mxdx    — Command execution (non-interactive)
│   │   ├── #launcher-belthanior-status:mxdx  — Telemetry (state events)
│   │   ├── #launcher-belthanior-logs:mxdx    — System logs
│   │   └── #launcher-belthanior-terminal-{uuid}    — Per-session terminal rooms (ephemeral)
│   │
│   └── Space: launcher-pi-farm
│       ├── #launcher-pi-farm-exec:mxdx
│       ├── #launcher-pi-farm-status:mxdx
│       ├── #launcher-pi-farm-logs:mxdx
│       └── #launcher-pi-farm-terminal-{uuid}
│
└── Space: Projects
    ├── Space: project-girt
    │   ├── #girt-builds:mxdx
    │   └── #girt-tests:mxdx
    └── ...
```

Terminal session rooms are created on demand within the launcher's space and destroyed when the session ends.

---

## 4. Event Schema (new events for management console)

All events use the existing `org.mxdx.*` namespace. The core events (`command`, `output`, `result`, `host_telemetry`, `secret_request`, `secret_response`, `worker_spawned`, `worker_tombstoned`, `policy.*`) are defined in the mxdx architecture. This section defines **additions** for interactive terminal and extended telemetry.

### 4.1 Terminal Data (session rooms, bidirectional)

```json
{
  "type": "org.mxdx.terminal.data",
  "content": {
    "data": "<base64(compressed PTY chunk)>",
    "encoding": "zlib+base64",
    "seq": 12345
  }
}
```

**Adaptive compression:** Payloads below 32 bytes use `raw+base64`. Payloads >= 32 bytes use `zlib+base64`. The `encoding` field is forward-compatible for future compression schemes (e.g., `zstd+base64`).

### 4.2 Terminal Resize (session rooms, client → launcher)

```json
{
  "type": "org.mxdx.terminal.resize",
  "content": {
    "cols": 120,
    "rows": 40
  }
}
```

### 4.3 Terminal Session Lifecycle (launcher exec room)

**Request a new terminal session:**

```json
{
  "type": "org.mxdx.terminal.session_request",
  "content": {
    "uuid": "550e8400-e29b-41d4-a716-446655440000",
    "command": "/bin/bash",
    "env": { "TERM": "xterm-256color" },
    "cols": 80,
    "rows": 24
  }
}
```

**Session created response (threaded reply):**

```json
{
  "type": "org.mxdx.terminal.session_response",
  "content": {
    "uuid": "550e8400-e29b-41d4-a716-446655440000",
    "session_id": "tmux-session-name",
    "room_id": "!abc123:mxdx",
    "status": "created"
  },
  "m.relates_to": {
    "rel_type": "m.thread",
    "event_id": "$request_event_id"
  }
}
```

**List terminal sessions:**

```json
{
  "type": "org.mxdx.terminal.session_list",
  "content": { "uuid": "request-uuid" }
}
```

**Kill a terminal session:**

```json
{
  "type": "org.mxdx.terminal.session_kill",
  "content": { "session_id": "tmux-session-name" }
}
```

### 4.4 Retransmit Request (session rooms, client → launcher)

```json
{
  "type": "org.mxdx.terminal.retransmit",
  "content": {
    "session_id": "tmux-session-name",
    "from_seq": 145,
    "to_seq": 148
  }
}
```

The launcher keeps a ring buffer of the last 1000 events per session.

### 4.5 Launcher Identity (launcher status room, state event)

Extends `org.mxdx.host_telemetry` with multi-homeserver identity:

```json
{
  "type": "org.mxdx.launcher.identity",
  "state_key": "",
  "content": {
    "launcher_id": "belthanior",
    "accounts": [
      "@launcher-belthanior:hs1.mxdx.dev",
      "@launcher-belthanior:hs2.mxdx.dev",
      "@launcher-belthanior:hs3.mxdx.dev"
    ],
    "primary": "@launcher-belthanior:hs1.mxdx.dev",
    "admins": [
      "@liam:mxdx.dev",
      "@fleet-manager:automation.mxdx.dev"
    ]
  }
}
```

### 4.6 Extended Host Telemetry (launcher status room, state event)

Extends the mxdx `org.mxdx.host_telemetry` with richer data:

```json
{
  "type": "org.mxdx.host_telemetry",
  "state_key": "",
  "content": {
    "timestamp": "2026-03-04T14:32:10Z",
    "timezone": "America/Toronto",
    "hostname": "belthanior",
    "os": "Ubuntu 24.04 LTS",
    "arch": "x86_64",
    "uptime_seconds": 864000,
    "load_avg": [2.1, 1.8, 1.5],
    "cpu": {
      "model": "AMD Ryzen 9 7950X",
      "cores": 32,
      "utilization_percent": 23.5
    },
    "gpu": [
      { "model": "RX 7900 XTX", "memory_mb": 24576, "utilization_percent": 80.0 }
    ],
    "memory": { "total_mb": 32768, "used_mb": 12390 },
    "disk": [
      { "mount": "/", "total_gb": 500, "used_gb": 120 }
    ],
    "network": {
      "interfaces": [
        {
          "name": "eth0",
          "ip": "10.0.1.5",
          "speed_mbps": 10000,
          "rx_bytes_sec": 125000000,
          "tx_bytes_sec": 50000000,
          "rx_packets_sec": 85000,
          "tx_packets_sec": 62000,
          "rx_errors": 0,
          "tx_errors": 0,
          "rx_dropped": 0,
          "tx_dropped": 0
        }
      ],
      "routes": [
        { "destination": "0.0.0.0/0", "gateway": "10.0.1.1", "interface": "eth0", "metric": 100 },
        { "destination": "10.0.1.0/24", "gateway": "", "interface": "eth0", "metric": 0 }
      ]
    },
    "services": [
      { "name": "docker", "status": "running" },
      { "name": "postgres", "status": "running" }
    ],
    "devices": [
      { "type": "usb", "name": "YubiKey 5" }
    ],
    "iops": { "read": 1240, "write": 380 },
    "capabilities": {
      "mode": "allowlist",
      "allowed_commands": ["/bin/bash", "/usr/bin/python3"]
    },
    "active_sessions": {
      "terminal": 2,
      "worker": 3,
      "command": 1
    },
    "poll_interval_seconds": 30,
    "telemetry": {
      "source": "opentelemetry",
      "metrics": [
        { "name": "system.cpu.utilization.by_core", "values": [0.12, 0.45] },
        { "name": "gpu.0.power_watts", "value": 285 },
        { "name": "custom.postgres.connections", "value": 42 }
      ]
    }
  }
}
```

A launcher is considered offline if `now - timestamp > poll_interval_seconds * 2`.

---

## 5. Client Library API

```typescript
interface MxdxClient {
  // Connect to Matrix and discover launchers
  connect(homeserver: string, accessToken: string): Promise<void>

  // Launcher discovery
  listLaunchers(): Promise<Launcher[]>
  getLauncherStatus(launcherId: string): Promise<HostTelemetry>

  // Terminal session management
  listTerminalSessions(launcherId: string): Promise<TerminalSession[]>
  createTerminalSession(launcherId: string, command: string, cols: number, rows: number): Promise<TerminalSession>
  killTerminalSession(sessionId: string): Promise<void>
  attachTerminalSession(sessionId: string): TerminalSocket

  // Non-interactive command execution (mxdx core)
  executeCommand(launcherId: string, cmd: string, env?: Record<string, string>): Promise<CommandResult>
}

interface TerminalSocket {
  send(data: string): void
  onmessage: (data: string) => void
  onclose: () => void
  onstatuschange: (status: "connected" | "reconnecting" | "disconnected") => void
  close(): void
  resize(cols: number, rows: number): void
}
```

`TerminalSocket` implements enough of the WebSocket interface for xterm.js's `AttachAddon` to bind directly. Internally:

- Joins the session room on `attachTerminalSession()`
- Listens for `org.mxdx.terminal.data` events, decompresses/decodes, reorders by `seq`, emits via `onmessage`
- Takes `send()` calls from xterm.js, applies adaptive compression (raw < 32 bytes, zlib >= 32 bytes), base64 encodes, sends as Matrix events
- Translates `resize()` into `org.mxdx.terminal.resize` events

One session per browser tab. Multiple tabs provide multiplexing.

---

## 6. Multi-Homeserver Launcher

The launcher registers accounts on 2-3 federated Tuwunel instances for transport redundancy.

### Startup

1. Connects to all homeservers concurrently
2. Measures sync latency to each
3. Selects the lowest-latency homeserver as primary
4. Creates its Space and rooms on the primary
5. Invites its other identities as room admins (power level 100)
6. Invites all configured admin accounts at power level 100
7. All identities listen for events — primary handles responses, secondaries are hot standbys

### Failover

1. Launcher detects sync failure on primary
2. Promotes next-lowest-latency identity to primary
3. Updates `org.mxdx.launcher.identity` state event
4. All rooms already federated — seamless transition
5. Latency re-checked periodically (configurable, default 60s)

### Configuration

```toml
[homeservers]
accounts = [
  { homeserver = "https://hs1.mxdx.dev", user = "@launcher-belthanior:hs1.mxdx.dev" },
  { homeserver = "https://hs2.mxdx.dev", user = "@launcher-belthanior:hs2.mxdx.dev" },
  { homeserver = "https://hs3.mxdx.dev", user = "@launcher-belthanior:hs3.mxdx.dev" },
]
latency_check_interval_seconds = 60
failover_threshold_ms = 5000

[admins]
accounts = [
  "@liam:mxdx.dev",
  "@fleet-manager:automation.mxdx.dev",
]
allow_membership_management = [
  "@fleet-manager:automation.mxdx.dev",
]

[capabilities]
mode = "allowlist"  # or "unrestricted"
allowed_commands = [
  "/bin/bash",
  "/usr/bin/python3",
  "cargo",
  "git",
  "docker compose *",
]
denied_commands = [
  "rm -rf /*",
  "shutdown",
]
max_sessions = 10
max_workers = 20
allow_interactive = true
allow_non_interactive = true
timeout_seconds = 3600

[terminal]
default_shell = "/bin/bash"
default_env = { "TERM" = "xterm-256color" }
max_terminal_sessions = 5
pty_batch_ms = 15
pty_batch_max_bytes = 4096
retransmit_buffer_size = 1000

[status]
poll_interval_seconds = 30

[telemetry]
enabled = true
collector_binary = "/usr/local/bin/otelcol"
collector_config = "/etc/otel/config.yaml"
export_to_matrix = true
export_interval_seconds = 30

[rate_limits]
max_events_per_second_per_session = 100
max_concurrent_sessions_per_user = 5
max_payload_bytes = 65536
```

---

## 7. WASI Packaging & Distribution

The launcher is compiled to WASI (WebAssembly System Interface) and published to npm for zero-install deployment.

### Build Pipeline

```
Rust source → cargo build --target wasm32-wasip2 → mxdx-launcher.wasm
                                                          ↓
                                              npm package: @mxdx/launcher
                                              includes: WASI binary + wasmtime shim
```

### Usage

```bash
# Zero-install launch — downloads and runs instantly
npx @mxdx/launcher --config ./launcher.toml

# Or install globally
npm install -g @mxdx/launcher
mxdx-launcher --config ./launcher.toml
```

### How It Works

- The npm package bundles the `.wasm` binary and a thin Node.js shim that invokes it via a WASI runtime (wasmtime or jco)
- WASI preview 2 provides access to: filesystem (config, tmux sockets), network sockets (Matrix homeserver sync), process spawning (tmux, OTel collector)
- Single binary runs on any architecture: x86_64, ARM64, RISC-V — no cross-compilation needed
- The WASI runtime is bundled as a dependency, not a system requirement

### Trade-offs

| Aspect | Native | WASI |
|:---|:---|:---|
| Performance | Baseline | ~10-20% overhead (I/O bound, negligible in practice) |
| Distribution | Per-arch binaries, package managers | Single `npx` command, any machine with Node.js |
| Installation | Compile or download binary | Zero install |
| Sandboxing | OS-level only | WASI capability-based (filesystem, network scoped) |
| Process spawning | Unrestricted | Requires WASI preview 2 `wasi:cli/command` |

For constrained environments without Node.js (embedded, minimal containers), native binaries can still be cross-compiled as a fallback.

---

## 8. Terminal Session Architecture

### PTY Bridge

For each active terminal session, a tokio task in the launcher:

- Reads from the tmux session's PTY fd in a loop
- Batches output: accumulates for 15ms or until 4KB, whichever comes first
- Applies adaptive compression, base64 encodes, sends as `org.mxdx.terminal.data`
- Listens for incoming `org.mxdx.terminal.data` events, decodes, writes to PTY
- Handles `org.mxdx.terminal.resize` by calling `tmux resize-window`

### Session Room Lifecycle

On `org.mxdx.terminal.session_request`:

1. Policy Agent validates the sender's access (fail-closed)
2. Launcher validates the command against its capability config
3. Creates a tmux session running the requested command
4. Creates a new encrypted room within the launcher's Space
5. Invites the requesting user and the launcher's other identities
6. Sets history visibility to `joined` — late joiners cannot read terminal history
7. Responds with `org.mxdx.terminal.session_response`

On session end (user closes or process exits):

1. Launcher sends a final `org.mxdx.result` event with exit code
2. Launcher leaves the room
3. Room is tombstoned

### Interaction with Workers

Terminal sessions can spawn worker agents. When a terminal session needs secrets (e.g., a user runs `git clone` in the terminal and needs credentials), the launcher can spawn an ephemeral worker that requests the secret from the Secrets Coordinator, injects it into the tmux session's environment, and tombstones itself.

---

## 9. Error Handling & Reconnection

### Browser-Side

On Matrix sync drop:

1. Buffer pending `send()` calls from xterm.js
2. Reconnect with exponential backoff (1s, 2s, 4s... max 30s)
3. On reconnect, re-sync session room — Matrix sync catch-up delivers missed events, reordered by `seq`
4. Emit `onstatuschange` so the HTMX app can show connection state

### Launcher-Side

On launcher crash:

1. tmux sessions survive independently
2. On restart, launcher reconnects to all homeservers, re-reads room state
3. Re-discovers existing tmux sessions via `tmux list-sessions`
4. Re-attaches PTY bridges to sessions with matching room IDs
5. Status event timestamp goes stale during downtime — clients detect offline

### Homeserver Failover

See [Section 6: Multi-Homeserver Launcher](#6-multi-homeserver-launcher).

### Sequence Gap Handling

On `seq` gap detection:

1. Buffer incoming events for up to 500ms
2. If gap not filled, request retransmit via `org.mxdx.terminal.retransmit`
3. If retransmit fails, accept the gap and continue — terminal output is lossy-tolerant

---

## 10. Security

### Inherited from mxdx Core

- **Policy Agent (fail-closed)** — All access controlled by the appservice. If it's down, nothing works.
- **Secrets Coordinator** — HSM-backed, zero ambient credentials, full audit trail.
- **Ephemeral Workers** — Per-task identities, tombstoned after completion.
- **E2EE everywhere** — Megolm encryption on all rooms. Homeserver cannot read content.

### Terminal-Specific

- **Session room isolation** — Each terminal session gets its own encrypted room. History visibility `joined` prevents late joiners from reading prior output.
- **Output injection prevention** — Power levels set so only launcher identities can send `org.mxdx.terminal.data` responses.
- **PTY bridge is a dumb pipe** — Bytes from Matrix go directly to tmux stdin. Control actions happen only through typed events.
- **Capability enforcement** — Commands validated against allowlist before terminal session creation. No shell expansion in matching.

### Browser-Specific

- **E2EE in browser (WASM)** — The Rust backend serves static assets only. No server-side decryption.
- **Key verification** — First-connection verification via emoji comparison or QR code.
- **No server relay** — Terminal data flows browser → Matrix homeserver → launcher. The web server never sees terminal content.

### Rate Limiting

Configurable per launcher:

- Max events per second per session
- Max concurrent sessions per user
- Max payload size per event

---

## 11. Progressive Web Application

### Manifest & Service Worker

- **Standalone display mode** with appropriate icons and theme color
- **Service worker** caches app shell (HTML, JS, CSS, WASM crypto module) for instant loading
- **Terminal view** works full-screen on mobile with virtual keyboard integration

### Capabilities

- **Installable** — "Add to Home Screen" on Android
- **Push notifications** — Launcher status changes (offline, high load) via Matrix event stream + sygnal push gateway
- **Offline app shell** — Dashboard loads from cache; live data populates on Matrix sync
- **Responsive layout** — HTMX views adapt between desktop and mobile

### Constraints

- WASM crypto module must be cached by service worker for offline E2EE initialization
- Matrix sync requires network — offline mode shows cached last-known state with staleness indicator
- Push notifications require a push gateway (sygnal) for delivery to backgrounded service workers

---

## 12. Key Design Decisions

| Decision | Choice | Rationale |
|:---|:---|:---|
| Extend mxdx | Over standalone system | Reuse Policy Agent, Secrets Coordinator, Worker model, audit trail, event namespace |
| `org.mxdx.*` namespace | Over `m.terminal.*` | Consistent with existing mxdx events |
| Launcher (not "daemon") | Terminology alignment | Matches mxdx's "minimal persistent ear on host" concept |
| WASI packaging via npm | Over native-only distribution | Zero-install via `npx`, runs on any architecture, WASI sandboxing |
| Fail-closed (Policy Agent) | Over power-levels-only | Appservice intercept is strictly more secure than room-level power checks |
| tmux for terminal sessions | Over raw PTY | Session persistence, survives launcher restarts, built-in multiplexing |
| Adaptive compression | Over always-on/always-off | No overhead for keystrokes, significant savings for bulk output |
| Multi-homeserver launcher | Over single homeserver | Transport redundancy, automatic failover |
| Hierarchical Spaces | Over flat rooms | Mirrors mxdx topology, clean separation per launcher |
| UUID-threaded events | Over sequence-only | Consistent with mxdx's command/output threading model |
| OTel as optional subprocess | Over built-in-only metrics | Lightweight base with extensible depth when needed |
| One terminal session per tab | Over multi-session library | Keeps client library simple |
| PWA | Over native Android app | Single codebase, no app store dependency, instant updates |
| Browser-native E2EE (WASM) | Over server-side proxy | No trusted server needed for decryption |
| Cloudflare Workers web service | Over traditional server | Edge-deployed, globally cached, stateless, zero security surface |
| Web service separate from Tuwunel | Over co-located | Web service is a dumb asset server; separation means compromise of web tier cannot access Matrix traffic |
| HTMX + React islands | Over SPA or HTMX-only | HTMX for dashboard shell, React for complex visualizations where needed |
| Secrets helper CLI (v2) | Over env injection | User-initiated, explicit, auditable; avoids ambient credential leakage in tmux env |

---

## 13. Resolved Questions

1. **WASI vs native binaries** — WASI via npm/npx is the convenience distribution path. Native Rust binaries are available as a fallback for platforms where WASI is insufficient (e.g., constrained embedded devices, or where process spawning via WASI preview 2 is immature). Both target the same codebase.

2. **Terminal session secret injection** — A dedicated helper CLI (`npx @epiphytic/mxdx-secret name.of.secret`) will be available inside terminal sessions for on-demand secret retrieval. This is a **v2 feature** — it depends on the Secrets Coordinator and a secret management helper being built first.

3. **Session recording/playback** — Confirmed as a **v2 feature**. All terminal data exists in the Matrix room DAG, so recording is implicit. Playback tooling (asciinema-style) can reconstruct sessions from room history.

4. **Federation scope for multi-homeserver** — The design assumes all homeservers are operated by the same entity. Cross-operator federation changes the trust model significantly and is out of scope for v1.

5. **HTMX vs richer frontend** — HTMX is the default for all dashboard views. React components (or other JS) are used as needed for complex visualizations (network topology graphs, real-time charts), integrated via HTMX partials. Not an either/or — HTMX is the shell, React islands fill in where needed.

---

## 14. Web Service Architecture

The management console web service runs **separately from the Tuwunel homeservers**. It is a static-first, edge-deployable application.

### Design Constraints

- **Lightweight** — The web service serves static assets (HTML, JS, CSS, WASM) and renders HTMX partials. It holds no state beyond what's in Matrix rooms.
- **Cacheable** — All assets are cache-friendly with content-addressed hashes. HTMX partials are short-lived or SSE-driven.
- **Cloudflare Workers compatible** — The Rust backend compiles to WASM and runs on Cloudflare Workers (or any edge runtime). No Node.js server required in production.

### Architecture

```
┌─ Edge (Cloudflare Workers / any edge runtime) ─┐
│                                                 │
│  Rust → WASM web service                        │
│  ├─ Serves static assets (HTML, JS, CSS, WASM)  │
│  ├─ Renders HTMX partials (server-side)         │
│  ├─ PWA manifest + service worker               │
│  └─ No Matrix credentials, no state             │
│                                                 │
└──────────────────┬──────────────────────────────┘
                   │ Static assets + HTMX partials
                   ↓
┌─ Browser / PWA ─────────────────────────────────┐
│  mxdx-client (JS/TS + WASM)               │
│  matrix-sdk-crypto-wasm                         │
│  Connects directly to Tuwunel homeservers       │
└─────────────────────────────────────────────────┘
```

The web service **never touches Matrix traffic**. All E2EE communication flows directly between the browser and the Tuwunel homeservers. The web service is a dumb asset server with HTMX rendering — it can be replaced, CDN-cached, or run at the edge with zero security implications.

### Deployment Options

| Environment | Runtime | Notes |
|:---|:---|:---|
| Cloudflare Workers | Rust → WASM | Primary production target. Edge-deployed, globally distributed |
| Any WASI runtime | Rust → WASM | Self-hosted alternative |
| Native binary | Rust (axum/actix) | Development, or environments without WASM support |
| Static hosting | Pre-rendered + client JS | Minimal deployment — no server-side HTMX, fully client-rendered fallback |
