use std::time::{SystemTime, UNIX_EPOCH};

use mxdx_types::events::session::SessionHeartbeat;

pub struct HeartbeatPoster {
    interval_seconds: u64,
}

impl HeartbeatPoster {
    pub fn new(interval_seconds: u64) -> Self {
        Self { interval_seconds }
    }

    /// Create a heartbeat event.
    /// Always active regardless of `no_room_output` — heartbeats are liveness signals.
    pub fn create_heartbeat(
        &self,
        session_uuid: &str,
        worker_id: &str,
        progress: Option<String>,
    ) -> SessionHeartbeat {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        SessionHeartbeat {
            session_uuid: session_uuid.to_string(),
            worker_id: worker_id.to_string(),
            timestamp,
            progress,
        }
    }

    pub fn interval_seconds(&self) -> u64 {
        self.interval_seconds
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heartbeat_event_has_correct_fields() {
        let poster = HeartbeatPoster::new(30);
        let hb = poster.create_heartbeat("sess-1", "worker-1", Some("running".into()));

        assert_eq!(hb.session_uuid, "sess-1");
        assert_eq!(hb.worker_id, "worker-1");
        assert_eq!(hb.progress, Some("running".into()));
    }

    #[test]
    fn heartbeat_event_has_recent_timestamp() {
        let poster = HeartbeatPoster::new(30);
        let before = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let hb = poster.create_heartbeat("s", "w", None);
        let after = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        assert!(hb.timestamp >= before);
        assert!(hb.timestamp <= after);
    }

    #[test]
    fn progress_field_is_optional() {
        let poster = HeartbeatPoster::new(10);

        let with_progress = poster.create_heartbeat("s", "w", Some("50%".into()));
        assert_eq!(with_progress.progress, Some("50%".into()));

        let without_progress = poster.create_heartbeat("s", "w", None);
        assert_eq!(without_progress.progress, None);
    }

    #[test]
    fn interval_getter_returns_correct_value() {
        let poster = HeartbeatPoster::new(45);
        assert_eq!(poster.interval_seconds(), 45);

        let poster2 = HeartbeatPoster::new(10);
        assert_eq!(poster2.interval_seconds(), 10);
    }
}
