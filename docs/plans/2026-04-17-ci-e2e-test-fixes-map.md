# Plan: CI & E2E Test Fixes Post-P2P Merge

**Slug:** ci-e2e-test-fixes
**ADRs:** docs/adr/2026-04-17-ci-e2e-test-fixes.md
**Research:** docs/plans/2026-04-17-ci-e2e-test-fixes-research.md
**Mode:** --parallel
**Autopilot:** true
**Branch:** brains/ci-e2e-test-fixes

## Phase 1 — Quick Wins (CI Noise Elimination)

All tasks are isolated to test files only. No production code. No inter-dependencies.

### T-01: Fix t42 backup restore — poll-based wait
Replace fixed 20s sleep in `crates/mxdx-worker/tests/e2e_profile.rs:1740` with poll loop using `wait_worker_ready` pattern (line 1718).
**Acceptance:** `cargo test -p mxdx-worker --test e2e_profile -- t42` passes. No fixed sleep at wait point.

### T-02: Fix CLI syntax in beta.js and 5 test files
Update `packages/e2e-tests/src/beta.js` and 5 beta test files: `--server` → `start --homeserver`.
**Acceptance:** No `--server` flag in test suite. Beta tests reach worker-started state without CLI parse error.

### T-03: Fix WASM cleanup leak in perf-terminal.test.js
Add `.free()` calls for `launcherClient` and `clientClient` in `after()` hook before `sleep(500)`.
**Acceptance:** `node --test packages/e2e-tests/tests/perf-terminal.test.js` exits cleanly.

### T-04: Fix command-round-trip event type strings
Update `packages/e2e-tests/tests/command-round-trip.test.js`: `org.mxdx.command` → `org.mxdx.session.task`, `org.mxdx.result` → `org.mxdx.session.result`.
**Acceptance:** Test receives result event within deadline.

### T-05: Add key exchange wait in p2p-public-server.test.js
Add poll-based key exchange readiness check after room join, before sending `m.call.invite`.
**Acceptance:** Test no longer times out on public servers due to premature signaling.

## Phase 2 — Null Contract Fix (Production + Tests)

Touches production code. T-06 must land before T-07 and T-08.

### T-06: Fix WASM onRoomEvent to return JS null
Add null coercion in `packages/core` wrapper: when WASM returns string `'null'`, convert to JS `null`.
**Acceptance:** `onRoomEvent` never returns the string `'null'`. Generated WASM files unmodified.
**Dependencies:** None.

### T-07: Update production callers of onRoomEvent
Update 6 sites: `runtime.js` (5 sites) and `interactive.js` (1 site) — `=== 'null'` / `!== 'null'` → null checks.
**Acceptance:** No `'null'` string comparisons in `packages/launcher/src/` or `packages/client/src/`.
**Dependencies:** T-06.

### T-08: Update test assertions for null contract
Update 3 test assertion sites: `strictEqual(result, 'null')` → `strictEqual(result, null)`.
**Acceptance:** All 3 tests pass. No `'null'` assertions remain for onRoomEvent returns.
**Dependencies:** T-06.

## Phase 3 — Infrastructure

Independent of all other phases.

### T-09: Move Playwright tests to packages/e2e-tests/playwright/
Move 5 files (web-console.test.js, session-persistence.test.js, p2p-web-console.test.js, public-session-persistence.test.js, web-console-rust-p2p-beta.test.js). Add `test:playwright` npm script. Ensure `node --test` globs exclude playwright/.
**Acceptance:** No `@playwright/test` imports in `tests/`. `node --test tests/**` clean. Relative imports updated.

### T-10: Switch musl CI job to cross tool
Rewrite `musl-build` job: remove musl-tools + manual env vars, use `cross build`. Retain `continue-on-error: true`.
**Acceptance:** Job uses `cross build`, no musl-g++ references. Cites ADR in comment.

## Dependency Graph

```
Phase 1: T-01, T-02, T-03, T-04, T-05 (all parallel, no deps)
Phase 2: T-06 → {T-07, T-08}
Phase 3: T-09, T-10 (all parallel, no deps)
```
