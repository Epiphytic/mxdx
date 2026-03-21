use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use mxdx_matrix::{MatrixClient, OwnedRoomId};
use tracing::{debug, info};

pub struct CapabilityIndex {
    matrix_client: Arc<MatrixClient>,
    index: HashMap<String, OwnedRoomId>,
}

impl CapabilityIndex {
    pub fn new(matrix_client: Arc<MatrixClient>) -> Self {
        Self {
            matrix_client,
            index: HashMap::new(),
        }
    }

    pub fn capability_room_name(caps: &[String]) -> String {
        let mut sorted: Vec<&str> = caps.iter().map(|s| s.as_str()).collect();
        sorted.sort();
        format!("workers.{}", sorted.join("."))
    }

    pub fn find_room(&self, caps: &[String]) -> Option<OwnedRoomId> {
        let name = Self::capability_room_name(caps);
        self.index.get(&name).cloned()
    }

    pub async fn get_or_create_room(
        &mut self,
        caps: &[String],
        _homeserver: &str,
    ) -> Result<OwnedRoomId> {
        if let Some(room_id) = self.find_room(caps) {
            return Ok(room_id);
        }

        let room_name = Self::capability_room_name(caps);
        let topic = format!("org.mxdx.fabric.workers:{room_name}");

        info!(room_name = %room_name, "creating capability room");

        let room_id = self
            .matrix_client
            .create_named_unencrypted_room(&room_name, &topic)
            .await?;

        debug!(room_name = %room_name, room_id = %room_id, "capability room created");

        self.index.insert(room_name, room_id.clone());
        Ok(room_id)
    }

    pub async fn populate_from_server(&mut self) -> Result<()> {
        self.matrix_client.sync_once().await?;

        for room in self.matrix_client.inner().joined_rooms() {
            let name = room.name().unwrap_or_default();
            if name.starts_with("workers.") {
                let room_id = room.room_id().to_owned();
                debug!(room_name = %name, room_id = %room_id, "indexed existing capability room");
                self.index.insert(name, room_id);
            }
        }

        info!(
            count = self.index.len(),
            "populated capability index from server"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_room_name_single_cap() {
        let caps = vec!["rust".to_string()];
        assert_eq!(CapabilityIndex::capability_room_name(&caps), "workers.rust");
    }

    #[test]
    fn capability_room_name_multiple_caps_sorted() {
        let caps = vec!["rust".to_string(), "linux".to_string(), "arm64".to_string()];
        assert_eq!(
            CapabilityIndex::capability_room_name(&caps),
            "workers.arm64.linux.rust"
        );
    }

    #[test]
    fn capability_room_name_already_sorted() {
        let caps = vec!["arm64".to_string(), "linux".to_string(), "rust".to_string()];
        assert_eq!(
            CapabilityIndex::capability_room_name(&caps),
            "workers.arm64.linux.rust"
        );
    }

    #[test]
    fn capability_room_name_empty_caps() {
        let caps: Vec<String> = vec![];
        assert_eq!(CapabilityIndex::capability_room_name(&caps), "workers.");
    }
}
