//! Standard Matrix VoIP `m.call.*` event types, serde-wired to match the
//! deployed npm emitter in `packages/core/p2p-signaling.js`.
//!
//! Wire-format contract (see ADR `docs/adr/2026-04-15-mcall-wire-format.md`
//! and its 2026-04-16 addendum):
//! - All five event types (`CallInvite`, `CallAnswer`, `CallCandidates`,
//!   `CallHangup`, `CallSelectAnswer`) use the standard Matrix VoIP field
//!   names (`call_id`, `party_id`, `version`, `lifetime`, `offer`, `answer`,
//!   `candidates`, `reason`, `selected_party_id`) and `version: "1"`.
//! - `m.call.invite` carries a single mxdx extension field,
//!   `mxdx_session_key`: base64 of the 32-byte AES-256 session key. Room
//!   E2EE (Megolm + MSC4362) protects this field from passive observers.
//! - `CallIceCandidate` uses the wire names `sdpMid` and `sdpMLineIndex`
//!   (camelCase, standard Matrix VoIP) — the npm emitter populates the
//!   former; the npm browser emitter populates both. Both fields are
//!   `Option` and absent from the wire when `None`.
//! - `session_uuid` on invites is optional and absent from the wire when
//!   `None` (matches deployed npm, which does not populate it on `m.call.*`).
//!
//! Coordinated-release policy (ADR `docs/adr/2026-04-16-coordinated-rust-npm-releases.md`):
//! any rename or shape change in this file MUST land in the same branch as
//! the corresponding change to `packages/core/p2p-signaling.js`.
//!
//! # SealedKey sealing invariant
//!
//! [`build_invite`] is the ONLY public path that embeds a
//! [`crate::crypto::SealedKey`] in an outgoing event. Raw key bytes are not
//! reachable from this module — `SealedKey::to_base64` lives in
//! `crypto.rs` so this module can encode a key
//! for the wire without ever touching its bytes. See trybuild negative
//! tests in `tests/trybuild/sealedkey-constructor-fails.rs` (T-13) for
//! compile-time enforcement.

use serde::{Deserialize, Serialize};

use crate::crypto::SealedKey;

/// Default value for `m.call.invite.lifetime` (milliseconds).
///
/// Per ADR `2026-04-15-mcall-wire-format.md` 2026-04-16 addendum: both Rust
/// and npm use 30_000 ms. npm callers in `packages/launcher/src/runtime.js`
/// and `packages/web-console/src/terminal-view.js` already pass explicit
/// `lifetime: 30000`; the coordinated-release npm change (T-44) updates the
/// default in `packages/core/p2p-signaling.js` from 60_000 → 30_000 so the
/// two runtimes agree on the default at the API boundary as well.
pub const DEFAULT_INVITE_LIFETIME_MS: u64 = 30_000;

/// The version string shared by every Matrix VoIP event we emit. Spec
/// requires `"1"` for the current Matrix call protocol revision; npm hard-
/// codes the same value.
pub const CALL_VERSION: &str = "1";

/// Offer or answer SDP field on `m.call.invite` / `m.call.answer`. On the
/// wire: `{ "type": "offer" | "answer", "sdp": "..." }`. The `kind` field is
/// renamed to `type` because `type` is a Rust keyword.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallSdpField {
    #[serde(rename = "type")]
    pub kind: String,
    pub sdp: String,
}

/// A single ICE candidate as carried in `m.call.candidates`. Camel-case wire
/// names (`sdpMid`, `sdpMLineIndex`) match the standard Matrix VoIP spec and
/// the deployed npm emitters. Both optional fields are omitted from the
/// serialized output when `None`, matching the Node-side emitter in
/// `packages/core/webrtc-channel-node.js:64`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallIceCandidate {
    pub candidate: String,
    #[serde(rename = "sdpMid", skip_serializing_if = "Option::is_none", default)]
    pub sdp_mid: Option<String>,
    #[serde(
        rename = "sdpMLineIndex",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub sdp_mline_index: Option<u32>,
}

/// `m.call.invite` event content.
///
/// Field order matches `packages/core/p2p-signaling.js:46-53`:
/// `call_id, party_id, version, lifetime, offer, [mxdx_session_key], [session_uuid]`.
/// serde preserves struct declaration order when serializing to JSON, so the
/// emitted byte sequence matches the npm emitter for any invite with the
/// same inputs.
///
/// `mxdx_session_key` carries the base64 AES-256 session key and is absent
/// from the wire when `None`. Construct via [`build_invite`] — the field is
/// `pub` for deserialization and inspection in tests, but external emitters
/// should use the builder so the [`SealedKey`]-to-base64 step stays in one
/// auditable place.
///
/// `Debug` is implemented by hand to redact `mxdx_session_key` so the
/// base64 key never leaks through structured logs. The locked behavior is
/// asserted by `tests::invite_debug_redacts_mxdx_session_key`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallInvite {
    pub call_id: String,
    pub party_id: String,
    pub version: String,
    pub lifetime: u64,
    pub offer: CallSdpField,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub mxdx_session_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub session_uuid: Option<String>,
}

impl core::fmt::Debug for CallInvite {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CallInvite")
            .field("call_id", &self.call_id)
            .field("party_id", &self.party_id)
            .field("version", &self.version)
            .field("lifetime", &self.lifetime)
            .field("offer", &self.offer)
            .field(
                "mxdx_session_key",
                &self
                    .mxdx_session_key
                    .as_ref()
                    .map(|_| "<redacted>")
                    .unwrap_or("None"),
            )
            .field("session_uuid", &self.session_uuid)
            .finish()
    }
}

/// `m.call.answer` event content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallAnswer {
    pub call_id: String,
    pub party_id: String,
    pub version: String,
    pub answer: CallSdpField,
}

/// `m.call.candidates` event content. ICE candidates are batched by Phase 5's
/// transport driver; the serialized order matches insertion order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallCandidates {
    pub call_id: String,
    pub party_id: String,
    pub version: String,
    pub candidates: Vec<CallIceCandidate>,
}

/// `m.call.hangup` event content. `reason` is optional on the wire per the
/// Matrix VoIP spec; npm populates it with `"user_hangup"` by default but
/// accepts absence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallHangup {
    pub call_id: String,
    pub party_id: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reason: Option<String>,
}

/// `m.call.select_answer` event content. Used for glare resolution: the
/// offerer whose invite won sends this to nominate one of the received
/// answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallSelectAnswer {
    pub call_id: String,
    pub party_id: String,
    pub version: String,
    pub selected_party_id: String,
}

// ---------------------------------------------------------------------------
// Builders
// ---------------------------------------------------------------------------

/// Build a `CallInvite` with an embedded `SealedKey`.
///
/// This is the **only** public entry point that transports a [`SealedKey`]
/// out of the `mxdx-p2p` crate — the key is consumed, base64-encoded via
/// `SealedKey::to_base64` (which itself wraps the sealed `as_bytes()`
/// accessor inside the crypto module), and embedded in the
/// `mxdx_session_key` field. The caller never sees raw key bytes.
///
/// # Parameters
/// - `call_id`, `party_id`: standard Matrix VoIP identifiers (generate via
///   `P2PSignaling::generateCallId`/`generatePartyId` on the npm side or any
///   equivalent random hex generator on the Rust side — these are just
///   opaque strings on the wire).
/// - `sdp_offer`: local SDP offer from `WebRtcChannel::create_offer`.
/// - `sealed_key`: consumed; embedded base64 in `mxdx_session_key`.
/// - `lifetime`: invite validity in milliseconds. Use
///   [`DEFAULT_INVITE_LIFETIME_MS`] unless there's a specific reason not to.
/// - `session_uuid`: optional mxdx session identifier; omitted from the wire
///   when `None` (matches deployed npm, which does not populate it on
///   `m.call.*` events).
pub fn build_invite(
    call_id: impl Into<String>,
    party_id: impl Into<String>,
    sdp_offer: impl Into<String>,
    sealed_key: SealedKey,
    lifetime: u64,
    session_uuid: Option<String>,
) -> CallInvite {
    let mxdx_session_key = Some(sealed_key.to_base64());
    // `sealed_key` dropped here — its bytes are zeroized by `SealedKey::drop`.
    CallInvite {
        call_id: call_id.into(),
        party_id: party_id.into(),
        version: CALL_VERSION.to_string(),
        lifetime,
        offer: CallSdpField {
            kind: "offer".to_string(),
            sdp: sdp_offer.into(),
        },
        mxdx_session_key,
        session_uuid,
    }
}

/// Build a `CallAnswer`. Pure constructor — no sealed data involved.
pub fn build_answer(
    call_id: impl Into<String>,
    party_id: impl Into<String>,
    sdp_answer: impl Into<String>,
) -> CallAnswer {
    CallAnswer {
        call_id: call_id.into(),
        party_id: party_id.into(),
        version: CALL_VERSION.to_string(),
        answer: CallSdpField {
            kind: "answer".to_string(),
            sdp: sdp_answer.into(),
        },
    }
}

/// Build a `CallCandidates`. Pure constructor.
pub fn build_candidates(
    call_id: impl Into<String>,
    party_id: impl Into<String>,
    candidates: Vec<CallIceCandidate>,
) -> CallCandidates {
    CallCandidates {
        call_id: call_id.into(),
        party_id: party_id.into(),
        version: CALL_VERSION.to_string(),
        candidates,
    }
}

/// Build a `CallHangup`. Pure constructor.
pub fn build_hangup(
    call_id: impl Into<String>,
    party_id: impl Into<String>,
    reason: Option<String>,
) -> CallHangup {
    CallHangup {
        call_id: call_id.into(),
        party_id: party_id.into(),
        version: CALL_VERSION.to_string(),
        reason,
    }
}

/// Build a `CallSelectAnswer`. Pure constructor.
pub fn build_select_answer(
    call_id: impl Into<String>,
    party_id: impl Into<String>,
    selected_party_id: impl Into<String>,
) -> CallSelectAnswer {
    CallSelectAnswer {
        call_id: call_id.into(),
        party_id: party_id.into(),
        version: CALL_VERSION.to_string(),
        selected_party_id: selected_party_id.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::P2PCrypto;
    use serde_json::Value;

    #[test]
    fn invite_roundtrips_with_session_key() {
        let (_crypto, sealed) = P2PCrypto::generate();
        let invite = build_invite(
            "c1",
            "p1",
            "v=0\r\n...",
            sealed,
            DEFAULT_INVITE_LIFETIME_MS,
            None,
        );
        let json = serde_json::to_string(&invite).unwrap();
        let back: CallInvite = serde_json::from_str(&json).unwrap();
        assert_eq!(back, invite);
        assert!(back.mxdx_session_key.is_some());
        assert!(back.session_uuid.is_none());
    }

    #[test]
    fn invite_serialized_field_order_matches_npm() {
        // npm field order: call_id, party_id, version, lifetime, offer, [mxdx_session_key]
        let (_crypto, sealed) = P2PCrypto::generate();
        let invite = build_invite("c1", "p1", "sdp", sealed, 30_000, None);
        let json = serde_json::to_string(&invite).unwrap();
        // Assert field order by character position. serde_json preserves
        // struct declaration order, so this locks the wire byte sequence.
        let call_id_pos = json.find("\"call_id\"").unwrap();
        let party_id_pos = json.find("\"party_id\"").unwrap();
        let version_pos = json.find("\"version\"").unwrap();
        let lifetime_pos = json.find("\"lifetime\"").unwrap();
        let offer_pos = json.find("\"offer\"").unwrap();
        let session_key_pos = json.find("\"mxdx_session_key\"").unwrap();
        assert!(call_id_pos < party_id_pos);
        assert!(party_id_pos < version_pos);
        assert!(version_pos < lifetime_pos);
        assert!(lifetime_pos < offer_pos);
        assert!(offer_pos < session_key_pos);
    }

    #[test]
    fn invite_omits_optional_fields_when_none() {
        // Build a bare invite directly (not via build_invite, which always
        // populates mxdx_session_key).
        let invite = CallInvite {
            call_id: "c1".into(),
            party_id: "p1".into(),
            version: CALL_VERSION.into(),
            lifetime: 30_000,
            offer: CallSdpField {
                kind: "offer".into(),
                sdp: "sdp".into(),
            },
            mxdx_session_key: None,
            session_uuid: None,
        };
        let json = serde_json::to_string(&invite).unwrap();
        assert!(
            !json.contains("mxdx_session_key"),
            "expected mxdx_session_key absent, got: {json}"
        );
        assert!(
            !json.contains("session_uuid"),
            "expected session_uuid absent, got: {json}"
        );
    }

    #[test]
    fn answer_matches_npm_shape() {
        let answer = build_answer("c1", "p1", "sdp-answer");
        let json = serde_json::to_string(&answer).unwrap();
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["call_id"], "c1");
        assert_eq!(v["party_id"], "p1");
        assert_eq!(v["version"], "1");
        assert_eq!(v["answer"]["type"], "answer");
        assert_eq!(v["answer"]["sdp"], "sdp-answer");
    }

    #[test]
    fn candidates_matches_npm_shape_sdpmid_camel_case() {
        let candidates = build_candidates(
            "c1",
            "p1",
            vec![
                CallIceCandidate {
                    candidate: "c1-payload".into(),
                    sdp_mid: Some("0".into()),
                    sdp_mline_index: None,
                },
                CallIceCandidate {
                    candidate: "c2-payload".into(),
                    sdp_mid: Some("0".into()),
                    sdp_mline_index: Some(0),
                },
            ],
        );
        let json = serde_json::to_string(&candidates).unwrap();
        // sdpMid camel-case (npm convention + Matrix VoIP spec).
        assert!(json.contains("\"sdpMid\":\"0\""));
        // sdpMLineIndex camel-case only populated on browser-emitted candidates.
        assert!(json.contains("\"sdpMLineIndex\":0"));
        // First candidate has no sdpMLineIndex → must not pollute wire.
        let first = json.split("\"candidate\":\"c1-payload\"").nth(1).unwrap();
        let first_end = first.find("},").unwrap();
        let first_slice = &first[..first_end];
        assert!(
            !first_slice.contains("sdpMLineIndex"),
            "sdpMLineIndex should be absent when None, got: {first_slice}"
        );
    }

    #[test]
    fn hangup_reason_optional_on_wire() {
        let with_reason = build_hangup("c1", "p1", Some("idle_timeout".into()));
        let json_with = serde_json::to_string(&with_reason).unwrap();
        assert!(json_with.contains("\"reason\":\"idle_timeout\""));

        let no_reason = build_hangup("c1", "p1", None);
        let json_without = serde_json::to_string(&no_reason).unwrap();
        assert!(
            !json_without.contains("reason"),
            "expected reason absent, got: {json_without}"
        );
    }

    #[test]
    fn select_answer_carries_selected_party_id() {
        let s = build_select_answer("c1", "p1", "remote-party-42");
        let json = serde_json::to_string(&s).unwrap();
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["call_id"], "c1");
        assert_eq!(v["party_id"], "p1");
        assert_eq!(v["version"], "1");
        assert_eq!(v["selected_party_id"], "remote-party-42");
    }

    #[test]
    fn sealed_key_roundtrips_through_invite_base64() {
        // Generate → build invite → serialize → deserialize → reconstruct
        // P2PCrypto from the base64 session key → roundtrip a payload.
        let (alice, sealed) = P2PCrypto::generate();
        let alice_frame = alice.encrypt(b"hello").unwrap();

        let invite = build_invite("c1", "p1", "sdp", sealed, 30_000, None);
        let json = serde_json::to_string(&invite).unwrap();
        let received: CallInvite = serde_json::from_str(&json).unwrap();

        let b64 = received
            .mxdx_session_key
            .as_ref()
            .expect("build_invite must populate mxdx_session_key");
        let bob_sealed = SealedKey::from_base64(b64).expect("valid base64");
        let bob = P2PCrypto::from_sealed(bob_sealed);
        let plaintext = bob.decrypt(&alice_frame).unwrap();
        assert_eq!(plaintext, b"hello".to_vec());
    }

    #[test]
    fn invite_debug_redacts_mxdx_session_key() {
        // Important security property: Debug output must not leak the key.
        // This guards against log-scraping of structured logs at any layer
        // that ends up calling {:?} on a CallInvite.
        //
        // Note: the derived `Debug` here will print the base64 string. We
        // do NOT want that. A custom Debug is provided below to redact. The
        // test locks that behavior so a future `#[derive(Debug)]` bitrot is
        // caught.
        let (_crypto, sealed) = P2PCrypto::generate();
        let invite = build_invite("c1", "p1", "sdp", sealed, 30_000, None);
        let dbg = format!("{invite:?}");
        assert!(
            !dbg.contains(invite.mxdx_session_key.as_ref().unwrap().as_str()),
            "Debug output leaked the session key: {dbg}"
        );
        assert!(
            dbg.contains("<redacted>"),
            "Debug output must visibly indicate redaction, got: {dbg}"
        );
    }
}
