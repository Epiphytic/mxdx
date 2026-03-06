use crate::terminal::compression::{compress_encode, decode_decompress_bounded};
use crate::terminal::ring_buffer::EventRingBuffer;
use crate::terminal::tmux::TmuxSession;

pub struct TerminalSession {
    pub session_id: String,
    pub tmux: TmuxSession,
    pub ring_buffer: EventRingBuffer<Vec<u8>>,
    pub seq: u64,
    pub dm_room_id: Option<String>,
}

impl TerminalSession {
    pub async fn create(
        session_id: &str,
        command: &str,
        cols: u32,
        rows: u32,
    ) -> Result<Self, anyhow::Error> {
        let tmux = TmuxSession::create(session_id, command, cols, rows).await?;
        Ok(Self {
            session_id: session_id.to_string(),
            tmux,
            ring_buffer: EventRingBuffer::new(1000),
            seq: 0,
            dm_room_id: None,
        })
    }

    /// Process incoming terminal data (from user via Matrix).
    pub async fn handle_input(
        &self,
        encoded_data: &str,
        encoding: &str,
    ) -> Result<(), anyhow::Error> {
        let data = decode_decompress_bounded(encoded_data, encoding, 1_048_576)?;
        let input = std::str::from_utf8(&data)?;
        self.tmux.send_input(input).await?;
        Ok(())
    }

    /// Capture current output and return as compressed data with seq number.
    pub async fn capture_output(
        &mut self,
    ) -> Result<Option<(String, String, u64)>, anyhow::Error> {
        let output = self.tmux.capture_pane().await?;
        if output.is_empty() {
            return Ok(None);
        }
        let (encoded, encoding) = compress_encode(output.as_bytes());
        let seq = self.seq;
        self.ring_buffer.push(seq, output.into_bytes());
        self.seq += 1;
        Ok(Some((encoded, encoding, seq)))
    }

    pub async fn resize(&self, cols: u32, rows: u32) -> Result<(), anyhow::Error> {
        self.tmux.resize(cols, rows).await
    }

    pub async fn kill(self) -> Result<(), anyhow::Error> {
        self.tmux.kill().await
    }
}
