# Multi-Homeserver Support for WASM Clients & Launchers

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Enable WASM-based clients and launchers to connect to multiple Matrix homeservers simultaneously, with latency-based preferred server selection, circuit breaker failover, and federated room topology.

**Architecture:** A `MultiHsClient` class in `packages/core/` wraps N `WasmMatrixClient` instances (one per homeserver). All instances sync concurrently. One is designated preferred (lowest latency). Sends route through preferred; receives are deduplicated across all servers. Circuit breaker triggers failover when a server becomes unhealthy.

**Tech Stack:** WASM (mxdx-core-wasm), JavaScript (packages/core, packages/launcher, packages/client), Matrix federation

---

## 1. Architecture Overview

Each launcher and client holds N `WasmMatrixClient` instances — one per configured homeserver. All instances sync concurrently, polling their respective servers independently. One is designated the **preferred** (lowest latency, measured via filtered `sync_once()`).

**Preferred server** is used for:
- Initiating commands (client) or responding to commands (launcher)
- Creating new DM rooms for interactive sessions
- Publishing status events (launcher posts `org.mxdx.host_telemetry` with preferred identity)

**All servers** are used for:
- Receiving events (first to deliver wins, deduplicated by event ID)
- Health monitoring (sync failures feed circuit breaker)

**Room topology unchanged**: space + exec + logs per launcher, all federated across every homeserver in the list. A launcher creates rooms on its initial primary; federation syncs them to the others. A launcher has a single room set federated across all servers.

**Shared code**: The multi-homeserver logic lives in `packages/core/` as a `MultiHsClient` class, used by both launcher and client runtimes. The WASM layer (`mxdx-core-wasm`) stays single-client — the multiplexing happens in JS.

---

## 2. Preferred Server Selection & Failover

### Initial Selection

On startup, all homeservers connect in parallel. Each runs a filtered `sync_once()` (single room, no timeline) and the round-trip time is recorded. Lowest latency becomes preferred. A config override (`preferredServer`) can pin a specific server as preferred regardless of latency.

### Circuit Breaker

Per-server, tracks failures in a 5-minute sliding window. 5 timeouts or invalid responses within 5 minutes marks that server as **down**. When the preferred server is circuit-broken:

1. Check if all servers are failing — if yes, assume local network issue, don't circuit-break anyone, keep retrying
2. If at least one server is healthy, measure latency across healthy survivors via `sync_once()`
3. Lowest-latency survivor becomes the new preferred
4. Launcher posts updated `org.mxdx.host_telemetry` with new preferred identity
5. Existing DM sessions continue via federation through the surviving server

### Recovery

Circuit-broken servers are probed every **60 + random(1-100) seconds**. Each probe is an independent jittered interval to prevent dogpiling when a server comes back up. On success, the server rejoins the healthy pool but does NOT automatically become preferred — it just becomes available for future failover.

### Re-measurement

Latency is only re-measured on health check failure. No periodic re-measurement. This minimizes unnecessary preferred server switches, which would trigger status updates that all clients need to process.

---

## 3. Event Deduplication & Polling

Since all servers sync concurrently, the same event will arrive from multiple servers via federation. Deduplication is required.

### Approach

`MultiHsClient` maintains a seen-event set keyed by Matrix event ID (`$event_id`). When an event arrives from any server's sync:

1. Check if event ID is in the seen set
2. If new, process it and add to seen set
3. If duplicate, discard silently

### Seen Set Management

Event IDs are kept for a bounded window — either a max size (e.g., 10,000 entries) or a time-based TTL (e.g., 10 minutes). Old entries are evicted to prevent unbounded memory growth. This is safe because duplicate delivery of an event minutes later is not a realistic concern.

### First-to-Deliver Wins

The event is processed from whichever server's sync delivers it first. No preference for the preferred server — this gives the fastest possible event delivery and natural redundancy.

---

## 4. MultiHsClient API Surface

The `MultiHsClient` class in `packages/core/` wraps N `WasmMatrixClient` instances and exposes a simplified interface that the launcher and client runtimes use without worrying about server selection.

```
MultiHsClient {
  // Lifecycle
  static async connect(configs[]) → MultiHsClient   // parallel connect, measure latency, pick preferred
  async shutdown()                                     // disconnect all

  // Preferred server
  preferred()          → { client, server, userId }    // current preferred
  onPreferredChange(cb)                                // fires on failover

  // Sending (always via preferred)
  async sendEvent(roomId, type, content)
  async sendStateEvent(roomId, type, stateKey, content)
  async createDmRoom(userId)

  // Receiving (deduplicated across all servers)
  onRoomEvent(roomId, type, cb)
  onStateEvent(roomId, type, cb)

  // Discovery (searches all servers)
  async findLauncherSpace(launcherName)

  // Health
  serverHealth()       → Map<server, { status, latency, failures }>
}
```

### Key Behaviors

- `sendEvent` / `sendStateEvent` route through preferred client. On failure, the circuit breaker kicks in and may trigger failover — the call is NOT automatically retried on another server (the caller retries after `onPreferredChange` fires).
- `onRoomEvent` delivers deduplicated events from all servers. Callback receives the event plus the originating server (for debugging).
- `findLauncherSpace` queries all servers in parallel, returns first match (rooms are federated, so any server should find it).

---

## 5. Status Posting & Identity Advertisement

Launchers advertise their multi-homeserver state via the existing `org.mxdx.host_telemetry` state event in the exec room. The event gains new fields:

```json
{
  "timestamp": "2026-03-12T10:00:00Z",
  "status": "online",
  "heartbeat_interval_ms": 60000,
  "preferred_server": "hs1.mxdx.dev",
  "preferred_identity": "@launcher-belthanior:hs1.mxdx.dev",
  "accounts": [
    "@launcher-belthanior:hs1.mxdx.dev",
    "@launcher-belthanior:hs2.mxdx.dev"
  ],
  "server_health": {
    "hs1.mxdx.dev": { "status": "healthy", "latency_ms": 45 },
    "hs2.mxdx.dev": { "status": "healthy", "latency_ms": 120 }
  },
  "cpu_usage_percent": 12,
  "memory_used_bytes": 1073741824,
  "p2p": { "status": "p2p" }
}
```

### When This Event Is Republished

- On preferred server change (failover)
- On regular heartbeat interval (existing behavior)
- On server health status change (server goes down or recovers)

### Client Behavior

Clients read this state event to know the launcher's current preferred identity. This is informational — clients don't need to match the launcher's preferred server. They send commands through their own preferred server, and federation delivers them.

---

## 6. Interactive Session Continuity

When a launcher's preferred server goes down and it switches to a new preferred:

- **DM rooms survive via federation.** The DM room exists on all federated servers. Both launcher and client continue reading/writing the same room through their respective surviving servers.
- **P2P sessions are unaffected.** WebRTC data channels are server-agnostic — they connect directly between peers. Server failover has no impact on active P2P connections.
- **New sessions** are created on the new preferred server. Old DM rooms on the failed server remain accessible via federation when that server recovers.

No session migration, no reconnection logic, no new DM rooms needed during failover.

---

## 7. Configuration

Multi-homeserver is activated by providing more than one server in config.

### Launcher Config (TOML)

```toml
username = "belthanior"
password = "hunter2"
servers = ["hs1.mxdx.dev", "hs2.mxdx.dev"]
preferred_server = "hs1.mxdx.dev"  # optional, overrides latency selection

# Optional per-server credential override
[server_credentials."hs2.mxdx.dev"]
username = "belthanior-alt"
password = "different-pass"
```

### Client CLI

```bash
mx exec --servers hs1.mxdx.dev,hs2.mxdx.dev launcher-name whoami
mx shell --servers hs1.mxdx.dev,hs2.mxdx.dev launcher-name
```

### Credential Resolution

- Default: top-level `username` and `password` apply to all servers
- Override: per-server credentials in `[server_credentials."server"]` block

### Behavioral Defaults

- **Single server**: no circuit breaker (nowhere to fail over), no deduplication, no health comparison. Behaves exactly like today.
- **Two+ servers**: full multi-homeserver behavior enabled.

### Web Console

Same pattern — `servers` array in the login form or config. Multiple `WasmMatrixClient` instances in the browser. Reasonable limit of 2-3 servers to keep browser resource usage manageable.

### Credentials Storage

Each server gets its own session/credentials stored independently in the OS keychain (existing `connectWithSession` pattern, keyed by server URL). Each server has its own IndexedDB crypto store.

---

## 8. Network Sanity Check

To avoid false circuit-breaking when the launcher's own internet is down:

**Cross-server comparison**: If ALL servers are failing simultaneously, assume it's a local network issue rather than all servers going down at once. In this case, don't circuit-break any server — keep retrying all of them. Only circuit-break a server if at least one other server is healthy.

This requires no external dependencies, works in air-gapped environments, and is robust when you have 2+ servers. For single-server configs, circuit-breaking is disabled entirely (nowhere to fail over to).

---

## 9. Testing Strategy

### Unit Tests (packages/core)

- `MultiHsClient` creation with 1, 2, 3 servers
- Preferred selection picks lowest latency
- Config override pins preferred server
- Event deduplication by event ID
- Seen set eviction (max size / TTL)
- Circuit breaker triggers after 5 failures in 5 minutes
- Circuit breaker suppressed when all servers fail (network down)
- Recovery probe jitter (60 + random 1-100s)
- Recovered server rejoins pool but doesn't auto-become preferred
- Credential resolution (shared vs per-server)
- Single-server mode: no circuit breaker, no deduplication

### Integration Tests (packages/e2e-tests with dual TuwunelInstance)

- Dual-Tuwunel setup: launcher connects to both, rooms federate
- Client sends command on server A, launcher receives on server B
- Events deduplicated when received from both servers
- Failover: stop primary Tuwunel, verify launcher switches preferred, posts new status
- DM session survives failover via federation
- P2P session unaffected by server failover
- Discovery works across servers

### Security Tests

- Non-launcher user cannot post identity state events
- Credentials are stored per-server in keychain, never logged
- Circuit breaker cannot be triggered by malicious events (only transport failures)
