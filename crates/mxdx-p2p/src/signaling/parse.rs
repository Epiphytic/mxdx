//! Incoming `m.call.*` event parser.
//!
//! Bridges the Matrix sync inbound path (which hands us decrypted event
//! JSON as a raw string or [`serde_json::Value`]) to the typed variants in
//! [`crate::signaling::events`]. The parser is deliberately tolerant:
//!
//! - Unknown `m.call.*` event types are NOT errors — they surface as
//!   [`ParsedCallEvent::Unknown`] so the state machine (Phase 5) can log
//!   and ignore them without escalating. New call event types added by
//!   future Matrix spec revisions therefore don't break the receive path.
//! - Malformed JSON for a known event type IS an error
//!   ([`ParseError::InvalidContent`]), but the error is returned instead
//!   of panicked — the caller (Phase 5 transport driver) decides whether
//!   to drop the event or fail the call.
//! - The parser never panics on arbitrary bytes.
//!
//! See ADR `docs/adr/2026-04-15-mcall-wire-format.md` (and 2026-04-16
//! addendum) for the event type list and field shapes.

use serde::Deserialize;
use serde_json::Value;

use super::events::{CallAnswer, CallCandidates, CallHangup, CallInvite, CallSelectAnswer};

/// The five recognized Matrix VoIP event types plus an `Unknown` fall-
/// through. Each known variant carries the fully-parsed typed content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedCallEvent {
    Invite(CallInvite),
    Answer(CallAnswer),
    Candidates(CallCandidates),
    Hangup(CallHangup),
    SelectAnswer(CallSelectAnswer),
    /// An event whose `type` starts with `m.call.` but isn't one of the
    /// five we handle. Carries the original event type string so callers
    /// can log it. Future Matrix call event types (e.g. `m.call.reject`,
    /// `m.call.negotiate`) surface here rather than causing parse errors.
    Unknown {
        event_type: String,
    },
}

impl ParsedCallEvent {
    /// Get the Matrix event type string corresponding to this event.
    pub fn event_type(&self) -> &str {
        match self {
            ParsedCallEvent::Invite(_) => "m.call.invite",
            ParsedCallEvent::Answer(_) => "m.call.answer",
            ParsedCallEvent::Candidates(_) => "m.call.candidates",
            ParsedCallEvent::Hangup(_) => "m.call.hangup",
            ParsedCallEvent::SelectAnswer(_) => "m.call.select_answer",
            ParsedCallEvent::Unknown { event_type } => event_type.as_str(),
        }
    }

    /// `call_id` from the parsed content. Returns `None` for `Unknown`
    /// because we don't parse unknown content.
    pub fn call_id(&self) -> Option<&str> {
        match self {
            ParsedCallEvent::Invite(e) => Some(&e.call_id),
            ParsedCallEvent::Answer(e) => Some(&e.call_id),
            ParsedCallEvent::Candidates(e) => Some(&e.call_id),
            ParsedCallEvent::Hangup(e) => Some(&e.call_id),
            ParsedCallEvent::SelectAnswer(e) => Some(&e.call_id),
            ParsedCallEvent::Unknown { .. } => None,
        }
    }
}

/// Errors produced by the parser. Covers the two cases where returning a
/// typed event is impossible: JSON that can't be parsed at all, and JSON
/// whose structure is incompatible with the `CallFoo` struct shape.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("invalid JSON: {0}")]
    InvalidJson(String),
    #[error("event is not an m.call.* type: {event_type}")]
    NotACallEvent { event_type: String },
    #[error("invalid content for {event_type}: {reason}")]
    InvalidContent { event_type: String, reason: String },
}

/// Parse a Matrix event envelope of the form `{"type": "m.call.…", "content": {…}}`
/// from a raw JSON string. This is the typical input when working with
/// `Raw<AnyTimelineEvent>` or a decrypted timeline event — mxdx-matrix's
/// decrypt pipeline hands plaintext JSON strings to callers.
///
/// Returns [`ParseError::NotACallEvent`] if `type` is not an `m.call.*`
/// type, so callers can cheaply filter.
pub fn parse_event(json: &str) -> Result<ParsedCallEvent, ParseError> {
    let value: Value =
        serde_json::from_str(json).map_err(|e| ParseError::InvalidJson(e.to_string()))?;
    parse_value(&value)
}

/// Parse a Matrix event envelope from a [`serde_json::Value`] that already
/// has `{"type", "content"}` shape. Useful when the caller has already
/// deserialized the outer envelope (e.g. mxdx-matrix sync loop).
pub fn parse_value(event: &Value) -> Result<ParsedCallEvent, ParseError> {
    // Accept either the full `{type, content}` envelope (the common case
    // from Matrix sync/room events) or a bare content object alongside a
    // sibling event type — but the canonical input is the envelope.
    let event_type = match event.get("type").and_then(Value::as_str) {
        Some(s) => s,
        None => {
            return Err(ParseError::InvalidContent {
                event_type: "<unknown>".to_string(),
                reason: "event envelope missing `type` field".to_string(),
            });
        }
    };

    if !is_call_event(event_type) {
        return Err(ParseError::NotACallEvent {
            event_type: event_type.to_string(),
        });
    }

    // Content is required for the known types. Unknown m.call.* types are
    // surfaced below before any content validation.
    let content = event.get("content").unwrap_or(&Value::Null);

    parse_content(event_type, content)
}

/// Parse the content value of a known or unknown `m.call.*` event type.
/// Exposed separately so callers that already split type + content can
/// call this directly.
pub fn parse_content(event_type: &str, content: &Value) -> Result<ParsedCallEvent, ParseError> {
    if !is_call_event(event_type) {
        return Err(ParseError::NotACallEvent {
            event_type: event_type.to_string(),
        });
    }

    match event_type {
        "m.call.invite" => {
            deserialize::<CallInvite>(content, event_type).map(ParsedCallEvent::Invite)
        }
        "m.call.answer" => {
            deserialize::<CallAnswer>(content, event_type).map(ParsedCallEvent::Answer)
        }
        "m.call.candidates" => {
            deserialize::<CallCandidates>(content, event_type).map(ParsedCallEvent::Candidates)
        }
        "m.call.hangup" => {
            deserialize::<CallHangup>(content, event_type).map(ParsedCallEvent::Hangup)
        }
        "m.call.select_answer" => {
            deserialize::<CallSelectAnswer>(content, event_type).map(ParsedCallEvent::SelectAnswer)
        }
        other => Ok(ParsedCallEvent::Unknown {
            event_type: other.to_string(),
        }),
    }
}

fn is_call_event(event_type: &str) -> bool {
    event_type.starts_with("m.call.")
}

fn deserialize<'a, T: Deserialize<'a>>(
    content: &'a Value,
    event_type: &str,
) -> Result<T, ParseError> {
    T::deserialize(content).map_err(|e| ParseError::InvalidContent {
        event_type: event_type.to_string(),
        reason: e.to_string(),
    })
}

/// The five recognized call event types, as a constant array. Exposed for
/// use by [`crate::signaling`] callers that need to register sync filters
/// or iterate the recognized type space (e.g. T-43 sync filter wiring).
pub const CALL_EVENT_TYPES: &[&str] = &[
    "m.call.invite",
    "m.call.answer",
    "m.call.candidates",
    "m.call.hangup",
    "m.call.select_answer",
];

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn envelope(event_type: &str, content: Value) -> String {
        json!({ "type": event_type, "content": content }).to_string()
    }

    #[test]
    fn parses_invite_with_session_key() {
        let content = json!({
            "call_id": "c1",
            "party_id": "p1",
            "version": "1",
            "lifetime": 30_000,
            "offer": { "type": "offer", "sdp": "v=0" },
            "mxdx_session_key": "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8="
        });
        let env = envelope("m.call.invite", content);
        match parse_event(&env).expect("parse ok") {
            ParsedCallEvent::Invite(i) => {
                assert_eq!(i.call_id, "c1");
                assert_eq!(i.lifetime, 30_000);
                assert_eq!(i.offer.kind, "offer");
                assert!(i.mxdx_session_key.is_some());
                assert!(i.session_uuid.is_none());
            }
            other => panic!("expected Invite, got {other:?}"),
        }
    }

    #[test]
    fn parses_invite_without_optional_fields() {
        let content = json!({
            "call_id": "c2",
            "party_id": "p2",
            "version": "1",
            "lifetime": 30_000,
            "offer": { "type": "offer", "sdp": "sdp" }
        });
        let env = envelope("m.call.invite", content);
        match parse_event(&env).unwrap() {
            ParsedCallEvent::Invite(i) => {
                assert!(i.mxdx_session_key.is_none());
                assert!(i.session_uuid.is_none());
            }
            other => panic!("expected Invite, got {other:?}"),
        }
    }

    #[test]
    fn parses_answer() {
        let env = envelope(
            "m.call.answer",
            json!({
                "call_id": "c1", "party_id": "p2", "version": "1",
                "answer": { "type": "answer", "sdp": "answer-sdp" }
            }),
        );
        let parsed = parse_event(&env).unwrap();
        assert_eq!(parsed.event_type(), "m.call.answer");
        assert_eq!(parsed.call_id(), Some("c1"));
    }

    #[test]
    fn parses_candidates_node_shape() {
        let env = envelope(
            "m.call.candidates",
            json!({
                "call_id": "c1", "party_id": "p1", "version": "1",
                "candidates": [
                    { "candidate": "cand1", "sdpMid": "0" },
                    { "candidate": "cand2", "sdpMid": "0" }
                ]
            }),
        );
        match parse_event(&env).unwrap() {
            ParsedCallEvent::Candidates(c) => {
                assert_eq!(c.candidates.len(), 2);
                assert_eq!(c.candidates[0].sdp_mid.as_deref(), Some("0"));
                assert!(c.candidates[0].sdp_mline_index.is_none());
            }
            other => panic!("expected Candidates, got {other:?}"),
        }
    }

    #[test]
    fn parses_candidates_browser_shape() {
        let env = envelope(
            "m.call.candidates",
            json!({
                "call_id": "c1", "party_id": "p1", "version": "1",
                "candidates": [
                    { "candidate": "cand1", "sdpMid": "0", "sdpMLineIndex": 0 }
                ]
            }),
        );
        match parse_event(&env).unwrap() {
            ParsedCallEvent::Candidates(c) => {
                assert_eq!(c.candidates[0].sdp_mline_index, Some(0));
            }
            other => panic!("expected Candidates, got {other:?}"),
        }
    }

    #[test]
    fn parses_hangup_with_and_without_reason() {
        let with = envelope(
            "m.call.hangup",
            json!({ "call_id": "c1", "party_id": "p1", "version": "1", "reason": "idle_timeout" }),
        );
        match parse_event(&with).unwrap() {
            ParsedCallEvent::Hangup(h) => assert_eq!(h.reason.as_deref(), Some("idle_timeout")),
            other => panic!("expected Hangup, got {other:?}"),
        }
        let without = envelope(
            "m.call.hangup",
            json!({ "call_id": "c1", "party_id": "p1", "version": "1" }),
        );
        match parse_event(&without).unwrap() {
            ParsedCallEvent::Hangup(h) => assert!(h.reason.is_none()),
            other => panic!("expected Hangup, got {other:?}"),
        }
    }

    #[test]
    fn parses_select_answer() {
        let env = envelope(
            "m.call.select_answer",
            json!({ "call_id": "c1", "party_id": "p1", "version": "1", "selected_party_id": "rp-9" }),
        );
        match parse_event(&env).unwrap() {
            ParsedCallEvent::SelectAnswer(s) => assert_eq!(s.selected_party_id, "rp-9"),
            other => panic!("expected SelectAnswer, got {other:?}"),
        }
    }

    #[test]
    fn unknown_m_call_type_surfaces_as_unknown_not_error() {
        let env = envelope(
            "m.call.reject",
            json!({ "call_id": "c1", "party_id": "p1", "version": "1" }),
        );
        match parse_event(&env).unwrap() {
            ParsedCallEvent::Unknown { event_type } => {
                assert_eq!(event_type, "m.call.reject");
            }
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    #[test]
    fn non_call_event_returns_not_a_call_event_error() {
        let env = envelope(
            "m.room.message",
            json!({ "body": "hi", "msgtype": "m.text" }),
        );
        match parse_event(&env) {
            Err(ParseError::NotACallEvent { event_type }) => {
                assert_eq!(event_type, "m.room.message");
            }
            other => panic!("expected NotACallEvent, got {other:?}"),
        }
    }

    #[test]
    fn malformed_json_returns_invalid_json() {
        match parse_event("{not valid json") {
            Err(ParseError::InvalidJson(_)) => (),
            other => panic!("expected InvalidJson, got {other:?}"),
        }
    }

    #[test]
    fn missing_required_field_returns_invalid_content() {
        // Invite without `call_id` — required.
        let env = envelope(
            "m.call.invite",
            json!({
                "party_id": "p1", "version": "1", "lifetime": 30_000,
                "offer": { "type": "offer", "sdp": "sdp" }
            }),
        );
        match parse_event(&env) {
            Err(ParseError::InvalidContent { event_type, .. }) => {
                assert_eq!(event_type, "m.call.invite");
            }
            other => panic!("expected InvalidContent, got {other:?}"),
        }
    }

    #[test]
    fn missing_type_field_returns_invalid_content() {
        let env = json!({ "content": { "call_id": "c1" } }).to_string();
        match parse_event(&env) {
            Err(ParseError::InvalidContent { .. }) => (),
            other => panic!("expected InvalidContent, got {other:?}"),
        }
    }

    #[test]
    fn parser_does_not_panic_on_arbitrary_binary_like_input() {
        // Fuzzer-style inputs — all should either parse, return an error,
        // or surface as Unknown. None should panic.
        for input in [
            "",
            "\x00",
            "\x7f",
            "null",
            "true",
            "42",
            "[]",
            "{}",
            "{\"type\":null}",
            "{\"type\":\"m.call.invite\"}",
            "{\"type\":\"m.call.invite\",\"content\":null}",
            "{\"type\":\"m.call.candidates\",\"content\":{\"candidates\":\"not-an-array\"}}",
            &"a".repeat(100_000),
        ] {
            // Either Ok or Err — never a panic.
            let _ = parse_event(input);
        }
    }

    #[test]
    fn cross_runtime_fixture_parses() {
        // Roundtrip via the committed cross-runtime fixture: the parser
        // accepts npm-shaped JSON (same as Rust-emitted since T-44). This
        // mirrors what the sync loop in Phase 5 will receive.
        let content_json = r#"{"call_id":"c1","party_id":"p1","version":"1","lifetime":30000,"offer":{"type":"offer","sdp":"v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\n"},"mxdx_session_key":"AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8="}"#;
        let env = format!(r#"{{"type":"m.call.invite","content":{content_json}}}"#);
        let parsed = parse_event(&env).expect("parse ok");
        assert_eq!(parsed.event_type(), "m.call.invite");
        match parsed {
            ParsedCallEvent::Invite(i) => {
                assert_eq!(i.call_id, "c1");
                assert_eq!(i.lifetime, 30_000);
                assert!(i.mxdx_session_key.is_some());
            }
            other => panic!("expected Invite, got {other:?}"),
        }
    }
}
