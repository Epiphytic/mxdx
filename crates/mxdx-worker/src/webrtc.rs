use anyhow::Result;
use mxdx_types::events::webrtc::{WebRtcAnswer, WebRtcIce, WebRtcOffer, WebRtcSdp};
use serde::{Deserialize, Serialize};

/// TURN relay configuration, populated from the state room.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnConfig {
    pub uris: Vec<String>,
    pub username: Option<String>,
    pub password: Option<String>,
}

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
    turn_config: Option<TurnConfig>,
}

impl WebRtcManager {
    pub fn new() -> Self {
        Self {
            available: false,
            turn_config: None,
        }
    }

    /// Set the TURN relay configuration from the state room.
    pub fn set_turn_config(&mut self, config: TurnConfig) {
        self.turn_config = Some(config);
    }

    /// Get the current TURN relay configuration, if any.
    pub fn turn_config(&self) -> Option<&TurnConfig> {
        self.turn_config.as_ref()
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

    #[test]
    fn turn_config_none_by_default() {
        let mgr = WebRtcManager::new();
        assert!(mgr.turn_config().is_none());
    }

    #[test]
    fn set_and_get_turn_config() {
        let mut mgr = WebRtcManager::new();
        let config = TurnConfig {
            uris: vec!["turn:relay.example.com:3478".into()],
            username: Some("user".into()),
            password: Some("pass".into()),
        };
        mgr.set_turn_config(config);

        let stored = mgr.turn_config().unwrap();
        assert_eq!(stored.uris.len(), 1);
        assert_eq!(stored.uris[0], "turn:relay.example.com:3478");
        assert_eq!(stored.username, Some("user".into()));
        assert_eq!(stored.password, Some("pass".into()));
    }

    #[test]
    fn turn_config_serializes() {
        let config = TurnConfig {
            uris: vec!["turn:relay.example.com:3478".into()],
            username: None,
            password: None,
        };
        let json = serde_json::to_string(&config).unwrap();
        let back: TurnConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.uris.len(), 1);
        assert!(back.username.is_none());
    }
}
