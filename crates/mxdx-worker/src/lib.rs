pub mod compat;
pub mod config;
pub mod executor;
pub mod heartbeat;
pub mod identity;
pub mod matrix;
pub mod output;
pub mod retention;
pub mod session;
pub mod telemetry;
pub mod tmux;
pub mod trust;
pub mod webrtc;

use std::collections::HashMap;

use anyhow::Result;
use base64::Engine;
use config::WorkerRuntimeConfig;
use matrix::WorkerRoomOps;
use mxdx_matrix::MultiHsClient;
use mxdx_types::events::session::{
    OutputStream, SessionResult, SessionStart, SessionStatus, SessionTask,
};

/// Connect to Matrix and return the worker's room handle.
/// If `config.room_id` is set, uses that room directly (bypasses space creation).
/// Otherwise, logs in and finds/creates the launcher space.
///
/// Uses `MultiHsClient` for multi-homeserver failover. When only a single
/// account is configured, the client operates in single-server mode with
/// zero overhead (circuit breaker and deduplication are skipped).
pub async fn connect(config: &WorkerRuntimeConfig) -> Result<matrix::MatrixWorkerRoom> {
    let accounts = config.resolve_accounts();
    if accounts.is_empty() {
        anyhow::bail!(
            "No Matrix accounts configured (use --homeserver/--username/--password or config file)"
        );
    }

    let mut multi = MultiHsClient::connect(&accounts, None).await?;
    tracing::info!(
        user_id = %multi.user_id(),
        servers = multi.server_count(),
        preferred = %multi.preferred_server(),
        "connected to Matrix"
    );

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

    // Main sync loop
    loop {
        let events = room
            .sync_events(std::time::Duration::from_secs(30))
            .await?;

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

                // Determine exit code (tmux doesn't directly provide it, default to 0)
                let exit_code = Some(0i32);

                let completed =
                    session_manager.complete(&uuid, SessionStatus::Success, exit_code)?;

                // Post SessionResult
                if let Some(thread_root) = thread_roots.get(&uuid) {
                    let result = SessionResult {
                        session_uuid: uuid.clone(),
                        worker_id: identity.device_id().to_string(),
                        status: SessionStatus::Success,
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
            }
        }
    }
}
