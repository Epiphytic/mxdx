# Plan: Rust / npm Binary Parity Convergence

**Slug:** rust-npm-binary-parity
**ADRs:** docs/adr/2026-04-29-rust-npm-binary-parity.md
**Research:** docs/research/2026-04-29-rust-npm-binary-parity-research.md
**Mode:** --parallel
**Autopilot:** true
**Accept-ADRs:** false
**Lean:** false
**Bullets:** false
**Skills:** true
**Teammate-model:**
**Branch:** brains/rust-npm-binary-parity

## Overview

This plan closes the Rust / npm binary divergence catalogued in the companion research document and enforced by ADR 2026-04-29. It proceeds in six phases ordered by the ADR's mandatory sequencing constraints: test infrastructure and classification honesty come first (Phase 1) so that the wire-format-parity gate (Phase 2) can be activated on a clean foundation; config schema canonicalization (Phase 3) and WASM session-loop migration (Phase 4) are gated on the gate being green; feature parity closure (Phase 5) fills the remaining interop-required gaps in both runtimes; and a final integration and release-readiness pass (Phase 6) validates the full 8-combination matrix, performance dashboards, and release artifacts before the plan is closed.

## Phases

### Phase 1: Test Infrastructure Unblock

- [ ] **T-1.1**: Create `mxdx-test-perf` helper crate scaffold
  - Depends on: none
  - Acceptance: `crates/mxdx-test-perf/Cargo.toml` exists, crate compiles with `cargo check -p mxdx-test-perf`, and exports a public `write_perf_entry` function with fields `suite`, `transport`, `runtime`, `duration_ms`, `rss_max`.
  - Note: Schema must be identical to npm `writePerfEntry()` (research §4.4). This is a dev/test-only crate; do not add it to the default workspace members that affect WASM builds.

- [ ] **T-1.2**: Migrate `crates/mxdx-worker/tests/e2e_profile.rs` perf emission to `mxdx-test-perf`
  - Depends on: T-1.1
  - Acceptance: `e2e_profile.rs` no longer calls `worker_log_path()` as its sole perf-data path; it calls `mxdx_test_perf::write_perf_entry(...)` and `cargo test -p mxdx-worker --test e2e_profile` produces at least one valid JSONL line in `$TEST_PERF_OUTPUT` parseable by `scripts/e2e-test-suite.sh`.

- [ ] **T-1.3**: Add CI lint step (warn-only) for E2E subprocess discipline
  - Depends on: none
  - Acceptance: `.github/workflows/ci.yml` contains a job named `e2e-subprocess-lint` that runs a static grep/ast check on `packages/e2e-tests/tests/**/*.test.js`; the job passes (warn-only — `continue-on-error: true`) and its output lists any `describe`/`test` block lacking a `spawn`/`execFile`/`spawnSync` invocation.
  - Note: Must be warn-only on merge for 5 business days (req 28); a follow-up task in Phase 2 flips it to blocking after that window.

- [ ] **T-1.4**: Reclassify `launcher-commands.test.js` — extract to integration tests
  - Depends on: T-1.3
  - Acceptance: `packages/integration-tests/tests/launcher-commands.test.js` exists containing the extracted WASM-direct test blocks; `packages/e2e-tests/tests/launcher-commands.test.js` either no longer exists or contains only subprocess-spawning blocks; `npm test --workspace=packages/integration-tests` passes.

- [ ] **T-1.5**: Reclassify `public-server.test.js` WASM block — extract to integration tests
  - Depends on: T-1.3
  - Acceptance: The `Public Server: WASM Client` describe block is moved to `packages/integration-tests/tests/public-server-wasm.test.js`; the remaining `packages/e2e-tests/tests/public-server.test.js` contains only subprocess-spawning blocks; both test files pass in their respective workspaces.

- [ ] **T-1.6**: Reclassify `p2p-signaling.test.js` and `launcher-onboarding.test.js` WASM block
  - Depends on: T-1.3
  - Acceptance: `packages/integration-tests/tests/p2p-signaling.test.js` and `packages/integration-tests/tests/launcher-onboarding-wasm.test.js` exist containing the non-subprocess blocks; the `WASM: Room Topology` block is absent from `packages/e2e-tests/tests/launcher-onboarding.test.js`; both integration test files pass.

- [ ] **T-1.7**: Refactor `rust-npm-interop-beta.test.js` to `describe.each` parameterization
  - Depends on: T-1.4, T-1.5, T-1.6
  - Acceptance: `packages/e2e-tests/tests/rust-npm-interop-beta.test.js` drives combinations via `describe.each` over `[{client_runtime, worker_runtime, hs_topology}]` rather than 8 hand-written `it` blocks; running `npx vitest run rust-npm-interop-beta` produces 8 test entries in output (including currently-skipped ones shown as skipped/todo); no N×M drift is possible from the parameterized structure.

- [ ] **T-1.8**: Audit Rust E2E tests for subprocess discipline and perf-helper migration (req 26, 31 — coverage gap from review)
  - Depends on: T-1.1, T-1.2
  - Acceptance: An audit document `docs/plans/2026-04-29-rust-e2e-audit.md` enumerates every test file under `crates/*/tests/e2e_*.rs` (and any `tests/integration_*.rs` claiming E2E coverage); each entry records (a) whether the test spawns a compiled binary subprocess, (b) whether it emits perf data via `mxdx-test-perf`. Tests that fail (a) are either migrated in this task or filed as new beads issues with the `brains:cleanup` label. `cargo test --workspace --tests` passes at the end.

### Phase 2: Wire-Format-Parity Gate

- [ ] **T-2.1**: Define and document gate flake/quarantine policy (req 12a)
  - Depends on: none
  - Acceptance: A new file `docs/adr/overrides/wire-format-parity-gate-policy.md` exists and defines: (a) max 2 automatic retries per combination before marking failed; (b) quarantine mechanism with ≤14 day window; (c) P2P combinations start non-blocking advisory; (d) promotion to blocking after 10 consecutive green runs.
  - Note: This document must exist before the gate is enabled as a required check (req 12a blocker).

- [ ] **T-2.2**: Define gate override policy document (req 8a)
  - Depends on: none
  - Acceptance: `docs/adr/overrides/README.md` exists and specifies the override procedure: written justification in PR body naming the failing combination, project-owner approval, linked tracking issue with deadline; the file also states that security fixes must be processable within 24 hours even when the parity gate is red.

- [ ] **T-2.3**: Fix `--p2p` clap-parse bug in t1a / t1b (req 9)
  - Depends on: none
  - Acceptance: `crates/mxdx-worker/src/main.rs` (or equivalent clap struct) defines a `--p2p` boolean flag on the `start` subcommand, OR the flag is removed from both t1a and t1b invocations in `rust-npm-interop-beta.test.js`; running `mxdx-worker start --p2p` exits with code 0 (not a clap error).

- [ ] **T-2.4**: Wire npm subprocess spawning — close `mxdx-5qp` (req 10)
  - Depends on: T-1.7
  - Acceptance: `rust-npm-interop-beta.test.js` resolves the `'npm launcher subprocess not yet wired'` skip reason; t2a, t3a, t4a, t4b each call `node mxdx-launcher.js` or `node mxdx-client.js` as a subprocess (verifiable by grep for `spawn.*mxdx-launcher` or `spawn.*mxdx-client` in the file); `mxdx-5qp` bead is closed.

- [ ] **T-2.5**: Implement t2a / t2b (npm client → Rust worker, same-HS and federated)
  - Depends on: T-2.3, T-2.4
  - Acceptance: Running `npx vitest run rust-npm-interop-beta --reporter=verbose` shows t2a and t2b as passing (not skipped); each spawns `node mxdx-client.js` and `mxdx-worker` as subprocesses against local Tuwunel instances.

- [ ] **T-2.6**: Implement t3a / t3b (Rust client → npm launcher, same-HS and federated)
  - Depends on: T-2.3, T-2.4
  - Acceptance: t3a and t3b pass in `npx vitest run rust-npm-interop-beta --reporter=verbose`; each spawns `mxdx-client` and `node mxdx-launcher.js` as subprocesses against local Tuwunel instances.

- [ ] **T-2.7**: Write P2P cryptographic verification security document and implement t4a / t4b advisory combinations
  - Depends on: T-2.5, T-2.6
  - Acceptance: `docs/reviews/security/2026-04-29-p2p-cross-runtime-dtls-verification.md` exists and covers (a) mutual DTLS fingerprint acceptance, (b) no silent fallback to unencrypted transport, (c) AES-GCM key derivation identity between Rust and npm code paths; t4a and t4b are implemented in the interop test file and run in non-blocking advisory mode (`// advisory: not yet blocking` comment present in the test); the security doc is linked from the test.

- [ ] **T-2.8**: Wire `wire-format-parity` CI job as required check; flip E2E lint to blocking
  - Depends on: T-2.1, T-2.2, T-2.5, T-2.6, T-1.3
  - Acceptance: `.github/workflows/ci.yml` contains a job `wire-format-parity` that runs all 8 combinations and is listed in the branch protection required checks (verified by `gh api repos/:owner/:repo/branches/main/protection` showing `wire-format-parity` in required status checks); the `e2e-subprocess-lint` job has `continue-on-error: false` (blocking); P2P combinations (t4a/t4b) have `continue-on-error: true` (advisory); the gate workflow file separates beta-credential combinations into the existing `e2e-beta` workflow vs local-Tuwunel combinations in core CI (req 11). PR description includes a `git log --format='%ci' <T-1.3-sha>` excerpt showing the T-1.3 merge is at least 5 business days old at PR-open time (req 28 / Finding 3).

- [ ] **T-2.9**: Update `scripts/e2e-test-suite.sh` and npm test harness to emit unified JSONL perf output (req 27, 30 — coverage gap from review)
  - Depends on: T-1.1, T-1.2
  - Acceptance: `scripts/e2e-test-suite.sh` consumes a single unified JSONL stream from both Rust and npm tests on the same `TEST_PERF_OUTPUT` path; the npm test harness invokes `writePerfEntry({runtime: "npm", ...})` on every E2E test invocation including P2P transports; `jq -e 'select(.runtime == "rust")' $TEST_PERF_OUTPUT | wc -l` and `jq -e 'select(.runtime == "npm")' $TEST_PERF_OUTPUT | wc -l` both return ≥1 after a full E2E run.

### Phase 3: Config Schema Canonicalization

- [ ] **T-3.1**: Add npm-only fields to `mxdx-types::WorkerConfig` and `mxdx-types::ClientConfig` (req 3)
  - Depends on: T-2.8
  - Acceptance: `crates/mxdx-types/src/config.rs` defines `telemetry`, `use_tmux`, `batch_ms`, `p2p_batch_ms`, `p2p_advertise_ips`, `p2p_turn_only`, `registration_token`, `admin_user` as `Option<T>` fields with `#[serde(default)]`; `cargo check -p mxdx-types` passes; existing Rust tests that deserialize `WorkerConfig` still pass.
  - Note: Phase 3 depends on Phase 2's gate being green (req 12 — see review Finding 9). Even though Pillar 1 is technically not Pillar 3/4/5, schema changes mid-gate-construction risk false passes.

- [ ] **T-3.2**: Implement legacy-section detection and auto-migration in both runtimes (req 6a)
  - Depends on: T-3.1
  - Acceptance: Running `mxdx-worker start` with a `worker.toml` containing a `[launcher]` section header logs a warning to stderr, writes `worker.toml.legacy.bak`, rewrites `worker.toml` as flat keys, and continues; same behavior verified for `mxdx-client` with `[client]` section; a test in `crates/mxdx-worker/tests/` asserts the migration produces a valid flat-key TOML and the `.bak` file contains the original content.
  - **(Blocker — added per review Finding 4)**: A test explicitly asserts that `authorized_users`, `allowed_commands`, and `trust_anchor` field values in the original `[launcher]`-wrapped config are byte-for-byte identical in the migrated flat-key output. Silent loss of any of these security-critical fields fails the test.

- [ ] **T-3.3**: Rewrite npm config parsers to drop section wrappers (req 1, 2)
  - Depends on: T-3.2
  - Acceptance: `packages/launcher/src/config.js` reads flat top-level keys from `worker.toml` (no `[launcher]` wrapper); `packages/client/src/config.js` reads flat top-level keys from `client.toml` (no `[client]` wrapper); `npm test --workspace=packages/launcher` passes; a config file written by `mxdx-worker` is successfully parsed by the npm launcher without error.

- [ ] **T-3.4**: Update npm config writers to not clobber unrelated fields (req 6)
  - Depends on: T-3.3
  - Acceptance: `packages/launcher/src/config.js` save path reads the existing TOML, merges only its own fields (never touching keys it does not own), and writes back; a test verifies that a Rust-written `authorized_users` field survives a round-trip through the npm config writer unchanged.

- [ ] **T-3.5**: Update onboarding wizard to emit canonical flat layout (req 4)
  - Depends on: T-3.3
  - Acceptance: Running the onboarding wizard (or its test fixture) and inspecting `$HOME/.mxdx/worker.toml` shows flat top-level keys with no `[launcher]` or `[client]` section; `smol-toml` parses the output without error; `cargo test -p mxdx-types` parses the same file via `WorkerConfig::from_toml` without error.

- [ ] **T-3.6**: Add unknown-key tolerance to both runtimes' config parsers (req 5)
  - Depends on: T-3.3, T-3.1
  - Acceptance: Adding an unrecognized field `future_field = "x"` to `worker.toml` does not cause either `mxdx-worker start` or the npm launcher to error; `#[serde(deny_unknown_fields)]` is absent from `WorkerConfig` and `ClientConfig`; `packages/launcher/src/config.js` uses a permissive parse path (e.g., `smol-toml` parse + allowlist pick) that discards unknown keys without throwing.

### Phase 4: WASM Expansion

- [ ] **T-4.1**: Write security review document for session-loop migration (req 13a — blocker)
  - Depends on: none
  - Acceptance: `docs/reviews/security/2026-04-29-wasm-session-loop-migration.md` exists and contains sections covering: (a) enumeration of every Matrix send call in the migrated code path with confirmation of MSC4362 encryption; (b) cryptographic primitive inventory (AES-GCM, key derivation, nonce generation) with behavioral identity confirmation; (c) confirmation that the new WASM public API does not expose crypto state to JS callers; document is marked as approved (or contains an explicit approval gate for project owner sign-off before T-4.3 merges).

- [ ] **T-4.2**: Pin `wasm-bindgen`, `wasm-pack`, and `rustc` versions in CI and `Cargo.toml` (ADR Assumed Versions)
  - Depends on: none
  - Acceptance: `Cargo.toml` workspace section pins `wasm-bindgen` to an exact version (not a range); `.github/workflows/ci.yml` installs `wasm-pack` at the exact matching version (not `@latest`); `dtolnay/rust-toolchain@1.93.1` is present in both `ci.yml` and `release.yml` build jobs; a comment adjacent to each pin cites the ADR and the reason.

- [ ] **T-4.3**: Migrate batching and zlib compression logic from `runtime.js` to `mxdx-core-wasm`
  - Depends on: T-4.1, T-4.2
  - Acceptance: `packages/launcher/src/runtime.js` no longer contains the `BatchedTerminalSender` class implementation; equivalent logic exists in `crates/mxdx-core-wasm/src/` and is exposed via `wasm-bindgen`; both WASM targets build successfully (`packages/core/wasm/nodejs/` and `packages/core/wasm/web/` both exist); `npm test --workspace=packages/launcher` passes; security review doc T-4.1 is linked in the PR.
  - Note: This is an integration-test-verifiable increment; full E2E verification of the partially-migrated launcher is deferred to T-4.5 (see review Finding 2).

- [ ] **T-4.4**: Migrate telemetry emission and P2P state machine from `runtime.js` to `mxdx-core-wasm`
  - Depends on: T-4.3
  - Acceptance: Telemetry emit logic and P2P state machine are absent from `packages/launcher/src/runtime.js`; equivalent logic is in `crates/mxdx-core-wasm/src/`; integration tests under `packages/integration-tests/` that exercise telemetry and P2P pass; both WASM targets build. Full E2E verification deferred to T-4.5.

- [ ] **T-4.5**: Migrate session lifecycle and command dispatch from `runtime.js` to `mxdx-core-wasm` — **E2E gate for the migration**
  - Depends on: T-4.4
  - Acceptance: `packages/launcher/src/runtime.js` line count is below 400 lines (from ~2000+), containing only OS-bound shims enumerated in the ADR; `wc -l packages/launcher/src/runtime.js` outputs a value ≤ 400; the remaining JS passes the `e2e-subprocess-lint` (all blocks spawn subprocesses or delegate to WASM); the **full E2E suite passes (all 8 wire-format-parity combinations green) — this is the migration's E2E gate (see review Finding 2)**; `wire-format-parity` CI job on the PR shows green.

- [ ] **T-4.6**: Add cross-reference doc comments to all JS thin wrappers (req 14, 15)
  - Depends on: T-4.5
  - Acceptance: Every file in `packages/launcher/src/` that wraps an OS-bound operation contains at least one comment matching the pattern `// Rust equivalent: <crate-path>::<file>::<function-or-struct>`; `grep -r "Rust equivalent:" packages/launcher/src/` returns at least one result per OS-bound wrapper file listed in the ADR Pillar 3 table; **(strengthened per review Finding 10)** each cited path resolves — `cargo doc --open -p <crate>` finds each named function/struct, and a CI step OR a PR-template checklist requires reviewer sign-off that each cross-reference is accurate (not just syntactically present).

- [ ] **T-4.7**: Update build pipeline to fail if either WASM target is missing (req 18)
  - Depends on: T-4.2
  - Acceptance: `.github/workflows/ci.yml` WASM build step exits non-zero if either `packages/core/wasm/nodejs/mxdx_core_wasm.js` or `packages/core/wasm/web/mxdx_core_wasm.js` is absent after the build; a local test confirms that deleting one target and running the build script produces a non-zero exit code.

### Phase 5: Feature Parity Closure

- [ ] **T-5.1**: Implement Rust worker CLI flags: `--telemetry`, `--log-format`, `--use-tmux` (req 19)
  - Depends on: none
  - Acceptance: `mxdx-worker start --help` lists `--telemetry <full|summary>`, `--log-format <json|text>`, and `--use-tmux <auto|always|never>`; the flags are defined in the clap struct in `crates/mxdx-worker/src/main.rs`; `cargo check -p mxdx-worker` passes.

- [ ] **T-5.2**: Implement Rust worker CLI flags: `--registration-token`, `--admin-user`, `--config`, `--batch-ms`, `--p2p-batch-ms`, `--p2p-advertise-ips`, `--p2p-turn-only`
  - Depends on: T-3.1
  - Acceptance: `mxdx-worker start --help` lists all seven flags; each flag is defined with a clap `env()` annotation where applicable; `cargo check -p mxdx-worker` passes; flags that map to `WorkerConfig` fields write through to the config struct.

- [ ] **T-5.3**: Implement Rust worker `reload` subcommand (req 19)
  - Depends on: T-5.2
  - Acceptance: `mxdx-worker reload --help` exists and the subcommand sends a reload signal to a running worker process (SIGHUP or equivalent); an integration test verifies the worker picks up a changed config value after reload without restarting the process. **(Per review Finding 6)** Before merge, the PR MUST link a security note in `docs/reviews/security/` clarifying which config fields reload atomically accepts, which require a restart, and whether trust-critical fields (`authorized_users`, `allowed_commands`, `trust_anchor`) trigger re-verification of connected sessions.

- [ ] **T-5.4**: Implement Rust client `verify`, `launchers`, `telemetry display` subcommands (req 19)
  - Depends on: none
  - Acceptance: `mxdx-client verify --help`, `mxdx-client launchers --help`, and `mxdx-client telemetry --help` all exist; `mxdx-client launchers` returns a list of discovered launcher spaces from the connected homeserver; `mxdx-client verify <user_id>` performs a cross-signing verification and prints the result; `cargo check -p mxdx-client` passes.

- [ ] **T-5.5**: Implement unified `attach`/`shell` addressing model in `mxdx-types` and Rust client `attach` (req 24)
  - Depends on: T-3.1
  - Acceptance: `crates/mxdx-types/src/lib.rs` exports a `SessionAddress` type (or equivalent) usable by both Rust and npm runtimes; `mxdx-client attach <uuid>` is implemented beyond the stub at `src/main.rs:518` and opens an interactive terminal session; an E2E test verifies Rust client can attach to a session started by the npm launcher.

- [ ] **T-5.6**: Implement npm client `ls`, `logs`, `cancel`, `--detach` and exec flags parity (req 19)
  - Depends on: none
  - Acceptance: `node mxdx-client.js ls --help`, `logs --help`, `cancel --help`, and `exec --help` all show the new flags; `--detach`, `--no-room-output`, `--timeout`, `--cwd`, `--worker-room`, `--skip-liveness-check` are defined in `packages/client/bin/mxdx-client.js`; `npm test --workspace=packages/client` passes.

- [ ] **T-5.7**: Implement npm client `trust` subcommands and `diagnose` (req 19)
  - Depends on: none
  - Acceptance: `node mxdx-client.js trust list --help`, `trust add --help`, `trust remove --help`, `trust pull --help`, `trust anchor --help`, and `diagnose --help` all exist; `npm test --workspace=packages/client` passes; the commands call through to WASM crypto functions, not stub prints.

- [ ] **T-5.8**: Implement npm client liveness-failure exit codes 10 / 11 / 12 (req 19)
  - Depends on: T-5.6
  - Acceptance: `packages/client/src/` exits with code 10 when no worker room is found, 11 when no live worker is in the room, and 12 when no worker supports the requested command; an integration test verifies each exit code by running `node mxdx-client.js exec` against a Tuwunel instance in the relevant failure state.

- [ ] **T-5.9**: Implement npm `MXDX_*` env var fallbacks for all CLI flags (req 22a)
  - Depends on: none
  - Acceptance: `packages/launcher/bin/mxdx-launcher.js` and `packages/client/bin/mxdx-client.js` both read `MXDX_HOMESERVER`, `MXDX_USERNAME`, `MXDX_PASSWORD`, `MXDX_ROOM_ID` as fallbacks when the corresponding CLI flag is absent; a unit test verifies the fallback by setting env vars and omitting CLI flags; the flag names match the Rust clap `env()` equivalents.

- [ ] **T-5.10**: Implement graceful SIGTERM / SIGINT shutdown with OLM session flush in both runtimes (req 25a)
  - Depends on: T-4.5
  - Acceptance: `packages/client/bin/mxdx-client.js` registers `SIGTERM` and `SIGINT` handlers that call `runtime.stop()` and flush pending Matrix key uploads before exiting; `crates/mxdx-client/src/main.rs` direct mode wires `tokio::signal::ctrl_c()` and `tokio::signal::unix::signal(SIGTERM)` to a graceful shutdown path; an integration test sends `SIGTERM` to each client binary mid-session and verifies the process exits with code 0 and emits a structured exit log event.

- [ ] **T-5.11a**: npm coordinator — Matrix connection and worker room discovery (req 23, split per review Finding 5)
  - Depends on: none
  - Acceptance: `packages/coordinator/bin/mxdx-coordinator.js` no longer prints the "not yet connected" stub; it logs in to a Matrix homeserver via WASM `WasmMatrixClient`, joins the configured coordinator room, and discovers available worker capability rooms (per the existing `--capability-room-prefix` flag). Integration test verifies the npm coordinator can authenticate and report a list of discovered worker rooms.

- [ ] **T-5.11b**: npm coordinator — task routing and exec dispatch (req 23, split per review Finding 5)
  - Depends on: T-5.11a
  - Acceptance: The npm coordinator accepts an exec request via the same Matrix event format the Rust coordinator uses (`crates/mxdx-coordinator/src/`), selects a live worker, forwards the task, and relays the response. Integration test verifies a fake exec request through the coordinator reaches a target worker (mocked or real).

- [ ] **T-5.11c**: npm coordinator — E2E test for npm coordinator → Rust worker dispatch (req 23, split per review Finding 5)
  - Depends on: T-5.11b, T-2.8
  - Acceptance: A new E2E test under `packages/e2e-tests/tests/coordinator-cross-runtime.test.js` spawns `node mxdx-coordinator.js` and `mxdx-worker` as subprocesses against local Tuwunel and verifies that an exec routed through the npm coordinator reaches the Rust worker and returns the expected output; `packages/coordinator/package.json` version is bumped.

- [ ] **T-5.12**: Canonicalize npm CLI flag naming to clap conventions (req 22)
  - Depends on: T-5.6, T-5.7, T-5.9
  - Acceptance: `packages/client/bin/mxdx-client.js` and `packages/launcher/bin/mxdx-launcher.js` use kebab-case for all multi-word flags; comma-separated flags (e.g., `--allowed-commands`) are replaced with repeatable flags (e.g., `--allowed-command` used multiple times); `--no-X` toggles replace boolean `--X false` patterns; `npm test --workspace=packages/client` and `npm test --workspace=packages/launcher` pass.

- [ ] **T-5.13**: Add CODEOWNERS / CI policy enforcing runtime-native deny-list (reqs 16, 17, 20 — coverage gap from review)
  - Depends on: none
  - Acceptance: `.github/CODEOWNERS` (or an equivalent CI lint script in `.github/workflows/parity-policy.yml`) flags PRs that (a) add OS-unbound logic to JS files in `packages/launcher/src/` (req 16); (b) add new JS thin-wrapper files outside the ADR-enumerated list without an ADR amendment (req 17); (c) add `daemon`/`mcp` subcommands to npm or `inquirer`-style onboarding to Rust (req 20). The policy MUST be implemented as a CI check that produces a clear failure message naming the requirement and the offending file. A test with a deliberately non-conforming change verifies the check fires.

### Phase 6: Integration, Performance, and Release Readiness

- [ ] **T-6.1**: Run full E2E suite across all 8 combinations and verify all pass (per review Finding 7 — extended dependencies)
  - Depends on: T-2.8, T-3.5, T-3.6, T-4.5, T-5.1, T-5.2, T-5.3, T-5.4, T-5.5, T-5.6, T-5.7, T-5.8, T-5.9, T-5.10, T-5.11c, T-5.12, T-5.13
  - Acceptance: `scripts/e2e-test-suite.sh` exits with code 0; all 8 combinations in `rust-npm-interop-beta.test.js` show as passing in the Vitest output (t4a/t4b advisory pass or are in documented quarantine with a linked issue); no test is skipped without an explicit `// advisory` or `// quarantine:` annotation.

- [ ] **T-6.2**: Validate unified JSONL performance output across both runtimes
  - Depends on: T-1.2, T-5.1
  - Acceptance: Running `scripts/e2e-test-suite.sh` with `TEST_PERF_OUTPUT` set produces a valid JSONL file containing entries from both Rust (`runtime: "rust"`) and npm (`runtime: "npm"`) paths, covering same-HS, federated, and P2P transports for each runtime; `jq -e '.suite and .transport and .runtime and .duration_ms' $TEST_PERF_OUTPUT` exits 0 on every line.

- [ ] **T-6.3**: Validate performance parity — Rust ≤ npm latency within defined tolerance
  - Depends on: T-6.2
  - Acceptance: A script or test asserts that Rust worker round-trip latency (same-HS transport) is within 200% of npm launcher latency for the same transport (i.e., Rust is no more than 2× slower); the result is written to `test-results/perf-parity-report.json`; the CI `wire-format-parity` job includes this assertion.

- [ ] **T-6.4**: Audit and verify version pinning across the full dependency set
  - Depends on: T-4.2
  - Acceptance: `Cargo.toml` has exact-version pins for `wasm-bindgen`; `package.json` files have exact-version pins for `vitest` and `node-datachannel`; `ci.yml` pins `wasm-pack` to an exact version; a reviewer checklist in `docs/adr/2026-04-29-rust-npm-binary-parity.md` Assumed Versions section is annotated with the pinned versions actually in use.

- [ ] **T-6.5**: Update README "last compatible version" table and CHANGELOG (ADR 2026-04-16 ref)
  - Depends on: T-6.1
  - Acceptance: `README.md` contains a "Last compatible versions" table with a row for the current release showing Rust binary version, npm package version, WASM build version, and `matrix-sdk` version; a `CHANGELOG.md` entry under `[Unreleased]` documents the wire-format changes introduced by this plan.

- [ ] **T-6.6**: Promote P2P gate combinations from advisory to blocking — tracking task (per review Finding 8)
  - Depends on: T-6.1
  - Acceptance: This is a tracking gate, NOT an implementation task. The teammate's only deliverable is the CI promotion commit (flipping `continue-on-error: true` → `false` for t4a/t4b in `.github/workflows/ci.yml`) AFTER `gh run list --workflow=ci.yml --status=success` shows 10 consecutive runs where t4a and t4b passed. If 10 consecutive greens have not accumulated by plan-close time, this task closes with `--reason="deferred — see follow-up bead"` and a new bead is created with `brains:cleanup` label tracking the residual work.
  - Note: The 10-consecutive-greens window is a CI-time prerequisite, not work the teammate performs (per review Finding 8).

- [ ] **T-6.7**: Verify wire-format-parity gate test matrix covers every interop-required feature (req 21 — coverage gap from review)
  - Depends on: T-6.1
  - Acceptance: A coverage matrix document `docs/plans/2026-04-29-interop-required-coverage.md` enumerates every interop-required feature from ADR Pillar 4 (verify, launchers, telemetry display, reload, --telemetry, --log-format, --use-tmux, --registration-token, --admin-user, --config, --batch-ms, --p2p-batch-ms, --p2p-advertise-ips, --p2p-turn-only, --format, attach/shell, ls, logs, cancel, --detach, exec flags, trust, diagnose, exit codes 10/11/12, MXDX_* env vars, full coordinator) and maps each to at least one test in the wire-format-parity matrix that exercises it. UNCOVERED entries become new beads issues with `brains:cleanup` label.

## Risks and Mitigations

The largest delivery risk is Phase 2's gate activation blocking all Pillar 3/4/5 work: if t2a-t4b combinations surface deep protocol incompatibilities between `datachannel` 0.16 and `node-datachannel` 0.32, the gate cannot go green and all downstream phases stall. Mitigation: T-2.7 begins P2P combinations in non-blocking advisory mode so t4a/t4b failures do not block gate activation; t2a-t3b (non-P2P cross-runtime) can go green independently and unblock the gate for Phases 3-5. The second major risk is the WASM session-loop migration (Phase 4): moving 2000+ lines of JS into Rust/WASM is the highest-complexity task in the plan and carries WASM build fragility and matrix-sdk 0.16 / rustc 1.93.1 pin constraints; mitigation is the mandatory security review gate (T-4.1) and the incremental three-task migration split (T-4.3, T-4.4, T-4.5) so each increment is independently verifiable. Config migration (Phase 3, req 6a) carries a user-data safety risk since `authorized_users` and `allowed_commands` are security-critical fields; T-3.2 addresses this by requiring a `.legacy.bak` preservation and an explicit test asserting security fields survive migration intact.
