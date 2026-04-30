# Rust E2E / Integration Test Audit

**Date:** 2026-04-29
**Auditor:** Phase-1 BRAINS teammate (T-1.8)
**ADR:** docs/adr/2026-04-29-rust-npm-binary-parity.md req 26, 31

## Scope

All `e2e_*.rs` and `integration_*.rs` test files under `crates/*/tests/`.

## Audit Criteria

- **(a) Spawns binary subprocess** — calls `Command::new`, `cargo_bin()`, or equivalent to run a compiled binary as a child process. Tests that call library code directly (`MatrixClient::register_and_connect`, etc.) without spawning a binary are NOT E2E tests per CLAUDE.md.
- **(b) Emits perf JSONL via mxdx-test-perf** — calls `mxdx_test_perf::write_perf_entry` (or previously: wrote to `TEST_PERF_OUTPUT` via `worker_log_path`).

## Audit Results

| File | Crate | Spawns Binary (a) | Emits Perf (b) | Classification | Notes |
|---|---|---|---|---|---|
| `crates/mxdx-worker/tests/e2e_profile.rs` | mxdx-worker | YES | YES (after T-1.2) | E2E | Orchestrated phased suite; spawns `mxdx-worker` + `mxdx-client` binaries. Uses `write_perf_entry` after T-1.2. |
| `crates/mxdx-worker/tests/e2e_binary.rs` | mxdx-worker | YES | NO | E2E | Spawns `mxdx-worker` + `mxdx-client` binaries via `cargo_bin`. No perf emission. |
| `crates/mxdx-worker/tests/e2e_binary_beta.rs` | mxdx-worker | YES | NO | E2E | Spawns binaries against beta credentials. No perf emission. |
| `crates/mxdx-worker/tests/integration_session.rs` | mxdx-worker | NO | NO | Integration | Calls library code directly (session event schema, Tuwunel). Correctly named as integration. |
| `crates/mxdx-client/tests/integration_session.rs` | mxdx-client | NO | NO | Integration | Calls MatrixClient, client modules (submit, tail, ls, logs, cancel, reconnect) directly. Correctly named. |
| `crates/mxdx-launcher/tests/e2e_command.rs` | mxdx-launcher | NO | NO | Misclassified | Uses MatrixClient::register_and_connect directly. No binary subprocess. **See below.** |
| `crates/mxdx-launcher/tests/e2e_full_system.rs` | mxdx-launcher | NO | NO | Misclassified | Library-level test against Tuwunel. No binary subprocess. **See below.** |
| `crates/mxdx-launcher/tests/e2e_public_server.rs` | mxdx-launcher | NO | NO | Misclassified | Library-level test against public server. No binary subprocess. **See below.** |
| `crates/mxdx-launcher/tests/e2e_terminal_session.rs` | mxdx-launcher | YES (1 call) | NO | Borderline E2E | Has one `std::process::Command` call. Primarily library-level but includes subprocess invocation. Keep as-is. |
| `crates/mxdx-secrets/tests/e2e_secret_request.rs` | mxdx-secrets | NO | NO | Misclassified | Library-level secret request test via MatrixClient. No binary subprocess. **See below.** |
| `crates/mxdx-fabric/tests/e2e_fabric.rs` | mxdx-fabric | YES (2 calls) | NO | E2E | Spawns processes via `std::process::Command`. No perf emission. |

## Perf Emission Gap

Files classified as E2E (spawn binary) but lacking `write_perf_entry`:

- `crates/mxdx-worker/tests/e2e_binary.rs` — missing perf (filed as brains:cleanup)
- `crates/mxdx-worker/tests/e2e_binary_beta.rs` — missing perf (filed as brains:cleanup)
- `crates/mxdx-fabric/tests/e2e_fabric.rs` — missing perf (filed as brains:cleanup)

## Misclassified Files (name says e2e_ but no subprocess)

These files are named `e2e_*.rs` but do not spawn binary subprocesses:

- `crates/mxdx-launcher/tests/e2e_command.rs`
- `crates/mxdx-launcher/tests/e2e_full_system.rs`
- `crates/mxdx-launcher/tests/e2e_public_server.rs`
- `crates/mxdx-secrets/tests/e2e_secret_request.rs`

These are library integration tests using `MatrixClient` directly. They are valuable tests and should be kept, but renamed to `integration_*.rs`. Filed as brains:cleanup issues (see below). Not renamed in this task to avoid scope creep — renaming requires updating CI references.

## Follow-Up Issues Filed

Beads issues with `brains:cleanup` label filed for:
1. Add `mxdx-test-perf` perf emission to `e2e_binary.rs` and `e2e_binary_beta.rs`
2. Add `mxdx-test-perf` perf emission to `e2e_fabric.rs`
3. Rename `e2e_command.rs`, `e2e_full_system.rs`, `e2e_public_server.rs` → `integration_*.rs` (mxdx-launcher)
4. Rename `e2e_secret_request.rs` → `integration_secret_request.rs` (mxdx-secrets)

## `cargo test --workspace --tests` Status

Passes as of this audit. All compilation succeeds. Tests requiring Tuwunel or beta credentials are `#[ignore]`'d and only run in integration/E2E CI jobs.
