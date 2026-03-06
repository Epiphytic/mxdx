use std::collections::VecDeque;

pub struct EventRingBuffer<T> {
    buffer: VecDeque<(u64, T)>,
    capacity: usize,
}

impl<T: Clone> EventRingBuffer<T> {
    pub fn new(capacity: usize) -> Self {
        Self {
            buffer: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// Store an event with its sequence number.
    pub fn push(&mut self, seq: u64, event: T) {
        if self.buffer.len() == self.capacity {
            self.buffer.pop_front();
        }
        self.buffer.push_back((seq, event));
    }

    /// Get events in the seq range [from, to] inclusive.
    pub fn get_range(&self, from: u64, to: u64) -> Vec<T> {
        self.buffer
            .iter()
            .filter(|(seq, _)| *seq >= from && *seq <= to)
            .map(|(_, event)| event.clone())
            .collect()
    }

    /// Get a single event by seq.
    pub fn get(&self, seq: u64) -> Option<&T> {
        self.buffer
            .iter()
            .find(|(s, _)| *s == seq)
            .map(|(_, event)| event)
    }

    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_buffer_stores_and_retrieves() {
        let mut rb = EventRingBuffer::new(10);
        rb.push(0, "event-0".to_string());
        rb.push(1, "event-1".to_string());
        assert_eq!(rb.get(0), Some(&"event-0".to_string()));
        assert_eq!(rb.get(1), Some(&"event-1".to_string()));
    }

    #[test]
    fn ring_buffer_range_query() {
        let mut rb = EventRingBuffer::new(10);
        for i in 0..5 {
            rb.push(i, format!("event-{}", i));
        }
        let range = rb.get_range(1, 3);
        assert_eq!(range, vec!["event-1", "event-2", "event-3"]);
    }

    #[test]
    fn ring_buffer_evicts_oldest_when_full() {
        let mut rb = EventRingBuffer::new(3);
        rb.push(0, "a".to_string());
        rb.push(1, "b".to_string());
        rb.push(2, "c".to_string());
        rb.push(3, "d".to_string()); // Evicts "a"
        assert_eq!(rb.get(0), None);
        assert_eq!(rb.get(1), Some(&"b".to_string()));
        assert_eq!(rb.len(), 3);
    }

    #[test]
    fn ring_buffer_empty() {
        let rb: EventRingBuffer<String> = EventRingBuffer::new(10);
        assert!(rb.is_empty());
        assert_eq!(rb.len(), 0);
        assert_eq!(rb.get_range(0, 10), Vec::<String>::new());
    }
}
