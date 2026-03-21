use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use mxdx_matrix::{MatrixClient, OwnedRoomId};
use mxdx_types::events::fabric::{
    ClaimEvent, HeartbeatEvent, RoutingMode, TaskEvent, TaskResultEvent,
};
use tokio::time::Instant;
use tracing::{debug, info, warn};

use crate::capability_index::CapabilityIndex;

pub struct WatchEntry {
    pub task: TaskEvent,
    pub claimed_at: Option<Instant>,
    pub last_heartbeat: Instant,
}

pub struct CoordinatorBot {
    matrix_client: Arc<MatrixClient>,
    capability_index: CapabilityIndex,
    coordinator_room_id: OwnedRoomId,
    homeserver: String,
    watchlist: HashMap<String, WatchEntry>,
}

impl CoordinatorBot {
    pub fn new(
        matrix_client: Arc<MatrixClient>,
        coordinator_room_id: OwnedRoomId,
        homeserver: String,
    ) -> Self {
        let capability_index = CapabilityIndex::new(matrix_client.clone());
        Self {
            matrix_client,
            capability_index,
            coordinator_room_id,
            homeserver,
            watchlist: HashMap::new(),
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        info!(
            room_id = %self.coordinator_room_id,
            "coordinator routing loop starting"
        );

        let mut last_watchlist_check = Instant::now();

        loop {
            self.matrix_client.sync_once().await?;

            let events = self
                .matrix_client
                .sync_and_collect_events(&self.coordinator_room_id, Duration::from_secs(1))
                .await?;

            for event in &events {
                let event_type = event.get("type").and_then(|t| t.as_str()).unwrap_or("");

                let content = match event.get("content") {
                    Some(c) => c,
                    None => continue,
                };

                match event_type {
                    "org.mxdx.fabric.task" => {
                        if let Ok(task) = serde_json::from_value::<TaskEvent>(content.clone()) {
                            self.handle_task_event(task).await?;
                        }
                    }
                    "org.mxdx.fabric.claim" => {
                        if let Ok(claim) = serde_json::from_value::<ClaimEvent>(content.clone()) {
                            self.handle_claim_event(&claim);
                        }
                    }
                    "org.mxdx.fabric.heartbeat" => {
                        if let Ok(hb) = serde_json::from_value::<HeartbeatEvent>(content.clone()) {
                            self.handle_heartbeat_event(&hb);
                        }
                    }
                    "org.mxdx.fabric.result" => {
                        if let Ok(result) =
                            serde_json::from_value::<TaskResultEvent>(content.clone())
                        {
                            self.handle_result_event(&result);
                        }
                    }
                    _ => {}
                }
            }

            if last_watchlist_check.elapsed() >= Duration::from_secs(10) {
                self.check_watchlist();
                last_watchlist_check = Instant::now();
            }
        }
    }

    pub async fn handle_task_event(&mut self, task: TaskEvent) -> Result<()> {
        info!(
            uuid = %task.uuid,
            sender = %task.sender_id,
            caps = ?task.required_capabilities,
            routing = ?task.routing_mode,
            "received task event"
        );

        let worker_room_id = self
            .capability_index
            .get_or_create_room(&task.required_capabilities, &self.homeserver)
            .await?;

        let effective_mode = match &task.routing_mode {
            RoutingMode::Auto => {
                if task.timeout_seconds < 30 {
                    RoutingMode::Direct
                } else {
                    RoutingMode::Brokered
                }
            }
            other => other.clone(),
        };

        match effective_mode {
            RoutingMode::Direct => {
                self.route_direct(&task, &worker_room_id).await?;
            }
            RoutingMode::Brokered => {
                self.route_brokered(&task, &worker_room_id).await?;
            }
            RoutingMode::Auto => unreachable!(),
        }

        self.watchlist.insert(
            task.uuid.clone(),
            WatchEntry {
                task,
                claimed_at: None,
                last_heartbeat: Instant::now(),
            },
        );

        Ok(())
    }

    async fn route_direct(&self, task: &TaskEvent, worker_room_id: &OwnedRoomId) -> Result<()> {
        info!(
            uuid = %task.uuid,
            sender = %task.sender_id,
            worker_room = %worker_room_id,
            "routing direct: inviting sender to worker room"
        );

        // TODO: MatrixClient does not yet have an invite_user method.
        // When available, call: self.matrix_client.invite_user(worker_room_id, &task.sender_id).await?;
        warn!(
            uuid = %task.uuid,
            sender = %task.sender_id,
            "invite_user not yet implemented on MatrixClient - sender not invited"
        );

        Ok(())
    }

    async fn route_brokered(&self, task: &TaskEvent, worker_room_id: &OwnedRoomId) -> Result<()> {
        info!(
            uuid = %task.uuid,
            sender = %task.sender_id,
            worker_room = %worker_room_id,
            "routing brokered: posting task to worker room on sender's behalf"
        );

        let payload = serde_json::json!({
            "type": "org.mxdx.fabric.task",
            "content": task,
        });

        self.matrix_client
            .send_event(worker_room_id, payload)
            .await?;

        debug!(
            uuid = %task.uuid,
            worker_room = %worker_room_id,
            "task posted to worker room"
        );

        Ok(())
    }

    pub fn watchlist_len(&self) -> usize {
        self.watchlist.len()
    }

    pub fn watchlist_contains(&self, uuid: &str) -> bool {
        self.watchlist.contains_key(uuid)
    }

    pub fn capability_index(&self) -> &CapabilityIndex {
        &self.capability_index
    }

    pub fn handle_claim_event(&mut self, claim: &ClaimEvent) {
        info!(
            uuid = %claim.task_uuid,
            worker = %claim.worker_id,
            "claim event received"
        );

        if let Some(entry) = self.watchlist.get_mut(&claim.task_uuid) {
            entry.claimed_at = Some(Instant::now());
            debug!(
                uuid = %claim.task_uuid,
                worker = %claim.worker_id,
                "watchlist updated with claim"
            );
        } else {
            warn!(
                uuid = %claim.task_uuid,
                "claim event for unknown task"
            );
        }
    }

    pub fn handle_heartbeat_event(&mut self, hb: &HeartbeatEvent) {
        debug!(
            uuid = %hb.task_uuid,
            worker = %hb.worker_id,
            progress = ?hb.progress,
            "heartbeat received"
        );

        if let Some(entry) = self.watchlist.get_mut(&hb.task_uuid) {
            entry.last_heartbeat = Instant::now();
        } else {
            warn!(
                uuid = %hb.task_uuid,
                "heartbeat for unknown task"
            );
        }
    }

    pub fn handle_result_event(&mut self, result: &TaskResultEvent) {
        info!(
            uuid = %result.task_uuid,
            worker = %result.worker_id,
            status = ?result.status,
            duration = result.duration_seconds,
            "task completed"
        );

        if self.watchlist.remove(&result.task_uuid).is_some() {
            debug!(
                uuid = %result.task_uuid,
                "removed from watchlist"
            );
        } else {
            warn!(
                uuid = %result.task_uuid,
                "result event for unknown task"
            );
        }
    }

    fn check_watchlist(&self) {
        for (uuid, entry) in &self.watchlist {
            if entry.claimed_at.is_none()
                && entry.last_heartbeat.elapsed() > Duration::from_secs(60)
            {
                warn!(
                    uuid = %uuid,
                    "task {uuid} unclaimed for 60s"
                );
            }

            let heartbeat_overdue = Duration::from_secs(entry.task.heartbeat_interval_seconds * 2);
            if entry.last_heartbeat.elapsed() > heartbeat_overdue {
                warn!(
                    uuid = %uuid,
                    "task {uuid} heartbeat overdue"
                );
            }
        }
    }
}
