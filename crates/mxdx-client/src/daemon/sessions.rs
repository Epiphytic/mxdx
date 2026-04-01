use std::collections::{HashMap, VecDeque};
use tokio::sync::broadcast;

const DEFAULT_BUFFER_SIZE: usize = 65536; // 64KB ring buffer per session

/// Tracks active sessions the daemon is streaming output for.
pub struct SessionTracker {
    sessions: HashMap<String, TrackedSession>,
}

struct TrackedSession {
    output_buffer: VecDeque<String>,
    buffer_bytes: usize,
    max_buffer_bytes: usize,
    output_tx: broadcast::Sender<String>,
    worker_room: String,
    completed: bool,
}

impl SessionTracker {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
        }
    }

    pub fn track(&mut self, uuid: &str, worker_room: &str) -> broadcast::Receiver<String> {
        let (tx, rx) = broadcast::channel(256);
        self.sessions.insert(uuid.to_string(), TrackedSession {
            output_buffer: VecDeque::new(),
            buffer_bytes: 0,
            max_buffer_bytes: DEFAULT_BUFFER_SIZE,
            output_tx: tx,
            worker_room: worker_room.to_string(),
            completed: false,
        });
        rx
    }

    pub fn subscribe(&self, uuid: &str) -> Option<broadcast::Receiver<String>> {
        self.sessions.get(uuid).map(|s| s.output_tx.subscribe())
    }

    pub fn push_output(&mut self, uuid: &str, line: String) {
        if let Some(session) = self.sessions.get_mut(uuid) {
            let line_len = line.len();
            // Reject lines larger than the entire buffer to prevent unbounded growth
            if line_len > session.max_buffer_bytes {
                return;
            }
            while session.buffer_bytes + line_len > session.max_buffer_bytes {
                if let Some(old) = session.output_buffer.pop_front() {
                    session.buffer_bytes -= old.len();
                } else {
                    break;
                }
            }
            session.buffer_bytes += line_len;
            session.output_buffer.push_back(line.clone());
            let _ = session.output_tx.send(line);
        }
    }

    pub fn complete(&mut self, uuid: &str) {
        if let Some(session) = self.sessions.get_mut(uuid) {
            session.completed = true;
        }
    }

    pub fn remove(&mut self, uuid: &str) {
        self.sessions.remove(uuid);
    }

    pub fn buffered_output(&self, uuid: &str) -> Vec<String> {
        self.sessions
            .get(uuid)
            .map(|s| s.output_buffer.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn active_count(&self) -> usize {
        self.sessions.values().filter(|s| !s.completed).count()
    }

    pub fn session_uuids(&self) -> Vec<String> {
        self.sessions.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_and_push_output() {
        let mut tracker = SessionTracker::new();
        let _rx = tracker.track("uuid-1", "!room:example.com");
        tracker.push_output("uuid-1", "line 1".into());
        tracker.push_output("uuid-1", "line 2".into());
        assert_eq!(tracker.buffered_output("uuid-1"), vec!["line 1", "line 2"]);
        assert_eq!(tracker.active_count(), 1);
    }

    #[test]
    fn ring_buffer_evicts_old_entries() {
        let mut tracker = SessionTracker::new();
        tracker.track("uuid-1", "!room:example.com");
        for i in 0..2000 {
            tracker.push_output("uuid-1", format!("line {:05} padding padding padding padding", i));
        }
        let buffered = tracker.buffered_output("uuid-1");
        assert!(buffered.len() < 2000);
        assert!(buffered.last().unwrap().contains("01999"));
    }

    #[test]
    fn complete_and_remove() {
        let mut tracker = SessionTracker::new();
        tracker.track("uuid-1", "!room:example.com");
        assert_eq!(tracker.active_count(), 1);
        tracker.complete("uuid-1");
        assert_eq!(tracker.active_count(), 0);
        tracker.remove("uuid-1");
        assert!(tracker.buffered_output("uuid-1").is_empty());
    }

    #[test]
    fn subscribe_to_existing_session() {
        let mut tracker = SessionTracker::new();
        tracker.track("uuid-1", "!room:example.com");
        let rx2 = tracker.subscribe("uuid-1");
        assert!(rx2.is_some());
        assert!(tracker.subscribe("nonexistent").is_none());
    }

    #[test]
    fn oversized_line_rejected() {
        let mut tracker = SessionTracker::new();
        tracker.track("uuid-1", "!room:example.com");
        // Push a line larger than the 64KB buffer
        let huge = "x".repeat(DEFAULT_BUFFER_SIZE + 1);
        tracker.push_output("uuid-1", huge);
        // Should be rejected — buffer stays empty
        assert!(tracker.buffered_output("uuid-1").is_empty());
    }
}
