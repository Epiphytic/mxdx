use anyhow::Result;

// ---------------------------------------------------------------------------
// WebRTC manager stub — actual implementation in Phase 6
// ---------------------------------------------------------------------------

/// Stub WebRTC manager. All methods return errors or false until Phase 6
/// provides the real implementation.
pub struct WebRtcManager;

impl WebRtcManager {
    pub fn new() -> Self {
        Self
    }

    /// Initiate a WebRTC offer for an interactive session.
    ///
    /// Stub: returns not-implemented error. Interactive sessions fall back to
    /// Matrix thread transport until Phase 6.
    pub async fn initiate_offer(&self, _session_uuid: &str) -> Result<()> {
        tracing::warn!(
            "WebRTC not yet implemented, interactive sessions will use Matrix thread only"
        );
        Err(anyhow::anyhow!("WebRTC not implemented"))
    }

    /// Handle an incoming WebRTC answer (stub).
    pub async fn handle_answer(&self, _session_uuid: &str) -> Result<()> {
        Err(anyhow::anyhow!("WebRTC not implemented"))
    }

    /// Check if WebRTC transport is available.
    pub fn is_available(&self) -> bool {
        false
    }
}

impl Default for WebRtcManager {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn initiate_offer_returns_err() {
        let mgr = WebRtcManager::new();
        let result = mgr.initiate_offer("test-session").await;
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("not implemented"),
            "error message should mention not implemented"
        );
    }

    #[tokio::test]
    async fn handle_answer_returns_err() {
        let mgr = WebRtcManager::new();
        let result = mgr.handle_answer("test-session").await;
        assert!(result.is_err());
    }

    #[test]
    fn is_available_returns_false() {
        let mgr = WebRtcManager::new();
        assert!(!mgr.is_available());
    }
}
