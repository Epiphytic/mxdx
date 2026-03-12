# mxdx — Core Architecture

**Status:** DRAFT — Review requested
**Date:** 2026-02-27 (updated 2026-03-04)
**Authors:** Liam Helmer, Bel

---

## 1. Overview

mxdx is a device and server management framework built on the Matrix protocol. Agents communicate, receive commands, deliver results, and obtain secrets through encrypted Matrix rooms. The system is designed for fail-closed security, ephemeral worker identities, and zero ambient credentials.

This document covers the **core components** — Policy Agent, Orchestrator, Worker Agents, Secrets Coordinator, and the foundational event schema. For the full management console design (launcher, terminal, dashboard, PWA, multi-homeserver), see [mxdx Management Console](./mxdx-management-console.md). For the build methodology, see [Development Methodology](./mxdx-development-methodology.md).

### Homeserver

- **Implementation:** Tuwunel (Rust, embedded RocksDB, single binary)
- **OIDC:** Built-in OIDC server (PR #342 branch) with rich claims extension for external service federation

---

## 2. Policy Agent (Appservice)

A Matrix Application Service that enforces access control.

**Responsibilities:**
- Owns the `@agent-*:mxdx` namespace exclusively
- Intercepts all events for agent users before delivery
- Checks policy rooms for authorization rules
- Grants/revokes room membership based on policy
- **Fail-closed:** If the appservice is down, the homeserver rejects events for the agent namespace

**Policy model:**
- Policies are stored as state events in dedicated policy rooms (MSC2313 pattern)
- The policy agent subscribes to these rooms and enforces rules in real-time
- Policy changes are auditable (they're Matrix events)

**Example policy room state:**
```json
{
  "type": "org.mxdx.policy.agent_access",
  "state_key": "@launcher-belthanior:mxdx",
  "content": {
    "spaces": ["!project-girt:mxdx"],
    "rooms": ["#builds:mxdx", "#deployments:mxdx"],
    "power_level": 50,
    "can_spawn_workers": true,
    "allowed_commands": ["cargo", "git", "npm", "node"],
    "secret_scopes": ["github:Epiphytic/girt:*"]
  }
}
```

---

## 3. Orchestrator

A high-level agent (could be AI-driven or human-controlled) that plans and delegates work.

**Responsibilities:**
- Creates project spaces and rooms
- Invites leader agents to project rooms
- Sends commands to launchers
- Monitors worker progress via room events
- Makes placement decisions based on host telemetry

**The orchestrator does NOT:**
- Hold secrets (it requests them like any other agent if needed)
- Bypass the policy agent
- Directly execute work on hosts

---

## 4. Worker Agents

Ephemeral agents spawned by launchers for specific tasks.

**Lifecycle:**
1. Launcher registers a new Matrix user: `@worker-{uuid}:mxdx` (~55ms)
2. Worker logs in, initializes E2EE crypto (~75ms)
3. Worker joins its assigned room, exchanges keys (~30ms)
4. Worker requests any needed secrets from the Secrets Coordinator via DM
5. Worker executes its task (could be a Claude Code session, a build, a test run, etc.)
6. Worker posts results to its room
7. Worker account is **deactivated and tombstoned** — identity permanently retired

**Total spin-up to first E2EE message: ~155ms** (measured).

**After tombstoning:**
- All access tokens invalidated
- Device keys removed
- All rooms left
- MXID permanently retired (cannot be re-registered)
- Any late-arriving events for this user are rejected by the homeserver

---

## 5. Core Event Schema

### 5.1 Namespacing

All custom events use the `org.mxdx.*` namespace:

| Event Type | Purpose |
|---|---|
| `org.mxdx.command` | Command from orchestrator/leader to launcher |
| `org.mxdx.output` | Stdout/stderr stream from execution |
| `org.mxdx.result` | Exit status and summary of a command |
| `org.mxdx.host_telemetry` | Host resource utilization (state event) |
| `org.mxdx.secret_request` | Agent requesting a secret (DM) |
| `org.mxdx.secret_response` | Coordinator delivering a secret (DM) |
| `org.mxdx.worker_spawned` | Notification that a worker was created |
| `org.mxdx.worker_tombstoned` | Notification that a worker was retired |
| `org.mxdx.policy.agent_access` | Policy room: agent access rules (state event) |
| `org.mxdx.policy.secret_scope` | Policy room: secret scope grants (state event) |

For terminal-specific events (`org.mxdx.terminal.*`, `org.mxdx.launcher.*`), see the [Management Console](./mxdx-management-console.md).

### 5.2 Command Event

```json
{
  "type": "org.mxdx.command",
  "content": {
    "uuid": "550e8400-e29b-41d4-a716-446655440000",
    "action": "exec | spawn_worker | install | update",
    "cmd": "cargo build --release",
    "args": ["--features", "gpu"],
    "env": {
      "CARGO_HOME": "/tmp/cargo",
      "RUST_LOG": "info"
    },
    "cwd": "/workspace/girt",
    "wasm_module": null,
    "allowed_commands": ["cargo", "git"],
    "timeout_seconds": 3600,
    "reply_room": null
  }
}
```

### 5.3 Output Event (threaded reply)

```json
{
  "type": "org.mxdx.output",
  "content": {
    "uuid": "550e8400-e29b-41d4-a716-446655440000",
    "stream": "stdout | stderr",
    "data": "Compiling girt v0.1.0...",
    "seq": 42,
    "timestamp": "2026-02-27T15:30:01.234Z"
  },
  "m.relates_to": {
    "rel_type": "m.thread",
    "event_id": "$command_event_id"
  }
}
```

### 5.4 Result Event (threaded reply)

```json
{
  "type": "org.mxdx.result",
  "content": {
    "uuid": "550e8400-e29b-41d4-a716-446655440000",
    "status": "exit | killed | timeout | error",
    "exit_code": 0,
    "duration_ms": 34200,
    "output_lines": 847,
    "summary": "Build succeeded"
  },
  "m.relates_to": {
    "rel_type": "m.thread",
    "event_id": "$command_event_id"
  }
}
```

### 5.5 Secret Request (DM)

```json
{
  "type": "org.mxdx.secret_request",
  "content": {
    "request_id": "req-001",
    "scope": "github:Epiphytic/girt:contents:read",
    "ttl_seconds": 3600,
    "reason": "Need to clone repo for build task abc-123"
  }
}
```

### 5.6 Secret Response (DM)

```json
{
  "type": "org.mxdx.secret_response",
  "content": {
    "request_id": "req-001",
    "granted": true,
    "secret_type": "bearer_token",
    "value": "ghs_xxxxxxxxxxxx",
    "expires_at": "2026-02-27T16:30:00Z",
    "scope": "github:Epiphytic/girt:contents:read"
  }
}
```

---

## 6. Secrets Architecture

### 6.1 Design Goals

- No secrets stored on agent hosts (only Matrix device keys)
- Secrets scoped by Matrix identity — the coordinator checks who is asking, not what they claim to need
- Dynamic secrets preferred over static (short-lived > long-lived)
- HSM support for the coordinator's own key material
- Full audit trail of every secret access

### 6.2 Secret Types

| Type | Generation | Storage | Example |
|---|---|---|---|
| **Dynamic (STS)** | Generated on demand via token exchange | Never stored — created and delivered | GitHub tokens (octo-sts), GCP access tokens, AWS STS |
| **Static (encrypted)** | Pre-provisioned by admin | Encrypted at rest, decrypted in-memory by coordinator | API keys, webhook secrets, database passwords |
| **Derived** | Computed from other secrets + context | Never stored | HMAC signatures, scoped tokens |

### 6.3 Secrets Coordinator Architecture

```
┌────────────────────────────────────────────────┐
│           Secrets Coordinator Process           │
│                                                │
│  ┌──────────────┐  ┌────────────────────────┐  │
│  │ Matrix Client │  │    Secret Backends     │  │
│  │              │  │                        │  │
│  │  Receives    │  │  ┌─────────────────┐   │  │
│  │  DMs         │──│──│  Dynamic (STS)  │   │  │
│  │  Verifies    │  │  │  - octo-sts     │   │  │
│  │  identity    │  │  │  - GCP WIF      │   │  │
│  │  Checks      │  │  │  - AWS STS      │   │  │
│  │  policy      │  │  └─────────────────┘   │  │
│  │  Returns     │  │                        │  │
│  │  via E2EE DM │  │  ┌─────────────────┐   │  │
│  │              │  │  │  Static Store   │   │  │
│  └──────────────┘  │  │  (age-encrypted │   │  │
│                    │  │   + HSM key)    │   │  │
│                    │  └─────────────────┘   │  │
│                    │                        │  │
│                    │  ┌─────────────────┐   │  │
│                    │  │  HSM / KMS      │   │  │
│                    │  │  (PKCS#11 or    │   │  │
│                    │  │   cloud KMS)    │   │  │
│                    │  └─────────────────┘   │  │
│                    └────────────────────────┘  │
│                                                │
│  ┌──────────────────────────────────────────┐  │
│  │           Audit Logger                   │  │
│  │  Posts to #secrets-audit:mxdx            │  │
│  └──────────────────────────────────────────┘  │
└────────────────────────────────────────────────┘
```

### 6.4 Request Flow

```
1. Worker @worker-abc:mxdx sends E2EE DM to @secrets:mxdx:
   → org.mxdx.secret_request { scope: "github:Epiphytic/girt:contents:read" }

2. Coordinator receives DM. Verifies:
   a. Sender is on same homeserver (server_name == "mxdx")
   b. Sender account is active (not deactivated)
   c. Sender's MXID matches an allowed pattern in policy room
   d. Requested scope is within sender's allowed scopes

3. If denied:
   → org.mxdx.secret_response { granted: false, reason: "scope not authorized" }
   → Audit event posted to #secrets-audit

4. If approved (dynamic secret):
   a. Coordinator calls octo-sts with its own GitHub App credentials
   b. octo-sts returns ephemeral GitHub token (scoped, short-lived)
   c. Coordinator returns token via E2EE DM
   d. Audit event posted

5. If approved (static secret):
   a. Coordinator looks up secret in its encrypted store
   b. Decrypts using HSM-backed key (PKCS#11 unwrap, or cloud KMS decrypt)
   c. Returns value via E2EE DM
   d. Audit event posted
```

### 6.5 Static Secret Storage

**Encryption at rest:**
- Secrets stored as an `age`-encrypted file on the coordinator's host
- The `age` identity (private key) is either:
  - Stored in an HSM (PKCS#11) — **preferred for production**
  - Wrapped by a cloud KMS key (GCP/AWS) — **good for cloud deployments**
  - Stored as a file readable only by the coordinator process — **acceptable for dev/home**
- On startup, coordinator decrypts the store into memory
- Memory is locked (mlock) to prevent swapping to disk

**Access tiers:**
- **Write-only:** Admin user DMs coordinator to add/update secrets. Coordinator re-encrypts the store.
- **Read-only:** Agents request secrets via DM. Coordinator serves from memory.
- **Admin:** Human operator with access to the raw encrypted file and HSM credentials. Can rotate the encryption key, export/import secrets.

**Secret rotation:**
- Dynamic secrets rotate automatically (every STS call = new token)
- Static secrets: coordinator can be told to rotate via admin command
- The coordinator tracks which agents received which secrets. On rotation, it can notify active agents that their secret has changed.

### 6.6 HSM Integration

The coordinator's own key material (GitHub App private key, age identity, signing keys) must be protected by hardware security:

**Option A: PKCS#11 HSM (on-premise)**
- YubiHSM 2, Nitrokey HSM, or SoftHSM for development
- Coordinator uses PKCS#11 interface to unwrap/sign
- Private keys never leave the HSM
- Suitable for: belthanior, on-premise deployments

**Option B: Cloud KMS (cloud deployments)**
- GCP Cloud KMS, AWS KMS, Azure Key Vault
- Coordinator calls KMS API to decrypt the secret store encryption key
- KMS key never leaves the cloud provider's HSM
- Suitable for: GCP/AWS deployments

**Option C: TPM (host-bound)**
- Use the host's TPM 2.0 to seal the coordinator's key material
- Key is bound to the specific hardware + software state
- Suitable for: dedicated hardware, high-security deployments

For development/home use, SoftHSM with PKCS#11 gives the same API surface as a real HSM, so the code is production-ready from day one.

### 6.7 Threat Model

| Threat | Mitigation |
|---|---|
| Compromised worker requests unauthorized secrets | Policy check on every request; scopes enforced per-identity |
| Compromised homeserver reads secrets in transit | E2EE (Megolm) — homeserver cannot decrypt DM content |
| Late messages to tombstoned worker | Coordinator checks account is active before responding |
| Federated user spoofs local MXID | Coordinator strictly checks server_name portion of MXID |
| Coordinator process compromise | HSM protects key material; secrets in memory only (mlock'd) |
| Coordinator host compromise | HSM keys require physical presence / cloud IAM; blast radius = static secrets in memory |
| Replay of secret_response events | Secrets have TTL; coordinator tracks issued secrets and can revoke |
| Admin user compromise | Require 2FA for admin commands; dual-control for high-value secrets |

### 6.8 What This Does NOT Solve

- **Homeserver availability** — if Tuwunel is down, nothing works. Multi-homeserver federation addresses this (see [Management Console](./mxdx-management-console.md)).
- **Agent code integrity** — a launcher executes what it's told. If the orchestrator is compromised, it can send malicious commands. Code signing / WASM verification could address this (future work).
- **Network-level isolation** — Matrix messages transit the network. TLS protects the wire; E2EE protects the content. But traffic analysis is possible.

---

## 7. Performance Baseline

Measured on belthanior (AMD Ryzen, 32GB RAM, Tuwunel local):

| Operation | Latency |
|---|---|
| Agent registration | 53ms |
| Login | 21ms |
| Crypto init (OlmMachine) | 36ms |
| E2EE key exchange | 25ms |
| **Registration → first E2EE message** | **155ms** |
| E2EE send (encrypt + PUT) | 1.8ms p50 |
| E2EE receive (sync + decrypt) | 4.2ms p50 |
| **E2EE round-trip** | **6.0ms p50** |
| OIDC discovery | 0.10ms p50 |
| OIDC userinfo | 0.18ms p50 |
| JWKS fetch | 0.09ms p50 |
