# Wrap-up: CI & E2E Test Fixes Post-P2P Merge

**Slug:** ci-e2e-test-fixes
**Paused:** false

## Per-Phase Summary

### Phase 1 — Quick Wins
- Tasks completed: 5/5 (T-01 through T-05)
- T-01: Replaced fixed 20s sleep with poll-based `wait_worker_ready` in t42 backup restore test
- T-02: Updated CLI syntax in 5 beta test files (`--server` → `start --homeserver`)
- T-03: Added WASM `.free()` calls in perf-terminal.test.js teardown
- T-04: Aligned command-round-trip event types with `SESSION_EVENTS` constants
- T-05: Added key exchange readiness poll in p2p-public-server.test.js
- Issues found (nurture): None
- Issues found (secure): None — all changes test-only

### Phase 2 — Null Contract Fix
- Tasks completed: 3/3 (T-06 through T-08)
- T-06: Patched WASM onRoomEvent at both Node.js and web entry points to coerce `'null'` → `null`
- T-07: Updated 15 production call sites across 7 files (scope 2.5x larger than ADR estimate of 6)
- T-08: Updated 35+ test assertions and mock clients across 9 files
- Issues found (nurture): Scope was larger than estimated — ADR listed 6 production sites but 15 existed
- Issues found (secure): None — changes are serialization-artifact fixes, no E2EE impact

### Phase 3 — Infrastructure
- Tasks completed: 2/2 (T-09, T-10)
- T-09: Moved 5 Playwright test files to `packages/e2e-tests/playwright/`, updated imports, added `test:playwright` script
- T-10: Replaced musl-tools + manual env vars with `cross` tool in CI `musl-build` job
- Issues found (nurture): None
- Issues found (secure): None — CI and test infrastructure only

## Outstanding Work

None — all 10 implementation tasks closed. The musl `continue-on-error: true` is retained pending 3 consecutive green CI runs (per ADR).

## Known Gaps and Limitations

1. **musl cross-compile may still fail** — the `cross` tool resolves the toolchain issue but datachannel-sys's C++ vendor build on musl hasn't been validated end-to-end. First CI run will confirm.
2. **Playwright test runner** — the `test:playwright` script is defined but Playwright may not be installed in CI. The files were crashing before; now they're simply not run by `node --test`. Running them requires Playwright infrastructure setup (separate effort).

## Commits (10)

1. `775ba82` docs(plans): add implementation map
2. `4d0ba30` fix: t42 backup restore poll-based wait
3. `6c6728a` fix: beta test CLI syntax
4. `5993cac` fix: WASM .free() in perf-terminal teardown
5. `cf2bc3c` fix: command-round-trip event types
6. `fb5770c` fix: p2p-public-server key exchange wait
7. `b841ffb` fix: WASM onRoomEvent null coercion
8. `5dd77b7` fix: production callers null checks
9. `7db7981` fix: test assertions null contract
10. `2eb105f` fix: Playwright isolation + musl cross tool
