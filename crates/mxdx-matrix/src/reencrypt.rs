//! Idempotent room re-encryption: detects rooms missing m.room.encryption
//! or missing encrypt_state_events, tombstones them, and creates encrypted
//! replacements with the same name/topic/members.
//!
//! Tombstone state events are sent via REST directly (not through the SDK)
//! because the old room may not be in the SDK's local cache yet (it was
//! discovered via REST before a sync). Since the old room is unencrypted,
//! a plain REST PUT is the correct approach.

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

    // Tombstone the old room via REST. We cannot use the SDK's
    // `tombstone_room` here because the SDK may not know about the old room
    // yet (it was discovered via REST, not via sync). The old room is
    // unencrypted, so a plain REST PUT is correct.
    let tombstone_content = serde_json::json!({
        "body": "This room has been replaced",
        "replacement_room": new_id.to_string(),
    });
    if let Err(e) = rest
        .put_state_event(room, "m.room.tombstone", "", tombstone_content)
        .await
    {
        tracing::warn!(room=%room, error=%e, "failed to tombstone old room (non-fatal)");
    }

    // Best-effort leave via REST — the SDK may not know about this room.
    if let Err(e) = rest.leave_room(room).await {
        tracing::warn!(room=%room, error=%e, "failed to leave old room (non-fatal)");
    }
    Ok(new_id)
}
