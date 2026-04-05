# E2E Test Execution & Security Gate Design

## Goal

Restructure the Rust E2E test suite into phased execution with security gates, persistent worker/client lifecycle, sync profiling, and client exit codes for security-relevant failures.

## Architecture

Tests are ordered functions (`t00_*` through `t41_*`) sharing a persistent worker/client via `OnceLock<SharedTestState>`. Security gate tests run first with no worker — they verify the client fails fast and correctly when conditions are unsafe. Only after gates pass does the worker start. Local tests reuse the shared worker/client without restarts. Federated tests spawn their own isolated instances. Accounts are purged only on full success.

## Components

### 1. Test Execution Phases

**Phase 0 — Security Gates** (`t00_*`, `t01_*`, `t02_*`)

Tests that verify the client refuses to send events under unsafe conditions. These run first. If any fails, all remaining tests are skipped — a failing security gate indicates a security vulnerability.

| Test | Setup | Assert |
|------|-------|--------|
| `t00_security_no_worker_room` | Client targets a room name that doesn't exist | Exit code 10, stderr contains "No worker room found" |
| `t01_security_stale_worker` | Start worker with `--telemetry-refresh 1`, let it post telemetry, kill it, wait 3s. Client submits task. | Exit code 11, stderr contains "stale" or "No live worker" |
| `t02_security_capability_mismatch` | Start worker with `--allowed-command echo` only. Client runs `md5sum`. | Exit code 12, stderr contains "No worker supports command" |

Each security gate test uses its own isolated tempdir (not the persistent store). The stale worker test's worker is killed after posting telemetry — it is not the persistent worker.

**Phase 1 — Sync Profile** (`t10_*`)

| Test | What it does |
|------|-------------|
| `t10_start_worker_and_sync` | Start the persistent worker. Run warmup command via client. Measure three timings: `worker-startup` (spawn to ready), `client-connect` (client spawn to warmup complete), `sync-total` (worker spawn to warmup complete). Store worker process and connection details in shared state. Log whether fresh login or session restore was used. |

**Phase 2 — Local Tests** (`t20_*`)

All reuse the shared worker/client from Phase 1. No process restarts between tests.

| Test | Command | Transport |
|------|---------|-----------|
| `t20_echo_local` | `/bin/echo hello world` | mxdx-local |
| `t21_exit_code_local` | `/bin/false` | mxdx-local |
| `t22_md5sum_local` | md5sum 10k lines | mxdx-local |
| `t23_ping_local` | `ping -c 30 1.1.1.1` | mxdx-local |

**Phase 3 — Federated Tests** (`t30_*`)

Spawn their own worker (server1) + client (server2) with tempdir isolation. Independent lifecycle from the shared state.

| Test | Command | Transport |
|------|---------|-----------|
| `t30_echo_federated` | `/bin/echo hello world` | mxdx-federated |
| `t31_exit_code_federated` | `/bin/false` | mxdx-federated |
| `t32_md5sum_federated` | md5sum 10k lines | mxdx-federated |
| `t33_ping_federated` | `ping -c 30 1.1.1.1` | mxdx-federated |

Worker invites the client's Matrix ID on server2 (`client_matrix_id_on(s2)`).

**Phase 4 — Special Tests** (`t40_*`)

| Test | Setup | What it tests |
|------|-------|--------------|
| `t40_echo_explicit_room_name` | Own worker/client with `--room-name mxdx-e2e-profile-explicit` | Room name override still works |
| `t41_session_restore` | Two sequential worker runs with same store/keychain dirs | Second run attempts session restore |

**Cleanup — Purge**

After all tests complete, if all passed: run `node scripts/purge-test-accounts.mjs` to clean all devices and rooms on both servers. If any test failed: skip purge, preserve state for debugging.

### 2. Client Exit Codes

The client binary (`mxdx-client`) returns specific exit codes for security-relevant failures. These are checked programmatically by the security gate tests.

| Exit Code | Meaning | Stderr Message |
|-----------|---------|----------------|
| 0 | Success | — |
| 1 | Generic error (unchanged) | varies |
| 10 | No worker room found | "No worker room found for '{name}'. Has the worker started and invited this client?" |
| 11 | No live worker | "No live worker in room '{name}'" (with details: "last seen {N}s ago" for stale, "Worker is offline" for offline, "No telemetry found" for missing) |
| 12 | No worker supports command | "No worker supports command '{cmd}'" |

**Implementation:** A mapping function in `crates/mxdx-client/src/main.rs` converts error types to exit codes. The errors are produced by a pre-send validation step that runs after room discovery but before posting any task event.

**Pre-send validation flow:**

1. `find_launcher_space()` → error with exit 10 if None
2. Read all `org.mxdx.host_telemetry` state events from exec room (keyed by `worker/{device_id}`)
3. Run `check_worker_liveness()` on each → collect all `Online` workers
4. If zero online workers → error with exit 11
5. Check if any online worker's capabilities include the requested command → error with exit 12 if none
6. Only then: post the task event

Multiple workers can post telemetry to the same exec room. The client checks all of them and succeeds if any live worker supports the command.

### 3. Shared Test State

```rust
static SHARED_STATE: OnceLock<SharedTestState> = OnceLock::new();
static SECURITY_GATE_FAILED: AtomicBool = AtomicBool::new(false);

struct SharedTestState {
    worker: Mutex<Child>,
    worker_room: String,
    creds: TestCreds,
    store_dir: PathBuf,      // ~/.mxdx/e2e-local/store
    keychain_dir: PathBuf,   // ~/.mxdx/e2e-local/keychain
}
```

- `t10_start_worker_and_sync` initializes the `OnceLock`.
- `t20_*` tests access `SHARED_STATE.get()`. If `SECURITY_GATE_FAILED` is true, they return immediately (skip).
- Store/keychain paths are fixed at `~/.mxdx/e2e-local/store` and `~/.mxdx/e2e-local/keychain` — persisted across test runs. First run after purge = fresh login. Subsequent runs = session restore.
- Federated and security gate tests use tempdirs (independent of shared state).
- Worker cleanup: the test binary exits after all tests; the OS kills the child process.

### 4. Sync Profiling

`t10_start_worker_and_sync` reports three performance metrics:

| Metric Name | Transport | What it measures |
|-------------|-----------|-----------------|
| `worker-startup` | setup | Worker spawn to "Listening for commands" in stderr |
| `client-connect` | setup | Client warmup command start to completion |
| `sync-total` | setup | Worker spawn to warmup complete (wall clock) |

All reported via `report()` so they appear in perf JSON and stderr table.

The test also logs the connection type observed (parsed from stderr):
- "fresh login completed" → cold start
- "session restored successfully" → warm start

This makes session restore regressions visible in the perf JSON over time.

### 5. Security Gate Failure Behavior

When any `t0*` test fails (the client did NOT reject as expected):

1. Set `SECURITY_GATE_FAILED` to `true`
2. All subsequent tests (`t10_*` through `t41_*`) check this flag and skip immediately
3. The test binary reports the security gate failure prominently in stderr
4. The overall test result is FAILED
5. Account purge is skipped (preserving state for investigation)

This ensures a security regression is never masked by later tests passing.

## Files Changed

| File | Change |
|---|---|
| `crates/mxdx-worker/tests/e2e_profile.rs` | Restructure into phased `t00_*`–`t41_*` tests with shared state |
| `crates/mxdx-client/src/main.rs` | Add exit code mapping (10, 11, 12) for security failures |
| `crates/mxdx-client/src/matrix.rs` | Add pre-send validation: read all worker telemetry, check liveness + capabilities |
| `crates/mxdx-client/src/liveness.rs` | Add capability checking helper (already has liveness check) |
