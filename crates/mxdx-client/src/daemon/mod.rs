pub mod handler;
pub mod sessions;
pub mod subscriptions;
pub mod transport;

use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn, error};

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

    // Start Unix socket transport FIRST so clients can connect immediately.
    // Matrix connection happens in the background; session.run waits for it.
    let socket_path = transport::unix::socket_path(profile);
    let handler_clone = Arc::clone(&handler);
    tokio::spawn(async move {
        if let Err(e) = transport::unix::serve(&socket_path, handler_clone).await {
            error!(error = %e, "Unix socket transport failed");
        }
    });

    info!(profile, "daemon socket ready, connecting to Matrix...");

    // Connect to Matrix in the background — handler.set_matrix() signals readiness
    let accounts = config.resolve_accounts();
    if accounts.is_empty() {
        warn!("no Matrix accounts configured — session commands will fail until connected");
    } else {
        let default_room = config.client.default_worker_room.clone()
            .unwrap_or_else(|| "default".to_string());
        let handler_mx = Arc::clone(&handler);
        let force_new = config.force_new_device;
        tokio::spawn(async move {
            match crate::matrix::connect_multi(
                &accounts,
                &default_room,
                None,
                force_new,
            ).await {
                Ok(mx_room) => {
                    info!(room_id = %mx_room.room_id(), "daemon connected to Matrix");

                    // Server-side megolm key backup. Mirrors the worker's setup so
                    // recovered keys are available before serving session commands.
                    // Clients never reencrypt — only ensure_backup + download_all_keys.
                    {
                        use mxdx_matrix::backup::{download_all_keys, ensure_backup, BackupState};
                        use mxdx_types::identity::backup_keychain_key;

                        // Build a fresh keychain (same chain used by connect_multi).
                        let keychain: Box<dyn mxdx_types::identity::KeychainBackend> =
                            match mxdx_types::keychain_chain::ChainedKeychain::default_chain() {
                                Ok(kc) => Box::new(kc),
                                Err(e) => {
                                    warn!(error = %e, "failed to create keychain for backup, using in-memory");
                                    Box::new(mxdx_types::identity::InMemoryKeychain::new())
                                }
                            };

                        let unix_user = std::env::var("USER")
                            .or_else(|_| std::env::var("LOGNAME"))
                            .unwrap_or_else(|_| "unknown".into());
                        let sdk_client = mx_room.client().inner().clone();
                        let server = sdk_client.homeserver().to_string();

                        match sdk_client.user_id().map(|u| u.to_owned()) {
                            None => {
                                warn!("backup: no user_id after login, skipping");
                            }
                            Some(matrix_user) => {
                                // Keychain-presence fallback: MatrixClientRoom doesn't
                                // expose fresh_logins, so derive is_first_run from
                                // whether a recovery key is already stored.
                                let kc_key = backup_keychain_key(
                                    &server,
                                    matrix_user.as_str(),
                                    &unix_user,
                                );
                                let is_first_run = match keychain.get(&kc_key) {
                                    Ok(Some(_)) => false,
                                    Ok(None) => true,
                                    Err(e) => {
                                        warn!(error = %e, "keychain lookup failed; assuming first run");
                                        true
                                    }
                                };

                                let backup_state = match ensure_backup(
                                    &sdk_client,
                                    keychain.as_ref(),
                                    &server,
                                    &matrix_user,
                                    &unix_user,
                                    is_first_run,
                                )
                                .await
                                {
                                    Ok(state) => state,
                                    Err(e) if is_first_run => {
                                        error!(error = %e, "backup setup failed (first run); session commands may fail to decrypt history");
                                        BackupState {
                                            enabled: false,
                                            degraded: true,
                                            error: Some(e.to_string()),
                                            ..Default::default()
                                        }
                                    }
                                    Err(e) => {
                                        warn!(error = %e, "backup setup failed (subsequent run); continuing degraded");
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
                                        Ok(n) => info!(rooms = n, "backup: room keys downloaded"),
                                        Err(e) => warn!(error = %e, "backup: download_all_keys failed"),
                                    }
                                }
                                info!(
                                    enabled = backup_state.enabled,
                                    degraded = backup_state.degraded,
                                    "backup state"
                                );
                            }
                        }
                    }

                    handler_mx.set_matrix(mx_room).await;
                    // Signal full readiness — Matrix connected, synced, backup
                    // attempted, sync loop running. Tests and orchestrators
                    // can watch for this log line.
                    info!("MXDX_DAEMON_READY: daemon fully connected, synced, and accepting commands");
                }
                Err(e) => {
                    error!(error = %e, "daemon failed to connect to Matrix — session commands will fail");
                }
            }
        });
    }

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
