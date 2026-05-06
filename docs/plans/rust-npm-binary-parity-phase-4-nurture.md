# Phase 4 Nurture Report

**Topic:** rust-npm-binary-parity
**Phase:** 4 — WASM Expansion
**Date:** 2026-05-05 (orchestrator-driven recovery after teammate stall)
**Mode:** --parallel (star-chamber consulted via review subagent)

## Review Summary

- Files reviewed: 8 (commits ad00f20, a5882aa, 78b1fe4, 249b254, cd8db4b plus security review doc)
- Issues found: 14 (P0: 3, P1: 8, P2: 3)
- Issues fixed in nurture: 11 (3 P0, 8 P1)
- Issues filed as `brains:cleanup` beads: 3 (P2 + 1 P1 deferred for cross-package migration)

## Background

The phase-4 teammate stalled before invoking nurture/secure. Orchestrator (team-lead) took over the umbrella tasks directly, ran the review, presented P0 findings to the project owner for the architectural-judgment items (P0-2 migration completeness, P0-3 space-room encryption), then dispatched a fix subagent for the agreed-upon resolutions.

## Issues Fixed

### P0-1 — `serde_wasm_bindgen` anti-pattern → JSON-string return (commit `7b1807a`)
- `WasmMatrixClient::create_launcher_space` and `find_launcher_space` returned empty `{}` to JS callers via `serde_wasm_bindgen::to_value(&LauncherTopology)`. Project memory documents this as a known anti-pattern.
- Fix: both call sites return `serde_json::to_string(&topology)`; JS callers (`packages/launcher/src/runtime.js`, `packages/client/src/discovery.js`) `JSON.parse(...)` the response.
- Doc comment added to `LauncherTopology` warning future contributors.

### P0-2 — Complete WasmBatchedSender migration with 429 backoff (commit `8ab2611`)
- User selected Option A (full migration). Approach: structured-retry-action — Rust owns compression + state, JS owns timing.
- Rust state machine: `push(bytes)` / `takePayload() → JSON|null` / `markSent` / `markRateLimited` / `markError` / `parseRetryAfterMs`. Coalesce-on-retry retains in-flight bytes and merges with newly buffered chunks on next `takePayload()`, preserving the original sequence.
- JS thin wrapper: `packages/launcher/src/batched-sender-wasm.js::BatchedSenderWasm` mirrors the legacy `BatchedSender` public API.
- Wired into `runtime.js:188` — legacy `BatchedSender` no longer in launcher hot path.
- Cross-package callers (`packages/core/terminal-socket.js`, `packages/web-console/src/terminal-socket.js`) deferred as `brains:cleanup` (mxdx-xbfr). Legacy `BatchedSender` retained with deprecation header until those callers migrate.

### P0-3 — Encrypt launcher space room under MSC4362 (commit `433ff34`)
- User selected Option A (encrypt the space room). Pattern matches `create_named_encrypted_mxdx_room` already used for exec/logs rooms: `m.room.encryption` (`m.megolm.v1.aes-sha2`) plus `with_encrypted_state()` for MSC4362.
- The two `m.space.child` state events linking exec/logs to the space are now E2EE on the wire.
- Security review doc updated: §1 Send Call Table now enumerates the `m.space.child` send with verdict `ENCRYPTED via MSC4362`. A `2026-04-30 update` note above the table documents the bug + fix.

### P1 — Cross-reference citation accuracy (commit `5bf30c6`)
- `runtime.js:15`: cited private `compress_terminal_data` → corrected to public `compress_terminal_data_wasm` (exported as `compressTerminalData`).
- `session-mux.js:6`: cited semantically-wrong `SessionTransportManager` → corrected to "none — PTY I/O multiplexing is OS-bound via node-pty (see ADR Pillar 3 OS-bound table)". ADR Pillar 3 OS-bound table updated with `session-mux.js` row.
- `packages/core/batched-sender.js`: header comment added pointing at `WasmBatchedSender`.
- `Cargo.toml:44`: `wasm-bindgen-test` ↔ `wasm-bindgen` version-pairing comment added.

### P1 — Test coverage for `WasmSessionManager` (commit `8ff0440`)
- 6 new tests added to `runtime-unit.test.js` covering: `list_sessions` (with security check that sender/dm_room_id are not leaked), `session_cancel` → `kill_pty/SIGTERM`, `session_signal` → `kill_pty/<custom>`, unknown-UUID `session_signal` silent no-op, `spawn_pty` happy path (active sessions counter increments, `batch_ms` negotiated), `spawn_pty` rejection on disallowed command.
- 11 new tests in `batched-sender-wasm.test.js` covering the WASM state machine + JS wrapper.

### P1 — Security review §1 enumeration (folded into commit `433ff34`)
- §1 Send Call Table now lists 16+ send call sites (was 15+).
- The `m.space.child` send is enumerated with line citation.
- Summary line updated.

### P1 — MANIFEST regenerated (commit `d851056`)
- MANIFEST.md was red on CI since phase-4 ship; now includes `WasmBatchedSender`, `WasmSessionManager`, `SessionTransportManager`, `build_telemetry_payload` symbols.

## Tests Added

- 6 `WasmSessionManager` test cases in `runtime-unit.test.js`
- 11 `WasmBatchedSender` test cases in `batched-sender-wasm.test.js`

## Deferred Items (filed as `brains:cleanup`)

| ID | Priority | Title |
|---|---|---|
| `mxdx-xbfr` | P1 | Migrate `terminal-socket.js` callers off legacy JS `BatchedSender` |
| `mxdx-072h` | P2 | Audit `WasmSessionManager` `sessionSender` / `sessionDmRoomId` exposed methods |
| `mxdx-6sxb` | P2 | Replace hand-rolled base64 in `mxdx-core-wasm` with `base64` crate |

## Test Results

| Check | Result |
|---|---|
| `cargo check --workspace --exclude mxdx-core-wasm` | clean (only pre-existing dead-code warnings in mxdx-fabric-cli) |
| `cargo check -p mxdx-core-wasm --target wasm32-unknown-unknown` | clean |
| `wasm-pack build --target nodejs` | success (20.3 MB optimized, 6 min build) |
| `wasm-pack build --target web` | success (20.3 MB optimized, 6 min build) |
| `node --test` on launcher tests | **58 pass / 0 fail** (12 suites) |

## Commits on `brains/rust-npm-binary-parity` (nurture-fix package)

```
4d3ed3f chore: file P2 cleanup beads from phase-4 nurture review
d851056 chore: update MANIFEST.md with Phase-4 WASM symbols
8ff0440 test(wasm): add WasmSessionManager and WasmBatchedSender coverage
8ab2611 feat(wasm): complete WasmBatchedSender migration with 429 backoff (P0-2)
433ff34 fix(security): encrypt launcher space room under MSC4362 (P0-3)
5bf30c6 fix(docs): correct cross-reference citations in JS thin wrappers
7b1807a fix(wasm): return JSON string from create/find_launcher_space (P0-1)
```

All pushed to `origin/brains/rust-npm-binary-parity`.

## Architectural Note (post-nurture, 2026-05-05)

The project owner provided a topology directive after nurture completed. The directive does not invalidate any of the nurture-fix work above — phase-4 commits as landed are sound under the corrected topology. Specifically:

- The space-room encryption fix (P0-3) is required regardless of topology
- The WasmBatchedSender migration (P0-2) is room-agnostic
- The serde_wasm_bindgen fix (P0-1) is correct under any topology

Future work introduced by the topology directive (telemetry room shift, private worker state room, `launcher`→`worker` rename) will be captured in an ADR amendment and new beads tasks; these do not block phase-4 closure.

## Phase Closure

- `Nurture: phase 4` umbrella (mxdx-uthj): closed by this report.
- Next: `/brains:secure --scope phase-4` (mxdx-fbo7), then completion marker.
