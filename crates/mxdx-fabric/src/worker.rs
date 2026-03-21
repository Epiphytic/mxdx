use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use mxdx_matrix::{MatrixClient, RoomId};
use mxdx_types::events::fabric::{
    CapabilityEvent, ClaimEvent, HeartbeatEvent, TaskEvent, TaskResultEvent, TaskStatus,
};
use tracing::{debug, info};

pub const EVENT_TASK: &str = "org.mxdx.fabric.task";
pub const EVENT_CLAIM: &str = "org.mxdx.fabric.claim";
pub const EVENT_HEARTBEAT: &str = "org.mxdx.fabric.heartbeat";
pub const EVENT_RESULT: &str = "org.mxdx.fabric.result";
pub const EVENT_CAPABILITY: &str = "org.mxdx.fabric.capability";

pub struct WorkerClient {
    matrix_client: Arc<MatrixClient>,
    worker_id: String,
    homeserver: String,
}

impl WorkerClient {
    pub fn new(matrix_client: Arc<MatrixClient>, worker_id: String, homeserver: String) -> Self {
        Self {
            matrix_client,
            worker_id,
            homeserver,
        }
    }

    pub async fn advertise_capabilities(
        &self,
        caps: &[String],
        room_id: &RoomId,
    ) -> Result<()> {
        info!(
            worker_id = %self.worker_id,
            capabilities = ?caps,
            room_id = %room_id,
            "advertising capabilities"
        );

        let event = CapabilityEvent {
            worker_id: self.worker_id.clone(),
            capabilities: caps.to_vec(),
            max_concurrent_tasks: 1,
            current_task_count: 0,
        };

        let content = serde_json::to_value(&event)?;

        self.matrix_client
            .send_state_event(room_id, EVENT_CAPABILITY, &self.worker_id, content)
            .await?;

        debug!(
            worker_id = %self.worker_id,
            "capability advertisement sent"
        );

        Ok(())
    }

    pub async fn watch_and_claim(
        &self,
        room_id: &RoomId,
        my_caps: &[String],
    ) -> Result<Option<TaskEvent>> {
        self.matrix_client.sync_once().await?;

        let events = self
            .matrix_client
            .sync_and_collect_events(room_id, std::time::Duration::from_secs(1))
            .await?;

        for event in &events {
            let event_type = event.get("type").and_then(|t| t.as_str()).unwrap_or("");
            if event_type != EVENT_TASK {
                continue;
            }

            let content = match event.get("content") {
                Some(c) => c,
                None => continue,
            };

            let task: TaskEvent = match serde_json::from_value(content.clone()) {
                Ok(t) => t,
                Err(_) => continue,
            };

            let has_caps = task
                .required_capabilities
                .iter()
                .all(|cap| my_caps.contains(cap));

            if !has_caps {
                debug!(
                    uuid = %task.uuid,
                    required = ?task.required_capabilities,
                    mine = ?my_caps,
                    "skipping task: missing capabilities"
                );
                continue;
            }

            if self.try_claim(&task, room_id).await? {
                return Ok(Some(task));
            }
        }

        Ok(None)
    }

    pub async fn try_claim(&self, task: &TaskEvent, room_id: &RoomId) -> Result<bool> {
        let state_key = format!("task/{}/claim", task.uuid);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let claim = ClaimEvent {
            task_uuid: task.uuid.clone(),
            worker_id: self.worker_id.clone(),
            claimed_at: now,
        };

        let content = serde_json::to_value(&claim)?;

        info!(
            uuid = %task.uuid,
            worker_id = %self.worker_id,
            state_key = %state_key,
            "attempting claim"
        );

        self.matrix_client
            .send_state_event(room_id, EVENT_CLAIM, &state_key, content)
            .await?;

        self.matrix_client.sync_once().await?;

        let state = self
            .matrix_client
            .get_room_state_event(room_id, EVENT_CLAIM, &state_key)
            .await?;

        let winner = state
            .get("worker_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if winner == self.worker_id {
            info!(
                uuid = %task.uuid,
                worker_id = %self.worker_id,
                "claim won"
            );
            Ok(true)
        } else {
            debug!(
                uuid = %task.uuid,
                worker_id = %self.worker_id,
                winner = %winner,
                "claim lost, backing off"
            );
            Ok(false)
        }
    }

    pub async fn post_heartbeat(
        &self,
        task_uuid: &str,
        progress: Option<String>,
        room_id: &RoomId,
    ) -> Result<()> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let heartbeat = HeartbeatEvent {
            task_uuid: task_uuid.to_string(),
            worker_id: self.worker_id.clone(),
            progress,
            timestamp: now,
        };

        let payload = serde_json::json!({
            "type": EVENT_HEARTBEAT,
            "content": heartbeat,
        });

        debug!(
            uuid = %task_uuid,
            worker_id = %self.worker_id,
            "posting heartbeat"
        );

        self.matrix_client.send_event(room_id, payload).await?;

        Ok(())
    }

    pub async fn post_result(
        &self,
        task_uuid: &str,
        status: TaskStatus,
        output: Option<serde_json::Value>,
        error: Option<String>,
        duration_seconds: u64,
        room_id: &RoomId,
    ) -> Result<()> {
        let result = TaskResultEvent {
            task_uuid: task_uuid.to_string(),
            worker_id: self.worker_id.clone(),
            status,
            output,
            error,
            duration_seconds,
        };

        let payload = serde_json::json!({
            "type": EVENT_RESULT,
            "content": result,
        });

        info!(
            uuid = %task_uuid,
            worker_id = %self.worker_id,
            "posting result"
        );

        self.matrix_client.send_event(room_id, payload).await?;

        Ok(())
    }

    pub fn worker_id(&self) -> &str {
        &self.worker_id
    }

    pub fn homeserver(&self) -> &str {
        &self.homeserver
    }
}
