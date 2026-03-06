use std::time::{Duration, Instant};

pub struct OutputBatcher {
    buffer: Vec<u8>,
    max_bytes: usize,
    flush_interval: Duration,
    last_flush: Instant,
}

impl OutputBatcher {
    pub fn new(max_bytes: usize, flush_interval: Duration) -> Self {
        Self {
            buffer: Vec::new(),
            max_bytes,
            flush_interval,
            last_flush: Instant::now(),
        }
    }

    /// Add data to the buffer. Returns Some(data) if flush threshold reached.
    pub fn push(&mut self, data: &[u8]) -> Option<Vec<u8>> {
        self.buffer.extend_from_slice(data);
        if self.buffer.len() >= self.max_bytes {
            Some(self.drain())
        } else {
            None
        }
    }

    /// Check if the flush interval has elapsed. Returns Some(data) if so.
    pub fn tick(&mut self) -> Option<Vec<u8>> {
        if !self.buffer.is_empty() && self.last_flush.elapsed() >= self.flush_interval {
            Some(self.drain())
        } else {
            None
        }
    }

    /// Force flush any remaining data.
    pub fn flush(&mut self) -> Option<Vec<u8>> {
        if self.buffer.is_empty() {
            None
        } else {
            Some(self.drain())
        }
    }

    fn drain(&mut self) -> Vec<u8> {
        self.last_flush = Instant::now();
        std::mem::take(&mut self.buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn batcher_flushes_at_size_threshold() {
        let mut batcher = OutputBatcher::new(10, Duration::from_millis(15));
        let result = batcher.push(b"hello world!"); // 12 bytes > 10
        assert!(result.is_some());
    }

    #[test]
    fn batcher_holds_small_data() {
        let mut batcher = OutputBatcher::new(4096, Duration::from_millis(15));
        let result = batcher.push(b"hi");
        assert!(result.is_none());
    }

    #[test]
    fn batcher_flush_returns_accumulated_data() {
        let mut batcher = OutputBatcher::new(4096, Duration::from_millis(15));
        batcher.push(b"hello ");
        batcher.push(b"world");
        let data = batcher.flush().unwrap();
        assert_eq!(data, b"hello world");
    }

    #[test]
    fn batcher_flush_empty_returns_none() {
        let mut batcher = OutputBatcher::new(4096, Duration::from_millis(15));
        assert!(batcher.flush().is_none());
    }
}
