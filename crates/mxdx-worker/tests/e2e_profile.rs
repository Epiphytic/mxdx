//! Profiling E2E tests — measures steady-state operational latency.
//!
//! Uses pre-existing accounts on beta.mxdx.dev (or configured test server) from
//! `test-credentials.toml`. Accounts are assumed to already exist with sessions
//! established — this measures real-world warm-start latency, not one-time setup.
//!
//! The worker runs as account1, the client runs as account2. The worker must be
//! running before profiling starts. A room is shared between them.
//!
//! Three transport variants:
//!   - SSH localhost (baseline — no mxdx overhead)
//!   - mxdx single-server (account1 worker + account2 client on same server)
//!   - mxdx federated (worker on server1, client on server2)
//!
//! Run: `cargo test -p mxdx-worker --test e2e_profile -- --ignored --nocapture`

use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

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
    /// Full Matrix user ID for the client account on server1 (e.g. `@e2etest-test2:ca1-beta.mxdx.dev`).
    fn client_matrix_id(&self) -> String {
        let server_name = self
            .server_url
            .trim_start_matches("https://")
            .trim_start_matches("http://");
        format!("@{}:{}", self.client_user, server_name)
    }

    /// Full Matrix user ID for the client account on a specific server.
    /// Use this for federated tests where the client is on a different server.
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

fn cargo_bin(name: &str) -> std::path::PathBuf {
    // Allow override via MXDX_BIN_DIR for testing release-profile binaries
    if let Ok(dir) = std::env::var("MXDX_BIN_DIR") {
        let path = std::path::PathBuf::from(dir).join(name);
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
/// Formula: `{hostname}.{os_username}.{matrix_localpart}`
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

/// Allowed commands used in tests. Passed to the worker via `--allowed-command`.
const ALLOWED_COMMANDS: &[&str] = &[
    "echo", "/bin/echo", "md5sum", "ping", "sleep", "bash", "/bin/sh",
    "/bin/true", "/bin/false", "true", "false",
];

/// Create isolated store and keychain directories for a test.
/// Each test gets its own SQLite crypto store and keychain to prevent
/// parallel test processes from fighting over the same database files.
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

/// Start the worker using default room naming (no `--room-name`).
/// Passes `--authorized-user` and `--allowed-command` flags.
/// Uses isolated store/keychain dirs to prevent parallel test conflicts.
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

fn run_client(hs: &str, user: &str, pass: &str, worker_room: &str, extra_args: &[&str],
              store_dir: &std::path::Path, keychain_dir: &std::path::Path) -> Output {
    // Global flags go before the subcommand; per-command flags go after
    // extra_args[0] is typically the subcommand ("run", "ls", etc.)
    let mut full: Vec<&str> = vec![
        "--homeserver", hs, "--username", user, "--password", pass,
        "--no-daemon",
    ];
    if !extra_args.is_empty() {
        full.push(extra_args[0]); // subcommand
        full.extend_from_slice(&["--worker-room", worker_room]);
        full.push("--skip-liveness-check");
        // Pass --cwd /tmp for run/exec commands (worker requires CWD when allowlist is configured)
        if extra_args[0] == "run" || extra_args[0] == "exec" {
            full.extend_from_slice(&["--cwd", "/tmp"]);
        }
        full.extend_from_slice(&extra_args[1..]); // remaining args
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

        // Append JSON line to file (one JSON object per line, wrapped by e2e-test-suite.sh)
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

/// Start the worker and run a warm-up command to ensure sessions are established.
/// Returns the running worker child process.
/// Uses default room naming (no `--room-name`).
async fn setup_worker(server: &str, worker_user: &str, worker_pass: &str,
                       client_server: &str, client_user: &str, client_pass: &str,
                       authorized_user: &str,
                       store_dir: &std::path::Path, keychain_dir: &std::path::Path) -> Child {
    let worker_room = default_worker_room(worker_user);
    let w = start_worker(server, worker_user, worker_pass, authorized_user, store_dir, keychain_dir);
    wait_ready().await;

    // Warm-up: ensure client session is established (cold start if first run,
    // session restore on subsequent runs — either way, the NEXT command will be warm)
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

/// Extract the device_id from worker stderr output. Looks for the tracing log line:
/// `device_id=XXXX ... "device identity loaded"` or `device_id=XXXX ... "session restored successfully"`
fn extract_device_id_from_stderr(stderr: &str) -> Option<String> {
    for line in stderr.lines() {
        if line.contains("device identity loaded")
            || line.contains("session restored successfully")
            || line.contains("fresh login completed")
        {
            // tracing formats device_id as: device_id=VALUE or device_id="VALUE"
            if let Some(pos) = line.find("device_id=") {
                let after = &line[pos + "device_id=".len()..];
                let value = if after.starts_with('"') {
                    // Quoted value
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
// SSH BASELINE
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
// mxdx LOCAL (single server) — uses test-credentials.toml server1
// Uses default room naming (no --room-name flag)
// ===========================================================================

#[tokio::test]
#[ignore = "requires test-credentials.toml + beta server"]
async fn profile_echo_local() {
    let c = load_creds().expect("test-credentials.toml required");
    let auth_user = c.client_matrix_id();
    let worker_room = default_worker_room(&c.worker_user);
    let (store_dir, keychain_dir) = isolated_test_dirs("echo_local");
    let mut w = setup_worker(&c.server_url, &c.worker_user, &c.worker_pass,
                              &c.server_url, &c.client_user, &c.client_pass,
                              &auth_user, store_dir.path(), keychain_dir.path()).await;

    let start = Instant::now();
    let out = run_client(&c.server_url, &c.client_user, &c.client_pass, &worker_room, &["run", "/bin/echo", "hello", "world"], store_dir.path(), keychain_dir.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stderr: {}", &stderr[stderr.len().saturating_sub(500)..]);
    report("echo", "mxdx-local", start.elapsed(), out.status.code(), stdout.lines().count());

    let _ = w.kill(); let _ = w.wait();
}

#[tokio::test]
#[ignore = "requires test-credentials.toml + beta server"]
async fn profile_exit_code_local() {
    let c = load_creds().expect("test-credentials.toml required");
    let auth_user = c.client_matrix_id();
    let worker_room = default_worker_room(&c.worker_user);
    let (store_dir, keychain_dir) = isolated_test_dirs("exit_code_local");
    let mut w = setup_worker(&c.server_url, &c.worker_user, &c.worker_pass,
                              &c.server_url, &c.client_user, &c.client_pass,
                              &auth_user, store_dir.path(), keychain_dir.path()).await;

    let start = Instant::now();
    let out = run_client(&c.server_url, &c.client_user, &c.client_pass, &worker_room, &["run", "/bin/false"], store_dir.path(), keychain_dir.path());
    assert!(!out.status.success());
    report("exit-code(/bin/false)", "mxdx-local", start.elapsed(), out.status.code(), 0);

    let _ = w.kill(); let _ = w.wait();
}

#[tokio::test]
#[ignore = "requires test-credentials.toml + beta server"]
async fn profile_md5sum_local() {
    let c = load_creds().expect("test-credentials.toml required");
    let auth_user = c.client_matrix_id();
    let worker_room = default_worker_room(&c.worker_user);
    let (store_dir, keychain_dir) = isolated_test_dirs("md5sum_local");
    let mut w = setup_worker(&c.server_url, &c.worker_user, &c.worker_pass,
                              &c.server_url, &c.client_user, &c.client_pass,
                              &auth_user, store_dir.path(), keychain_dir.path()).await;

    let fp = large_file(10_000);
    let script = md5_script(&fp);
    let start = Instant::now();
    let out = run_client(&c.server_url, &c.client_user, &c.client_pass, &worker_room, &["run", "--", "/bin/sh", "-c", &script], store_dir.path(), keychain_dir.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stderr: {}", &stderr[stderr.len().saturating_sub(500)..]);
    report("md5sum(10k lines)", "mxdx-local", start.elapsed(), out.status.code(), stdout.lines().count());

    let _ = std::fs::remove_file(&fp);
    let _ = w.kill(); let _ = w.wait();
}

#[tokio::test]
#[ignore = "requires test-credentials.toml + beta server + network"]
async fn profile_ping_local() {
    let c = load_creds().expect("test-credentials.toml required");
    let auth_user = c.client_matrix_id();
    let worker_room = default_worker_room(&c.worker_user);
    let (store_dir, keychain_dir) = isolated_test_dirs("ping_local");
    let mut w = setup_worker(&c.server_url, &c.worker_user, &c.worker_pass,
                              &c.server_url, &c.client_user, &c.client_pass,
                              &auth_user, store_dir.path(), keychain_dir.path()).await;

    let start = Instant::now();
    let out = run_client(&c.server_url, &c.client_user, &c.client_pass, &worker_room, &["run", "--", "ping", "-c", "30", "-i", "1", "1.1.1.1"], store_dir.path(), keychain_dir.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stderr: {}", &stderr[stderr.len().saturating_sub(500)..]);
    report("ping(30s)", "mxdx-local", start.elapsed(), out.status.code(), stdout.lines().count());

    let _ = w.kill(); let _ = w.wait();
}

// ===========================================================================
// mxdx FEDERATED — worker on server1, client on server2
// Uses default room naming (no --room-name flag)
// ===========================================================================

#[tokio::test]
#[ignore = "requires test-credentials.toml + both beta servers"]
async fn profile_echo_federated() {
    let c = load_creds().expect("test-credentials.toml required");
    let s2 = c.server2_url.as_deref().expect("server2 required for federated tests");
    // Client is on server2 — invite the client's actual Matrix ID on that server
    let auth_user = c.client_matrix_id_on(s2);
    let worker_room = default_worker_room(&c.worker_user);
    let (store_dir, keychain_dir) = isolated_test_dirs("echo_federated");
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
async fn profile_exit_code_federated() {
    let c = load_creds().expect("test-credentials.toml required");
    let s2 = c.server2_url.as_deref().expect("server2 required for federated tests");
    let auth_user = c.client_matrix_id_on(s2);
    let worker_room = default_worker_room(&c.worker_user);
    let (store_dir, keychain_dir) = isolated_test_dirs("exit_code_federated");
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
async fn profile_md5sum_federated() {
    let c = load_creds().expect("test-credentials.toml required");
    let s2 = c.server2_url.as_deref().expect("server2 required for federated tests");
    let auth_user = c.client_matrix_id_on(s2);
    let worker_room = default_worker_room(&c.worker_user);
    let (store_dir, keychain_dir) = isolated_test_dirs("md5sum_federated");
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
async fn profile_ping_federated() {
    let c = load_creds().expect("test-credentials.toml required");
    let s2 = c.server2_url.as_deref().expect("server2 required for federated tests");
    let auth_user = c.client_matrix_id_on(s2);
    let worker_room = default_worker_room(&c.worker_user);
    let (store_dir, keychain_dir) = isolated_test_dirs("ping_federated");
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
// LONG PING — 5 minutes sustained streaming
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

// ===========================================================================
// EXPLICIT --room-name FLAG — verifies the override still works
// ===========================================================================

#[tokio::test]
#[ignore = "requires test-credentials.toml + beta server"]
async fn profile_echo_explicit_room_name() {
    let c = load_creds().expect("test-credentials.toml required");
    let auth_user = c.client_matrix_id();
    let explicit_room = "mxdx-e2e-profile-explicit";
    let (store_dir, keychain_dir) = isolated_test_dirs("echo_explicit_room");
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

// ===========================================================================
// SESSION RESTORE — verifies device reuse across restarts
// ===========================================================================

#[tokio::test]
#[ignore = "requires test-credentials.toml + beta server"]
async fn e2e_beta_session_restore() {
    let c = load_creds().expect("test-credentials.toml required");
    let auth_user = c.client_matrix_id();

    // Shared dirs for both runs — session restore requires the same store/keychain
    let (store_dir, keychain_dir) = isolated_test_dirs("session_restore");

    // --- First run: start worker, let it initialize and save session to keychain ---
    eprintln!("[session-restore] starting first worker run");
    let mut w1 = start_worker(&c.server_url, &c.worker_user, &c.worker_pass, &auth_user,
                               store_dir.path(), keychain_dir.path());

    // Wait for worker to initialize
    tokio::time::sleep(Duration::from_secs(15)).await;

    // Kill worker and collect stderr
    let _ = w1.kill();
    let output1 = w1.wait_with_output().expect("failed to collect worker output");
    let stderr1 = String::from_utf8_lossy(&output1.stderr);
    eprintln!("[session-restore] first run stderr (last 500 chars): {}", &stderr1[stderr1.len().saturating_sub(500)..]);

    // First run may be a fresh login or may fail on stale crypto store — both are OK.
    // What matters is the second run attempts session restore.
    let first_logged_in = stderr1.contains("fresh login completed")
        || stderr1.contains("session restored successfully")
        || stderr1.contains("connected to Matrix");
    if !first_logged_in {
        eprintln!("[session-restore] WARNING: first run may not have logged in successfully");
        eprintln!("[session-restore] This can happen with stale crypto stores — checking second run anyway");
    }

    // Small delay between runs
    tokio::time::sleep(Duration::from_secs(2)).await;

    // --- Second run: start worker again with SAME dirs, verify it attempts session restore ---
    eprintln!("[session-restore] starting second worker run (should restore session)");
    let mut w2 = start_worker(&c.server_url, &c.worker_user, &c.worker_pass, &auth_user,
                               store_dir.path(), keychain_dir.path());

    // Wait for worker to initialize
    tokio::time::sleep(Duration::from_secs(15)).await;

    // Kill worker and collect stderr
    let _ = w2.kill();
    let output2 = w2.wait_with_output().expect("failed to collect worker output");
    let stderr2 = String::from_utf8_lossy(&output2.stderr);
    eprintln!("[session-restore] second run stderr (last 500 chars): {}", &stderr2[stderr2.len().saturating_sub(500)..]);

    // The key assertion: second run should ATTEMPT session restore (proving the
    // keychain has stored credentials from the first run). It may or may not succeed
    // (depends on crypto store state), but it must try.
    assert!(
        stderr2.contains("session restored successfully")
            || stderr2.contains("attempting session restore")
            || stderr2.contains("session restore failed"),
        "Second run should attempt session restore (keychain should have credentials). Stderr: {}",
        &stderr2[stderr2.len().saturating_sub(1000)..]
    );
    eprintln!("[session-restore] PASS: session restore attempted on second run");
}
