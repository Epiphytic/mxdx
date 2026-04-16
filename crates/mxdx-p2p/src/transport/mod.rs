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

#[cfg(not(target_arch = "wasm32"))]
pub mod driver;

#[cfg(not(target_arch = "wasm32"))]
pub mod idle;

pub mod verify;

pub use state::{
    transition, Command, Event, P2PState, ScheduledEvent, SecurityEventKind, TelemetryKind,
    TransitionResult, VerifyFailureReason,
};

/// Outcome of a single `try_send` call. Non-blocking per storm §3.2 — the
/// caller learns immediately whether the payload was handed to the P2P
/// channel, must be routed through Matrix as fallback, or was dropped
/// because the channel is unrecoverably closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendOutcome {
    /// Payload was handed to the encrypted P2P channel.
    SentP2P,
    /// State is not `Open` (or the outbound queue is full). Caller should
    /// route through `MatrixClient::send_megolm`. This is the **common
    /// path** during connect and tear-down windows; not an error.
    FallbackToMatrix,
    /// The transport has been closed (`hangup()` or driver exited). Caller
    /// should stop trying P2P entirely.
    ChannelClosed,
}

/// Lightweight snapshot of the transport's current state — returned by
/// [`P2PTransport::state`] without awaiting. Used by callers (the worker
/// session_mux and client daemon) to drive telemetry and UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct P2PStateSnapshot {
    pub name: &'static str,
    pub is_open: bool,
}

impl P2PStateSnapshot {
    pub fn from(state: &P2PState) -> Self {
        Self {
            name: state.name(),
            is_open: state.is_open(),
        }
    }
}

// --------------------------------------------------------------------------
// Native P2PTransport API
// --------------------------------------------------------------------------

#[cfg(not(target_arch = "wasm32"))]
pub use driver::P2PTransport;

/// WASM stub — the real WASM driver lands in Phase 8 (storm §5.9 / §5.11).
/// For now, keep a stub that compiles but has no public surface on wasm so
/// `mxdx-core-wasm` does not accidentally re-export it before the Phase-8
/// web-sys WebRtcChannel impl exists.
#[cfg(target_arch = "wasm32")]
pub struct P2PTransport;

/// Marker type re-exported for the worker/client fallback path. Ensures
/// callers name the wrapper they hand to `MatrixClient::send_megolm` and
/// to [`P2PTransport::try_send`] consistently. Native-only — the wasm
/// driver (Phase 8) will introduce a wasm-compatible shim. See ADR
/// `2026-04-15-megolm-bytes-newtype.md`.
#[cfg(not(target_arch = "wasm32"))]
pub use mxdx_matrix::Megolm;

/// Outbound-queue depth limit (storm §4.4). Exposed so tests can exercise
/// the overflow → `FallbackToMatrix` path.
pub const OUTBOUND_QUEUE_DEPTH: usize = 256;

/// Inbound decrypt-failure threshold: more than this many failures in one
/// second triggers a channel tear-down (storm §4.4).
pub const DECRYPT_FAILURE_RATE_PER_SEC: u32 = 3;

/// Bounded inbound queue for decrypted `Megolm<Bytes>` payloads that the
/// driver surfaces to the caller via [`P2PTransport::incoming`]. Callers
/// MUST drain this — if the queue fills, the driver drops oldest frames
/// and emits a telemetry event.
pub const INBOUND_QUEUE_DEPTH: usize = 256;

/// Maximum decoded frame size. Enforced by the channel layer already; this
/// constant is public so tests and telemetry can reference the same limit.
pub use crate::channel::MAX_INBOUND_FRAME_SIZE;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::state::P2PState;
    use std::time::Instant;

    #[test]
    fn snapshot_captures_open_state() {
        let s = P2PState::Open {
            call_id: "c".into(),
            last_io: Instant::now(),
        };
        let snap = P2PStateSnapshot::from(&s);
        assert_eq!(snap.name, "Open");
        assert!(snap.is_open);
    }

    #[test]
    fn snapshot_captures_idle_state() {
        let snap = P2PStateSnapshot::from(&P2PState::Idle);
        assert_eq!(snap.name, "Idle");
        assert!(!snap.is_open);
    }

    #[test]
    fn send_outcome_variants_exist() {
        let _a = SendOutcome::SentP2P;
        let _b = SendOutcome::FallbackToMatrix;
        let _c = SendOutcome::ChannelClosed;
    }

    #[test]
    fn outbound_queue_depth_matches_storm_spec() {
        assert_eq!(OUTBOUND_QUEUE_DEPTH, 256);
    }

    // Compile-time assertion: `SendOutcome` is `Send + Copy + 'static`.
    #[test]
    fn send_outcome_is_trivial() {
        fn assert_copy<T: Copy + Send + 'static>() {}
        assert_copy::<SendOutcome>();
    }

}
