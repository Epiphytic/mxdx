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
    let mut path = std::env::current_exe().expect("cannot resolve test binary path");
    path.pop();
    path.pop();
    path.push(name);
    assert!(path.exists(), "Binary not found at {}", path.display());
    path
}

fn start_worker(hs: &str, user: &str, pass: &str, room: &str) -> Child {
    Command::new(cargo_bin("mxdx-worker"))
        .args(["start", "--homeserver", hs, "--username", user, "--password", pass, "--room-name", room])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to start mxdx-worker")
}

fn run_client(hs: &str, user: &str, pass: &str, room: &str, args: &[&str]) -> Output {
    // Global flags go before the subcommand; --worker-room goes after the subcommand
    // args[0] is typically the subcommand ("run", "ls", etc.)
    let mut full: Vec<&str> = vec!["--homeserver", hs, "--username", user, "--password", pass];
    if !args.is_empty() {
        full.push(args[0]); // subcommand
        full.extend_from_slice(&["--worker-room", room]);
        full.extend_from_slice(&args[1..]); // remaining args
    }
    Command::new("timeout")
        .arg("330")
        .arg(cargo_bin("mxdx-client"))
        .args(&full)
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
}

fn md5_script(file_path: &str) -> String {
    format!("while IFS= read -r line; do printf '%s\\n' \"$line\" | md5sum; done < '{file_path}'")
}

/// The worker room name used for profiling. Shared between worker and client.
const PROFILE_ROOM: &str = "mxdx-e2e-profile";

/// Start the worker and run a warm-up command to ensure sessions are established.
/// Returns the running worker child process.
async fn setup_worker(server: &str, worker_user: &str, worker_pass: &str,
                       client_server: &str, client_user: &str, client_pass: &str) -> Child {
    let mut w = start_worker(server, worker_user, worker_pass, PROFILE_ROOM);
    wait_ready().await;

    // Warm-up: ensure client session is established (cold start if first run,
    // session restore on subsequent runs — either way, the NEXT command will be warm)
    let warmup = run_client(client_server, client_user, client_pass, PROFILE_ROOM, &["run", "/bin/true"]);
    if !warmup.status.success() {
        let stderr = String::from_utf8_lossy(&warmup.stderr);
        eprintln!("[profile] warmup failed (may need account setup): {}", &stderr[stderr.len().saturating_sub(500)..]);
    }

    w
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
// ===========================================================================

#[tokio::test]
#[ignore = "requires test-credentials.toml + beta server"]
async fn profile_echo_local() {
    let c = load_creds().expect("test-credentials.toml required");
    let mut w = setup_worker(&c.server_url, &c.worker_user, &c.worker_pass,
                              &c.server_url, &c.client_user, &c.client_pass).await;

    let start = Instant::now();
    let out = run_client(&c.server_url, &c.client_user, &c.client_pass, PROFILE_ROOM, &["run", "/bin/echo", "hello", "world"]);
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
    let mut w = setup_worker(&c.server_url, &c.worker_user, &c.worker_pass,
                              &c.server_url, &c.client_user, &c.client_pass).await;

    let start = Instant::now();
    let out = run_client(&c.server_url, &c.client_user, &c.client_pass, PROFILE_ROOM, &["run", "/bin/false"]);
    assert!(!out.status.success());
    report("exit-code(/bin/false)", "mxdx-local", start.elapsed(), out.status.code(), 0);

    let _ = w.kill(); let _ = w.wait();
}

#[tokio::test]
#[ignore = "requires test-credentials.toml + beta server"]
async fn profile_md5sum_local() {
    let c = load_creds().expect("test-credentials.toml required");
    let mut w = setup_worker(&c.server_url, &c.worker_user, &c.worker_pass,
                              &c.server_url, &c.client_user, &c.client_pass).await;

    let fp = large_file(10_000);
    let script = md5_script(&fp);
    let start = Instant::now();
    let out = run_client(&c.server_url, &c.client_user, &c.client_pass, PROFILE_ROOM, &["run", "--", "/bin/sh", "-c", &script]);
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
    let mut w = setup_worker(&c.server_url, &c.worker_user, &c.worker_pass,
                              &c.server_url, &c.client_user, &c.client_pass).await;

    let start = Instant::now();
    let out = run_client(&c.server_url, &c.client_user, &c.client_pass, PROFILE_ROOM, &["run", "--", "ping", "-c", "30", "-i", "1", "1.1.1.1"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stderr: {}", &stderr[stderr.len().saturating_sub(500)..]);
    report("ping(30s)", "mxdx-local", start.elapsed(), out.status.code(), stdout.lines().count());

    let _ = w.kill(); let _ = w.wait();
}

// ===========================================================================
// mxdx FEDERATED — worker on server1, client on server2
// ===========================================================================

#[tokio::test]
#[ignore = "requires test-credentials.toml + both beta servers"]
async fn profile_echo_federated() {
    let c = load_creds().expect("test-credentials.toml required");
    let s2 = c.server2_url.as_deref().expect("server2 required for federated tests");
    let mut w = setup_worker(&c.server_url, &c.worker_user, &c.worker_pass,
                              s2, &c.client_user, &c.client_pass).await;

    let start = Instant::now();
    let out = run_client(s2, &c.client_user, &c.client_pass, PROFILE_ROOM, &["run", "/bin/echo", "hello", "world"]);
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
    let mut w = setup_worker(&c.server_url, &c.worker_user, &c.worker_pass,
                              s2, &c.client_user, &c.client_pass).await;

    let start = Instant::now();
    let out = run_client(s2, &c.client_user, &c.client_pass, PROFILE_ROOM, &["run", "/bin/false"]);
    assert!(!out.status.success());
    report("exit-code(/bin/false)", "mxdx-federated", start.elapsed(), out.status.code(), 0);

    let _ = w.kill(); let _ = w.wait();
}

#[tokio::test]
#[ignore = "requires test-credentials.toml + both beta servers"]
async fn profile_md5sum_federated() {
    let c = load_creds().expect("test-credentials.toml required");
    let s2 = c.server2_url.as_deref().expect("server2 required for federated tests");
    let mut w = setup_worker(&c.server_url, &c.worker_user, &c.worker_pass,
                              s2, &c.client_user, &c.client_pass).await;

    let fp = large_file(10_000);
    let script = md5_script(&fp);
    let start = Instant::now();
    let out = run_client(s2, &c.client_user, &c.client_pass, PROFILE_ROOM, &["run", "--", "/bin/sh", "-c", &script]);
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
    let mut w = setup_worker(&c.server_url, &c.worker_user, &c.worker_pass,
                              s2, &c.client_user, &c.client_pass).await;

    let start = Instant::now();
    let out = run_client(s2, &c.client_user, &c.client_pass, PROFILE_ROOM, &["run", "--", "ping", "-c", "30", "-i", "1", "1.1.1.1"]);
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
    let mut w = setup_worker(&c.server_url, &c.worker_user, &c.worker_pass,
                              &c.server_url, &c.client_user, &c.client_pass).await;

    let start = Instant::now();
    let out = run_client(&c.server_url, &c.client_user, &c.client_pass, PROFILE_ROOM, &["run", "--", "ping", "-c", "300", "-i", "1", "1.1.1.1"]);
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
    let mut w = setup_worker(&c.server_url, &c.worker_user, &c.worker_pass,
                              s2, &c.client_user, &c.client_pass).await;

    let start = Instant::now();
    let out = run_client(s2, &c.client_user, &c.client_pass, PROFILE_ROOM, &["run", "--", "ping", "-c", "300", "-i", "1", "1.1.1.1"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stderr: {}", &stderr[stderr.len().saturating_sub(500)..]);
    report("ping(5min)", "mxdx-federated", start.elapsed(), out.status.code(), stdout.lines().count());

    let _ = w.kill(); let _ = w.wait();
}
