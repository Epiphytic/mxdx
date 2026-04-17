# Research: CI & E2E Test Fixes Post-P2P Merge

**Date:** 2026-04-17
**Slug:** ci-e2e-test-fixes

## Issues Investigated

### 1. t42 backup restore decryption failure
- **Root cause:** Fixed 20s sleep in `e2e_profile.rs:1740` is too short for worker_b to initialize, upload device keys, post telemetry, and have the client daemon complete backup download.
- **Error:** `daemon error -7` (MATRIX_UNAVAILABLE, `protocol/error.rs:15`) — daemon's 30s `wait_for_matrix` in `handler.rs:208` expires.
- **Fix:** Replace fixed sleep with `wait_worker_ready` poll pattern (already used elsewhere in the file at line 1718).
- **Type:** Test bug. Complexity: Small.

### 2. mxdx-worker CLI rejects --server flag
- **Root cause:** Worker CLI uses subcommand structure: `mxdx-worker start --homeserver <url>`. Beta test harness (`packages/e2e-tests/src/beta.js`) passes `--server` (old syntax).
- **Fix:** Update beta.js and 5 test files to use `['start', '--homeserver', url]`.
- **Type:** Test bug. Complexity: Trivial.

### 3. Playwright test.skip() incompatible with Node.js test runner
- **Root cause:** 5 files import from `@playwright/test` and call `test.skip()` at module scope. Crashes when run via `node --test`.
- **Files:** web-console.test.js, session-persistence.test.js, p2p-web-console.test.js, public-session-persistence.test.js, web-console-rust-p2p-beta.test.js
- **Decision:** Move to `packages/e2e-tests/playwright/` directory.
- **Type:** Test infrastructure. Complexity: Trivial.

### 4. P2P assertion bug — null vs 'null'
- **Root cause:** `P2PTransport.#waitForInbox` (p2p-transport.js:737) and `#clearAllWaiters` (line 751) resolve with JS `null`. WASM `onRoomEvent` returns string `'null'`. Contract inconsistency.
- **Decision:** Fix both sides — transport returns `null`, WASM wrapper also returns `null`. Uniform JS-idiomatic contract.
- **Type:** Code bug + test bug. Complexity: Trivial.

### 5. Public server signaling timeouts
- **Root cause:** `p2p-public-server.test.js` sends m.call.invite before E2EE key exchange completes. 2s sleep insufficient on public server.
- **Fix:** Add key exchange readiness wait loop after room join, before sending signaling events.
- **Type:** Test bug. Complexity: Small.

### 6. WASM async cleanup leak
- **Root cause:** `perf-terminal.test.js` `after()` hook doesn't call `launcherClient.free()` / `clientClient.free()`. Dangling WASM timers fire post-test.
- **Fix:** Add `.free()` calls before sleep in teardown.
- **Type:** Test bug. Complexity: Trivial.

### 7. command-round-trip.test.js timeout
- **Root cause:** Test sends `org.mxdx.command` but launcher listens for `org.mxdx.session.task`. Response type also mismatched (`org.mxdx.result` vs `org.mxdx.session.result`).
- **Fix:** Update event type strings to match `SESSION_EVENTS` constants in `runtime.js:16-27`.
- **Type:** Test bug. Complexity: Small.

### 8. Musl cross-compile
- **Root cause:** `musl-tools` on Ubuntu lacks `musl-g++`. datachannel-sys vendor build needs C++ musl runtime.
- **Decision:** Fix with `cross` tool or musl.cc cross-toolchain.
- **Type:** Infrastructure. Complexity: Medium.
