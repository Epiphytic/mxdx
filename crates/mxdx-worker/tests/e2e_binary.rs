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

use mxdx_matrix::MatrixClient;
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

/// Start the mxdx-worker binary as a subprocess, using a direct room ID.
/// Returns the Child handle so the caller can kill it later.
fn start_worker_with_room_id(
    homeserver: &str,
    username: &str,
    password: &str,
    room_id: &str,
) -> Child {
    Command::new(cargo_bin("mxdx-worker"))
        .args([
            "start",
            "--homeserver", homeserver,
            "--username", username,
            "--password", password,
            "--room-id", room_id,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to start mxdx-worker binary")
}

/// Run an mxdx-client command synchronously with a direct room ID.
fn run_client_with_room_id(
    homeserver: &str,
    username: &str,
    password: &str,
    room_id: &str,
    subcommand_args: &[&str],
) -> Output {
    let mut full_args: Vec<&str> = vec![
        "--homeserver", homeserver,
        "--username", username,
        "--password", password,
        "--room-id", room_id,
    ];
    full_args.extend_from_slice(subcommand_args);
    Command::new(cargo_bin("mxdx-client"))
        .args(&full_args)
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

/// Create a shared encrypted room with both users at power level 100.
/// Returns the room ID as a string.
async fn create_shared_room(
    homeserver: &str,
    creator_user: &str,
    creator_pass: &str,
    invitee_user: &str,
    invitee_pass: &str,
) -> String {
    // Login as creator
    let creator = MatrixClient::login_and_connect(homeserver, creator_user, creator_pass)
        .await
        .expect("creator login failed");

    // Login as invitee to get their user_id
    let invitee = MatrixClient::login_and_connect(homeserver, invitee_user, invitee_pass)
        .await
        .expect("invitee login failed");

    let invitee_uid = invitee.user_id().to_owned();
    let creator_uid = creator.user_id().to_owned();

    // Create encrypted room with invitee, setting power levels so both users
    // can post state events (needed for session state tracking)
    let room_id = creator
        .create_encrypted_room(&[invitee_uid.clone()])
        .await
        .expect("room creation failed");

    // Set power levels: both users at 100, state_default at 0
    let power_levels = serde_json::json!({
        "users": {
            creator_uid.to_string(): 100,
            invitee_uid.to_string(): 100,
        },
        "state_default": 0,
        "events_default": 0,
    });
    creator
        .send_state_event(&room_id, "m.room.power_levels", "", power_levels)
        .await
        .expect("failed to set power levels");

    // Invitee joins the room
    invitee.sync_once().await.expect("invitee sync failed");
    invitee.join_room(&room_id).await.expect("invitee join failed");

    // Exchange keys so E2EE works
    for _ in 0..4 {
        creator.sync_once().await.ok();
        invitee.sync_once().await.ok();
    }

    room_id.to_string()
}

/// Give the worker binary time to start up and connect to Matrix.
async fn wait_for_worker_ready() {
    tokio::time::sleep(Duration::from_secs(5)).await;
}

// ---------------------------------------------------------------------------
// Test 1: Echo command lifecycle
// ---------------------------------------------------------------------------

/// Start tuwunel, register users, create shared room, start worker binary,
/// run client `run /bin/echo hello world`, assert stdout contains "hello world"
/// and exit code is 0, then kill the worker.
#[tokio::test]
#[ignore = "requires tuwunel binary and compiled mxdx-worker/mxdx-client"]
async fn e2e_echo_command_lifecycle() {
    let mut hs = TuwunelInstance::start().await.unwrap();
    let base_url = format!("http://127.0.0.1:{}", hs.port);

    // Register both users
    register_user(&hs, "worker-e2e", "pass123").await;
    register_user(&hs, "client-e2e", "pass123").await;

    // Create a shared encrypted room (client creates, invites worker)
    let room_id = create_shared_room(
        &base_url, "client-e2e", "pass123", "worker-e2e", "pass123",
    ).await;
    eprintln!("[e2e_echo] shared room: {}", room_id);

    // Start the worker binary with the shared room ID
    let mut worker = start_worker_with_room_id(&base_url, "worker-e2e", "pass123", &room_id);
    wait_for_worker_ready().await;

    // Run a command via the client binary
    let output = run_client_with_room_id(
        &base_url,
        "client-e2e",
        "pass123",
        &room_id,
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

    register_user(&hs, "worker-e2e2", "pass123").await;
    register_user(&hs, "client-e2e2", "pass123").await;

    let room_id = create_shared_room(
        &base_url, "client-e2e2", "pass123", "worker-e2e2", "pass123",
    ).await;

    let mut worker = start_worker_with_room_id(&base_url, "worker-e2e2", "pass123", &room_id);
    wait_for_worker_ready().await;

    let output = run_client_with_room_id(
        &base_url, "client-e2e2", "pass123", &room_id,
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

    register_user(&hs, "worker-e2e3", "pass123").await;
    register_user(&hs, "client-e2e3", "pass123").await;

    let room_id = create_shared_room(
        &base_url, "client-e2e3", "pass123", "worker-e2e3", "pass123",
    ).await;

    let mut worker = start_worker_with_room_id(&base_url, "worker-e2e3", "pass123", &room_id);
    wait_for_worker_ready().await;

    let detach_output = run_client_with_room_id(
        &base_url, "client-e2e3", "pass123", &room_id,
        &["run", "--detach", "sleep", "300"],
    );

    let uuid = String::from_utf8_lossy(&detach_output.stdout).trim().to_string();
    eprintln!("[e2e_ls] detached UUID: {}", uuid);
    assert!(!uuid.is_empty(), "detach mode should print a UUID, got empty stdout");

    tokio::time::sleep(Duration::from_secs(3)).await;

    let ls_output = run_client_with_room_id(
        &base_url, "client-e2e3", "pass123", &room_id,
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

    register_user(&hs, "worker-e2e4", "pass123").await;
    register_user(&hs, "client-e2e4", "pass123").await;

    let room_id = create_shared_room(
        &base_url, "client-e2e4", "pass123", "worker-e2e4", "pass123",
    ).await;

    let mut worker = start_worker_with_room_id(&base_url, "worker-e2e4", "pass123", &room_id);
    wait_for_worker_ready().await;

    let detach_output = run_client_with_room_id(
        &base_url, "client-e2e4", "pass123", &room_id,
        &["run", "--detach", "sleep", "300"],
    );

    let uuid = String::from_utf8_lossy(&detach_output.stdout).trim().to_string();
    eprintln!("[e2e_cancel] detached UUID: {}", uuid);
    assert!(!uuid.is_empty(), "should get a UUID from detach mode");

    tokio::time::sleep(Duration::from_secs(3)).await;

    let cancel_output = run_client_with_room_id(
        &base_url, "client-e2e4", "pass123", &room_id,
        &["cancel", &uuid],
    );

    let cancel_stderr = String::from_utf8_lossy(&cancel_output.stderr);
    eprintln!("[e2e_cancel] cancel stderr: {}", cancel_stderr);

    assert!(
        cancel_output.status.success(),
        "cancel command should succeed, got: {:?}",
        cancel_output.status.code(),
    );

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

    register_user(&hs, "worker-e2e5", "pass123").await;
    register_user(&hs, "client-e2e5", "pass123").await;

    let room_id = create_shared_room(
        &base_url, "client-e2e5", "pass123", "worker-e2e5", "pass123",
    ).await;

    let mut worker = start_worker_with_room_id(&base_url, "worker-e2e5", "pass123", &room_id);
    wait_for_worker_ready().await;

    let detach1 = run_client_with_room_id(
        &base_url, "client-e2e5", "pass123", &room_id,
        &["run", "--detach", "sleep", "300"],
    );
    let uuid1 = String::from_utf8_lossy(&detach1.stdout).trim().to_string();
    eprintln!("[e2e_concurrent] UUID 1: {}", uuid1);

    let detach2 = run_client_with_room_id(
        &base_url, "client-e2e5", "pass123", &room_id,
        &["run", "--detach", "sleep", "301"],
    );
    let uuid2 = String::from_utf8_lossy(&detach2.stdout).trim().to_string();
    eprintln!("[e2e_concurrent] UUID 2: {}", uuid2);

    assert!(!uuid1.is_empty(), "first detach should return UUID");
    assert!(!uuid2.is_empty(), "second detach should return UUID");
    assert_ne!(uuid1, uuid2, "UUIDs must be different");

    tokio::time::sleep(Duration::from_secs(5)).await;

    let ls_output = run_client_with_room_id(
        &base_url, "client-e2e5", "pass123", &room_id,
        &["ls"],
    );

    let ls_stdout = String::from_utf8_lossy(&ls_output.stdout);
    eprintln!("[e2e_concurrent] ls output: {}", ls_stdout);

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
