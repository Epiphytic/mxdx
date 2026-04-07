//! Server-side megolm key backup facade.
//!
//! Wraps matrix-sdk 0.16's `Encryption::backups()` and `Encryption::recovery()`
//! with three flows: first-run-create, keychain-load, and secret-storage-fallback.
//!
//! The recovery key is stored in the chained keychain under a per-launcher key
//! computed via [`mxdx_types::identity::backup_keychain_key`].

use anyhow::{Context, Result};
use matrix_sdk::ruma::UserId;
use matrix_sdk::Client;
use mxdx_types::identity::{backup_keychain_key, KeychainBackend};

/// Observable state of the megolm backup for a given launcher session.
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
/// 1. If no backup exists on the server, create one (first-run path).
/// 2. Otherwise, try to recover using a recovery key stored in the keychain.
/// 3. On local-keychain failure, fall back to secret-storage recovery.
/// 4. On `is_first_run == true`, any failure is fatal. Otherwise we degrade
///    gracefully and report the error in [`BackupState`].
pub async fn ensure_backup(
    client: &Client,
    keychain: &dyn KeychainBackend,
    server: &str,
    matrix_user: &UserId,
    unix_user: &str,
    is_first_run: bool,
) -> Result<BackupState> {
    let key = backup_keychain_key(server, matrix_user.as_str(), unix_user);
    let backups = client.encryption().backups();
    let exists_on_server = backups.exists_on_server().await.unwrap_or(false);

    if !exists_on_server {
        return create_new_backup(client, keychain, &key, is_first_run).await;
    }

    match load_from_keychain(client, keychain, &key).await {
        Ok(state) => Ok(state),
        Err(e_local) => {
            tracing::info!(error=%e_local, "local recovery key unavailable, trying secret storage");
            match load_from_secret_storage(client, keychain, &key).await {
                Ok(state) => Ok(state),
                Err(e_ss) => {
                    let msg = format!("local: {e_local}; secret-storage: {e_ss}");
                    if is_first_run {
                        anyhow::bail!("backup setup failed (first run): {msg}");
                    }
                    tracing::warn!(error=%msg, "backup setup degraded");
                    Ok(BackupState {
                        enabled: false,
                        degraded: true,
                        error: Some(msg),
                        ..Default::default()
                    })
                }
            }
        }
    }
}

async fn create_new_backup(
    client: &Client,
    keychain: &dyn KeychainBackend,
    keychain_key: &str,
    is_first_run: bool,
) -> Result<BackupState> {
    // matrix-sdk 0.16: `Recovery::enable()` returns an `Enable<'_>` builder whose
    // `IntoFuture` output is `Result<String>` — the freshly-minted base58 recovery
    // key. This also provisions the backup on the server, so we use it instead of
    // `Backups::create()` (which doesn't surface the recovery key).
    let recovery = client.encryption().recovery();
    match recovery.enable().await {
        Ok(recovery_key_str) => {
            keychain
                .set(keychain_key, recovery_key_str.as_bytes())
                .context("persist recovery key to keychain")?;
            Ok(BackupState {
                enabled: true,
                version: None,
                keys_downloaded: 0,
                degraded: false,
                error: None,
            })
        }
        Err(e) => {
            if is_first_run {
                anyhow::bail!("failed to create backup: {e}");
            }
            tracing::warn!(error=%e, "backup creation failed, continuing degraded");
            Ok(BackupState {
                enabled: false,
                degraded: true,
                error: Some(e.to_string()),
                ..Default::default()
            })
        }
    }
}

async fn load_from_keychain(
    client: &Client,
    keychain: &dyn KeychainBackend,
    keychain_key: &str,
) -> Result<BackupState> {
    let raw = keychain
        .get(keychain_key)
        .context("keychain get")?
        .ok_or_else(|| anyhow::anyhow!("no recovery key in keychain"))?;
    let recovery_key = String::from_utf8(raw).context("recovery key not utf-8")?;
    let recovery = client.encryption().recovery();
    recovery
        .recover(&recovery_key)
        .await
        .context("recover() rejected stored key")?;
    Ok(BackupState {
        enabled: true,
        version: None,
        keys_downloaded: 0,
        degraded: false,
        error: None,
    })
}

async fn load_from_secret_storage(
    _client: &Client,
    _keychain: &dyn KeychainBackend,
    _keychain_key: &str,
) -> Result<BackupState> {
    // STUB: matrix-sdk 0.16's `Recovery` type does not expose a direct
    // `recover_from_secret_storage()` helper — secret storage is instead accessed
    // through `client.encryption().secret_storage()` with an open secret store.
    // Task 7+ will wire up the secret-storage-based recovery path properly.
    anyhow::bail!("not yet implemented: secret-storage recovery fallback")
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
