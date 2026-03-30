pub mod batched_sender;
pub mod compat;
pub mod config;
pub mod executor;
pub mod heartbeat;
pub mod identity;
pub mod matrix;
pub mod output;
pub mod retention;
pub mod session;
pub mod session_persist;
pub mod telemetry;
pub mod tmux;
pub mod trust;
pub mod webrtc;

use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;
use base64::Engine;
use config::WorkerRuntimeConfig;
use matrix::WorkerRoomOps;
use mxdx_matrix::MultiHsClient;
use mxdx_types::events::session::{
    OutputStream, SessionResult, SessionStart, SessionStatus, SessionTask,
};

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
    )
    .await?;

    tracing::info!(
        user_id = %multi.user_id(),
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

    let room_id = if let Some(ref direct_room_id) = config.room_id {
        // Use a specific room ID directly (for E2E tests or pre-arranged rooms)
        let rid = mxdx_matrix::OwnedRoomId::try_from(direct_room_id.as_str())
            .map_err(|e| anyhow::anyhow!("Invalid room ID '{}': {}", direct_room_id, e))?;
        // Sync to pick up any pending invites, then join the room
        multi.sync_once().await?;
        if let Err(e) = multi.join_room(&rid).await {
            tracing::warn!(room_id = %rid, error = %e, "join_room failed (may already be a member)");
        }
        // Wait for key exchange so we can decrypt E2EE events in this room
        tracing::info!(room_id = %rid, "waiting for E2EE key exchange");
        multi
            .wait_for_key_exchange(&rid, std::time::Duration::from_secs(15))
            .await?;
        tracing::info!(room_id = %rid, "using direct room ID");
        rid
    } else {
        let topology = multi
            .get_or_create_launcher_space(&config.resolved_room_name)
            .await?;
        topology.exec_room_id
    };

    tracing::info!(room_id = %room_id, "worker room ready");

    // Bootstrap cross-signing on all servers and sync trust across identities.
    // This ensures all identities are verified and trust is propagated so
    // failover to another server maintains the same trust relationships.
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
    tracing::info!(
        room = %config.resolved_room_name,
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

    // 8. Initialize WebRTC manager
    let _webrtc = webrtc::WebRtcManager::new();

    tracing::info!("worker initialized, ready for sessions");

    // If no accounts available, just initialize and return (for testing)
    if config.resolve_accounts().is_empty() {
        tracing::info!("no credentials provided, skipping Matrix connection");
        return Ok(());
    }

    // Connect to Matrix
    let mut room = connect(&config).await?;
    let room_id_str = room.room_id().to_string();

    // Post WorkerInfo state event
    let worker_info = telemetry.collect_info()?;
    room.write_state(
        &room_id_str,
        "org.mxdx.worker.info",
        &format!("worker/{}", identity.device_id()),
        serde_json::to_value(&worker_info)?,
    )
    .await?;
    tracing::info!("posted WorkerInfo state event");

    // Track active sessions and their thread root event IDs
    let mut thread_roots: HashMap<String, String> = HashMap::new();

    // Recover any orphaned sessions from a previous crash
    match session_persist::recover_sessions(None) {
        Ok(recovered) if !recovered.is_empty() => {
            tracing::info!(count = recovered.len(), "recovered orphaned sessions from disk");
            for s in &recovered {
                thread_roots.insert(s.uuid.clone(), s.thread_root.clone().unwrap_or_default());
            }
        }
        Ok(_) => {}
        Err(e) => {
            tracing::warn!(error = %e, "failed to recover sessions from disk");
        }
    }

    // Periodic trust sync (every ~60 sync cycles, ~30 minutes at 30s sync timeout)
    let mut sync_cycle_count: u64 = 0;
    let trust_sync_interval: u64 = 60;

    // Exponential backoff for sync failures
    let mut backoff = SyncBackoff::new();

    // Main sync loop
    loop {
        // Periodic cross-server trust sync (multi-homeserver only)
        sync_cycle_count += 1;
        if sync_cycle_count % trust_sync_interval == 0 {
            let rid = mxdx_matrix::RoomId::parse(&room_id_str).ok();
            if let Some(rid) = rid.as_ref() {
                room.multi().sync_trust(rid).await;
            }
        }
        let events = match room
            .sync_events(std::time::Duration::from_secs(30))
            .await
        {
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
        };

        for event in events {
            match event {
                matrix::IncomingEvent::TaskSubmission { event_id, content } => {
                    let task: SessionTask = serde_json::from_value(content)?;
                    tracing::info!(uuid = %task.uuid, bin = %task.bin, "received task");

                    // Validate command
                    let validated = executor::validate_command(
                        &task.bin,
                        &task.args,
                        task.env.as_ref(),
                        task.cwd.as_deref(),
                    )?;

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

                    tracing::info!(uuid = %task.uuid, "session started");
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

                tracing::info!(uuid = %uuid, exit_code = ?exit_code, "session completed");

                // Update persisted sessions after completion
                persist_active_sessions(&session_manager, &thread_roots);
            }
        }
    }
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
