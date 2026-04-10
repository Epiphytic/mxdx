//! Idempotent room re-encryption: detects rooms missing m.room.encryption
//! or missing encrypt_state_events, tombstones them, and creates encrypted
//! replacements with the same name/topic/members.
//!
//! Security note: tombstone state events are sent via
//! `MatrixClient::tombstone_room`, which goes through `send_state_event` on
//! a room with `encrypt_state_events=true` (MSC4362). The MSC4362 feature is
//! enabled project-wide on `matrix-sdk-base`, so state events on properly
//! encrypted rooms are encrypted on the wire.

use crate::client::MatrixClient;
use crate::error::Result;
use crate::rest::RestClient;
use crate::rooms::LauncherTopology;
use matrix_sdk::ruma::{OwnedRoomId, OwnedUserId, RoomId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RoomRole {
    Space,
    Exec,
    Logs,
}

/// Verify each room in `topology` has proper E2EE state. For any room that
/// doesn't, create an encrypted replacement, tombstone the old room, and
/// leave/forget it. Returns a topology with possibly-updated room IDs.
pub async fn verify_or_replace_topology(
    matrix: &MatrixClient,
    rest: &RestClient,
    topology: LauncherTopology,
    launcher_id: &str,
    authorized_users: &[OwnedUserId],
) -> Result<LauncherTopology> {
    let new_space = ensure_encrypted(
        matrix,
        rest,
        &topology.space_id,
        RoomRole::Space,
        launcher_id,
        authorized_users,
    )
    .await?;
    let new_exec = ensure_encrypted(
        matrix,
        rest,
        &topology.exec_room_id,
        RoomRole::Exec,
        launcher_id,
        authorized_users,
    )
    .await?;
    let new_logs = ensure_encrypted(
        matrix,
        rest,
        &topology.logs_room_id,
        RoomRole::Logs,
        launcher_id,
        authorized_users,
    )
    .await?;
    Ok(LauncherTopology {
        space_id: new_space,
        exec_room_id: new_exec,
        logs_room_id: new_logs,
    })
}

async fn ensure_encrypted(
    matrix: &MatrixClient,
    rest: &RestClient,
    room: &RoomId,
    role: RoomRole,
    launcher_id: &str,
    authorized_users: &[OwnedUserId],
) -> Result<OwnedRoomId> {
    let enc = rest
        .get_room_encryption(room)
        .await
        .map_err(|e| crate::error::MatrixClientError::Other(e))?;
    let needs_replace = match enc {
        None => true,
        Some(s) => s.algorithm != "m.megolm.v1.aes-sha2" || !s.encrypt_state_events,
    };
    if !needs_replace {
        return Ok(room.to_owned());
    }
    tracing::warn!(
        room=%room,
        ?role,
        "room not properly encrypted, creating encrypted replacement"
    );

    let (name, topic, role_key) = match role {
        RoomRole::Space => (
            format!("mxdx: {launcher_id}"),
            format!("org.mxdx.launcher.space:{launcher_id}"),
            "space",
        ),
        RoomRole::Exec => (
            format!("mxdx: {launcher_id} \u{2014} exec"),
            format!("org.mxdx.launcher.exec:{launcher_id}"),
            "exec",
        ),
        RoomRole::Logs => (
            format!("mxdx: {launcher_id} \u{2014} logs"),
            format!("org.mxdx.launcher.logs:{launcher_id}"),
            "logs",
        ),
    };

    // Use the mxdx variant so the replacement room carries launcher_id + role
    // in its `m.room.create` content — the next startup must be able to
    // rediscover it via plain REST.
    let new_id = matrix
        .create_mxdx_encrypted_room(&name, &topic, authorized_users, launcher_id, role_key)
        .await?;
    matrix.tombstone_room(room, &new_id).await?;
    if let Err(e) = matrix.leave_and_forget_room(room).await {
        tracing::warn!(room=%room, error=%e, "failed to leave old room (non-fatal)");
    }
    Ok(new_id)
}
