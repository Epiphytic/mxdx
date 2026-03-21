use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use mxdx_matrix::{MatrixClient, RoomId};
use mxdx_types::events::fabric::{TaskEvent, TaskResultEvent};
use tokio::net::UnixStream;
use tracing::{debug, info, warn};

use crate::worker::{EVENT_RESULT, EVENT_TASK};

const EVENT_STREAM_OFFER: &str = "org.mxdx.fabric.stream_offer";

pub struct SenderClient {
    matrix_client: Arc<MatrixClient>,
    sender_id: String,
}

impl SenderClient {
    pub fn new(matrix_client: Arc<MatrixClient>, sender_id: String) -> Self {
        Self {
            matrix_client,
            sender_id,
        }
    }

    pub async fn post_task(&self, task: TaskEvent, coordinator_room_id: &RoomId) -> Result<String> {
        info!(
            sender_id = %self.sender_id,
            task_uuid = %task.uuid,
            room_id = %coordinator_room_id,
            "posting task"
        );

        let payload = serde_json::json!({
            "type": EVENT_TASK,
            "content": task,
        });

        self.matrix_client
            .send_event(coordinator_room_id, payload)
            .await?;

        debug!(
            task_uuid = %task.uuid,
            "task posted successfully"
        );

        Ok(task.uuid)
    }

    pub async fn wait_for_result(
        &self,
        task_uuid: &str,
        room_id: &RoomId,
        timeout: Duration,
    ) -> Result<Option<TaskResultEvent>> {
        info!(
            sender_id = %self.sender_id,
            task_uuid = %task_uuid,
            timeout_secs = timeout.as_secs(),
            "waiting for task result"
        );

        let deadline = tokio::time::Instant::now() + timeout;

        while tokio::time::Instant::now() < deadline {
            let remaining = deadline - tokio::time::Instant::now();
            let poll_duration = remaining.min(Duration::from_secs(2));

            let events = self
                .matrix_client
                .sync_and_collect_events(room_id, poll_duration)
                .await?;

            for event in &events {
                let event_type = event.get("type").and_then(|t| t.as_str()).unwrap_or("");
                if event_type != EVENT_RESULT {
                    continue;
                }

                let content = match event.get("content") {
                    Some(c) => c,
                    None => continue,
                };

                let result: TaskResultEvent = match serde_json::from_value(content.clone()) {
                    Ok(r) => r,
                    Err(e) => {
                        warn!(error = %e, "failed to parse TaskResultEvent");
                        continue;
                    }
                };

                if result.task_uuid == task_uuid {
                    info!(
                        task_uuid = %task_uuid,
                        status = ?result.status,
                        worker_id = %result.worker_id,
                        "received task result"
                    );
                    return Ok(Some(result));
                }
            }
        }

        warn!(
            task_uuid = %task_uuid,
            timeout_secs = timeout.as_secs(),
            "timed out waiting for task result"
        );

        Ok(None)
    }

    pub async fn submit_and_wait(
        &self,
        task: TaskEvent,
        coordinator_room_id: &RoomId,
        timeout: Duration,
    ) -> Result<Option<TaskResultEvent>> {
        let task_uuid = self.post_task(task, coordinator_room_id).await?;
        self.wait_for_result(&task_uuid, coordinator_room_id, timeout)
            .await
    }

    pub fn sender_id(&self) -> &str {
        &self.sender_id
    }

    pub async fn connect_stream(
        &self,
        task_uuid: &str,
        room_id: &RoomId,
        timeout: Duration,
    ) -> Result<Option<UnixStream>> {
        info!(
            sender_id = %self.sender_id,
            task_uuid = %task_uuid,
            timeout_secs = timeout.as_secs(),
            "polling for stream offer"
        );

        let deadline = tokio::time::Instant::now() + timeout;
        let state_key = format!("task/{}/stream", task_uuid);

        while tokio::time::Instant::now() < deadline {
            let remaining = deadline - tokio::time::Instant::now();
            let poll_duration = remaining.min(Duration::from_secs(2));

            let _events = self
                .matrix_client
                .sync_and_collect_events(room_id, poll_duration)
                .await?;

            let state = self
                .matrix_client
                .get_room_state_event(room_id, EVENT_STREAM_OFFER, &state_key)
                .await;

            match state {
                Ok(offer) => {
                    if let Some(socket_path) = offer.get("socket_path").and_then(|v| v.as_str()) {
                        info!(
                            task_uuid = %task_uuid,
                            socket_path = %socket_path,
                            "found stream offer, connecting"
                        );

                        match UnixStream::connect(socket_path).await {
                            Ok(stream) => {
                                info!(
                                    task_uuid = %task_uuid,
                                    "connected to P2P stream"
                                );
                                return Ok(Some(stream));
                            }
                            Err(e) => {
                                warn!(
                                    task_uuid = %task_uuid,
                                    socket_path = %socket_path,
                                    error = %e,
                                    "failed to connect to stream socket"
                                );
                                return Ok(None);
                            }
                        }
                    }
                }
                Err(e) => {
                    debug!(
                        task_uuid = %task_uuid,
                        error = %e,
                        "stream offer not found yet, retrying"
                    );
                }
            }
        }

        warn!(
            task_uuid = %task_uuid,
            timeout_secs = timeout.as_secs(),
            "timed out waiting for stream offer"
        );

        Ok(None)
    }
}
