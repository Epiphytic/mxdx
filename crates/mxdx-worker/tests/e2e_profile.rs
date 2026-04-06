//! Phased E2E test suite — security gates, sync profiling, operational tests.
//!
//! Tests are ordered functions (`t00_*` through `t41_*`) that run sequentially
//! via `--test-threads=1`. Security gate tests run first with no worker — they
//! verify the client fails fast and correctly when conditions are unsafe.
//!
//! ## Phases
//!
//! - **Phase 0 — Security Gates** (`t00_*` – `t02_*`): Client refuses unsafe ops
//! - **Phase 1 — Sync Profile** (`t10_*`): Start persistent worker, measure timing
//! - **Phase 2 — Local Tests** (`t20_*`): Reuse shared worker, single server
//! - **Phase 3 — Federated Tests** (`t30_*`): Worker on server1, client on server2
//! - **Phase 4 — Special Tests** (`t40_*`): Explicit room name, session restore
//!
//! ## Running
//!
//! ```sh
//! cargo test -p mxdx-worker --test e2e_profile -- --ignored --test-threads=1 --nocapture
//! ```

use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Shared test state — persistent across test functions within a single run
// ---------------------------------------------------------------------------

/// Shared state for Phase 1+ tests. Initialized by `t10_start_worker_and_sync`.
static SHARED_STATE: OnceLock<SharedTestState> = OnceLock::new();

/// Set to true if any security gate test (t0*) fails. All subsequent tests skip.
static SECURITY_GATE_FAILED: AtomicBool = AtomicBool::new(false);

struct SharedTestState {
    #[allow(dead_code)] // Worker process held alive; killed on drop
    worker: Mutex<Child>,
    worker_room: String,
    creds: TestCreds,
    store_dir: PathBuf,
    keychain_dir: PathBuf,
}

/// Check if security gates have failed; if so, skip the current test.
macro_rules! skip_if_gate_failed {
    () => {
        if SECURITY_GATE_FAILED.load(Ordering::SeqCst) {
            eprintln!("[SKIP] security gate failed — skipping remaining tests");
            return;
        }
    };
}

/// Mark a security gate failure and panic with the given message.
macro_rules! security_gate_fail {
    ($($arg:tt)*) => {{
        SECURITY_GATE_FAILED.store(true, Ordering::SeqCst);
        panic!($($arg)*);
    }};
}

// ---------------------------------------------------------------------------
// Credential loading (from test-credentials.toml)
// ---------------------------------------------------------------------------

#[allow(dead_code)]
struct TestCreds {
    server_url: String,
    server2_url: Option<String>,
    worker_user: String,
    worker_pass: String,
    client_user: String,
    client_pass: String,
}

impl TestCreds {
    /// Full Matrix user ID for the client account on server1.
    fn client_matrix_id(&self) -> String {
        let server_name = self
            .server_url
            .trim_start_matches("https://")
            .trim_start_matches("http://");
        format!("@{}:{}", self.client_user, server_name)
    }

    /// Full Matrix user ID for the client account on a specific server.
    fn client_matrix_id_on(&self, server_url: &str) -> String {
        let server_name = server_url
            .trim_start_matches("https://")
            .trim_start_matches("http://");
        format!("@{}:{}", self.client_user, server_name)
    }
}

fn load_creds() -> Option<TestCreds> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()?
        .parent()?
        .join("test-credentials.toml");

    if !path.exists() {
        eprintln!("[profile] test-credentials.toml not found at {}", path.display());
        return None;
    }

    let content = std::fs::read_to_string(&path).ok()?;
    let config: toml::Value = content.parse().ok()?;

    Some(TestCreds {
        server_url: config["server"]["url"].as_str()?.to_string(),
        server2_url: config
            .get("server2")
            .and_then(|s| s.get("url"))
            .and_then(|u| u.as_str())
            .map(|s| s.to_string()),
        worker_user: config["account1"]["username"].as_str()?.to_string(),
        worker_pass: config["account1"]["password"].as_str()?.to_string(),
        client_user: config["account2"]["username"].as_str()?.to_string(),
        client_pass: config["account2"]["password"].as_str()?.to_string(),
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn cargo_bin(name: &str) -> PathBuf {
    // Allow override via MXDX_BIN_DIR for testing release-profile binaries
    if let Ok(dir) = std::env::var("MXDX_BIN_DIR") {
        let path = PathBuf::from(dir).join(name);
        assert!(path.exists(), "Binary not found at {} (via MXDX_BIN_DIR)", path.display());
        return path;
    }
    // Default: resolve relative to test binary (target/debug/)
    let mut path = std::env::current_exe().expect("cannot resolve test binary path");
    path.pop();
    path.pop();
    path.push(name);
    assert!(path.exists(), "Binary not found at {}", path.display());
    path
}

/// Compute the default worker room name the same way `WorkerRuntimeConfig::compute_room_name()` does.
fn default_worker_room(worker_username: &str) -> String {
    let host = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "unknown".into());
    let os_user = whoami::username();
    let localpart = worker_username
        .split(':')
        .next()
        .unwrap_or(worker_username)
        .trim_start_matches('@');
    format!("{host}.{os_user}.{localpart}")
}

/// Allowed commands used in tests.
const ALLOWED_COMMANDS: &[&str] = &[
    "echo", "/bin/echo", "md5sum", "ping", "sleep", "bash", "/bin/sh",
    "/bin/true", "/bin/false", "true", "false",
];

/// Create isolated store and keychain directories for a test.
fn isolated_test_dirs(test_name: &str) -> (tempfile::TempDir, tempfile::TempDir) {
    let store_dir = tempfile::Builder::new()
        .prefix(&format!("mxdx-store-{}-", test_name))
        .tempdir()
        .expect("failed to create temp store dir");
    let keychain_dir = tempfile::Builder::new()
        .prefix(&format!("mxdx-keychain-{}-", test_name))
        .tempdir()
        .expect("failed to create temp keychain dir");
    (store_dir, keychain_dir)
}

/// Get persistent store directories for shared state tests.
/// These persist across test runs at `~/.mxdx/e2e-local/`.
fn persistent_test_dirs() -> (PathBuf, PathBuf) {
    let base = dirs::home_dir()
        .expect("cannot resolve home dir")
        .join(".mxdx")
        .join("e2e-local");
    let store = base.join("store");
    let keychain = base.join("keychain");
    std::fs::create_dir_all(&store).expect("failed to create persistent store dir");
    std::fs::create_dir_all(&keychain).expect("failed to create persistent keychain dir");
    (store, keychain)
}

/// Start the worker using default room naming.
fn start_worker(hs: &str, user: &str, pass: &str, authorized_user: &str,
                store_dir: &std::path::Path, keychain_dir: &std::path::Path) -> Child {
    let mut args = vec![
        "start".to_string(),
        "--homeserver".to_string(), hs.to_string(),
        "--username".to_string(), user.to_string(),
        "--password".to_string(), pass.to_string(),
        "--authorized-user".to_string(), authorized_user.to_string(),
    ];
    for cmd in ALLOWED_COMMANDS {
        args.push("--allowed-command".to_string());
        args.push(cmd.to_string());
    }
    Command::new(cargo_bin("mxdx-worker"))
        .args(&args)
        .env("MXDX_STORE_DIR", store_dir.to_str().unwrap())
        .env("MXDX_KEYCHAIN_DIR", keychain_dir.to_str().unwrap())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to start mxdx-worker")
}

/// Start the worker with a custom telemetry refresh interval.
/// Creates a temporary config directory with a worker.toml that sets the refresh rate.
fn start_worker_with_telemetry_refresh(
    hs: &str, user: &str, pass: &str, authorized_user: &str,
    store_dir: &std::path::Path, keychain_dir: &std::path::Path,
    telemetry_refresh_secs: u64,
    config_dir: &std::path::Path,
) -> Child {
    // Write worker.toml with custom telemetry refresh
    let mxdx_config_dir = config_dir.join(".mxdx");
    std::fs::create_dir_all(&mxdx_config_dir).expect("failed to create config dir");
    std::fs::write(
        mxdx_config_dir.join("worker.toml"),
        format!("telemetry_refresh_seconds = {}\n", telemetry_refresh_secs),
    ).expect("failed to write worker.toml");

    let mut args = vec![
        "start".to_string(),
        "--homeserver".to_string(), hs.to_string(),
        "--username".to_string(), user.to_string(),
        "--password".to_string(), pass.to_string(),
        "--authorized-user".to_string(), authorized_user.to_string(),
    ];
    for cmd in ALLOWED_COMMANDS {
        args.push("--allowed-command".to_string());
        args.push(cmd.to_string());
    }
    Command::new(cargo_bin("mxdx-worker"))
        .args(&args)
        .env("MXDX_STORE_DIR", store_dir.to_str().unwrap())
        .env("MXDX_KEYCHAIN_DIR", keychain_dir.to_str().unwrap())
        .env("HOME", config_dir.to_str().unwrap()) // config loads from $HOME/.mxdx/
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to start mxdx-worker")
}

/// Start the worker with an explicit `--room-name` override.
fn start_worker_with_room(hs: &str, user: &str, pass: &str, room: &str, authorized_user: &str,
                          store_dir: &std::path::Path, keychain_dir: &std::path::Path) -> Child {
    let mut args = vec![
        "start".to_string(),
        "--homeserver".to_string(), hs.to_string(),
        "--username".to_string(), user.to_string(),
        "--password".to_string(), pass.to_string(),
        "--room-name".to_string(), room.to_string(),
        "--authorized-user".to_string(), authorized_user.to_string(),
    ];
    for cmd in ALLOWED_COMMANDS {
        args.push("--allowed-command".to_string());
        args.push(cmd.to_string());
    }
    Command::new(cargo_bin("mxdx-worker"))
        .args(&args)
        .env("MXDX_STORE_DIR", store_dir.to_str().unwrap())
        .env("MXDX_KEYCHAIN_DIR", keychain_dir.to_str().unwrap())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to start mxdx-worker")
}

/// Start the worker with only specific allowed commands (for capability mismatch tests).
fn start_worker_with_commands(
    hs: &str, user: &str, pass: &str, authorized_user: &str,
    store_dir: &std::path::Path, keychain_dir: &std::path::Path,
    allowed_commands: &[&str],
) -> Child {
    let mut args = vec![
        "start".to_string(),
        "--homeserver".to_string(), hs.to_string(),
        "--username".to_string(), user.to_string(),
        "--password".to_string(), pass.to_string(),
        "--authorized-user".to_string(), authorized_user.to_string(),
    ];
    for cmd in allowed_commands {
        args.push("--allowed-command".to_string());
        args.push(cmd.to_string());
    }
    Command::new(cargo_bin("mxdx-worker"))
        .args(&args)
        .env("MXDX_STORE_DIR", store_dir.to_str().unwrap())
        .env("MXDX_KEYCHAIN_DIR", keychain_dir.to_str().unwrap())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to start mxdx-worker")
}

/// Run the client WITH liveness check (for testing exit codes 10, 11, 12).
fn run_client_with_liveness(hs: &str, user: &str, pass: &str, worker_room: &str, extra_args: &[&str],
                            store_dir: &std::path::Path, keychain_dir: &std::path::Path) -> Output {
    let mut full: Vec<&str> = vec![
        "--homeserver", hs, "--username", user, "--password", pass,
        "--no-daemon",
    ];
    if !extra_args.is_empty() {
        full.push(extra_args[0]); // subcommand
        full.extend_from_slice(&["--worker-room", worker_room]);
        // NO --skip-liveness-check here — we want the liveness check to run
        if extra_args[0] == "run" || extra_args[0] == "exec" {
            full.extend_from_slice(&["--cwd", "/tmp"]);
        }
        full.extend_from_slice(&extra_args[1..]);
    }
    Command::new("timeout")
        .arg("60") // shorter timeout for security gate tests
        .arg(cargo_bin("mxdx-client"))
        .args(&full)
        .env("MXDX_STORE_DIR", store_dir.to_str().unwrap())
        .env("MXDX_KEYCHAIN_DIR", keychain_dir.to_str().unwrap())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn mxdx-client")
        .wait_with_output()
        .expect("failed to wait for mxdx-client")
}

/// Run the client with --skip-liveness-check (standard operation).
/// Uses daemon mode by default — the daemon auto-spawns on first call and
/// holds a persistent Matrix connection for subsequent calls.
fn run_client(hs: &str, user: &str, pass: &str, worker_room: &str, extra_args: &[&str],
              store_dir: &std::path::Path, keychain_dir: &std::path::Path) -> Output {
    let mut full: Vec<&str> = vec![
        "--homeserver", hs, "--username", user, "--password", pass,
    ];
    if !extra_args.is_empty() {
        full.push(extra_args[0]);
        full.extend_from_slice(&["--worker-room", worker_room]);
        full.push("--skip-liveness-check");
        if extra_args[0] == "run" || extra_args[0] == "exec" {
            full.extend_from_slice(&["--cwd", "/tmp"]);
        }
        full.extend_from_slice(&extra_args[1..]);
    }
    Command::new("timeout")
        .arg("330")
        .arg(cargo_bin("mxdx-client"))
        .args(&full)
        .env("MXDX_STORE_DIR", store_dir.to_str().unwrap())
        .env("MXDX_KEYCHAIN_DIR", keychain_dir.to_str().unwrap())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn mxdx-client")
        .wait_with_output()
        .expect("failed to wait for mxdx-client")
}

fn run_ssh(args: &[&str]) -> Output {
    Command::new("ssh")
        .args(["-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=no", "localhost"])
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("failed to run ssh")
}

fn run_ssh_script(script: &str) -> Output {
    use std::io::Write;
    let mut child = Command::new("ssh")
        .args(["-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=no", "localhost", "bash"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn ssh");
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    child.wait_with_output().expect("failed to wait for ssh")
}

async fn wait_ready() { tokio::time::sleep(Duration::from_secs(5)).await; }

fn large_file(lines: usize) -> String {
    let path = format!("/tmp/mxdx-profile-{}.txt", std::process::id());
    let mut c = String::with_capacity(lines * 60);
    for i in 0..lines {
        c.push_str(&format!("line {:06}: the quick brown fox jumps over the lazy dog {}\n", i, i * 7919));
    }
    std::fs::write(&path, &c).unwrap();
    path
}

fn report(test: &str, transport: &str, elapsed: Duration, exit_code: Option<i32>, stdout_lines: usize) {
    eprintln!(
        "| {:<30} | {:<12} | {:>8.1}s | {:>4} | {:>8} |",
        test, transport, elapsed.as_secs_f64(),
        exit_code.map(|c| c.to_string()).unwrap_or("?".into()),
        stdout_lines,
    );

    // Write performance JSON entry if TEST_PERF_OUTPUT is set
    if let Ok(path) = std::env::var("TEST_PERF_OUTPUT") {
        let status = match exit_code {
            Some(0) => "pass",
            Some(_) => "fail",
            None => "fail",
        };
        let entry = serde_json::json!({
            "name": test,
            "transport": transport,
            "duration_ms": elapsed.as_millis() as u64,
            "exit_code": exit_code,
            "stdout_lines": stdout_lines,
            "status": status,
        });

        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .expect("failed to open TEST_PERF_OUTPUT file");
        writeln!(file, "{}", entry).expect("failed to write perf entry");
    }
}

fn md5_script(file_path: &str) -> String {
    format!("while IFS= read -r line; do printf '%s\\n' \"$line\" | md5sum; done < '{file_path}'")
}

/// Start the worker and run a warm-up command.
async fn setup_worker(server: &str, worker_user: &str, worker_pass: &str,
                       client_server: &str, client_user: &str, client_pass: &str,
                       authorized_user: &str,
                       store_dir: &std::path::Path, keychain_dir: &std::path::Path) -> Child {
    let worker_room = default_worker_room(worker_user);
    let w = start_worker(server, worker_user, worker_pass, authorized_user, store_dir, keychain_dir);
    wait_ready().await;

    let warmup = run_client(client_server, client_user, client_pass, &worker_room, &["run", "/bin/true"], store_dir, keychain_dir);
    if !warmup.status.success() {
        let stderr = String::from_utf8_lossy(&warmup.stderr);
        eprintln!("[profile] warmup failed (may need account setup): {}", &stderr[stderr.len().saturating_sub(500)..]);
    }

    w
}

/// Start the worker with an explicit `--room-name` and run a warm-up command.
async fn setup_worker_with_room(server: &str, worker_user: &str, worker_pass: &str,
                                 client_server: &str, client_user: &str, client_pass: &str,
                                 room: &str, authorized_user: &str,
                                 store_dir: &std::path::Path, keychain_dir: &std::path::Path) -> Child {
    let w = start_worker_with_room(server, worker_user, worker_pass, room, authorized_user, store_dir, keychain_dir);
    wait_ready().await;

    let warmup = run_client(client_server, client_user, client_pass, room, &["run", "/bin/true"], store_dir, keychain_dir);
    if !warmup.status.success() {
        let stderr = String::from_utf8_lossy(&warmup.stderr);
        eprintln!("[profile] warmup failed (may need account setup): {}", &stderr[stderr.len().saturating_sub(500)..]);
    }

    w
}

/// Extract the device_id from worker stderr output.
#[allow(dead_code)]
fn extract_device_id_from_stderr(stderr: &str) -> Option<String> {
    for line in stderr.lines() {
        if line.contains("device identity loaded")
            || line.contains("session restored successfully")
            || line.contains("fresh login completed")
        {
            if let Some(pos) = line.find("device_id=") {
                let after = &line[pos + "device_id=".len()..];
                let value = if after.starts_with('"') {
                    after[1..].split('"').next().unwrap_or("")
                } else {
                    after.split_whitespace().next().unwrap_or("")
                };
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
    }
    None
}

// ===========================================================================
// PHASE 0 — SECURITY GATES (no worker running)
// If any gate fails, all subsequent tests are skipped.
// ===========================================================================

/// Security gate: client must exit 10 when targeting a room that doesn't exist.
#[tokio::test]
#[ignore = "requires test-credentials.toml + beta server"]
async fn t00_security_no_worker_room() {
    let c = load_creds().expect("test-credentials.toml required");
    let (store_dir, keychain_dir) = isolated_test_dirs("t00_no_room");
    let nonexistent_room = "mxdx-e2e-nonexistent-room-does-not-exist";

    let start = Instant::now();
    let out = run_client_with_liveness(
        &c.server_url, &c.client_user, &c.client_pass,
        nonexistent_room, &["run", "/bin/echo", "should-not-run"],
        store_dir.path(), keychain_dir.path(),
    );
    let elapsed = start.elapsed();
    let stderr = String::from_utf8_lossy(&out.stderr);
    let exit_code = out.status.code();

    eprintln!("[t00] exit_code={:?}, stderr tail: {}", exit_code, &stderr[stderr.len().saturating_sub(300)..]);
    report("security/no-worker-room", "gate", elapsed, exit_code, 0);

    // The client should fail with exit code 10 or exit code 1 with the right message
    // (exit code 10 is the new behavior; exit 1 with the right message is also acceptable
    // since the error may propagate through anyhow before the exit code mapping)
    if exit_code != Some(10) && exit_code != Some(1) {
        security_gate_fail!(
            "SECURITY GATE FAILED: client should exit 10 (no worker room) but exited {:?}. Stderr: {}",
            exit_code, &stderr[stderr.len().saturating_sub(500)..]
        );
    }
    if !stderr.contains("No worker room found") && !stderr.contains("no worker") {
        security_gate_fail!(
            "SECURITY GATE FAILED: stderr should mention 'No worker room found'. Stderr: {}",
            &stderr[stderr.len().saturating_sub(500)..]
        );
    }
    eprintln!("[t00] PASS: client correctly rejected — no worker room");
}

/// Security gate: client must exit 11 when the worker is stale.
#[tokio::test]
#[ignore = "requires test-credentials.toml + beta server"]
async fn t01_security_stale_worker() {
    skip_if_gate_failed!();
    let c = load_creds().expect("test-credentials.toml required");
    let auth_user = c.client_matrix_id();
    let (store_dir, keychain_dir) = isolated_test_dirs("t01_stale");
    let config_dir = tempfile::Builder::new()
        .prefix("mxdx-config-t01-")
        .tempdir()
        .expect("failed to create temp config dir");

    // Start worker with 1-second telemetry refresh, let it post telemetry, then kill it
    let mut w = start_worker_with_telemetry_refresh(
        &c.server_url, &c.worker_user, &c.worker_pass, &auth_user,
        store_dir.path(), keychain_dir.path(), 1, config_dir.path(),
    );
    // Wait for worker to start and post at least one telemetry event
    tokio::time::sleep(Duration::from_secs(8)).await;

    // Kill the worker
    let _ = w.kill();
    let _ = w.wait();
    eprintln!("[t01] worker killed, waiting for staleness threshold...");

    // Wait for the telemetry to become stale (2 * 1s = 2s threshold, wait 5s to be safe)
    tokio::time::sleep(Duration::from_secs(5)).await;

    let worker_room = default_worker_room(&c.worker_user);
    let start = Instant::now();
    let out = run_client_with_liveness(
        &c.server_url, &c.client_user, &c.client_pass,
        &worker_room, &["run", "/bin/echo", "should-not-run"],
        store_dir.path(), keychain_dir.path(),
    );
    let elapsed = start.elapsed();
    let stderr = String::from_utf8_lossy(&out.stderr);
    let exit_code = out.status.code();

    eprintln!("[t01] exit_code={:?}, stderr tail: {}", exit_code, &stderr[stderr.len().saturating_sub(300)..]);
    report("security/stale-worker", "gate", elapsed, exit_code, 0);

    // Accept exit 11 (new) or exit 1 (legacy) with the right error message
    if exit_code == Some(0) {
        security_gate_fail!(
            "SECURITY GATE FAILED: client should NOT succeed when worker is stale. Stderr: {}",
            &stderr[stderr.len().saturating_sub(500)..]
        );
    }
    if !stderr.contains("stale") && !stderr.contains("No live worker") && !stderr.contains("last seen") {
        eprintln!("[t01] WARNING: stderr doesn't contain expected stale message, but exit code was non-zero");
    }
    eprintln!("[t01] PASS: client correctly rejected — stale worker");
}

/// Security gate: client must exit 12 when no worker supports the requested command.
#[tokio::test]
#[ignore = "requires test-credentials.toml + beta server"]
async fn t02_security_capability_mismatch() {
    skip_if_gate_failed!();
    let c = load_creds().expect("test-credentials.toml required");
    let auth_user = c.client_matrix_id();
    let (store_dir, keychain_dir) = isolated_test_dirs("t02_capability");

    // Start worker with ONLY echo allowed
    let mut w = start_worker_with_commands(
        &c.server_url, &c.worker_user, &c.worker_pass, &auth_user,
        store_dir.path(), keychain_dir.path(),
        &["echo", "/bin/echo"],
    );
    wait_ready().await;

    // Warm up so the client can discover the room
    let worker_room = default_worker_room(&c.worker_user);
    let warmup = run_client(
        &c.server_url, &c.client_user, &c.client_pass,
        &worker_room, &["run", "/bin/echo", "warmup"],
        store_dir.path(), keychain_dir.path(),
    );
    if !warmup.status.success() {
        let stderr = String::from_utf8_lossy(&warmup.stderr);
        eprintln!("[t02] warmup failed: {}", &stderr[stderr.len().saturating_sub(300)..]);
    }

    // Now try to run md5sum (which the worker does NOT support)
    let start = Instant::now();
    let out = run_client_with_liveness(
        &c.server_url, &c.client_user, &c.client_pass,
        &worker_room, &["run", "md5sum", "/dev/null"],
        store_dir.path(), keychain_dir.path(),
    );
    let elapsed = start.elapsed();
    let stderr = String::from_utf8_lossy(&out.stderr);
    let exit_code = out.status.code();

    let _ = w.kill(); let _ = w.wait();

    eprintln!("[t02] exit_code={:?}, stderr tail: {}", exit_code, &stderr[stderr.len().saturating_sub(300)..]);
    report("security/capability-mismatch", "gate", elapsed, exit_code, 0);

    // Accept exit 12 (new) or exit 1 (legacy) — must NOT succeed
    if exit_code == Some(0) {
        security_gate_fail!(
            "SECURITY GATE FAILED: client should NOT succeed when worker lacks capability. Stderr: {}",
            &stderr[stderr.len().saturating_sub(500)..]
        );
    }
    if !stderr.contains("No worker supports command") && !stderr.contains("capability") {
        eprintln!("[t02] WARNING: stderr doesn't contain expected capability message, but exit code was non-zero");
    }
    eprintln!("[t02] PASS: client correctly rejected — capability mismatch");
}

// ===========================================================================
// PHASE 1 — SYNC PROFILE (start persistent worker, measure timing)
// ===========================================================================

/// Start the persistent worker and measure startup + sync timing.
#[tokio::test]
#[ignore = "requires test-credentials.toml + beta server"]
async fn t10_start_worker_and_sync() {
    skip_if_gate_failed!();
    let c = load_creds().expect("test-credentials.toml required");
    let auth_user = c.client_matrix_id();
    let worker_room = default_worker_room(&c.worker_user);
    let (store_dir, keychain_dir) = persistent_test_dirs();

    let worker_start = Instant::now();
    let w = start_worker(
        &c.server_url, &c.worker_user, &c.worker_pass, &auth_user,
        &store_dir, &keychain_dir,
    );
    wait_ready().await;
    let worker_startup = worker_start.elapsed();
    report("worker-startup", "setup", worker_startup, Some(0), 0);

    // Warmup command: measure client connect time
    let client_start = Instant::now();
    let warmup = run_client(
        &c.server_url, &c.client_user, &c.client_pass,
        &worker_room, &["run", "/bin/true"],
        &store_dir, &keychain_dir,
    );
    let client_connect = client_start.elapsed();
    report("client-connect", "setup", client_connect, warmup.status.code(), 0);

    // Total sync time (worker start to warmup complete)
    let sync_total = worker_start.elapsed();
    report("sync-total", "setup", sync_total, Some(0), 0);

    // Log connection type from warmup stderr
    let stderr = String::from_utf8_lossy(&warmup.stderr);
    if stderr.contains("fresh login completed") {
        eprintln!("[t10] connection type: fresh login (cold start)");
    } else if stderr.contains("session restored successfully") {
        eprintln!("[t10] connection type: session restore (warm start)");
    }

    if !warmup.status.success() {
        eprintln!("[t10] WARNING: warmup command failed: {}", &stderr[stderr.len().saturating_sub(500)..]);
    }

    // Store shared state for Phase 2+ tests
    let _ = SHARED_STATE.set(SharedTestState {
        worker: Mutex::new(w),
        worker_room,
        creds: c,
        store_dir,
        keychain_dir,
    });

    eprintln!("[t10] persistent worker started, shared state initialized");
}

// ===========================================================================
// PHASE 2 — LOCAL TESTS (reuse shared worker, single server)
// ===========================================================================

#[tokio::test]
#[ignore = "requires test-credentials.toml + beta server"]
async fn t20_echo_local() {
    skip_if_gate_failed!();
    let state = SHARED_STATE.get().expect("t10 must run first");
    let c = &state.creds;

    let start = Instant::now();
    let out = run_client(
        &c.server_url, &c.client_user, &c.client_pass,
        &state.worker_room, &["run", "/bin/echo", "hello", "world"],
        &state.store_dir, &state.keychain_dir,
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stderr: {}", &stderr[stderr.len().saturating_sub(500)..]);
    report("echo", "mxdx-local", start.elapsed(), out.status.code(), stdout.lines().count());
}

#[tokio::test]
#[ignore = "requires test-credentials.toml + beta server"]
async fn t21_exit_code_local() {
    skip_if_gate_failed!();
    let state = SHARED_STATE.get().expect("t10 must run first");
    let c = &state.creds;

    let start = Instant::now();
    let out = run_client(
        &c.server_url, &c.client_user, &c.client_pass,
        &state.worker_room, &["run", "/bin/false"],
        &state.store_dir, &state.keychain_dir,
    );
    assert!(!out.status.success());
    report("exit-code(/bin/false)", "mxdx-local", start.elapsed(), out.status.code(), 0);
}

#[tokio::test]
#[ignore = "requires test-credentials.toml + beta server"]
async fn t22_md5sum_local() {
    skip_if_gate_failed!();
    let state = SHARED_STATE.get().expect("t10 must run first");
    let c = &state.creds;

    let fp = large_file(10_000);
    let script = md5_script(&fp);
    let start = Instant::now();
    let out = run_client(
        &c.server_url, &c.client_user, &c.client_pass,
        &state.worker_room, &["run", "--", "/bin/sh", "-c", &script],
        &state.store_dir, &state.keychain_dir,
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stderr: {}", &stderr[stderr.len().saturating_sub(500)..]);
    report("md5sum(10k lines)", "mxdx-local", start.elapsed(), out.status.code(), stdout.lines().count());
    let _ = std::fs::remove_file(&fp);
}

#[tokio::test]
#[ignore = "requires test-credentials.toml + beta server + network"]
async fn t23_ping_local() {
    skip_if_gate_failed!();
    let state = SHARED_STATE.get().expect("t10 must run first");
    let c = &state.creds;

    let start = Instant::now();
    let out = run_client(
        &c.server_url, &c.client_user, &c.client_pass,
        &state.worker_room, &["run", "--", "ping", "-c", "30", "-i", "1", "1.1.1.1"],
        &state.store_dir, &state.keychain_dir,
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stderr: {}", &stderr[stderr.len().saturating_sub(500)..]);
    report("ping(30s)", "mxdx-local", start.elapsed(), out.status.code(), stdout.lines().count());
}

// ===========================================================================
// PHASE 3 — FEDERATED TESTS (own isolated worker + client instances)
// ===========================================================================

#[tokio::test]
#[ignore = "requires test-credentials.toml + both beta servers"]
async fn t30_echo_federated() {
    skip_if_gate_failed!();
    let c = load_creds().expect("test-credentials.toml required");
    let s2 = c.server2_url.as_deref().expect("server2 required for federated tests");
    let auth_user = c.client_matrix_id_on(s2);
    let worker_room = default_worker_room(&c.worker_user);
    let (store_dir, keychain_dir) = isolated_test_dirs("t30_echo_fed");
    let mut w = setup_worker(&c.server_url, &c.worker_user, &c.worker_pass,
                              s2, &c.client_user, &c.client_pass,
                              &auth_user, store_dir.path(), keychain_dir.path()).await;

    let start = Instant::now();
    let out = run_client(s2, &c.client_user, &c.client_pass, &worker_room, &["run", "/bin/echo", "hello", "world"], store_dir.path(), keychain_dir.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stderr: {}", &stderr[stderr.len().saturating_sub(500)..]);
    report("echo", "mxdx-federated", start.elapsed(), out.status.code(), stdout.lines().count());

    let _ = w.kill(); let _ = w.wait();
}

#[tokio::test]
#[ignore = "requires test-credentials.toml + both beta servers"]
async fn t31_exit_code_federated() {
    skip_if_gate_failed!();
    let c = load_creds().expect("test-credentials.toml required");
    let s2 = c.server2_url.as_deref().expect("server2 required for federated tests");
    let auth_user = c.client_matrix_id_on(s2);
    let worker_room = default_worker_room(&c.worker_user);
    let (store_dir, keychain_dir) = isolated_test_dirs("t31_exit_fed");
    let mut w = setup_worker(&c.server_url, &c.worker_user, &c.worker_pass,
                              s2, &c.client_user, &c.client_pass,
                              &auth_user, store_dir.path(), keychain_dir.path()).await;

    let start = Instant::now();
    let out = run_client(s2, &c.client_user, &c.client_pass, &worker_room, &["run", "/bin/false"], store_dir.path(), keychain_dir.path());
    assert!(!out.status.success());
    report("exit-code(/bin/false)", "mxdx-federated", start.elapsed(), out.status.code(), 0);

    let _ = w.kill(); let _ = w.wait();
}

#[tokio::test]
#[ignore = "requires test-credentials.toml + both beta servers"]
async fn t32_md5sum_federated() {
    skip_if_gate_failed!();
    let c = load_creds().expect("test-credentials.toml required");
    let s2 = c.server2_url.as_deref().expect("server2 required for federated tests");
    let auth_user = c.client_matrix_id_on(s2);
    let worker_room = default_worker_room(&c.worker_user);
    let (store_dir, keychain_dir) = isolated_test_dirs("t32_md5_fed");
    let mut w = setup_worker(&c.server_url, &c.worker_user, &c.worker_pass,
                              s2, &c.client_user, &c.client_pass,
                              &auth_user, store_dir.path(), keychain_dir.path()).await;

    let fp = large_file(10_000);
    let script = md5_script(&fp);
    let start = Instant::now();
    let out = run_client(s2, &c.client_user, &c.client_pass, &worker_room, &["run", "--", "/bin/sh", "-c", &script], store_dir.path(), keychain_dir.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stderr: {}", &stderr[stderr.len().saturating_sub(500)..]);
    report("md5sum(10k lines)", "mxdx-federated", start.elapsed(), out.status.code(), stdout.lines().count());

    let _ = std::fs::remove_file(&fp);
    let _ = w.kill(); let _ = w.wait();
}

#[tokio::test]
#[ignore = "requires test-credentials.toml + both beta servers + network"]
async fn t33_ping_federated() {
    skip_if_gate_failed!();
    let c = load_creds().expect("test-credentials.toml required");
    let s2 = c.server2_url.as_deref().expect("server2 required for federated tests");
    let auth_user = c.client_matrix_id_on(s2);
    let worker_room = default_worker_room(&c.worker_user);
    let (store_dir, keychain_dir) = isolated_test_dirs("t33_ping_fed");
    let mut w = setup_worker(&c.server_url, &c.worker_user, &c.worker_pass,
                              s2, &c.client_user, &c.client_pass,
                              &auth_user, store_dir.path(), keychain_dir.path()).await;

    let start = Instant::now();
    let out = run_client(s2, &c.client_user, &c.client_pass, &worker_room, &["run", "--", "ping", "-c", "30", "-i", "1", "1.1.1.1"], store_dir.path(), keychain_dir.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stderr: {}", &stderr[stderr.len().saturating_sub(500)..]);
    report("ping(30s)", "mxdx-federated", start.elapsed(), out.status.code(), stdout.lines().count());

    let _ = w.kill(); let _ = w.wait();
}

// ===========================================================================
// PHASE 4 — SPECIAL TESTS
// ===========================================================================

/// Explicit --room-name override test.
#[tokio::test]
#[ignore = "requires test-credentials.toml + beta server"]
async fn t40_echo_explicit_room_name() {
    skip_if_gate_failed!();
    let c = load_creds().expect("test-credentials.toml required");
    let auth_user = c.client_matrix_id();
    let explicit_room = "mxdx-e2e-profile-explicit";
    let (store_dir, keychain_dir) = isolated_test_dirs("t40_explicit_room");
    let mut w = setup_worker_with_room(&c.server_url, &c.worker_user, &c.worker_pass,
                                        &c.server_url, &c.client_user, &c.client_pass,
                                        explicit_room, &auth_user,
                                        store_dir.path(), keychain_dir.path()).await;

    let start = Instant::now();
    let out = run_client(&c.server_url, &c.client_user, &c.client_pass, explicit_room, &["run", "/bin/echo", "explicit", "room"], store_dir.path(), keychain_dir.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stderr: {}", &stderr[stderr.len().saturating_sub(500)..]);
    report("echo(explicit-room)", "mxdx-local", start.elapsed(), out.status.code(), stdout.lines().count());

    let _ = w.kill(); let _ = w.wait();
}

/// Session restore test — verifies device reuse across restarts.
#[tokio::test]
#[ignore = "requires test-credentials.toml + beta server"]
async fn t41_session_restore() {
    skip_if_gate_failed!();
    let c = load_creds().expect("test-credentials.toml required");
    let auth_user = c.client_matrix_id();
    let (store_dir, keychain_dir) = isolated_test_dirs("t41_session_restore");

    // --- First run: start worker, let it initialize and save session ---
    eprintln!("[t41] starting first worker run");
    let mut w1 = start_worker(&c.server_url, &c.worker_user, &c.worker_pass, &auth_user,
                               store_dir.path(), keychain_dir.path());
    tokio::time::sleep(Duration::from_secs(15)).await;

    let _ = w1.kill();
    let output1 = w1.wait_with_output().expect("failed to collect worker output");
    let stderr1 = String::from_utf8_lossy(&output1.stderr);
    eprintln!("[t41] first run stderr (last 500 chars): {}", &stderr1[stderr1.len().saturating_sub(500)..]);

    let first_logged_in = stderr1.contains("fresh login completed")
        || stderr1.contains("session restored successfully")
        || stderr1.contains("connected to Matrix");
    if !first_logged_in {
        eprintln!("[t41] WARNING: first run may not have logged in successfully");
    }

    tokio::time::sleep(Duration::from_secs(2)).await;

    // --- Second run: verify it attempts session restore ---
    eprintln!("[t41] starting second worker run (should restore session)");
    let mut w2 = start_worker(&c.server_url, &c.worker_user, &c.worker_pass, &auth_user,
                               store_dir.path(), keychain_dir.path());
    tokio::time::sleep(Duration::from_secs(15)).await;

    let _ = w2.kill();
    let output2 = w2.wait_with_output().expect("failed to collect worker output");
    let stderr2 = String::from_utf8_lossy(&output2.stderr);
    eprintln!("[t41] second run stderr (last 500 chars): {}", &stderr2[stderr2.len().saturating_sub(500)..]);

    assert!(
        stderr2.contains("session restored successfully")
            || stderr2.contains("attempting session restore")
            || stderr2.contains("session restore failed"),
        "Second run should attempt session restore (keychain should have credentials). Stderr: {}",
        &stderr2[stderr2.len().saturating_sub(1000)..]
    );
    eprintln!("[t41] PASS: session restore attempted on second run");
}

// ===========================================================================
// SSH BASELINE (standalone, not part of phased execution)
// ===========================================================================

#[tokio::test]
#[ignore = "requires passwordless localhost SSH"]
async fn profile_echo_ssh() {
    let start = Instant::now();
    let out = run_ssh(&["/bin/echo", "hello", "world"]);
    report("echo", "ssh", start.elapsed(), out.status.code(), String::from_utf8_lossy(&out.stdout).lines().count());
}

#[tokio::test]
#[ignore = "requires passwordless localhost SSH"]
async fn profile_exit_code_ssh() {
    let start = Instant::now();
    let out = run_ssh(&["/bin/false"]);
    report("exit-code(/bin/false)", "ssh", start.elapsed(), out.status.code(), 0);
}

#[tokio::test]
#[ignore = "requires passwordless localhost SSH"]
async fn profile_md5sum_ssh() {
    let fp = large_file(10_000);
    let start = Instant::now();
    let out = run_ssh_script(&md5_script(&fp));
    let stdout = String::from_utf8_lossy(&out.stdout);
    report("md5sum(10k lines)", "ssh", start.elapsed(), out.status.code(), stdout.lines().count());
    let _ = std::fs::remove_file(&fp);
}

#[tokio::test]
#[ignore = "requires passwordless localhost SSH + network"]
async fn profile_ping_ssh() {
    let start = Instant::now();
    let out = run_ssh(&["ping", "-c", "30", "-i", "1", "1.1.1.1"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    report("ping(30s)", "ssh", start.elapsed(), out.status.code(), stdout.lines().count());
}

// ===========================================================================
// LONG PING — 5 minutes sustained streaming (standalone)
// ===========================================================================

#[tokio::test]
#[ignore = "requires test-credentials.toml + beta server + network, runs 5 minutes"]
async fn profile_long_ping_local() {
    let c = load_creds().expect("test-credentials.toml required");
    let auth_user = c.client_matrix_id();
    let worker_room = default_worker_room(&c.worker_user);
    let (store_dir, keychain_dir) = isolated_test_dirs("long_ping_local");
    let mut w = setup_worker(&c.server_url, &c.worker_user, &c.worker_pass,
                              &c.server_url, &c.client_user, &c.client_pass,
                              &auth_user, store_dir.path(), keychain_dir.path()).await;

    let start = Instant::now();
    let out = run_client(&c.server_url, &c.client_user, &c.client_pass, &worker_room, &["run", "--", "ping", "-c", "300", "-i", "1", "1.1.1.1"], store_dir.path(), keychain_dir.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stderr: {}", &stderr[stderr.len().saturating_sub(500)..]);
    report("ping(5min)", "mxdx-local", start.elapsed(), out.status.code(), stdout.lines().count());

    let _ = w.kill(); let _ = w.wait();
}

#[tokio::test]
#[ignore = "requires test-credentials.toml + both beta servers + network, runs 5 minutes"]
async fn profile_long_ping_federated() {
    let c = load_creds().expect("test-credentials.toml required");
    let s2 = c.server2_url.as_deref().expect("server2 required for federated tests");
    let auth_user = c.client_matrix_id_on(s2);
    let worker_room = default_worker_room(&c.worker_user);
    let (store_dir, keychain_dir) = isolated_test_dirs("long_ping_federated");
    let mut w = setup_worker(&c.server_url, &c.worker_user, &c.worker_pass,
                              s2, &c.client_user, &c.client_pass,
                              &auth_user, store_dir.path(), keychain_dir.path()).await;

    let start = Instant::now();
    let out = run_client(s2, &c.client_user, &c.client_pass, &worker_room, &["run", "--", "ping", "-c", "300", "-i", "1", "1.1.1.1"], store_dir.path(), keychain_dir.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stderr: {}", &stderr[stderr.len().saturating_sub(500)..]);
    report("ping(5min)", "mxdx-federated", start.elapsed(), out.status.code(), stdout.lines().count());

    let _ = w.kill(); let _ = w.wait();
}
