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

use anyhow::Result;
use config::WorkerRuntimeConfig;

/// Run the worker with the given configuration.
/// This is the main entry point for the worker binary and npm launcher.
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
    let _session_manager = session::SessionManager::new(identity.device_id().to_string());

    // 5. Initialize output router
    let _output_router = output::OutputRouter::new(false);

    // 6. Initialize heartbeat poster
    let _heartbeat = heartbeat::HeartbeatPoster::new(30);

    // 7. Initialize retention sweeper
    let _retention = retention::RetentionSweeper::new(config.worker.history_retention);

    // 8. Initialize WebRTC manager
    let _webrtc = webrtc::WebRtcManager::new();

    tracing::info!("worker initialized, ready for sessions");

    // Main event loop will be connected to Matrix sync in integration.
    // For now, the worker starts up, initializes all components, and exits.
    // Full Matrix sync loop requires a running homeserver.

    Ok(())
}
