//! True end-to-end tests against the beta Matrix servers (ca1-beta.mxdx.dev / ca2-beta.mxdx.dev).
//!
//! These tests spawn mxdx-worker and mxdx-client as COMPILED BINARIES,
//! connected to the real beta infrastructure. Credentials come from `test-credentials.toml`
//! in the repository root.
//!
//! Requirements:
//!   - `test-credentials.toml` configured in the repo root
//!   - Binaries built: `cargo build -p mxdx-worker -p mxdx-client`
//!   - Beta servers reachable
//!
//! Run with: `cargo test -p mxdx-worker --test e2e_binary_beta -- --ignored --nocapture`

use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use mxdx_test_perf::{PerfEntry, write_perf_entry};

// ---------------------------------------------------------------------------
// Credential loading
// ---------------------------------------------------------------------------

#[allow(dead_code)]
struct TestCredentials {
    server_url: String,
    server2_url: Option<String>,
    username1: String,
    password1: String,
    username2: String,
    password2: String,
    coordinator_username: Option<String>,
    coordinator_password: Option<String>,
}

fn load_test_credentials() -> Option<TestCredentials> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()?
        .parent()?
        .join("test-credentials.toml");

    if !path.exists() {
        return None;
    }

    let content = std::fs::read_to_string(&path).ok()?;
    let config: toml::Value = content.parse().ok()?;

    Some(TestCredentials {
        server_url: config["server"]["url"].as_str()?.to_string(),
        server2_url: config
            .get("server2")
            .and_then(|s| s.get("url"))
            .and_then(|u| u.as_str())
            .map(|s| s.to_string()),
        username1: config["account1"]["username"].as_str()?.to_string(),
        password1: config["account1"]["password"].as_str()?.to_string(),
        username2: config["account2"]["username"].as_str()?.to_string(),
        password2: config["account2"]["password"].as_str()?.to_string(),
        coordinator_username: config
            .get("coordinator")
            .and_then(|c| c.get("username"))
            .and_then(|u| u.as_str())
            .map(|s| s.to_string()),
        coordinator_password: config
            .get("coordinator")
            .and_then(|c| c.get("password"))
            .and_then(|u| u.as_str())
            .map(|s| s.to_string()),
    })
}

// ---------------------------------------------------------------------------
// Binary helpers
// ---------------------------------------------------------------------------

/// Resolve the path to a cargo-built binary in the target directory.
fn cargo_bin(name: &str) -> std::path::PathBuf {
    let mut path = std::env::current_exe().expect("cannot resolve test binary path");
    path.pop(); // remove test binary name
    path.pop(); // remove 'deps'
    path.push(name);
    assert!(
        path.exists(),
        "Binary not found at {}. Run `cargo build -p {}` first.",
        path.display(),
        name,
    );
    path
}

/// Start the mxdx-worker binary as a subprocess against the beta server.
fn start_worker_beta(
    homeserver: &str,
    username: &str,
    password: &str,
    room_name: &str,
) -> Child {
    Command::new(cargo_bin("mxdx-worker"))
        .args([
            "start",
            "--homeserver", homeserver,
            "--username", username,
            "--password", password,
            "--room-name", room_name,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to start mxdx-worker binary")
}

/// Run an mxdx-client command against the beta server.
fn run_client_beta(
    homeserver: &str,
    username: &str,
    password: &str,
    worker_room: &str,
    subcommand_args: &[&str],
) -> Output {
    Command::new(cargo_bin("mxdx-client"))
        .args([
            "--homeserver", homeserver,
            "--username", username,
            "--password", password,
        ])
        .args(subcommand_args)
        .args(["--worker-room", worker_room])
        .output()
        .expect("failed to run mxdx-client binary")
}

/// Give the worker binary time to start, connect to Matrix, and create its room.
async fn wait_for_worker_ready() {
    tokio::time::sleep(Duration::from_secs(10)).await;
}

// ---------------------------------------------------------------------------
// Test 1: Echo command lifecycle on beta server
// ---------------------------------------------------------------------------

/// Uses account1 as worker, account2 as client on the primary beta server.
/// Runs `/bin/echo hello world` and verifies output.
#[tokio::test]
#[ignore = "requires test-credentials.toml and beta server access"]
async fn e2e_beta_echo_command_lifecycle() {
    let test_start = Instant::now();
    let creds = load_test_credentials().expect("test-credentials.toml required");
    let room_name = format!("e2e-beta-echo-{}", std::process::id());

    eprintln!("[beta_echo] Starting worker as {}...", creds.username1);
    let mut worker = start_worker_beta(
        &creds.server_url,
        &creds.username1,
        &creds.password1,
        &room_name,
    );

    wait_for_worker_ready().await;

    eprintln!("[beta_echo] Running client as {}...", creds.username2);
    let output = run_client_beta(
        &creds.server_url,
        &creds.username2,
        &creds.password2,
        &room_name,
        &["run", "/bin/echo", "hello", "world"],
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    eprintln!("[beta_echo] stdout: {}", stdout);
    eprintln!("[beta_echo] stderr: {}", stderr);
    eprintln!("[beta_echo] exit code: {:?}", output.status.code());

    assert!(
        stdout.contains("hello world"),
        "stdout should contain 'hello world', got: {}",
        stdout,
    );
    assert!(
        output.status.success(),
        "client should exit 0 for echo, got: {:?}",
        output.status.code(),
    );

    let _ = write_perf_entry(&PerfEntry {
        suite: "e2e_binary_beta/echo_command_lifecycle".to_string(),
        transport: "same-hs".to_string(),
        runtime: "rust".to_string(),
        duration_ms: test_start.elapsed().as_millis() as u64,
        rss_max: None,
    });

    let _ = worker.kill();
    let _ = worker.wait();
}

// ---------------------------------------------------------------------------
// Test 2: Non-zero exit code on beta server
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires test-credentials.toml and beta server access"]
async fn e2e_beta_nonzero_exit_code() {
    let creds = load_test_credentials().expect("test-credentials.toml required");
    let room_name = format!("e2e-beta-false-{}", std::process::id());

    let mut worker = start_worker_beta(
        &creds.server_url,
        &creds.username1,
        &creds.password1,
        &room_name,
    );

    wait_for_worker_ready().await;

    let output = run_client_beta(
        &creds.server_url,
        &creds.username2,
        &creds.password2,
        &room_name,
        &["run", "/bin/false"],
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    eprintln!("[beta_false] stderr: {}", stderr);
    eprintln!("[beta_false] exit code: {:?}", output.status.code());

    assert!(
        !output.status.success(),
        "client should exit non-zero for /bin/false, got: {:?}",
        output.status.code(),
    );

    let _ = worker.kill();
    let _ = worker.wait();
}

// ---------------------------------------------------------------------------
// Test 3: Detach and ls on beta server
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires test-credentials.toml and beta server access"]
async fn e2e_beta_ls_shows_sessions() {
    let creds = load_test_credentials().expect("test-credentials.toml required");
    let room_name = format!("e2e-beta-ls-{}", std::process::id());

    let mut worker = start_worker_beta(
        &creds.server_url,
        &creds.username1,
        &creds.password1,
        &room_name,
    );

    wait_for_worker_ready().await;

    // Submit detached task
    let detach_output = run_client_beta(
        &creds.server_url,
        &creds.username2,
        &creds.password2,
        &room_name,
        &["run", "--detach", "sleep", "300"],
    );

    let uuid = String::from_utf8_lossy(&detach_output.stdout).trim().to_string();
    eprintln!("[beta_ls] detached UUID: {}", uuid);
    assert!(!uuid.is_empty(), "detach mode should print a UUID");

    tokio::time::sleep(Duration::from_secs(5)).await;

    // List sessions
    let ls_output = run_client_beta(
        &creds.server_url,
        &creds.username2,
        &creds.password2,
        &room_name,
        &["ls"],
    );

    let ls_stdout = String::from_utf8_lossy(&ls_output.stdout);
    eprintln!("[beta_ls] ls output: {}", ls_stdout);

    assert!(
        ls_stdout.contains(&uuid[..8]),
        "ls should show session UUID, got: {}",
        ls_stdout,
    );

    let _ = worker.kill();
    let _ = worker.wait();
}

// ---------------------------------------------------------------------------
// Test 4: Cancel on beta server
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires test-credentials.toml and beta server access"]
async fn e2e_beta_cancel_running_session() {
    let creds = load_test_credentials().expect("test-credentials.toml required");
    let room_name = format!("e2e-beta-cancel-{}", std::process::id());

    let mut worker = start_worker_beta(
        &creds.server_url,
        &creds.username1,
        &creds.password1,
        &room_name,
    );

    wait_for_worker_ready().await;

    // Submit a long-running detached task
    let detach_output = run_client_beta(
        &creds.server_url,
        &creds.username2,
        &creds.password2,
        &room_name,
        &["run", "--detach", "sleep", "300"],
    );

    let uuid = String::from_utf8_lossy(&detach_output.stdout).trim().to_string();
    eprintln!("[beta_cancel] detached UUID: {}", uuid);
    assert!(!uuid.is_empty(), "should get UUID from detach mode");

    tokio::time::sleep(Duration::from_secs(5)).await;

    // Cancel the session
    let cancel_output = run_client_beta(
        &creds.server_url,
        &creds.username2,
        &creds.password2,
        &room_name,
        &["cancel", &uuid],
    );

    let cancel_stderr = String::from_utf8_lossy(&cancel_output.stderr);
    eprintln!("[beta_cancel] cancel stderr: {}", cancel_stderr);

    assert!(
        cancel_output.status.success(),
        "cancel command should succeed, got: {:?}",
        cancel_output.status.code(),
    );

    let _ = worker.kill();
    let _ = worker.wait();
}

// ---------------------------------------------------------------------------
// Test 5: Cross-server (federated) test
// ---------------------------------------------------------------------------

/// If server2 is configured, test running a worker on server1 and client on server2.
/// This exercises federation between the two beta homeservers.
#[tokio::test]
#[ignore = "requires test-credentials.toml with server2 and beta server access"]
async fn e2e_beta_cross_server_echo() {
    let creds = load_test_credentials().expect("test-credentials.toml required");
    let server2 = creds
        .server2_url
        .as_ref()
        .expect("server2 URL required for cross-server test");
    let room_name = format!("e2e-beta-xserver-{}", std::process::id());

    eprintln!(
        "[beta_xserver] Worker on {}, Client on {}",
        creds.server_url, server2,
    );

    // Worker on server1 with account1
    let mut worker = start_worker_beta(
        &creds.server_url,
        &creds.username1,
        &creds.password1,
        &room_name,
    );

    wait_for_worker_ready().await;

    // Client on server2 with account2
    let output = run_client_beta(
        server2,
        &creds.username2,
        &creds.password2,
        &room_name,
        &["run", "/bin/echo", "federated", "hello"],
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    eprintln!("[beta_xserver] stdout: {}", stdout);
    eprintln!("[beta_xserver] stderr: {}", stderr);
    eprintln!("[beta_xserver] exit code: {:?}", output.status.code());

    assert!(
        stdout.contains("federated hello"),
        "federated echo should produce output, got: {}",
        stdout,
    );

    let _ = worker.kill();
    let _ = worker.wait();
}
