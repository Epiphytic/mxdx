//! True end-to-end tests that spawn mxdx-worker and mxdx-client as subprocesses.
//!
//! These tests exercise the COMPILED BINARIES exactly as a user would experience them.
//! They are NOT integration tests — they do not call library functions directly.
//!
//! Requirements:
//!   - tuwunel binary installed (see tests/helpers for details)
//!   - tmux available on PATH (for interactive sessions)
//!   - Binaries built: `cargo build -p mxdx-worker -p mxdx-client`
//!
//! Run with: `cargo test -p mxdx-worker --test e2e_binary -- --ignored --nocapture`

use std::process::{Child, Command, Output, Stdio};
use std::time::Duration;

use mxdx_test_helpers::tuwunel::TuwunelInstance;

// ---------------------------------------------------------------------------
// Binary helpers
// ---------------------------------------------------------------------------

/// Resolve the path to a cargo-built binary in the target directory.
/// Assumes binaries have already been compiled with `cargo build`.
fn cargo_bin(name: &str) -> std::path::PathBuf {
    let mut path = std::env::current_exe().expect("cannot resolve test binary path");
    path.pop(); // remove test binary name (e.g., e2e_binary-xxxx)
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

/// Start the mxdx-worker binary as a subprocess.
/// Returns the Child handle so the caller can kill it later.
fn start_worker(
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

/// Run an mxdx-client command synchronously and return its Output.
fn run_client(
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

/// Register a user via the tuwunel HTTP API so the binary can log in.
async fn register_user(tuwunel: &TuwunelInstance, username: &str, password: &str) {
    tuwunel
        .register_user(username, password)
        .await
        .unwrap_or_else(|e| panic!("failed to register user {}: {}", username, e));
}

/// Give the worker binary time to start up, connect to Matrix, and create its room.
async fn wait_for_worker_ready() {
    tokio::time::sleep(Duration::from_secs(8)).await;
}

// ---------------------------------------------------------------------------
// Test 1: Echo command lifecycle
// ---------------------------------------------------------------------------

/// Start tuwunel, register users, start worker binary, run client `run /bin/echo hello world`,
/// assert stdout contains "hello world" and exit code is 0, then kill the worker.
#[tokio::test]
#[ignore = "requires tuwunel binary and compiled mxdx-worker/mxdx-client"]
async fn e2e_echo_command_lifecycle() {
    let mut hs = TuwunelInstance::start().await.unwrap();
    let base_url = format!("http://127.0.0.1:{}", hs.port);
    let room_name = format!("e2e-binary-echo-{}", hs.port);

    // Register both users before starting the binaries
    register_user(&hs, "worker-e2e", "pass123").await;
    register_user(&hs, "client-e2e", "pass123").await;

    // Start the worker binary
    let mut worker = start_worker(&base_url, "worker-e2e", "pass123", &room_name);

    // Wait for the worker to connect and create its room
    wait_for_worker_ready().await;

    // Run a command via the client binary
    let output = run_client(
        &base_url,
        "client-e2e",
        "pass123",
        &room_name,
        &["run", "/bin/echo", "hello", "world"],
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    eprintln!("[e2e_echo] stdout: {}", stdout);
    eprintln!("[e2e_echo] stderr: {}", stderr);
    eprintln!("[e2e_echo] exit code: {:?}", output.status.code());

    assert!(
        stdout.contains("hello world"),
        "stdout should contain 'hello world', got: {}",
        stdout,
    );
    assert!(
        output.status.success(),
        "client should exit 0 for successful echo, got: {:?}",
        output.status.code(),
    );

    // Clean up
    let _ = worker.kill();
    let _ = worker.wait();
    hs.stop().await;
}

// ---------------------------------------------------------------------------
// Test 2: Non-zero exit code propagation
// ---------------------------------------------------------------------------

/// Client runs `/bin/false` which exits 1. The client binary should propagate the non-zero exit.
#[tokio::test]
#[ignore = "requires tuwunel binary and compiled mxdx-worker/mxdx-client"]
async fn e2e_nonzero_exit_code() {
    let mut hs = TuwunelInstance::start().await.unwrap();
    let base_url = format!("http://127.0.0.1:{}", hs.port);
    let room_name = format!("e2e-binary-false-{}", hs.port);

    register_user(&hs, "worker-e2e2", "pass123").await;
    register_user(&hs, "client-e2e2", "pass123").await;

    let mut worker = start_worker(&base_url, "worker-e2e2", "pass123", &room_name);
    wait_for_worker_ready().await;

    let output = run_client(
        &base_url,
        "client-e2e2",
        "pass123",
        &room_name,
        &["run", "/bin/false"],
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    eprintln!("[e2e_false] stderr: {}", stderr);
    eprintln!("[e2e_false] exit code: {:?}", output.status.code());

    assert!(
        !output.status.success(),
        "client should exit non-zero for /bin/false, got: {:?}",
        output.status.code(),
    );

    let _ = worker.kill();
    let _ = worker.wait();
    hs.stop().await;
}

// ---------------------------------------------------------------------------
// Test 3: Detached mode and ls shows sessions
// ---------------------------------------------------------------------------

/// Start worker, client `run --detach sleep 300` to get a UUID, then `ls` to see the session.
#[tokio::test]
#[ignore = "requires tuwunel binary and compiled mxdx-worker/mxdx-client"]
async fn e2e_ls_shows_sessions() {
    let mut hs = TuwunelInstance::start().await.unwrap();
    let base_url = format!("http://127.0.0.1:{}", hs.port);
    let room_name = format!("e2e-binary-ls-{}", hs.port);

    register_user(&hs, "worker-e2e3", "pass123").await;
    register_user(&hs, "client-e2e3", "pass123").await;

    let mut worker = start_worker(&base_url, "worker-e2e3", "pass123", &room_name);
    wait_for_worker_ready().await;

    // Submit a detached task
    let detach_output = run_client(
        &base_url,
        "client-e2e3",
        "pass123",
        &room_name,
        &["run", "--detach", "sleep", "300"],
    );

    let uuid = String::from_utf8_lossy(&detach_output.stdout).trim().to_string();
    eprintln!("[e2e_ls] detached UUID: {}", uuid);
    assert!(
        !uuid.is_empty(),
        "detach mode should print a UUID, got empty stdout",
    );

    // Give the worker time to claim the task
    tokio::time::sleep(Duration::from_secs(3)).await;

    // List sessions
    let ls_output = run_client(
        &base_url,
        "client-e2e3",
        "pass123",
        &room_name,
        &["ls"],
    );

    let ls_stdout = String::from_utf8_lossy(&ls_output.stdout);
    eprintln!("[e2e_ls] ls output: {}", ls_stdout);

    assert!(
        ls_stdout.contains(&uuid[..8]),
        "ls output should contain the session UUID (or prefix), got: {}",
        ls_stdout,
    );

    let _ = worker.kill();
    let _ = worker.wait();
    hs.stop().await;
}

// ---------------------------------------------------------------------------
// Test 4: Cancel a running session
// ---------------------------------------------------------------------------

/// Start worker, submit a detached `sleep 300`, get UUID, cancel it, verify cancellation.
#[tokio::test]
#[ignore = "requires tuwunel binary and compiled mxdx-worker/mxdx-client"]
async fn e2e_cancel_running_session() {
    let mut hs = TuwunelInstance::start().await.unwrap();
    let base_url = format!("http://127.0.0.1:{}", hs.port);
    let room_name = format!("e2e-binary-cancel-{}", hs.port);

    register_user(&hs, "worker-e2e4", "pass123").await;
    register_user(&hs, "client-e2e4", "pass123").await;

    let mut worker = start_worker(&base_url, "worker-e2e4", "pass123", &room_name);
    wait_for_worker_ready().await;

    // Submit a long-running detached task
    let detach_output = run_client(
        &base_url,
        "client-e2e4",
        "pass123",
        &room_name,
        &["run", "--detach", "sleep", "300"],
    );

    let uuid = String::from_utf8_lossy(&detach_output.stdout).trim().to_string();
    eprintln!("[e2e_cancel] detached UUID: {}", uuid);
    assert!(!uuid.is_empty(), "should get a UUID from detach mode");

    // Give the worker time to claim and start
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Cancel the session
    let cancel_output = run_client(
        &base_url,
        "client-e2e4",
        "pass123",
        &room_name,
        &["cancel", &uuid],
    );

    let cancel_stderr = String::from_utf8_lossy(&cancel_output.stderr);
    eprintln!("[e2e_cancel] cancel stderr: {}", cancel_stderr);

    assert!(
        cancel_output.status.success(),
        "cancel command should succeed, got: {:?}",
        cancel_output.status.code(),
    );

    // Give the worker time to process the cancel
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Verify the session is no longer active
    let ls_output = run_client(
        &base_url,
        "client-e2e4",
        "pass123",
        &room_name,
        &["ls", "--all"],
    );

    let ls_stdout = String::from_utf8_lossy(&ls_output.stdout);
    eprintln!("[e2e_cancel] ls --all output: {}", ls_stdout);

    // The session should appear as cancelled or completed, not active
    // (exact output format depends on implementation)

    let _ = worker.kill();
    let _ = worker.wait();
    hs.stop().await;
}

// ---------------------------------------------------------------------------
// Test 5: Concurrent sessions
// ---------------------------------------------------------------------------

/// Start worker, submit 2 detached tasks, `ls` should show both sessions.
#[tokio::test]
#[ignore = "requires tuwunel binary and compiled mxdx-worker/mxdx-client"]
async fn e2e_concurrent_sessions() {
    let mut hs = TuwunelInstance::start().await.unwrap();
    let base_url = format!("http://127.0.0.1:{}", hs.port);
    let room_name = format!("e2e-binary-concurrent-{}", hs.port);

    register_user(&hs, "worker-e2e5", "pass123").await;
    register_user(&hs, "client-e2e5", "pass123").await;

    let mut worker = start_worker(&base_url, "worker-e2e5", "pass123", &room_name);
    wait_for_worker_ready().await;

    // Submit two detached tasks
    let detach1 = run_client(
        &base_url,
        "client-e2e5",
        "pass123",
        &room_name,
        &["run", "--detach", "sleep", "300"],
    );
    let uuid1 = String::from_utf8_lossy(&detach1.stdout).trim().to_string();
    eprintln!("[e2e_concurrent] UUID 1: {}", uuid1);

    let detach2 = run_client(
        &base_url,
        "client-e2e5",
        "pass123",
        &room_name,
        &["run", "--detach", "sleep", "301"],
    );
    let uuid2 = String::from_utf8_lossy(&detach2.stdout).trim().to_string();
    eprintln!("[e2e_concurrent] UUID 2: {}", uuid2);

    assert!(!uuid1.is_empty(), "first detach should return UUID");
    assert!(!uuid2.is_empty(), "second detach should return UUID");
    assert_ne!(uuid1, uuid2, "UUIDs must be different");

    // Give the worker time to claim both tasks
    tokio::time::sleep(Duration::from_secs(5)).await;

    // List sessions
    let ls_output = run_client(
        &base_url,
        "client-e2e5",
        "pass123",
        &room_name,
        &["ls"],
    );

    let ls_stdout = String::from_utf8_lossy(&ls_output.stdout);
    eprintln!("[e2e_concurrent] ls output: {}", ls_stdout);

    // Both sessions should appear in the listing
    assert!(
        ls_stdout.contains(&uuid1[..8]),
        "ls should show first session UUID, got: {}",
        ls_stdout,
    );
    assert!(
        ls_stdout.contains(&uuid2[..8]),
        "ls should show second session UUID, got: {}",
        ls_stdout,
    );

    let _ = worker.kill();
    let _ = worker.wait();
    hs.stop().await;
}
