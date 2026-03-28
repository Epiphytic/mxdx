use anyhow::Result;
use mxdx_types::events::webrtc::{WebRtcAnswer, WebRtcIce, WebRtcOffer, WebRtcSdp};

/// WebRTC acceleration manager for interactive sessions.
///
/// Architecture (split signaling model):
/// - Thread events (WebRtcOffer/Answer): metadata only, auditable, no crypto material
/// - To-device messages (WebRtcSdp/Ice): private SDP/ICE/keys, Olm-encrypted
///
/// Data flow:
/// 1. Client sends WebRtcOffer thread event (metadata) + WebRtcSdp to-device (SDP+pubkey)
/// 2. Worker receives both, creates DataChannel, sends WebRtcAnswer + WebRtcSdp back
/// 3. ICE candidates exchanged via to-device messages
/// 4. DataChannel established with app-level E2EE (ephemeral Curve25519 key exchange)
///
/// Failover:
/// - On ICE disconnect: worker continues posting to Matrix thread
/// - Client falls back to thread tailing
/// - On reconnect: fresh key exchange, new DataChannel
pub struct WebRtcManager {
    available: bool,
}

impl WebRtcManager {
    pub fn new() -> Self {
        Self { available: false }
    }

    /// Check if WebRTC is available on this platform
    pub fn is_available(&self) -> bool {
        self.available
    }

    /// Create an offer for an interactive session (worker side)
    pub async fn create_offer(
        &self,
        session_uuid: &str,
        device_id: &str,
    ) -> Result<(WebRtcOffer, WebRtcSdp)> {
        if !self.available {
            anyhow::bail!("WebRTC not available — interactive sessions fall back to Matrix thread");
        }
        // Future: use webrtc-rs to create PeerConnection, generate SDP offer
        let offer = WebRtcOffer {
            session_uuid: session_uuid.to_string(),
            device_id: device_id.to_string(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs(),
        };
        let sdp = WebRtcSdp {
            session_uuid: session_uuid.to_string(),
            sdp_type: "offer".to_string(),
            sdp: String::new(),            // Placeholder — real SDP from webrtc-rs
            e2ee_public_key: String::new(), // Placeholder — ephemeral Curve25519 key
        };
        Ok((offer, sdp))
    }

    /// Handle an incoming answer (worker side)
    pub async fn handle_answer(&self, _answer: &WebRtcAnswer, _sdp: &WebRtcSdp) -> Result<()> {
        if !self.available {
            anyhow::bail!("WebRTC not available");
        }
        // Future: set remote description, complete ICE
        Ok(())
    }

    /// Handle an incoming ICE candidate
    pub async fn handle_ice(&self, _ice: &WebRtcIce) -> Result<()> {
        if !self.available {
            anyhow::bail!("WebRTC not available");
        }
        Ok(())
    }

    /// Gracefully close a DataChannel
    pub async fn close(&self, _session_uuid: &str) -> Result<()> {
        Ok(())
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

    #[test]
    fn new_is_not_available() {
        let mgr = WebRtcManager::new();
        assert!(!mgr.is_available());
    }

    #[tokio::test]
    async fn create_offer_returns_err_when_unavailable() {
        let mgr = WebRtcManager::new();
        let result = mgr.create_offer("test-session", "DEVICE1").await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("not available"),
            "error message should mention not available"
        );
    }

    #[tokio::test]
    async fn handle_answer_returns_err_when_unavailable() {
        let mgr = WebRtcManager::new();
        let answer = WebRtcAnswer {
            session_uuid: "s1".into(),
            device_id: "d1".into(),
            timestamp: 0,
        };
        let sdp = WebRtcSdp {
            session_uuid: "s1".into(),
            sdp_type: "answer".into(),
            sdp: String::new(),
            e2ee_public_key: String::new(),
        };
        let result = mgr.handle_answer(&answer, &sdp).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn handle_ice_returns_err_when_unavailable() {
        let mgr = WebRtcManager::new();
        let ice = WebRtcIce {
            session_uuid: "s1".into(),
            candidate: "candidate:1".into(),
            sdp_mid: Some("0".into()),
            sdp_mline_index: Some(0),
        };
        let result = mgr.handle_ice(&ice).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn close_succeeds_even_when_unavailable() {
        let mgr = WebRtcManager::new();
        assert!(mgr.close("test-session").await.is_ok());
    }
}
