//! Diagnostic / troubleshooting subcommand.
//!
//! Produces a single JSON document describing the runtime state of an mxdx
//! deployment from the perspective of either `mxdx-client` or `mxdx-worker`.
//!
//! Design constraints (see CLAUDE.md and the diagnose spec):
//! * MUST NOT take over the matrix-sdk crypto store. We use REST + file reads
//!   only — no `matrix_sdk::Client::builder()` calls.
//! * MUST NOT block. Every HTTP call has a 10s timeout, every file read is
//!   wrapped in error handling.
//! * MUST always emit valid JSON on stdout, even on partial failure.
//! * MUST be quiet — no log noise on stdout (set `RUST_LOG=off` if not set).

use anyhow::Result;
use mxdx_types::identity::KeychainBackend;
use mxdx_types::keychain_chain::ChainedKeychain;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

/// Which binary is invoking the diagnose run.
#[derive(Debug, Clone, Copy)]
pub enum DiagnoseBinary {
    Client,
    Worker,
}

impl DiagnoseBinary {
    fn as_str(&self) -> &'static str {
        match self {
            DiagnoseBinary::Client => "mxdx-client",
            DiagnoseBinary::Worker => "mxdx-worker",
        }
    }
}

/// Inputs collected from CLI flags + env. Resolution against profile config
/// is performed inside `run_diagnose`.
#[derive(Debug, Clone, Default)]
pub struct DiagnoseInput {
    pub profile: String,
    pub homeserver: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub pretty: bool,
}

/// Entry point: gather everything we can, emit JSON, never panic.
pub async fn run_diagnose(binary: DiagnoseBinary, input: DiagnoseInput) -> Result<()> {
    // Quiet by default — caller wants clean parseable JSON on stdout.
    if std::env::var_os("RUST_LOG").is_none() {
        std::env::set_var("RUST_LOG", "off");
    }

    let pretty = input.pretty;
    let report = build_report(binary, input).await;

    let serialized = if pretty {
        serde_json::to_string_pretty(&report)
    } else {
        serde_json::to_string(&report)
    };

    match serialized {
        Ok(s) => {
            println!("{}", s);
            Ok(())
        }
        Err(e) => {
            // Fall back to a minimal error doc — we promised valid JSON.
            let fallback = json!({
                "error": format!("failed to serialize diagnose report: {e}"),
                "binary": binary.as_str(),
            });
            println!("{}", fallback);
            Ok(())
        }
    }
}

async fn build_report(binary: DiagnoseBinary, input: DiagnoseInput) -> Value {
    let mut report = serde_json::Map::new();
    report.insert("binary".into(), json!(binary.as_str()));
    report.insert("version".into(), json!(env!("CARGO_PKG_VERSION")));
    report.insert("timestamp".into(), json!(rfc3339_now()));
    report.insert("profile".into(), json!(input.profile));

    // -------- config section --------
    let (config_section, resolved_homeserver, resolved_username, resolved_password) =
        resolve_config(binary, &input);
    report.insert("config".into(), config_section);

    // -------- process section --------
    report.insert("process".into(), inspect_process(binary, &input.profile));

    // -------- session (local stored credentials) --------
    let (session_section, stored_token, stored_device_id) = inspect_local_session(
        resolved_homeserver.as_deref(),
        resolved_username.as_deref(),
    );
    report.insert("session".into(), session_section);

    // -------- matrix (REST) --------
    let http = match reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            report.insert(
                "matrix".into(),
                json!({"error": format!("failed to build http client: {e}"), "login_ok": false}),
            );
            report.insert("trust".into(), json!({"error": "no http client"}));
            report.insert("engagement".into(), json!({}));
            return Value::Object(report);
        }
    };

    let (matrix_section, access_token, login_device_id, user_id, joined_room_ids) = match (
        resolved_homeserver.as_deref(),
        resolved_username.as_deref(),
    ) {
        (Some(hs), Some(user)) => {
            collect_matrix(
                &http,
                hs,
                user,
                resolved_password.as_deref(),
                stored_token.as_deref(),
                stored_device_id.as_deref(),
            )
            .await
        }
        _ => (
            json!({
                "login_ok": false,
                "error": "no homeserver/username available (provide --homeserver/--username/--password, env vars, or profile config)",
            }),
            None,
            None,
            None,
            Vec::new(),
        ),
    };
    report.insert("matrix".into(), matrix_section);

    // -------- trust (REST keys/query) --------
    let trust_section = if let (Some(hs), Some(token), Some(uid)) = (
        resolved_homeserver.as_deref(),
        access_token.as_deref(),
        user_id.as_deref(),
    ) {
        collect_trust(&http, hs, token, uid, &joined_room_ids).await
    } else {
        json!({"error": "no authenticated session, cannot query keys", "device_keys": [], "cross_signing": null})
    };
    report.insert("trust".into(), trust_section);

    // engagement is partly populated inside collect_matrix already; keep a top-level summary
    report.insert(
        "engagement".into(),
        json!({
            "note": "see matrix.joined_rooms[].recent_sessions and recent_workers for live state",
        }),
    );

    let _ = login_device_id; // already in matrix section

    Value::Object(report)
}

// ---------------------------------------------------------------------------
// config resolution
// ---------------------------------------------------------------------------

fn resolve_config(
    binary: DiagnoseBinary,
    input: &DiagnoseInput,
) -> (Value, Option<String>, Option<String>, Option<String>) {
    // Try to load the runtime config — but never fail the whole diagnose.
    let mut homeserver: Option<String> = input.homeserver.clone();
    let mut username: Option<String> = input.username.clone();
    let mut password: Option<String> = input.password.clone();
    let mut config_file_path: Option<PathBuf> = None;
    let mut load_error: Option<String> = None;

    match binary {
        DiagnoseBinary::Client => {
            match crate::config::ClientRuntimeConfig::load() {
                Ok(cfg) => {
                    if homeserver.is_none() || username.is_none() || password.is_none() {
                        if let Some(acct) = cfg.defaults.accounts.first() {
                            if homeserver.is_none() {
                                homeserver = Some(acct.homeserver.clone());
                            }
                            if username.is_none() {
                                let local = acct
                                    .user_id
                                    .split(':')
                                    .next()
                                    .unwrap_or(&acct.user_id)
                                    .trim_start_matches('@')
                                    .to_string();
                                username = Some(local);
                            }
                            if password.is_none() {
                                if let Some(p) = &acct.password {
                                    password = Some(p.clone());
                                }
                            }
                        }
                    }
                    let cfg_dir = mxdx_types::config::config_dir();
                    let cf = cfg_dir.join("client.toml");
                    if cf.exists() {
                        config_file_path = Some(cf);
                    }
                }
                Err(e) => load_error = Some(format!("client config load failed: {e}")),
            }
        }
        DiagnoseBinary::Worker => {
            // Avoid pulling in worker crate; load defaults + worker.toml directly.
            match mxdx_types::config::load_config::<mxdx_types::config::DefaultsConfig>(
                "defaults.toml",
            ) {
                Ok(defaults) => {
                    if let Some(acct) = defaults.accounts.first() {
                        if homeserver.is_none() {
                            homeserver = Some(acct.homeserver.clone());
                        }
                        if username.is_none() {
                            let local = acct
                                .user_id
                                .split(':')
                                .next()
                                .unwrap_or(&acct.user_id)
                                .trim_start_matches('@')
                                .to_string();
                            username = Some(local);
                        }
                        if password.is_none() {
                            if let Some(p) = &acct.password {
                                password = Some(p.clone());
                            }
                        }
                    }
                }
                Err(e) => load_error = Some(format!("worker defaults load failed: {e}")),
            }
            let cfg_dir = mxdx_types::config::config_dir();
            let cf = cfg_dir.join("worker.toml");
            if cf.exists() {
                config_file_path = Some(cf);
            }
        }
    }

    // Env vars (lower priority than CLI flags but checked here as fallback)
    if homeserver.is_none() {
        homeserver = std::env::var("MXDX_HOMESERVER").ok();
    }
    if username.is_none() {
        username = std::env::var("MXDX_USERNAME").ok();
    }
    if password.is_none() {
        password = std::env::var("MXDX_PASSWORD").ok();
    }

    // Compose Matrix user_id when we have enough info
    let matrix_user_id = match (&username, &homeserver) {
        (Some(u), Some(hs)) => {
            let server = hs
                .trim_start_matches("https://")
                .trim_start_matches("http://")
                .trim_end_matches('/');
            // Strip path components if any (e.g. matrix.org/_matrix/...)
            let server = server.split('/').next().unwrap_or(server);
            let local = u.trim_start_matches('@').split(':').next().unwrap_or(u);
            Some(format!("@{}:{}", local, server))
        }
        _ => None,
    };

    let store_dir = dirs::home_dir()
        .map(|h| h.join(".mxdx").join("store"))
        .map(|p| p.display().to_string());
    let keychain_dir = dirs::config_dir()
        .map(|c| c.join("mxdx"))
        .map(|p| p.display().to_string());

    let cfg_value = json!({
        "homeserver": homeserver,
        "username": username,
        "matrix_user_id": matrix_user_id,
        "store_dir": store_dir,
        "keychain_dir": keychain_dir,
        "config_file": config_file_path.as_ref().map(|p| p.display().to_string()),
        "load_error": load_error,
        "password_present": password.is_some(),
    });
    (cfg_value, homeserver, username, password)
}

// ---------------------------------------------------------------------------
// process inspection
// ---------------------------------------------------------------------------

fn inspect_process(binary: DiagnoseBinary, profile: &str) -> Value {
    let mut out = serde_json::Map::new();

    let target_name = match binary {
        DiagnoseBinary::Worker => "mxdx-worker",
        DiagnoseBinary::Client => "mxdx-client",
    };

    // For client, also probe daemon socket
    let socket_path = match binary {
        DiagnoseBinary::Client => Some(crate::daemon::transport::unix::socket_path(profile)),
        DiagnoseBinary::Worker => None,
    };
    let pid_file_path = match binary {
        DiagnoseBinary::Client => Some(crate::daemon::transport::unix::pid_path(profile)),
        DiagnoseBinary::Worker => None,
    };

    // PID discovery: prefer daemon PID file (client) → /proc scan
    let mut pid: Option<i64> = None;
    if let Some(p) = &pid_file_path {
        if let Ok(s) = std::fs::read_to_string(p) {
            if let Ok(n) = s.trim().parse::<i64>() {
                pid = Some(n);
            }
        }
    }
    if pid.is_none() {
        pid = scan_proc_for_name(target_name);
    }

    let (binary_path, started_at) = match pid {
        Some(p) => (proc_exe(p), proc_start_time(p)),
        None => (None, None),
    };
    let running = pid
        .map(|p| std::path::Path::new(&format!("/proc/{}", p)).exists())
        .unwrap_or(false);

    out.insert("running".into(), json!(running));
    out.insert("pid".into(), json!(pid));
    out.insert("binary_path".into(), json!(binary_path));
    out.insert("started_at".into(), json!(started_at));
    out.insert(
        "socket_path".into(),
        json!(socket_path.as_ref().map(|p| p.display().to_string())),
    );

    let socket_connectable = match &socket_path {
        Some(p) if p.exists() => std::os::unix::net::UnixStream::connect(p).is_ok(),
        _ => false,
    };
    out.insert("socket_connectable".into(), json!(socket_connectable));

    Value::Object(out)
}

fn scan_proc_for_name(name: &str) -> Option<i64> {
    let entries = std::fs::read_dir("/proc").ok()?;
    for entry in entries.flatten() {
        let pid: i64 = match entry.file_name().to_string_lossy().parse() {
            Ok(p) => p,
            Err(_) => continue,
        };
        let comm = match std::fs::read_to_string(format!("/proc/{}/comm", pid)) {
            Ok(s) => s.trim().to_string(),
            Err(_) => continue,
        };
        if comm == name {
            // Extra: skip our own PID
            if pid as u32 != std::process::id() {
                return Some(pid);
            }
        }
    }
    None
}

fn proc_exe(pid: i64) -> Option<String> {
    std::fs::read_link(format!("/proc/{}/exe", pid))
        .ok()
        .map(|p| p.display().to_string())
}

fn proc_start_time(pid: i64) -> Option<String> {
    // /proc/[pid]/stat field 22 = starttime in clock ticks since boot
    let stat = std::fs::read_to_string(format!("/proc/{}/stat", pid)).ok()?;
    // The 2nd field is comm in parens and may contain spaces.
    let close = stat.rfind(')')?;
    let after = &stat[close + 1..];
    let fields: Vec<&str> = after.split_whitespace().collect();
    // After ')' the fields start at index 0 = state (field 3 in 1-indexed)
    // starttime is field 22 → index 22 - 3 = 19
    let starttime_ticks: u64 = fields.get(19)?.parse().ok()?;

    let clk_tck = unsafe { libc_sysconf_clk_tck() } as u64;
    if clk_tck == 0 {
        return None;
    }
    let proc_stat = std::fs::read_to_string("/proc/stat").ok()?;
    let btime: u64 = proc_stat
        .lines()
        .find_map(|l| l.strip_prefix("btime "))
        .and_then(|v| v.trim().parse().ok())?;
    let secs_since_epoch = btime + starttime_ticks / clk_tck;
    Some(unix_to_rfc3339(secs_since_epoch as i64))
}

// Avoid pulling libc as a dependency: use a tiny FFI shim.
unsafe fn libc_sysconf_clk_tck() -> i64 {
    extern "C" {
        fn sysconf(name: i32) -> i64;
    }
    // _SC_CLK_TCK = 2 on Linux glibc
    sysconf(2)
}

// ---------------------------------------------------------------------------
// local stored session
// ---------------------------------------------------------------------------

fn inspect_local_session(
    homeserver: Option<&str>,
    username: Option<&str>,
) -> (Value, Option<String>, Option<String>) {
    let (Some(hs), Some(user)) = (homeserver, username) else {
        return (
            json!({
                "stored": false,
                "device_id": null,
                "device_id_source": "none",
                "fresh_login_required": true,
                "error": "no homeserver/username to look up",
            }),
            None,
            None,
        );
    };

    let chain = match ChainedKeychain::default_chain() {
        Ok(c) => c,
        Err(e) => {
            return (
                json!({
                    "stored": false,
                    "device_id": null,
                    "device_id_source": "none",
                    "fresh_login_required": true,
                    "error": format!("keychain init failed: {e}"),
                }),
                None,
                None,
            );
        }
    };

    let key = mxdx_matrix::session::session_key(user, hs);
    match chain.get(&key) {
        Ok(Some(bytes)) => {
            let parsed: Result<mxdx_matrix::session::SessionData, _> = serde_json::from_slice(&bytes);
            match parsed {
                Ok(sess) => (
                    json!({
                        "stored": true,
                        "device_id": sess.device_id,
                        "device_id_source": "keychain",
                        "fresh_login_required": false,
                    }),
                    Some(sess.access_token),
                    Some(sess.device_id),
                ),
                Err(e) => (
                    json!({
                        "stored": true,
                        "device_id": null,
                        "device_id_source": "keychain",
                        "fresh_login_required": true,
                        "error": format!("stored session present but failed to parse: {e}"),
                    }),
                    None,
                    None,
                ),
            }
        }
        Ok(None) => (
            json!({
                "stored": false,
                "device_id": null,
                "device_id_source": "none",
                "fresh_login_required": true,
            }),
            None,
            None,
        ),
        Err(e) => (
            json!({
                "stored": false,
                "device_id": null,
                "device_id_source": "none",
                "fresh_login_required": true,
                "error": format!("keychain get failed: {e}"),
            }),
            None,
            None,
        ),
    }
}

// ---------------------------------------------------------------------------
// matrix REST collection
// ---------------------------------------------------------------------------

async fn collect_matrix(
    http: &reqwest::Client,
    homeserver: &str,
    username: &str,
    password: Option<&str>,
    stored_token: Option<&str>,
    _stored_device_id: Option<&str>,
) -> (
    Value,
    Option<String>, // access_token
    Option<String>, // device_id
    Option<String>, // user_id
    Vec<String>,    // joined_room_ids
) {
    let base = homeserver.trim_end_matches('/');
    let base = if base.starts_with("http://") || base.starts_with("https://") {
        base.to_string()
    } else {
        format!("https://{}", base)
    };

    let mut errors: Vec<String> = Vec::new();
    let mut access_token: Option<String> = None;
    let mut device_id: Option<String> = None;
    let mut user_id: Option<String> = None;

    // 1) Validate stored token via /account/whoami if present
    if let Some(token) = stored_token {
        let url = format!("{}/_matrix/client/v3/account/whoami", base);
        match http.get(&url).bearer_auth(token).send().await {
            Ok(resp) if resp.status().is_success() => {
                if let Ok(v) = resp.json::<Value>().await {
                    user_id = v.get("user_id").and_then(|s| s.as_str()).map(String::from);
                    device_id = v.get("device_id").and_then(|s| s.as_str()).map(String::from);
                    access_token = Some(token.to_string());
                }
            }
            Ok(resp) => {
                errors.push(format!("stored token whoami failed: {}", resp.status()));
            }
            Err(e) => errors.push(format!("stored token whoami error: {e}")),
        }
    }

    // 2) Fall back to password login
    if access_token.is_none() {
        if let Some(pass) = password {
            let url = format!("{}/_matrix/client/v3/login", base);
            let body = json!({
                "type": "m.login.password",
                "identifier": {"type": "m.id.user", "user": username},
                "password": pass,
                "initial_device_display_name": "mxdx-diagnose",
            });
            match http.post(&url).json(&body).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        if let Ok(v) = resp.json::<Value>().await {
                            access_token =
                                v.get("access_token").and_then(|s| s.as_str()).map(String::from);
                            device_id = v.get("device_id").and_then(|s| s.as_str()).map(String::from);
                            user_id = v.get("user_id").and_then(|s| s.as_str()).map(String::from);
                        }
                    } else {
                        let body = resp.text().await.unwrap_or_default();
                        errors.push(format!("login {}: {}", status, body));
                    }
                }
                Err(e) => errors.push(format!("login error: {e}")),
            }
        } else {
            errors.push("no stored token and no password — cannot authenticate".into());
        }
    }

    let Some(token) = access_token.clone() else {
        return (
            json!({
                "login_ok": false,
                "errors": errors,
                "joined_rooms": [],
                "invited_rooms": [],
                "left_rooms": [],
            }),
            None,
            None,
            None,
            Vec::new(),
        );
    };

    // 3) /joined_rooms
    let mut joined_room_ids: Vec<String> = Vec::new();
    let url = format!("{}/_matrix/client/v3/joined_rooms", base);
    match http.get(&url).bearer_auth(&token).send().await {
        Ok(resp) if resp.status().is_success() => {
            if let Ok(v) = resp.json::<Value>().await {
                if let Some(arr) = v.get("joined_rooms").and_then(|a| a.as_array()) {
                    joined_room_ids = arr
                        .iter()
                        .filter_map(|s| s.as_str().map(String::from))
                        .collect();
                }
            }
        }
        Ok(resp) => errors.push(format!("joined_rooms: {}", resp.status())),
        Err(e) => errors.push(format!("joined_rooms error: {e}")),
    }

    // 4) Initial sync (timeout=0) for invite/state
    let mut invited_rooms: Vec<Value> = Vec::new();
    let mut left_rooms: Vec<Value> = Vec::new();
    let mut sync_state: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let sync_url = format!("{}/_matrix/client/v3/sync?timeout=0", base);
    match http.get(&sync_url).bearer_auth(&token).send().await {
        Ok(resp) if resp.status().is_success() => {
            if let Ok(v) = resp.json::<Value>().await {
                if let Some(invite) = v.pointer("/rooms/invite").and_then(|i| i.as_object()) {
                    for (rid, data) in invite {
                        let inviter = data
                            .pointer("/invite_state/events")
                            .and_then(|e| e.as_array())
                            .and_then(|arr| {
                                arr.iter().find_map(|ev| {
                                    if ev.get("type").and_then(|t| t.as_str())
                                        == Some("m.room.member")
                                    {
                                        ev.get("sender").and_then(|s| s.as_str()).map(String::from)
                                    } else {
                                        None
                                    }
                                })
                            });
                        let name = data
                            .pointer("/invite_state/events")
                            .and_then(|e| e.as_array())
                            .and_then(|arr| {
                                arr.iter().find_map(|ev| {
                                    if ev.get("type").and_then(|t| t.as_str())
                                        == Some("m.room.name")
                                    {
                                        ev.pointer("/content/name")
                                            .and_then(|n| n.as_str())
                                            .map(String::from)
                                    } else {
                                        None
                                    }
                                })
                            });
                        invited_rooms.push(json!({
                            "room_id": rid,
                            "name": name,
                            "inviter": inviter,
                        }));
                    }
                }
                if let Some(leave) = v.pointer("/rooms/leave").and_then(|i| i.as_object()) {
                    for (rid, _) in leave {
                        left_rooms.push(json!({"room_id": rid}));
                    }
                }
                if let Some(joined) = v.pointer("/rooms/join").and_then(|i| i.as_object()) {
                    for (rid, data) in joined {
                        if let Some(events) = data
                            .pointer("/state/events")
                            .and_then(|e| e.as_array())
                        {
                            sync_state.insert(rid.clone(), events.clone());
                        }
                    }
                }
            }
        }
        Ok(resp) => errors.push(format!("sync: {}", resp.status())),
        Err(e) => errors.push(format!("sync error: {e}")),
    }

    // 5) Build per-room reports
    let mut joined_rooms_out: Vec<Value> = Vec::new();
    for rid in &joined_room_ids {
        let state_events = sync_state.remove(rid).unwrap_or_default();
        joined_rooms_out
            .push(build_room_report(http, &base, &token, rid, &state_events).await);
    }

    let matrix_section = json!({
        "login_ok": true,
        "login_device_id": device_id,
        "login_user_id": user_id,
        "errors": errors,
        "joined_rooms": joined_rooms_out,
        "invited_rooms": invited_rooms,
        "left_rooms": left_rooms,
    });

    (matrix_section, Some(token), device_id, user_id, joined_room_ids)
}

async fn build_room_report(
    http: &reqwest::Client,
    base: &str,
    token: &str,
    room_id: &str,
    state_events: &[Value],
) -> Value {
    let mut name: Option<String> = None;
    let mut topic: Option<String> = None;
    let mut encryption: Option<Value> = None;
    let mut member_count: usize = 0;
    let mut type_counts: BTreeMap<String, u64> = BTreeMap::new();
    let mut recent_workers: Vec<Value> = Vec::new();
    let mut recent_sessions: Vec<Value> = Vec::new();

    for ev in state_events {
        let etype = ev.get("type").and_then(|t| t.as_str()).unwrap_or("");
        *type_counts.entry(etype.to_string()).or_insert(0) += 1;
        match etype {
            "m.room.name" => {
                name = ev
                    .pointer("/content/name")
                    .and_then(|n| n.as_str())
                    .map(String::from);
            }
            "m.room.topic" => {
                topic = ev
                    .pointer("/content/topic")
                    .and_then(|n| n.as_str())
                    .map(String::from);
            }
            "m.room.encryption" => {
                encryption = Some(ev.get("content").cloned().unwrap_or(json!({})));
            }
            "m.room.member" => {
                if ev.pointer("/content/membership").and_then(|m| m.as_str()) == Some("join") {
                    member_count += 1;
                }
            }
            "org.mxdx.host_telemetry" | "org.mxdx.worker.telemetry" => {
                let state_key = ev.get("state_key").and_then(|s| s.as_str()).unwrap_or("");
                let content = ev.get("content").cloned().unwrap_or(json!({}));
                recent_workers.push(json!({
                    "state_key": state_key,
                    "uuid": content.get("uuid"),
                    "status": content.get("status"),
                    "timestamp": content.get("timestamp").or_else(|| content.get("ts")),
                    "encrypted": false,
                }));
            }
            "org.mxdx.session.active" | "org.mxdx.session.completed" => {
                let content = ev.get("content").cloned().unwrap_or(json!({}));
                recent_sessions.push(json!({
                    "uuid": content.get("uuid").or_else(|| content.get("session_uuid")),
                    "bin": content.get("bin"),
                    "args": content.get("args"),
                    "status": content.get("status"),
                    "exit_code": content.get("exit_code"),
                    "encrypted": false,
                }));
            }
            "m.room.encrypted" => {
                let state_key = ev.get("state_key").and_then(|s| s.as_str()).unwrap_or("");
                if state_key.starts_with("org.mxdx.host_telemetry")
                    || state_key.starts_with("org.mxdx.worker.telemetry")
                {
                    recent_workers.push(json!({
                        "state_key": state_key,
                        "encrypted": true,
                    }));
                } else if state_key.starts_with("org.mxdx.session.") {
                    recent_sessions.push(json!({
                        "state_key": state_key,
                        "encrypted": true,
                    }));
                }
            }
            _ => {}
        }
    }

    // Fallback: fetch m.room.name / topic / encryption directly if missing
    if name.is_none() {
        if let Some(v) = fetch_state(http, base, token, room_id, "m.room.name").await {
            name = v
                .get("name")
                .and_then(|n| n.as_str())
                .map(String::from);
        }
    }
    if topic.is_none() {
        if let Some(v) = fetch_state(http, base, token, room_id, "m.room.topic").await {
            topic = v
                .get("topic")
                .and_then(|n| n.as_str())
                .map(String::from);
        }
    }
    if encryption.is_none() {
        if let Some(v) = fetch_state(http, base, token, room_id, "m.room.encryption").await {
            encryption = Some(v);
        }
    }

    // type_hint from topic or name
    let type_hint = derive_type_hint(topic.as_deref(), name.as_deref());

    // Normalize encrypt_state_events: accept both the canonical key and the
    // unstable io.element.msc4362.* variant that tuwunel emits.
    let encrypt_state_events: Option<bool> = encryption.as_ref().and_then(|c| {
        c.get("encrypt_state_events")
            .and_then(|b| b.as_bool())
            .or_else(|| {
                c.get("io.element.msc4362.encrypt_state_events")
                    .and_then(|b| b.as_bool())
            })
    });

    json!({
        "room_id": room_id,
        "name": name,
        "topic": topic,
        "member_count": member_count,
        "encryption": encryption,
        "encrypt_state_events": encrypt_state_events,
        "type_hint": type_hint,
        "recent_state_event_types": type_counts,
        "recent_workers": recent_workers,
        "recent_sessions": recent_sessions,
    })
}

async fn fetch_state(
    http: &reqwest::Client,
    base: &str,
    token: &str,
    room_id: &str,
    event_type: &str,
) -> Option<Value> {
    let url = format!(
        "{}/_matrix/client/v3/rooms/{}/state/{}/",
        base,
        urlencoding::encode(room_id),
        event_type
    );
    let resp = http.get(&url).bearer_auth(token).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.json::<Value>().await.ok()
}

fn derive_type_hint(topic: Option<&str>, name: Option<&str>) -> &'static str {
    let t = topic.unwrap_or("");
    if t.contains("launcher.exec") || t.contains(".exec") {
        return "exec";
    }
    if t.contains("launcher.logs") || t.contains(".logs") {
        return "logs";
    }
    if t.contains("space") {
        return "space";
    }
    let n = name.unwrap_or("");
    if n.contains("exec") {
        return "exec";
    }
    if n.contains("logs") {
        return "logs";
    }
    "other"
}

// ---------------------------------------------------------------------------
// trust / device keys
// ---------------------------------------------------------------------------

async fn collect_trust(
    http: &reqwest::Client,
    homeserver: &str,
    token: &str,
    self_user_id: &str,
    joined_room_ids: &[String],
) -> Value {
    let base = homeserver.trim_end_matches('/');
    let base = if base.starts_with("http") {
        base.to_string()
    } else {
        format!("https://{}", base)
    };

    // Build set of users to query: self + members of joined rooms (capped)
    let mut users: Vec<String> = vec![self_user_id.to_string()];
    'outer: for rid in joined_room_ids.iter().take(20) {
        let url = format!(
            "{}/_matrix/client/v3/rooms/{}/joined_members",
            base,
            urlencoding::encode(rid)
        );
        if let Ok(resp) = http.get(&url).bearer_auth(token).send().await {
            if resp.status().is_success() {
                if let Ok(v) = resp.json::<Value>().await {
                    if let Some(members) = v.get("joined").and_then(|j| j.as_object()) {
                        for uid in members.keys() {
                            if !users.contains(uid) {
                                users.push(uid.clone());
                            }
                            if users.len() > 50 {
                                break 'outer;
                            }
                        }
                    }
                }
            }
        }
    }

    let mut device_keys_map = serde_json::Map::new();
    for u in &users {
        device_keys_map.insert(u.clone(), json!([]));
    }
    let body = json!({"device_keys": device_keys_map, "timeout": 5000});
    let url = format!("{}/_matrix/client/v3/keys/query", base);
    let resp = match http.post(&url).bearer_auth(token).json(&body).send().await {
        Ok(r) => r,
        Err(e) => {
            return json!({
                "error": format!("keys/query failed: {e}"),
                "device_keys": [],
                "cross_signing": null,
            })
        }
    };
    if !resp.status().is_success() {
        return json!({
            "error": format!("keys/query status {}", resp.status()),
            "device_keys": [],
            "cross_signing": null,
        });
    }
    let v: Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => {
            return json!({
                "error": format!("keys/query parse: {e}"),
                "device_keys": [],
                "cross_signing": null,
            })
        }
    };

    let mut device_keys_out: Vec<Value> = Vec::new();
    if let Some(dk_obj) = v.get("device_keys").and_then(|x| x.as_object()) {
        for (user_id, devices) in dk_obj {
            if let Some(devmap) = devices.as_object() {
                for (device_id, dev_data) in devmap {
                    let display_name = dev_data
                        .pointer("/unsigned/device_display_name")
                        .and_then(|n| n.as_str())
                        .map(String::from);
                    let ed25519 = dev_data
                        .get("keys")
                        .and_then(|k| k.as_object())
                        .and_then(|k| k.get(&format!("ed25519:{}", device_id)))
                        .and_then(|s| s.as_str())
                        .map(String::from);
                    let curve25519 = dev_data
                        .get("keys")
                        .and_then(|k| k.as_object())
                        .and_then(|k| k.get(&format!("curve25519:{}", device_id)))
                        .and_then(|s| s.as_str())
                        .map(String::from);
                    device_keys_out.push(json!({
                        "user_id": user_id,
                        "device_id": device_id,
                        "display_name": display_name,
                        "ed25519": ed25519,
                        "curve25519": curve25519,
                        "trust_level": "unknown",
                    }));
                }
            }
        }
    }

    let cross_signing = json!({
        "master_keys": v.get("master_keys"),
        "self_signing_keys": v.get("self_signing_keys"),
        "user_signing_keys": v.get("user_signing_keys"),
    });

    json!({
        "queried_users": users,
        "device_keys": device_keys_out,
        "cross_signing": cross_signing,
    })
}

// ---------------------------------------------------------------------------
// timestamp helpers (no chrono dep here — keep it tiny)
// ---------------------------------------------------------------------------

fn rfc3339_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    unix_to_rfc3339(secs)
}

fn unix_to_rfc3339(secs: i64) -> String {
    // Convert Unix seconds → UTC RFC3339 with no external deps.
    // Algorithm: civil_from_days (Howard Hinnant).
    let days = secs.div_euclid(86_400);
    let secs_of_day = secs.rem_euclid(86_400);
    let hour = secs_of_day / 3600;
    let minute = (secs_of_day % 3600) / 60;
    let second = secs_of_day % 60;

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y, m, d, hour, minute, second
    )
}
