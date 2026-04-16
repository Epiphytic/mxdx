//! Verifying handshake — Ed25519-signed transcript over AES-GCM (T-53).
//!
//! Implemented in T-53 (mxdx-awe.24). This module is declared in T-51 so
//! the driver can reference its surface (`build_transcript`, `verify`,
//! `sign`) once T-53 fills in.

/// Domain-separation tag for the Verifying transcript. ASCII constant,
/// versioned. Storm §3.1 / §4.5. MUST match the npm side when T-53's
/// coordinated-release ships.
pub const DOMAIN_SEPARATION_TAG: &[u8] = b"mxdx.p2p.verify.v1";

/// Verifying-handshake frame envelope (wire shape — T-53 wires the actual
/// emit/parse paths). Carried AES-GCM-encrypted on the P2P channel.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum HandshakeMsg {
    #[serde(rename = "verify_challenge")]
    Challenge {
        nonce_b64: String,
        device_id: String,
    },
    #[serde(rename = "verify_response")]
    Response {
        nonce_b64: String,
        signature_b64: String,
        device_id: String,
        signer_ed25519_b64: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_separation_tag_is_versioned_and_ascii() {
        assert_eq!(DOMAIN_SEPARATION_TAG, b"mxdx.p2p.verify.v1");
        assert!(DOMAIN_SEPARATION_TAG.iter().all(|b| b.is_ascii()));
    }

    #[test]
    fn handshake_msg_challenge_roundtrips() {
        let m = HandshakeMsg::Challenge {
            nonce_b64: "abc".into(),
            device_id: "DEV".into(),
        };
        let json = serde_json::to_string(&m).unwrap();
        assert!(json.contains("\"type\":\"verify_challenge\""));
        let back: HandshakeMsg = serde_json::from_str(&json).unwrap();
        assert_eq!(back, m);
    }
}
