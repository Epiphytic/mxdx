//! Server-side megolm key backup facade.
//!
//! Uses the SDK's built-in `Backups` API directly:
//! - `backups().create()` provisions a new backup and saves the decryption key
//!   in the crypto store (SQLite).
//! - On session restore, the SDK's `setup_and_resume()` automatically loads the
//!   stored key and activates the backup — no external keychain needed.
//! - If a stale backup from another device exists and the SDK can't resume,
//!   we delete it and create a fresh one.

use anyhow::Result;
use matrix_sdk::Client;

/// Observable state of the megolm backup for a given session.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct BackupState {
    pub enabled: bool,
    pub version: Option<String>,
    pub keys_downloaded: u64,
    pub degraded: bool,
    pub error: Option<String>,
}

/// Ensure a megolm key backup exists and this client is enrolled in it.
///
/// Flow:
/// 1. Wait for the SDK's background `setup_and_resume()` to complete
///    (it runs on `restore_session` / login and loads the backup key from
///    the crypto store automatically).
/// 2. If backups are already enabled, we're done.
/// 3. If no backup exists on the server, create one.
/// 4. If a backup exists but this device can't resume it (stale backup from
///    a different device), delete the stale backup and create a fresh one.
///
/// On failure with `is_first_run == true`, the error is fatal.
/// Otherwise we degrade gracefully.
pub async fn ensure_backup(
    client: &Client,
    is_first_run: bool,
) -> Result<BackupState> {
    let backups = client.encryption().backups();

    // Give the SDK's background initialization a chance to finish.
    // This is where it loads the decryption key from the crypto store.
    client.encryption().wait_for_e2ee_initialization_tasks().await;

    // Check if the SDK already resumed the backup from stored keys.
    if backups.are_enabled().await {
        tracing::info!("backup already enabled via SDK auto-resume");
        return Ok(BackupState {
            enabled: true,
            ..Default::default()
        });
    }

    // SDK didn't auto-resume. Check if a backup exists on the server.
    let exists_on_server = backups.fetch_exists_on_server().await.unwrap_or(false);

    if !exists_on_server {
        // No backup on server — create a fresh one.
        tracing::info!("no backup on server, creating new backup");
        return create_backup(client, is_first_run).await;
    }

    // A backup exists but we can't use it (no stored decryption key).
    // This means either:
    //   - A different device created this backup and never shared the key
    //   - The crypto store was wiped (fresh login)
    //
    // The safe action: delete the stale backup and create a fresh one.
    // Room keys already in the old backup are lost (they belonged to sessions
    // we can't decrypt anyway without the old decryption key).
    tracing::warn!("stale backup exists on server but this device has no decryption key; replacing");
    match backups.disable_and_delete().await {
        Ok(()) => tracing::info!("deleted stale server backup"),
        Err(e) => {
            let msg = format!("failed to delete stale backup: {e}");
            if is_first_run {
                anyhow::bail!("{msg}");
            }
            tracing::warn!(error = %e, "failed to delete stale backup, continuing degraded");
            return Ok(BackupState {
                enabled: false,
                degraded: true,
                error: Some(msg),
                ..Default::default()
            });
        }
    }

    create_backup(client, is_first_run).await
}

/// Create a new backup version on the server.
///
/// Uses `backups().create()` which:
/// 1. Generates a new `BackupDecryptionKey`
/// 2. Uploads the public key to the homeserver
/// 3. Saves the decryption key + version in the crypto store
/// 4. Activates the backup for room key uploads
async fn create_backup(
    client: &Client,
    is_first_run: bool,
) -> Result<BackupState> {
    let backups = client.encryption().backups();

    match backups.create().await {
        Ok(()) => {
            tracing::info!(state = ?backups.state(), "backup created and activated");
            Ok(BackupState {
                enabled: true,
                ..Default::default()
            })
        }
        Err(e) => {
            let msg = format!("backup creation failed: {e}");
            if is_first_run {
                anyhow::bail!("{msg}");
            }
            tracing::warn!(error = %e, "backup creation failed, continuing degraded");
            Ok(BackupState {
                enabled: false,
                degraded: true,
                error: Some(msg),
                ..Default::default()
            })
        }
    }
}

/// Download every megolm key currently in the server-side backup.
///
/// matrix-sdk 0.16 does not expose a bulk "download all" helper on `Backups`,
/// so we iterate the client's joined rooms and call `download_room_keys_for_room`
/// for each. Per-room failures are logged at DEBUG and do not abort the
/// operation; we return the number of rooms whose keys downloaded successfully.
pub async fn download_all_keys(client: &Client) -> Result<u64> {
    let backups = client.encryption().backups();
    let mut count: u64 = 0;
    for room in client.joined_rooms() {
        match backups.download_room_keys_for_room(room.room_id()).await {
            Ok(()) => count += 1,
            Err(e) => tracing::debug!(
                room_id = %room.room_id(),
                error = %e,
                "backup: download_room_keys_for_room failed"
            ),
        }
    }
    tracing::info!(rooms = count, "backup: downloaded keys for joined rooms");
    Ok(count)
}
