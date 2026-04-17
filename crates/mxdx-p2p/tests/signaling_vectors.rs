#![cfg(not(target_arch = "wasm32"))]
//! Cross-language signaling vectors for the Matrix VoIP `m.call.*` events.
//!
//! These vectors lock the wire format between the Rust emitter in
//! `mxdx_p2p::signaling::events` and the npm emitter/parser in
//! `packages/core/p2p-signaling.js`.
//!
//! Two passes per fixture:
//!
//! 1. **Byte-exact emit**: for the invite/answer/candidates/hangup/
//!    select_answer vectors, building the struct with the documented inputs
//!    and then `serde_json::to_string` produces the committed fixture
//!    bytes exactly. Guarantees serde struct-field declaration order is
//!    stable and matches npm.
//! 2. **Roundtrip parse**: the committed fixture bytes parse into the
//!    expected struct fields. Guarantees the Rust parser accepts valid
//!    npm-emitted JSON (both node-shape candidates without sdpMLineIndex
//!    and browser-shape candidates with sdpMLineIndex).
//!
//! The complementary Node sidecar test lives at
//! `packages/e2e-tests/tests/rust-npm-signaling-vectors.test.js` and runs
//! the same JSON bytes through `P2PSignaling` and a homemade JSON parse to
//! validate the other direction: npm can read Rust-emitted JSON, and JSON
//! emitted by npm's `JSON.stringify(content)` for the same inputs matches
//! the committed fixture.
//!
//! Regenerate with:
//!
//!     cargo test -p mxdx-p2p --test signaling_vectors -- \
//!         --ignored generate_vectors --exact --nocapture

use std::path::PathBuf;

use mxdx_p2p::signaling::events::{
    build_answer, build_candidates, build_hangup, build_select_answer, CallAnswer, CallCandidates,
    CallHangup, CallIceCandidate, CallInvite, CallSdpField, CallSelectAnswer,
};
use serde_json::Value;

const FIXTURE_VERSION: u32 = 1;

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("signaling-vectors.json")
}

fn load_fixture() -> Value {
    let path = fixture_path();
    let data = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read fixture {}: {e}", path.display()));
    let v: Value = serde_json::from_str(&data).expect("valid JSON fixture");
    assert_eq!(
        v["version"].as_u64().unwrap() as u32,
        FIXTURE_VERSION,
        "fixture version mismatch"
    );
    v
}

/// Deterministic 32-byte key for fixture stability. Never used in
/// production — constructed by hand via the public
/// [`CallInvite.mxdx_session_key`] field on the struct (no sealed-key
/// involvement in the fixture test, which is deliberate: the fixture
/// captures the *wire bytes*, not the crypto path).
const FIXTURE_KEY_B64: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";

fn make_fixture_invite_with_key() -> CallInvite {
    CallInvite {
        call_id: "c1".into(),
        party_id: "p1".into(),
        version: "1".into(),
        lifetime: 30_000,
        offer: CallSdpField {
            kind: "offer".into(),
            sdp: "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\n".into(),
        },
        mxdx_session_key: Some(FIXTURE_KEY_B64.into()),
        session_uuid: None,
    }
}

fn make_fixture_invite_without_key() -> CallInvite {
    CallInvite {
        call_id: "c2".into(),
        party_id: "p2".into(),
        version: "1".into(),
        lifetime: 30_000,
        offer: CallSdpField {
            kind: "offer".into(),
            sdp: "sdp-placeholder".into(),
        },
        mxdx_session_key: None,
        session_uuid: None,
    }
}

fn make_fixture_invite_with_session_uuid() -> CallInvite {
    CallInvite {
        call_id: "c3".into(),
        party_id: "p3".into(),
        version: "1".into(),
        lifetime: 30_000,
        offer: CallSdpField {
            kind: "offer".into(),
            sdp: "sdp-placeholder".into(),
        },
        mxdx_session_key: Some(FIXTURE_KEY_B64.into()),
        session_uuid: Some("sess-abc-123".into()),
    }
}

#[test]
fn emit_byte_exact_invite_with_key() {
    let fixture = load_fixture();
    let expected = fixture["invite_with_session_key"]["json"].as_str().unwrap();
    let got = serde_json::to_string(&make_fixture_invite_with_key()).unwrap();
    assert_eq!(
        got, expected,
        "Rust emitter diverged from committed fixture (npm-compat wire)"
    );
}

#[test]
fn emit_byte_exact_invite_without_key() {
    let fixture = load_fixture();
    let expected = fixture["invite_without_session_key"]["json"]
        .as_str()
        .unwrap();
    let got = serde_json::to_string(&make_fixture_invite_without_key()).unwrap();
    assert_eq!(got, expected);
}

#[test]
fn emit_byte_exact_invite_with_session_uuid() {
    let fixture = load_fixture();
    let expected = fixture["invite_with_session_uuid"]["json"]
        .as_str()
        .unwrap();
    let got = serde_json::to_string(&make_fixture_invite_with_session_uuid()).unwrap();
    assert_eq!(got, expected);
}

#[test]
fn emit_byte_exact_answer() {
    let fixture = load_fixture();
    let expected = fixture["answer"]["json"].as_str().unwrap();
    let answer = CallAnswer {
        call_id: "c1".into(),
        party_id: "p2".into(),
        version: "1".into(),
        answer: CallSdpField {
            kind: "answer".into(),
            sdp: "v=0\r\na=answer\r\n".into(),
        },
    };
    let got = serde_json::to_string(&answer).unwrap();
    assert_eq!(got, expected);

    // Builder produces the same output.
    let via_builder = build_answer("c1", "p2", "v=0\r\na=answer\r\n");
    assert_eq!(serde_json::to_string(&via_builder).unwrap(), expected);
}

#[test]
fn emit_byte_exact_candidates_node_shape() {
    let fixture = load_fixture();
    let expected = fixture["candidates_node_shape"]["json"].as_str().unwrap();
    let c = build_candidates(
        "c1",
        "p1",
        vec![
            CallIceCandidate {
                candidate: "candidate:1 1 UDP 2130706431 192.168.1.100 12345 typ host".into(),
                sdp_mid: Some("0".into()),
                sdp_mline_index: None,
            },
            CallIceCandidate {
                candidate: "candidate:2 1 UDP 1694498815 203.0.113.1 54321 typ srflx".into(),
                sdp_mid: Some("0".into()),
                sdp_mline_index: None,
            },
        ],
    );
    let got = serde_json::to_string(&c).unwrap();
    assert_eq!(got, expected);
}

#[test]
fn emit_byte_exact_candidates_browser_shape() {
    let fixture = load_fixture();
    let expected = fixture["candidates_browser_shape"]["json"]
        .as_str()
        .unwrap();
    let c = build_candidates(
        "c1",
        "p1",
        vec![CallIceCandidate {
            candidate: "candidate:3 1 UDP 1 1.2.3.4 9999 typ relay".into(),
            sdp_mid: Some("0".into()),
            sdp_mline_index: Some(0),
        }],
    );
    let got = serde_json::to_string(&c).unwrap();
    assert_eq!(got, expected);
}

#[test]
fn emit_byte_exact_hangup_with_reason() {
    let fixture = load_fixture();
    let expected = fixture["hangup_with_reason"]["json"].as_str().unwrap();
    let h = build_hangup("c1", "p1", Some("idle_timeout".into()));
    let got = serde_json::to_string(&h).unwrap();
    assert_eq!(got, expected);
}

#[test]
fn emit_byte_exact_hangup_without_reason() {
    let fixture = load_fixture();
    let expected = fixture["hangup_without_reason"]["json"].as_str().unwrap();
    let h = build_hangup("c1", "p1", None);
    let got = serde_json::to_string(&h).unwrap();
    assert_eq!(got, expected);
}

#[test]
fn emit_byte_exact_select_answer() {
    let fixture = load_fixture();
    let expected = fixture["select_answer"]["json"].as_str().unwrap();
    let s = build_select_answer("c1", "p1", "remote-party-7");
    let got = serde_json::to_string(&s).unwrap();
    assert_eq!(got, expected);
}

#[test]
fn parse_all_committed_fixtures_roundtrip() {
    // Parse every fixture JSON into its Rust struct and back; the re-emitted
    // bytes must equal the input. Guarantees the Rust parser accepts
    // npm-shaped input and doesn't drop optional fields.
    let fixture = load_fixture();

    for (name, ctor) in [
        ("invite_with_session_key", "invite"),
        ("invite_without_session_key", "invite"),
        ("invite_with_session_uuid", "invite"),
        ("answer", "answer"),
        ("candidates_node_shape", "candidates"),
        ("candidates_browser_shape", "candidates"),
        ("hangup_with_reason", "hangup"),
        ("hangup_without_reason", "hangup"),
        ("select_answer", "select_answer"),
    ] {
        let json = fixture[name]["json"].as_str().unwrap();
        let reemitted = match ctor {
            "invite" => {
                let v: CallInvite =
                    serde_json::from_str(json).unwrap_or_else(|e| panic!("parse {name}: {e}"));
                serde_json::to_string(&v).unwrap()
            }
            "answer" => {
                let v: CallAnswer = serde_json::from_str(json).unwrap();
                serde_json::to_string(&v).unwrap()
            }
            "candidates" => {
                let v: CallCandidates = serde_json::from_str(json).unwrap();
                serde_json::to_string(&v).unwrap()
            }
            "hangup" => {
                let v: CallHangup = serde_json::from_str(json).unwrap();
                serde_json::to_string(&v).unwrap()
            }
            "select_answer" => {
                let v: CallSelectAnswer = serde_json::from_str(json).unwrap();
                serde_json::to_string(&v).unwrap()
            }
            _ => unreachable!(),
        };
        assert_eq!(
            reemitted, json,
            "roundtrip bytes mismatch for {name}:\n  got:      {reemitted}\n  expected: {json}"
        );
    }
}

/// Regenerate the committed fixture by re-emitting every struct to JSON and
/// writing back. Ignored by default — only run when intentionally updating
/// wire format. Must be followed by a re-run of the Node sidecar test.
#[test]
#[ignore = "regenerates the committed fixture; run explicitly when updating vectors"]
fn generate_vectors() {
    use serde_json::json;

    let fixture = json!({
        "version": FIXTURE_VERSION,
        "description": "Cross-language m.call.* signaling vectors. Generated with the Rust emitter (see tests/signaling_vectors.rs) and asserted byte-for-byte against the npm parser in packages/e2e-tests/tests/rust-npm-signaling-vectors.test.js. Do not edit by hand — regenerate via: cargo test -p mxdx-p2p --test signaling_vectors -- --ignored generate_vectors --exact --nocapture",
        "invite_with_session_key": {
            "json": serde_json::to_string(&make_fixture_invite_with_key()).unwrap(),
            "expected_fields": {
                "call_id": "c1",
                "party_id": "p1",
                "version": "1",
                "lifetime": 30_000,
                "offer_type": "offer",
                "offer_sdp": "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\n",
                "mxdx_session_key": FIXTURE_KEY_B64,
                "session_uuid_absent": true,
            }
        },
        "invite_without_session_key": {
            "json": serde_json::to_string(&make_fixture_invite_without_key()).unwrap(),
            "expected_fields": {
                "call_id": "c2",
                "party_id": "p2",
                "version": "1",
                "lifetime": 30_000,
                "offer_type": "offer",
                "offer_sdp": "sdp-placeholder",
                "mxdx_session_key_absent": true,
                "session_uuid_absent": true,
            }
        },
        "invite_with_session_uuid": {
            "json": serde_json::to_string(&make_fixture_invite_with_session_uuid()).unwrap(),
            "expected_fields": {
                "call_id": "c3",
                "session_uuid": "sess-abc-123",
            }
        },
        "answer": {
            "json": serde_json::to_string(&build_answer("c1", "p2", "v=0\r\na=answer\r\n")).unwrap(),
            "expected_fields": {
                "call_id": "c1", "party_id": "p2", "version": "1",
                "answer_type": "answer", "answer_sdp": "v=0\r\na=answer\r\n",
            }
        },
        "candidates_node_shape": {
            "json": serde_json::to_string(&build_candidates("c1", "p1", vec![
                CallIceCandidate {
                    candidate: "candidate:1 1 UDP 2130706431 192.168.1.100 12345 typ host".into(),
                    sdp_mid: Some("0".into()),
                    sdp_mline_index: None,
                },
                CallIceCandidate {
                    candidate: "candidate:2 1 UDP 1694498815 203.0.113.1 54321 typ srflx".into(),
                    sdp_mid: Some("0".into()),
                    sdp_mline_index: None,
                },
            ])).unwrap(),
            "expected_fields": {
                "call_id": "c1", "count": 2,
                "candidate_0_sdpMid": "0",
                "candidate_0_sdpMLineIndex_absent": true,
            }
        },
        "candidates_browser_shape": {
            "json": serde_json::to_string(&build_candidates("c1", "p1", vec![
                CallIceCandidate {
                    candidate: "candidate:3 1 UDP 1 1.2.3.4 9999 typ relay".into(),
                    sdp_mid: Some("0".into()),
                    sdp_mline_index: Some(0),
                },
            ])).unwrap(),
            "expected_fields": {
                "call_id": "c1", "count": 1,
                "candidate_0_sdpMid": "0",
                "candidate_0_sdpMLineIndex": 0,
            }
        },
        "hangup_with_reason": {
            "json": serde_json::to_string(&build_hangup("c1", "p1", Some("idle_timeout".into()))).unwrap(),
            "expected_fields": { "call_id": "c1", "reason": "idle_timeout" }
        },
        "hangup_without_reason": {
            "json": serde_json::to_string(&build_hangup("c1", "p1", None)).unwrap(),
            "expected_fields": { "call_id": "c1", "reason_absent": true }
        },
        "select_answer": {
            "json": serde_json::to_string(&build_select_answer("c1", "p1", "remote-party-7")).unwrap(),
            "expected_fields": { "call_id": "c1", "selected_party_id": "remote-party-7" }
        }
    });

    let path = fixture_path();
    let json = serde_json::to_string_pretty(&fixture).expect("serialize fixture");
    std::fs::write(&path, format!("{}\n", json)).expect("write fixture");
    eprintln!("wrote fixture: {}", path.display());
}
