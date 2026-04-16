//! `P2PTransport` state machine — single implementation shared across native and wasm.
//!
//! Phase 5 implementation. See submodules:
//! - [`state`]: pure `P2PState` enum + `transition(state, event) -> TransitionResult`
//!   with all 9 states and side-effect-free dispatch (T-50).
//! - [`driver`]: tokio-select-based executor that applies returned
//!   [`state::Command`]s, owns the `WebRtcChannel` and `P2PCrypto`, and
//!   exposes the public `P2PTransport` API (T-51).
//! - [`idle`]: idle-timeout watchdog (T-52).
//! - [`verify`]: Ed25519-signed transcript handshake (T-53).

pub mod state;

pub use state::{
    transition, Command, Event, P2PState, ScheduledEvent, SecurityEventKind, TelemetryKind,
    TransitionResult, VerifyFailureReason,
};
