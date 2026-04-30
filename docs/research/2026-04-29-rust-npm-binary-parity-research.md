# Rust / npm Binary Parity Research

**Date:** 2026-04-29
**Author:** research agent (read-only)
**Status:** draft — for architecture review

---

## 1. Binary Inventory

### 1.1 Rust compiled artifacts

| Binary | Crate | Entry point | Status |
|--------|-------|-------------|--------|
| `mxdx-worker` | `crates/mxdx-worker` | `src/main.rs` | Active — primary worker |
| `mxdx-exec` | `crates/mxdx-worker` | `src/bin/mxdx_exec.rs` | Active — internal process helper |
| `mxdx-client` | `crates/mxdx-client` | `src/main.rs` | Active — primary client CLI |
| `mxdx-coordinator` | `crates/mxdx-coordinator` | `src/main.rs` | Active — routing coordinator |
| `mxdx-launcher` | `crates/mxdx-launcher` | `src/main.rs` | **DEPRECATED** (Cargo.toml comment: "use mxdx-worker instead, removed in v2.0"); native feature stub only |
| `fabric` | `crates/mxdx-fabric-cli` | `src/main.rs` | **DEPRECATED** (Cargo.toml: mxdx-fabric deprecated, removed in v2.0) |

WASM artifact (not a standalone binary but a compiled output):

| Output | Source crate | Targets |
|--------|-------------|---------|
| `mxdx_core_wasm.js` + `.wasm` | `crates/mxdx-core-wasm` | `nodejs` → `packages/core/wasm/nodejs/`; `web` → `packages/core/wasm/web/` |

### 1.2 npm bin entries

| Binary name | Package | Entry point |
|-------------|---------|-------------|
| `mxdx-launcher` | `@mxdx/launcher` (v1.1.0) | `packages/launcher/bin/mxdx-launcher.js` |
| `mxdx-client` | `@mxdx/client` (v1.1.0) | `packages/client/bin/mxdx-client.js` |
| `mxdx` / `mx` | `@mxdx/cli` (v1.1.0) | `packages/mxdx/bin/mxdx.js` (dispatcher) |
| `mxdx-coordinator` | `@mxdx/coordinator` (v1.1.0) | `packages/coordinator/bin/mxdx-coordinator.js` |

---

## 2. CLI Surface Comparison

### 2.1 Worker / Launcher pair

**Rust: `mxdx-worker start`** (`crates/mxdx-worker/src/main.rs:14-67`)

| Flag | Env var | Notes |
|------|---------|-------|
| `--trust-anchor` | — | |
| `--history-retention` | — | |
| `--cross-signing-mode` | — | |
| `--room-name` | — | |
| `--room-id` | `MXDX_ROOM_ID` | |
| `--homeserver` | `MXDX_HOMESERVER` | |
| `--username` | `MXDX_USERNAME` | |
| `--password` | `MXDX_PASSWORD` | |
| `--force-new-device` | — | |
| `--max-sessions` | — | |
| `--allowed-command` (repeatable) | — | |
| `--allowed-cwd` (repeatable) | — | |
| `--authorized-user` (repeatable) | — | |
| `--p2p` | — | **Present in interop test calls** (`rust-npm-interop-beta.test.js:46`) but **not defined in the clap struct** — the binary will reject this flag at runtime |

**Rust: `mxdx-worker diagnose`**

| Flag | Env var |
|------|---------|
| `--profile` | — |
| `--homeserver` | `MXDX_HOMESERVER` |
| `--username` | `MXDX_USERNAME` |
| `--password` | `MXDX_PASSWORD` |
| `--pretty` | — |
| `--decrypt` | — |

**npm: `mxdx-launcher start` (default command)** (`packages/launcher/bin/mxdx-launcher.js`)

| Flag | Notes |
|------|-------|
| `--username` | |
| `--servers` | **Plural, comma-separated** vs Rust's singular `--homeserver` |
| `--registration-token` | No Rust equivalent |
| `--admin-user` | No Rust equivalent |
| `--allowed-commands` | Comma-separated string vs Rust's repeatable flag |
| `--allowed-cwd` | Comma-separated string |
| `--config` | Explicit config file path; no Rust equivalent |
| `--telemetry` | `full|summary`; no Rust equivalent (Rust always emits full) |
| `--max-sessions` | |
| `--password` | |
| `--log-format` | `json|text`; Rust hard-codes stderr with `tracing_subscriber::fmt` |
| `--use-tmux` | `auto|always|never`; no Rust equivalent |
| `--batch-ms` | No Rust equivalent |
| `--p2p-enabled` | Rust uses `p2p.enabled` in `worker.toml`; no CLI flag on worker |
| `--p2p-batch-ms` | No Rust equivalent |
| `--p2p-idle-timeout-s` | Partial Rust equivalent: `worker.toml [p2p] idle_timeout_seconds` |
| `--p2p-advertise-ips` | No Rust equivalent |
| `--p2p-turn-only` | No Rust equivalent |

**npm: `mxdx-launcher reload`** — no Rust equivalent.

**Divergences (worker/launcher pair):**

1. Multi-homeserver: npm uses `--servers <url,...>` (comma-separated); Rust uses `--homeserver` (single, falls back to `defaults.toml` account list). Semantically equivalent via config; not flag-compatible.
2. P2P tuning: npm exposes `--p2p-batch-ms`, `--p2p-advertise-ips`, `--p2p-turn-only` at CLI level; Rust only exposes `p2p.enabled` and `p2p.idle_timeout_seconds` in `worker.toml` (no CLI flags at all for P2P on the worker).
3. `--telemetry full|summary`: npm only; Rust always emits full telemetry.
4. `--log-format`: npm only; Rust tracing format is hardcoded.
5. `--use-tmux`: npm only; Rust always uses tmux when available.
6. `--registration-token`, `--admin-user`, `--config`: npm only.
7. `--p2p` (undocumented): present in `rust-npm-interop-beta.test.js` test call but **not defined in the Rust clap struct**. This flag will cause `mxdx-worker start` to exit with a clap parse error.

### 2.2 Client pair

**Rust: `mxdx-client`** (`crates/mxdx-client/src/cli/mod.rs`)

Global flags: `--homeserver`, `--username`, `--password`, `--room-id`, `--force-new-device`, `--profile`, `--no-daemon`, `--no-p2p`

| Subcommand | Key flags |
|-----------|-----------|
| `run` / `exec` | `command`, `args...`, `--detach`, `--interactive`, `--no-room-output`, `--timeout`, `--cwd`, `--worker-room`, `--skip-liveness-check` |
| `attach` | `uuid`, `--interactive` |
| `ls` | `--all`, `--worker-room` |
| `logs` | `uuid`, `--follow`, `--worker-room` |
| `cancel` | `uuid`, `--signal`, `--worker-room` |
| `trust` | `list|add|remove|pull|anchor` |
| `daemon` | `start|stop|status|mcp` |
| `cleanup` | `targets`, `--force`, `--delete-all-sessions` |
| `diagnose` | `--pretty`, `--decrypt` |

**npm: `mxdx-client`** (`packages/client/bin/mxdx-client.js`)

Global flags: `--server`, `--username`, `--password`, `--registration-token`, `--format`, `--config`, `--batch-ms`, `--p2p-enabled`, `--p2p-batch-ms`, `--p2p-idle-timeout-s`

| Subcommand | Key flags |
|-----------|-----------|
| `exec <launcher> [cmd...]` | `--cwd` |
| `shell <launcher> [command]` | `--cols`, `--rows` |
| `verify <user_id>` | none |
| `launchers` | none |
| `telemetry <launcher>` | none |
| `cleanup <targets>` | `--force-cleanup`, `--older-than`, `--delete-all-sessions` |

**Divergences (client pair):**

1. `shell` / interactive terminal: npm has `mxdx-client shell <launcher>`; Rust has `mxdx-client attach <uuid>` — different model (npm connects to launcher by name and starts a session; Rust attaches to an existing session by UUID). The Rust `attach` body is still a stub (`eprintln!("Interactive terminal attach not yet fully implemented.")`; `src/main.rs:518`).
2. `launchers` command: npm-only. Lists discovered launcher spaces. Rust has no equivalent listing command.
3. `verify` command: npm-only. Rust has `trust anchor/add/remove` but no `verify <user_id>` subcommand.
4. `telemetry` command: npm-only. Rust reads telemetry via liveness checks internally but exposes no CLI display command.
5. `daemon` / `mcp` subcommands: Rust-only. No npm daemon or MCP server mode.
6. `trust` subcommands: Rust-only. No npm equivalent.
7. `diagnose` subcommand: Rust-only.
8. `--no-p2p` flag: Rust global flag disabling P2P per-invocation. npm uses `--p2p-enabled false`.
9. `--no-daemon` / `--profile`: Rust-only (daemon architecture).
10. `--format text|json`: npm-only.
11. `--registration-token`: npm-only.
12. `--batch-ms`: npm-only global flag.
13. Worker targeting: Rust uses `--worker-room <room-name>` or `--room-id <room-id>`; npm uses `exec <launcher-name>` positional argument (launcher-by-name discovery model).

### 2.3 Coordinator pair

**Rust: `mxdx-coordinator start`**: `--room`, `--capability-room-prefix`, `--default-on-timeout`, `--default-on-heartbeat-miss`.

**npm: `mxdx-coordinator`** (`packages/coordinator/bin/mxdx-coordinator.js`): `--room`, `--capability-room-prefix`, plus a stub body that prints "Coordinator not yet connected to Matrix — use native binary for full functionality". The npm coordinator is a non-functional shell.

---

## 3. Feature Parity Matrix

| Feature | Rust `mxdx-worker` | npm `@mxdx/launcher` | Notes |
|---------|-------------------|---------------------|-------|
| Matrix login / session restore | Yes | Yes (via WASM) | Both use `mxdx-core-wasm` crypto; npm uses IndexedDB |
| E2EE messaging (Megolm) | Yes | Yes (via WASM) | |
| MSC4362 encrypted state events | Yes | Yes (via WASM) | |
| Command execution (non-interactive) | Yes | Yes | |
| Interactive terminal (PTY) | Yes | Yes | npm uses `PtyBridge`; Rust uses `tmux` + `mxdx-exec` |
| Session batching / zlib compression | Yes | Yes | Both implement batch windows and zlib compression |
| Telemetry state events | Yes | Yes | npm has `full|summary` levels; Rust always full |
| Multi-homeserver failover | Yes | Yes | Rust: `defaults.toml` accounts; npm: `--servers` list |
| P2P transport (WebRTC / AES-GCM) | Yes (via mxdx-p2p) | Yes (via node-datachannel) | Same AES-GCM protocol; different WebRTC backing |
| Onboarding / first-run registration | No | Yes | npm runs interactive wizard via `inquirer` |
| Reload without restart | No | Yes (`mxdx-launcher reload`) | |
| `--telemetry summary` mode | No | Yes | |
| Log format selection | No | Yes (`--log-format json|text`) | |
| tmux mode selection | No | Yes (`--use-tmux`) | |

| Feature | Rust `mxdx-client` | npm `@mxdx/client` | Notes |
|---------|-------------------|---------------------|-------|
| Exec command on worker | Yes | Yes | Different addressing model |
| Interactive shell attach | Stub only | Yes (full) | Rust attach is unimplemented |
| List sessions (`ls`) | Yes | No | |
| View session logs | Yes | No | |
| Cancel session | Yes | No | |
| Session follow (`--follow`) | Yes | No | |
| Detached mode (`--detach`) | Yes | No | |
| Worker liveness check | Yes | No (implicit discovery) | |
| Daemon / Unix socket IPC | Yes | No | |
| MCP server mode | Yes | No | |
| Trust management | Yes (stub) | No | |
| Diagnose runtime | Yes | No | |
| Cross-sign verify user | No | Yes | |
| List launcher spaces | No | Yes | |
| Display telemetry | No | Yes | |
| Multi-homeserver | Yes | Yes | |
| P2P enable/disable per-invocation | Yes (`--no-p2p`) | Partial (`--p2p-enabled false`) | |

| Feature | Rust `mxdx-coordinator` | npm `@mxdx/coordinator` |
|---------|------------------------|------------------------|
| Route tasks to workers | Yes | No (stub) |
| Monitor sessions | Yes | No (stub) |

---

## 4. Test Coverage Parity

### 4.1 Test file inventory

| File | Runtime under test | Classification per CLAUDE.md |
|------|--------------------|------------------------------|
| `packages/e2e-tests/tests/launcher-onboarding.test.js` | npm subprocess (`node mxdx-launcher.js`) + WASM lib | Mixed: one subprocess test, rest are library-level |
| `packages/e2e-tests/tests/launcher-commands.test.js` | WASM `WasmMatrixClient` directly | **Integration test** (library-level, not binary subprocess) |
| `packages/e2e-tests/tests/public-server.test.js` | npm subprocess (`node mxdx-launcher.js`, `node mxdx-client.js`) + WASM lib | Mixed; subprocess round-trip is true E2E; WASM-only tests are integration |
| `packages/e2e-tests/tests/rust-npm-interop-beta.test.js` | Rust subprocesses (`mxdx-worker`, `mxdx-client`) | True E2E for t1a/t1b; t2a/t2b/t3a/t3b/t4a/t4b are **skipped placeholders** |
| `packages/e2e-tests/tests/p2p-signaling.test.js` | JS `P2PSignaling` class directly | **Unit/integration test** (no binary subprocess) |
| `packages/launcher/tests/runtime-unit.test.js` | JS runtime logic directly | **Unit test** (no binary subprocess) |
| `crates/mxdx-worker/tests/e2e_profile.rs` | `mxdx-worker` and `mxdx-client` subprocesses | True E2E (spawns compiled binaries) |

### 4.2 Tests that violate the CLAUDE.md E2E definition

Per CLAUDE.md: "End-to-end tests MUST exercise the compiled binaries (mxdx-worker, mxdx-client) as subprocesses, or they are NOT end-to-end tests."

The following are **misclassified**:

1. `launcher-commands.test.js` — describes itself as "E2E tests for launcher command execution" but uses `WasmMatrixClient` directly; no binary subprocess is spawned. This is an integration test.

2. `public-server.test.js` — the `Public Server: WASM Client` describe block uses `WasmMatrixClient` API directly. These are integration tests masquerading as E2E. The `Public Server: Launcher + Client Round-Trip` block does spawn `node mxdx-launcher.js` and `node mxdx-client.js` as subprocesses — these are genuine E2E for the npm path.

3. `p2p-signaling.test.js` — pure unit tests on the JS `P2PSignaling` class. Not labeled E2E but referenced in the test structure.

4. `launcher-onboarding.test.js` — the `E2E: Launcher Onboarding` block spawns the npm launcher binary (`node mxdx-launcher.js`) correctly. The `WASM: Room Topology` block uses `WasmMatrixClient` directly — integration tests.

### 4.3 Coverage gaps

**Rust binaries completely uncovered by npm E2E suite:**
- `mxdx-worker` is never spawned as a subprocess by any npm test that runs in CI without `TEST_CREDENTIALS_TOML`. The `rust-npm-interop-beta.test.js` t1a/t1b tests are the only correct Rust-subprocess E2E tests in the npm suite, and they require credentials to run.
- `mxdx-client` is never spawned as a subprocess by any npm test in CI.

**npm binaries completely uncovered by Rust E2E suite:**
- `crates/mxdx-worker/tests/e2e_profile.rs` never spawns `node mxdx-launcher.js` or `node mxdx-client.js`. It tests Rust↔Rust only.

**Cross-runtime combinations (6 of 8 are placeholders):**

From `rust-npm-interop-beta.test.js`:
- t1a (Rust client → Rust worker, same-HS): implemented
- t1b (Rust client → Rust worker, federated): implemented
- t2a (npm client → Rust worker, same-HS): **placeholder skip**
- t2b (npm client → Rust worker, federated): **placeholder skip**
- t3a (Rust client → npm launcher, same-HS): **placeholder skip**
- t3b (Rust client → npm launcher, federated): **placeholder skip**
- t4a (npm client → npm launcher, same-HS): **placeholder skip**
- t4b (npm client → npm launcher, federated): **placeholder skip**

Skip reason for all six: `'npm launcher subprocess not yet wired'` / `'npm client subprocess not yet wired'`. This is issue `mxdx-5qp`: "Wire npm client/launcher subprocess spawning in rust-npm-interop-beta.test.js".

### 4.4 Performance tests

Performance data is collected via the `TEST_PERF_OUTPUT` environment variable:
- `scripts/e2e-test-suite.sh` sets `TEST_PERF_OUTPUT` and wraps entries per suite.
- `packages/e2e-tests/tests/public-server.test.js` calls `writePerfEntry()` in one test: `'launcher-client-round-trip'` on the `npm-public` transport path.
- `crates/mxdx-worker/tests/e2e_profile.rs` has a `TEST_PERF_OUTPUT` path (`worker_log_path()` env var) used to log timing to a file.

**Perf coverage gap:** Performance tests cover the npm launcher + npm client path (public server test). There is no equivalent performance benchmark for the Rust worker + Rust client path that writes to the structured `TEST_PERF_OUTPUT` format. The `e2e_profile.rs` test writes to a log file, not to the shared `TEST_PERF_OUTPUT` JSONL format, so Rust perf results do not appear in the unified `test-results/rust-e2e-perf.json` output unless the test manually emits the right format.

The e2e-test-suite.sh does call `cargo test -p mxdx-worker --test e2e_profile` and wraps its output, but the Rust test produces raw log output not the JSONL entries that `writePerfEntry()` produces. The two perf output mechanisms are not unified.

---

## 5. Build and Release Coupling

### 5.1 Current state (from ADR 2026-04-16 and `.github/workflows/release.yml`)

The release workflow (`release.yml`) performs:
1. Runs `e2e-gate` job (builds Rust release binaries, builds WASM, runs `npm install`, runs `e2e-test-suite.sh`).
2. Uploads `mxdx-worker` and `mxdx-client` as artifacts.
3. `release` job: downloads artifacts, runs `semantic-release` via npm.

Both Rust crates and npm packages are published from the same workflow run. Version is locked at `workspace.package.version = "1.1.0"` in `Cargo.toml` and `"version": "1.1.0"` in all `package.json` files — currently manually synchronized.

**Gaps identified:**

1. **Version synchronization is manual.** There is no automated check that `Cargo.toml workspace.package.version` matches `package.json` versions. `semantic-release` bumps npm package versions; Rust workspace version must be manually updated to match.

2. **Only `mxdx-worker` and `mxdx-client` are uploaded as release assets.** `mxdx-coordinator` is not included in the release artifacts or upload step. If the coordinator is needed standalone, there is no binary asset for it.

3. **No cross-platform release binaries.** The CI only builds for `x86_64-unknown-linux-gnu`. macOS (`build-only-macos`) is manual/dispatch-only and produces no release artifact. There are no Windows binaries.

4. **The `wire-format-parity` CI gate is not yet implemented.** ADR 2026-04-16 specifies this as the enforcement mechanism for coordinated releases: "CI adds a `wire-format-parity` check that runs a cross-language round-trip test." No such job exists in `ci.yml` or `release.yml`. The `cross-vectors` job referenced is not present in either workflow file as a visible job.

5. **CHANGELOG.md**: ADR requires "Release notes call out wire-format changes explicitly in both CHANGELOG.md (Rust) and the npm package changelogs." There is no CHANGELOG.md in the repo root (not found in the directory listing). npm changelogs are managed by `semantic-release`, which generates them from commit messages; Rust CHANGELOG.md is not generated or enforced.

6. **"Last compatible version" table**: ADR requires maintenance in README.md; the policy itself says coordinated releases keep this trivial, but the table's existence was not verified.

---

## 6. Cross-Cutting Concerns

### 6.1 Configuration files

**Rust config schema** (from `crates/mxdx-types/src/config.rs`):

- `$HOME/.mxdx/defaults.toml`: `[[accounts]]` with `user_id`, `homeserver`, `password`; `[trust]`; `[webrtc]`
- `$HOME/.mxdx/worker.toml`: `room_name`, `trust_anchor`, `history_retention`, `max_sessions`, `allowed_commands`, `allowed_cwd`, `authorized_users`, `[capabilities]`, `[p2p]`
- `$HOME/.mxdx/client.toml`: `default_worker_room`, `coordinator_room`, `[session]`, `[daemon]`, `[p2p]`

**npm config schema** (`packages/launcher/src/config.js`, `packages/client/src/config.js`):

- `$HOME/.mxdx/worker.toml` (`[launcher]` section): `username`, `servers`, `allowed_commands`, `allowed_cwd`, `telemetry`, `max_sessions`, `admin_users`, `use_tmux`, `batch_ms`, `p2p_enabled`, `p2p_batch_ms`, `p2p_idle_timeout_s`, `p2p_advertise_ips`, `p2p_turn_only`, `telemetry_interval_s`
- `$HOME/.mxdx/client.toml` (`[client]` section): `username`, `servers`, `batch_ms`, `p2p_enabled`, `p2p_batch_ms`, `p2p_idle_timeout_s`

**Compatibility issues:**

Both Rust and npm write to `$HOME/.mxdx/worker.toml` but in different TOML sections: Rust writes a flat `WorkerConfig` (no section header); npm writes under `[launcher]`. These files are **not interchangeable** — if the Rust worker reads a file written by npm launcher, it will find all fields missing (wrong section). If npm launcher reads a file written by Rust worker, same issue.

Similarly for client config: Rust reads from `client.toml` with a flat schema; npm reads from `[client]` section. These are separate field-level incompatibilities, not just section issues. For example:
- npm client stores `servers` (array); Rust client reads account list from `defaults.toml`.
- npm stores `batch_ms`; Rust stores `[p2p] idle_timeout_seconds` (no `batch_ms` concept in Rust config).

### 6.2 Logging and structured output

**Rust**: `tracing_subscriber::fmt().with_writer(std::io::stderr)` — unstructured text to stderr. Not JSON. No `log-format` selection at the CLI level. (`crates/mxdx-worker/src/main.rs:102`, `crates/mxdx-client/src/main.rs:33`)

**npm launcher**: configurable via `--log-format json|text`. When `json`: emits `{ level, msg, ts, ...data }` to stdout (info/debug) or stderr (error). (`packages/launcher/src/runtime.js:32-57`)

These are **not compatible** log streams. Log aggregation systems treating them as the same source would need separate parsers.

### 6.3 Exit codes

**Rust client** defines specific exit codes:
- `10`: no worker room found
- `11`: no live worker in room (stale or offline)
- `12`: no worker supports the requested command
- `exit_code.unwrap_or(1)`: propagates remote command exit code or defaults to 1

**npm client**: `process.exit(result.exitCode)` — propagates remote command exit code. No defined codes for liveness failures (fails with connection errors or hangs).

The liveness-failure exit codes (10/11/12) are Rust-only. Scripts that need to distinguish "worker offline" from "command failed" can only do so with the Rust binary.

### 6.4 Environment variables

| Env var | Rust | npm |
|---------|------|-----|
| `MXDX_HOMESERVER` | Yes (`--homeserver` fallback) | No |
| `MXDX_USERNAME` | Yes | No |
| `MXDX_PASSWORD` | Yes | No |
| `MXDX_ROOM_ID` | Yes (client) | No |
| `MXDX_BIN_DIR` | Yes (test helper) | No |
| `TEST_PERF_OUTPUT` | Partial (log file; not JSONL format) | Yes (JSONL format) |
| `E2E_PRESET` | Yes (`e2e_profile.rs`) | No |
| `SKIP_NPM_NPM`, `SKIP_RUST_NPM`, etc. | No | Yes (interop test) |

The `MXDX_HOMESERVER` / `MXDX_USERNAME` / `MXDX_PASSWORD` env vars exist only on the Rust side. npm relies solely on config file or CLI flags.

### 6.5 Signal handling and graceful shutdown

**npm launcher** (`packages/launcher/bin/mxdx-launcher.js:83-97`):
```
process.on('SIGINT', async () => { await shutdown(); process.exit(0); });
process.on('SIGTERM', async () => { await shutdown(); process.exit(0); });
```
Shutdown calls `runtime.stop()` then `saveIndexedDB()`.

**Rust worker**: the e2e_profile.rs test sends `SIGTERM` to the worker subprocess to trigger shutdown. The worker handles this through the tokio signal machinery. No explicit `SIGINT` mention in main.rs — needs confirmation that `tokio::signal::ctrl_c()` is wired. Both `SIGTERM` and `SIGINT` should be handled.

**npm client**: no signal handler. If killed mid-session, no cleanup of the Matrix session state.

**Rust client (daemon mode)**: the daemon itself should handle signals. Direct mode (`--no-daemon`) has no signal handler in `main.rs`.

---

## 7. Idiomatic Patterns and Prior Art

Projects shipping both a Rust binary and an npm package wrapping it typically fall into one of two models:

**Model A — npm as thin installer/shim (esbuild, swc, biome, rspack):** The npm package downloads a pre-built Rust binary and delegates to it. The CLI surface is defined once in Rust; npm provides zero-overhead dispatch. mxdx's architecture goal ("npm launcher and Rust worker converge to one unified worker; implement in Rust, expose via WASM") points toward this model, but is not yet realized. The npm launcher currently implements its own full runtime.

**Model B — npm as parallel first-class implementation (Prettier + Prettier Rust):** Both runtimes implement the full surface independently. Wire-format compatibility is the integration point, not CLI flag compatibility. This is effectively what mxdx has today.

The hybrid that makes most sense for mxdx's WASM-first strategy (memory note: "implement in Rust, expose via WASM") is a variant of Model A where the npm runtime calls WASM for all crypto/Matrix operations but retains Node.js for PTY bridging, IndexedDB, and OS-level integration points that WASM cannot address in Node.js context. This is already the architecture of `@mxdx/launcher` — it calls into `mxdx-core-wasm` for all Matrix and crypto operations. The gap is that the WASM module does not expose the full worker session-execution loop; `packages/launcher/src/runtime.js` implements that loop in JS (2000+ lines).

---

## 8. Stable Versions of Key Dependencies

| Dependency | Rust version in use | npm version in use | Notes |
|-----------|--------------------|--------------------|-------|
| `matrix-sdk` | 0.16 | — (via WASM) | Pinned to rustc 1.93.1 due to 1.94.0 trait solver regression |
| `clap` | 4 (derive + env) | — | |
| `commander` | — | ^14.0.3 | |
| `serde` / `serde_json` | 1 | — | |
| `tokio` | 1 (full features) | — | |
| `aes-gcm` | 0.10 | Web Crypto API | WASM: browser crypto.subtle; Node: same |
| `ed25519-dalek` | 2 | — | Used for P2P handshake signing |
| `datachannel` / `node-datachannel` | 0.16 (vendored) | ^0.32.2 | Different major versions; both wrap libdatachannel |
| `zod` | — | ^4.3.6 (recently bumped from 3.x) | |
| `inquirer` | — | ^13.4.0 | Launcher onboarding only |
| `smol-toml` | — | ^1.6.1 | npm TOML parser |
| `toml` (Rust) | 0.8 | — | |

The `datachannel` (Rust, 0.16) vs `node-datachannel` (npm, 0.32.2) version gap is worth noting — both wrap the same underlying C++ libdatachannel library but at very different API surface versions. Protocol-level compatibility should be verified if ICE/DTLS behavior changed between these versions.

---

## 9. Constraints Discovered

1. **WASM cannot replace the Node.js PTY bridge.** `packages/launcher/src/pty-bridge.js` uses `node-pty`, which requires native addons. This is not portable to WASM. Any "unified Rust worker via WASM" must retain a thin Node.js shim for PTY.

2. **WASM IndexedDB dependency.** The npm/WASM path requires `fake-indexeddb` in Node.js context (`packages/core/package.json`). The Rust native path uses SQLite (`matrix-sdk` sqlite feature). These are different persistence backends for the same Megolm crypto store — they are not interchangeable file formats. A Node.js session store cannot be read by `mxdx-worker`, and vice versa.

3. **WASM build fragility.** Building WASM for both `nodejs` and `web` targets is a required step before `npm install` succeeds. The `mxdx-core-wasm` crate cannot be compiled to WASM with the full workspace because `matrix-sdk`'s `sqlite` feature conflicts with `rustls-tls` under feature unification. The CI comment explicitly excludes `mxdx-core-wasm` from workspace builds (`ci.yml:54`). This makes the Rust and WASM builds structurally separate.

4. **rustc version pin.** `dtolnay/rust-toolchain@1.93.1` is pinned in both `ci.yml` and `release.yml` due to a trait solver regression in 1.94.0. This means the Rust binaries cannot be compiled with current stable until upstream fixes the regression or matrix-sdk works around it.

5. **`mxdx-exec` is Linux/Unix only.** Uses `std::os::unix::net::UnixStream` for the exit-code socket, which has no Windows equivalent. npm launcher uses Node.js child process events — platform-agnostic.

6. **Config file section incompatibility.** Rust worker reads flat TOML keys (no section wrapper); npm launcher writes to `[launcher]` section. A config file written by one cannot be read by the other without transformation.

7. **`--p2p` flag bug in interop test.** `rust-npm-interop-beta.test.js` lines 46 and 56 pass `'--p2p'` to `spawnRustBinary('mxdx-worker', [..., '--p2p'])`. This flag is not defined in the Rust clap struct and will cause the worker to exit with a parse error. Tests t1a and t1b — the only currently-implemented interop tests — both contain this bug and will fail when run against a real binary.

---

## Files Referenced

- `/home/liamhelmer/repos/epiphytic/mxdx/Cargo.toml`
- `/home/liamhelmer/repos/epiphytic/mxdx/package.json`
- `/home/liamhelmer/repos/epiphytic/mxdx/crates/mxdx-worker/Cargo.toml`
- `/home/liamhelmer/repos/epiphytic/mxdx/crates/mxdx-worker/src/main.rs`
- `/home/liamhelmer/repos/epiphytic/mxdx/crates/mxdx-worker/src/lib.rs`
- `/home/liamhelmer/repos/epiphytic/mxdx/crates/mxdx-worker/src/config.rs`
- `/home/liamhelmer/repos/epiphytic/mxdx/crates/mxdx-worker/src/bin/mxdx_exec.rs`
- `/home/liamhelmer/repos/epiphytic/mxdx/crates/mxdx-worker/tests/e2e_profile.rs`
- `/home/liamhelmer/repos/epiphytic/mxdx/crates/mxdx-client/Cargo.toml`
- `/home/liamhelmer/repos/epiphytic/mxdx/crates/mxdx-client/src/main.rs`
- `/home/liamhelmer/repos/epiphytic/mxdx/crates/mxdx-client/src/lib.rs`
- `/home/liamhelmer/repos/epiphytic/mxdx/crates/mxdx-client/src/cli/mod.rs`
- `/home/liamhelmer/repos/epiphytic/mxdx/crates/mxdx-client/src/config.rs`
- `/home/liamhelmer/repos/epiphytic/mxdx/crates/mxdx-launcher/Cargo.toml`
- `/home/liamhelmer/repos/epiphytic/mxdx/crates/mxdx-launcher/src/main.rs`
- `/home/liamhelmer/repos/epiphytic/mxdx/crates/mxdx-coordinator/Cargo.toml`
- `/home/liamhelmer/repos/epiphytic/mxdx/crates/mxdx-coordinator/src/main.rs`
- `/home/liamhelmer/repos/epiphytic/mxdx/crates/mxdx-p2p/Cargo.toml`
- `/home/liamhelmer/repos/epiphytic/mxdx/crates/mxdx-types/src/config.rs`
- `/home/liamhelmer/repos/epiphytic/mxdx/crates/mxdx-fabric-cli/Cargo.toml`
- `/home/liamhelmer/repos/epiphytic/mxdx/packages/core/package.json`
- `/home/liamhelmer/repos/epiphytic/mxdx/packages/core/p2p-crypto.js`
- `/home/liamhelmer/repos/epiphytic/mxdx/packages/launcher/package.json`
- `/home/liamhelmer/repos/epiphytic/mxdx/packages/launcher/bin/mxdx-launcher.js`
- `/home/liamhelmer/repos/epiphytic/mxdx/packages/launcher/src/config.js`
- `/home/liamhelmer/repos/epiphytic/mxdx/packages/launcher/src/runtime.js`
- `/home/liamhelmer/repos/epiphytic/mxdx/packages/launcher/tests/runtime-unit.test.js`
- `/home/liamhelmer/repos/epiphytic/mxdx/packages/client/package.json`
- `/home/liamhelmer/repos/epiphytic/mxdx/packages/client/bin/mxdx-client.js`
- `/home/liamhelmer/repos/epiphytic/mxdx/packages/client/src/config.js`
- `/home/liamhelmer/repos/epiphytic/mxdx/packages/mxdx/package.json`
- `/home/liamhelmer/repos/epiphytic/mxdx/packages/mxdx/bin/mxdx.js`
- `/home/liamhelmer/repos/epiphytic/mxdx/packages/coordinator/package.json`
- `/home/liamhelmer/repos/epiphytic/mxdx/packages/coordinator/bin/mxdx-coordinator.js`
- `/home/liamhelmer/repos/epiphytic/mxdx/packages/e2e-tests/package.json`
- `/home/liamhelmer/repos/epiphytic/mxdx/packages/e2e-tests/src/beta.js`
- `/home/liamhelmer/repos/epiphytic/mxdx/packages/e2e-tests/tests/launcher-onboarding.test.js`
- `/home/liamhelmer/repos/epiphytic/mxdx/packages/e2e-tests/tests/launcher-commands.test.js`
- `/home/liamhelmer/repos/epiphytic/mxdx/packages/e2e-tests/tests/public-server.test.js`
- `/home/liamhelmer/repos/epiphytic/mxdx/packages/e2e-tests/tests/rust-npm-interop-beta.test.js`
- `/home/liamhelmer/repos/epiphytic/mxdx/packages/e2e-tests/tests/p2p-signaling.test.js`
- `/home/liamhelmer/repos/epiphytic/mxdx/docs/adr/2026-04-16-coordinated-rust-npm-releases.md`
- `/home/liamhelmer/repos/epiphytic/mxdx/.github/workflows/ci.yml`
- `/home/liamhelmer/repos/epiphytic/mxdx/.github/workflows/release.yml`
- `/home/liamhelmer/repos/epiphytic/mxdx/scripts/e2e-test-suite.sh`
