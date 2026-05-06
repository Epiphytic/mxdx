# Security Review: rust-npm-binary-parity Phase 1

**Date:** 2026-04-30
**Reviewer:** Phase-1 BRAINS teammate (T5)
**Scope:** Phase 1 changes — `mxdx-test-perf` crate, CI lint job, integration-tests package, test reclassification

## Scope

Files reviewed:
- `crates/mxdx-test-perf/src/lib.rs` + `Cargo.toml`
- `crates/mxdx-worker/Cargo.toml` (dev-dep addition only)
- `crates/mxdx-worker/tests/e2e_profile.rs` (perf emission changes only)
- `scripts/lint-e2e-subprocess.mjs`
- `packages/integration-tests/package.json`, `src/tuwunel.js`
- `packages/integration-tests/tests/*.test.js` (4 files)
- `packages/e2e-tests/tests/rust-npm-interop-beta.test.js`
- `.github/workflows/ci.yml` (lint job addition)

## Secrets Scan

**Clean.** No hardcoded credentials, API keys, or tokens found in Phase 1 changes.

- `public-server-wasm.test.js` reads `password` fields from `test-credentials.toml` — file is gitignored (`.gitignore` confirmed), never committed.
- No base64-encoded secrets, no hardcoded URLs with credentials.

## OWASP Assessment

Phase 1 is test infrastructure only. No production code paths modified. Relevant categories:

| Category | Assessment |
|----------|------------|
| Injection | `lint-e2e-subprocess.mjs`: reads repo files from a hardcoded relative path, no user-controlled input. No injection surface. |
| Sensitive Data | `write_perf_entry()` emits `suite`, `transport`, `runtime`, `duration_ms`, `rss_max` — no PII, no credentials. `TEST_PERF_OUTPUT` is CI-controlled. |
| Broken Auth | N/A — no auth code introduced. |
| Insecure Deserialization | `public-server-wasm.test.js` parses TOML credentials with a hand-rolled parser reading known-structure files. Input is local, not network-sourced. |
| Security Misconfiguration | CI `e2e-subprocess-lint` job uses `continue-on-error: true` intentionally (5-business-day window); `actions/checkout@v4` + `actions/setup-node@v4` pinned to current version. Acceptable. |
| XSS | N/A — no browser-facing code introduced. |

## Dependency Audit

**Rust (cargo audit):** 2 pre-existing allowed warnings:
- `RUSTSEC-2024-0388`: `derivative` unmaintained — transitive via `matrix-sdk-indexeddb 0.16.0`. Not introduced by Phase 1 (`mxdx-test-perf` adds only `serde`, `serde_json`, `anyhow`, `tempfile`).
- `RUSTSEC-2026-0097`: `rand` unsound with custom logger — transitive via `matrix-sdk-crypto 0.16.0`. Not Phase 1.

**npm (npm audit):** 3 pre-existing vulnerabilities:
- `brace-expansion` ReDoS (high) — in `node_modules/npm/...` (npm tool's own bundled deps)
- `picomatch` ReDoS (moderate) — same
- `postcss` (moderate) — pre-existing in Vite devDeps

None are introduced by Phase 1. `@mxdx/integration-tests` adds only `@mxdx/core` as a dependency.

## Threat Model

**Assets:** Test output files (JSONL), test credentials (gitignored), CI pipeline integrity.

**Trust Boundaries and Vectors:**

| Boundary | Vector | Mitigation |
|----------|--------|------------|
| `TEST_PERF_OUTPUT` env var → file write | Path traversal (write to sensitive file) | Acceptable: test-only crate (`publish=false`), env var is CI-controlled. Writes are append-only JSONL. |
| Lint script reading test files | File read outside test directory | Hardcoded `path.resolve(__dirname, '..')` — no user-controlled path. |
| Integration test → Tuwunel | Network, registration | Tuwunel is ephemeral local instance; test accounts are disposable. |
| `test-credentials.toml` → public server | Credential leak | File gitignored; CLAUDE.md mandates never commit. |

**E2EE invariant compliance:** Verified. `launcher-onboarding-wasm.test.js` exec/logs room tests exercise the E2EE round-trip path via `sendEvent()` + `collectRoomEvents()` (decrypted output). MSC4362 encrypted state event path is covered by room creation via `getOrCreateLauncherSpace()`. Telemetry state event test updated with explicit E2EE traceability comment.

## Findings

| Severity | Category | Finding | File | Status |
|----------|----------|---------|------|--------|
| Low | Accepted Risk | `TEST_PERF_OUTPUT` write-to-arbitrary-path if env compromised | `lib.rs` | Accepted — test-only, CI-controlled env |
| Informational | Transitive Advisory | `derivative` unmaintained (RUSTSEC-2024-0388) | Cargo.lock | Pre-existing, not Phase 1 |
| Informational | Transitive Advisory | `rand` unsound (RUSTSEC-2026-0097) | Cargo.lock | Pre-existing, not Phase 1 |

## Remediations Applied

None required. No critical or high severity issues found.

Nurture-phase fixes (P0/P1 from star-chamber) already committed in `ea355b9`:
- ENV_LOCK mutex for parallel test safety
- Doc clarity on `write_perf_entry()` error behavior
- Lint script comment accuracy + tightened regex
- Header comment fixes for reclassified tests
- E2EE traceability in telemetry state event test

## Remaining Risks

- `TEST_PERF_OUTPUT` write-to-arbitrary-path: accepted. Test infrastructure only; mitigated by `publish=false`.
- Pre-existing transitive advisories: tracked by upstream matrix-sdk maintainers. Not Phase 1 responsibility.

## Council Feedback

Not invoked (single mode). Star-chamber review was conducted in Nurture (T4) and identified no security issues beyond E2EE traceability clarity (addressed).
