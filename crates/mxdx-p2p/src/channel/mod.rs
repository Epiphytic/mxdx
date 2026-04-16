//! `WebRtcChannel` trait and cfg-gated backend implementations.
//!
//! The trait is intentionally minimal — it exposes only the operations that
//! both libdatachannel (native) and the browser `RTCPeerConnection` (wasm)
//! can implement identically. Anything backend-specific is handled inside
//! the implementation and translated to the common [`ChannelEvent`] vocab.
//!
//! Cardinal rule: this trait is a **byte pipe**. Callers hand it already-
//! Megolm+AES-GCM-encrypted bytes; the channel itself never inspects
//! payload contents and never logs them. See ADR
//! `docs/adr/2026-04-15-datachannel-rs.md` and the storm spec §2.2.

use std::fmt;

#[cfg(not(target_arch = "wasm32"))]
pub mod native;

#[cfg(not(target_arch = "wasm32"))]
pub use native::NativeWebRtcChannel;

/// Maximum size (bytes) of an inbound data-channel frame. Frames larger than
/// this are dropped by the channel implementation with a
/// [`ChannelEvent::Failure`]. Matches the `TerminalSocket` bomb cap and
/// storm §4.4. Public so Phase 5's transport driver can surface the same
/// limit in its telemetry.
pub const MAX_INBOUND_FRAME_SIZE: usize = 1024 * 1024;

/// Outbound queue depth for the backend → consumer event mpsc. Storm §4.4
/// "outbound queue depth 256 frames → overflow returns `FallbackToMatrix`".
/// Used as the bounded-channel capacity; caller (Phase 5) interprets
/// overflow.
pub const EVENT_QUEUE_DEPTH: usize = 256;

/// A single STUN/TURN server. Converted by the native backend to the
/// libdatachannel URL-string form (`stun:host:port`,
/// `turn:user:pass@host:port`) and by the wasm backend to
/// `RTCIceServer` objects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IceServer {
    pub urls: Vec<String>,
    pub username: Option<String>,
    pub credential: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SdpKind {
    Offer,
    Answer,
}

impl fmt::Display for SdpKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Offer => "offer",
            Self::Answer => "answer",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sdp {
    pub kind: SdpKind,
    pub sdp: String,
}

/// A single ICE candidate in the standard `m.call.candidates` shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IceCandidate {
    pub candidate: String,
    pub sdp_mid: Option<String>,
    pub sdp_mline_index: Option<u32>,
}

/// Event emitted by a channel backend. Backends push these via an mpsc;
/// consumers drain with [`WebRtcChannel::events`].
#[derive(Debug)]
pub enum ChannelEvent {
    /// A local ICE candidate was gathered and should be sent to the peer.
    LocalIce(IceCandidate),
    /// An inbound frame arrived. Contents are opaque encrypted bytes — the
    /// channel never inspects or logs them.
    Message(bytes::Bytes),
    /// The data channel reached the open state on this side.
    Open,
    /// The data channel was closed locally or by the peer.
    Closed { reason: String },
    /// A non-recoverable backend error (libdatachannel `on_error`, ICE
    /// failure, etc.). The consumer should transition to `Failed`.
    Failure(String),
}

#[derive(Debug, thiserror::Error)]
pub enum ChannelError {
    #[error("invalid SDP: {0}")]
    InvalidSdp(String),
    #[error("invalid ICE candidate: {0}")]
    InvalidCandidate(String),
    #[error("ICE initialisation failed: {0}")]
    IceInitFailed(String),
    /// Backend does not support `restart_ice`. Documented limitation on
    /// libdatachannel 0.16 / libdatachannel-sys 0.23: the FFI does not
    /// expose `rtcSetConfiguration`. Phase 5's state machine should treat
    /// this as "tear down and fully re-invite" rather than a hard failure.
    #[error("restart_ice not supported by this backend")]
    RestartIceUnsupported,
    /// The frame exceeded [`MAX_INBOUND_FRAME_SIZE`] — defensive cap from
    /// storm §4.4. Raised on the receive path only.
    #[error("frame exceeds {max}-byte limit ({actual} bytes)")]
    FrameTooLarge { max: usize, actual: usize },
    #[error("channel is closed")]
    Closed,
    #[error("backend error: {0}")]
    Backend(String),
}

pub type ChannelResult<T> = std::result::Result<T, ChannelError>;

/// Platform-agnostic WebRTC data-channel transport.
///
/// The trait is cfg-gated with `?Send` for wasm because `web-sys` futures
/// are `!Send`; native futures are `Send`.
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait WebRtcChannel {
    /// Create a local offer. Configures ICE servers on the underlying peer
    /// connection before gathering begins.
    async fn create_offer(&mut self, ice_servers: &[IceServer]) -> ChannelResult<Sdp>;

    /// Accept an inbound offer and produce a local answer. Configures ICE
    /// servers before gathering begins.
    async fn accept_offer(
        &mut self,
        ice_servers: &[IceServer],
        remote: Sdp,
    ) -> ChannelResult<Sdp>;

    /// Install a remote answer (for the offerer side).
    async fn accept_answer(&mut self, remote: Sdp) -> ChannelResult<()>;

    /// Add a remote ICE candidate. If the remote description has not yet
    /// been set, implementations MAY buffer candidates until it is.
    async fn add_ice_candidate(&mut self, c: IceCandidate) -> ChannelResult<()>;

    /// Restart ICE with a new set of ICE servers (for mid-call TURN creds
    /// rotation). May return [`ChannelError::RestartIceUnsupported`] on
    /// backends that do not expose ICE restart — callers should treat this
    /// as "fall back to full reconnect".
    async fn restart_ice(&mut self, new_ice_servers: &[IceServer]) -> ChannelResult<Sdp>;

    /// Send an already-encrypted frame. The channel never inspects or logs
    /// payload bytes — this is a pure byte pipe.
    async fn send(&self, frame: &[u8]) -> ChannelResult<()>;

    /// Borrow the inbound event receiver. Backends emit ICE candidates,
    /// message arrivals, and lifecycle events here.
    fn events(&mut self) -> &mut EventReceiver;

    /// Close the channel with a reason string (logged only as metadata —
    /// not payload).
    async fn close(&mut self, reason: &str) -> ChannelResult<()>;
}

// -------------------------------------------------------------------------
// Event-channel type alias. Wasm uses the same tokio sync::mpsc which works
// via tokio's "sync" feature without requiring a runtime — receivers are
// pollable from wasm-bindgen-futures. Kept as a type alias so backends and
// tests can name the receiver without committing every crate consumer to
// the concrete tokio type directly.
// -------------------------------------------------------------------------

pub type EventSender = tokio::sync::mpsc::Sender<ChannelEvent>;
pub type EventReceiver = tokio::sync::mpsc::Receiver<ChannelEvent>;

/// Construct a bounded event mpsc pair with the default depth.
pub fn event_channel() -> (EventSender, EventReceiver) {
    tokio::sync::mpsc::channel(EVENT_QUEUE_DEPTH)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ice_server_is_clone_and_debug() {
        let s = IceServer {
            urls: vec!["stun:a.example:3478".into()],
            username: None,
            credential: None,
        };
        let _ = format!("{:?}", s);
        let _ = s.clone();
    }

    #[test]
    fn sdp_kind_display() {
        assert_eq!(SdpKind::Offer.to_string(), "offer");
        assert_eq!(SdpKind::Answer.to_string(), "answer");
    }

    #[test]
    fn event_channel_has_expected_depth() {
        // Sanity check: the pair compiles and obeys the configured depth.
        let (tx, _rx) = event_channel();
        assert_eq!(tx.capacity(), EVENT_QUEUE_DEPTH);
    }

    /// Compile-time assertion: `ChannelEvent` and the top-level types are
    /// `Send + 'static` on the native target (wasm target has `?Send`).
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn event_is_send_and_static_on_native() {
        fn assert_send<T: Send + 'static>() {}
        assert_send::<ChannelEvent>();
        assert_send::<Sdp>();
        assert_send::<IceCandidate>();
        assert_send::<IceServer>();
        assert_send::<ChannelError>();
    }

    #[test]
    fn frame_too_large_error_formats() {
        let err = ChannelError::FrameTooLarge {
            max: MAX_INBOUND_FRAME_SIZE,
            actual: MAX_INBOUND_FRAME_SIZE + 1,
        };
        // We only verify the error renders — not the exact text — so
        // future rewording doesn't silently break tests.
        let s = err.to_string();
        assert!(s.contains(&MAX_INBOUND_FRAME_SIZE.to_string()));
    }
}
