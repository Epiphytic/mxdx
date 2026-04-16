//! `P2PState` enum + pure transition table.
//!
//! The state machine is intentionally a pure function: `transition(state, event)`
//! returns a new state plus a list of [`Command`]s the driver must execute.
//! The driver ([`super::driver`]) is the only place with I/O and timers;
//! everything here is deterministic, synchronous, and heap-allocation-light.
//!
//! See storm spec §2.6 for the 9-state design, §3.1 for the Verifying
//! handshake, §4.1 for the failure taxonomy, and §4.4 for resource limits.
//!
//! # Nine states
//!
//! ```text
//! Idle ──Start──► FetchingTurn ──TurnRefreshed──► Inviting ──Answer──► Connecting ──Open──► Verifying ──VerifyOk──► Open
//!                                     │                                                             │
//!                                     └──peer_invite──► Glare ──WeLose──► Answering ──Open──►  (re-enters Verifying)
//!                                                                                               │
//!                                                                                      VerifyFail
//!                                                                                               ▼
//!                                                                                            Failed
//! ```
//!
//! # Design notes
//!
//! * The state enum does **not** carry the `Box<dyn WebRtcChannel>` or
//!   `P2PCrypto` — those live in the driver's struct so the pure state is
//!   Clone-able and the transition table is trivially testable. The driver
//!   pairs a `P2PState` with `Option<Box<dyn WebRtcChannel>>` and
//!   `Option<P2PCrypto>`.
//! * [`TransitionResult::Illegal`] is returned instead of panicking when an
//!   unexpected (state, event) pair arrives. The driver logs a warning and
//!   keeps the current state.
//! * `Instant` fields use `std::time::Instant` on native; the driver
//!   translates to `tokio::time::Instant` at the sleep boundary, which
//!   makes `tokio::time::pause`/`advance` work in tests.

use std::time::{Duration, Instant};

use bytes::Bytes;

use crate::channel::{ChannelEvent as BackendEvent, IceCandidate, IceServer, Sdp};
use crate::signaling::glare::GlareResult;
use crate::signaling::parse::ParsedCallEvent;

/// Retry delay for the `Failed` state after a recoverable error (signaling
/// send failure, ICE timeout). Storm §4.2 — reused from the `SyncBackoff`
/// convention (initial 5s, capped 5min, with jitter). The state machine
/// stores only the scheduled `retry_after` instant; the driver applies
/// jitter when it schedules the retry.
pub const DEFAULT_RETRY_AFTER: Duration = Duration::from_secs(30);

/// Retry delay after an ICE timeout (no candidate pair within 30s). Slightly
/// longer than signaling failures per storm §4.1.
pub const ICE_TIMEOUT_RETRY_AFTER: Duration = Duration::from_secs(60);

/// Retry delay after a decrypt-failure storm (3/sec inbound decrypt failures
/// hangup the channel). Storm §4.4. Longer so we don't reconnect into the
/// same tag-spam.
pub const DECRYPT_STORM_RETRY_AFTER: Duration = Duration::from_secs(60);

/// Nine-state P2P lifecycle per storm §2.6.
///
/// The `WebRtcChannel` and `P2PCrypto` handles live in the driver, not the
/// enum — this keeps the pure transition table testable without dragging
/// dyn-trait bounds through the state type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum P2PState {
    /// No P2P attempted. `try_send` always returns `FallbackToMatrix`.
    Idle,

    /// Fetching TURN credentials. Serializes TURN fetch before `m.call.invite`
    /// per storm §2.4 (expiry-during-reconnect race).
    FetchingTurn { since: Instant },

    /// We sent `m.call.invite` and are awaiting `m.call.answer` within
    /// [`lifetime`] ms. `started + Duration::from_millis(lifetime)` is the
    /// timeout.
    Inviting {
        call_id: String,
        started: Instant,
        our_offer: Sdp,
        our_party_id: String,
        lifetime_ms: u64,
    },

    /// We received `m.call.invite`; sent `m.call.answer`; waiting for ICE
    /// to complete.
    Answering {
        call_id: String,
        party_id: String,
        their_party_id: String,
    },

    /// Both peers invited simultaneously. Glare resolver (phase 4) decides
    /// winner. `resolution == WeWin` → keep our invite, discard theirs,
    /// expect their answer. `resolution == TheyWin` → hang up our invite,
    /// answer theirs.
    Glare {
        our_call: String,
        their_call: String,
        resolution: GlareResult,
    },

    /// ICE negotiation in progress. Transitions to `Verifying` on
    /// `ChannelEvent::Open`, or `Failed` on timeout / `Failure`.
    Connecting {
        call_id: String,
        our_party_id: String,
        ice_started: Instant,
    },

    /// Data channel is open. Running the Ed25519-signed transcript handshake
    /// (storm §3.1). Neither peer sends application data until both sides
    /// verify. `our_nonce` is 32 random bytes from OsRng — unique per call,
    /// retained for replay detection.
    Verifying {
        call_id: String,
        our_party_id: String,
        our_nonce: [u8; 32],
    },

    /// Verified P2P session. `try_send(Megolm<Bytes>)` encrypts via
    /// `P2PCrypto` and writes to the channel. `last_io` is updated on every
    /// send and receive; the idle watchdog fires when `now - last_io >
    /// idle_window`.
    Open { call_id: String, last_io: Instant },

    /// Recoverable failure. `retry_after` is the earliest time the driver
    /// may attempt a fresh `start()`. Until then, `try_send` returns
    /// `FallbackToMatrix`.
    Failed {
        reason: String,
        retry_after: Instant,
    },
}

impl P2PState {
    /// A terse descriminant name suitable for logs, telemetry, and tests.
    /// Keeps the eight-byte allocation overhead of `format!("{:?}", state)`
    /// out of the hot path.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::FetchingTurn { .. } => "FetchingTurn",
            Self::Inviting { .. } => "Inviting",
            Self::Answering { .. } => "Answering",
            Self::Glare { .. } => "Glare",
            Self::Connecting { .. } => "Connecting",
            Self::Verifying { .. } => "Verifying",
            Self::Open { .. } => "Open",
            Self::Failed { .. } => "Failed",
        }
    }

    /// `true` iff `try_send` should encrypt+send on P2P. Only `Open`
    /// qualifies — every other state returns `FallbackToMatrix`.
    pub fn is_open(&self) -> bool {
        matches!(self, Self::Open { .. })
    }
}

/// Events that drive state transitions. Every inbound source funnels into
/// a single [`Event`] enum so the driver's `tokio::select!` arms can share
/// a single dispatch path.
#[derive(Debug, Clone)]
pub enum Event {
    /// Caller requested a new call. `peer_device_id` is optional per
    /// Matrix VoIP — `None` targets all peer devices in the room.
    Start {
        peer_user_id: String,
        peer_device_id: Option<String>,
        our_user_id: String,
        our_device_id: String,
        room_id: String,
        session_uuid: Option<String>,
    },

    /// Caller requested hangup (session end, device shutdown, manual
    /// abort). Always transitions to `Idle` regardless of current state.
    Hangup { reason: String },

    /// TURN credentials were fetched successfully. Carries the full
    /// ICE-server list we'll configure on the peer connection.
    TurnRefreshed { servers: Vec<IceServer> },

    /// TURN credentials expired without a successful refresh (storm §2.4).
    /// In `Open`, hang up. In `FetchingTurn`, transition to `Failed`.
    TurnExpired,

    /// A `m.call.*` event arrived via the Matrix sync loop. Glare,
    /// answer processing, and remote hangup are all funneled through here.
    CallEventReceived(ParsedCallEvent),

    /// An event from the WebRTC backend (`WebRtcChannel::events()`).
    /// Open, Message, LocalIce, Closed, Failure.
    ChannelEvent(BackendEvent),

    /// The caller pushed a payload onto the outbound queue. The state
    /// machine does not carry the payload — the driver owns the queue —
    /// this event just signals "outbound pressure exists".
    OutboundPressure,

    /// Idle watchdog fired — no I/O for the configured window.
    IdleTick,

    /// Invite lifetime expired without an answer.
    InviteTimeout,

    /// ICE connection did not reach `connected` within the configured
    /// window (default 30s per storm §4.1).
    IceTimeout,

    /// Verifying handshake completed — peer signature verified.
    VerifyOk,

    /// Verifying handshake failed — signature mismatch, replay, or 3-strike
    /// device lockout.
    VerifyFail { reason: VerifyFailureReason },

    /// The driver's bounded inbound decrypt-failure counter crossed the
    /// 3/sec threshold (storm §4.4). Triggers a channel teardown.
    DecryptStorm,

    /// A retry window elapsed — the driver may attempt a fresh `start()`
    /// if the caller requests one.
    RetryReady,
}

/// Why the Verifying handshake failed. Surfaces directly as a security
/// telemetry `kind` (storm §4.6) and distinguishes retry-worthy failures
/// (should never happen) from adversary-signal failures (device lockout).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyFailureReason {
    /// Peer's Ed25519 signature over the transcript did not verify.
    SignatureMismatch,
    /// Peer's nonce was seen in a recent call — replay attempt.
    ReplayDetected,
    /// Handshake timed out (channel never produced peer's nonce+signature
    /// within the verify window).
    Timeout,
    /// Handshake message deserialization failed (malformed JSON after
    /// AES-GCM decrypt). Treated as an adversary signal.
    InvalidPayload,
    /// Third consecutive verify failure against this peer device — lock
    /// out P2P for the rest of the session.
    DeviceLockedOut,
}

/// Telemetry event kinds emitted by the transport. Mapped 1:1 to storm
/// §4.6 `p2p.*` events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TelemetryKind {
    /// `p2p.state_transition { from, to, reason, session_uuid }`.
    StateTransition {
        from: &'static str,
        to: &'static str,
        reason: String,
    },
    /// `p2p.handshake_completed { session_uuid, total_ms, ice_ms, dtls_ms, verify_ms }`.
    HandshakeCompleted {
        total_ms: u64,
        ice_ms: u64,
        verify_ms: u64,
    },
    /// `p2p.turn_refresh { session_uuid, outcome }`.
    TurnRefresh { outcome: String },
    /// `p2p.fallback { session_uuid, reason }`.
    Fallback { reason: String },
    /// `p2p.security_event { kind: VerifyFailure | DecryptStorm | ReplayDetected | WrongPeer, ... }`.
    SecurityEvent {
        kind: SecurityEventKind,
        peer: Option<String>,
    },
}

/// Security telemetry subtypes (storm §4.6 "always on").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityEventKind {
    VerifyFailure,
    DecryptStorm,
    ReplayDetected,
    WrongPeer,
}

/// Side effects the driver executes when the state machine returns. Every
/// I/O call — Matrix send, channel tear-down, timer schedule, telemetry —
/// goes through a [`Command`]. This keeps the state machine pure and makes
/// the driver a thin executor.
#[derive(Debug, Clone)]
pub enum Command {
    /// Emit `m.call.invite` with the given offer SDP and embedded
    /// `mxdx_session_key`. The driver generates the `SealedKey`, builds
    /// the `CallInvite` via [`crate::signaling::events::build_invite`], and
    /// sends via `MatrixClient::send_call_event`.
    SendInvite {
        call_id: String,
        party_id: String,
        sdp: Sdp,
        lifetime_ms: u64,
        session_uuid: Option<String>,
    },

    /// Emit `m.call.answer`.
    SendAnswer {
        call_id: String,
        party_id: String,
        sdp: Sdp,
    },

    /// Emit `m.call.candidates` with a batch of ICE candidates.
    SendCandidates {
        call_id: String,
        party_id: String,
        candidates: Vec<IceCandidate>,
    },

    /// Emit `m.call.hangup`.
    SendHangup {
        call_id: String,
        party_id: String,
        reason: String,
    },

    /// Emit `m.call.select_answer` (glare-resolution winner only).
    SendSelectAnswer {
        call_id: String,
        party_id: String,
        selected_party_id: String,
    },

    /// Begin TURN fetch. Driver routes through `TurnRefreshTask::spawn` or
    /// an immediate `fetch_turn_credentials` call.
    BeginFetchTurn,

    /// Start the Verifying handshake. Driver constructs the AES-GCM
    /// challenge frame with the transcript+nonce+signature and sends it
    /// over the channel.
    StartVerifying { our_nonce: [u8; 32] },

    /// Cancel any pending TURN refresh and tear down the data channel.
    /// Driver calls `WebRtcChannel::close(reason)`.
    TearDownChannel { reason: String },

    /// Configure ICE servers on a newly-created peer connection. Emitted
    /// during `start()` right before `create_offer`.
    ConfigureIceServers { servers: Vec<IceServer> },

    /// Schedule a one-shot event to re-fire at `at`. Driver arms a
    /// tokio sleep; when it fires, the driver pushes [`Event::RetryReady`]
    /// (or similar) back into the state machine.
    Schedule { at: Instant, event: ScheduledEvent },

    /// Reset the idle watchdog. Emitted on every successful send and
    /// receive in `Open`.
    ResetIdle,

    /// Emit a telemetry event. Always non-blocking; driver forwards to
    /// the telemetry bus.
    EmitTelemetry(TelemetryKind),

    /// Mark a peer device as `unverified_p2p` for the rest of the session.
    /// After three verify failures against the same peer device.
    LockoutDevice { peer: String },

    /// Outbound `try_send` queue had a payload waiting and we're now in
    /// `Open` — drain it. Driver pops from queue, encrypts, writes to
    /// channel, emits `EmitTelemetry`.
    DrainOutbound,
}

/// Side-effect events that can be scheduled with [`Command::Schedule`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduledEvent {
    InviteTimeout,
    IceTimeout,
    RetryReady,
}

/// Result of applying an [`Event`] to a [`P2PState`]. The driver matches
/// on this and either applies the transition + commands (`Ok`) or logs
/// a warning and keeps the current state (`Illegal`).
#[derive(Debug, Clone)]
pub enum TransitionResult {
    Ok {
        next: P2PState,
        commands: Vec<Command>,
    },
    /// The `(state, event)` pair is not a valid transition. The driver
    /// must not panic — it logs `note` at warn level and retains the
    /// current state. `note` is a `&'static str` for structured logging.
    Illegal { note: &'static str },
}

impl TransitionResult {
    fn ok(next: P2PState, commands: Vec<Command>) -> Self {
        Self::Ok { next, commands }
    }

    fn illegal(note: &'static str) -> Self {
        Self::Illegal { note }
    }

    fn stay(current: &P2PState) -> Self {
        Self::Ok {
            next: current.clone(),
            commands: Vec::new(),
        }
    }
}

/// Pure state-transition function. No I/O, no timers, no panics.
///
/// The driver is responsible for:
/// 1. Executing every returned [`Command`] before processing the next
///    event.
/// 2. Logging [`TransitionResult::Illegal`] at warn level and retaining
///    the current state.
///
/// # Note on `OutboundPressure`
///
/// The state machine does not move the payload — the driver owns the
/// outbound queue. This event just signals "caller wants to send". In
/// `Open`, the driver pops + encrypts directly without asking the state
/// machine (emitting [`Command::DrainOutbound`] is only needed to cover
/// the Connecting→Open transition when the queue already has pending
/// frames).
pub fn transition(state: &P2PState, event: Event) -> TransitionResult {
    use Event::*;
    use P2PState::*;

    // Global events that are valid in every state go first.
    if let Hangup { reason } = &event {
        let mut commands = Vec::new();
        if let Some((call_id, party_id)) = current_call(state) {
            commands.push(Command::SendHangup {
                call_id: call_id.to_string(),
                party_id: party_id.to_string(),
                reason: reason.clone(),
            });
            commands.push(Command::TearDownChannel {
                reason: reason.clone(),
            });
        }
        commands.push(Command::EmitTelemetry(TelemetryKind::StateTransition {
            from: state.name(),
            to: "Idle",
            reason: reason.clone(),
        }));
        return TransitionResult::ok(Idle, commands);
    }

    match (state, event) {
        // ---- Idle ----
        (
            Idle,
            Start {
                peer_user_id: _,
                peer_device_id: _,
                our_user_id: _,
                our_device_id: _,
                room_id: _,
                session_uuid: _,
            },
        ) => TransitionResult::ok(
            FetchingTurn {
                since: Instant::now(),
            },
            vec![
                Command::BeginFetchTurn,
                Command::EmitTelemetry(TelemetryKind::StateTransition {
                    from: "Idle",
                    to: "FetchingTurn",
                    reason: "start".into(),
                }),
            ],
        ),
        (Idle, CallEventReceived(ParsedCallEvent::Invite(inv))) => {
            // Inbound invite while Idle → transition to Answering. The
            // driver is responsible for calling `P2PCrypto::from_sealed`
            // on the decoded `mxdx_session_key`, running
            // `WebRtcChannel::accept_offer`, and emitting SendAnswer.
            TransitionResult::ok(
                Answering {
                    call_id: inv.call_id.clone(),
                    party_id: "our-party".into(), // driver overrides
                    their_party_id: inv.party_id.clone(),
                },
                vec![
                    Command::BeginFetchTurn, // need TURN before accept_offer
                    Command::EmitTelemetry(TelemetryKind::StateTransition {
                        from: "Idle",
                        to: "Answering",
                        reason: "peer_invite".into(),
                    }),
                ],
            )
        }
        (Idle, CallEventReceived(_)) => {
            // Hangup/candidates/answer/select_answer/unknown while Idle
            // are harmless — drop.
            TransitionResult::stay(state)
        }
        (Idle, ChannelEvent(_)) => TransitionResult::stay(state),
        (Idle, OutboundPressure | IdleTick | RetryReady) => TransitionResult::stay(state),
        (Idle, TurnRefreshed { .. } | TurnExpired | VerifyOk | VerifyFail { .. }) => {
            TransitionResult::stay(state)
        }
        (Idle, InviteTimeout | IceTimeout | DecryptStorm) => TransitionResult::stay(state),

        // ---- FetchingTurn ----
        (FetchingTurn { .. }, TurnRefreshed { servers }) => {
            // Ready to invite. Driver generates call_id/party_id/session_key
            // and produces an SDP via WebRtcChannel::create_offer.
            // The state machine returns placeholders for call_id/sdp; the
            // driver patches them in the SendInvite command after
            // create_offer completes. For the pure-transition tests, we
            // model that via Commands only.
            let call_id = "<pending>".to_string();
            let party_id = "<pending>".to_string();
            let sdp = Sdp {
                kind: crate::channel::SdpKind::Offer,
                sdp: String::new(),
            };
            let next = Inviting {
                call_id: call_id.clone(),
                started: Instant::now(),
                our_offer: sdp.clone(),
                our_party_id: party_id.clone(),
                lifetime_ms: crate::signaling::events::DEFAULT_INVITE_LIFETIME_MS,
            };
            TransitionResult::ok(
                next,
                vec![
                    Command::ConfigureIceServers { servers },
                    Command::SendInvite {
                        call_id,
                        party_id,
                        sdp,
                        lifetime_ms: crate::signaling::events::DEFAULT_INVITE_LIFETIME_MS,
                        session_uuid: None,
                    },
                    Command::Schedule {
                        at: Instant::now()
                            + Duration::from_millis(
                                crate::signaling::events::DEFAULT_INVITE_LIFETIME_MS,
                            ),
                        event: ScheduledEvent::InviteTimeout,
                    },
                    Command::EmitTelemetry(TelemetryKind::StateTransition {
                        from: "FetchingTurn",
                        to: "Inviting",
                        reason: "turn_ready".into(),
                    }),
                ],
            )
        }
        (FetchingTurn { .. }, TurnExpired) => TransitionResult::ok(
            Failed {
                reason: "turn_expired".into(),
                retry_after: Instant::now() + DEFAULT_RETRY_AFTER,
            },
            vec![
                Command::EmitTelemetry(TelemetryKind::Fallback {
                    reason: "turn_expired".into(),
                }),
                Command::EmitTelemetry(TelemetryKind::StateTransition {
                    from: "FetchingTurn",
                    to: "Failed",
                    reason: "turn_expired".into(),
                }),
            ],
        ),
        (FetchingTurn { .. }, CallEventReceived(ParsedCallEvent::Invite(inv))) => {
            // We were trying to invite them; they're trying to invite us.
            // Can't run glare yet — we don't have our own call_id because
            // we haven't sent our invite. Transition to Answering (we
            // effectively concede the glare since we hadn't sent anything).
            TransitionResult::ok(
                Answering {
                    call_id: inv.call_id.clone(),
                    party_id: "<pending>".into(),
                    their_party_id: inv.party_id.clone(),
                },
                vec![Command::EmitTelemetry(TelemetryKind::StateTransition {
                    from: "FetchingTurn",
                    to: "Answering",
                    reason: "peer_invite_preempts_turn".into(),
                })],
            )
        }
        (FetchingTurn { .. }, CallEventReceived(_)) => TransitionResult::stay(state),
        (FetchingTurn { .. }, ChannelEvent(_)) => TransitionResult::stay(state),
        (FetchingTurn { .. }, OutboundPressure | IdleTick | RetryReady) => {
            TransitionResult::stay(state)
        }
        (FetchingTurn { .. }, InviteTimeout | IceTimeout | VerifyOk | VerifyFail { .. }) => {
            TransitionResult::illegal("fetching_turn_timer_events_dropped")
        }
        (FetchingTurn { .. }, Start { .. }) => {
            TransitionResult::illegal("double_start_while_fetching_turn")
        }
        (FetchingTurn { .. }, DecryptStorm) => TransitionResult::stay(state),

        // ---- Inviting ----
        (Inviting { call_id, .. }, CallEventReceived(ParsedCallEvent::Answer(ans))) => {
            if ans.call_id != *call_id {
                return TransitionResult::illegal("inviting_answer_call_id_mismatch");
            }
            let our_party = match state {
                Inviting { our_party_id, .. } => our_party_id.clone(),
                _ => unreachable!(),
            };
            let call = call_id.clone();
            TransitionResult::ok(
                Connecting {
                    call_id: call,
                    our_party_id: our_party,
                    ice_started: Instant::now(),
                },
                vec![
                    Command::Schedule {
                        at: Instant::now() + Duration::from_secs(30),
                        event: ScheduledEvent::IceTimeout,
                    },
                    Command::EmitTelemetry(TelemetryKind::StateTransition {
                        from: "Inviting",
                        to: "Connecting",
                        reason: "answer_received".into(),
                    }),
                ],
            )
        }
        (
            Inviting {
                call_id: our_call,
                our_party_id,
                ..
            },
            CallEventReceived(ParsedCallEvent::Invite(peer_inv)),
        ) => {
            // Glare: both peers invited simultaneously. Resolve on the
            // driver side using our user_id/their user_id — here we record
            // the glare state; the driver computes GlareResult and
            // re-dispatches.
            let _ = (our_party_id, &peer_inv);
            // State machine doesn't have user_ids here — driver resolves
            // glare and drives the state forward via the
            // CallEventReceived::Answer path (if we won) or via a new
            // transition to Answering (if we lost). We model the
            // intermediate state with a placeholder GlareResult; the
            // driver replaces this immediately.
            TransitionResult::ok(
                Glare {
                    our_call: our_call.clone(),
                    their_call: peer_inv.call_id.clone(),
                    resolution: GlareResult::WeWin, // driver overrides via resolve()
                },
                vec![Command::EmitTelemetry(TelemetryKind::StateTransition {
                    from: "Inviting",
                    to: "Glare",
                    reason: "concurrent_invites".into(),
                })],
            )
        }
        (Inviting { .. }, InviteTimeout) => {
            let call = match state {
                Inviting { call_id, .. } => call_id.clone(),
                _ => unreachable!(),
            };
            let party = match state {
                Inviting { our_party_id, .. } => our_party_id.clone(),
                _ => unreachable!(),
            };
            TransitionResult::ok(
                Failed {
                    reason: "invite_timeout".into(),
                    retry_after: Instant::now() + DEFAULT_RETRY_AFTER,
                },
                vec![
                    Command::SendHangup {
                        call_id: call,
                        party_id: party,
                        reason: "invite_timeout".into(),
                    },
                    Command::TearDownChannel {
                        reason: "invite_timeout".into(),
                    },
                    Command::EmitTelemetry(TelemetryKind::StateTransition {
                        from: "Inviting",
                        to: "Failed",
                        reason: "invite_timeout".into(),
                    }),
                ],
            )
        }
        (Inviting { .. }, CallEventReceived(ParsedCallEvent::Hangup(_))) => TransitionResult::ok(
            Failed {
                reason: "peer_hangup".into(),
                retry_after: Instant::now() + DEFAULT_RETRY_AFTER,
            },
            vec![
                Command::TearDownChannel {
                    reason: "peer_hangup".into(),
                },
                Command::EmitTelemetry(TelemetryKind::StateTransition {
                    from: "Inviting",
                    to: "Failed",
                    reason: "peer_hangup".into(),
                }),
            ],
        ),
        (Inviting { .. }, CallEventReceived(_)) => TransitionResult::stay(state),
        (Inviting { .. }, ChannelEvent(_)) => TransitionResult::stay(state),
        (Inviting { .. }, OutboundPressure | IdleTick | RetryReady) => {
            TransitionResult::stay(state)
        }
        (Inviting { .. }, TurnRefreshed { .. } | TurnExpired) => TransitionResult::stay(state),
        (Inviting { .. }, IceTimeout) => TransitionResult::stay(state),
        (Inviting { .. }, VerifyOk | VerifyFail { .. } | DecryptStorm) => {
            TransitionResult::illegal("inviting_post_verify_events_dropped")
        }
        (Inviting { .. }, Start { .. }) => TransitionResult::illegal("double_start_while_inviting"),

        // ---- Answering ----
        (
            Answering {
                call_id, party_id, ..
            },
            ChannelEvent(BackendEvent::Open),
        ) => TransitionResult::ok(
            Verifying {
                call_id: call_id.clone(),
                our_party_id: party_id.clone(),
                our_nonce: [0u8; 32], // driver overrides with OsRng
            },
            vec![
                Command::StartVerifying {
                    our_nonce: [0u8; 32], // driver overrides
                },
                Command::EmitTelemetry(TelemetryKind::StateTransition {
                    from: "Answering",
                    to: "Verifying",
                    reason: "channel_open".into(),
                }),
            ],
        ),
        (Answering { .. }, CallEventReceived(ParsedCallEvent::Hangup(_))) => TransitionResult::ok(
            Failed {
                reason: "peer_hangup".into(),
                retry_after: Instant::now() + DEFAULT_RETRY_AFTER,
            },
            vec![
                Command::TearDownChannel {
                    reason: "peer_hangup".into(),
                },
                Command::EmitTelemetry(TelemetryKind::StateTransition {
                    from: "Answering",
                    to: "Failed",
                    reason: "peer_hangup".into(),
                }),
            ],
        ),
        (Answering { .. }, ChannelEvent(BackendEvent::Failure(msg))) => TransitionResult::ok(
            Failed {
                reason: format!("channel_failure:{msg}"),
                retry_after: Instant::now() + ICE_TIMEOUT_RETRY_AFTER,
            },
            vec![
                Command::TearDownChannel {
                    reason: "channel_failure".into(),
                },
                Command::EmitTelemetry(TelemetryKind::StateTransition {
                    from: "Answering",
                    to: "Failed",
                    reason: "channel_failure".into(),
                }),
            ],
        ),
        (Answering { .. }, CallEventReceived(_)) => TransitionResult::stay(state),
        (Answering { .. }, ChannelEvent(_)) => TransitionResult::stay(state),
        (Answering { .. }, OutboundPressure | IdleTick | RetryReady) => {
            TransitionResult::stay(state)
        }
        (Answering { .. }, TurnRefreshed { .. } | TurnExpired) => TransitionResult::stay(state),
        (Answering { .. }, InviteTimeout | IceTimeout) => TransitionResult::stay(state),
        (Answering { .. }, VerifyOk | VerifyFail { .. } | DecryptStorm) => {
            TransitionResult::illegal("answering_post_verify_events_dropped")
        }
        (Answering { .. }, Start { .. }) => {
            TransitionResult::illegal("double_start_while_answering")
        }

        // ---- Glare ----
        // Glare resolution is computed by the driver using the pure glare
        // resolver; the driver then constructs the right transition
        // manually (not via this pure function). Anything reaching the
        // Glare state here is a no-op until the driver resolves it.
        (Glare { .. }, _) => TransitionResult::stay(state),

        // ---- Connecting ----
        (
            Connecting {
                call_id,
                our_party_id,
                ..
            },
            ChannelEvent(BackendEvent::Open),
        ) => TransitionResult::ok(
            Verifying {
                call_id: call_id.clone(),
                our_party_id: our_party_id.clone(),
                our_nonce: [0u8; 32], // driver overrides
            },
            vec![
                Command::StartVerifying {
                    our_nonce: [0u8; 32], // driver overrides
                },
                Command::EmitTelemetry(TelemetryKind::StateTransition {
                    from: "Connecting",
                    to: "Verifying",
                    reason: "channel_open".into(),
                }),
            ],
        ),
        (Connecting { .. }, IceTimeout) => {
            let call = match state {
                Connecting { call_id, .. } => call_id.clone(),
                _ => unreachable!(),
            };
            let party = match state {
                Connecting { our_party_id, .. } => our_party_id.clone(),
                _ => unreachable!(),
            };
            TransitionResult::ok(
                Failed {
                    reason: "ice_timeout".into(),
                    retry_after: Instant::now() + ICE_TIMEOUT_RETRY_AFTER,
                },
                vec![
                    Command::SendHangup {
                        call_id: call,
                        party_id: party,
                        reason: "ice_timeout".into(),
                    },
                    Command::TearDownChannel {
                        reason: "ice_timeout".into(),
                    },
                    Command::EmitTelemetry(TelemetryKind::StateTransition {
                        from: "Connecting",
                        to: "Failed",
                        reason: "ice_timeout".into(),
                    }),
                ],
            )
        }
        (Connecting { .. }, CallEventReceived(ParsedCallEvent::Hangup(_))) => TransitionResult::ok(
            Failed {
                reason: "peer_hangup".into(),
                retry_after: Instant::now() + DEFAULT_RETRY_AFTER,
            },
            vec![
                Command::TearDownChannel {
                    reason: "peer_hangup".into(),
                },
                Command::EmitTelemetry(TelemetryKind::StateTransition {
                    from: "Connecting",
                    to: "Failed",
                    reason: "peer_hangup".into(),
                }),
            ],
        ),
        (Connecting { .. }, ChannelEvent(BackendEvent::Failure(msg))) => TransitionResult::ok(
            Failed {
                reason: format!("channel_failure:{msg}"),
                retry_after: Instant::now() + ICE_TIMEOUT_RETRY_AFTER,
            },
            vec![
                Command::TearDownChannel {
                    reason: "channel_failure".into(),
                },
                Command::EmitTelemetry(TelemetryKind::StateTransition {
                    from: "Connecting",
                    to: "Failed",
                    reason: "channel_failure".into(),
                }),
            ],
        ),
        (Connecting { .. }, ChannelEvent(_)) => TransitionResult::stay(state),
        (Connecting { .. }, CallEventReceived(_)) => TransitionResult::stay(state),
        (Connecting { .. }, OutboundPressure | IdleTick | RetryReady) => {
            TransitionResult::stay(state)
        }
        (Connecting { .. }, TurnRefreshed { .. } | TurnExpired) => TransitionResult::stay(state),
        (Connecting { .. }, InviteTimeout) => TransitionResult::stay(state),
        (Connecting { .. }, VerifyOk | VerifyFail { .. } | DecryptStorm) => {
            TransitionResult::illegal("connecting_post_verify_events_dropped")
        }
        (Connecting { .. }, Start { .. }) => {
            TransitionResult::illegal("double_start_while_connecting")
        }

        // ---- Verifying ----
        (Verifying { call_id, .. }, VerifyOk) => TransitionResult::ok(
            Open {
                call_id: call_id.clone(),
                last_io: Instant::now(),
            },
            vec![
                Command::ResetIdle,
                Command::DrainOutbound,
                Command::EmitTelemetry(TelemetryKind::StateTransition {
                    from: "Verifying",
                    to: "Open",
                    reason: "verify_ok".into(),
                }),
                Command::EmitTelemetry(TelemetryKind::HandshakeCompleted {
                    total_ms: 0, // driver fills in
                    ice_ms: 0,
                    verify_ms: 0,
                }),
            ],
        ),
        (Verifying { .. }, VerifyFail { reason }) => {
            let (retry_after, security_kind, reason_str) = match reason {
                VerifyFailureReason::SignatureMismatch => (
                    Instant::now() + DEFAULT_RETRY_AFTER,
                    SecurityEventKind::VerifyFailure,
                    "verify_failure",
                ),
                VerifyFailureReason::ReplayDetected => (
                    Instant::now() + DEFAULT_RETRY_AFTER,
                    SecurityEventKind::ReplayDetected,
                    "replay_detected",
                ),
                VerifyFailureReason::Timeout => (
                    Instant::now() + DEFAULT_RETRY_AFTER,
                    SecurityEventKind::VerifyFailure,
                    "verify_timeout",
                ),
                VerifyFailureReason::InvalidPayload => (
                    Instant::now() + DEFAULT_RETRY_AFTER,
                    SecurityEventKind::VerifyFailure,
                    "verify_invalid_payload",
                ),
                VerifyFailureReason::DeviceLockedOut => (
                    Instant::now() + DEFAULT_RETRY_AFTER,
                    SecurityEventKind::WrongPeer,
                    "device_locked_out",
                ),
            };
            let call = match state {
                Verifying { call_id, .. } => call_id.clone(),
                _ => unreachable!(),
            };
            let party = match state {
                Verifying { our_party_id, .. } => our_party_id.clone(),
                _ => unreachable!(),
            };
            TransitionResult::ok(
                Failed {
                    reason: reason_str.into(),
                    retry_after,
                },
                vec![
                    Command::SendHangup {
                        call_id: call,
                        party_id: party,
                        reason: reason_str.into(),
                    },
                    Command::TearDownChannel {
                        reason: reason_str.into(),
                    },
                    Command::EmitTelemetry(TelemetryKind::SecurityEvent {
                        kind: security_kind,
                        peer: None,
                    }),
                    Command::EmitTelemetry(TelemetryKind::StateTransition {
                        from: "Verifying",
                        to: "Failed",
                        reason: reason_str.into(),
                    }),
                ],
            )
        }
        (Verifying { .. }, ChannelEvent(BackendEvent::Closed { reason })) => TransitionResult::ok(
            Failed {
                reason: format!("channel_closed_during_verify:{reason}"),
                retry_after: Instant::now() + DEFAULT_RETRY_AFTER,
            },
            vec![Command::EmitTelemetry(TelemetryKind::StateTransition {
                from: "Verifying",
                to: "Failed",
                reason: "channel_closed".into(),
            })],
        ),
        (Verifying { .. }, ChannelEvent(BackendEvent::Failure(msg))) => TransitionResult::ok(
            Failed {
                reason: format!("channel_failure:{msg}"),
                retry_after: Instant::now() + ICE_TIMEOUT_RETRY_AFTER,
            },
            vec![
                Command::TearDownChannel {
                    reason: "channel_failure".into(),
                },
                Command::EmitTelemetry(TelemetryKind::StateTransition {
                    from: "Verifying",
                    to: "Failed",
                    reason: "channel_failure".into(),
                }),
            ],
        ),
        (Verifying { .. }, ChannelEvent(_)) => TransitionResult::stay(state),
        (Verifying { .. }, CallEventReceived(ParsedCallEvent::Hangup(_))) => TransitionResult::ok(
            Failed {
                reason: "peer_hangup".into(),
                retry_after: Instant::now() + DEFAULT_RETRY_AFTER,
            },
            vec![
                Command::TearDownChannel {
                    reason: "peer_hangup".into(),
                },
                Command::EmitTelemetry(TelemetryKind::StateTransition {
                    from: "Verifying",
                    to: "Failed",
                    reason: "peer_hangup".into(),
                }),
            ],
        ),
        (Verifying { .. }, CallEventReceived(_)) => TransitionResult::stay(state),
        (Verifying { .. }, OutboundPressure | IdleTick | RetryReady) => {
            TransitionResult::stay(state)
        }
        (Verifying { .. }, TurnRefreshed { .. } | TurnExpired) => TransitionResult::stay(state),
        (Verifying { .. }, InviteTimeout | IceTimeout | DecryptStorm) => {
            TransitionResult::illegal("verifying_unexpected_timer")
        }
        (Verifying { .. }, Start { .. }) => {
            TransitionResult::illegal("double_start_while_verifying")
        }

        // ---- Open ----
        (Open { call_id, .. }, OutboundPressure) => TransitionResult::ok(
            Open {
                call_id: call_id.clone(),
                last_io: Instant::now(),
            },
            vec![Command::ResetIdle],
        ),
        (Open { call_id, .. }, ChannelEvent(BackendEvent::Message(_))) => TransitionResult::ok(
            Open {
                call_id: call_id.clone(),
                last_io: Instant::now(),
            },
            vec![Command::ResetIdle],
        ),
        (Open { .. }, IdleTick) => {
            let call = match state {
                Open { call_id, .. } => call_id.clone(),
                _ => unreachable!(),
            };
            TransitionResult::ok(
                Idle,
                vec![
                    Command::SendHangup {
                        call_id: call,
                        party_id: "<current>".into(), // driver fills in
                        reason: "idle_timeout".into(),
                    },
                    Command::TearDownChannel {
                        reason: "idle_timeout".into(),
                    },
                    Command::EmitTelemetry(TelemetryKind::StateTransition {
                        from: "Open",
                        to: "Idle",
                        reason: "idle_timeout".into(),
                    }),
                ],
            )
        }
        (Open { .. }, TurnRefreshed { servers }) => {
            // Storm §3.4: attempt restart_ice; on RestartIceUnsupported
            // (datachannel-sys 0.23 per phase-3 marker), tear down and
            // re-invite. The state machine signals intent via
            // ConfigureIceServers; the driver invokes restart_ice and on
            // Unsupported re-triggers the start flow.
            TransitionResult::ok(
                state.clone(),
                vec![Command::ConfigureIceServers { servers }],
            )
        }
        (Open { .. }, TurnExpired) => {
            let call = match state {
                Open { call_id, .. } => call_id.clone(),
                _ => unreachable!(),
            };
            TransitionResult::ok(
                Failed {
                    reason: "turn_expired".into(),
                    retry_after: Instant::now() + DEFAULT_RETRY_AFTER,
                },
                vec![
                    Command::SendHangup {
                        call_id: call,
                        party_id: "<current>".into(),
                        reason: "turn_expired".into(),
                    },
                    Command::TearDownChannel {
                        reason: "turn_expired".into(),
                    },
                    Command::EmitTelemetry(TelemetryKind::StateTransition {
                        from: "Open",
                        to: "Failed",
                        reason: "turn_expired".into(),
                    }),
                ],
            )
        }
        (Open { .. }, DecryptStorm) => {
            let call = match state {
                Open { call_id, .. } => call_id.clone(),
                _ => unreachable!(),
            };
            TransitionResult::ok(
                Failed {
                    reason: "decrypt_storm".into(),
                    retry_after: Instant::now() + DECRYPT_STORM_RETRY_AFTER,
                },
                vec![
                    Command::SendHangup {
                        call_id: call,
                        party_id: "<current>".into(),
                        reason: "decrypt_storm".into(),
                    },
                    Command::TearDownChannel {
                        reason: "decrypt_storm".into(),
                    },
                    Command::EmitTelemetry(TelemetryKind::SecurityEvent {
                        kind: SecurityEventKind::DecryptStorm,
                        peer: None,
                    }),
                    Command::EmitTelemetry(TelemetryKind::StateTransition {
                        from: "Open",
                        to: "Failed",
                        reason: "decrypt_storm".into(),
                    }),
                ],
            )
        }
        (Open { .. }, CallEventReceived(ParsedCallEvent::Hangup(_))) => TransitionResult::ok(
            Idle,
            vec![
                Command::TearDownChannel {
                    reason: "peer_hangup".into(),
                },
                Command::EmitTelemetry(TelemetryKind::StateTransition {
                    from: "Open",
                    to: "Idle",
                    reason: "peer_hangup".into(),
                }),
            ],
        ),
        (Open { .. }, ChannelEvent(BackendEvent::Closed { reason })) => TransitionResult::ok(
            Idle,
            vec![Command::EmitTelemetry(TelemetryKind::StateTransition {
                from: "Open",
                to: "Idle",
                reason: format!("channel_closed:{reason}"),
            })],
        ),
        (Open { .. }, ChannelEvent(BackendEvent::Failure(msg))) => TransitionResult::ok(
            Failed {
                reason: format!("channel_failure:{msg}"),
                retry_after: Instant::now() + DEFAULT_RETRY_AFTER,
            },
            vec![
                Command::TearDownChannel {
                    reason: "channel_failure".into(),
                },
                Command::EmitTelemetry(TelemetryKind::StateTransition {
                    from: "Open",
                    to: "Failed",
                    reason: "channel_failure".into(),
                }),
            ],
        ),
        (Open { .. }, ChannelEvent(_)) => TransitionResult::stay(state),
        (Open { .. }, CallEventReceived(_)) => TransitionResult::stay(state),
        (Open { .. }, RetryReady) => TransitionResult::stay(state),
        (Open { .. }, InviteTimeout | IceTimeout | VerifyOk | VerifyFail { .. }) => {
            TransitionResult::illegal("open_timer_events_dropped")
        }
        (Open { .. }, Start { .. }) => TransitionResult::illegal("double_start_while_open"),

        // ---- Failed ----
        (Failed { retry_after, .. }, RetryReady) => {
            let now = Instant::now();
            if now >= *retry_after {
                TransitionResult::ok(
                    Idle,
                    vec![Command::EmitTelemetry(TelemetryKind::StateTransition {
                        from: "Failed",
                        to: "Idle",
                        reason: "retry_ready".into(),
                    })],
                )
            } else {
                TransitionResult::stay(state)
            }
        }
        (Failed { .. }, Start { .. }) => {
            // Caller wants to restart immediately — honor it regardless of
            // retry_after. The driver may rate-limit externally.
            TransitionResult::ok(
                FetchingTurn {
                    since: Instant::now(),
                },
                vec![
                    Command::BeginFetchTurn,
                    Command::EmitTelemetry(TelemetryKind::StateTransition {
                        from: "Failed",
                        to: "FetchingTurn",
                        reason: "start_from_failed".into(),
                    }),
                ],
            )
        }
        (Failed { .. }, CallEventReceived(ParsedCallEvent::Invite(inv))) => TransitionResult::ok(
            Answering {
                call_id: inv.call_id.clone(),
                party_id: "<pending>".into(),
                their_party_id: inv.party_id.clone(),
            },
            vec![Command::EmitTelemetry(TelemetryKind::StateTransition {
                from: "Failed",
                to: "Answering",
                reason: "peer_invite".into(),
            })],
        ),
        (Failed { .. }, _) => TransitionResult::stay(state),

        // Hangup is handled before this match by the pre-filter above and
        // short-circuits. This arm exists only to satisfy the exhaustiveness
        // checker — it is unreachable at runtime.
        (_, Hangup { .. }) => TransitionResult::stay(state),
    }
}

/// Extract the current `(call_id, party_id)` pair if the state holds one,
/// used by the hangup handler. `Failed` and `Idle` return `None`.
fn current_call(state: &P2PState) -> Option<(&str, &str)> {
    match state {
        P2PState::Idle | P2PState::FetchingTurn { .. } | P2PState::Failed { .. } => None,
        P2PState::Inviting {
            call_id,
            our_party_id,
            ..
        }
        | P2PState::Connecting {
            call_id,
            our_party_id,
            ..
        }
        | P2PState::Verifying {
            call_id,
            our_party_id,
            ..
        } => Some((call_id.as_str(), our_party_id.as_str())),
        P2PState::Answering {
            call_id, party_id, ..
        } => Some((call_id.as_str(), party_id.as_str())),
        P2PState::Glare { our_call, .. } => Some((our_call.as_str(), "<pending>")),
        P2PState::Open { call_id, .. } => Some((call_id.as_str(), "<current>")),
    }
}

/// Drain signal: placeholder for compatibility with inbound mpsc types.
/// Not part of the public API.
#[allow(dead_code)]
pub(crate) type Inbox = Vec<Bytes>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::SdpKind;
    use crate::signaling::events::{
        CallAnswer, CallHangup, CallInvite, CallSdpField, CALL_VERSION,
    };

    fn mk_offer() -> Sdp {
        Sdp {
            kind: SdpKind::Offer,
            sdp: "v=0\r\n".into(),
        }
    }

    fn mk_answer_event(call_id: &str) -> ParsedCallEvent {
        ParsedCallEvent::Answer(CallAnswer {
            call_id: call_id.into(),
            party_id: "them".into(),
            version: CALL_VERSION.into(),
            answer: CallSdpField {
                kind: "answer".into(),
                sdp: "v=0\r\n".into(),
            },
        })
    }

    fn mk_invite_event(call_id: &str) -> ParsedCallEvent {
        ParsedCallEvent::Invite(CallInvite {
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
        })
    }

    fn mk_hangup_event(call_id: &str) -> ParsedCallEvent {
        ParsedCallEvent::Hangup(CallHangup {
            call_id: call_id.into(),
            party_id: "them".into(),
            version: CALL_VERSION.into(),
            reason: Some("peer".into()),
        })
    }

    fn start_event() -> Event {
        Event::Start {
            peer_user_id: "@peer:example.org".into(),
            peer_device_id: None,
            our_user_id: "@us:example.org".into(),
            our_device_id: "OURDEV".into(),
            room_id: "!r:example.org".into(),
            session_uuid: None,
        }
    }

    fn is_ok(result: &TransitionResult) -> bool {
        matches!(result, TransitionResult::Ok { .. })
    }

    fn assert_goes_to(result: TransitionResult, expected_name: &str) {
        match result {
            TransitionResult::Ok { next, .. } => {
                assert_eq!(
                    next.name(),
                    expected_name,
                    "expected transition to {}, got {}",
                    expected_name,
                    next.name()
                );
            }
            TransitionResult::Illegal { note } => {
                panic!("expected Ok({expected_name}), got Illegal({note})");
            }
        }
    }

    fn assert_stays(state: &P2PState, event: Event) {
        let result = transition(state, event);
        match result {
            TransitionResult::Ok { next, .. } => {
                assert_eq!(
                    next.name(),
                    state.name(),
                    "expected to stay in {}, got {}",
                    state.name(),
                    next.name()
                );
            }
            TransitionResult::Illegal { .. } => {
                // Illegal also means "stay" — the driver logs and keeps
                // current state.
            }
        }
    }

    fn assert_illegal(state: &P2PState, event: Event) {
        let result = transition(state, event);
        assert!(
            matches!(result, TransitionResult::Illegal { .. }),
            "expected Illegal, got {:?}",
            result
        );
    }

    // --- Row 1-8: happy-path Idle → FetchingTurn → Inviting → Connecting
    //             → Verifying → Open ---

    #[test]
    fn row01_idle_start_goes_to_fetching_turn() {
        assert_goes_to(transition(&P2PState::Idle, start_event()), "FetchingTurn");
    }

    #[test]
    fn row02_fetching_turn_turn_refreshed_goes_to_inviting() {
        let state = P2PState::FetchingTurn {
            since: Instant::now(),
        };
        assert_goes_to(
            transition(
                &state,
                Event::TurnRefreshed {
                    servers: Vec::new(),
                },
            ),
            "Inviting",
        );
    }

    #[test]
    fn row03_inviting_answer_goes_to_connecting() {
        let state = P2PState::Inviting {
            call_id: "c1".into(),
            started: Instant::now(),
            our_offer: mk_offer(),
            our_party_id: "us".into(),
            lifetime_ms: 30_000,
        };
        let result = transition(&state, Event::CallEventReceived(mk_answer_event("c1")));
        assert_goes_to(result, "Connecting");
    }

    #[test]
    fn row04_connecting_channel_open_goes_to_verifying() {
        let state = P2PState::Connecting {
            call_id: "c1".into(),
            our_party_id: "us".into(),
            ice_started: Instant::now(),
        };
        assert_goes_to(
            transition(&state, Event::ChannelEvent(BackendEvent::Open)),
            "Verifying",
        );
    }

    #[test]
    fn row05_verifying_verify_ok_goes_to_open() {
        let state = P2PState::Verifying {
            call_id: "c1".into(),
            our_party_id: "us".into(),
            our_nonce: [0u8; 32],
        };
        assert_goes_to(transition(&state, Event::VerifyOk), "Open");
    }

    #[test]
    fn row06_open_outbound_pressure_stays_open_with_reset_idle() {
        let state = P2PState::Open {
            call_id: "c1".into(),
            last_io: Instant::now(),
        };
        let result = transition(&state, Event::OutboundPressure);
        assert_goes_to(result.clone(), "Open");
        // Assert ResetIdle command emitted.
        if let TransitionResult::Ok { commands, .. } = result {
            assert!(
                commands.iter().any(|c| matches!(c, Command::ResetIdle)),
                "expected ResetIdle in commands: {commands:?}"
            );
        }
    }

    #[test]
    fn row07_open_channel_message_stays_open_with_reset_idle() {
        let state = P2PState::Open {
            call_id: "c1".into(),
            last_io: Instant::now(),
        };
        let result = transition(
            &state,
            Event::ChannelEvent(BackendEvent::Message(bytes::Bytes::from_static(b"xyz"))),
        );
        assert_goes_to(result, "Open");
    }

    #[test]
    fn row08_open_idle_tick_goes_to_idle() {
        let state = P2PState::Open {
            call_id: "c1".into(),
            last_io: Instant::now(),
        };
        assert_goes_to(transition(&state, Event::IdleTick), "Idle");
    }

    // --- Row 9-12: inbound invite, Answering path ---

    #[test]
    fn row09_idle_peer_invite_goes_to_answering() {
        assert_goes_to(
            transition(
                &P2PState::Idle,
                Event::CallEventReceived(mk_invite_event("peer-call")),
            ),
            "Answering",
        );
    }

    #[test]
    fn row10_answering_channel_open_goes_to_verifying() {
        let state = P2PState::Answering {
            call_id: "c1".into(),
            party_id: "us".into(),
            their_party_id: "them".into(),
        };
        assert_goes_to(
            transition(&state, Event::ChannelEvent(BackendEvent::Open)),
            "Verifying",
        );
    }

    #[test]
    fn row11_fetching_turn_peer_invite_preempts_and_goes_to_answering() {
        let state = P2PState::FetchingTurn {
            since: Instant::now(),
        };
        assert_goes_to(
            transition(
                &state,
                Event::CallEventReceived(mk_invite_event("peer-call")),
            ),
            "Answering",
        );
    }

    #[test]
    fn row12_inviting_concurrent_invite_goes_to_glare() {
        let state = P2PState::Inviting {
            call_id: "our-call".into(),
            started: Instant::now(),
            our_offer: mk_offer(),
            our_party_id: "us".into(),
            lifetime_ms: 30_000,
        };
        assert_goes_to(
            transition(
                &state,
                Event::CallEventReceived(mk_invite_event("their-call")),
            ),
            "Glare",
        );
    }

    // --- Row 13-17: failure paths ---

    #[test]
    fn row13_inviting_invite_timeout_goes_to_failed() {
        let state = P2PState::Inviting {
            call_id: "c1".into(),
            started: Instant::now(),
            our_offer: mk_offer(),
            our_party_id: "us".into(),
            lifetime_ms: 30_000,
        };
        assert_goes_to(transition(&state, Event::InviteTimeout), "Failed");
    }

    #[test]
    fn row14_connecting_ice_timeout_goes_to_failed() {
        let state = P2PState::Connecting {
            call_id: "c1".into(),
            our_party_id: "us".into(),
            ice_started: Instant::now(),
        };
        assert_goes_to(transition(&state, Event::IceTimeout), "Failed");
    }

    #[test]
    fn row15_verifying_verify_fail_signature_goes_to_failed() {
        let state = P2PState::Verifying {
            call_id: "c1".into(),
            our_party_id: "us".into(),
            our_nonce: [0u8; 32],
        };
        let result = transition(
            &state,
            Event::VerifyFail {
                reason: VerifyFailureReason::SignatureMismatch,
            },
        );
        assert_goes_to(result.clone(), "Failed");
        // Assert a security-event telemetry command with VerifyFailure.
        if let TransitionResult::Ok { commands, .. } = result {
            assert!(commands.iter().any(|c| matches!(
                c,
                Command::EmitTelemetry(TelemetryKind::SecurityEvent {
                    kind: SecurityEventKind::VerifyFailure,
                    ..
                })
            )));
        }
    }

    #[test]
    fn row16_verifying_verify_fail_replay_emits_replay_security_event() {
        let state = P2PState::Verifying {
            call_id: "c1".into(),
            our_party_id: "us".into(),
            our_nonce: [0u8; 32],
        };
        let result = transition(
            &state,
            Event::VerifyFail {
                reason: VerifyFailureReason::ReplayDetected,
            },
        );
        if let TransitionResult::Ok { commands, .. } = result {
            assert!(commands.iter().any(|c| matches!(
                c,
                Command::EmitTelemetry(TelemetryKind::SecurityEvent {
                    kind: SecurityEventKind::ReplayDetected,
                    ..
                })
            )));
        } else {
            panic!("expected Ok");
        }
    }

    #[test]
    fn row17_verifying_device_lockout_emits_wrong_peer() {
        let state = P2PState::Verifying {
            call_id: "c1".into(),
            our_party_id: "us".into(),
            our_nonce: [0u8; 32],
        };
        let result = transition(
            &state,
            Event::VerifyFail {
                reason: VerifyFailureReason::DeviceLockedOut,
            },
        );
        if let TransitionResult::Ok { commands, .. } = result {
            assert!(commands.iter().any(|c| matches!(
                c,
                Command::EmitTelemetry(TelemetryKind::SecurityEvent {
                    kind: SecurityEventKind::WrongPeer,
                    ..
                })
            )));
        } else {
            panic!("expected Ok");
        }
    }

    // --- Row 18-22: recoveries, hangup, turn_expired ---

    #[test]
    fn row18_open_turn_expired_goes_to_failed() {
        let state = P2PState::Open {
            call_id: "c1".into(),
            last_io: Instant::now(),
        };
        assert_goes_to(transition(&state, Event::TurnExpired), "Failed");
    }

    #[test]
    fn row19_open_decrypt_storm_emits_security_event_and_goes_to_failed() {
        let state = P2PState::Open {
            call_id: "c1".into(),
            last_io: Instant::now(),
        };
        let result = transition(&state, Event::DecryptStorm);
        assert_goes_to(result.clone(), "Failed");
        if let TransitionResult::Ok { commands, .. } = result {
            assert!(commands.iter().any(|c| matches!(
                c,
                Command::EmitTelemetry(TelemetryKind::SecurityEvent {
                    kind: SecurityEventKind::DecryptStorm,
                    ..
                })
            )));
        }
    }

    #[test]
    fn row20_failed_start_goes_to_fetching_turn() {
        let state = P2PState::Failed {
            reason: "test".into(),
            retry_after: Instant::now() - Duration::from_secs(1),
        };
        assert_goes_to(transition(&state, start_event()), "FetchingTurn");
    }

    #[test]
    fn row21_failed_retry_ready_after_time_goes_to_idle() {
        let state = P2PState::Failed {
            reason: "test".into(),
            retry_after: Instant::now() - Duration::from_secs(1),
        };
        assert_goes_to(transition(&state, Event::RetryReady), "Idle");
    }

    #[test]
    fn row22_failed_retry_ready_before_time_stays_failed() {
        let state = P2PState::Failed {
            reason: "test".into(),
            retry_after: Instant::now() + Duration::from_secs(30),
        };
        assert_stays(&state, Event::RetryReady);
    }

    // --- Row 23-26: hangup behavior ---

    #[test]
    fn row23_hangup_from_open_goes_to_idle() {
        let state = P2PState::Open {
            call_id: "c1".into(),
            last_io: Instant::now(),
        };
        assert_goes_to(
            transition(
                &state,
                Event::Hangup {
                    reason: "user".into(),
                },
            ),
            "Idle",
        );
    }

    #[test]
    fn row24_hangup_from_inviting_goes_to_idle() {
        let state = P2PState::Inviting {
            call_id: "c1".into(),
            started: Instant::now(),
            our_offer: mk_offer(),
            our_party_id: "us".into(),
            lifetime_ms: 30_000,
        };
        assert_goes_to(
            transition(
                &state,
                Event::Hangup {
                    reason: "user".into(),
                },
            ),
            "Idle",
        );
    }

    #[test]
    fn row25_peer_hangup_while_open_goes_to_idle() {
        let state = P2PState::Open {
            call_id: "c1".into(),
            last_io: Instant::now(),
        };
        assert_goes_to(
            transition(&state, Event::CallEventReceived(mk_hangup_event("c1"))),
            "Idle",
        );
    }

    #[test]
    fn row26_peer_hangup_while_connecting_goes_to_failed() {
        let state = P2PState::Connecting {
            call_id: "c1".into(),
            our_party_id: "us".into(),
            ice_started: Instant::now(),
        };
        assert_goes_to(
            transition(&state, Event::CallEventReceived(mk_hangup_event("c1"))),
            "Failed",
        );
    }

    // --- Row 27-32: illegal transitions (must not panic) ---

    #[test]
    fn row27_double_start_while_fetching_turn_illegal() {
        let state = P2PState::FetchingTurn {
            since: Instant::now(),
        };
        assert_illegal(&state, start_event());
    }

    #[test]
    fn row28_double_start_while_inviting_illegal() {
        let state = P2PState::Inviting {
            call_id: "c1".into(),
            started: Instant::now(),
            our_offer: mk_offer(),
            our_party_id: "us".into(),
            lifetime_ms: 30_000,
        };
        assert_illegal(&state, start_event());
    }

    #[test]
    fn row29_double_start_while_open_illegal() {
        let state = P2PState::Open {
            call_id: "c1".into(),
            last_io: Instant::now(),
        };
        assert_illegal(&state, start_event());
    }

    #[test]
    fn row30_verify_ok_while_inviting_illegal() {
        let state = P2PState::Inviting {
            call_id: "c1".into(),
            started: Instant::now(),
            our_offer: mk_offer(),
            our_party_id: "us".into(),
            lifetime_ms: 30_000,
        };
        assert_illegal(&state, Event::VerifyOk);
    }

    #[test]
    fn row31_ice_timeout_while_open_illegal() {
        let state = P2PState::Open {
            call_id: "c1".into(),
            last_io: Instant::now(),
        };
        assert_illegal(&state, Event::IceTimeout);
    }

    #[test]
    fn row32_invite_timeout_while_verifying_illegal() {
        let state = P2PState::Verifying {
            call_id: "c1".into(),
            our_party_id: "us".into(),
            our_nonce: [0u8; 32],
        };
        assert_illegal(&state, Event::InviteTimeout);
    }

    #[test]
    fn row33_verify_ok_while_idle_illegal_or_stay() {
        // Stay is also acceptable here — driver logs and ignores.
        // Our design choice: stay (no-op).
        assert_stays(&P2PState::Idle, Event::VerifyOk);
    }

    #[test]
    fn row34_random_channel_event_while_idle_stays() {
        assert_stays(&P2PState::Idle, Event::ChannelEvent(BackendEvent::Open));
    }

    #[test]
    fn row35_outbound_pressure_while_inviting_stays() {
        let state = P2PState::Inviting {
            call_id: "c1".into(),
            started: Instant::now(),
            our_offer: mk_offer(),
            our_party_id: "us".into(),
            lifetime_ms: 30_000,
        };
        assert_stays(&state, Event::OutboundPressure);
    }

    // --- Bonus sanity checks ---

    #[test]
    fn hangup_from_idle_is_ok_no_ops() {
        // Hangup from Idle is a no-op — emits state_transition telemetry
        // but no send/teardown (no active call).
        let result = transition(
            &P2PState::Idle,
            Event::Hangup {
                reason: "test".into(),
            },
        );
        assert!(is_ok(&result));
        if let TransitionResult::Ok { commands, .. } = result {
            assert!(!commands
                .iter()
                .any(|c| matches!(c, Command::SendHangup { .. })));
        }
    }

    #[test]
    fn state_name_matches_variant() {
        assert_eq!(P2PState::Idle.name(), "Idle");
        assert_eq!(
            P2PState::Open {
                call_id: "c".into(),
                last_io: Instant::now()
            }
            .name(),
            "Open"
        );
    }

    #[test]
    fn is_open_only_true_for_open_variant() {
        assert!(P2PState::Open {
            call_id: "c".into(),
            last_io: Instant::now()
        }
        .is_open());
        assert!(!P2PState::Idle.is_open());
        assert!(!P2PState::Failed {
            reason: "x".into(),
            retry_after: Instant::now()
        }
        .is_open());
    }

    #[test]
    fn transition_never_panics_on_random_event_state_pairs() {
        let states = [
            P2PState::Idle,
            P2PState::FetchingTurn {
                since: Instant::now(),
            },
            P2PState::Inviting {
                call_id: "c".into(),
                started: Instant::now(),
                our_offer: mk_offer(),
                our_party_id: "us".into(),
                lifetime_ms: 30_000,
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
            Event::TurnExpired,
            Event::InviteTimeout,
            Event::IceTimeout,
            Event::DecryptStorm,
            Event::RetryReady,
        ];
        for s in &states {
            for e in &events {
                // Just exercise — the assertion is "does not panic".
                let _ = transition(s, e.clone());
            }
        }
    }
}
