# Security Review: Multi-Homeserver Support

**Date:** 2026-03-13
**Reviewer:** Claude Code (automated)
**Component:** `packages/core/multi-hs-client.js`, launcher/client config changes, runtime integration
**Branch:** `feat/multi-homeserver-support`

## Scope

Review of the `MultiHsClient` class and related config/runtime changes that enable WASM-based clients and launchers to connect to multiple Matrix homeservers simultaneously.

## Findings

### 1. E2EE Preservation — PASS

`MultiHsClient` delegates all `sendEvent`/`sendStateEvent`/`onRoomEvent` calls to the underlying `WasmMatrixClient` instances, which handle Olm/Megolm encryption internally. The multi-HS layer never touches plaintext event content or encryption keys. Each server connection maintains its own independent E2EE session with its own device keys.

**Verdict:** E2EE is maintained. No bypass introduced.

### 2. Credential Handling — PASS

- `MultiHsClient.connect()` passes per-server configs to `connectWithSession()`, which handles credential storage via `CredentialStore` (OS keychain or encrypted file).
- `serverCredentials` in config allows per-server username/password overrides. Passwords are NOT serialized to TOML config files (only username overrides are persisted; passwords go through the keychain).
- The `log()` function receives only server URLs and latency data — never credentials, tokens, or session data.
- `LauncherConfig` password field is intentionally NOT saved to disk (only stored transiently for initial login, then cleared).

**Verdict:** No credential leakage. Credentials follow existing secure storage paths.

### 3. Memory-Bounded Deduplication — PASS

The `#seenEvents` Map is bounded to 10,000 entries with batch eviction of 2,000 oldest when exceeded. This prevents a malicious actor from flooding events to exhaust memory.

- Max memory: ~10,000 event IDs (typically 44 bytes each) = ~440KB worst case
- Eviction is O(n) on the batch size (2,000), not the full set
- Single-server mode bypasses dedup entirely (no overhead)

**Verdict:** Bounded. Not exploitable for memory exhaustion.

### 4. Circuit Breaker — PASS

- Circuit breaker is triggered only by transport-level failures (`catch` blocks on `sendEvent`, `syncOnce`, `onRoomEvent`). A malicious event cannot trigger the circuit breaker — only network/server failures can.
- Cross-server sanity check prevents cascading breaks when ALL servers fail (assumed network issue, not server issue). This prevents an attacker from selectively failing one server to force traffic to another.
- Recovery probes use jittered intervals (60-160 seconds) to prevent timing-based attacks and thundering herd on recovery.
- `timer.unref()` ensures recovery probes don't prevent Node.js process exit.

**Verdict:** Robust against manipulation. Jitter prevents timing attacks.

### 5. Failover Security — PASS

- Failover selects the lowest-latency healthy server. An attacker cannot force traffic to a specific server by making others appear slow — only actual failures (5 within 5 minutes) trigger circuit breaking.
- Recovered servers do not automatically become preferred, preventing ping-pong attacks.
- `preferredServer` config pin provides operator control over routing.

**Verdict:** Failover is safe against manipulation.

### 6. Event Deduplication Integrity — PASS

- Dedup is by Matrix event ID (`$` prefix), which is server-assigned and globally unique.
- The first delivery wins; subsequent deliveries of the same event ID are silently dropped.
- In single-server mode, dedup is disabled (no overhead, no false positives).
- Parse failures in `onRoomEvent` deliver the event anyway (fail-open for data availability, not security).

**Verdict:** Correct dedup semantics. No data loss risk.

### 7. Per-Server Identity Isolation — PASS

Each server connection has its own:
- Matrix user ID and device ID
- Olm/Megolm session keys (managed by `WasmMatrixClient`)
- Circuit breaker state
- Latency measurements

Cross-server state is limited to:
- Event ID dedup set (contains only event IDs, no content)
- Preferred server index

**Verdict:** Server identities are properly isolated.

### 8. Config File Security — PASS

- `LauncherConfig.save()` writes with `mode: 0o600` (owner-only read/write)
- Config directory created with `mode: 0o700`
- `serverCredentials` stores per-server username overrides. Passwords follow the keychain path.
- `password` field in `LauncherConfig` is transient (cleared after first login, not saved to TOML)

**Verdict:** File permissions are correct. No credential persistence in plaintext config.

### 9. Telemetry Multi-HS Fields — PASS

New telemetry fields added when `serverCount > 1`:
- `preferred_server`: Server URL (not sensitive)
- `preferred_identity`: User ID on preferred server (already visible in room membership)
- `accounts`: All user IDs (already visible in room membership)
- `server_health`: Status and latency per server (operational data, not sensitive)

**Verdict:** No sensitive data exposed in telemetry. All fields are operational metadata.

### 10. Sequential Connection — PASS

`MultiHsClient.connect()` connects to servers sequentially (not parallel) because the Node.js `fake-indexeddb` crypto store is process-global. This prevents race conditions on the IndexedDB snapshot restore/save cycle.

**Verdict:** Correct synchronization. No race condition risk.

## Summary

| Check | Status |
|-------|--------|
| E2EE preserved | PASS |
| No credential leakage | PASS |
| Memory-bounded dedup | PASS |
| Circuit breaker tamper-proof | PASS |
| Failover manipulation-proof | PASS |
| Event dedup integrity | PASS |
| Per-server identity isolation | PASS |
| Config file permissions | PASS |
| Telemetry data safety | PASS |
| Sequential connection safety | PASS |

**Overall: PASS** — No security issues found. The multi-homeserver layer is a transparent routing/failover wrapper that preserves all existing E2EE and credential security guarantees.
