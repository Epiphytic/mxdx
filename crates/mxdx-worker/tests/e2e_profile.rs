//! Profiling and federated E2E tests.
//!
//! Three transport variants for each workload:
//!   - SSH localhost (baseline)
//!   - mxdx local (single Tuwunel, E2EE)
//!   - mxdx federated (two TLS Tuwunel instances, E2EE + federation)
//!
//! Run all:  `cargo test -p mxdx-worker --test e2e_profile -- --ignored --nocapture`
//! Run fast: `cargo test -p mxdx-worker --test e2e_profile echo -- --ignored --nocapture`

use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use mxdx_test_helpers::federation::FederatedPair;
use mxdx_test_helpers::tuwunel::TuwunelInstance;

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

fn start_worker(hs: &str, user: &str, pass: &str, room_id: &str) -> Child {
    Command::new(cargo_bin("mxdx-worker"))
        .args(["start", "--homeserver", hs, "--username", user, "--password", pass, "--room-id", room_id])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to start mxdx-worker")
}

fn run_client(hs: &str, user: &str, pass: &str, room_id: &str, args: &[&str]) -> Output {
    let mut full: Vec<&str> = vec!["--homeserver", hs, "--username", user, "--password", pass, "--room-id", room_id];
    full.extend_from_slice(args);
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
    // SSH concatenates all remote args with spaces, which breaks quoting.
    // Pass the script via stdin to avoid escaping issues.
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

async fn register(hs: &TuwunelInstance, user: &str, pass: &str) {
    hs.register_user(user, pass).await.unwrap_or_else(|e| panic!("register {user}: {e}"));
}

async fn shared_room(hs: &str, server_name: &str, creator: &str, pass: &str, invitee: &str) -> String {
    let http = reqwest::Client::builder().danger_accept_invalid_certs(true).build().unwrap();
    let login: serde_json::Value = http
        .post(format!("{hs}/_matrix/client/v3/login"))
        .json(&serde_json::json!({"type":"m.login.password","identifier":{"type":"m.id.user","user":creator},"password":pass}))
        .send().await.unwrap().json().await.unwrap();
    let token = login["access_token"].as_str().unwrap();
    let cu = format!("@{creator}:{server_name}");
    let iu = format!("@{invitee}:{server_name}");
    let resp: serde_json::Value = http
        .post(format!("{hs}/_matrix/client/v3/createRoom"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&serde_json::json!({
            "invite":[iu],
            "initial_state":[{"type":"m.room.encryption","state_key":"","content":{"algorithm":"m.megolm.v1.aes-sha2"}}],
            "power_level_content_override":{"users":{cu:100,iu:100},"state_default":0,"events_default":0}
        }))
        .send().await.unwrap().json().await.unwrap();
    resp["room_id"].as_str().unwrap().to_string()
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

/// Report a benchmark result line for the final table.
fn report(test: &str, transport: &str, elapsed: Duration, exit_code: Option<i32>, stdout_lines: usize) {
    eprintln!(
        "| {:<30} | {:<12} | {:>8.1}s | {:>4} | {:>8} |",
        test, transport, elapsed.as_secs_f64(),
        exit_code.map(|c| c.to_string()).unwrap_or("?".into()),
        stdout_lines,
    );
}

// ===========================================================================
// ECHO — simple command, measures session setup latency
// ===========================================================================

#[tokio::test]
#[ignore = "requires passwordless localhost SSH"]
async fn profile_echo_ssh() {
    let start = Instant::now();
    let out = run_ssh(&["/bin/echo", "hello", "world"]);
    let elapsed = start.elapsed();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("hello world"));
    report("echo", "ssh", elapsed, out.status.code(), stdout.lines().count());
}

#[tokio::test]
#[ignore = "requires tuwunel"]
async fn profile_echo_local() {
    let mut hs = TuwunelInstance::start().await.unwrap();
    let url = format!("http://127.0.0.1:{}", hs.port);
    register(&hs, "w-echo", "p").await;
    register(&hs, "c-echo", "p").await;
    let rid = shared_room(&url, &hs.server_name, "c-echo", "p", "w-echo").await;
    let mut w = start_worker(&url, "w-echo", "p", &rid);
    wait_ready().await;

    let start = Instant::now();
    let out = run_client(&url, "c-echo", "p", &rid, &["run", "/bin/echo", "hello", "world"]);
    let elapsed = start.elapsed();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stdout: {stdout}\nstderr: {stderr}");
    report("echo", "mxdx-local", elapsed, out.status.code(), stdout.lines().count());

    let _ = w.kill(); let _ = w.wait(); hs.stop().await;
}

#[tokio::test]
#[ignore = "requires tuwunel + openssl"]
async fn profile_echo_federated() {
    let mut pair = FederatedPair::start().await.unwrap();
    let url_a = format!("https://127.0.0.1:{}", pair.hs_a.port);
    let url_b = format!("https://127.0.0.1:{}", pair.hs_b.port);
    register(&pair.hs_a, "w-fecho", "p").await;
    register(&pair.hs_b, "c-fecho", "p").await;
    let rid = shared_room(&url_a, &pair.hs_a.server_name, "w-fecho", "p", "c-fecho").await;
    let mut w = start_worker(&url_a, "w-fecho", "p", &rid);
    wait_ready().await;

    let start = Instant::now();
    let out = run_client(&url_b, "c-fecho", "p", &rid, &["run", "/bin/echo", "hello", "world"]);
    let elapsed = start.elapsed();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stdout: {stdout}\nstderr: {stderr}");
    report("echo", "mxdx-federated", elapsed, out.status.code(), stdout.lines().count());

    let _ = w.kill(); let _ = w.wait(); pair.stop().await;
}

// ===========================================================================
// EXIT CODE — /bin/false, measures exit code propagation latency
// ===========================================================================

#[tokio::test]
#[ignore = "requires passwordless localhost SSH"]
async fn profile_exit_code_ssh() {
    let start = Instant::now();
    let out = run_ssh(&["/bin/false"]);
    let elapsed = start.elapsed();
    assert!(!out.status.success());
    report("exit-code(/bin/false)", "ssh", elapsed, out.status.code(), 0);
}

#[tokio::test]
#[ignore = "requires tuwunel"]
async fn profile_exit_code_local() {
    let mut hs = TuwunelInstance::start().await.unwrap();
    let url = format!("http://127.0.0.1:{}", hs.port);
    register(&hs, "w-exit", "p").await;
    register(&hs, "c-exit", "p").await;
    let rid = shared_room(&url, &hs.server_name, "c-exit", "p", "w-exit").await;
    let mut w = start_worker(&url, "w-exit", "p", &rid);
    wait_ready().await;

    let start = Instant::now();
    let out = run_client(&url, "c-exit", "p", &rid, &["run", "/bin/false"]);
    let elapsed = start.elapsed();
    assert!(!out.status.success());
    report("exit-code(/bin/false)", "mxdx-local", elapsed, out.status.code(), 0);

    let _ = w.kill(); let _ = w.wait(); hs.stop().await;
}

#[tokio::test]
#[ignore = "requires tuwunel + openssl"]
async fn profile_exit_code_federated() {
    let mut pair = FederatedPair::start().await.unwrap();
    let url_a = format!("https://127.0.0.1:{}", pair.hs_a.port);
    let url_b = format!("https://127.0.0.1:{}", pair.hs_b.port);
    register(&pair.hs_a, "w-fexit", "p").await;
    register(&pair.hs_b, "c-fexit", "p").await;
    let rid = shared_room(&url_a, &pair.hs_a.server_name, "w-fexit", "p", "c-fexit").await;
    let mut w = start_worker(&url_a, "w-fexit", "p", &rid);
    wait_ready().await;

    let start = Instant::now();
    let out = run_client(&url_b, "c-fexit", "p", &rid, &["run", "/bin/false"]);
    let elapsed = start.elapsed();
    assert!(!out.status.success());
    report("exit-code(/bin/false)", "mxdx-federated", elapsed, out.status.code(), 0);

    let _ = w.kill(); let _ = w.wait(); pair.stop().await;
}

// ===========================================================================
// MD5SUM — 10,000 lines, measures output throughput
// ===========================================================================

fn md5_script(file_path: &str) -> String {
    format!("while IFS= read -r line; do printf '%s\\n' \"$line\" | md5sum; done < '{file_path}'")
}

#[tokio::test]
#[ignore = "requires passwordless localhost SSH"]
async fn profile_md5sum_ssh() {
    let fp = large_file(10_000);
    let script = md5_script(&fp);

    let start = Instant::now();
    let out = run_ssh_script(&script);
    let elapsed = start.elapsed();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "ssh md5sum failed (exit {:?}): {stderr}", out.status.code());
    report("md5sum(10k lines)", "ssh", elapsed, out.status.code(), stdout.lines().count());

    let _ = std::fs::remove_file(&fp);
}

#[tokio::test]
#[ignore = "requires tuwunel"]
async fn profile_md5sum_local() {
    let mut hs = TuwunelInstance::start().await.unwrap();
    let url = format!("http://127.0.0.1:{}", hs.port);
    register(&hs, "w-md5", "p").await;
    register(&hs, "c-md5", "p").await;
    let rid = shared_room(&url, &hs.server_name, "c-md5", "p", "w-md5").await;
    let mut w = start_worker(&url, "w-md5", "p", &rid);
    wait_ready().await;

    let fp = large_file(10_000);
    let script = md5_script(&fp);

    let start = Instant::now();
    let out = run_client(&url, "c-md5", "p", &rid, &["run", "--", "/bin/sh", "-c", &script]);
    let elapsed = start.elapsed();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stderr: {stderr}");
    report("md5sum(10k lines)", "mxdx-local", elapsed, out.status.code(), stdout.lines().count());

    let _ = std::fs::remove_file(&fp);
    let _ = w.kill(); let _ = w.wait(); hs.stop().await;
}

#[tokio::test]
#[ignore = "requires tuwunel + openssl"]
async fn profile_md5sum_federated() {
    let mut pair = FederatedPair::start().await.unwrap();
    let url_a = format!("https://127.0.0.1:{}", pair.hs_a.port);
    let url_b = format!("https://127.0.0.1:{}", pair.hs_b.port);
    register(&pair.hs_a, "w-fmd5", "p").await;
    register(&pair.hs_b, "c-fmd5", "p").await;
    let rid = shared_room(&url_a, &pair.hs_a.server_name, "w-fmd5", "p", "c-fmd5").await;
    let mut w = start_worker(&url_a, "w-fmd5", "p", &rid);
    wait_ready().await;

    let fp = large_file(10_000);
    let script = md5_script(&fp);

    let start = Instant::now();
    let out = run_client(&url_b, "c-fmd5", "p", &rid, &["run", "--", "/bin/sh", "-c", &script]);
    let elapsed = start.elapsed();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stderr: {stderr}");
    report("md5sum(10k lines)", "mxdx-federated", elapsed, out.status.code(), stdout.lines().count());

    let _ = std::fs::remove_file(&fp);
    let _ = w.kill(); let _ = w.wait(); pair.stop().await;
}

// ===========================================================================
// PING — 30 pings (30s), measures streaming output latency
// ===========================================================================

#[tokio::test]
#[ignore = "requires passwordless localhost SSH + network"]
async fn profile_ping_ssh() {
    let start = Instant::now();
    let out = run_ssh(&["ping", "-c", "30", "-i", "1", "1.1.1.1"]);
    let elapsed = start.elapsed();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success());
    report("ping(30s)", "ssh", elapsed, out.status.code(), stdout.lines().count());
}

#[tokio::test]
#[ignore = "requires tuwunel + network"]
async fn profile_ping_local() {
    let mut hs = TuwunelInstance::start().await.unwrap();
    let url = format!("http://127.0.0.1:{}", hs.port);
    register(&hs, "w-ping", "p").await;
    register(&hs, "c-ping", "p").await;
    let rid = shared_room(&url, &hs.server_name, "c-ping", "p", "w-ping").await;
    let mut w = start_worker(&url, "w-ping", "p", &rid);
    wait_ready().await;

    let start = Instant::now();
    let out = run_client(&url, "c-ping", "p", &rid, &["run", "ping", "-c", "30", "-i", "1", "1.1.1.1"]);
    let elapsed = start.elapsed();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stderr: {stderr}");
    report("ping(30s)", "mxdx-local", elapsed, out.status.code(), stdout.lines().count());

    let _ = w.kill(); let _ = w.wait(); hs.stop().await;
}

#[tokio::test]
#[ignore = "requires tuwunel + openssl + network"]
async fn profile_ping_federated() {
    let mut pair = FederatedPair::start().await.unwrap();
    let url_a = format!("https://127.0.0.1:{}", pair.hs_a.port);
    let url_b = format!("https://127.0.0.1:{}", pair.hs_b.port);
    register(&pair.hs_a, "w-fping", "p").await;
    register(&pair.hs_b, "c-fping", "p").await;
    let rid = shared_room(&url_a, &pair.hs_a.server_name, "w-fping", "p", "c-fping").await;
    let mut w = start_worker(&url_a, "w-fping", "p", &rid);
    wait_ready().await;

    let start = Instant::now();
    let out = run_client(&url_b, "c-fping", "p", &rid, &["run", "ping", "-c", "30", "-i", "1", "1.1.1.1"]);
    let elapsed = start.elapsed();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stderr: {stderr}");
    report("ping(30s)", "mxdx-federated", elapsed, out.status.code(), stdout.lines().count());

    let _ = w.kill(); let _ = w.wait(); pair.stop().await;
}

// ===========================================================================
// LONG PING — 300 pings (5min), measures sustained streaming
// ===========================================================================

#[tokio::test]
#[ignore = "requires tuwunel + network, runs 5 minutes"]
async fn profile_long_ping_local() {
    let mut hs = TuwunelInstance::start().await.unwrap();
    let url = format!("http://127.0.0.1:{}", hs.port);
    register(&hs, "w-lping", "p").await;
    register(&hs, "c-lping", "p").await;
    let rid = shared_room(&url, &hs.server_name, "c-lping", "p", "w-lping").await;
    let mut w = start_worker(&url, "w-lping", "p", &rid);
    wait_ready().await;

    let start = Instant::now();
    let out = run_client(&url, "c-lping", "p", &rid, &["run", "ping", "-c", "300", "-i", "1", "1.1.1.1"]);
    let elapsed = start.elapsed();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stderr: {stderr}");
    report("ping(5min)", "mxdx-local", elapsed, out.status.code(), stdout.lines().count());

    let _ = w.kill(); let _ = w.wait(); hs.stop().await;
}

#[tokio::test]
#[ignore = "requires tuwunel + openssl + network, runs 5 minutes"]
async fn profile_long_ping_federated() {
    let mut pair = FederatedPair::start().await.unwrap();
    let url_a = format!("https://127.0.0.1:{}", pair.hs_a.port);
    let url_b = format!("https://127.0.0.1:{}", pair.hs_b.port);
    register(&pair.hs_a, "w-flping", "p").await;
    register(&pair.hs_b, "c-flping", "p").await;
    let rid = shared_room(&url_a, &pair.hs_a.server_name, "w-flping", "p", "c-flping").await;
    let mut w = start_worker(&url_a, "w-flping", "p", &rid);
    wait_ready().await;

    let start = Instant::now();
    let out = run_client(&url_b, "c-flping", "p", &rid, &["run", "ping", "-c", "300", "-i", "1", "1.1.1.1"]);
    let elapsed = start.elapsed();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stderr: {stderr}");
    report("ping(5min)", "mxdx-federated", elapsed, out.status.code(), stdout.lines().count());

    let _ = w.kill(); let _ = w.wait(); pair.stop().await;
}
