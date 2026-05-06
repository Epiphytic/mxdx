# Security Review: Rust/npm Binary Parity Phase 2

**Date:** 2026-04-30
**Scope:** Wire-format-parity gate (T-2.1 through T-2.8a)
**Reviewer:** Phase-2 BRAINS teammate (single mode)

## Scope

Files reviewed: all non-doc changes on `brains/rust-npm-binary-parity` vs `main` as of 2026-04-30.

Key surfaces:
- `crates/mxdx-test-perf/src/lib.rs` — new JSONL writer crate
- `crates/mxdx-worker/src/config.rs` + `main.rs` — `--p2p` flag addition
- `packages/e2e-tests/src/beta.js` — `spawnNpmBinary` subprocess helper
- `packages/e2e-tests/tests/rust-npm-interop-beta.test.js` — 8-combination test matrix
- `.github/workflows/ci.yml` — `wire-format-parity` and `wire-format-parity-p2p` jobs
- `scripts/lint-e2e-subprocess.mjs` — E2E discipline lint script
- `scripts/e2e-test-suite.sh` — unified perf output orchestration

## Secrets Scan

**Result: CLEAN**

- No hardcoded credentials, API keys, or tokens in any changed file.
- All credential access uses `creds.<field>` from `test-credentials.toml` (gitignored).
- CI jobs use `${{ secrets.TEST_CREDENTIALS_TOML }}` GitHub secret, never inlined.
- `test-credentials.toml` confirmed gitignored (`grep -n "test-credentials" .gitignore` → line 11).

## OWASP Assessment

**Relevant categories for this phase:**

| Category | Finding |
|----------|---------|
| Injection | No shell injection risk: all subprocess spawns use array args (no `shell: true`). `spawn(binPath, args, { ... })` and `spawn(process.execPath, [binPath, ...args], { ... })`. |
| Sensitive Data | Test credentials written to disk via `echo "$SECRET" > file` in CI. File is ephemeral (GitHub Actions runner), runner destroyed after job. Risk: acceptable for test infrastructure. |
| Broken Access Control | `--p2p` flag parsed but not yet wired to `P2pConfig` — no behavior change. P2P still routes through Matrix E2EE when flag is a no-op. |
| Security Misconfiguration | `wire-format-parity-p2p` job has `continue-on-error: true` — by design (advisory combinations). Non-blocking gate is policy, not misconfiguration. |
| Insufficient Logging | `mxdx-test-perf` write errors are logged and swallowed (`let _ = write_perf_entry(...)`) — acceptable for performance telemetry. A failed perf write does not affect test correctness. |

**Non-applicable categories:** XXE (no XML), XSS (no web output in changed code), Insecure Deserialization (no untrusted deserialization).

## Dependency Audit

**Rust (cargo audit):**
- `RUSTSEC-2024-0388`: `derivative` unmaintained — pre-existing, warning level, cosmetic.
- `RUSTSEC-2026-0097`: `rand` unsound with custom logger — pre-existing, warning level. Affects `rand::rng()` only with a custom global logger, which mxdx does not use in rand-calling paths. Transitive via `matrix-sdk-crypto → ulid → rand`. Not actionable without upstream matrix-sdk update.

**npm (npm audit):**
- `picomatch` high: ReDoS/method injection in `node_modules/npm/node_modules/tinyglobby/node_modules/picomatch` — nested inside npm itself, not in mxdx's code. Pre-existing.
- `postcss` moderate: XSS in CSS stringify — pre-existing, in dev dependencies.

**New dependencies introduced by phase-2:** `mxdx-test-perf` (new crate, no external deps beyond `serde`, `serde_json`, `anyhow`, `tempfile` in dev). No new npm packages added.

## Threat Model

**Assets:** Beta test credentials (homeserver URLs, usernames, passwords), E2E test result integrity.

**Trust boundaries:**

| Boundary | Risk | Mitigation |
|----------|------|------------|
| CI secret → disk file | Credential exposure on shared runner | GitHub Actions ephemeral runner; secret masked in logs; test-credentials.toml gitignored |
| `TEST_PERF_OUTPUT` env var | Attacker-controlled path traversal to write anywhere | Test-only crate; no production binary; JSONL content is benign timing data |
| `--p2p` CLI flag | Unexpected P2P transport activation | Flag parsed but NOT wired to `P2pConfig` until Phase 6; behavior identical to no-flag |
| Advisory `continue-on-error` jobs | Advisory failures silently hide real regressions | t4a/t4b are structurally separated from t1a-t3b; separate artifact upload; promotion policy in gate-policy doc |

**E2EE invariant:** The security grep gate (`scripts/check-no-unencrypted-sends.sh`) passes. No Matrix send calls are present in any phase-2 code. The interop test exclusively spawns subprocesses — all actual Matrix events are sent by the worker/launcher binaries which are covered by the existing E2EE audit.

## Findings

| Severity | Category | Finding | File | Status |
|----------|----------|---------|------|--------|
| Low | Misconfiguration | `wire-format-parity` job writes `test-credentials.toml` without `if: env.TEST_CREDENTIALS_TOML != ''` guard (unlike `npm-public-server` job which has the guard) | `.github/workflows/ci.yml` lines 413-416, 469-472 | **Accepted**: `wire-format-parity` requires credentials to run (no skip path exists); the job is gated to `workflow_dispatch + full_e2e`. The `npm-public-server` guard is for cases where the job might run without credentials. Here the job would fail anyway if credentials are absent, so the guard is unnecessary. |
| Info | Dependency | 2 pre-existing cargo audit warnings (`derivative`, `rand`) | Cargo.lock | Pre-existing, not introduced by phase-2 |
| Info | Dependency | 3 pre-existing npm audit findings | package-lock.json | Pre-existing, not introduced by phase-2 |

## Remediations Applied

None required. All findings are accepted risks or pre-existing issues not introduced by phase-2.

## Remaining Risks

1. **`RUSTSEC-2026-0097` (rand unsound with custom logger):** Transitive via matrix-sdk. Not exploitable in mxdx's usage. Will be resolved when matrix-sdk updates its rand dependency.

2. **Credential file on CI disk:** Ephemeral and acceptable for beta test infrastructure. The alternative (in-memory only) is not supported by the TOML-file credential loading model.

## E2EE Security Gate

```
ok: no forbidden patterns in crates/mxdx-p2p
self-test ok: synthetic violation correctly rejected
```

All E2EE invariants from CLAUDE.md are maintained. No Matrix send calls in phase-2 code.
