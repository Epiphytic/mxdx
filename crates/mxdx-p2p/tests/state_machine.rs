//! T-54: comprehensive state machine test suite.
//!
//! Covers storm §3.1–§4.5 invariants at the `transition()` + driver-flow
//! level. Unit tests inside `crates/mxdx-p2p/src/transport/state.rs`
//! cover the 35-row transition table; this file adds the end-to-end
//! integration-level assertions that exercise the pure state machine
//! across representative call flows.
//!
//! Every test here runs fully in-process (no homeserver, no WebRTC, no
//! network). Timing-sensitive tests use `tokio::time::pause` +
//! `advance` for virtual-clock determinism.

use std::time::{Duration, Instant};

use mxdx_p2p::channel::{ChannelEvent, IceServer, Sdp, SdpKind};
use mxdx_p2p::signaling::events::{
    CallAnswer, CallHangup, CallInvite, CallSdpField, CallSelectAnswer, CALL_VERSION,
};
use mxdx_p2p::signaling::parse::ParsedCallEvent;
use mxdx_p2p::transport::state::{
    transition, Command, Event, P2PState, SecurityEventKind, TelemetryKind, TransitionResult,
    VerifyFailureReason,
};

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

fn mk_offer_sdp() -> Sdp {
    Sdp {
        kind: SdpKind::Offer,
        sdp:
            "v=0\r\nm=application 9 DTLS/SCTP webrtc-datachannel\r\na=fingerprint:sha-256 AA:BB\r\n"
                .into(),
    }
}

fn start_event() -> Event {
    Event::Start {
        peer_user_id: "@peer:ex".into(),
        peer_device_id: None,
        our_user_id: "@us:ex".into(),
        our_device_id: "OURDEV".into(),
        room_id: "!room:ex".into(),
        session_uuid: Some("sid".into()),
    }
}

fn answer_event(call_id: &str) -> Event {
    Event::CallEventReceived(ParsedCallEvent::Answer(CallAnswer {
        call_id: call_id.into(),
        party_id: "them".into(),
        version: CALL_VERSION.into(),
        answer: CallSdpField {
            kind: "answer".into(),
            sdp: "v=0\r\n".into(),
        },
    }))
}

fn invite_event(call_id: &str) -> Event {
    Event::CallEventReceived(ParsedCallEvent::Invite(CallInvite {
        call_id: call_id.into(),
        party_id: "them".into(),
        version: CALL_VERSION.into(),
        lifetime: 30_000,
        offer: CallSdpField {
            kind: "offer".into(),
            sdp: "v=0\r\n".into(),
        },
        mxdx_session_key: None,
        session_uuid: None,
    }))
}

fn hangup_event(call_id: &str) -> Event {
    Event::CallEventReceived(ParsedCallEvent::Hangup(CallHangup {
        call_id: call_id.into(),
        party_id: "them".into(),
        version: CALL_VERSION.into(),
        reason: Some("peer".into()),
    }))
}

/// Apply a series of events, returning the final state. Panics on
/// Illegal so test authors can assert on the sequence being valid.
fn run_sequence(initial: P2PState, events: impl IntoIterator<Item = Event>) -> P2PState {
    let mut state = initial;
    for event in events {
        match transition(&state, event) {
            TransitionResult::Ok { next, .. } => state = next,
            TransitionResult::Illegal { note } => {
                panic!("illegal transition from {}: {note}", state.name())
            }
        }
    }
    state
}

/// Drive a full offerer-side happy-path flow up to `Open`. Returns the
/// final state so callers can assert on `Open` fields.
fn drive_full_offerer_path() -> P2PState {
    let mut s = P2PState::Idle;

    // Start → FetchingTurn
    s = match transition(&s, start_event()) {
        TransitionResult::Ok { next, .. } => next,
        _ => panic!(),
    };
    assert_eq!(s.name(), "FetchingTurn");

    // TurnRefreshed → Inviting
    s = match transition(
        &s,
        Event::TurnRefreshed {
            servers: vec![IceServer {
                urls: vec!["stun:stun.ex:3478".into()],
                username: None,
                credential: None,
            }],
        },
    ) {
        TransitionResult::Ok { next, .. } => next,
        _ => panic!(),
    };
    assert_eq!(s.name(), "Inviting");

    // Inviting → Answer → Connecting
    // Note: the state machine's Inviting captures call_id = "<pending>" from
    // the pure transition (the driver is responsible for patching real
    // call_id after create_offer completes). For this test we patch the
    // Inviting state to have a specific call_id.
    if let P2PState::Inviting {
        started,
        our_offer,
        our_party_id,
        lifetime_ms,
        ..
    } = s.clone()
    {
        s = P2PState::Inviting {
            call_id: "c1".into(),
            started,
            our_offer,
            our_party_id,
            lifetime_ms,
        };
    }

    // Answer received → Connecting
    s = match transition(&s, answer_event("c1")) {
        TransitionResult::Ok { next, .. } => next,
        _ => panic!(),
    };
    assert_eq!(s.name(), "Connecting");

    // ChannelEvent::Open → Verifying
    s = match transition(&s, Event::ChannelEvent(ChannelEvent::Open)) {
        TransitionResult::Ok { next, .. } => next,
        _ => panic!(),
    };
    assert_eq!(s.name(), "Verifying");

    // VerifyOk → Open
    s = match transition(&s, Event::VerifyOk) {
        TransitionResult::Ok { next, .. } => next,
        _ => panic!(),
    };
    assert_eq!(s.name(), "Open");

    s
}

// ---------------------------------------------------------------------------
// Tests — happy path coverage
// ---------------------------------------------------------------------------

#[test]
fn happy_path_offerer_reaches_open() {
    let s = drive_full_offerer_path();
    assert!(matches!(s, P2PState::Open { .. }));
}

#[test]
fn happy_path_answerer_reaches_open() {
    let mut s = P2PState::Idle;

    // Peer invite → Answering
    s = match transition(&s, invite_event("peer-call")) {
        TransitionResult::Ok { next, .. } => next,
        _ => panic!(),
    };
    assert_eq!(s.name(), "Answering");

    // Channel open → Verifying
    s = match transition(&s, Event::ChannelEvent(ChannelEvent::Open)) {
        TransitionResult::Ok { next, .. } => next,
        _ => panic!(),
    };
    assert_eq!(s.name(), "Verifying");

    // VerifyOk → Open
    s = match transition(&s, Event::VerifyOk) {
        TransitionResult::Ok { next, .. } => next,
        _ => panic!(),
    };
    assert_eq!(s.name(), "Open");
}

#[test]
fn happy_path_emits_state_transition_telemetry() {
    // Verify every transition emits a `p2p.state_transition` telemetry
    // command so operators can trace the lifecycle.
    let transitions = [
        (P2PState::Idle, start_event(), "start"),
        (
            P2PState::FetchingTurn {
                since: Instant::now(),
            },
            Event::TurnRefreshed {
                servers: Vec::new(),
            },
            "turn_ready",
        ),
    ];
    for (state, event, expected_reason_substr) in transitions {
        let result = transition(&state, event);
        if let TransitionResult::Ok { commands, .. } = result {
            let found = commands.iter().any(|c| {
                matches!(
                    c,
                    Command::EmitTelemetry(TelemetryKind::StateTransition { reason, .. })
                        if reason.contains(expected_reason_substr)
                )
            });
            assert!(
                found,
                "expected state_transition telemetry with reason containing `{expected_reason_substr}`, commands={commands:?}",
            );
        } else {
            panic!("transition was Illegal");
        }
    }
}

// ---------------------------------------------------------------------------
// Tests — failure paths
// ---------------------------------------------------------------------------

#[test]
fn verify_fail_signature_mismatch_prevents_open_and_emits_security_event() {
    let s = P2PState::Verifying {
        call_id: "c1".into(),
        our_party_id: "us".into(),
        our_nonce: [0u8; 32],
    };
    let result = transition(
        &s,
        Event::VerifyFail {
            reason: VerifyFailureReason::SignatureMismatch,
        },
    );
    match result {
        TransitionResult::Ok { next, commands } => {
            // Must NOT reach Open.
            assert_ne!(next.name(), "Open");
            assert_eq!(next.name(), "Failed");
            // SecurityEvent VerifyFailure must be present.
            let sec = commands.iter().any(|c| {
                matches!(
                    c,
                    Command::EmitTelemetry(TelemetryKind::SecurityEvent {
                        kind: SecurityEventKind::VerifyFailure,
                        ..
                    })
                )
            });
            assert!(sec, "expected SecurityEvent{{VerifyFailure}}");
            // Must send hangup + tear down channel.
            let hangup = commands
                .iter()
                .any(|c| matches!(c, Command::SendHangup { .. }));
            let tear = commands
                .iter()
                .any(|c| matches!(c, Command::TearDownChannel { .. }));
            assert!(hangup && tear, "expected hangup + teardown");
        }
        TransitionResult::Illegal { note } => panic!("unexpected illegal: {note}"),
    }
}

#[test]
fn verify_fail_replay_detected_emits_replay_security_event() {
    let s = P2PState::Verifying {
        call_id: "c1".into(),
        our_party_id: "us".into(),
        our_nonce: [0u8; 32],
    };
    if let TransitionResult::Ok { commands, .. } = transition(
        &s,
        Event::VerifyFail {
            reason: VerifyFailureReason::ReplayDetected,
        },
    ) {
        assert!(commands.iter().any(|c| matches!(
            c,
            Command::EmitTelemetry(TelemetryKind::SecurityEvent {
                kind: SecurityEventKind::ReplayDetected,
                ..
            })
        )));
    } else {
        panic!();
    }
}

#[test]
fn verify_fail_device_lockout_emits_wrong_peer_security_event() {
    let s = P2PState::Verifying {
        call_id: "c1".into(),
        our_party_id: "us".into(),
        our_nonce: [0u8; 32],
    };
    if let TransitionResult::Ok { commands, .. } = transition(
        &s,
        Event::VerifyFail {
            reason: VerifyFailureReason::DeviceLockedOut,
        },
    ) {
        assert!(commands.iter().any(|c| matches!(
            c,
            Command::EmitTelemetry(TelemetryKind::SecurityEvent {
                kind: SecurityEventKind::WrongPeer,
                ..
            })
        )));
    } else {
        panic!();
    }
}

#[test]
fn verify_fail_never_reaches_open() {
    // Property: from Verifying, no VerifyFail reason can produce Open.
    let reasons = [
        VerifyFailureReason::SignatureMismatch,
        VerifyFailureReason::ReplayDetected,
        VerifyFailureReason::Timeout,
        VerifyFailureReason::InvalidPayload,
        VerifyFailureReason::DeviceLockedOut,
    ];
    for reason in reasons {
        let s = P2PState::Verifying {
            call_id: "c1".into(),
            our_party_id: "us".into(),
            our_nonce: [0u8; 32],
        };
        let result = transition(&s, Event::VerifyFail { reason });
        let final_state_name = match result {
            TransitionResult::Ok { next, .. } => next.name(),
            TransitionResult::Illegal { .. } => s.name(),
        };
        assert_ne!(
            final_state_name, "Open",
            "VerifyFail({reason:?}) must never reach Open"
        );
    }
}

#[test]
fn invite_timeout_from_inviting_goes_to_failed() {
    let s = P2PState::Inviting {
        call_id: "c1".into(),
        started: Instant::now(),
        our_offer: mk_offer_sdp(),
        our_party_id: "us".into(),
        lifetime_ms: 30_000,
    };
    let final_state = run_sequence(s, [Event::InviteTimeout]);
    assert_eq!(final_state.name(), "Failed");
}

#[test]
fn ice_timeout_from_connecting_goes_to_failed() {
    let s = P2PState::Connecting {
        call_id: "c1".into(),
        our_party_id: "us".into(),
        ice_started: Instant::now(),
    };
    let final_state = run_sequence(s, [Event::IceTimeout]);
    assert_eq!(final_state.name(), "Failed");
}

#[test]
fn turn_expired_from_open_goes_to_failed() {
    let s = P2PState::Open {
        call_id: "c1".into(),
        last_io: Instant::now(),
    };
    let final_state = run_sequence(s, [Event::TurnExpired]);
    assert_eq!(final_state.name(), "Failed");
}

#[test]
fn decrypt_storm_from_open_emits_security_event_and_goes_to_failed() {
    let s = P2PState::Open {
        call_id: "c1".into(),
        last_io: Instant::now(),
    };
    if let TransitionResult::Ok { next, commands } = transition(&s, Event::DecryptStorm) {
        assert_eq!(next.name(), "Failed");
        assert!(commands.iter().any(|c| matches!(
            c,
            Command::EmitTelemetry(TelemetryKind::SecurityEvent {
                kind: SecurityEventKind::DecryptStorm,
                ..
            })
        )));
    } else {
        panic!("expected Ok");
    }
}

#[test]
fn peer_hangup_from_open_goes_to_idle() {
    let s = P2PState::Open {
        call_id: "c1".into(),
        last_io: Instant::now(),
    };
    let final_state = run_sequence(s, [hangup_event("c1")]);
    assert_eq!(final_state.name(), "Idle");
}

#[test]
fn local_hangup_from_verifying_goes_to_idle_and_emits_hangup() {
    let s = P2PState::Verifying {
        call_id: "c1".into(),
        our_party_id: "us".into(),
        our_nonce: [0u8; 32],
    };
    if let TransitionResult::Ok { next, commands } = transition(
        &s,
        Event::Hangup {
            reason: "user".into(),
        },
    ) {
        assert_eq!(next.name(), "Idle");
        assert!(commands
            .iter()
            .any(|c| matches!(c, Command::SendHangup { .. })));
        assert!(commands
            .iter()
            .any(|c| matches!(c, Command::TearDownChannel { .. })));
    } else {
        panic!();
    }
}

// ---------------------------------------------------------------------------
// Tests — TURN expiry during reconnect serialization
// ---------------------------------------------------------------------------

#[test]
fn turn_refresh_while_open_emits_configure_ice_servers() {
    // Storm §3.4: TURN refresh on Open should emit
    // ConfigureIceServers (driver then tries restart_ice; on
    // RestartIceUnsupported per Phase-3 marker, driver tears down
    // + re-invites).
    let s = P2PState::Open {
        call_id: "c1".into(),
        last_io: Instant::now(),
    };
    let servers = vec![IceServer {
        urls: vec!["turn:new.example:3478".into()],
        username: Some("u".into()),
        credential: Some("p".into()),
    }];
    if let TransitionResult::Ok { next, commands } = transition(
        &s,
        Event::TurnRefreshed {
            servers: servers.clone(),
        },
    ) {
        assert_eq!(next.name(), "Open", "Open is preserved during TURN refresh");
        assert!(commands
            .iter()
            .any(|c| matches!(c, Command::ConfigureIceServers { .. })));
    } else {
        panic!();
    }
}

#[test]
fn turn_expired_during_fetching_turn_goes_to_failed() {
    let s = P2PState::FetchingTurn {
        since: Instant::now(),
    };
    let final_state = run_sequence(s, [Event::TurnExpired]);
    assert_eq!(final_state.name(), "Failed");
}

#[test]
fn fetching_turn_does_not_accept_verify_events() {
    // Out-of-order events during TURN fetch should be Illegal, NOT
    // panic, NOT reach Open.
    let s = P2PState::FetchingTurn {
        since: Instant::now(),
    };
    let result = transition(&s, Event::VerifyOk);
    match result {
        TransitionResult::Illegal { .. } => {}
        TransitionResult::Ok { next, .. } => assert_eq!(next.name(), "FetchingTurn"),
    }
}

// ---------------------------------------------------------------------------
// Tests — recovery and retry
// ---------------------------------------------------------------------------

#[test]
fn failed_retry_ready_after_deadline_goes_to_idle() {
    let s = P2PState::Failed {
        reason: "test".into(),
        retry_after: Instant::now() - Duration::from_secs(1),
    };
    let final_state = run_sequence(s, [Event::RetryReady]);
    assert_eq!(final_state.name(), "Idle");
}

#[test]
fn failed_retry_ready_before_deadline_stays_failed() {
    let s = P2PState::Failed {
        reason: "test".into(),
        retry_after: Instant::now() + Duration::from_secs(30),
    };
    let result = transition(&s, Event::RetryReady);
    match result {
        TransitionResult::Ok { next, .. } => assert_eq!(next.name(), "Failed"),
        TransitionResult::Illegal { .. } => {}
    }
}

#[test]
fn failed_start_immediately_transitions_to_fetching_turn() {
    // Caller explicitly requests restart after a failure — honor it
    // regardless of retry_after.
    let s = P2PState::Failed {
        reason: "test".into(),
        retry_after: Instant::now() + Duration::from_secs(3600),
    };
    let final_state = run_sequence(s, [start_event()]);
    assert_eq!(final_state.name(), "FetchingTurn");
}

#[test]
fn failed_peer_invite_goes_to_answering() {
    // Even in Failed, a peer invite transitions us to Answering (the
    // peer has already recovered and is trying to re-establish).
    let s = P2PState::Failed {
        reason: "test".into(),
        retry_after: Instant::now() + Duration::from_secs(30),
    };
    let final_state = run_sequence(s, [invite_event("new-call")]);
    assert_eq!(final_state.name(), "Answering");
}

// ---------------------------------------------------------------------------
// Tests — idle timeout
// ---------------------------------------------------------------------------

#[test]
fn idle_tick_from_open_goes_to_idle_and_tears_down() {
    let s = P2PState::Open {
        call_id: "c1".into(),
        last_io: Instant::now(),
    };
    if let TransitionResult::Ok { next, commands } = transition(&s, Event::IdleTick) {
        assert_eq!(next.name(), "Idle");
        assert!(commands
            .iter()
            .any(|c| matches!(c, Command::SendHangup { .. })));
        assert!(commands
            .iter()
            .any(|c| matches!(c, Command::TearDownChannel { .. })));
    } else {
        panic!();
    }
}

#[test]
fn open_channel_message_resets_idle() {
    let s = P2PState::Open {
        call_id: "c1".into(),
        last_io: Instant::now(),
    };
    let result = transition(
        &s,
        Event::ChannelEvent(ChannelEvent::Message(bytes::Bytes::from_static(b"hello"))),
    );
    match result {
        TransitionResult::Ok { next, commands } => {
            assert_eq!(next.name(), "Open", "remain Open on message");
            assert!(
                commands.iter().any(|c| matches!(c, Command::ResetIdle)),
                "inbound message must trigger ResetIdle"
            );
        }
        TransitionResult::Illegal { .. } => panic!(),
    }
}

#[test]
fn open_outbound_pressure_resets_idle() {
    let s = P2PState::Open {
        call_id: "c1".into(),
        last_io: Instant::now(),
    };
    let result = transition(&s, Event::OutboundPressure);
    match result {
        TransitionResult::Ok { next, commands } => {
            assert_eq!(next.name(), "Open");
            assert!(commands.iter().any(|c| matches!(c, Command::ResetIdle)));
        }
        TransitionResult::Illegal { .. } => panic!(),
    }
}

// ---------------------------------------------------------------------------
// Tests — glare
// ---------------------------------------------------------------------------

#[test]
fn concurrent_invite_while_inviting_goes_to_glare() {
    let s = P2PState::Inviting {
        call_id: "our".into(),
        started: Instant::now(),
        our_offer: mk_offer_sdp(),
        our_party_id: "us".into(),
        lifetime_ms: 30_000,
    };
    let final_state = run_sequence(s, [invite_event("their")]);
    assert_eq!(final_state.name(), "Glare");
}

#[test]
fn glare_state_is_no_op_for_pure_transitions() {
    // Glare resolution is computed OUTSIDE the pure transition function
    // (the driver calls glare::resolve and then injects the right
    // follow-up). Pure transitions from Glare are stay().
    let s = P2PState::Glare {
        our_call: "a".into(),
        their_call: "b".into(),
        resolution: mxdx_p2p::signaling::glare::GlareResult::WeWin,
    };
    let result = transition(&s, Event::VerifyOk);
    match result {
        TransitionResult::Ok { next, .. } => assert_eq!(next.name(), "Glare"),
        TransitionResult::Illegal { .. } => {}
    }
}

// ---------------------------------------------------------------------------
// Tests — illegal transitions never panic, never reach Open
// ---------------------------------------------------------------------------

#[test]
fn no_illegal_pair_panics() {
    // Grid-sweep across a representative sample of state × event pairs
    // and assert panic-freedom.
    let states = [
        P2PState::Idle,
        P2PState::FetchingTurn {
            since: Instant::now(),
        },
        P2PState::Inviting {
            call_id: "c".into(),
            started: Instant::now(),
            our_offer: mk_offer_sdp(),
            our_party_id: "us".into(),
            lifetime_ms: 30_000,
        },
        P2PState::Answering {
            call_id: "c".into(),
            party_id: "us".into(),
            their_party_id: "them".into(),
        },
        P2PState::Connecting {
            call_id: "c".into(),
            our_party_id: "us".into(),
            ice_started: Instant::now(),
        },
        P2PState::Verifying {
            call_id: "c".into(),
            our_party_id: "us".into(),
            our_nonce: [0u8; 32],
        },
        P2PState::Open {
            call_id: "c".into(),
            last_io: Instant::now(),
        },
        P2PState::Failed {
            reason: "x".into(),
            retry_after: Instant::now(),
        },
    ];

    let events = [
        Event::OutboundPressure,
        Event::IdleTick,
        Event::VerifyOk,
        Event::VerifyFail {
            reason: VerifyFailureReason::SignatureMismatch,
        },
        Event::TurnExpired,
        Event::InviteTimeout,
        Event::IceTimeout,
        Event::DecryptStorm,
        Event::RetryReady,
        Event::ChannelEvent(ChannelEvent::Open),
        Event::ChannelEvent(ChannelEvent::Closed {
            reason: "peer".into(),
        }),
        Event::ChannelEvent(ChannelEvent::Failure("err".into())),
        Event::TurnRefreshed {
            servers: Vec::new(),
        },
        hangup_event("c"),
        Event::Hangup {
            reason: "user".into(),
        },
    ];

    for s in &states {
        for e in &events {
            // Simply running the transition without panic is the
            // assertion — the function is #[must_use] and we don't
            // care about the result here.
            let _ = transition(s, e.clone());
        }
    }
}

#[test]
fn illegal_transitions_never_go_to_open() {
    let states = [
        P2PState::Idle,
        P2PState::FetchingTurn {
            since: Instant::now(),
        },
        P2PState::Inviting {
            call_id: "c".into(),
            started: Instant::now(),
            our_offer: mk_offer_sdp(),
            our_party_id: "us".into(),
            lifetime_ms: 30_000,
        },
        P2PState::Failed {
            reason: "x".into(),
            retry_after: Instant::now(),
        },
    ];
    let illegal_events = [
        Event::VerifyOk, // only valid from Verifying
        Event::IceTimeout,
    ];
    for s in &states {
        for e in &illegal_events {
            let result = transition(s, e.clone());
            let final_name = match result {
                TransitionResult::Ok { next, .. } => next.name(),
                TransitionResult::Illegal { .. } => s.name(),
            };
            assert_ne!(
                final_name,
                "Open",
                "state {} + illegal event {:?} must not produce Open",
                s.name(),
                e
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Tests — hangup propagation
// ---------------------------------------------------------------------------

#[test]
fn hangup_from_any_active_state_goes_to_idle() {
    let active_states = [
        P2PState::FetchingTurn {
            since: Instant::now(),
        },
        P2PState::Inviting {
            call_id: "c".into(),
            started: Instant::now(),
            our_offer: mk_offer_sdp(),
            our_party_id: "us".into(),
            lifetime_ms: 30_000,
        },
        P2PState::Answering {
            call_id: "c".into(),
            party_id: "us".into(),
            their_party_id: "them".into(),
        },
        P2PState::Connecting {
            call_id: "c".into(),
            our_party_id: "us".into(),
            ice_started: Instant::now(),
        },
        P2PState::Verifying {
            call_id: "c".into(),
            our_party_id: "us".into(),
            our_nonce: [0u8; 32],
        },
        P2PState::Open {
            call_id: "c".into(),
            last_io: Instant::now(),
        },
    ];
    for s in active_states {
        let name = s.name();
        let final_state = run_sequence(
            s,
            [Event::Hangup {
                reason: "test".into(),
            }],
        );
        assert_eq!(
            final_state.name(),
            "Idle",
            "Hangup from {name} did not reach Idle"
        );
    }
}

// ---------------------------------------------------------------------------
// Tests — select_answer from glare winner
// ---------------------------------------------------------------------------

#[test]
fn select_answer_event_passes_through_in_open() {
    // After glare resolution, the winner may emit select_answer.
    // The state machine treats incoming SelectAnswer as benign (stays
    // in current state, no-op) — the driver uses it as a signal to
    // ignore any other received answers.
    let s = P2PState::Open {
        call_id: "c".into(),
        last_io: Instant::now(),
    };
    let result = transition(
        &s,
        Event::CallEventReceived(ParsedCallEvent::SelectAnswer(CallSelectAnswer {
            call_id: "c".into(),
            party_id: "winner".into(),
            version: CALL_VERSION.into(),
            selected_party_id: "us".into(),
        })),
    );
    match result {
        TransitionResult::Ok { next, .. } => assert_eq!(next.name(), "Open"),
        TransitionResult::Illegal { .. } => panic!(),
    }
}
