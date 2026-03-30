//! Profiling E2E tests that exercise the compiled binaries with long-running
//! and output-heavy workloads. Both local (single Tuwunel) and federated
//! (two TLS Tuwunel instances) variants.
//!
//! Run with: `cargo test -p mxdx-worker --test e2e_profile -- --ignored --nocapture`

use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use mxdx_test_helpers::federation::FederatedPair;
use mxdx_test_helpers::tuwunel::TuwunelInstance;

// ---------------------------------------------------------------------------
// Helpers (same pattern as e2e_binary.rs)
// ---------------------------------------------------------------------------

fn cargo_bin(name: &str) -> std::path::PathBuf {
    let mut path = std::env::current_exe().expect("cannot resolve test binary path");
    path.pop();
    path.pop();
    path.push(name);
    assert!(path.exists(), "Binary not found at {}", path.display());
    path
}

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
    let mut child = Command::new("timeout")
        .arg("330") // 5.5 min timeout (tests run up to 5 min)
        .arg(cargo_bin("mxdx-client"))
        .args(&full_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn mxdx-client binary");
    child.wait_with_output().expect("failed to wait for mxdx-client")
}

async fn register_user(tuwunel: &TuwunelInstance, username: &str, password: &str) {
    tuwunel
        .register_user(username, password)
        .await
        .unwrap_or_else(|e| panic!("failed to register user {}: {}", username, e));
}

async fn create_shared_room(
    homeserver: &str,
    server_name: &str,
    creator_user: &str,
    creator_pass: &str,
    invitee_user: &str,
    _invitee_pass: &str,
) -> String {
    let http = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .unwrap();
    let login_resp: serde_json::Value = http
        .post(format!("{homeserver}/_matrix/client/v3/login"))
        .json(&serde_json::json!({
            "type": "m.login.password",
            "identifier": {"type": "m.id.user", "user": creator_user},
            "password": creator_pass,
        }))
        .send().await.expect("login failed")
        .json().await.expect("login parse failed");
    let token = login_resp["access_token"].as_str().expect("no token");

    let creator_uid = format!("@{creator_user}:{server_name}");
    let invitee_uid = format!("@{invitee_user}:{server_name}");

    let create_resp: serde_json::Value = http
        .post(format!("{homeserver}/_matrix/client/v3/createRoom"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&serde_json::json!({
            "invite": [invitee_uid],
            "initial_state": [{
                "type": "m.room.encryption",
                "state_key": "",
                "content": {"algorithm": "m.megolm.v1.aes-sha2"}
            }],
            "power_level_content_override": {
                "users": { creator_uid: 100, invitee_uid: 100 },
                "state_default": 0,
                "events_default": 0,
            }
        }))
        .send().await.expect("create room failed")
        .json().await.expect("create room parse failed");

    create_resp["room_id"].as_str().expect("no room_id").to_string()
}

async fn wait_for_worker_ready() {
    tokio::time::sleep(Duration::from_secs(5)).await;
}

/// Generate a large file with sequential numbered lines for md5sum profiling.
fn generate_large_file(lines: usize) -> String {
    let path = format!("/tmp/mxdx-profile-{}.txt", std::process::id());
    let mut content = String::with_capacity(lines * 50);
    for i in 0..lines {
        content.push_str(&format!(
            "line {:06}: the quick brown fox jumps over the lazy dog {}\n",
            i,
            i * 7919 // prime multiplier for variety
        ));
    }
    std::fs::write(&path, &content).expect("failed to write test file");
    path
}

// ===========================================================================
// LOCAL (single Tuwunel) profiling tests
// ===========================================================================

// ---------------------------------------------------------------------------
// Test: Ping 1.1.1.1 every second for 5 minutes (local)
// ---------------------------------------------------------------------------

/// Profile: long-running ping with streaming output through Matrix events.
/// Measures total latency and verifies output capture over 5 minutes.
#[tokio::test]
#[ignore = "requires tuwunel, network access, runs 5 minutes"]
async fn profile_ping_local() {
    let mut hs = TuwunelInstance::start().await.unwrap();
    let base_url = format!("http://127.0.0.1:{}", hs.port);

    register_user(&hs, "worker-ping", "pass123").await;
    register_user(&hs, "client-ping", "pass123").await;

    let room_id = create_shared_room(
        &base_url, &hs.server_name, "client-ping", "pass123", "worker-ping", "pass123",
    ).await;

    let mut worker = start_worker_with_room_id(&base_url, "worker-ping", "pass123", &room_id);
    wait_for_worker_ready().await;

    let start = Instant::now();

    // ping -c 300 -i 1 = 300 pings, 1 per second, ~5 minutes
    let output = run_client_with_room_id(
        &base_url, "client-ping", "pass123", &room_id,
        &["run", "ping", "-c", "300", "-i", "1", "1.1.1.1"],
    );

    let elapsed = start.elapsed();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    eprintln!("=== PROFILE: ping local ===");
    eprintln!("Duration: {:.1}s", elapsed.as_secs_f64());
    eprintln!("Exit code: {:?}", output.status.code());
    eprintln!("Stdout lines: {}", stdout.lines().count());
    eprintln!("Stderr (last 200): {}", &stderr[stderr.len().saturating_sub(200)..]);

    assert!(
        output.status.success(),
        "ping should succeed, got exit {:?}",
        output.status.code(),
    );

    let _ = worker.kill();
    let _ = worker.wait();
    hs.stop().await;
}

// ---------------------------------------------------------------------------
// Test: md5sum lines of a large file (local)
// ---------------------------------------------------------------------------

/// Profile: pipe 10000+ lines through md5sum. Tests output-heavy workload
/// throughput through Matrix events.
#[tokio::test]
#[ignore = "requires tuwunel, runs ~30s"]
async fn profile_md5sum_local() {
    let mut hs = TuwunelInstance::start().await.unwrap();
    let base_url = format!("http://127.0.0.1:{}", hs.port);

    register_user(&hs, "worker-md5", "pass123").await;
    register_user(&hs, "client-md5", "pass123").await;

    let room_id = create_shared_room(
        &base_url, &hs.server_name, "client-md5", "pass123", "worker-md5", "pass123",
    ).await;

    let mut worker = start_worker_with_room_id(&base_url, "worker-md5", "pass123", &room_id);
    wait_for_worker_ready().await;

    // Generate a 10000-line file
    let file_path = generate_large_file(10_000);

    let start = Instant::now();

    // Run: while read line; do echo "$line" | md5sum; done < file
    // This produces 10000 lines of md5 hashes — heavy output through Matrix
    let script = format!(
        "while IFS= read -r line; do printf '%s\\n' \"$line\" | md5sum; done < '{}'",
        file_path,
    );
    let output = run_client_with_room_id(
        &base_url, "client-md5", "pass123", &room_id,
        &["run", "--", "/bin/sh", "-c", &script],
    );

    let elapsed = start.elapsed();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    eprintln!("=== PROFILE: md5sum local ===");
    eprintln!("Duration: {:.1}s", elapsed.as_secs_f64());
    eprintln!("Exit code: {:?}", output.status.code());
    eprintln!("Stdout lines: {}", stdout.lines().count());
    eprintln!("Stderr (last 200): {}", &stderr[stderr.len().saturating_sub(200)..]);

    assert!(
        output.status.success(),
        "md5sum pipeline should succeed, got exit {:?}",
        output.status.code(),
    );

    // Cleanup
    let _ = std::fs::remove_file(&file_path);
    let _ = worker.kill();
    let _ = worker.wait();
    hs.stop().await;
}

// ===========================================================================
// FEDERATED (two TLS Tuwunel instances) profiling tests
// ===========================================================================

// ---------------------------------------------------------------------------
// Test: Ping 1.1.1.1 every second for 5 minutes (federated)
// ---------------------------------------------------------------------------

/// Profile: same as local ping but worker on hs_a and client on hs_b.
/// Measures federation overhead on streaming output.
#[tokio::test]
#[ignore = "requires tuwunel, openssl, network access, runs 5 minutes"]
async fn profile_ping_federated() {
    let mut pair = FederatedPair::start().await.unwrap();
    let hs_a_url = format!("https://127.0.0.1:{}", pair.hs_a.port);
    let hs_b_url = format!("https://127.0.0.1:{}", pair.hs_b.port);

    register_user(&pair.hs_a, "worker-fping", "pass123").await;
    register_user(&pair.hs_b, "client-fping", "pass123").await;

    // Worker creates room on hs_a, invites client on hs_b
    let room_id = create_shared_room(
        &hs_a_url, &pair.hs_a.server_name,
        "worker-fping", "pass123", "client-fping", "pass123",
    ).await;

    let mut worker = start_worker_with_room_id(
        &hs_a_url, "worker-fping", "pass123", &room_id,
    );
    wait_for_worker_ready().await;

    let start = Instant::now();

    let output = run_client_with_room_id(
        &hs_b_url, "client-fping", "pass123", &room_id,
        &["run", "ping", "-c", "300", "-i", "1", "1.1.1.1"],
    );

    let elapsed = start.elapsed();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    eprintln!("=== PROFILE: ping federated ===");
    eprintln!("Duration: {:.1}s", elapsed.as_secs_f64());
    eprintln!("Exit code: {:?}", output.status.code());
    eprintln!("Stdout lines: {}", stdout.lines().count());
    eprintln!("Stderr (last 200): {}", &stderr[stderr.len().saturating_sub(200)..]);

    assert!(
        output.status.success(),
        "federated ping should succeed, got exit {:?}",
        output.status.code(),
    );

    let _ = worker.kill();
    let _ = worker.wait();
    pair.stop().await;
}

// ---------------------------------------------------------------------------
// Test: md5sum lines of a large file (federated)
// ---------------------------------------------------------------------------

/// Profile: same as local md5sum but worker on hs_a, client on hs_b.
/// Measures federation overhead on output-heavy workloads.
#[tokio::test]
#[ignore = "requires tuwunel, openssl, runs ~60s"]
async fn profile_md5sum_federated() {
    let mut pair = FederatedPair::start().await.unwrap();
    let hs_a_url = format!("https://127.0.0.1:{}", pair.hs_a.port);
    let hs_b_url = format!("https://127.0.0.1:{}", pair.hs_b.port);

    register_user(&pair.hs_a, "worker-fmd5", "pass123").await;
    register_user(&pair.hs_b, "client-fmd5", "pass123").await;

    let room_id = create_shared_room(
        &hs_a_url, &pair.hs_a.server_name,
        "worker-fmd5", "pass123", "client-fmd5", "pass123",
    ).await;

    let mut worker = start_worker_with_room_id(
        &hs_a_url, "worker-fmd5", "pass123", &room_id,
    );
    wait_for_worker_ready().await;

    let file_path = generate_large_file(10_000);

    let start = Instant::now();

    let script = format!(
        "while IFS= read -r line; do printf '%s\\n' \"$line\" | md5sum; done < '{}'",
        file_path,
    );
    let output = run_client_with_room_id(
        &hs_b_url, "client-fmd5", "pass123", &room_id,
        &["run", "--", "/bin/sh", "-c", &script],
    );

    let elapsed = start.elapsed();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    eprintln!("=== PROFILE: md5sum federated ===");
    eprintln!("Duration: {:.1}s", elapsed.as_secs_f64());
    eprintln!("Exit code: {:?}", output.status.code());
    eprintln!("Stdout lines: {}", stdout.lines().count());
    eprintln!("Stderr (last 200): {}", &stderr[stderr.len().saturating_sub(200)..]);

    assert!(
        output.status.success(),
        "federated md5sum should succeed, got exit {:?}",
        output.status.code(),
    );

    let _ = std::fs::remove_file(&file_path);
    let _ = worker.kill();
    let _ = worker.wait();
    pair.stop().await;
}
