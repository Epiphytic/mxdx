#![cfg(not(target_arch = "wasm32"))]
//! Golden tests for the `signaling::parse` module, integration-test flavor.
//!
//! These exercise the public parse entry points end-to-end against a mix of
//! hand-crafted Matrix event envelopes (the typical shape produced by the
//! decrypt pipeline in `mxdx-matrix`) and the committed cross-runtime
//! fixture used by `signaling_vectors.rs` (T-40). The in-module unit tests
//! in `src/signaling/parse.rs` cover the error-path + unknown-variant
//! contract; this file is the positive-path integration goldens that Phase
//! 5 (state machine) can extend as it wires new branches.

use std::path::PathBuf;

use mxdx_p2p::signaling::parse::{parse_event, ParsedCallEvent};
use serde_json::Value;

fn fixture_value() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("signaling-vectors.json");
    let data = std::fs::read_to_string(&path).expect("read fixture");
    serde_json::from_str(&data).expect("fixture is JSON")
}

fn envelope(event_type: &str, content_json: &str) -> String {
    // `content_json` is already the committed fixture's inner JSON.
    // Wrap it in a Matrix event envelope as the decrypt pipeline does.
    format!(r#"{{"type":"{event_type}","content":{content_json}}}"#)
}

#[test]
fn golden_invite_with_session_key() {
    let fixture = fixture_value();
    let env = envelope(
        "m.call.invite",
        fixture["invite_with_session_key"]["json"].as_str().unwrap(),
    );
    match parse_event(&env).unwrap() {
        ParsedCallEvent::Invite(i) => {
            assert_eq!(i.call_id, "c1");
            assert_eq!(i.party_id, "p1");
            assert_eq!(i.version, "1");
            assert_eq!(i.lifetime, 30_000);
            assert_eq!(i.offer.kind, "offer");
            assert_eq!(
                i.mxdx_session_key.as_deref(),
                Some("AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=")
            );
            assert!(i.session_uuid.is_none());
        }
        other => panic!("expected Invite, got {other:?}"),
    }
}

#[test]
fn golden_invite_without_session_key() {
    let fixture = fixture_value();
    let env = envelope(
        "m.call.invite",
        fixture["invite_without_session_key"]["json"]
            .as_str()
            .unwrap(),
    );
    match parse_event(&env).unwrap() {
        ParsedCallEvent::Invite(i) => {
            assert!(i.mxdx_session_key.is_none());
        }
        other => panic!("expected Invite, got {other:?}"),
    }
}

#[test]
fn golden_invite_with_session_uuid() {
    let fixture = fixture_value();
    let env = envelope(
        "m.call.invite",
        fixture["invite_with_session_uuid"]["json"]
            .as_str()
            .unwrap(),
    );
    match parse_event(&env).unwrap() {
        ParsedCallEvent::Invite(i) => {
            assert_eq!(i.session_uuid.as_deref(), Some("sess-abc-123"));
        }
        other => panic!("expected Invite, got {other:?}"),
    }
}

#[test]
fn golden_answer() {
    let fixture = fixture_value();
    let env = envelope("m.call.answer", fixture["answer"]["json"].as_str().unwrap());
    match parse_event(&env).unwrap() {
        ParsedCallEvent::Answer(a) => {
            assert_eq!(a.answer.kind, "answer");
            assert_eq!(a.answer.sdp, "v=0\r\na=answer\r\n");
        }
        other => panic!("expected Answer, got {other:?}"),
    }
}

#[test]
fn golden_candidates_node_shape() {
    let fixture = fixture_value();
    let env = envelope(
        "m.call.candidates",
        fixture["candidates_node_shape"]["json"].as_str().unwrap(),
    );
    match parse_event(&env).unwrap() {
        ParsedCallEvent::Candidates(c) => {
            assert_eq!(c.candidates.len(), 2);
            assert!(c.candidates[0].sdp_mline_index.is_none());
        }
        other => panic!("expected Candidates, got {other:?}"),
    }
}

#[test]
fn golden_candidates_browser_shape() {
    let fixture = fixture_value();
    let env = envelope(
        "m.call.candidates",
        fixture["candidates_browser_shape"]["json"]
            .as_str()
            .unwrap(),
    );
    match parse_event(&env).unwrap() {
        ParsedCallEvent::Candidates(c) => {
            assert_eq!(c.candidates[0].sdp_mline_index, Some(0));
        }
        other => panic!("expected Candidates, got {other:?}"),
    }
}

#[test]
fn golden_hangup_variants() {
    let fixture = fixture_value();
    let with_reason_env = envelope(
        "m.call.hangup",
        fixture["hangup_with_reason"]["json"].as_str().unwrap(),
    );
    let without_reason_env = envelope(
        "m.call.hangup",
        fixture["hangup_without_reason"]["json"].as_str().unwrap(),
    );

    match parse_event(&with_reason_env).unwrap() {
        ParsedCallEvent::Hangup(h) => assert_eq!(h.reason.as_deref(), Some("idle_timeout")),
        other => panic!("expected Hangup, got {other:?}"),
    }
    match parse_event(&without_reason_env).unwrap() {
        ParsedCallEvent::Hangup(h) => assert!(h.reason.is_none()),
        other => panic!("expected Hangup, got {other:?}"),
    }
}

#[test]
fn golden_select_answer() {
    let fixture = fixture_value();
    let env = envelope(
        "m.call.select_answer",
        fixture["select_answer"]["json"].as_str().unwrap(),
    );
    match parse_event(&env).unwrap() {
        ParsedCallEvent::SelectAnswer(s) => {
            assert_eq!(s.selected_party_id, "remote-party-7");
        }
        other => panic!("expected SelectAnswer, got {other:?}"),
    }
}
