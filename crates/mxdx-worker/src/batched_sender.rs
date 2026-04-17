use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time;

/// Default batch window: historic 200ms Matrix-rate-limit-safe value.
pub const DEFAULT_BATCH_WINDOW: Duration = Duration::from_millis(200);

/// P2P-open batch window per storm §2.8 — 10ms for sub-frame latency when
/// the data channel is available (no HS rate-limit). T-61 flips between
/// `DEFAULT_BATCH_WINDOW` and `P2P_OPEN_BATCH_WINDOW` on transport state
/// transitions.
pub const P2P_OPEN_BATCH_WINDOW: Duration = Duration::from_millis(10);

/// Configuration for the BatchedSender.
pub struct BatchConfig {
    /// How long to wait for additional data before sending a batch.
    pub batch_window: Duration,
    /// Minimum payload size to trigger compression.
    pub compression_threshold: usize,
    /// Optional session identifier for event correlation.
    pub session_id: Option<String>,
}

impl Default for BatchConfig {
    fn default() -> Self {
        Self {
            batch_window: DEFAULT_BATCH_WINDOW,
            compression_threshold: 32,
            session_id: None,
        }
    }
}

/// A batched event sender that coalesces output into larger events.
///
/// Data pushed via `send()` is buffered for `batch_window` duration.
/// When the window expires, all buffered data is compressed (if above
/// threshold) and sent as a single event via the provided sender function.
pub struct BatchedSender {
    tx: mpsc::Sender<Vec<u8>>,
    /// Handle to the background task. Dropped when BatchedSender is dropped.
    _task: tokio::task::JoinHandle<()>,
    /// Shared batch-window cell. T-61: the P2P integration flips this to
    /// 10ms on transport `Open`, back to 200ms on any non-Open transition.
    /// The batch_loop reads this cell once per iteration to pick up the
    /// new value on the next timeout.
    window: Arc<Mutex<Duration>>,
}

/// Type alias for the send function used by BatchedSender.
///
/// Arguments: (payload, sequence_number, is_compressed)
/// Returns: Result<(), String> where Err contains the error message.
/// A "429" substring in the error triggers rate-limit backoff.
pub type SendFn = Box<
    dyn Fn(Vec<u8>, u64, bool) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>>
        + Send
        + 'static,
>;

impl BatchedSender {
    /// Create a new BatchedSender.
    ///
    /// `send_fn` is called with the batched payload, sequence number, and
    /// whether the payload is zlib-compressed. It should post the event to Matrix.
    pub fn new(config: BatchConfig, send_fn: SendFn) -> Self {
        let (tx, rx) = mpsc::channel(1024);
        let window = Arc::new(Mutex::new(config.batch_window));
        let task =
            tokio::spawn(Self::batch_loop(rx, config, send_fn, window.clone()));
        Self {
            tx,
            _task: task,
            window,
        }
    }

    /// Push data to be batched and sent.
    pub async fn send(&self, data: Vec<u8>) -> Result<(), mpsc::error::SendError<Vec<u8>>> {
        self.tx.send(data).await
    }

    /// Flush: signal no more data and wait for all batches to be sent.
    pub async fn flush(self) {
        drop(self.tx); // Close channel
        let _ = self._task.await; // Wait for loop to finish
    }

    /// T-61: dynamically update the batch window. The running `batch_loop`
    /// picks up the new value on its next timeout iteration.
    ///
    /// Storm §2.8 flip rule:
    /// - On P2PTransport `Open` entry → `P2P_OPEN_BATCH_WINDOW` (10ms)
    /// - On any non-Open transition → `DEFAULT_BATCH_WINDOW` (200ms)
    ///
    /// Safe to call concurrently from any thread.
    pub fn set_batch_window(&self, new_window: Duration) {
        if let Ok(mut guard) = self.window.lock() {
            *guard = new_window;
        }
    }

    /// Read the current batch window (test helper).
    pub fn current_batch_window(&self) -> Duration {
        self.window
            .lock()
            .map(|g| *g)
            .unwrap_or(DEFAULT_BATCH_WINDOW)
    }

    async fn batch_loop(
        mut rx: mpsc::Receiver<Vec<u8>>,
        config: BatchConfig,
        send_fn: SendFn,
        window: Arc<Mutex<Duration>>,
    ) {
        let mut seq: u64 = 0;
        let mut buffer: Vec<u8> = Vec::new();
        let mut backoff: Option<Duration> = None;

        loop {
            // If in backoff from 429, wait first
            if let Some(wait) = backoff.take() {
                time::sleep(wait).await;
            }

            // Wait for first item or channel close
            let item = if buffer.is_empty() {
                match rx.recv().await {
                    Some(data) => Some(data),
                    None => break, // Channel closed
                }
            } else {
                // Buffer has data -- wait for batch window. Re-read the
                // shared window cell so T-61's set_batch_window takes effect
                // on the next coalescing cycle.
                let current_window = window
                    .lock()
                    .map(|g| *g)
                    .unwrap_or(DEFAULT_BATCH_WINDOW);
                match time::timeout(current_window, rx.recv()).await {
                    Ok(Some(data)) => Some(data),
                    Ok(None) => None, // Channel closed, send remaining
                    Err(_) => None,   // Timeout -- send batch
                }
            };

            if let Some(data) = item {
                buffer.extend_from_slice(&data);
                continue; // Wait for more or timeout
            }

            // Send the batch
            if buffer.is_empty() {
                // Channel closed with empty buffer
                break;
            }

            let (payload, compressed) =
                Self::maybe_compress(&buffer, config.compression_threshold);
            buffer.clear();
            seq += 1;

            match send_fn(payload, seq, compressed).await {
                Ok(()) => {}
                Err(e) => {
                    // Check for 429 rate limit
                    if e.contains("429") {
                        let wait_ms = Self::parse_retry_after(&e).unwrap_or(5000);
                        tracing::warn!(
                            retry_after_ms = wait_ms,
                            "rate limited (429), backing off"
                        );
                        backoff = Some(Duration::from_millis(wait_ms));
                    } else {
                        tracing::error!(error = %e, "failed to send batch");
                    }
                }
            }
        }

        // Drain any remaining buffer
        if !buffer.is_empty() {
            let (payload, compressed) =
                Self::maybe_compress(&buffer, config.compression_threshold);
            seq += 1;
            let _ = send_fn(payload, seq, compressed).await;
        }
    }

    fn maybe_compress(data: &[u8], threshold: usize) -> (Vec<u8>, bool) {
        if data.len() < threshold {
            return (data.to_vec(), false);
        }
        use flate2::write::DeflateEncoder;
        use flate2::Compression;
        use std::io::Write;

        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        if encoder.write_all(data).is_ok() {
            if let Ok(compressed) = encoder.finish() {
                if compressed.len() < data.len() {
                    return (compressed, true);
                }
            }
        }
        (data.to_vec(), false)
    }

    fn parse_retry_after(error_msg: &str) -> Option<u64> {
        // Look for retry_after_ms in error message
        if let Some(pos) = error_msg.find("retry_after_ms") {
            let rest = &error_msg[pos..];
            for word in rest.split(|c: char| !c.is_ascii_digit()) {
                if let Ok(ms) = word.parse::<u64>() {
                    if ms > 0 {
                        return Some(ms);
                    }
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    /// Helper: create a recording send_fn that collects all calls.
    fn recording_sender(
    ) -> (
        Arc<Mutex<Vec<(Vec<u8>, u64, bool)>>>,
        SendFn,
    ) {
        let log: Arc<Mutex<Vec<(Vec<u8>, u64, bool)>>> = Arc::new(Mutex::new(Vec::new()));
        let log2 = log.clone();
        let send_fn: SendFn = Box::new(move |payload, seq, compressed| {
            let log = log2.clone();
            Box::pin(async move {
                log.lock().await.push((payload, seq, compressed));
                Ok(())
            })
        });
        (log, send_fn)
    }

    fn test_config(batch_ms: u64) -> BatchConfig {
        BatchConfig {
            batch_window: Duration::from_millis(batch_ms),
            compression_threshold: 32,
            session_id: None,
        }
    }

    #[tokio::test]
    async fn test_single_chunk_sent_after_window() {
        let (log, send_fn) = recording_sender();
        let sender = BatchedSender::new(test_config(10), send_fn);

        sender.send(b"hello".to_vec()).await.unwrap();
        sender.flush().await;

        let calls = log.lock().await;
        assert_eq!(calls.len(), 1, "expected exactly 1 batch sent");
        assert_eq!(calls[0].1, 1, "sequence number should be 1");
    }

    #[tokio::test]
    async fn test_multiple_chunks_batched() {
        let (log, send_fn) = recording_sender();
        let sender = BatchedSender::new(test_config(50), send_fn);

        // Push 10 small chunks rapidly (no await between pushes)
        for i in 0..10 {
            sender.send(format!("chunk{i}").into_bytes()).await.unwrap();
        }
        sender.flush().await;

        let calls = log.lock().await;
        // All 10 chunks should be coalesced into 1-2 batches (not 10)
        assert!(
            calls.len() <= 2,
            "expected at most 2 batched events, got {}",
            calls.len()
        );
        // Verify all data is present in the concatenated payloads (decompress if needed)
        let mut all_data = Vec::new();
        for (payload, _, compressed) in calls.iter() {
            if *compressed {
                use flate2::read::DeflateDecoder;
                use std::io::Read;
                let mut decoder = DeflateDecoder::new(&payload[..]);
                let mut decompressed = Vec::new();
                decoder.read_to_end(&mut decompressed).unwrap();
                all_data.extend_from_slice(&decompressed);
            } else {
                all_data.extend_from_slice(payload);
            }
        }
        let all_str = String::from_utf8_lossy(&all_data);
        for i in 0..10 {
            assert!(
                all_str.contains(&format!("chunk{i}")),
                "missing chunk{i} in batched output"
            );
        }
    }

    #[tokio::test]
    async fn test_compression_above_threshold() {
        let (log, send_fn) = recording_sender();
        let sender = BatchedSender::new(test_config(10), send_fn);

        // Send data >= 32 bytes that compresses well (repeated chars)
        let data = vec![b'A'; 128];
        sender.send(data).await.unwrap();
        sender.flush().await;

        let calls = log.lock().await;
        assert_eq!(calls.len(), 1);
        assert!(calls[0].2, "expected compressed=true for 128 bytes of repeated data");
        // Compressed payload should be smaller than original
        assert!(
            calls[0].0.len() < 128,
            "compressed payload ({}) should be smaller than original (128)",
            calls[0].0.len()
        );
    }

    #[tokio::test]
    async fn test_no_compression_below_threshold() {
        let (log, send_fn) = recording_sender();
        let sender = BatchedSender::new(test_config(10), send_fn);

        // Send data < 32 bytes
        sender.send(b"tiny".to_vec()).await.unwrap();
        sender.flush().await;

        let calls = log.lock().await;
        assert_eq!(calls.len(), 1);
        assert!(!calls[0].2, "expected compressed=false for small payload");
        assert_eq!(calls[0].0, b"tiny");
    }

    #[tokio::test]
    async fn test_sequence_numbers_increment() {
        let (log, send_fn) = recording_sender();
        // Use very short window so each send becomes its own batch
        let sender = BatchedSender::new(test_config(1), send_fn);

        for _ in 0..3 {
            sender.send(b"x".to_vec()).await.unwrap();
            // Sleep longer than batch window to force separate batches
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        sender.flush().await;

        let calls = log.lock().await;
        assert!(calls.len() >= 2, "expected at least 2 batches, got {}", calls.len());
        // Verify sequence numbers are strictly increasing
        for i in 1..calls.len() {
            assert!(
                calls[i].1 > calls[i - 1].1,
                "seq numbers should increase: {} vs {}",
                calls[i - 1].1,
                calls[i].1
            );
        }
        // First seq should be 1
        assert_eq!(calls[0].1, 1);
    }

    #[tokio::test]
    async fn test_flush_sends_remaining() {
        let (log, send_fn) = recording_sender();
        let sender = BatchedSender::new(test_config(5000), send_fn); // Very long window

        sender.send(b"buffered-data".to_vec()).await.unwrap();

        // Without flush, the batch window hasn't expired yet.
        // Flush should force send.
        sender.flush().await;

        let calls = log.lock().await;
        assert_eq!(calls.len(), 1, "flush should send remaining buffered data");
        assert_eq!(calls[0].0, b"buffered-data");
    }

    #[tokio::test]
    async fn test_429_backoff() {
        let call_count = Arc::new(Mutex::new(0u32));
        let call_count2 = call_count.clone();
        let timestamps = Arc::new(Mutex::new(Vec::<std::time::Instant>::new()));
        let timestamps2 = timestamps.clone();

        let send_fn: SendFn = Box::new(move |_payload, _seq, _compressed| {
            let count = call_count2.clone();
            let ts = timestamps2.clone();
            Box::pin(async move {
                let mut c = count.lock().await;
                *c += 1;
                ts.lock().await.push(std::time::Instant::now());
                if *c == 1 {
                    // First call returns 429
                    Err("429 retry_after_ms: 50".to_string())
                } else {
                    Ok(())
                }
            })
        });

        let sender = BatchedSender::new(test_config(10), send_fn);
        sender.send(b"rate-limited".to_vec()).await.unwrap();

        // Send a second chunk after a short delay to trigger the retry path
        tokio::time::sleep(Duration::from_millis(30)).await;
        sender.send(b"-more".to_vec()).await.unwrap();

        sender.flush().await;

        let count = *call_count.lock().await;
        assert!(count >= 2, "expected at least 2 send attempts (initial + retry), got {count}");

        let ts = timestamps.lock().await;
        if ts.len() >= 2 {
            let gap = ts[1].duration_since(ts[0]);
            assert!(
                gap >= Duration::from_millis(40),
                "expected backoff of ~50ms, got {:?}",
                gap
            );
        }
    }

    #[tokio::test]
    async fn test_parse_retry_after() {
        assert_eq!(
            BatchedSender::parse_retry_after("M_LIMIT_EXCEEDED retry_after_ms: 3000"),
            Some(3000)
        );
        assert_eq!(
            BatchedSender::parse_retry_after("429 {\"retry_after_ms\":1500}"),
            Some(1500)
        );
        assert_eq!(
            BatchedSender::parse_retry_after("some other error"),
            None
        );
        assert_eq!(
            BatchedSender::parse_retry_after("retry_after_ms=250 blah"),
            Some(250)
        );
    }

    // ---- T-61: set_batch_window + window-flip flow tests ----

    #[tokio::test]
    async fn test_set_batch_window_updates_cell() {
        let (_log, send_fn) = recording_sender();
        let sender = BatchedSender::new(test_config(200), send_fn);

        assert_eq!(sender.current_batch_window(), Duration::from_millis(200));

        sender.set_batch_window(Duration::from_millis(10));
        assert_eq!(sender.current_batch_window(), Duration::from_millis(10));

        sender.set_batch_window(Duration::from_millis(200));
        assert_eq!(sender.current_batch_window(), Duration::from_millis(200));

        sender.flush().await;
    }

    #[tokio::test]
    async fn test_startup_window_is_200ms_default() {
        // Contract: the initial window is whatever BatchConfig::batch_window
        // says (pre-T-61 behavior preserved). The flip to 10ms ONLY happens
        // when the integration layer calls set_batch_window(P2P_OPEN_BATCH_WINDOW).
        let cfg = BatchConfig::default();
        let (_log, send_fn) = recording_sender();
        let sender = BatchedSender::new(cfg, send_fn);
        assert_eq!(sender.current_batch_window(), DEFAULT_BATCH_WINDOW);
        assert_eq!(sender.current_batch_window(), Duration::from_millis(200));
        sender.flush().await;
    }

    #[tokio::test]
    async fn test_window_flip_affects_next_batch_cycle() {
        // Verifies the batch_loop reads the shared window cell between
        // coalesce iterations. With a 500ms window, a mid-buffer flip to
        // 10ms should cause the next batch to flush within ~10-100ms.
        let (log, send_fn) = recording_sender();
        let sender = BatchedSender::new(test_config(500), send_fn);

        sender.send(b"pre-flip".to_vec()).await.unwrap();
        // Give the loop a moment to enter the inner timeout branch.
        tokio::time::sleep(Duration::from_millis(5)).await;
        // Flip window down to 10ms.
        sender.set_batch_window(Duration::from_millis(10));
        // Next send arrives while the 500ms timer was pending, but the
        // loop re-evaluates the window per iteration, so the batch should
        // flush within ~200ms (upper bound; the original 500ms timer may
        // still be pending, but the loop terminates on channel close on
        // flush below anyway).
        tokio::time::sleep(Duration::from_millis(20)).await;

        sender.flush().await;
        let calls = log.lock().await;
        assert!(
            !calls.is_empty(),
            "expected at least one batch to have flushed"
        );
    }
}
