# WASM E2EE Memory Security Hardening — Design

## Goal

Mitigate crypto key exposure in WASM linear memory and fake-indexeddb for the mxdx launcher, which runs as a long-lived daemon handling encrypted Matrix commands with shell access.

## Threat Model (Priority Order)

1. **Compromised host** — attacker with shell access dumps process memory or swap
2. **Malicious commands** — executed command tries to read parent process memory (`/proc/self/mem`, native addon)
3. **Supply chain** — compromised npm dependency reads WASM memory or fake-indexeddb globals from within the same process

## Architecture: Layered Defense

Four independent layers, each reducing attack surface. Each layer is independently shippable and testable.

```
+---------------------------------------------------+
|                   Main Process                     |
|                                                    |
|  +----------------+     +----------------------+  |
|  | Command         |     | Crypto Worker         |  |
|  | Subprocess      |     | (worker_threads)      |  |
|  |                 |     |                        |  |
|  | child_process   |     | WASM + matrix-sdk     |  |
|  | spawn()         |     | fake-indexeddb         |  |
|  |                 |     | (encrypted via         |  |
|  | No access to    |     |  Web Crypto API)       |  |
|  | WASM memory     |     |                        |  |
|  +----------------+     | CryptoKey objects      |  |
|         ^                | held by runtime,       |  |
|         | stdin/stdout   | NOT in WASM memory     |  |
|         |                +----------+-------------+  |
|         |                           |                |
|  +------+---------------------------+-------------+  |
|  |          Runtime Orchestrator                   |  |
|  |  - Receives commands via MessagePort            |  |
|  |  - Spawns command subprocesses                  |  |
|  |  - Forwards results back to Worker              |  |
|  |  - No crypto state, no keys                     |  |
|  +-------------------------------------------------+  |
+---------------------------------------------------+
```

**Message flow:**
1. Worker syncs Matrix, receives `org.mxdx.command` event, decrypts it
2. Worker sends plaintext command to main process via `MessagePort`
3. Main process spawns child process, streams output back
4. Main process sends result to Worker via `MessagePort`
5. Worker encrypts and sends `org.mxdx.result` to Matrix

## Layer 1: Process Isolation

Move all WASM/crypto into a `worker_threads` Worker. The main process orchestrates command execution but never touches key material.

- Worker holds: WASM module, matrix-sdk client, fake-indexeddb, CryptoKey handles
- Main process holds: runtime config, process-bridge, MessagePort to Worker
- Command subprocesses: already isolated via `child_process.spawn()` (existing)

The main process and Worker communicate via structured messages over `MessagePort`. No shared memory (no `SharedArrayBuffer`).

## Layer 2: Encrypted Crypto Store

Wrap fake-indexeddb with an encryption proxy using the Web Crypto API.

- On Worker startup, generate an AES-256-GCM `CryptoKey` via `crypto.subtle.generateKey()` with `extractable: false`
- `CryptoKey` is opaque — V8/Node.js stores raw key bytes in native memory, not the JS heap or WASM linear memory
- Intercept fake-indexeddb `put`/`get` operations: encrypt values before storage, decrypt on retrieval
- Nonce: unique random 12 bytes per write via `crypto.getRandomValues()`

**Defends against:**
- Memory dump: fake-indexeddb contains ciphertext; CryptoKey bytes not in heap dump
- Supply chain reads of `globalThis.indexedDB`: gets encrypted blobs

**Does not defend against:**
- Code in the same Worker calling `crypto.subtle.decrypt()` — Layer 1 (process isolation) prevents this

**Implementation:** JS module (`encrypted-idb-proxy.js`) that wraps fake-indexeddb, loaded before WASM in the Worker. Uses `browser-crypto` crate on the Rust side for any additional WASM-level encryption needs.

## Layer 3: Megolm Key Rotation & Retention

Tighten matrix-sdk's default rotation for a high-value command execution context.

**Defaults:**
- Rotate after every 10 messages (roughly 2-3 command exchanges)
- Rotate after 1 hour maximum
- Force rotation on security events (new device join, admin user change)

**Configurable retention policy:**

```toml
[security]
megolm_rotation_message_count = 10
megolm_rotation_interval_secs = 3600
key_retention = "forward-secrecy"  # or "audit-trail"
```

- `forward-secrecy` (default): Discard old inbound Megolm session keys after rotation. Past messages become undecryptable. Compromised key only exposes current window.
- `audit-trail`: Retain inbound session keys in the encrypted store. Old messages remain readable. Still protected by Layer 2 encryption.

## Layer 4: OS-Level Hardening & Zeroize

**Core dump prevention:**
- `prctl(PR_SET_DUMPABLE, 0)` on Linux — prevents core dumps and `/proc/self/mem` reads
- Set `RLIMIT_CORE` to 0
- Exposed via small N-API addon or `ulimit -c 0` fallback

**Swap protection:**
- Best-effort `mlock` on Worker's WASM ArrayBuffer via native addon
- If mlock fails (unprivileged), log warning and continue
- Linux-only; macOS has `mlock` equivalent; Windows out of scope

**Zeroize on cleanup:**
- `secureShutdown()` method on `WasmMatrixClient`
- Rust side: zero Olm/Megolm state via `zeroize` crate (volatile writes) before drop
- JS side: clear all fake-indexeddb stores, null CryptoKey references
- Called on `SIGTERM`, `SIGINT`, and clean shutdown
- Best-effort: V8 GC may have copies, but eliminates easy targets

## Testing Strategy

**Layer 1 — Process Isolation:**
- Verify main process cannot access Worker WASM memory
- Verify command subprocesses cannot read `/proc/<parent-pid>/mem` after prctl
- Verify command flows Worker -> main -> subprocess -> main -> Worker without key leaks

**Layer 2 — Encrypted Store:**
- Verify raw fake-indexeddb contents are ciphertext
- Verify CryptoKey is non-extractable
- E2E: encrypted room round-trip works with store wrapper

**Layer 3 — Key Rotation:**
- Verify rotation fires after configured message count
- Verify `forward-secrecy`: old messages undecryptable after rotation
- Verify `audit-trail`: old messages remain decryptable
- Verify forced rotation on new device join

**Layer 4 — OS Hardening:**
- Verify `Dumpable: 0` in `/proc/self/status`
- Verify WASM memory zeroed after `secureShutdown()`
- Verify mlock applied or graceful fallback logged

**Smoke test:**
- Full command round-trip with all layers enabled
- Existing E2E tests pass with Worker thread architecture

## Dependencies

- `browser-crypto` (Rust crate) — AES-256-GCM via Web Crypto API in WASM
- `fake-indexeddb` (npm) — already in use
- `zeroize` (Rust crate) — volatile memory zeroing
- Small N-API native addon for `prctl`/`mlock` (or existing npm package)
- `worker_threads` (Node.js built-in)

## Decisions

- Forward secrecy by default, configurable for audit use cases
- Both crypto isolation (Worker) and command isolation (subprocess)
- Linux-first for OS hardening; graceful degradation on other platforms
- `browser-crypto` for WASM-side encryption; `crypto.subtle` for JS-side store encryption
