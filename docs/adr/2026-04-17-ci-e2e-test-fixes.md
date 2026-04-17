# ADR 2026-04-17: CI and E2E test infrastructure fixes post-P2P merge

**Date:** 2026-04-17
**Status:** Accepted
**Decision makers:** Liam Helmer

## Context

The P2P branch merge (PR #20, 51 commits, 131 files) introduced native Rust P2P interactive sessions. Post-merge CI and local E2E test runs exposed 8 issues — 7 test bugs, 1 code bug, and 1 infrastructure limitation. None are security regressions (E2EE cardinal rule intact), but they cause CI noise and mask real failures.

The issues cluster into three categories:
1. **Test infrastructure misalignment** — Playwright tests running under Node.js runner, event type string mismatches, CLI flag drift, fragile timing assumptions.
2. **Contract inconsistency** — `P2PTransport.#waitForInbox` returns JS `null` while the WASM `onRoomEvent` API returns the string `'null'`, breaking callers that check one way.
3. **CI infrastructure gap** — musl cross-compile fails because `musl-tools` on Ubuntu lacks a C++ musl runtime for datachannel-sys's vendor build.

## Decision

Fix all 8 issues in a single coordinated effort. Three decisions required user input:

1. **Playwright test isolation:** Move Playwright-only test files to `packages/e2e-tests/playwright/`. This physically separates them from `node --test`-compatible files, preventing accidental cross-runner execution.

2. **`null` contract unification:** Fix both sides — `P2PTransport.#waitForInbox` and `#clearAllWaiters` continue returning JS `null` (already correct), and the WASM `onRoomEvent` wrapper is updated to also return JS `null` instead of the string `'null'`. This gives callers a uniform, JS-idiomatic contract. Test assertions are updated to match.

3. **Musl cross-compile:** Fix the CI job using the `cross` tool (or musl.cc cross-toolchain) to provide a proper musl C++ environment. This validates the Alpine deployment path used by mxdx-hosting.

## Requirements (RFC 2119)

### Test Infrastructure
- Playwright test files MUST reside in `packages/e2e-tests/playwright/`, NOT in `packages/e2e-tests/tests/`.
- `node --test` globs in CI MUST NOT include `packages/e2e-tests/playwright/`.
- Playwright tests SHOULD be runnable via a dedicated npm script (e.g., `npm run test:playwright`).
- New Playwright tests MUST be placed in the `playwright/` directory.

### P2P Transport Contract
- `P2PTransport.#waitForInbox` MUST resolve with JS `null` (not the string `'null'`) when no event is received within the timeout.
- The WASM `onRoomEvent` wrapper in `@mxdx/core` MUST return JS `null` (not the string `'null'`) when the underlying WASM returns a null/empty result.
- All callers of `onRoomEvent` MUST compare against `null`, not `'null'`. This includes production call sites in `packages/launcher/src/runtime.js` (lines 120, 148, 1496, 1562, 1605) and `packages/client/src/interactive.js` (line 53) that currently check `!== 'null'` or `=== 'null'`.

### E2E Test Reliability
- E2E tests that wait for async processes (worker startup, key exchange, backup download) MUST use poll-based readiness checks, NOT fixed-duration sleeps.
- E2E tests that send E2EE events MUST wait for key exchange completion before sending.
- E2E tests that create WASM clients MUST call `.free()` on them in teardown.
- E2E test event type strings MUST match the constants defined in the runtime they're testing (e.g., `SESSION_EVENTS` in `runtime.js`).
- Beta test harness MUST use the current CLI syntax for spawning binaries (`mxdx-worker start --homeserver` not `mxdx-worker --server`).

### CI Infrastructure
- The musl cross-compile job MUST use a toolchain that provides both `musl-gcc` and a musl-compatible C++ compiler.
- The musl job MUST use the `cross` tool (or equivalent musl.cc cross-toolchain) for reproducible cross-compilation, replacing the current broken `musl-tools` + manual env var approach.
- The musl job MAY be promoted from `continue-on-error: true` to a blocking check once it passes reliably for 3 consecutive runs.

## Rationale

- **Playwright directory separation** over file renaming: Physical separation is enforceable by glob patterns and discoverable by convention. Renaming files (`*.playwright.test.js`) is fragile — a new contributor might not know the convention. A separate directory makes the boundary obvious and enables different `package.json` scripts.
- **Uniform `null` contract** over mixed types: The string `'null'` is a WASM serialization artifact, not a meaningful semantic value. JS callers should never need to check `=== 'null'` — it's error-prone and surprising. Fixing at the WASM boundary keeps the rest of the codebase idiomatic.
- **`cross` tool for musl** over Docker images or deferral: `cross` is the standard Rust cross-compilation tool, handles toolchain provisioning automatically, and works in GitHub Actions with minimal configuration. It avoids maintaining custom Docker images.

## Alternatives Considered

### Playwright isolation: Rename to `*.playwright.test.js`
- Pros: No file moves, smaller diff.
- Cons: Convention-based (easy to forget), doesn't prevent `node --test **/*.test.js` from picking them up without explicit excludes.
- Why rejected: Physical separation is more robust than naming conventions.

### Playwright isolation: Early-exit guard in each file
- Pros: No structural changes.
- Cons: `process.exit(0)` in test files is an anti-pattern; masks real import errors; requires env var discipline.
- Why rejected: Fragile and non-obvious to new contributors.

### null contract: Fix only the tests
- Pros: Minimal code change.
- Cons: Leaves a contract split — WASM returns `'null'`, JS transport returns `null`. Future callers will hit the same bug.
- Why rejected: Fixing the symptom without fixing the cause guarantees recurrence.

### Musl: Defer indefinitely
- Pros: Zero effort.
- Cons: Blocks Alpine deployment validation; CI stays red (even if non-blocking).
- Why rejected: mxdx-hosting targets Alpine; validating the build path now catches issues before production deployment.

## Consequences

- 5 Playwright test files move from `packages/e2e-tests/tests/` to `packages/e2e-tests/playwright/`.
- `onRoomEvent` WASM wrapper gains a `null` coercion step at the JS/WASM boundary.
- `p2p-transport.js` `#waitForInbox` and `#clearAllWaiters` retain current `null` return (no change needed — already correct).
- 3 test assertions updated from `'null'` to `null`.
- Production callers in `runtime.js` (5 sites) and `interactive.js` (1 site) updated from `!== 'null'` / `=== 'null'` to `!== null` / `=== null` / `== null` as appropriate.
- `e2e_profile.rs` t42 replaces 20s sleep with poll loop.
- `beta.js` updated to use `start --homeserver` CLI syntax.
- `command-round-trip.test.js` event types aligned with `SESSION_EVENTS` constants.
- `perf-terminal.test.js` teardown adds `.free()` calls.
- `p2p-public-server.test.js` adds key exchange readiness wait.
- CI `musl-build` job switches to `cross` tool; `continue-on-error` retained until 3 green runs.

## Council Input

Star-chamber review (parallel mode, 2026-04-17) flagged two material issues, both integrated:

1. **Null contract fix scope understated.** The original ADR listed only 3 test assertion changes but missed 6 production call sites in `runtime.js` and `interactive.js` that check `=== 'null'` / `!== 'null'`. Updated the Requirements and Consequences sections to enumerate these.
2. **Musl `cross` requirement too weak.** SHOULD was insufficient for a known-broken configuration. Promoted to MUST.

No security issues identified. All other decisions confirmed as sound.
