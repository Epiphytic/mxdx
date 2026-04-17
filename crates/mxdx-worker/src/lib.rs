pub mod batched_sender;
pub mod compat;
pub mod config;
pub mod executor;
pub mod heartbeat;
pub mod identity;
pub mod matrix;
pub mod output;
pub mod p2p_integration;
pub mod retention;
pub mod session;
pub mod session_mux;
pub mod session_persist;
pub mod state_room;
pub mod telemetry;
pub mod tmux;
pub mod trust;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::Result;
use base64::Engine;
use config::WorkerRuntimeConfig;
use matrix::WorkerRoomOps;
use mxdx_matrix::MultiHsClient;
use mxdx_types::events::session::{
    OutputStream, SessionResult, SessionStart, SessionStatus, SessionTask,
};
use mxdx_types::events::telemetry::WORKER_TELEMETRY;
use state_room::WorkerStateRoom;

/// Parse a basic ISO 8601 timestamp (e.g., "2026-04-06T12:00:00Z") to epoch millis.
fn parse_iso8601(ts: &str) -> Result<u64> {
    // Parse "YYYY-MM-DDTHH:MM:SSZ" or "YYYY-MM-DDTHH:MM:SS.sssZ"
    let ts = ts.trim_end_matches('Z');
    let (date, time) = ts.split_once('T')
        .ok_or_else(|| anyhow::anyhow!("invalid ISO 8601 timestamp: {}", ts))?;
    let parts: Vec<&str> = date.split('-').collect();
    if parts.len() != 3 { anyhow::bail!("invalid date in timestamp"); }
    let year: u64 = parts[0].parse()?;
    let month: u64 = parts[1].parse()?;
    let day: u64 = parts[2].parse()?;

    let time_parts: Vec<&str> = time.split(':').collect();
    if time_parts.len() != 3 { anyhow::bail!("invalid time in timestamp"); }
    let hour: u64 = time_parts[0].parse()?;
    let min: u64 = time_parts[1].parse()?;
    let sec: u64 = time_parts[2].split('.').next().unwrap_or("0").parse()?;

    // Approximate days since epoch (good enough for staleness checks)
    let days = (year - 1970) * 365 + (year - 1969) / 4
        + [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334][(month - 1) as usize]
        + day - 1;
    let epoch_secs = days * 86400 + hour * 3600 + min * 60 + sec;
    Ok(epoch_secs * 1000)
}

/// Exponential backoff for sync failures.
pub struct SyncBackoff {
    current: Duration,
    min: Duration,
    max: Duration,
}

impl SyncBackoff {
    pub fn new() -> Self {
        Self {
            current: Duration::from_secs(1),
            min: Duration::from_secs(1),
            max: Duration::from_secs(30),
        }
    }

    /// Record a failure. Returns the duration to sleep before retrying.
    pub fn fail(&mut self) -> Duration {
        let wait = self.current;
        self.current = (self.current * 2).min(self.max);
        wait
    }

    /// Record a success. Resets backoff to minimum.
    pub fn success(&mut self) {
        self.current = self.min;
    }
}

impl Default for SyncBackoff {
    fn default() -> Self {
        Self::new()
    }
}

/// Connect to Matrix and return the worker's room handle.
/// If `config.room_id` is set, uses that room directly (bypasses space creation).
/// Otherwise, logs in and finds/creates the launcher space.
///
/// Uses `MultiHsClient` for multi-homeserver failover. When only a single
/// account is configured, the client operates in single-server mode with
/// zero overhead (circuit breaker and deduplication are skipped).
///
/// Session restore: When a keychain is available, tries to restore each server's
/// session (reusing the same device ID) before falling back to fresh login.
/// This avoids creating a new device on every restart.
pub async fn connect(config: &WorkerRuntimeConfig) -> Result<matrix::MatrixWorkerRoom> {
    let accounts = config.resolve_accounts();
    if accounts.is_empty() {
        anyhow::bail!(
            "No Matrix accounts configured (use --homeserver/--username/--password or config file)"
        );
    }

    // Create keychain for session restore (OS keychain -> file fallback)
    let keychain: Box<dyn mxdx_types::identity::KeychainBackend> =
        match mxdx_types::keychain_chain::ChainedKeychain::default_chain() {
            Ok(kc) => Box::new(kc),
            Err(e) => {
                tracing::warn!(error = %e, "failed to create keychain, session restore disabled");
                Box::new(mxdx_types::identity::InMemoryKeychain::new())
            }
        };

    let store_base = mxdx_matrix::default_store_base_path("worker");
    let (mut multi, fresh_logins) = MultiHsClient::connect_with_keychain(
        &accounts,
        None,
        store_base,
        Some(keychain.as_ref()),
        config.force_new_device,
    )
    .await?;

    tracing::info!(
        matrix_account = %multi.user_id(),
        servers = multi.server_count(),
        preferred = %multi.preferred_server(),
        fresh_logins = ?fresh_logins,
        "connected to Matrix"
    );

    // After fresh login, remove passwords from config (now saved in keychain)
    if fresh_logins.iter().any(|&f| f) {
        if let Err(e) = mxdx_types::config::remove_passwords_from_config("defaults.toml", None) {
            tracing::warn!(error = %e, "failed to remove passwords from config");
        }
    }

    let any_fresh = fresh_logins.iter().any(|&f| f);

    // Server-side megolm key backup. Run after login but before room
    // discovery so any keys recovered from backup are available when we
    // start decrypting room history.
    {
        use mxdx_matrix::backup::{download_all_keys, ensure_backup, BackupState};
        let sdk_client = multi.preferred().inner().clone();

        // Backup setup is best-effort. The worker still operates without it.
        // Pass is_first_run = false so ensure_backup always degrades gracefully.
        let backup_state = match ensure_backup(&sdk_client, false).await {
            Ok(state) => state,
            Err(e) => {
                tracing::warn!(error=%e, "backup setup failed; continuing without backup");
                BackupState {
                    enabled: false,
                    degraded: true,
                    error: Some(e.to_string()),
                    ..Default::default()
                }
            }
        };
        if backup_state.enabled {
            match download_all_keys(&sdk_client).await {
                Ok(n) => tracing::info!(rooms = n, "backup: room keys downloaded"),
                Err(e) => tracing::warn!(error=%e, "backup: download_all_keys failed"),
            }
        }
        tracing::info!(
            enabled = backup_state.enabled,
            degraded = backup_state.degraded,
            "backup state"
        );
    }

    let room_id = if let Some(ref direct_room_id) = config.room_id {
        // Use a specific room ID directly (for E2E tests or pre-arranged rooms)
        let rid = mxdx_matrix::OwnedRoomId::try_from(direct_room_id.as_str())
            .map_err(|e| anyhow::anyhow!("Invalid room ID '{}': {}", direct_room_id, e))?;

        if any_fresh {
            // Fresh login: need to sync, join, and exchange keys from scratch
            multi.sync_once().await?;
            if let Err(e) = multi.join_room(&rid).await {
                tracing::warn!(room_id = %rid, error = %e, "join_room failed (may already be a member)");
            }
            tracing::info!(room_id = %rid, "waiting for E2EE key exchange");
            multi
                .wait_for_key_exchange(&rid, std::time::Duration::from_secs(90))
                .await?;
        } else {
            // Session restore: device already has keys cached in persistent crypto store.
            // Just do a quick sync to catch up on any events missed while offline.
            multi.sync_once().await?;
        }
        tracing::info!(room_id = %rid, "using direct room ID");
        rid
    } else {
        let launcher_id = config.resolved_room_name.as_str();
        let homeserver = config
            .credentials
            .as_ref()
            .map(|c| c.homeserver.clone())
            .or_else(|| {
                config
                    .defaults
                    .accounts
                    .first()
                    .map(|a| a.homeserver.clone())
            })
            .ok_or_else(|| anyhow::anyhow!("no homeserver configured for REST discovery"))?;
        let access_token = multi
            .preferred()
            .access_token()
            .ok_or_else(|| anyhow::anyhow!("no access token available on preferred client"))?;

        let topology = match multi
            .preferred()
            .find_launcher_space_via_rest(launcher_id, &homeserver, &access_token)
            .await?
        {
            Some(t) => {
                tracing::info!(launcher_id, "discovered existing launcher space via REST");
                t
            }
            None => {
                tracing::info!(launcher_id, "no existing launcher space found; creating");
                multi.preferred().create_launcher_space(launcher_id).await?
            }
        };

        // Self-heal: verify exec/logs rooms are encrypted and have correct topology,
        // replacing any that are unencrypted or misconfigured.
        let topology = {
            use mxdx_matrix::reencrypt::verify_or_replace_topology;
            use mxdx_matrix::rest::RestClient;
            let rest = RestClient::new(&homeserver, &access_token);
            let mut authorized: Vec<mxdx_matrix::OwnedUserId> = Vec::new();
            for user_str in &config.worker.authorized_users {
                match mxdx_matrix::UserId::parse(user_str.as_str()) {
                    Ok(uid) => authorized.push(uid),
                    Err(e) => tracing::warn!(
                        user = %user_str, error = %e,
                        "invalid authorized user ID for reencrypt; skipping"
                    ),
                }
            }
            verify_or_replace_topology(
                multi.preferred(),
                &rest,
                topology,
                launcher_id,
                &authorized,
            )
            .await?
        };

        let rid = topology.exec_room_id.clone();

        if any_fresh {
            multi.sync_once().await?;
            if let Err(e) = multi.join_room(&rid).await {
                tracing::warn!(room_id = %rid, error = %e, "join_room failed (may already be a member)");
            }
            tracing::info!(room_id = %rid, "waiting for E2EE key exchange");
            multi
                .wait_for_key_exchange(&rid, std::time::Duration::from_secs(90))
                .await?;
        } else {
            multi.sync_once().await?;
        }

        // Invite authorized users to the space + all child rooms so clients
        // can discover the topology via find_launcher_space.
        for user_str in &config.worker.authorized_users {
            match mxdx_matrix::UserId::parse(user_str.as_str()) {
                Ok(uid) => {
                    for (label, room) in [
                        ("space", &topology.space_id),
                        ("exec", &topology.exec_room_id),
                        ("logs", &topology.logs_room_id),
                    ] {
                        if let Err(e) = multi.preferred().invite_user(room, &uid).await {
                            tracing::warn!(
                                user = %user_str, room = label,
                                error = %e,
                                "failed to invite authorized user (may already be a member)"
                            );
                        }
                    }
                    tracing::info!(user = %user_str, "invited authorized user to launcher rooms");
                }
                Err(e) => {
                    tracing::warn!(
                        user = %user_str, error = %e,
                        "invalid Matrix user ID for authorized user, skipping invite"
                    );
                }
            }
        }

        rid
    };

    tracing::info!(room_id = %room_id, matrix_account = %multi.user_id(), "worker room ready");

    // Bootstrap cross-signing: on fresh login this sets up keys;
    // on session restore this no-ops quickly (keys already exist).
    multi.bootstrap_and_sync_trust(&room_id).await;

    Ok(matrix::MatrixWorkerRoom::new(multi, room_id))
}

/// Run the worker with the given configuration.
/// This is the main entry point for the worker binary and npm launcher.
///
/// Initializes identity, trust, telemetry, and session management components,
/// then connects to Matrix and enters the main sync loop processing tasks,
/// cancellations, and session completions.
pub async fn run_worker(config: WorkerRuntimeConfig) -> Result<()> {
    let matrix_account = config
        .defaults
        .accounts
        .first()
        .map(|a| a.user_id.as_str())
        .unwrap_or("unknown");
    tracing::info!(
        room = %config.resolved_room_name,
        matrix_account = %matrix_account,
        "starting mxdx-worker"
    );

    // 1. Load identity from keychain (or create new)
    // For now, use InMemoryKeychain as placeholder — OS keychain integration comes later
    let user_id = config
        .defaults
        .accounts
        .first()
        .map(|a| a.user_id.as_str())
        .unwrap_or("@worker:localhost");
    let host = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "unknown".into());
    let os_user = whoami::username();

    let keychain = Box::new(mxdx_types::identity::InMemoryKeychain::new());
    let identity =
        identity::WorkerIdentity::load_or_create(keychain, user_id, &host, &os_user)?;
    tracing::info!(device_id = %identity.device_id(), "device identity loaded");

    // 2. Initialize trust store
    let trust_anchor = config.worker.trust_anchor.as_deref().unwrap_or(user_id);
    let trust_keychain = Box::new(mxdx_types::identity::InMemoryKeychain::new());
    let _trust =
        trust::WorkerTrust::load_or_create(trust_keychain, user_id, trust_anchor)?;
    tracing::info!(anchor = %trust_anchor, "trust store initialized");

    // 3. Initialize telemetry collector
    let telemetry = telemetry::TelemetryCollector::new(
        identity.device_id().to_string(),
        config.worker.telemetry_refresh_seconds,
        config.worker.capabilities.extra.clone(),
    );
    let info = telemetry.collect_info()?;
    tracing::info!(
        host = %info.host,
        cpus = info.cpu_count,
        memory_mb = info.memory_total_mb,
        tools = info.tools.len(),
        "worker info collected"
    );

    // 4. Initialize session manager
    let mut session_manager = session::SessionManager::new(identity.device_id().to_string());

    // 5. Initialize output router
    let _output_router = output::OutputRouter::new(false);

    // 6. Initialize heartbeat poster
    let _heartbeat = heartbeat::HeartbeatPoster::new(30);

    // 7. Initialize retention sweeper
    let _retention = retention::RetentionSweeper::new(config.worker.history_retention);

    // 8. P2P transport (mxdx-p2p crate) is wired in Phase 6.
    // Until then, interactive sessions fall back to Matrix-room E2EE only.

    // 9. Initialize session mux for interactive DM routing
    let mut session_mux = session_mux::SessionMux::new();

    tracing::info!("worker initialized, ready for sessions");

    // If no accounts available, just initialize and return (for testing)
    if config.resolve_accounts().is_empty() {
        tracing::info!("no credentials provided, skipping Matrix connection");
        return Ok(());
    }

    // Connect to Matrix
    let mut room = connect(&config).await?;
    let room_id_str = room.room_id().to_string();

    // Post WorkerInfo state event (do this before entering the sync loop
    // so other participants can see the worker is online)
    let worker_info = telemetry.collect_info()?;
    room.write_state(
        &room_id_str,
        "org.mxdx.worker.info",
        &format!("worker/{}", identity.device_id()),
        serde_json::to_value(&worker_info)?,
    )
    .await?;
    tracing::info!("posted WorkerInfo state event");

    // Telemetry state key: unique per worker name
    let telemetry_state_key = format!("worker/{}", config.resolved_room_name);

    // Check for competing worker instance before claiming the name.
    // If another worker with a different UUID has a non-timed-out telemetry
    // event for this name, refuse to start.
    {
        let existing = room.read_state(
            &room_id_str,
            WORKER_TELEMETRY,
            &telemetry_state_key,
        ).await;
        if let Ok(Some(state_json)) = existing {
            if let Ok(existing_state) = serde_json::from_value::<mxdx_types::events::telemetry::WorkerTelemetryState>(state_json) {
                if existing_state.status == "online" {
                    if let Some(ref existing_uuid) = existing_state.worker_uuid {
                        if existing_uuid != telemetry.worker_uuid() {
                            // Check if the existing worker is still alive (within 2x heartbeat)
                            if let Ok(existing_ts) = parse_iso8601(&existing_state.timestamp) {
                                let now = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_millis() as u64;
                                let stale_threshold = existing_state.heartbeat_interval_ms * 2;
                                let age_ms = now.saturating_sub(existing_ts);
                                if age_ms < stale_threshold {
                                    anyhow::bail!(
                                        "Another worker instance owns '{}' (uuid={}, last seen {}ms ago). \
                                         Wait for it to shut down or time out before starting a new instance.",
                                        config.resolved_room_name,
                                        existing_uuid,
                                        age_ms,
                                    );
                                }
                                tracing::info!(
                                    existing_uuid,
                                    age_ms,
                                    "previous worker instance is stale, taking over"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    // Post initial telemetry (status: "online") immediately after WorkerInfo
    let initial_telemetry = telemetry.collect_telemetry_state(0, "online")?;
    room.write_state(
        &room_id_str,
        WORKER_TELEMETRY,
        &telemetry_state_key,
        serde_json::to_value(&initial_telemetry)?,
    )
    .await?;
    tracing::info!(
        worker_uuid = %telemetry.worker_uuid(),
        state_key = %telemetry_state_key,
        "posted initial telemetry (online)"
    );
    // Signal full readiness — synced, keys shared, telemetry posted.
    // Tests and orchestrators can watch for this log line.
    tracing::info!(
        matrix_account = %user_id,
        room_id = %room_id_str,
        "MXDX_WORKER_READY: worker fully started, synced, and accepting tasks"
    );
    let mut last_telemetry = Instant::now();
    let telemetry_interval = Duration::from_secs(config.worker.telemetry_refresh_seconds);

    // Track active sessions and their thread root event IDs
    let mut thread_roots: HashMap<String, String> = HashMap::new();

    // State room setup is deferred to after the first sync iteration to avoid
    // consuming timeline events during room discovery (sync_once calls in
    // find_worker_state_room / get_or_create would eat events meant for the
    // main loop). The worker can process tasks before state room is ready.
    let localpart = user_id
        .split(':')
        .next()
        .unwrap_or(user_id)
        .trim_start_matches('@')
        .to_string();
    let mut state_room_ready = false;
    let mut state_room: Option<WorkerStateRoom> = None;
    // Real, persistent keychain for state room discovery cache. Without this,
    // every worker start looks like a fresh account to `WorkerStateRoom::
    // get_or_create` and falls back to alias resolution, which in turn fails
    // when the server-side alias is bound to an older room we can no longer
    // validate. The file/OS keychain lets a restarted worker pick up its own
    // previously-recorded room id in O(1).
    let state_room_keychain: Box<dyn mxdx_types::identity::KeychainBackend> =
        match mxdx_types::keychain_chain::ChainedKeychain::default_chain() {
            Ok(kc) => Box::new(kc),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "state room keychain unavailable, using in-memory fallback"
                );
                Box::new(mxdx_types::identity::InMemoryKeychain::new())
            }
        };

    // Single-writer lock TTL matches the client's liveness rule: a worker is
    // considered dead after two missed telemetry refreshes. Graceful shutdown
    // releases the lock explicitly so a replacement can take over immediately.
    let lock_ttl_ms: u64 = config.worker.telemetry_refresh_seconds.saturating_mul(2) * 1000;
    // Did we successfully take the lock? If not, we skipped renewal entirely
    // and must not issue a release on shutdown (we don't own it).
    let mut lock_held = false;

    // Periodic trust sync (every ~60 sync cycles, ~30 minutes at 30s sync timeout)
    let mut sync_cycle_count: u64 = 0;
    let trust_sync_interval: u64 = 60;

    // Exponential backoff for sync failures
    let mut backoff = SyncBackoff::new();

    // Set up shutdown signal handler (SIGTERM on Unix)
    #[cfg(unix)]
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("failed to install SIGTERM handler");
    #[cfg(not(unix))]
    let mut sigterm = ();

    // Main sync loop
    loop {
        // Deferred state room setup — runs once after the sync loop starts
        // so that room discovery syncs don't consume task events.
        if !state_room_ready {
            state_room_ready = true;
            match WorkerStateRoom::get_or_create(
                room.client(),
                &host,
                &os_user,
                &localpart,
                state_room_keychain.as_ref(),
                &[],
            )
            .await
            {
                Ok(sr) => {
                    tracing::info!(state_room_id = %sr.room_id(), "state room ready");

                    // Single-writer lock: refuse to start if another live
                    // worker holds the lease. TTL is 2× telemetry refresh,
                    // matching the client's liveness detection rule.
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);
                    let lock = mxdx_types::events::state_room::WorkerStateLock {
                        device_id: identity.device_id().to_string(),
                        worker_uuid: telemetry.worker_uuid().to_string(),
                        host: host.clone(),
                        os_user: os_user.clone(),
                        acquired_at: now_ms,
                        expires_at: now_ms + lock_ttl_ms,
                    };
                    match sr.try_acquire_lock(room.client(), &lock, now_ms).await {
                        Ok(true) => {
                            tracing::info!(
                                device_id = %identity.device_id(),
                                ttl_ms = lock_ttl_ms,
                                "acquired state room lock"
                            );
                            lock_held = true;
                        }
                        Ok(false) => {
                            // A live peer holds the lock. Do not proceed —
                            // running two workers in the same state room is
                            // exactly the condition this lock exists to
                            // prevent, and races between them were the root
                            // cause of the "Unauthorized sender" test flakes.
                            anyhow::bail!(
                                "another worker holds the state room lock; \
                                 refusing to start a second instance"
                            );
                        }
                        Err(e) => {
                            // Couldn't read/write the lock event — log and
                            // continue so we don't regress availability for
                            // single-worker deployments if, e.g., the state
                            // room is temporarily unreachable.
                            tracing::warn!(
                                error = %e,
                                "state room lock acquire failed, continuing without lock"
                            );
                        }
                    }

                    // Recover sessions from state room
                    if let Ok(recovered) = sr.read_sessions(room.client()).await {
                        for s in &recovered {
                            thread_roots.insert(s.uuid.clone(), s.thread_root.clone());
                        }
                        if !recovered.is_empty() {
                            tracing::info!(count = recovered.len(), "recovered sessions from state room");
                        }
                    }

                    // Write topology
                    let topo = mxdx_types::events::state_room::StateRoomTopology {
                        space_id: String::new(),
                        exec_room_id: room_id_str.clone(),
                        logs_room_id: String::new(),
                    };
                    let _ = sr.write_topology(room.client(), &topo).await;

                    // Advertise in exec room
                    let _ = sr
                        .advertise_in_exec_room(
                            room.client(),
                            room.room_id(),
                            identity.device_id(),
                            &host,
                            &os_user,
                        )
                        .await;

                    state_room = Some(sr);
                }
                Err(e) => {
                    tracing::warn!(error = %e, "state room setup failed, continuing without it");
                }
            }
        }

        // Periodic telemetry refresh
        if last_telemetry.elapsed() >= telemetry_interval {
            let active_count = session_manager.active_sessions().len() as u32;
            match telemetry.collect_telemetry_state(active_count, "online") {
                Ok(state) => {
                    if let Err(e) = room
                        .write_state(
                            &room_id_str,
                            WORKER_TELEMETRY,
                            &telemetry_state_key,
                            serde_json::to_value(&state)?,
                        )
                        .await
                    {
                        tracing::warn!(error = %e, "failed to post periodic telemetry");
                    } else {
                        tracing::debug!("posted periodic telemetry");
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "failed to collect telemetry state");
                }
            }

            // Renew the single-writer lock on the same cadence as telemetry.
            // This keeps `expires_at` ahead of any client/peer consulting it,
            // so as long as this worker is alive nobody else can take over.
            if lock_held {
                if let Some(ref sr) = state_room {
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);
                    let lock = mxdx_types::events::state_room::WorkerStateLock {
                        device_id: identity.device_id().to_string(),
                        worker_uuid: telemetry.worker_uuid().to_string(),
                        host: host.clone(),
                        os_user: os_user.clone(),
                        acquired_at: now_ms,
                        expires_at: now_ms + lock_ttl_ms,
                    };
                    if let Err(e) = sr.renew_lock(room.client(), &lock).await {
                        tracing::warn!(error = %e, "failed to renew state room lock");
                    }
                }
            }

            last_telemetry = Instant::now();
        }

        // Periodic cross-server trust sync (multi-homeserver only)
        sync_cycle_count += 1;
        if sync_cycle_count % trust_sync_interval == 0 {
            let rid = mxdx_matrix::RoomId::parse(&room_id_str).ok();
            if let Some(rid) = rid.as_ref() {
                room.multi().sync_trust(rid).await;
            }
        }

        // Sync events with shutdown signal handling via tokio::select!
        let events = tokio::select! {
            result = room.sync_events(std::time::Duration::from_secs(30)) => {
                match result {
                    Ok(events) => {
                        backoff.success();
                        events
                    }
                    Err(e) => {
                        let wait = backoff.fail();
                        tracing::error!(
                            error = %e,
                            backoff_ms = wait.as_millis(),
                            "sync error, backing off"
                        );
                        tokio::time::sleep(wait).await;
                        continue;
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("received SIGINT, shutting down gracefully");
                post_offline_telemetry(&telemetry, &mut room, &room_id_str, &telemetry_state_key, &session_manager).await;
                if lock_held {
                    if let Some(ref sr) = state_room {
                        if let Err(e) = sr.release_lock(room.client()).await {
                            tracing::warn!(error = %e, "failed to release state room lock on SIGINT");
                        } else {
                            tracing::info!("released state room lock");
                        }
                    }
                }
                break;
            }
            _ = sigterm_recv(&mut sigterm) => {
                tracing::info!("received SIGTERM, shutting down gracefully");
                post_offline_telemetry(&telemetry, &mut room, &room_id_str, &telemetry_state_key, &session_manager).await;
                if lock_held {
                    if let Some(ref sr) = state_room {
                        if let Err(e) = sr.release_lock(room.client()).await {
                            tracing::warn!(error = %e, "failed to release state room lock on SIGTERM");
                        } else {
                            tracing::info!("released state room lock");
                        }
                    }
                }
                break;
            }
        };

        for event in events {
            match event {
                matrix::IncomingEvent::TaskSubmission { event_id, content } => {
                    let task: SessionTask = serde_json::from_value(content)?;
                    tracing::info!(uuid = %task.uuid, bin = %task.bin, sender = %task.sender_id, "received task");

                    // Check authorized users (empty list = allow all)
                    if !config.worker.authorized_users.is_empty()
                        && !config
                            .worker
                            .authorized_users
                            .iter()
                            .any(|u| u == &task.sender_id)
                    {
                        tracing::warn!(
                            uuid = %task.uuid,
                            sender = %task.sender_id,
                            authorized = ?config.worker.authorized_users,
                            "unauthorized sender, rejecting task"
                        );
                        let result = SessionResult {
                            session_uuid: task.uuid.clone(),
                            worker_id: identity.device_id().to_string(),
                            status: SessionStatus::Failed,
                            exit_code: Some(1),
                            duration_seconds: 0,
                            tail: Some("Unauthorized sender".to_string()),
                        };
                        room.post_to_thread(
                            &room_id_str,
                            &event_id,
                            mxdx_types::events::session::SESSION_RESULT,
                            serde_json::to_value(&result)?,
                        )
                        .await?;
                        continue;
                    }

                    // Check session limit
                    let active_count = session_manager.active_sessions().len();
                    if active_count >= config.worker.max_sessions as usize {
                        tracing::warn!(
                            uuid = %task.uuid,
                            active = active_count,
                            max = config.worker.max_sessions,
                            "session limit reached, rejecting task"
                        );
                        let result = SessionResult {
                            session_uuid: task.uuid.clone(),
                            worker_id: identity.device_id().to_string(),
                            status: SessionStatus::Failed,
                            exit_code: Some(1),
                            duration_seconds: 0,
                            tail: Some(format!(
                                "Session limit reached ({} max)",
                                config.worker.max_sessions
                            )),
                        };
                        room.post_to_thread(
                            &room_id_str,
                            &event_id,
                            mxdx_types::events::session::SESSION_RESULT,
                            serde_json::to_value(&result)?,
                        )
                        .await?;
                        continue;
                    }

                    // Validate command (including allowlists)
                    let validated = match executor::validate_command(
                        &task.bin,
                        &task.args,
                        task.env.as_ref(),
                        task.cwd.as_deref(),
                        &config.worker.allowed_commands,
                        &config.worker.allowed_cwd,
                    ) {
                        Ok(v) => v,
                        Err(e) => {
                            tracing::warn!(
                                uuid = %task.uuid,
                                error = %e,
                                "command validation failed, rejecting task"
                            );
                            let result = SessionResult {
                                session_uuid: task.uuid.clone(),
                                worker_id: identity.device_id().to_string(),
                                status: SessionStatus::Failed,
                                exit_code: Some(1),
                                duration_seconds: 0,
                                tail: Some(format!("Command validation failed: {}", e)),
                            };
                            room.post_to_thread(
                                &room_id_str,
                                &event_id,
                                mxdx_types::events::session::SESSION_RESULT,
                                serde_json::to_value(&result)?,
                            )
                            .await?;
                            continue;
                        }
                    };

                    // [duplicate task guard] Matrix redelivers events after
                    // reconnect and resync, so the same task UUID can arrive
                    // more than once. Check both active sessions AND the
                    // thread_roots map (which persists after completion).
                    if session_manager.contains_session(&task.uuid)
                        || thread_roots.contains_key(&task.uuid)
                    {
                        tracing::debug!(
                            uuid = %task.uuid,
                            "ignoring duplicate task event (already seen)"
                        );
                        continue;
                    }

                    // Claim session
                    let active_state = session_manager.claim(task.clone())?;
                    room.write_state(
                        &room_id_str,
                        "org.mxdx.session.active",
                        &format!("session/{}/active", task.uuid),
                        serde_json::to_value(&active_state)?,
                    )
                    .await?;

                    // Post SessionStart
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)?
                        .as_secs();
                    let start = SessionStart {
                        session_uuid: task.uuid.clone(),
                        worker_id: identity.device_id().to_string(),
                        tmux_session: Some(task.uuid.clone()),
                        pid: None,
                        started_at: now,
                        dm_room_id: None, // Set by interactive handler below
                    };
                    room.post_to_thread(
                        &room_id_str,
                        &event_id,
                        mxdx_types::events::session::SESSION_START,
                        serde_json::to_value(&start)?,
                    )
                    .await?;

                    thread_roots.insert(task.uuid.clone(), event_id.clone());

                    // Execute via tmux
                    let tmux = tmux::TmuxSession::create(
                        &task.uuid,
                        &validated.bin,
                        &validated.args,
                        validated.cwd.as_deref(),
                        &validated.env,
                    )
                    .await?;
                    session_manager.mark_running(&task.uuid, None, tmux)?;

                    // Persist active sessions to disk for crash recovery
                    persist_active_sessions(&session_manager, &thread_roots);

                    // Interactive session handling: if the task is interactive,
                    // create a DM room for terminal I/O and register with the mux.
                    if task.interactive {
                        tracing::info!(
                            uuid = %task.uuid,
                            sender = %task.sender_id,
                            "interactive session detected, DM room creation pending"
                        );
                        // TODO: Create E2EE DM room with task.sender_id via
                        // room.create_terminal_session_dm() and register in session_mux.
                        // Then set up background PTY bridge task.
                        // For now, register a placeholder so the mux knows about
                        // the session.
                        session_mux.add_session(
                            &task.uuid,
                            &room_id_str, // placeholder — will be DM room_id
                            &task.sender_id,
                        );
                    }

                    tracing::info!(uuid = %task.uuid, interactive = task.interactive, "session started");
                }
                matrix::IncomingEvent::SessionCancel {
                    session_uuid,
                    content: _,
                } => {
                    tracing::info!(uuid = %session_uuid, "received cancel");
                    if let Some(session) = session_manager.get_mut(&session_uuid) {
                        if let Some(ref tmux) = session.tmux {
                            tmux.kill().await?;
                        }
                    }
                    // Completion will be handled in the check-completed loop below
                }
                _ => {}
            }
        }

        // Check for completed sessions
        let active_uuids: Vec<String> = session_manager
            .active_sessions()
            .iter()
            .map(|s| s.uuid.clone())
            .collect();

        for uuid in active_uuids {
            let is_dead = {
                let session = session_manager.get(&uuid).unwrap();
                if let Some(ref tmux) = session.tmux {
                    !tmux.is_alive().await?
                } else {
                    false
                }
            };

            if is_dead {
                // Capture final output before completing
                let final_output = {
                    let session = session_manager.get(&uuid).unwrap();
                    if let Some(ref tmux) = session.tmux {
                        tmux.capture_pane().await.unwrap_or_default()
                    } else {
                        String::new()
                    }
                };

                // Post final output if non-empty
                if !final_output.trim().is_empty() {
                    if let Some(thread_root) = thread_roots.get(&uuid) {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)?
                            .as_secs();
                        let output_evt = mxdx_types::events::session::SessionOutput {
                            session_uuid: uuid.clone(),
                            worker_id: identity.device_id().to_string(),
                            stream: OutputStream::Stdout,
                            data: base64::engine::general_purpose::STANDARD
                                .encode(final_output.as_bytes()),
                            seq: 0,
                            timestamp: now,
                        };
                        room.post_to_thread(
                            &room_id_str,
                            thread_root,
                            mxdx_types::events::session::SESSION_OUTPUT,
                            serde_json::to_value(&output_evt)?,
                        )
                        .await?;
                    }
                }

                // Read exit code from the wrapper shell's exit code file
                let exit_code = {
                    let session = session_manager.get(&uuid).unwrap();
                    session
                        .tmux
                        .as_ref()
                        .and_then(|t| t.read_exit_code())
                        .or(Some(0))
                };

                let status = if exit_code == Some(0) {
                    SessionStatus::Success
                } else {
                    SessionStatus::Failed
                };
                let completed =
                    session_manager.complete(&uuid, status.clone(), exit_code)?;

                // Post SessionResult
                if let Some(thread_root) = thread_roots.get(&uuid) {
                    let result = SessionResult {
                        session_uuid: uuid.clone(),
                        worker_id: identity.device_id().to_string(),
                        status,
                        exit_code,
                        duration_seconds: completed.duration_seconds,
                        tail: Some(final_output.chars().take(1024).collect()),
                    };
                    room.post_to_thread(
                        &room_id_str,
                        thread_root,
                        mxdx_types::events::session::SESSION_RESULT,
                        serde_json::to_value(&result)?,
                    )
                    .await?;
                }

                // Write completed state, remove active state
                room.write_state(
                    &room_id_str,
                    "org.mxdx.session.completed",
                    &format!("session/{uuid}/completed"),
                    serde_json::to_value(&completed)?,
                )
                .await?;
                room.remove_state(
                    &room_id_str,
                    "org.mxdx.session.active",
                    &format!("session/{uuid}/active"),
                )
                .await?;

                // Remove from session mux if registered
                session_mux.remove_session(&uuid);

                tracing::info!(uuid = %uuid, exit_code = ?exit_code, "session completed");

                // Update persisted sessions after completion
                persist_active_sessions(&session_manager, &thread_roots);
            }
        }
    }

    tracing::info!("worker shut down cleanly");
    Ok(())
}

/// Post offline telemetry state with a 2-second timeout.
/// Best-effort: logs a warning on failure but does not propagate errors.
async fn post_offline_telemetry(
    telemetry: &telemetry::TelemetryCollector,
    room: &mut matrix::MatrixWorkerRoom,
    room_id_str: &str,
    telemetry_state_key: &str,
    session_manager: &session::SessionManager,
) {
    let active_count = session_manager.active_sessions().len() as u32;
    let state = match telemetry.collect_telemetry_state(active_count, "offline") {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "failed to collect offline telemetry");
            return;
        }
    };
    let value = match serde_json::to_value(&state) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "failed to serialize offline telemetry");
            return;
        }
    };
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        room.write_state(room_id_str, WORKER_TELEMETRY, telemetry_state_key, value),
    )
    .await;
    match result {
        Ok(Ok(())) => tracing::info!("posted offline telemetry"),
        Ok(Err(e)) => tracing::warn!(error = %e, "failed to post offline telemetry"),
        Err(_) => tracing::warn!("offline telemetry post timed out (2s)"),
    }
}

/// Platform-specific SIGTERM receiver.
/// On Unix, waits for SIGTERM. On other platforms, pends forever (ctrl_c handles shutdown).
#[cfg(unix)]
async fn sigterm_recv(signal: &mut tokio::signal::unix::Signal) {
    signal.recv().await;
}

#[cfg(not(unix))]
async fn sigterm_recv(_signal: &mut ()) {
    // On non-Unix, this future never completes; ctrl_c handles shutdown
    std::future::pending::<()>().await;
}

/// Persist the current active sessions to disk for crash recovery.
fn persist_active_sessions(
    session_manager: &session::SessionManager,
    thread_roots: &HashMap<String, String>,
) {
    let active = session_manager.active_sessions();
    let persisted: Vec<session_persist::PersistedSession> = active
        .iter()
        .map(|s| session_persist::PersistedSession {
            uuid: s.uuid.clone(),
            tmux_session: s.uuid.clone(),
            bin: s.task.bin.clone(),
            args: s.task.args.clone(),
            started_at: format!("{}",  s.started_at),
            thread_root: thread_roots.get(&s.uuid).cloned(),
        })
        .collect();

    if persisted.is_empty() {
        if let Err(e) = session_persist::clear_sessions(None) {
            tracing::warn!(error = %e, "failed to clear sessions file");
        }
    } else if let Err(e) = session_persist::save_sessions(&persisted, None) {
        tracing::warn!(error = %e, "failed to persist sessions to disk");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sync_backoff_doubles() {
        let mut backoff = SyncBackoff::new();
        assert_eq!(backoff.fail(), Duration::from_secs(1));
        assert_eq!(backoff.fail(), Duration::from_secs(2));
        assert_eq!(backoff.fail(), Duration::from_secs(4));
        assert_eq!(backoff.fail(), Duration::from_secs(8));
        assert_eq!(backoff.fail(), Duration::from_secs(16));
        // Capped at 30s
        assert_eq!(backoff.fail(), Duration::from_secs(30));
        assert_eq!(backoff.fail(), Duration::from_secs(30));
    }

    #[test]
    fn test_sync_backoff_resets_on_success() {
        let mut backoff = SyncBackoff::new();
        // Fail a few times
        backoff.fail(); // 1s
        backoff.fail(); // 2s
        backoff.fail(); // 4s

        // Success resets
        backoff.success();

        // Next fail should be back to 1s
        assert_eq!(backoff.fail(), Duration::from_secs(1));
    }
}
