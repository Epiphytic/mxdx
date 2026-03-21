use std::sync::Arc;

use anyhow::Result;
use mxdx_matrix::{MatrixClient, RoomId};
use mxdx_types::events::fabric::{FailurePolicy, TaskEvent};
use tracing::{info, warn};

pub struct FailureContext {
    pub task: TaskEvent,
    pub reason: String,
    pub attempt: u8,
    pub last_progress: Option<String>,
}

pub async fn apply_policy(
    policy: &FailurePolicy,
    ctx: FailureContext,
    matrix_client: &Arc<MatrixClient>,
    sender_room_id: &RoomId,
) -> Result<Option<TaskEvent>> {
    match policy {
        FailurePolicy::Escalate => {
            escalate(&ctx, matrix_client, sender_room_id).await?;
            Ok(None)
        }
        FailurePolicy::Respawn { max_retries } => {
            if ctx.attempt < *max_retries {
                info!(
                    uuid = %ctx.task.uuid,
                    attempt = ctx.attempt,
                    max_retries = max_retries,
                    "respawning task"
                );
                let mut new_task = ctx.task.clone();
                let new_attempt = ctx.attempt + 1;
                if let Some(meta) = new_task.payload.as_object_mut() {
                    meta.insert(
                        "attempt".to_string(),
                        serde_json::Value::Number(new_attempt.into()),
                    );
                } else {
                    new_task.payload = serde_json::json!({ "attempt": new_attempt });
                }
                new_task.uuid = format!("{}-retry-{}", ctx.task.uuid, new_attempt);
                Ok(Some(new_task))
            } else {
                warn!(
                    uuid = %ctx.task.uuid,
                    attempt = ctx.attempt,
                    max_retries = max_retries,
                    "max retries exhausted, escalating"
                );
                escalate(&ctx, matrix_client, sender_room_id).await?;
                Ok(None)
            }
        }
        FailurePolicy::RespawnWithContext => {
            if ctx.attempt < 3 {
                info!(
                    uuid = %ctx.task.uuid,
                    attempt = ctx.attempt,
                    "respawning task with context"
                );
                let mut new_task = ctx.task.clone();
                let new_attempt = ctx.attempt + 1;
                let original_plan = new_task.plan.as_deref().unwrap_or("(no plan)");
                new_task.plan = Some(format!(
                    "Previous attempt failed: {}. Last progress: {}. Original plan: {}",
                    ctx.reason,
                    ctx.last_progress.as_deref().unwrap_or("none"),
                    original_plan,
                ));
                if let Some(meta) = new_task.payload.as_object_mut() {
                    meta.insert(
                        "attempt".to_string(),
                        serde_json::Value::Number(new_attempt.into()),
                    );
                } else {
                    new_task.payload = serde_json::json!({ "attempt": new_attempt });
                }
                new_task.uuid = format!("{}-retry-{}", ctx.task.uuid, new_attempt);
                Ok(Some(new_task))
            } else {
                warn!(
                    uuid = %ctx.task.uuid,
                    attempt = ctx.attempt,
                    "max retries exhausted (respawn_with_context), escalating"
                );
                escalate(&ctx, matrix_client, sender_room_id).await?;
                Ok(None)
            }
        }
        FailurePolicy::Abandon => {
            let msg = format!("❌ Task {} abandoned: {}", ctx.task.uuid, ctx.reason,);
            info!(uuid = %ctx.task.uuid, "abandoning task");
            send_plain_message(matrix_client, sender_room_id, &msg).await?;
            Ok(None)
        }
    }
}

async fn escalate(
    ctx: &FailureContext,
    matrix_client: &Arc<MatrixClient>,
    sender_room_id: &RoomId,
) -> Result<()> {
    let plan = ctx.task.plan.as_deref().unwrap_or("(no plan)");
    let last_progress = ctx.last_progress.as_deref().unwrap_or("none");
    let msg = format!(
        "⚠️ Task {} stalled: {}. Plan: {}. Last progress: {}",
        ctx.task.uuid, ctx.reason, plan, last_progress,
    );
    info!(uuid = %ctx.task.uuid, "escalating task");
    send_plain_message(matrix_client, sender_room_id, &msg).await
}

async fn send_plain_message(
    matrix_client: &Arc<MatrixClient>,
    room_id: &RoomId,
    body: &str,
) -> Result<()> {
    let payload = serde_json::json!({
        "type": "m.room.message",
        "content": {
            "msgtype": "m.text",
            "body": body,
        },
    });
    matrix_client.send_event(room_id, payload).await?;
    Ok(())
}
