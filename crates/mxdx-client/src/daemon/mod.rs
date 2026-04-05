pub mod handler;
pub mod sessions;
pub mod subscriptions;
pub mod transport;

use std::sync::Arc;
use std::time::Duration;
use tracing::{info, error};

use crate::config::ClientRuntimeConfig;
use handler::Handler;

/// Run the daemon for a given profile. This is the main entry point
/// called by `mxdx-client _daemon --profile <name>`.
pub async fn run_daemon(
    config: ClientRuntimeConfig,
    profile: &str,
) -> anyhow::Result<()> {
    // Write PID file (ensure daemon dir has restricted permissions)
    let pid_path = transport::unix::pid_path(profile);
    if let Some(parent) = pid_path.parent() {
        std::fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    std::fs::write(&pid_path, std::process::id().to_string())?;

    info!(profile, pid = std::process::id(), "starting daemon");

    // Create handler
    let handler = Arc::new(Handler::new(profile));

    // Start Unix socket transport
    let socket_path = transport::unix::socket_path(profile);
    let handler_clone = Arc::clone(&handler);
    tokio::spawn(async move {
        if let Err(e) = transport::unix::serve(&socket_path, handler_clone).await {
            error!(error = %e, "Unix socket transport failed");
        }
    });

    info!(profile, "daemon ready");

    // Idle timeout tracking
    let idle_timeout_secs = config.client.daemon.idle_timeout_seconds;

    // Main loop: check for shutdown signals and idle timeout
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info!("received SIGINT, shutting down");
                break;
            }
            _ = tokio::time::sleep(Duration::from_secs(10)) => {
                // Check idle timeout (0 = never auto-shutdown)
                if idle_timeout_secs > 0 {
                    let sessions = handler.sessions.lock().await;
                    if sessions.active_count() == 0 && handler.idle_seconds() > idle_timeout_secs {
                        info!("idle timeout reached, shutting down");
                        break;
                    }
                }
            }
        }
    }

    // Cleanup
    let _ = std::fs::remove_file(transport::unix::socket_path(profile));
    let _ = std::fs::remove_file(&pid_path);
    info!(profile, "daemon stopped");
    Ok(())
}
