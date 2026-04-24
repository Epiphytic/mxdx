//! Phased E2E test suite — single orchestrator calls phase functions in order.
//!
//! ## Architecture
//!
//! A single `#[tokio::test] async fn e2e()` orchestrator calls phase functions
//! in order. Each phase is an `async fn phase_N(...)` that returns `Err` on any
//! test failure, stopping the suite immediately.
//!
//! ## Phases
//!
//! | Phase | Name            | Required | Execution | Description |
//! |-------|-----------------|----------|-----------|-------------|
//! | 0     | Security gates  | no       | serial    | Client refuses unsafe ops |
//! | 1     | Setup worker    | **yes**  | —         | Start persistent worker, authorize clients |
//! | 2     | Local tests     | no       | serial    | echo, exit-code, md5sum, ping(30s) via daemon |
//! | 3     | Federated tests | no       | serial    | Same tests, client on s2 via --no-daemon |
//! | 4     | Long + SSH      | no       | parallel  | 5-min pings + SSH baselines |
//! | 5     | Shutdown worker | **yes**  | —         | Graceful SIGTERM |
//! | 6     | Special tests   | no       | serial    | Session restore, backup, self-heal, diagnose |
//! | 7     | Cleanup         | **yes**  | —         | pkill safety net |
//!
//! ## Presets (E2E_PRESET env var)
//!
//! | Preset    | Phases            | Use case |
//! |-----------|-------------------|----------|
//! | `quick`   | 0, 1, 2, 3, 5, 7 | Fast feedback, ~2 minutes |
//! | `default` | 0, 1, 2, 3, 5, 6, 7 | Standard dev/CI, ~5 minutes |
//! | `full`    | 0, 1, 2, 3, 4, 5, 6, 7 | Everything including long tests |
//!
//! ## Running
//!
//! ```sh
//! cargo test -p mxdx-worker --test e2e_profile -- --ignored e2e --nocapture
//! E2E_PRESET=quick cargo test -p mxdx-worker --test e2e_profile -- --ignored e2e --nocapture
//! E2E_PRESET=full cargo test -p mxdx-worker --test e2e_profile -- --ignored e2e --nocapture
//! ```

use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{bail, Context as _, Result};

// ---------------------------------------------------------------------------
// Test context — owned by orchestrator, passed to phase functions
// ---------------------------------------------------------------------------

struct TestContext {
    worker: Child,
    worker_room: String,
    creds: TestCreds,
    store_dir: PathBuf,
    keychain_dir: PathBuf,
    config_home: PathBuf,
    /// Config home for the federated (s2) daemon. None if no s2 server.
    config_home_s2: Option<PathBuf>,
}

// ---------------------------------------------------------------------------
// Credential loading (from test-credentials.toml)
// ---------------------------------------------------------------------------

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

/// Path of the current worker log.
fn worker_log_path() -> String {
    std::env::var("MXDX_TEST_WORKER_LOG_FILE")
        .unwrap_or_else(|_| "/tmp/mxdx-worker-current.log".to_string())
}

/// Path of the current client daemon log for the e2e-local profile.
/// Matches the default in connect_or_spawn: ~/.mxdx/logs/{profile}.log
fn client_log_path() -> String {
    let base = dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
        .join(".mxdx")
        .join("logs");
    let _ = std::fs::create_dir_all(&base);
    base.join("e2e-local.log").to_string_lossy().to_string()
}

/// Truncate the daemon log at the start of the test run so each run starts clean.
fn setup_client_log() {
    let path = client_log_path();
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path);
    eprintln!("[e2e] client daemon log -> {path}");
}

/// Open worker log file for stdio redirection. Without file redirection the
/// `Stdio::piped()` pipe buffer (~64KB) fills up during backup/reencrypt/debug
/// output and the worker blocks on its next write, hanging every test.
fn worker_log_files() -> (std::fs::File, std::fs::File) {
    let path = worker_log_path();
    eprintln!("[e2e] worker log -> {path}");
    let f = std::fs::OpenOptions::new()
        .create(true).write(true).truncate(true).open(&path)
        .unwrap_or_else(|e| panic!("open worker log {path}: {e}"));
    let f2 = f.try_clone().expect("clone worker log fd");
    (f, f2)
}

/// Read the current worker log file as a String (best-effort).
fn worker_log_contents() -> String {
    std::fs::read_to_string(worker_log_path()).unwrap_or_default()
}

fn cargo_bin(name: &str) -> PathBuf {
    if let Ok(dir) = std::env::var("MXDX_BIN_DIR") {
        let path = PathBuf::from(dir).join(name);
        assert!(path.exists(), "Binary not found at {} (via MXDX_BIN_DIR)", path.display());
        return path;
    }
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
/// Write mxdx config files (defaults.toml + client.toml) into a config HOME dir.
fn write_test_config(config_home: &std::path::Path, creds: &TestCreds, worker_room: &str) {
    let mxdx_dir = config_home.join(".mxdx");
    std::fs::create_dir_all(&mxdx_dir).expect("failed to create .mxdx config dir");

    let localpart = creds.client_user
        .split(':')
        .next()
        .unwrap_or(&creds.client_user)
        .trim_start_matches('@');
    let defaults_toml = format!(
        r#"[[accounts]]
user_id = "@{localpart}:{server}"
homeserver = "{homeserver}"
password = "{password}"
"#,
        localpart = localpart,
        server = creds.server_url.trim_start_matches("https://").trim_start_matches("http://"),
        homeserver = creds.server_url,
        password = creds.client_pass,
    );
    std::fs::write(mxdx_dir.join("defaults.toml"), &defaults_toml)
        .expect("failed to write defaults.toml");

    let client_toml = format!(
        r#"default_worker_room = "{worker_room}"

[daemon]
idle_timeout_seconds = 300
"#,
        worker_room = worker_room,
    );
    std::fs::write(mxdx_dir.join("client.toml"), &client_toml)
        .expect("failed to write client.toml");
}

/// Write mxdx config for a specific server (for the federated daemon).
fn write_test_config_for_server(
    config_home: &std::path::Path,
    server_url: &str,
    client_user: &str,
    client_pass: &str,
    worker_room: &str,
) {
    let mxdx_dir = config_home.join(".mxdx");
    std::fs::create_dir_all(&mxdx_dir).expect("failed to create .mxdx config dir");

    let localpart = client_user
        .split(':')
        .next()
        .unwrap_or(client_user)
        .trim_start_matches('@');
    let defaults_toml = format!(
        r#"[[accounts]]
user_id = "@{localpart}:{server}"
homeserver = "{homeserver}"
password = "{password}"
"#,
        localpart = localpart,
        server = server_url.trim_start_matches("https://").trim_start_matches("http://"),
        homeserver = server_url,
        password = client_pass,
    );
    std::fs::write(mxdx_dir.join("defaults.toml"), &defaults_toml)
        .expect("failed to write defaults.toml");

    let client_toml = format!(
        r#"default_worker_room = "{worker_room}"

[daemon]
idle_timeout_seconds = 300
"#,
        worker_room = worker_room,
    );
    std::fs::write(mxdx_dir.join("client.toml"), &client_toml)
        .expect("failed to write client.toml");
}

/// Log path for the federated (s2) daemon.
fn client_log_path_s2() -> String {
    let base = dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
        .join(".mxdx")
        .join("logs");
    let _ = std::fs::create_dir_all(&base);
    base.join("e2e-s2.log").to_string_lossy().to_string()
}

/// Run the client via the federated (s2) daemon using its own config home.
fn run_client_daemon_s2(config_home_s2: &std::path::Path, extra_args: &[&str],
                        store_dir: &std::path::Path, keychain_dir: &std::path::Path,
                        timeout_secs: u32) -> Output {
    let mut full: Vec<&str> = Vec::new();
    if !extra_args.is_empty() {
        full.push(extra_args[0]);
        if extra_args[0] == "run" || extra_args[0] == "exec" {
            full.extend_from_slice(&["--cwd", "/tmp"]);
        }
        full.extend_from_slice(&extra_args[1..]);
    }
    Command::new("timeout")
        .arg(timeout_secs.to_string())
        .arg(cargo_bin("mxdx-client"))
        .args(&full)
        .env("HOME", config_home_s2.to_str().unwrap())
        .env("MXDX_STORE_DIR", store_dir.to_str().unwrap())
        .env("MXDX_KEYCHAIN_DIR", keychain_dir.to_str().unwrap())
        .env("MXDX_KEEP_PASSWORDS", "1")
        .env("MXDX_CLIENT_LOG_FILE", client_log_path_s2())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn mxdx-client (s2)")
        .wait_with_output()
        .expect("failed to wait for mxdx-client (s2)")
}

/// Wait for the federated daemon to be fully ready.
async fn wait_daemon_ready_s2(timeout: Duration) -> Result<()> {
    let log = client_log_path_s2();
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if tokio::time::Instant::now() >= deadline {
            bail!("s2 daemon did not become ready within {}s (no MXDX_DAEMON_READY in {})", timeout.as_secs(), log);
        }
        if let Ok(contents) = std::fs::read_to_string(&log) {
            if contents.contains("MXDX_DAEMON_READY") {
                return Ok(());
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// Get persistent store directories for shared state tests.
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

/// Persistent test dirs with a label suffix.
fn persistent_test_dirs_named(label: &str) -> (PathBuf, PathBuf) {
    let base = dirs::home_dir()
        .expect("cannot resolve home dir")
        .join(".mxdx")
        .join(format!("e2e-{label}"));
    let store = base.join("store");
    let keychain = base.join("keychain");
    std::fs::create_dir_all(&store).expect("failed to create persistent store dir");
    std::fs::create_dir_all(&keychain).expect("failed to create persistent keychain dir");
    (store, keychain)
}

/// Persistent config directory for a test. Survives across runs.
fn persistent_test_config_dir(label: &str) -> PathBuf {
    let dir = dirs::home_dir()
        .expect("cannot resolve home dir")
        .join(".mxdx")
        .join(format!("e2e-{label}"))
        .join("home");
    std::fs::create_dir_all(&dir).expect("failed to create persistent config dir");
    dir
}

/// Count devices for a Matrix user via REST.
async fn rest_device_count(server_url: &str, user: &str, pass: &str) -> Result<usize> {
    let token = rest_login_token(server_url, user, pass).await?;
    let url = format!("{}/_matrix/client/v3/devices", server_url.trim_end_matches('/'));
    let resp = reqwest::Client::new()
        .get(&url).bearer_auth(&token).send().await?;
    let v: serde_json::Value = resp.json().await?;
    Ok(v.get("devices").and_then(|d| d.as_array()).map(|a| a.len()).unwrap_or(0))
}

/// Count joined rooms for a Matrix user via REST.
async fn rest_room_count(server_url: &str, user: &str, pass: &str) -> Result<usize> {
    let token = rest_login_token(server_url, user, pass).await?;
    let url = format!("{}/_matrix/client/v3/joined_rooms", server_url.trim_end_matches('/'));
    let resp = reqwest::Client::new()
        .get(&url).bearer_auth(&token).send().await?;
    let v: serde_json::Value = resp.json().await?;
    Ok(v.get("joined_rooms").and_then(|r| r.as_array()).map(|a| a.len()).unwrap_or(0))
}

/// Kill stale mxdx processes from previous test runs.
fn cleanup_stale_processes() {
    eprintln!("[e2e] cleaning up stale mxdx processes from previous runs");
    let _ = std::process::Command::new("pkill")
        .args(["-KILL", "-f", "mxdx-worker start"])
        .status();
    let _ = std::process::Command::new("pkill")
        .args(["-KILL", "-f", "mxdx-client _daemon"])
        .status();
    if let Some(home) = dirs::home_dir() {
        let dirs_to_clean = [
            home.join(".mxdx").join("daemon"),
            home.join(".mxdx").join("e2e-local").join("home").join(".mxdx").join("daemon"),
        ];
        for d in &dirs_to_clean {
            if let Ok(entries) = std::fs::read_dir(d) {
                for e in entries.flatten() {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
    }
    std::thread::sleep(Duration::from_millis(500));
}

/// Graceful SIGTERM shutdown with 10s timeout, fallback to SIGKILL.
#[cfg(unix)]
fn kill_worker_graceful(w: &mut Child) {
    let pid = w.id() as i32;
    unsafe { libc::kill(pid, libc::SIGTERM); }
    for _ in 0..100 {
        match w.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(_) => break,
        }
    }
    let _ = w.kill();
    let _ = w.wait();
}

#[cfg(not(unix))]
fn kill_worker_graceful(w: &mut Child) {
    let _ = w.kill();
    let _ = w.wait();
}

fn spawn_child(mut cmd: Command) -> Child {
    cmd.spawn().expect("failed to spawn child")
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
    let (out, err) = worker_log_files();
    let mut cmd = Command::new(cargo_bin("mxdx-worker"));
    cmd.args(&args)
        .env("MXDX_STORE_DIR", store_dir.to_str().unwrap())
        .env("MXDX_KEYCHAIN_DIR", keychain_dir.to_str().unwrap())
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err));
    spawn_child(cmd)
}

/// Write a worker.toml with a short telemetry refresh so the state room lock
/// TTL is minimal (avoids blocking other workers after SIGKILL).
fn write_short_telemetry_config(label: &str) -> PathBuf {
    let config_dir = persistent_test_config_dir(label);
    let mxdx_dir = config_dir.join(".mxdx");
    std::fs::create_dir_all(&mxdx_dir).expect("create .mxdx config dir");
    std::fs::write(mxdx_dir.join("worker.toml"), "telemetry_refresh_seconds = 2\n")
        .expect("write worker.toml");
    config_dir
}

/// Start the worker with an explicit `--room-name` override.
fn start_worker_with_room(hs: &str, user: &str, pass: &str, room: &str, authorized_user: &str,
                          store_dir: &std::path::Path, keychain_dir: &std::path::Path) -> Child {
    start_worker_with_room_home(hs, user, pass, room, authorized_user, store_dir, keychain_dir, None)
}

fn start_worker_with_room_home(hs: &str, user: &str, pass: &str, room: &str, authorized_user: &str,
                          store_dir: &std::path::Path, keychain_dir: &std::path::Path,
                          home_dir: Option<&std::path::Path>) -> Child {
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
    let (out, err) = worker_log_files();
    let mut cmd = Command::new(cargo_bin("mxdx-worker"));
    cmd.args(&args)
        .env("MXDX_STORE_DIR", store_dir.to_str().unwrap())
        .env("MXDX_KEYCHAIN_DIR", keychain_dir.to_str().unwrap());
    if let Some(h) = home_dir {
        cmd.env("HOME", h.to_str().unwrap());
    }
    cmd.stdout(Stdio::from(out))
        .stderr(Stdio::from(err));
    spawn_child(cmd)
}

/// Start the worker with specific allowed commands, optional room name, and optional HOME.
fn start_worker_with_room_and_commands_home(
    hs: &str, user: &str, pass: &str, room: Option<&str>, authorized_user: &str,
    store_dir: &std::path::Path, keychain_dir: &std::path::Path,
    allowed_commands: &[&str], home_dir: Option<&std::path::Path>,
) -> Child {
    let mut args = vec![
        "start".to_string(),
        "--homeserver".to_string(), hs.to_string(),
        "--username".to_string(), user.to_string(),
        "--password".to_string(), pass.to_string(),
        "--authorized-user".to_string(), authorized_user.to_string(),
    ];
    if let Some(r) = room {
        args.push("--room-name".to_string());
        args.push(r.to_string());
    }
    for cmd in allowed_commands {
        args.push("--allowed-command".to_string());
        args.push(cmd.to_string());
    }
    let (out, err) = worker_log_files();
    let mut cmd = Command::new(cargo_bin("mxdx-worker"));
    cmd.args(&args)
        .env("MXDX_STORE_DIR", store_dir.to_str().unwrap())
        .env("MXDX_KEYCHAIN_DIR", keychain_dir.to_str().unwrap());
    if let Some(h) = home_dir {
        cmd.env("HOME", h.to_str().unwrap());
    }
    cmd.stdout(Stdio::from(out))
        .stderr(Stdio::from(err));
    spawn_child(cmd)
}

/// Run the client WITH liveness check (for testing exit codes 10, 11, 12).
fn run_client_with_liveness(hs: &str, user: &str, pass: &str, worker_room: &str, extra_args: &[&str],
                            store_dir: &std::path::Path, keychain_dir: &std::path::Path) -> Output {
    let mut full: Vec<&str> = vec![
        "--homeserver", hs, "--username", user, "--password", pass,
        "--no-daemon",
    ];
    if !extra_args.is_empty() {
        full.push(extra_args[0]);
        full.extend_from_slice(&["--worker-room", worker_room]);
        if extra_args[0] == "run" || extra_args[0] == "exec" {
            full.extend_from_slice(&["--cwd", "/tmp"]);
        }
        full.extend_from_slice(&extra_args[1..]);
    }
    Command::new("timeout")
        .arg("60")
        .arg(cargo_bin("mxdx-client"))
        .args(&full)
        .env("MXDX_STORE_DIR", store_dir.to_str().unwrap())
        .env("MXDX_KEYCHAIN_DIR", keychain_dir.to_str().unwrap())
        .env("MXDX_KEEP_PASSWORDS", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn mxdx-client")
        .wait_with_output()
        .expect("failed to wait for mxdx-client")
}

/// Run the client via daemon mode using config files.
/// `timeout_secs`: hard ceiling for the command (default 330s for long tests,
/// use 10 for echo/exit-code to catch session reuse failures).
fn run_client_daemon(config_home: &std::path::Path, extra_args: &[&str],
                     store_dir: &std::path::Path, keychain_dir: &std::path::Path,
                     timeout_secs: u32) -> Output {
    let mut full: Vec<&str> = Vec::new();
    if !extra_args.is_empty() {
        full.push(extra_args[0]);
        if extra_args[0] == "run" || extra_args[0] == "exec" {
            full.extend_from_slice(&["--cwd", "/tmp"]);
        }
        full.extend_from_slice(&extra_args[1..]);
    }
    Command::new("timeout")
        .arg(timeout_secs.to_string())
        .arg(cargo_bin("mxdx-client"))
        .args(&full)
        .env("HOME", config_home.to_str().unwrap())
        .env("MXDX_STORE_DIR", store_dir.to_str().unwrap())
        .env("MXDX_KEYCHAIN_DIR", keychain_dir.to_str().unwrap())
        .env("MXDX_KEEP_PASSWORDS", "1")
        .env("MXDX_CLIENT_LOG_FILE", client_log_path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn mxdx-client")
        .wait_with_output()
        .expect("failed to wait for mxdx-client")
}

/// Run the client in direct mode (--no-daemon) with CLI credentials.
fn run_client(hs: &str, user: &str, pass: &str, worker_room: &str, extra_args: &[&str],
              store_dir: &std::path::Path, keychain_dir: &std::path::Path,
              timeout_secs: u32) -> Output {
    let mut full: Vec<&str> = vec![
        "--homeserver", hs, "--username", user, "--password", pass,
        "--no-daemon",
    ];
    if !extra_args.is_empty() {
        full.push(extra_args[0]);
        full.extend_from_slice(&["--worker-room", worker_room]);
        if extra_args[0] == "run" || extra_args[0] == "exec" {
            full.extend_from_slice(&["--cwd", "/tmp"]);
        }
        full.extend_from_slice(&extra_args[1..]);
    }
    Command::new("timeout")
        .arg(timeout_secs.to_string())
        .arg(cargo_bin("mxdx-client"))
        .args(&full)
        .env("MXDX_STORE_DIR", store_dir.to_str().unwrap())
        .env("MXDX_KEYCHAIN_DIR", keychain_dir.to_str().unwrap())
        .env("MXDX_KEEP_PASSWORDS", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn mxdx-client")
        .wait_with_output()
        .expect("failed to wait for mxdx-client")
}

/// Check if an Output was killed by `timeout(1)` (exit code 124).
fn was_timeout(out: &Output) -> bool {
    out.status.code() == Some(124)
}

/// Format a test failure with timeout-aware messaging.
/// `session_reuse_hint`: if true, a timeout adds a note about session reuse.
fn format_test_failure(test_id: &str, label: &str, out: &Output, timeout_secs: u32, session_reuse_hint: bool) -> String {
    let stderr = String::from_utf8_lossy(&out.stderr);
    let tail = &stderr[stderr.len().saturating_sub(500)..];
    if was_timeout(out) {
        let mut msg = format!(
            "{test_id} {label}: TIMEOUT after {timeout_secs}s (exit code 124)"
        );
        if session_reuse_hint {
            msg.push_str(
                "\n  NOTE: This indicates the client is NOT reusing sessions correctly. \
                 Simple commands should complete in under 10 seconds when the daemon \
                 reuses its existing Matrix session."
            );
        }
        msg.push_str(&format!("\n  stderr (last 500): {tail}"));
        msg
    } else {
        format!("{test_id} {label} failed (exit code {:?}): {tail}", out.status.code())
    }
}

/// Like [`run_client`] but fail-fasts if no stdout within `no_output_timeout`.
fn run_client_no_output_timeout(
    hs: &str,
    user: &str,
    pass: &str,
    worker_room: &str,
    extra_args: &[&str],
    store_dir: &std::path::Path,
    keychain_dir: &std::path::Path,
    no_output_timeout: Duration,
    total_timeout: Duration,
) -> Output {
    use std::io::Read;
    use std::sync::Arc;

    let mut full: Vec<&str> = vec![
        "--homeserver", hs, "--username", user, "--password", pass,
        "--no-daemon",
    ];
    if !extra_args.is_empty() {
        full.push(extra_args[0]);
        full.extend_from_slice(&["--worker-room", worker_room]);
        if extra_args[0] == "run" || extra_args[0] == "exec" {
            full.extend_from_slice(&["--cwd", "/tmp"]);
        }
        full.extend_from_slice(&extra_args[1..]);
    }

    let mut child = Command::new(cargo_bin("mxdx-client"))
        .args(&full)
        .env("MXDX_STORE_DIR", store_dir.to_str().unwrap())
        .env("MXDX_KEYCHAIN_DIR", keychain_dir.to_str().unwrap())
        .env("MXDX_KEEP_PASSWORDS", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn mxdx-client");

    let stdout_pipe = child.stdout.take().expect("stdout piped");
    let stderr_pipe = child.stderr.take().expect("stderr piped");

    let stdout_buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let stderr_buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let got_stdout = Arc::new(AtomicBool::new(false));

    let sb = Arc::clone(&stdout_buf);
    let gs = Arc::clone(&got_stdout);
    let stdout_thread = std::thread::spawn(move || {
        let mut r = stdout_pipe;
        let mut buf = [0u8; 4096];
        loop {
            match r.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    gs.store(true, Ordering::SeqCst);
                    sb.lock().unwrap().extend_from_slice(&buf[..n]);
                }
                Err(_) => break,
            }
        }
    });

    let eb = Arc::clone(&stderr_buf);
    let stderr_thread = std::thread::spawn(move || {
        let mut r = stderr_pipe;
        let mut buf = [0u8; 4096];
        loop {
            match r.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => eb.lock().unwrap().extend_from_slice(&buf[..n]),
                Err(_) => break,
            }
        }
    });

    let start = Instant::now();
    let mut kill_reason: Option<String> = None;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {
                let elapsed = start.elapsed();
                if !got_stdout.load(Ordering::SeqCst) && elapsed >= no_output_timeout {
                    kill_reason = Some(format!(
                        "no stdout within {:?} — worker never relayed output",
                        no_output_timeout
                    ));
                    let _ = child.kill();
                    break child.wait().expect("wait after kill");
                }
                if elapsed >= total_timeout {
                    kill_reason =
                        Some(format!("total timeout {:?} reached", total_timeout));
                    let _ = child.kill();
                    break child.wait().expect("wait after kill");
                }
                std::thread::sleep(Duration::from_millis(500));
            }
            Err(e) => panic!("try_wait: {e}"),
        }
    };

    let _ = stdout_thread.join();
    let _ = stderr_thread.join();

    let stdout_vec = stdout_buf.lock().unwrap().clone();
    let mut stderr_vec = stderr_buf.lock().unwrap().clone();
    if let Some(reason) = kill_reason {
        stderr_vec.extend_from_slice(
            format!("\n[e2e fast-fail] killed mxdx-client: {}\n", reason).as_bytes(),
        );
    }

    Output { status, stdout: stdout_vec, stderr: stderr_vec }
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


/// Wait for the worker to be fully ready by polling its log file for the
/// readiness marker. Only looks at log content after `start_offset` bytes,
/// so multiple workers writing to the same log don't cause false positives.
/// Returns when the worker has synced, shared keys, and posted telemetry.
async fn wait_worker_ready(timeout: Duration, start_offset: u64) -> Result<()> {
    let log = worker_log_path();
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if tokio::time::Instant::now() >= deadline {
            bail!("worker did not become ready within {}s (no MXDX_WORKER_READY in {})", timeout.as_secs(), log);
        }
        if let Ok(contents) = std::fs::read_to_string(&log) {
            let search_from = (start_offset as usize).min(contents.len());
            if contents[search_from..].contains("MXDX_WORKER_READY") {
                return Ok(());
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// Wait for the client daemon to be fully ready by polling its log file for
/// the readiness marker. Returns when the daemon has connected to Matrix,
/// synced, and started its sync loop.
async fn wait_daemon_ready(timeout: Duration) -> Result<()> {
    let log = client_log_path();
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if tokio::time::Instant::now() >= deadline {
            bail!("daemon did not become ready within {}s (no MXDX_DAEMON_READY in {})", timeout.as_secs(), log);
        }
        if let Ok(contents) = std::fs::read_to_string(&log) {
            if contents.contains("MXDX_DAEMON_READY") {
                return Ok(());
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

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

/// Start the worker with an explicit `--room-name` and run a warm-up command.
async fn setup_worker_with_room(server: &str, worker_user: &str, worker_pass: &str,
                                 client_server: &str, client_user: &str, client_pass: &str,
                                 room: &str, authorized_user: &str,
                                 store_dir: &std::path::Path, keychain_dir: &std::path::Path) -> Child {
    let w = start_worker_with_room(server, worker_user, worker_pass, room, authorized_user, store_dir, keychain_dir);
    // start_worker_with_room truncates the log file, so read from offset 0
    wait_worker_ready(Duration::from_secs(120), 0).await.expect("temp worker startup timed out");

    let warmup = run_client(client_server, client_user, client_pass, room, &["run", "/bin/true"], store_dir, keychain_dir, 330);
    if !warmup.status.success() {
        let stderr = String::from_utf8_lossy(&warmup.stderr);
        eprintln!("[profile] warmup failed (may need account setup): {}", &stderr[stderr.len().saturating_sub(500)..]);
    }

    w
}

/// Login to Matrix via REST and return an access token.
async fn rest_login_token(server_url: &str, user: &str, pass: &str) -> Result<String> {
    let url = format!("{}/_matrix/client/v3/login", server_url.trim_end_matches('/'));
    let body = serde_json::json!({
        "type": "m.login.password",
        "identifier": { "type": "m.id.user", "user": user },
        "password": pass,
        "device_id": "mxdx-e2e-helper",
        "initial_device_display_name": "mxdx-e2e-helper",
    });
    let client = reqwest::Client::new();
    let resp = client.post(&url).json(&body).send().await?;
    if !resp.status().is_success() {
        bail!("rest_login: HTTP {}", resp.status());
    }
    let v: serde_json::Value = resp.json().await?;
    Ok(v["access_token"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("rest_login: no access_token in response"))?
        .to_string())
}

/// Create an UNENCRYPTED room with mxdx discovery markers via REST.
async fn rest_create_unencrypted_room(
    creds: &TestCreds,
    name: &str,
    topic: &str,
    launcher_id: &str,
    role: &str,
) -> Result<String> {
    let token = rest_login_token(&creds.server_url, &creds.worker_user, &creds.worker_pass).await?;
    let url = format!(
        "{}/_matrix/client/v3/createRoom",
        creds.server_url.trim_end_matches('/')
    );
    let mut creation_content = serde_json::Map::new();
    if role == "space" {
        creation_content.insert("type".to_string(), serde_json::Value::String("m.space".to_string()));
    }
    creation_content.insert(
        "org.mxdx.launcher_id".to_string(),
        serde_json::Value::String(launcher_id.to_string()),
    );
    creation_content.insert(
        "org.mxdx.role".to_string(),
        serde_json::Value::String(role.to_string()),
    );
    let body = serde_json::json!({
        "name": name,
        "topic": topic,
        "preset": "private_chat",
        "creation_content": serde_json::Value::Object(creation_content),
    });
    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .bearer_auth(&token)
        .json(&body)
        .send()
        .await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        bail!("rest_create_unencrypted_room: HTTP {}: {}", status, text);
    }
    let v: serde_json::Value = resp.json().await?;
    Ok(v["room_id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("rest_create_unencrypted_room: no room_id"))?
        .to_string())
}

/// Fetch the m.room.tombstone state event for a room.
async fn rest_get_tombstone(creds: &TestCreds, room_id: &str) -> Result<Option<String>> {
    let token = rest_login_token(&creds.server_url, &creds.worker_user, &creds.worker_pass).await?;
    let url = format!(
        "{}/_matrix/client/v3/rooms/{}/state/m.room.tombstone/",
        creds.server_url.trim_end_matches('/'),
        room_id,
    );
    let client = reqwest::Client::new();
    let resp = client.get(&url).bearer_auth(&token).send().await?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !resp.status().is_success() {
        bail!("rest_get_tombstone: HTTP {}", resp.status());
    }
    let v: serde_json::Value = resp.json().await?;
    Ok(v.get("replacement_room")
        .and_then(|r| r.as_str())
        .map(|s| s.to_string()))
}

/// Leave a room via REST. Best-effort, errors are silently ignored.
async fn rest_leave_room(server_url: &str, token: &str, room_id: &str) {
    let encoded: String = room_id.bytes().map(|b| {
        if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.' || b == b'~' {
            format!("{}", b as char)
        } else {
            format!("%{:02X}", b)
        }
    }).collect();
    let url = format!(
        "{}/_matrix/client/v3/rooms/{}/leave",
        server_url.trim_end_matches('/'), encoded
    );
    let _ = reqwest::Client::new()
        .post(&url).bearer_auth(token)
        .json(&serde_json::json!({}))
        .send().await;
}

// ---------------------------------------------------------------------------
// Preset parsing
// ---------------------------------------------------------------------------

fn parse_preset() -> Vec<u8> {
    let preset = std::env::var("E2E_PRESET").unwrap_or_else(|_| "default".to_string());
    match preset.as_str() {
        "quick"   => vec![0, 1, 2, 3, 5, 7],
        "default" => vec![0, 1, 2, 3, 5, 6, 7],
        "full"    => vec![0, 1, 2, 3, 4, 5, 6, 7],
        other     => panic!("Unknown E2E_PRESET: {other}. Use quick, default, or full."),
    }
}

fn should_run(phases: &[u8], phase: u8) -> bool {
    // Required phases always run
    matches!(phase, 1 | 5 | 7) || phases.contains(&phase)
}

// ===========================================================================
// PHASE 0 — SECURITY GATES
// ===========================================================================

async fn phase_0(creds: &TestCreds) -> Result<()> {
    eprintln!("\n=== Phase 0: Security Gates ===");

    // t00: no worker room
    {
        eprintln!("[t00] testing: no worker room...");
        let (store_dir, keychain_dir) = persistent_test_dirs_named("t00");
        let nonexistent_room = "mxdx-e2e-nonexistent-room-does-not-exist";

        let start = Instant::now();
        let out = run_client_with_liveness(
            &creds.server_url, &creds.client_user, &creds.client_pass,
            nonexistent_room, &["run", "/bin/echo", "should-not-run"],
            &store_dir, &keychain_dir,
        );
        let elapsed = start.elapsed();
        let stderr = String::from_utf8_lossy(&out.stderr);
        let exit_code = out.status.code();

        eprintln!("[t00] exit_code={:?}, stderr tail: {}", exit_code, &stderr[stderr.len().saturating_sub(300)..]);
        report("security/no-worker-room", "gate", elapsed, exit_code, 0);

        if exit_code != Some(10) && exit_code != Some(1) {
            bail!("SECURITY GATE FAILED: client should exit 10 (no worker room) but exited {:?}", exit_code);
        }
        if !stderr.contains("No worker room found") && !stderr.contains("no worker") {
            bail!("SECURITY GATE FAILED: stderr should mention 'No worker room found'");
        }
        eprintln!("[t00] PASS: client correctly rejected — no worker room");
    }

    // t01: stale worker — uses a dedicated room name so its lock doesn't
    // collide with Phase 1's persistent worker on the default room.
    {
        eprintln!("[t01] testing: stale worker...");
        let auth_user = creds.client_matrix_id();
        let (store_dir, keychain_dir) = persistent_test_dirs_named("t01");
        let t01_room = "mxdx-e2e-t01-stale-worker";
        let t01_home = write_short_telemetry_config("t01");

        let mut w = start_worker_with_room_home(
            &creds.server_url, &creds.worker_user, &creds.worker_pass,
            t01_room, &auth_user, &store_dir, &keychain_dir, Some(&t01_home),
        );
        tokio::time::sleep(Duration::from_secs(8)).await;

        let _ = w.kill();
        let _ = w.wait();
        eprintln!("[t01] worker killed, waiting for staleness threshold...");
        tokio::time::sleep(Duration::from_secs(5)).await;

        let worker_room = t01_room;
        let start = Instant::now();
        let out = run_client_with_liveness(
            &creds.server_url, &creds.client_user, &creds.client_pass,
            &worker_room, &["run", "/bin/echo", "should-not-run"],
            &store_dir, &keychain_dir,
        );
        let elapsed = start.elapsed();
        let stderr = String::from_utf8_lossy(&out.stderr);
        let exit_code = out.status.code();

        eprintln!("[t01] exit_code={:?}, stderr tail: {}", exit_code, &stderr[stderr.len().saturating_sub(300)..]);
        report("security/stale-worker", "gate", elapsed, exit_code, 0);

        if exit_code == Some(0) {
            bail!("SECURITY GATE FAILED: client should NOT succeed when worker is stale");
        }
        if !stderr.contains("stale") && !stderr.contains("No live worker") && !stderr.contains("last seen") {
            eprintln!("[t01] WARNING: stderr doesn't contain expected stale message, but exit code was non-zero");
        }
        eprintln!("[t01] PASS: client correctly rejected — stale worker");
    }

    // t02: capability mismatch
    {
        eprintln!("[t02] testing: capability mismatch...");
        let auth_user = creds.client_matrix_id();
        let (store_dir, keychain_dir) = persistent_test_dirs_named("t02");

        let t02_room = "mxdx-e2e-t02-capability";
        let t02_home = write_short_telemetry_config("t02");
        eprintln!("[t02] starting isolated worker (echo-only)...");
        let mut w = start_worker_with_room_and_commands_home(
            &creds.server_url, &creds.worker_user, &creds.worker_pass,
            Some(t02_room), &auth_user, &store_dir, &keychain_dir,
            &["echo", "/bin/echo"], Some(&t02_home),
        );
        wait_worker_ready(Duration::from_secs(120), 0).await.expect("temp worker startup timed out");

        let worker_room = t02_room;
        eprintln!("[t02] running warmup...");
        let warmup = run_client(
            &creds.server_url, &creds.client_user, &creds.client_pass,
            &worker_room, &["run", "/bin/echo", "warmup"],
            &store_dir, &keychain_dir, 330,
        );
        if !warmup.status.success() {
            let stderr = String::from_utf8_lossy(&warmup.stderr);
            eprintln!("[t02] warmup failed: {}", &stderr[stderr.len().saturating_sub(300)..]);
        }

        eprintln!("[t02] running liveness check (md5sum should be rejected)...");
        let start = Instant::now();
        let out = run_client_with_liveness(
            &creds.server_url, &creds.client_user, &creds.client_pass,
            &worker_room, &["run", "md5sum", "/dev/null"],
            &store_dir, &keychain_dir,
        );
        let elapsed = start.elapsed();
        let stderr = String::from_utf8_lossy(&out.stderr);
        let exit_code = out.status.code();

        let _ = w.kill(); let _ = w.wait();

        eprintln!("[t02] exit_code={:?}, stderr tail: {}", exit_code, &stderr[stderr.len().saturating_sub(300)..]);
        report("security/capability-mismatch", "gate", elapsed, exit_code, 0);

        if exit_code == Some(0) {
            bail!("SECURITY GATE FAILED: client should NOT succeed when worker lacks capability");
        }
        if !stderr.contains("No worker supports command") && !stderr.contains("capability") {
            eprintln!("[t02] WARNING: stderr doesn't contain expected capability message, but exit code was non-zero");
        }
        eprintln!("[t02] PASS: client correctly rejected — capability mismatch");
    }

    Ok(())
}

// ===========================================================================
// PHASE 1 — SETUP WORKER (start persistent worker, authorize both s1 + s2)
// ===========================================================================

async fn phase_1(creds: &TestCreds) -> Result<TestContext> {
    eprintln!("\n=== Phase 1: Setup Worker ===");

    let worker_room = default_worker_room(&creds.worker_user);
    let (store_dir, keychain_dir) = persistent_test_dirs();

    let config_home = dirs::home_dir()
        .expect("cannot resolve home dir")
        .join(".mxdx")
        .join("e2e-local")
        .join("home");
    std::fs::create_dir_all(&config_home).expect("failed to create config home");
    write_test_config(&config_home, creds, &worker_room);

    // Authorize both s1 and s2 client IDs so the same worker handles both
    // local and federated tests.
    let auth_s1 = creds.client_matrix_id();
    let auth_s2 = creds.server2_url.as_ref().map(|s2| creds.client_matrix_id_on(s2));

    let mut args = vec![
        "start".to_string(),
        "--homeserver".to_string(), creds.server_url.clone(),
        "--username".to_string(), creds.worker_user.clone(),
        "--password".to_string(), creds.worker_pass.clone(),
        "--authorized-user".to_string(), auth_s1.clone(),
    ];
    if let Some(ref s2_id) = auth_s2 {
        args.push("--authorized-user".to_string());
        args.push(s2_id.clone());
    }
    for cmd in ALLOWED_COMMANDS {
        args.push("--allowed-command".to_string());
        args.push(cmd.to_string());
    }
    let (out, err) = worker_log_files();
    let mut cmd = Command::new(cargo_bin("mxdx-worker"));
    cmd.args(&args)
        .env("MXDX_STORE_DIR", store_dir.to_str().unwrap())
        .env("MXDX_KEYCHAIN_DIR", keychain_dir.to_str().unwrap())
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err));

    let worker_start = Instant::now();
    let worker = spawn_child(cmd);

    // Wait for the worker to be FULLY ready: synced, keys shared, telemetry
    // posted. Log file was just truncated, so read from offset 0.
    wait_worker_ready(Duration::from_secs(120), 0).await?;
    report("worker-startup", "setup", worker_start.elapsed(), Some(0), 0);
    eprintln!("[t10] worker fully ready ({:.1}s)", worker_start.elapsed().as_secs_f64());

    // t11: Direct-mode warmup — seeds the client keychain + verifies the
    // worker accepts tasks. Cold start (first run) requires up to 120s
    // because the worker's 30s sync poll may delay key sharing.
    {
        let client_start = Instant::now();
        let warmup = run_client(
            &creds.server_url, &creds.client_user, &creds.client_pass,
            &worker_room, &["run", "/bin/true"],
            &store_dir, &keychain_dir, 120,
        );
        let client_connect = client_start.elapsed();
        report("client-connect(direct)", "setup", client_connect, warmup.status.code(), 0);

        let stderr = String::from_utf8_lossy(&warmup.stderr);
        if stderr.contains("fresh login completed") {
            eprintln!("[t11] connection type: fresh login (cold start)");
        } else if stderr.contains("session restored successfully") {
            eprintln!("[t11] connection type: session restore (warm start)");
        }

        if !warmup.status.success() {
            bail!("{}", format_test_failure("t11", "direct-warmup(/bin/true)", &warmup, 120, false));
        }
        eprintln!("[t11] PASS: direct-mode client warmup OK ({:.1}s)", client_connect.as_secs_f64());
    }

    // t12: Daemon warmup — starts the daemon, waits for it to connect to
    // Matrix and become fully synced. Must succeed within 60s.
    eprintln!("[t12] starting daemon warmup...");
    {
        // Spawn the daemon by running a command through it. The daemon starts
        // in the background, connects to Matrix, and processes the /bin/true
        // task once ready.
        let daemon_start = Instant::now();
        let daemon_warmup = run_client_daemon(
            &config_home, &["run", "/bin/true"],
            &store_dir, &keychain_dir, 60,
        );
        let daemon_elapsed = daemon_start.elapsed();
        report("daemon-warmup", "setup", daemon_elapsed, daemon_warmup.status.code(), 0);

        if !daemon_warmup.status.success() {
            bail!("{}", format_test_failure("t12", "daemon-warmup(/bin/true)", &daemon_warmup, 60, false));
        }

        // Verify the daemon reported full readiness in its log
        wait_daemon_ready(Duration::from_secs(10)).await?;
        eprintln!("[t12] PASS: daemon warmup OK ({:.1}s)", daemon_elapsed.as_secs_f64());
    }

    // t13: Federated (s2) daemon warmup — if a second server is configured,
    // start a separate daemon that connects via s2. This daemon runs in
    // parallel with the primary daemon for federated tests.
    let config_home_s2 = if let Some(ref s2) = creds.server2_url {
        eprintln!("[t13] starting federated (s2) daemon warmup...");
        let ch = dirs::home_dir()
            .expect("cannot resolve home dir")
            .join(".mxdx")
            .join("e2e-s2")
            .join("home");
        std::fs::create_dir_all(&ch).expect("failed to create s2 config home");

        // Truncate the s2 daemon log
        let s2_log = client_log_path_s2();
        let _ = std::fs::OpenOptions::new()
            .create(true).write(true).truncate(true)
            .open(&s2_log);

        write_test_config_for_server(
            &ch, s2, &creds.client_user, &creds.client_pass, &worker_room,
        );

        let s2_start = Instant::now();
        let s2_warmup = run_client_daemon_s2(
            &ch, &["run", "/bin/true"],
            &store_dir, &keychain_dir, 60,
        );
        let s2_elapsed = s2_start.elapsed();
        report("daemon-warmup(s2)", "setup", s2_elapsed, s2_warmup.status.code(), 0);

        if !s2_warmup.status.success() {
            eprintln!("[t13] WARNING: s2 daemon warmup failed ({:.1}s, exit {:?}) — federated tests may fail",
                s2_elapsed.as_secs_f64(), s2_warmup.status.code());
        } else {
            wait_daemon_ready_s2(Duration::from_secs(10)).await.ok();
            eprintln!("[t13] PASS: s2 daemon warmup OK ({:.1}s)", s2_elapsed.as_secs_f64());
        }
        Some(ch)
    } else {
        None
    };

    report("sync-total", "setup", worker_start.elapsed(), Some(0), 0);

    Ok(TestContext {
        worker,
        worker_room,
        creds: TestCreds {
            server_url: creds.server_url.clone(),
            server2_url: creds.server2_url.clone(),
            worker_user: creds.worker_user.clone(),
            worker_pass: creds.worker_pass.clone(),
            client_user: creds.client_user.clone(),
            client_pass: creds.client_pass.clone(),
        },
        store_dir,
        keychain_dir,
        config_home,
        config_home_s2,
    })
}

// ===========================================================================
// PHASE 2 — LOCAL TESTS (reuse shared worker, daemon mode)
// ===========================================================================

async fn phase_2(ctx: &TestContext) -> Result<()> {
    eprintln!("\n=== Phase 2: Local Tests ===");

    // t20: echo
    {
        let start = Instant::now();
        let out = run_client_daemon(
            &ctx.config_home, &["run", "/bin/echo", "hello", "world"],
            &ctx.store_dir, &ctx.keychain_dir, 10,
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        if !out.status.success() {
            bail!("{}", format_test_failure("t20", "echo", &out, 10, true));
        }
        report("echo", "mxdx-local-daemon", start.elapsed(), out.status.code(), stdout.lines().count());
    }

    // t21: exit code
    {
        let start = Instant::now();
        let out = run_client_daemon(
            &ctx.config_home, &["run", "/bin/false"],
            &ctx.store_dir, &ctx.keychain_dir, 10,
        );
        if was_timeout(&out) {
            bail!("{}", format_test_failure("t21", "exit-code", &out, 10, true));
        }
        if out.status.success() {
            bail!("t21 exit-code: expected failure but got success");
        }
        report("exit-code(/bin/false)", "mxdx-local-daemon", start.elapsed(), out.status.code(), 0);
    }

    // t22: md5sum
    {
        let fp = large_file(10_000);
        let script = md5_script(&fp);
        let start = Instant::now();
        let out = run_client_daemon(
            &ctx.config_home, &["run", "--", "/bin/sh", "-c", &script],
            &ctx.store_dir, &ctx.keychain_dir, 330,
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        let _ = std::fs::remove_file(&fp);
        if !out.status.success() {
            bail!("{}", format_test_failure("t22", "md5sum", &out, 330, false));
        }
        report("md5sum(10k lines)", "mxdx-local-daemon", start.elapsed(), out.status.code(), stdout.lines().count());
    }

    // t23: ping 30s
    {
        let start = Instant::now();
        let out = run_client_daemon(
            &ctx.config_home, &["run", "--", "ping", "-c", "30", "-i", "1", "1.1.1.1"],
            &ctx.store_dir, &ctx.keychain_dir, 90,
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        if !out.status.success() {
            bail!("{}", format_test_failure("t23", "ping", &out, 90, false));
        }
        report("ping(30s)", "mxdx-local-daemon", start.elapsed(), out.status.code(), stdout.lines().count());
    }

    Ok(())
}

// ===========================================================================
// PHASE 3 — FEDERATED TESTS (reuse persistent worker, client on s2)
// ===========================================================================

async fn phase_3(ctx: &TestContext) -> Result<()> {
    eprintln!("\n=== Phase 3: Federated Tests ===");

    let _s2 = ctx.creds.server2_url.as_deref()
        .context("server2 required for federated tests")?;
    let config_s2 = ctx.config_home_s2.as_ref()
        .context("s2 daemon config not available (s2 warmup may have failed)")?;

    // t30: echo federated (via s2 daemon)
    // Federated tests go through two homeservers + the worker's 30s sync
    // long-poll, so allow 45s for the round trip.
    {
        let start = Instant::now();
        let out = run_client_daemon_s2(
            config_s2, &["run", "/bin/echo", "hello", "world"],
            &ctx.store_dir, &ctx.keychain_dir, 45,
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        if !out.status.success() {
            bail!("{}", format_test_failure("t30", "echo federated", &out, 45, true));
        }
        report("echo", "mxdx-federated-daemon", start.elapsed(), out.status.code(), stdout.lines().count());
    }

    // t31: exit code federated
    {
        let start = Instant::now();
        let out = run_client_daemon_s2(
            config_s2, &["run", "/bin/false"],
            &ctx.store_dir, &ctx.keychain_dir, 45,
        );
        if was_timeout(&out) {
            bail!("{}", format_test_failure("t31", "exit-code federated", &out, 45, true));
        }
        if out.status.success() {
            bail!("t31 exit-code federated: expected failure but got success");
        }
        report("exit-code(/bin/false)", "mxdx-federated-daemon", start.elapsed(), out.status.code(), 0);
    }

    // t32: md5sum federated
    {
        let fp = large_file(10_000);
        let script = md5_script(&fp);
        let start = Instant::now();
        let out = run_client_daemon_s2(
            config_s2, &["run", "--", "/bin/sh", "-c", &script],
            &ctx.store_dir, &ctx.keychain_dir, 330,
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        let _ = std::fs::remove_file(&fp);
        if !out.status.success() {
            bail!("{}", format_test_failure("t32", "md5sum federated", &out, 330, false));
        }
        report("md5sum(10k lines)", "mxdx-federated-daemon", start.elapsed(), out.status.code(), stdout.lines().count());
    }

    // t33: ping 30s federated
    {
        let start = Instant::now();
        let out = run_client_daemon_s2(
            config_s2, &["run", "--", "ping", "-c", "30", "-i", "1", "1.1.1.1"],
            &ctx.store_dir, &ctx.keychain_dir, 90,
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        if !out.status.success() {
            bail!("{}", format_test_failure("t33", "ping federated", &out, 90, false));
        }
        report("ping(30s)", "mxdx-federated-daemon", start.elapsed(), out.status.code(), stdout.lines().count());
    }

    Ok(())
}

// ===========================================================================
// PHASE 4 — LONG TESTS + SSH BASELINES (parallel via std::thread::spawn)
// ===========================================================================

fn phase_4(creds: &TestCreds) -> Result<()> {
    eprintln!("\n=== Phase 4: Long + SSH Tests (parallel) ===");

    // Capture values needed by threads
    let server_url = creds.server_url.clone();
    let server2_url = creds.server2_url.clone();
    let worker_user = creds.worker_user.clone();
    let client_user = creds.client_user.clone();
    let client_pass = creds.client_pass.clone();
    let worker_room = default_worker_room(&worker_user);

    let mut handles: Vec<std::thread::JoinHandle<Result<()>>> = Vec::new();

    // Long ping local — uses shared worker via --no-daemon
    {
        let srv = server_url.clone();
        let cu = client_user.clone();
        let cp = client_pass.clone();
        let wr = worker_room.clone();
        handles.push(std::thread::spawn(move || {
            let (sd, kd) = persistent_test_dirs_named("t80-ping-local");
            let start = Instant::now();
            let out = run_client_no_output_timeout(
                &srv, &cu, &cp, &wr,
                &["run", "--", "ping", "-c", "300", "-i", "1", "1.1.1.1"],
                &sd, &kd,
                Duration::from_secs(60),
                Duration::from_secs(330),
            );
            let stdout = String::from_utf8_lossy(&out.stdout);
            if !out.status.success() {
                bail!("{}", format_test_failure("t80", "long ping local", &out, 330, false));
            }
            report("ping(5min)", "mxdx-local", start.elapsed(), out.status.code(), stdout.lines().count());
            Ok(())
        }));
    }

    // Long ping federated
    if let Some(ref s2) = server2_url {
        let s2 = s2.clone();
        let cu = client_user.clone();
        let cp = client_pass.clone();
        let wr = worker_room.clone();
        handles.push(std::thread::spawn(move || {
            let (sd, kd) = persistent_test_dirs_named("t80-ping-federated");
            let start = Instant::now();
            let out = run_client_no_output_timeout(
                &s2, &cu, &cp, &wr,
                &["run", "--", "ping", "-c", "300", "-i", "1", "1.1.1.1"],
                &sd, &kd,
                Duration::from_secs(60),
                Duration::from_secs(330),
            );
            let stdout = String::from_utf8_lossy(&out.stdout);
            if !out.status.success() {
                bail!("{}", format_test_failure("t81", "long ping federated", &out, 330, false));
            }
            report("ping(5min)", "mxdx-federated", start.elapsed(), out.status.code(), stdout.lines().count());
            Ok(())
        }));
    }

    // SSH baselines (4 threads)
    handles.push(std::thread::spawn(|| {
        let start = Instant::now();
        let out = run_ssh(&["/bin/echo", "hello", "world"]);
        report("echo", "ssh", start.elapsed(), out.status.code(), String::from_utf8_lossy(&out.stdout).lines().count());
        Ok(())
    }));

    handles.push(std::thread::spawn(|| {
        let start = Instant::now();
        let out = run_ssh(&["/bin/false"]);
        report("exit-code(/bin/false)", "ssh", start.elapsed(), out.status.code(), 0);
        Ok(())
    }));

    handles.push(std::thread::spawn(|| {
        let fp = large_file(10_000);
        let start = Instant::now();
        let out = run_ssh_script(&md5_script(&fp));
        let stdout = String::from_utf8_lossy(&out.stdout);
        report("md5sum(10k lines)", "ssh", start.elapsed(), out.status.code(), stdout.lines().count());
        let _ = std::fs::remove_file(&fp);
        Ok(())
    }));

    handles.push(std::thread::spawn(|| {
        let start = Instant::now();
        let out = run_ssh(&["ping", "-c", "30", "-i", "1", "1.1.1.1"]);
        let stdout = String::from_utf8_lossy(&out.stdout);
        report("ping(30s)", "ssh", start.elapsed(), out.status.code(), stdout.lines().count());
        Ok(())
    }));

    // Collect results — all threads run to completion
    let mut errors = Vec::new();
    for h in handles {
        match h.join() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => errors.push(format!("{e}")),
            Err(_) => errors.push("thread panicked".to_string()),
        }
    }

    if !errors.is_empty() {
        bail!("Phase 4 failures:\n  {}", errors.join("\n  "));
    }

    Ok(())
}

// ===========================================================================
// PHASE 5 — SHUTDOWN WORKER
// ===========================================================================

fn phase_5(ctx: &mut TestContext) {
    eprintln!("\n=== Phase 5: Shutdown Worker ===");
    kill_worker_graceful(&mut ctx.worker);
    eprintln!("[phase5] worker shut down");
}

// ===========================================================================
// PHASE 6 — SPECIAL TESTS (each spins up its own isolated worker as needed)
// ===========================================================================

async fn phase_6(creds: &TestCreds) -> Result<()> {
    eprintln!("\n=== Phase 6: Special Tests ===");

    // t40: explicit room name
    {
        let auth_user = creds.client_matrix_id();
        let explicit_room = "mxdx-e2e-profile-explicit";
        let (store_dir, keychain_dir) = persistent_test_dirs_named("t40");
        let mut w = setup_worker_with_room(
            &creds.server_url, &creds.worker_user, &creds.worker_pass,
            &creds.server_url, &creds.client_user, &creds.client_pass,
            explicit_room, &auth_user,
            &store_dir, &keychain_dir,
        ).await;

        let start = Instant::now();
        let out = run_client(
            &creds.server_url, &creds.client_user, &creds.client_pass,
            explicit_room, &["run", "/bin/echo", "explicit", "room"],
            &store_dir, &keychain_dir, 330,
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        let _ = w.kill(); let _ = w.wait();
        if !out.status.success() {
            bail!("{}", format_test_failure("t40", "explicit room", &out, 330, false));
        }
        report("echo(explicit-room)", "mxdx-local", start.elapsed(), out.status.code(), stdout.lines().count());
        eprintln!("[t40] PASS");
    }

    // t41: session restore
    {
        let auth_user = creds.client_matrix_id();
        let (store_dir, keychain_dir) = persistent_test_dirs_named("t41");

        eprintln!("[t41] starting first worker run");
        let mut w1 = start_worker(
            &creds.server_url, &creds.worker_user, &creds.worker_pass, &auth_user,
            &store_dir, &keychain_dir,
        );
        tokio::time::sleep(Duration::from_secs(15)).await;

        let _ = w1.kill();
        let _ = w1.wait();
        let stderr1 = worker_log_contents();
        eprintln!("[t41] first run log (last 500 chars): {}", &stderr1[stderr1.len().saturating_sub(500)..]);

        tokio::time::sleep(Duration::from_secs(2)).await;

        eprintln!("[t41] starting second worker run (should restore session)");
        let mut w2 = start_worker(
            &creds.server_url, &creds.worker_user, &creds.worker_pass, &auth_user,
            &store_dir, &keychain_dir,
        );
        tokio::time::sleep(Duration::from_secs(15)).await;

        let _ = w2.kill();
        let _ = w2.wait();
        let stderr2 = worker_log_contents();
        eprintln!("[t41] second run log (last 500 chars): {}", &stderr2[stderr2.len().saturating_sub(500)..]);

        if !stderr2.contains("session restored successfully")
            && !stderr2.contains("attempting session restore")
            && !stderr2.contains("session restore failed")
        {
            bail!(
                "t41: Second run should attempt session restore. Stderr: {}",
                &stderr2[stderr2.len().saturating_sub(1000)..]
            );
        }
        eprintln!("[t41] PASS: session restore attempted on second run");
    }

    // t42: backup round trip
    {
        let auth_user = creds.client_matrix_id();
        let explicit_room = "mxdx-e2e-t42-backup-round-trip";

        let (store_a, kc_a) = persistent_test_dirs_named("t42-a");
        let config_a = tempfile::Builder::new()
            .prefix("mxdx-config-t42a-")
            .tempdir()
            .expect("failed to create temp config dir");
        write_test_config(config_a.path(), creds, explicit_room);

        let mut worker_a = start_worker_with_room(
            &creds.server_url, &creds.worker_user, &creds.worker_pass, explicit_room, &auth_user,
            &store_a, &kc_a,
        );
        wait_worker_ready(Duration::from_secs(120), 0).await.expect("temp worker startup timed out");

        let out_a = run_client_daemon(
            config_a.path(),
            &["run", "/bin/echo", "round-trip-marker"],
            &store_a, &kc_a, 330,
        );
        if !out_a.status.success() {
            let stderr = String::from_utf8_lossy(&out_a.stderr);
            eprintln!("[t42] phase 1 client output (non-fatal): {}", &stderr[stderr.len().saturating_sub(500)..]);
        }
        kill_worker_graceful(&mut worker_a);

        let (store_b, kc_b) = persistent_test_dirs_named("t42-b");

        let config_b_dir = persistent_test_config_dir("t42-b");
        write_test_config(&config_b_dir, creds, explicit_room);

        let log_offset = std::fs::metadata(worker_log_path()).map(|m| m.len()).unwrap_or(0);
        let mut worker_b = start_worker_with_room(
            &creds.server_url, &creds.worker_user, &creds.worker_pass, explicit_room, &auth_user,
            &store_b, &kc_b,
        );
        wait_worker_ready(Duration::from_secs(120), log_offset).await.expect("t42: second worker startup timed out");

        let out_b = run_client_daemon(
            &config_b_dir,
            &["run", "/bin/echo", "after-backup-restore"],
            &store_b, &kc_b, 330,
        );
        let _ = worker_b.kill();
        let _ = worker_b.wait();

        if !out_b.status.success() {
            let stderr = String::from_utf8_lossy(&out_b.stderr);
            bail!(
                "t42: second worker failed to decrypt after backup restore: exit={:?} stderr={}",
                out_b.status.code(),
                &stderr[stderr.len().saturating_sub(800)..]
            );
        }
        eprintln!("[t42] PASS: backup round trip across distinct store dirs");
    }

    // t43: unencrypted room self-heal
    {
        let auth_user = creds.client_matrix_id();
        let run_id = uuid::Uuid::new_v4().simple().to_string();
        let launcher_id = format!("mxdx-e2e-t43-{}", &run_id[..8]);

        let space_topic = format!("org.mxdx.launcher.space:{launcher_id}");
        let exec_topic = format!("org.mxdx.launcher.exec:{launcher_id}");
        let logs_topic = format!("org.mxdx.launcher.logs:{launcher_id}");

        let bad_space = rest_create_unencrypted_room(
            creds, "mxdx-test-bad-space", &space_topic, &launcher_id, "space",
        ).await.context("t43: failed to seed unencrypted space")?;
        let bad_exec = rest_create_unencrypted_room(
            creds, "mxdx-test-bad-exec", &exec_topic, &launcher_id, "exec",
        ).await.context("t43: failed to seed unencrypted exec")?;
        let _bad_logs = rest_create_unencrypted_room(
            creds, "mxdx-test-bad-logs", &logs_topic, &launcher_id, "logs",
        ).await.context("t43: failed to seed unencrypted logs")?;
        eprintln!("[t43] seeded unencrypted topology: space={} exec={} logs={}", bad_space, bad_exec, _bad_logs);

        let (store, kc) = persistent_test_dirs_named("t43");

        let mut worker = start_worker_with_room(
            &creds.server_url, &creds.worker_user, &creds.worker_pass, &launcher_id, &auth_user,
            &store, &kc,
        );
        tokio::time::sleep(Duration::from_secs(30)).await;

        let tomb_exec = rest_get_tombstone(creds, &bad_exec).await;
        kill_worker_graceful(&mut worker);

        // Clean up test-specific rooms (unencrypted rooms + any replacements).
        if let Ok(token) = rest_login_token(&creds.server_url, &creds.worker_user, &creds.worker_pass).await {
            let mut all_rooms = vec![bad_space.clone(), bad_exec.clone(), _bad_logs.clone()];
            if let Ok(Some(ref replacement)) = tomb_exec {
                all_rooms.push(replacement.clone());
            }
            for rid in &all_rooms {
                rest_leave_room(&creds.server_url, &token, rid).await;
            }
        }

        match tomb_exec {
            Ok(Some(replacement)) => {
                eprintln!("[t43] PASS: bad exec room tombstoned -> {}", replacement);
            }
            Ok(None) => {
                bail!("t43: worker did not tombstone the unencrypted exec room {}", bad_exec);
            }
            Err(e) => bail!("t43: tombstone lookup failed: {}", e),
        }
    }

    // t44: diagnose --decrypt
    {
        let auth_user = creds.client_matrix_id();
        let worker_room = default_worker_room(&creds.worker_user);

        let (store, kc) = persistent_test_dirs_named("t44");
        let config_home = persistent_test_config_dir("t44");
        write_test_config(&config_home, creds, &worker_room);

        let mut worker = start_worker(
            &creds.server_url, &creds.worker_user, &creds.worker_pass, &auth_user,
            &store, &kc,
        );
        // start_worker truncates the log file, so read from offset 0
        wait_worker_ready(Duration::from_secs(120), 0).await.expect("temp worker startup timed out");

        let _ = run_client_daemon(
            &config_home,
            &["run", "/bin/echo", "t44-marker"],
            &store, &kc, 330,
        );

        // Kill the worker and daemon BEFORE running diagnose so they don't
        // contend over the SQLite crypto store.
        let _ = worker.kill();
        let _ = worker.wait();
        // Kill daemon by reading its PID file
        let daemon_pid_path = config_home.join(".mxdx").join("daemon").join("default.pid");
        if let Ok(pid_str) = std::fs::read_to_string(&daemon_pid_path) {
            if let Ok(_pid) = pid_str.trim().parse::<u32>() {
                let _ = Command::new("kill").arg("-9").arg(pid_str.trim()).output();
            }
            let _ = std::fs::remove_file(&daemon_pid_path);
        }
        let daemon_sock = config_home.join(".mxdx").join("daemon").join("default.sock");
        let _ = std::fs::remove_file(&daemon_sock);
        tokio::time::sleep(Duration::from_secs(2)).await;

        let client_bin = cargo_bin("mxdx-client");
        let out = Command::new("timeout")
            .arg("120")
            .arg(&client_bin)
            .args([
                "diagnose",
                "--decrypt",
                "--homeserver", &creds.server_url,
                "--username", &creds.worker_user,
                "--password", &creds.worker_pass,
            ])
            .env("MXDX_STORE_DIR", store.to_str().unwrap())
            .env("MXDX_KEYCHAIN_DIR", kc.to_str().unwrap())
            .env("MXDX_KEEP_PASSWORDS", "1")
            .env("RUST_LOG", "off")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("diagnose failed to run");

        let stdout_raw = String::from_utf8_lossy(&out.stdout);
        let stderr_raw = String::from_utf8_lossy(&out.stderr);
        eprintln!("[t44] diagnose exit={:?} stdout_bytes={} stderr_bytes={}",
            out.status.code(), out.stdout.len(), out.stderr.len());

        if was_timeout(&out) {
            bail!("t44: diagnose --decrypt timed out after 120s");
        }
        if !out.status.success() {
            bail!(
                "t44: diagnose --decrypt exit {:?} stderr={}",
                out.status.code(),
                &stderr_raw[stderr_raw.len().saturating_sub(500)..]
            );
        }

        let json: serde_json::Value = match serde_json::from_str(stdout_raw.trim()) {
            Ok(v) => v,
            Err(e) => bail!(
                "t44: diagnose output is not valid JSON: {e}\n  exit={:?} stdout_len={} first_300=[{}]",
                out.status.code(),
                stdout_raw.len(),
                &stdout_raw[..stdout_raw.len().min(300)]
            ),
        };

        let found = json
            .get("decrypted_state")
            .and_then(|d| d.as_object())
            .map(|o| !o.is_empty())
            .unwrap_or(false);
        if !found {
            let err = json.get("decrypt_error").cloned().unwrap_or(serde_json::Value::Null);
            bail!(
                "t44: diagnose --decrypt produced no decrypted_state events. decrypt_error={}",
                err
            );
        }
        eprintln!("[t44] PASS: diagnose --decrypt surfaced decrypted state events");
    }

    Ok(())
}

// ===========================================================================
// PHASE 7 — CLEANUP (safety net)
// ===========================================================================

fn phase_7() {
    eprintln!("\n=== Phase 7: Cleanup ===");
    let _ = std::process::Command::new("pkill")
        .args(["-f", "mxdx-worker start"])
        .status();
    let _ = std::process::Command::new("pkill")
        .args(["-f", "mxdx-client _daemon"])
        .status();
    if let Some(home) = dirs::home_dir() {
        let dirs_to_clean = [
            home.join(".mxdx").join("daemon"),
            home.join(".mxdx").join("e2e-local").join("home").join(".mxdx").join("daemon"),
        ];
        for d in &dirs_to_clean {
            if let Ok(entries) = std::fs::read_dir(d) {
                for e in entries.flatten() {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
    }
    eprintln!("[phase7] cleanup complete");
}

// ===========================================================================
// ORCHESTRATOR
// ===========================================================================

#[tokio::test]
#[ignore = "requires test-credentials.toml + beta server"]
async fn e2e() {
    let phases = parse_preset();
    let preset_name = std::env::var("E2E_PRESET").unwrap_or_else(|_| "default".to_string());
    eprintln!("E2E test suite — preset: {preset_name}, phases: {phases:?}");

    let creds = load_creds().expect("test-credentials.toml required");
    cleanup_stale_processes();
    setup_client_log();

    // Snapshot device and room counts before the test run.
    // After all phases, we verify these didn't increase (device/room leak = bug).
    let pre_devices_worker = rest_device_count(&creds.server_url, &creds.worker_user, &creds.worker_pass).await.unwrap_or(0);
    let pre_devices_client = rest_device_count(&creds.server_url, &creds.client_user, &creds.client_pass).await.unwrap_or(0);
    let pre_rooms_worker = rest_room_count(&creds.server_url, &creds.worker_user, &creds.worker_pass).await.unwrap_or(0);
    let pre_rooms_client = rest_room_count(&creds.server_url, &creds.client_user, &creds.client_pass).await.unwrap_or(0);
    eprintln!("[stats] PRE-RUN: worker devices={pre_devices_worker} rooms={pre_rooms_worker}, client devices={pre_devices_client} rooms={pre_rooms_client}");

    // Track whether we need cleanup phases even on failure
    let mut ctx: Option<TestContext> = None;
    let mut suite_error: Option<String> = None;

    // Phase 0: Security Gates
    if should_run(&phases, 0) && suite_error.is_none() {
        if let Err(e) = phase_0(&creds).await {
            suite_error = Some(format!("Phase 0 failed: {e}"));
        }
    }

    // Phase 1: Setup Worker (required)
    if suite_error.is_none() {
        match phase_1(&creds).await {
            Ok(c) => ctx = Some(c),
            Err(e) => suite_error = Some(format!("Phase 1 failed: {e}")),
        }
    }

    // Phase 2: Local Tests
    if should_run(&phases, 2) && suite_error.is_none() {
        if let Some(ref c) = ctx {
            if let Err(e) = phase_2(c).await {
                suite_error = Some(format!("Phase 2 failed: {e}"));
            }
        }
    }

    // Phase 3: Federated Tests
    if should_run(&phases, 3) && suite_error.is_none() {
        if let Some(ref c) = ctx {
            if let Err(e) = phase_3(c).await {
                suite_error = Some(format!("Phase 3 failed: {e}"));
            }
        }
    }

    // Phase 4: Long + SSH (parallel) — only in full preset
    if should_run(&phases, 4) && suite_error.is_none() {
        if let Err(e) = phase_4(&creds) {
            suite_error = Some(format!("Phase 4 failed: {e}"));
        }
    }

    // Phase 5: Shutdown Worker (required — always runs if worker was started)
    if let Some(ref mut c) = ctx {
        phase_5(c);
    }

    // Phase 6: Special Tests
    if should_run(&phases, 6) && suite_error.is_none() {
        if let Err(e) = phase_6(&creds).await {
            suite_error = Some(format!("Phase 6 failed: {e}"));
        }
    }

    // Phase 7: Cleanup (required — always runs)
    phase_7();

    // Verify device/room counts didn't increase (leak detection).
    // The first run is allowed to create devices (up to ~3 per user for
    // persistent stores that don't yet exist). Subsequent runs must not
    // create additional devices — session restore should reuse existing ones.
    // The rest_login_token helper uses a fixed device_id so it doesn't
    // contribute to device growth.
    let post_devices_worker = rest_device_count(&creds.server_url, &creds.worker_user, &creds.worker_pass).await.unwrap_or(0);
    let post_devices_client = rest_device_count(&creds.server_url, &creds.client_user, &creds.client_pass).await.unwrap_or(0);
    let post_rooms_worker = rest_room_count(&creds.server_url, &creds.worker_user, &creds.worker_pass).await.unwrap_or(0);
    let post_rooms_client = rest_room_count(&creds.server_url, &creds.client_user, &creds.client_pass).await.unwrap_or(0);
    eprintln!("[stats] POST-RUN: worker devices={post_devices_worker} rooms={post_rooms_worker}, client devices={post_devices_client} rooms={post_rooms_client}");

    let worker_device_delta = post_devices_worker as i64 - pre_devices_worker as i64;
    let client_device_delta = post_devices_client as i64 - pre_devices_client as i64;
    let worker_room_delta = post_rooms_worker as i64 - pre_rooms_worker as i64;
    let client_room_delta = post_rooms_client as i64 - pre_rooms_client as i64;
    eprintln!("[stats] DELTA: worker devices={worker_device_delta:+} rooms={worker_room_delta:+}, client devices={client_device_delta:+} rooms={client_room_delta:+}");

    // Cold start (nuclear reset) creates many devices: Phase 0 security gates
    // (3 workers), Phase 1 persistent worker, Phase 6 temp workers (t40-t44),
    // plus client devices. On warm runs this should be 0.
    const MAX_NEW_DEVICES_PER_USER: i64 = 12;
    if worker_device_delta > MAX_NEW_DEVICES_PER_USER {
        let msg = format!(
            "DEVICE LEAK: worker user gained {worker_device_delta} devices (max {MAX_NEW_DEVICES_PER_USER}). \
             Before={pre_devices_worker}, After={post_devices_worker}"
        );
        if suite_error.is_none() {
            suite_error = Some(msg.clone());
        }
        eprintln!("[stats] FAIL: {msg}");
    }
    if client_device_delta > MAX_NEW_DEVICES_PER_USER {
        let msg = format!(
            "DEVICE LEAK: client user gained {client_device_delta} devices (max {MAX_NEW_DEVICES_PER_USER}). \
             Before={pre_devices_client}, After={post_devices_client}"
        );
        if suite_error.is_none() {
            suite_error = Some(msg.clone());
        }
        eprintln!("[stats] FAIL: {msg}");
    }

    // Report final result
    if let Some(err) = suite_error {
        panic!("E2E suite failed: {err}");
    }

    eprintln!("\nE2E suite complete — all phases passed");
}
