use serde::{Deserialize, Serialize};

// --- Event type string constants ---

pub const WEBRTC_OFFER: &str = "org.mxdx.session.webrtc.offer";
pub const WEBRTC_ANSWER: &str = "org.mxdx.session.webrtc.answer";
pub const WEBRTC_SDP: &str = "org.mxdx.webrtc.sdp";
pub const WEBRTC_ICE: &str = "org.mxdx.webrtc.ice";

// --- Thread events (metadata only, no crypto material) ---

/// Thread event: WebRTC offer signal posted to the session thread.
/// Contains only metadata; actual SDP is sent via to-device (Olm-encrypted).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WebRtcOffer {
    pub session_uuid: String,
    pub device_id: String,
    pub timestamp: u64,
}

/// Thread event: WebRTC answer signal posted to the session thread.
/// Contains only metadata; actual SDP is sent via to-device (Olm-encrypted).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WebRtcAnswer {
    pub session_uuid: String,
    pub device_id: String,
    pub timestamp: u64,
}

// --- To-device messages (private, Olm-encrypted) ---

/// To-device message carrying SDP offer or answer.
/// Sent via Olm encryption, never posted to a room timeline.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WebRtcSdp {
    pub session_uuid: String,
    pub sdp_type: String,
    pub sdp: String,
    pub e2ee_public_key: String,
}

/// To-device message carrying an ICE candidate.
/// Sent via Olm encryption, never posted to a room timeline.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WebRtcIce {
    pub session_uuid: String,
    pub candidate: String,
    pub sdp_mid: Option<String>,
    pub sdp_mline_index: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webrtc_offer_roundtrip() {
        let offer = WebRtcOffer {
            session_uuid: "sess-001".into(),
            device_id: "ABCDEF123".into(),
            timestamp: 1742572800,
        };
        let json = serde_json::to_string(&offer).unwrap();
        let back: WebRtcOffer = serde_json::from_str(&json).unwrap();
        assert_eq!(back.session_uuid, "sess-001");
        assert_eq!(back.device_id, "ABCDEF123");
        assert_eq!(back.timestamp, 1742572800);
    }

    #[test]
    fn webrtc_answer_roundtrip() {
        let answer = WebRtcAnswer {
            session_uuid: "sess-001".into(),
            device_id: "XYZABC789".into(),
            timestamp: 1742572801,
        };
        let json = serde_json::to_string(&answer).unwrap();
        let back: WebRtcAnswer = serde_json::from_str(&json).unwrap();
        assert_eq!(back.session_uuid, "sess-001");
        assert_eq!(back.device_id, "XYZABC789");
        assert_eq!(back.timestamp, 1742572801);
    }

    #[test]
    fn webrtc_sdp_roundtrip() {
        let sdp = WebRtcSdp {
            session_uuid: "sess-001".into(),
            sdp_type: "offer".into(),
            sdp: "v=0\r\no=- 123456 2 IN IP4 127.0.0.1\r\n".into(),
            e2ee_public_key: "ed25519:AAAAAA+base64key".into(),
        };
        let json = serde_json::to_string(&sdp).unwrap();
        let back: WebRtcSdp = serde_json::from_str(&json).unwrap();
        assert_eq!(back.session_uuid, "sess-001");
        assert_eq!(back.sdp_type, "offer");
        assert!(back.sdp.contains("v=0"));
        assert_eq!(back.e2ee_public_key, "ed25519:AAAAAA+base64key");
    }

    #[test]
    fn webrtc_ice_roundtrip() {
        let ice = WebRtcIce {
            session_uuid: "sess-001".into(),
            candidate: "candidate:1 1 UDP 2122252543 192.168.1.1 12345 typ host".into(),
            sdp_mid: Some("0".into()),
            sdp_mline_index: Some(0),
        };
        let json = serde_json::to_string(&ice).unwrap();
        let back: WebRtcIce = serde_json::from_str(&json).unwrap();
        assert_eq!(back.session_uuid, "sess-001");
        assert!(back.candidate.contains("candidate:1"));
        assert_eq!(back.sdp_mid, Some("0".into()));
        assert_eq!(back.sdp_mline_index, Some(0));
    }

    #[test]
    fn webrtc_ice_optional_fields() {
        let ice = WebRtcIce {
            session_uuid: "sess-002".into(),
            candidate: "candidate:2 1 TCP 1518280447 10.0.0.1 9999 typ srflx".into(),
            sdp_mid: None,
            sdp_mline_index: None,
        };
        let json = serde_json::to_string(&ice).unwrap();
        let back: WebRtcIce = serde_json::from_str(&json).unwrap();
        assert_eq!(back.sdp_mid, None);
        assert_eq!(back.sdp_mline_index, None);
    }

    #[test]
    fn webrtc_sdp_answer_type() {
        let sdp = WebRtcSdp {
            session_uuid: "sess-001".into(),
            sdp_type: "answer".into(),
            sdp: "v=0\r\no=- 654321 2 IN IP4 10.0.0.1\r\n".into(),
            e2ee_public_key: "ed25519:BBBBBB+base64key".into(),
        };
        let json = serde_json::to_string(&sdp).unwrap();
        let back: WebRtcSdp = serde_json::from_str(&json).unwrap();
        assert_eq!(back.sdp_type, "answer");
    }

    #[test]
    fn webrtc_snake_case_fields() {
        let sdp = WebRtcSdp {
            session_uuid: "s-1".into(),
            sdp_type: "offer".into(),
            sdp: "v=0".into(),
            e2ee_public_key: "key".into(),
        };
        let json = serde_json::to_string(&sdp).unwrap();
        assert!(json.contains("session_uuid"), "expected snake_case in: {json}");
        assert!(json.contains("sdp_type"), "expected snake_case in: {json}");
        assert!(json.contains("e2ee_public_key"), "expected snake_case in: {json}");

        let ice = WebRtcIce {
            session_uuid: "s-1".into(),
            candidate: "c".into(),
            sdp_mid: Some("0".into()),
            sdp_mline_index: Some(0),
        };
        let json = serde_json::to_string(&ice).unwrap();
        assert!(json.contains("sdp_mid"), "expected snake_case in: {json}");
        assert!(json.contains("sdp_mline_index"), "expected snake_case in: {json}");
    }

    #[test]
    fn webrtc_event_type_constants() {
        assert_eq!(WEBRTC_OFFER, "org.mxdx.session.webrtc.offer");
        assert_eq!(WEBRTC_ANSWER, "org.mxdx.session.webrtc.answer");
        assert_eq!(WEBRTC_SDP, "org.mxdx.webrtc.sdp");
        assert_eq!(WEBRTC_ICE, "org.mxdx.webrtc.ice");
    }
}
