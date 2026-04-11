# E2E Test Suite Restructure — Phased Orchestrator

## Problem

The current e2e test suite (`crates/mxdx-worker/tests/e2e_profile.rs`) has structural issues:

1. **Federated tests spin up/tear down their own workers** — ~15-20s setup overhead per test (t30-t33 each create and kill a worker). The worker on s1 is already running; federation only requires the client to connect via s2.

2. **All tests run serially** via `--test-threads=1` — long-running tests (5-minute pings, SSH baselines) could run in parallel since they're independent child processes.

3. **No preset control** — you either run everything or filter individual tests by name. No way to say "quick smoke test" vs "full suite."

4. **Client session overhead** — the ~15-20s per local daemon-mode test suggests the client is re-establishing Matrix connections rather than reusing a persistent daemon.

## Design

### Phase Architecture

A single `#[tokio::test] async fn e2e()` orchestrator calls phase functions in order. Each phase is an `async fn phase_N(...) -> Result<()>` that returns `Err` on any test failure, stopping the suite immediately.

| Phase | Name | Required | Execution | Description |
|-------|------|----------|-----------|-------------|
| 0 | Security gates | no | serial | t00-t02: client refuses unsafe ops (no worker room, stale worker, capability mismatch). Each test uses its own isolated short-lived worker. |
| 1 | Setup worker | **yes** | — | Start persistent worker on s1. Authorize both `@client:s1` and `@client:s2`. Write daemon config. Run warmup command. Store shared state. |
| 2 | Local tests | no | serial | echo, exit-code, md5sum, ping(30s) via daemon mode with s1 client. Reuses persistent worker. |
| 3 | Federated tests | no | serial | Same four tests, but client connects via s2 (`--no-daemon`). Same persistent worker on s1. Tests the s2→s1 federation path. |
| 4 | Long + SSH | no | **parallel** | 5-minute sustained pings (local + federated) and SSH perf baselines. All run as child processes via `std::thread::spawn`. Worker stays active for matrix tests; SSH tests are standalone. |
| 5 | Shutdown worker | **yes** | — | Graceful SIGTERM to persistent worker, wait up to 10s, fallback to SIGKILL. |
| 6 | Special tests | no | serial | session restore (t41), key backup round trip (t42), unencrypted room self-heal (t43), diagnose decrypt (t44), explicit room name (t40). Each spins up its own isolated worker as needed. |
| 7 | Cleanup | **yes** | — | `pkill -f mxdx-worker`, `pkill -f mxdx-client`, remove stale daemon sockets/pidfiles. Safety net for any leaked processes. |

### Presets

Selected via `E2E_PRESET` env var (default: `default`).

| Preset | Phases | Use case |
|--------|--------|----------|
| `quick` | 0, 1, 2, 3, 5, 7 | Fast feedback, ~2 minutes |
| `default` | 0, 1, 2, 3, 5, 6, 7 | Standard dev/CI, ~5 minutes |
| `full` | 0, 1, 2, 3, 4, 5, 6, 7 | Everything including long tests, ~10 minutes |

Required phases (1, 5, 7) always run regardless of preset. If a preset includes phase 4, phase 5 runs after phase 4 completes (worker must stay active for long matrix tests). If a preset excludes phase 4, phase 5 runs immediately after phase 3.

### Invocation

```sh
# Default preset (skip long tests)
cargo test -p mxdx-worker --test e2e_profile -- --ignored e2e --nocapture

# Quick smoke test
E2E_PRESET=quick cargo test -p mxdx-worker --test e2e_profile -- --ignored e2e --nocapture

# Full suite with long tests
E2E_PRESET=full cargo test -p mxdx-worker --test e2e_profile -- --ignored e2e --nocapture
```

### Federated Test Simplification

Current: Each federated test (t30-t33) spawns a dedicated worker on s1 with a unique room name, creates a client on s2, runs one command, kills the worker.

New: The persistent worker from phase 1 authorizes both s1 and s2 client matrix IDs. Federated tests simply call `run_client()` with s2 credentials and `--no-daemon` against the shared worker room. The federation path is: client→s2→federation→s1→worker.

This eliminates ~60-80s of worker setup/teardown across the four federated tests.

### Parallel Long Tests (Phase 4)

Phase 4 tests are child-process-based (they shell out to `mxdx-client` and `ssh`). They run via `std::thread::spawn` + `join()`, not tokio tasks, since the work is blocking process I/O.

Tests in phase 4:
- `long_ping_local` — 5-min ping via s1 daemon, shared worker
- `long_ping_federated` — 5-min ping via s2 --no-daemon, shared worker
- `perf_echo_ssh` — SSH echo baseline
- `perf_exit_code_ssh` — SSH exit code baseline
- `perf_md5sum_ssh` — SSH md5sum baseline
- `perf_ping_ssh` — SSH 30s ping baseline

All six run concurrently. Any failure collected and reported after all threads join.

### Fail-Fast Semantics

- Within serial phases: first test failure returns `Err`, orchestrator stops.
- Within parallel phase 4: all threads run to completion (can't cancel child processes cleanly), but any failure is reported and the suite stops before proceeding to phase 6.
- Required phases 5 and 7 always run, even after failure (cleanup must happen).

### Output

Each sub-test continues to call `report()` producing the existing table format:

```
| echo                           | mxdx-local   |     18.2s |    0 |       50 |
| exit-code(/bin/false)          | mxdx-local   |     23.1s |    1 |        0 |
```

Phase transitions are logged:

```
=== Phase 0: Security Gates ===
...
=== Phase 1: Setup Worker ===
...
=== Phase 2: Local Tests ===
```

### Shared State

Replace `OnceLock<SharedTestState>` + `skip_if_gate_failed!()` with direct parameter passing. The orchestrator owns the worker `Child` and passes references to phase functions:

```rust
struct TestContext {
    worker: Child,
    worker_room: String,
    creds: TestCreds,
    store_dir: PathBuf,
    keychain_dir: PathBuf,
    config_home: PathBuf,
}
```

Phase functions receive `&TestContext` (or `&TestCreds` for phases that don't need the shared worker).

### Files Changed

| File | Change |
|------|--------|
| `crates/mxdx-worker/tests/e2e_profile.rs` | Restructure into orchestrator + phase functions. Remove per-test `#[tokio::test]` wrappers. Add `E2E_PRESET` parsing. Simplify federated tests. Add parallel phase 4. |
