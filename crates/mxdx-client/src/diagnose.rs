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

/// Local + server-side key-backup state gathered via pure REST + keychain (no matrix-sdk Client).
#[derive(Debug, Clone, serde::Serialize, Default)]
pub struct BackupReport {
    pub keychain_present: bool,
    pub server_has_version: bool,
    pub version: Option<String>,
    pub algorithm: Option<String>,
    pub error: Option<String>,
}

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
    /// If true, spawn a temporary matrix-sdk client and try to decrypt
    /// joined-room state events, embedding results into the report.
    pub decrypt: bool,
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
    let decrypt_requested = input.decrypt;
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

    // -------- backup (keychain + server-side REST) --------
    let unix_user = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "unknown".into());
    // Prefer the full user_id from collect_matrix; fall back to constructing one.
    let matrix_user_for_backup = user_id.clone().or_else(|| {
        match (resolved_username.as_deref(), resolved_homeserver.as_deref()) {
            (Some(u), Some(hs)) => {
                let server = hs
                    .trim_start_matches("https://")
                    .trim_start_matches("http://")
                    .trim_end_matches('/');
                let server = server.split('/').next().unwrap_or(server);
                let local = u.trim_start_matches('@').split(':').next().unwrap_or(u);
                Some(format!("@{}:{}", local, server))
            }
            _ => None,
        }
    });
    let backup_section = gather_backup_report(
        resolved_homeserver.as_deref().unwrap_or(""),
        access_token.as_deref(),
        matrix_user_for_backup.as_deref().unwrap_or(""),
        &unix_user,
    )
    .await;
    report.insert(
        "backup".into(),
        serde_json::to_value(&backup_section).unwrap_or(json!({"error": "serialization failed"})),
    );

    // -------- state_room (discovery metadata) --------
    //
    // Surfaces whether the worker's expected state-room alias resolves, and
    // whether the current account is a joined member of the target room.
    // Lock content itself is encrypted (MSC4362) and requires --decrypt.
    if let Some(hs) = resolved_homeserver.as_deref() {
        let base = {
            let b = hs.trim_end_matches('/');
            if b.starts_with("http") { b.to_string() } else { format!("https://{}", b) }
        };
        let state_room_section =
            collect_state_room(&http, &base, user_id.as_deref(), &joined_room_ids).await;
        report.insert("state_room".into(), state_room_section);
    }

    // engagement is partly populated inside collect_matrix already; keep a top-level summary
    report.insert(
        "engagement".into(),
        json!({
            "note": "see matrix.joined_rooms[].recent_sessions and recent_workers for live state",
        }),
    );

    let _ = login_device_id; // already in matrix section

    // -------- optional decrypt pass --------
    if decrypt_requested {
        match (
            resolved_homeserver.as_deref(),
            resolved_username.as_deref(),
            resolved_password.as_deref(),
        ) {
            (Some(hs), Some(user), Some(pw)) => {
                match decrypt_with_temp_client(hs, user, pw, &joined_room_ids).await {
                    Ok(map) => {
                        report.insert(
                            "decrypted_state".into(),
                            serde_json::to_value(&map).unwrap_or(Value::Null),
                        );
                    }
                    Err(e) => {
                        report.insert("decrypt_error".into(), json!(e.to_string()));
                    }
                }
            }
            _ => {
                report.insert(
                    "decrypt_error".into(),
                    json!("--decrypt requires homeserver, username, and password"),
                );
            }
        }
    }

    Value::Object(report)
}

/// Gather key-backup state without spawning a matrix-sdk Client.
///
/// * Checks the local OS keychain for a stored recovery key.
/// * If an access token is available, queries the server-side backup version via REST.
async fn gather_backup_report(
    homeserver: &str,
    access_token: Option<&str>,
    matrix_user: &str,
    unix_user: &str,
) -> BackupReport {
    let mut report = BackupReport::default();

    // --- local keychain probe ---
    if !homeserver.is_empty() && !matrix_user.is_empty() {
        match ChainedKeychain::default_chain() {
            Ok(keychain) => {
                let key = mxdx_types::identity::backup_keychain_key(homeserver, matrix_user, unix_user);
                match keychain.get(&key) {
                    Ok(Some(_)) => report.keychain_present = true,
                    Ok(None) => report.keychain_present = false,
                    Err(e) => {
                        report.error = Some(format!("keychain lookup failed: {e}"));
                    }
                }
            }
            Err(e) => {
                report.error = Some(format!("keychain init failed: {e}"));
            }
        }
    }

    // --- server-side backup version (REST) ---
    let Some(token) = access_token else {
        return report;
    };
    if homeserver.is_empty() {
        return report;
    }

    let url = format!(
        "{}/_matrix/client/v3/room_keys/version",
        homeserver.trim_end_matches('/')
    );

    let http = match reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            let msg = format!("http client build failed: {e}");
            report.error = Some(match report.error {
                Some(existing) => format!("{existing}; {msg}"),
                None => msg,
            });
            return report;
        }
    };

    match http
        .get(&url)
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
    {
        Ok(resp) => {
            let status = resp.status();
            if status.as_u16() == 404 {
                report.server_has_version = false;
            } else if status.is_success() {
                report.server_has_version = true;
                match resp.json::<serde_json::Value>().await {
                    Ok(body) => {
                        report.version = body
                            .get("version")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        report.algorithm = body
                            .get("algorithm")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                    }
                    Err(e) => {
                        let msg = format!("failed to parse backup version response: {e}");
                        report.error = Some(match report.error {
                            Some(existing) => format!("{existing}; {msg}"),
                            None => msg,
                        });
                    }
                }
            } else {
                let msg = format!("backup version endpoint returned HTTP {status}");
                report.error = Some(match report.error {
                    Some(existing) => format!("{existing}; {msg}"),
                    None => msg,
                });
            }
        }
        Err(e) => {
            let msg = format!("backup version request failed: {e}");
            report.error = Some(match report.error {
                Some(existing) => format!("{existing}; {msg}"),
                None => msg,
            });
        }
    }

    report
}

/// Best-effort decrypt of joined-room state events using a *temporary* matrix-sdk
/// client backed by a throwaway sqlite store. The temp store is removed on exit
/// so we never collide with the real daemon/worker crypto store.
///
/// Coverage is intentionally minimal — we only enumerate a handful of well-known
/// state event types via `Room::get_state_events(StateEventType)`. The infrastructure
/// (temp client, login, key download) is the load-bearing part; expanding the type
/// list later is trivial.
async fn decrypt_with_temp_client(
    homeserver: &str,
    matrix_user: &str,
    password: &str,
    rooms: &[String],
) -> anyhow::Result<std::collections::HashMap<String, serde_json::Value>> {
    use anyhow::Context;
    use mxdx_matrix::matrix_sdk::ruma::events::StateEventType;
    use mxdx_matrix::matrix_sdk::Client;

    let temp_dir = std::env::temp_dir().join(format!(
        "mxdx-diagnose-{}-{}",
        std::process::id(),
        rfc3339_now().replace(':', "-")
    ));
    std::fs::create_dir_all(&temp_dir)?;

    // Ensure the temp dir is cleaned up even on error paths.
    struct TempDirGuard(std::path::PathBuf);
    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _guard = TempDirGuard(temp_dir.clone());

    let client = Client::builder()
        .homeserver_url(homeserver)
        .sqlite_store(&temp_dir, None)
        .build()
        .await
        .context("diagnose temp client build failed")?;

    client
        .matrix_auth()
        .login_username(matrix_user, password)
        .device_id("mxdx-diagnose")
        .send()
        .await
        .context("diagnose temp client login failed")?;

    // One sync to populate the state store with current room state.
    let _ = client
        .sync_once(mxdx_matrix::matrix_sdk::config::SyncSettings::default())
        .await;

    // Best-effort: pull megolm session keys from server-side backup so MSC4362
    // encrypted state events can be decrypted by the SDK.
    if client.user_id().is_some() {
        if let Err(e) = mxdx_matrix::backup::ensure_backup(&client, false).await {
            tracing::warn!(error=%e, "diagnose: ensure_backup failed, continuing");
        }
        let _ = mxdx_matrix::backup::download_all_keys(&client).await;
        // Sync once more so freshly-downloaded keys can decrypt cached events.
        let _ = client
            .sync_once(mxdx_matrix::matrix_sdk::config::SyncSettings::default())
            .await;
    }

    // Known state event types we attempt to surface. Keep small — this is
    // diagnostic, not exhaustive. The `org.mxdx.worker.*` entries let the
    // decrypt pass surface the contents of the worker state room — most
    // importantly the `WORKER_STATE_LOCK` event so a user running diagnose
    // can see who currently holds the single-writer lease.
    let known_types: Vec<StateEventType> = vec![
        StateEventType::RoomEncryption,
        StateEventType::RoomName,
        StateEventType::RoomTopic,
        StateEventType::RoomCreate,
        StateEventType::RoomPowerLevels,
        StateEventType::RoomJoinRules,
        StateEventType::RoomHistoryVisibility,
        StateEventType::from("org.mxdx.worker.lock"),
        StateEventType::from("org.mxdx.worker.config"),
        StateEventType::from("org.mxdx.worker.topology"),
        StateEventType::from("org.mxdx.worker.identity"),
        StateEventType::from("org.mxdx.worker.session"),
    ];

    let mut decrypted: std::collections::HashMap<String, serde_json::Value> =
        std::collections::HashMap::new();

    for rid_str in rooms {
        let Ok(rid) = mxdx_matrix::matrix_sdk::ruma::RoomId::parse(rid_str) else {
            continue;
        };
        let Some(room) = client.get_room(&rid) else {
            continue;
        };
        for ev_type in &known_types {
            match room.get_state_events(ev_type.clone()).await {
                Ok(raws) => {
                    for raw in raws {
                        // Parse the raw JSON bytes — this is what the SDK has
                        // after decrypt (or the encrypted envelope if decrypt
                        // wasn't possible). Either way, valid JSON.
                        use mxdx_matrix::matrix_sdk::deserialized_responses::RawAnySyncOrStrippedState;
                        let raw_json = match &raw {
                            RawAnySyncOrStrippedState::Sync(r) => r.json().get(),
                            RawAnySyncOrStrippedState::Stripped(r) => r.json().get(),
                        };
                        let value: serde_json::Value =
                            match serde_json::from_str(raw_json) {
                                Ok(v) => v,
                                Err(e) => {
                                    tracing::debug!(error=%e, "diagnose decrypt: bad json");
                                    continue;
                                }
                            };
                        let state_key = value
                            .get("state_key")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let ev_type_str = serde_json::to_value(ev_type.clone())
                            .ok()
                            .and_then(|v| v.as_str().map(|s| s.to_string()))
                            .unwrap_or_else(|| format!("{:?}", ev_type));
                        let key =
                            format!("{}:{}:{}", rid_str, ev_type_str, state_key);
                        decrypted.insert(key, value);
                    }
                }
                Err(e) => {
                    tracing::debug!(room=%rid_str, error=%e, "diagnose decrypt: get_state_events failed");
                }
            }
        }
    }

    drop(client);
    Ok(decrypted)
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

/// Resolve a Matrix room alias to a room_id via the unauthenticated directory
/// endpoint. Returns `Ok(Some(room_id))` on success, `Ok(None)` on 404.
async fn resolve_alias(
    http: &reqwest::Client,
    base: &str,
    alias: &str,
) -> Option<String> {
    let url = format!(
        "{}/_matrix/client/v3/directory/room/{}",
        base,
        urlencoding::encode(alias)
    );
    let resp = http.get(&url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let v: Value = resp.json().await.ok()?;
    v.get("room_id").and_then(|r| r.as_str()).map(String::from)
}

/// Build the state-room diagnostics section.
///
/// Answers three questions:
///
/// 1. What alias does THIS host's worker expect for its state room?
/// 2. Does that alias resolve on the homeserver (i.e., does a state room
///    exist), and if so, to which room id?
/// 3. Is that resolved room present in our `joined_rooms` list? (If yes, we
///    can read its state; if no, we'd hit `M_FORBIDDEN` — the exact symptom
///    we saw before the state-room discovery fix.)
///
/// The contents of the lock / config / sessions state events are encrypted
/// under MSC4362. Reading them requires `--decrypt`; this section only
/// exposes discovery metadata, which is unencrypted (m.room.create is never
/// encrypted and the `directory/room` endpoint is unauthenticated).
async fn collect_state_room(
    http: &reqwest::Client,
    base: &str,
    user_id: Option<&str>,
    joined_room_ids: &[String],
) -> Value {
    let host = hostname::get()
        .map(|h| h.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "unknown".to_string());
    let os_user = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "unknown".into());
    let (localpart, server_name) = match user_id {
        Some(u) => {
            let trimmed = u.trim_start_matches('@');
            match trimmed.split_once(':') {
                Some((l, s)) => (l.to_string(), s.to_string()),
                None => (trimmed.to_string(), String::new()),
            }
        }
        None => (String::new(), String::new()),
    };

    if localpart.is_empty() || server_name.is_empty() {
        return json!({
            "expected_alias": null,
            "note": "cannot compute expected alias without matrix user_id",
            "host": host,
            "os_user": os_user,
        });
    }

    let alias = format!(
        "#mxdx-state-{host}.{os_user}.{localpart}:{server_name}"
    );
    let resolved = resolve_alias(http, base, &alias).await;
    let joined_set: std::collections::HashSet<&str> =
        joined_room_ids.iter().map(|s| s.as_str()).collect();
    let is_member = resolved
        .as_deref()
        .map(|r| joined_set.contains(r))
        .unwrap_or(false);

    json!({
        "expected_alias": alias,
        "alias_resolves": resolved.is_some(),
        "resolved_room_id": resolved,
        "is_joined_member": is_member,
        "host": host,
        "os_user": os_user,
        "localpart": localpart,
        "note": if resolved.is_some() && !is_member {
            "resolved to a room but not a joined member — state reads will return 403"
        } else if resolved.is_none() {
            "alias not bound — worker will create on next start"
        } else {
            "state room is reachable"
        }
    })
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
