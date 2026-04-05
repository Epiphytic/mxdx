use std::sync::atomic::{AtomicU64, Ordering};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use mxdx_types::events::session::{OutputStream, SessionOutput};

pub struct OutputRouter {
    batch_window_ms: u64,
    batch_size_bytes: usize,
    seq: AtomicU64,
    no_room_output: bool,
}

impl OutputRouter {
    pub fn new(no_room_output: bool) -> Self {
        Self {
            batch_window_ms: 200,
            batch_size_bytes: 4096,
            seq: AtomicU64::new(0),
            no_room_output,
        }
    }

    pub fn with_batch_settings(mut self, window_ms: u64, size_bytes: usize) -> Self {
        self.batch_window_ms = window_ms;
        self.batch_size_bytes = size_bytes;
        self
    }

    /// Create a SessionOutput event from raw data.
    /// Returns None if no_room_output is set (output suppressed).
    pub fn create_output_event(
        &self,
        session_uuid: &str,
        worker_id: &str,
        stream: OutputStream,
        data: &[u8],
    ) -> Option<SessionOutput> {
        if self.no_room_output {
            return None;
        }
        let seq = self.seq.fetch_add(1, Ordering::SeqCst);
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let encoded = BASE64.encode(data);
        Some(SessionOutput {
            session_uuid: session_uuid.to_string(),
            worker_id: worker_id.to_string(),
            stream,
            data: encoded,
            seq,
            timestamp,
        })
    }

    /// Split large data into chunks that fit within batch_size_bytes.
    /// Returns a Vec of output events (one per chunk).
    pub fn create_chunked_output(
        &self,
        session_uuid: &str,
        worker_id: &str,
        stream: OutputStream,
        data: &[u8],
    ) -> Vec<SessionOutput> {
        if self.no_room_output {
            return vec![];
        }
        let mut events = vec![];
        for chunk in data.chunks(self.batch_size_bytes) {
            if let Some(event) =
                self.create_output_event(session_uuid, worker_id, stream.clone(), chunk)
            {
                events.push(event);
            }
        }
        events
    }

    pub fn batch_window_ms(&self) -> u64 {
        self.batch_window_ms
    }

    pub fn is_suppressed(&self) -> bool {
        self.no_room_output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    #[test]
    fn output_event_has_correct_fields_and_base64_data() {
        let router = OutputRouter::new(false);
        let data = b"hello world";
        let event = router
            .create_output_event("sess-1", "worker-1", OutputStream::Stdout, data)
            .expect("should produce event");

        assert_eq!(event.session_uuid, "sess-1");
        assert_eq!(event.worker_id, "worker-1");
        assert_eq!(event.stream, OutputStream::Stdout);
        assert_eq!(event.seq, 0);
        assert!(event.timestamp > 0);

        let decoded = BASE64.decode(&event.data).expect("valid base64");
        assert_eq!(decoded, b"hello world");
    }

    #[test]
    fn sequence_numbers_increment() {
        let router = OutputRouter::new(false);
        let e1 = router
            .create_output_event("s", "w", OutputStream::Stdout, b"a")
            .unwrap();
        let e2 = router
            .create_output_event("s", "w", OutputStream::Stdout, b"b")
            .unwrap();
        let e3 = router
            .create_output_event("s", "w", OutputStream::Stderr, b"c")
            .unwrap();

        assert_eq!(e1.seq, 0);
        assert_eq!(e2.seq, 1);
        assert_eq!(e3.seq, 2);
    }

    #[test]
    fn no_room_output_suppresses_output() {
        let router = OutputRouter::new(true);
        assert!(router.is_suppressed());

        let result = router.create_output_event("s", "w", OutputStream::Stdout, b"data");
        assert!(result.is_none());

        let chunked = router.create_chunked_output("s", "w", OutputStream::Stdout, b"data");
        assert!(chunked.is_empty());
    }

    #[test]
    fn large_data_is_chunked() {
        let router = OutputRouter::new(false).with_batch_settings(200, 100);
        let data = vec![0xABu8; 350]; // 350 bytes -> 4 chunks (100, 100, 100, 50)

        let events = router.create_chunked_output("s", "w", OutputStream::Stdout, &data);

        assert_eq!(events.len(), 4);

        // Verify total data matches
        let mut reassembled = vec![];
        for event in &events {
            let decoded = BASE64.decode(&event.data).unwrap();
            reassembled.extend_from_slice(&decoded);
        }
        assert_eq!(reassembled, data);
    }

    #[test]
    fn each_chunk_has_unique_sequence_number() {
        let router = OutputRouter::new(false).with_batch_settings(200, 50);
        let data = vec![0u8; 150]; // 3 chunks

        let events = router.create_chunked_output("s", "w", OutputStream::Stdout, &data);

        assert_eq!(events.len(), 3);
        assert_eq!(events[0].seq, 0);
        assert_eq!(events[1].seq, 1);
        assert_eq!(events[2].seq, 2);
    }
}
