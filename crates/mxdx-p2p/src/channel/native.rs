//! Native [`WebRtcChannel`] implementation backed by `datachannel-rs` 0.16
//! (FFI to libdatachannel).
//!
//! ## Design
//!
//! libdatachannel exposes a callback-driven API. Every callback fires from
//! a libdatachannel internal thread and must be `Send + 'static`. To
//! bridge that surface to our `async fn` trait we:
//!
//! 1. Keep an mpsc [`EventSender`] inside each handler. Handlers push
//!    [`ChannelEvent`] values via `try_send` — **non-blocking**, so an
//!    overfull event queue drops the event rather than blocking the C
//!    thread. Storm §4.4 expects the consumer (Phase 5) to surface
//!    overflow as `FallbackToMatrix`.
//!
//! 2. Use a `tokio::sync::oneshot` to surface the first `on_description`
//!    callback into the async `create_offer` / `accept_offer` methods.
//!    libdatachannel auto-generates the local description synchronously
//!    on `create_data_channel` / `set_remote_description` when the
//!    default auto-negotiation is on; the oneshot resolves almost
//!    immediately.
//!
//! 3. Buffer inbound ICE candidates received before the remote description
//!    is set (matches `packages/core/webrtc-channel-node.js` behaviour).
//!
//! ## Payload privacy
//!
//! The native channel is a byte pipe. `on_message` never formats or logs
//! the message bytes — only its length, and only at `log::debug!` level.
//! This preserves the cardinal rule: frames on the data channel are
//! already-encrypted (Megolm + AES-GCM) and must never appear in logs.
//!
//! ## restart_ice
//!
//! `datachannel-sys` 0.23 does not expose `rtcSetConfiguration`, so the
//! native impl returns [`ChannelError::RestartIceUnsupported`]. Phase 5's
//! state machine catches this and falls back to a full reconnect rather
//! than attempting a live ICE restart.

use std::sync::{Arc, Mutex};

use datachannel::{
    ConnectionState, DataChannelHandler, DataChannelInfo, IceState, PeerConnectionHandler,
    RtcConfig, RtcDataChannel, RtcPeerConnection, SdpType, SessionDescription,
};

use super::{
    event_channel, ChannelError, ChannelEvent, ChannelResult, EventReceiver, EventSender,
    IceCandidate, IceServer, Sdp, SdpKind, WebRtcChannel, MAX_INBOUND_FRAME_SIZE,
};

/// The label used for the single mxdx data channel. Matches
/// `packages/core/webrtc-channel-node.js:142` so libdatachannel peers and
/// node-datachannel peers negotiate the same label.
const DATA_CHANNEL_LABEL: &str = "mxdx-terminal";

// ---------------------------------------------------------------------
// Handlers — execute on libdatachannel C threads.
// ---------------------------------------------------------------------

/// Shared state held by the PC handler. Cloned into the data-channel
/// handler when a DC is opened.
struct PcShared {
    /// Sender for the public event mpsc. Cloned by both handlers.
    tx: EventSender,
    /// Held data channel box. Once a DC is open we must keep the Box alive
    /// (drop closes it). The DC is created by the offerer via
    /// `create_data_channel`; for the answerer it arrives via
    /// `on_data_channel`.
    dc: Mutex<Option<Box<RtcDataChannel<DcHandler>>>>,
    /// Oneshot used to surface the first `on_description` callback to
    /// `create_offer` / `accept_offer`. Set once then consumed. Wrapped in
    /// a `Mutex<Option<Sender>>` because oneshot::Sender is `!Clone` and
    /// the callback fires multiple times on re-negotiation.
    local_desc_tx: Mutex<Option<tokio::sync::oneshot::Sender<SessionDescription>>>,
}

impl PcShared {
    fn new(tx: EventSender, local_desc_tx: tokio::sync::oneshot::Sender<SessionDescription>) -> Self {
        Self {
            tx,
            dc: Mutex::new(None),
            local_desc_tx: Mutex::new(Some(local_desc_tx)),
        }
    }

    fn store_data_channel(&self, dc: Box<RtcDataChannel<DcHandler>>) {
        *self.dc.lock().expect("dc mutex poisoned") = Some(dc);
    }
}

struct PcHandler {
    shared: Arc<PcShared>,
}

impl PeerConnectionHandler for PcHandler {
    type DCH = DcHandler;

    fn data_channel_handler(&mut self, _info: DataChannelInfo) -> Self::DCH {
        DcHandler {
            tx: self.shared.tx.clone(),
        }
    }

    fn on_description(&mut self, sess_desc: SessionDescription) {
        // First description fires once auto-negotiation decides the local
        // SDP is ready. Feed the oneshot; subsequent re-negotiations are
        // ignored for now (Phase 5 will observe via the mpsc if needed).
        if let Some(sender) = self
            .shared
            .local_desc_tx
            .lock()
            .expect("local_desc_tx mutex poisoned")
            .take()
        {
            // Receiver may have been dropped if the caller timed out —
            // that's fine, nothing to do.
            let _ = sender.send(sess_desc);
        }
    }

    fn on_candidate(&mut self, cand: datachannel::IceCandidate) {
        let ice = IceCandidate {
            candidate: cand.candidate,
            sdp_mid: Some(cand.mid),
            // libdatachannel FFI does not expose the m-line index.
            // Matrix VoIP consumers tolerate a missing mline_index when
            // sdp_mid is present (see `packages/core/webrtc-channel-node
            // .js:62` which only forwards `candidate` + `sdpMid`).
            sdp_mline_index: None,
        };
        // Non-blocking send — backpressure drops the event. See storm §4.4.
        let _ = self.shared.tx.try_send(ChannelEvent::LocalIce(ice));
    }

    fn on_connection_state_change(&mut self, state: ConnectionState) {
        match state {
            ConnectionState::Failed => {
                let _ = self
                    .shared
                    .tx
                    .try_send(ChannelEvent::Failure(format!("connection state: {:?}", state)));
            }
            ConnectionState::Closed | ConnectionState::Disconnected => {
                let _ = self.shared.tx.try_send(ChannelEvent::Closed {
                    reason: format!("connection state: {:?}", state),
                });
            }
            _ => { /* New / Connecting / Connected surface via DC open or LocalIce */ }
        }
    }

    fn on_ice_state_change(&mut self, state: IceState) {
        if matches!(state, IceState::Failed) {
            let _ = self
                .shared
                .tx
                .try_send(ChannelEvent::Failure("ICE state: Failed".into()));
        }
    }

    fn on_data_channel(&mut self, data_channel: Box<RtcDataChannel<DcHandler>>) {
        // Answerer path — libdatachannel hands us a new DC.
        self.shared.store_data_channel(data_channel);
    }
}

struct DcHandler {
    tx: EventSender,
}

impl DataChannelHandler for DcHandler {
    fn on_open(&mut self) {
        let _ = self.tx.try_send(ChannelEvent::Open);
    }

    fn on_closed(&mut self) {
        let _ = self.tx.try_send(ChannelEvent::Closed {
            reason: "remote closed data channel".into(),
        });
    }

    fn on_error(&mut self, err: &str) {
        // `err` is the libdatachannel-provided error string; it does not
        // contain user payload bytes but is still only metadata-safe.
        let _ = self
            .tx
            .try_send(ChannelEvent::Failure(format!("data channel error: {err}")));
    }

    fn on_message(&mut self, msg: &[u8]) {
        // Enforce storm §4.4 inbound frame cap.
        if msg.len() > MAX_INBOUND_FRAME_SIZE {
            let _ = self.tx.try_send(ChannelEvent::Failure(format!(
                "inbound frame exceeds {MAX_INBOUND_FRAME_SIZE}-byte cap ({} bytes)",
                msg.len()
            )));
            return;
        }
        // Cardinal rule: do NOT log payload contents. Only size.
        log::debug!("native channel: inbound frame ({} bytes)", msg.len());
        let bytes = bytes::Bytes::copy_from_slice(msg);
        let _ = self.tx.try_send(ChannelEvent::Message(bytes));
    }
}

// ---------------------------------------------------------------------
// Channel façade
// ---------------------------------------------------------------------

/// Native `WebRtcChannel` backed by libdatachannel.
pub struct NativeWebRtcChannel {
    pc: Option<Box<RtcPeerConnection<PcHandler>>>,
    /// Inbound event queue drained via [`WebRtcChannel::events`].
    rx: EventReceiver,
    /// Shared state observable across callbacks + `async fn` methods.
    shared: Arc<PcShared>,
    /// Receiver for the first local description — consumed by
    /// `create_offer` / `accept_offer`.
    local_desc_rx: Option<tokio::sync::oneshot::Receiver<SessionDescription>>,
    /// Candidates received before `set_remote_description` is called —
    /// drained once the remote description is in place.
    pending_candidates: Vec<IceCandidate>,
    remote_desc_set: bool,
    closed: bool,
}

impl NativeWebRtcChannel {
    /// Construct a new channel. Does NOT start ICE gathering — that
    /// happens on `create_offer` / `accept_offer` when ICE servers are
    /// known.
    pub fn new() -> Self {
        let (tx, rx) = event_channel();
        let (local_desc_tx, local_desc_rx) = tokio::sync::oneshot::channel();
        let shared = Arc::new(PcShared::new(tx, local_desc_tx));
        Self {
            pc: None,
            rx,
            shared,
            local_desc_rx: Some(local_desc_rx),
            pending_candidates: Vec::new(),
            remote_desc_set: false,
            closed: false,
        }
    }

    /// Build the libdatachannel `RtcConfig` from our `IceServer` list.
    /// libdatachannel expects URL strings like `stun:host:port` or
    /// `turn:user:pass@host:port[?transport=udp|tcp]`. Matches the
    /// conversion in `packages/core/webrtc-channel-node.js:34-55`.
    fn build_config(servers: &[IceServer]) -> RtcConfig {
        let urls: Vec<String> = servers
            .iter()
            .flat_map(|s| {
                s.urls.iter().filter_map(move |u| {
                    // Only stun:/turn:/turns: schemes are supported. Skip
                    // anything else silently rather than failing — matches
                    // the npm impl (line 43 `if (!match) continue`).
                    if !u.starts_with("stun:") && !u.starts_with("turn:") && !u.starts_with("turns:") {
                        return None;
                    }
                    if u.starts_with("stun:") {
                        return Some(u.clone());
                    }
                    // turn:/turns: may carry creds in-URL. If the caller
                    // provided username+credential separately, inject them.
                    match (&s.username, &s.credential) {
                        (Some(user), Some(pass)) if !u.contains('@') => {
                            // Split scheme:rest; rest is host:port[?query]
                            if let Some(rest) = u.split_once(':').map(|(_, rest)| rest) {
                                let scheme = u.split_once(':').map(|(s, _)| s).unwrap_or("turn");
                                Some(format!("{scheme}:{user}:{pass}@{rest}"))
                            } else {
                                Some(u.clone())
                            }
                        }
                        _ => Some(u.clone()),
                    }
                })
            })
            .collect();
        RtcConfig::new(&urls)
    }

    /// Ensure the peer connection exists. Builds it with the supplied
    /// ICE server list and wires the shared handler.
    fn ensure_pc(&mut self, servers: &[IceServer]) -> ChannelResult<()> {
        if self.pc.is_some() {
            return Ok(());
        }
        let config = Self::build_config(servers);
        let handler = PcHandler {
            shared: Arc::clone(&self.shared),
        };
        let pc = RtcPeerConnection::new(&config, handler)
            .map_err(|e| ChannelError::IceInitFailed(format!("{e}")))?;
        self.pc = Some(pc);
        Ok(())
    }

    fn drain_pending_candidates(&mut self) {
        let pending = std::mem::take(&mut self.pending_candidates);
        if pending.is_empty() {
            return;
        }
        let Some(pc) = self.pc.as_mut() else { return };
        for cand in pending {
            let raw = datachannel::IceCandidate {
                candidate: cand.candidate,
                mid: cand.sdp_mid.unwrap_or_default(),
            };
            if let Err(e) = pc.add_remote_candidate(&raw) {
                log::warn!("native channel: failed to add buffered candidate: {e}");
            }
        }
    }
}

impl Default for NativeWebRtcChannel {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for NativeWebRtcChannel {
    fn drop(&mut self) {
        // Explicitly drop the data channel before the peer connection so
        // libdatachannel tears them down in the right order.
        if let Ok(mut guard) = self.shared.dc.lock() {
            let _ = guard.take();
        }
        // pc drops automatically via `Option<Box<_>>`.
    }
}

// ---------------------------------------------------------------------
// Helpers: Sdp <-> SessionDescription conversion
// ---------------------------------------------------------------------

fn to_session_description(sdp: &Sdp) -> ChannelResult<SessionDescription> {
    let parsed = webrtc_sdp::parse_sdp(&sdp.sdp, false)
        .map_err(|e| ChannelError::InvalidSdp(format!("{e}")))?;
    let sdp_type = match sdp.kind {
        SdpKind::Offer => SdpType::Offer,
        SdpKind::Answer => SdpType::Answer,
    };
    Ok(SessionDescription {
        sdp: parsed,
        sdp_type,
    })
}

fn from_session_description(sd: SessionDescription) -> ChannelResult<Sdp> {
    let kind = match sd.sdp_type {
        SdpType::Offer => SdpKind::Offer,
        SdpType::Answer => SdpKind::Answer,
        other => {
            return Err(ChannelError::InvalidSdp(format!(
                "unsupported sdp type: {:?}",
                other
            )));
        }
    };
    Ok(Sdp {
        kind,
        sdp: sd.sdp.to_string(),
    })
}

// ---------------------------------------------------------------------
// webrtc_sdp re-export — the `datachannel` crate re-exports it at the
// crate root. Bring it into scope so helpers above compile.
// ---------------------------------------------------------------------

use datachannel::sdp as webrtc_sdp;

// ---------------------------------------------------------------------
// WebRtcChannel impl
// ---------------------------------------------------------------------

#[async_trait::async_trait]
impl WebRtcChannel for NativeWebRtcChannel {
    async fn create_offer(&mut self, ice_servers: &[IceServer]) -> ChannelResult<Sdp> {
        if self.closed {
            return Err(ChannelError::Closed);
        }
        self.ensure_pc(ice_servers)?;

        // Create the data channel. With auto-negotiation on (the default)
        // libdatachannel will fire `on_description` with the local offer.
        let pc = self.pc.as_mut().expect("pc exists after ensure_pc");
        let dc = pc
            .create_data_channel(
                DATA_CHANNEL_LABEL,
                DcHandler {
                    tx: self.shared.tx.clone(),
                },
            )
            .map_err(|e| ChannelError::IceInitFailed(format!("create_data_channel: {e}")))?;
        self.shared.store_data_channel(dc);

        // Wait for on_description to deliver the local offer. The
        // oneshot resolves as soon as libdatachannel generates the SDP,
        // which is synchronous w.r.t. the C side.
        let rx = self
            .local_desc_rx
            .take()
            .ok_or_else(|| ChannelError::Backend("create_offer called twice".into()))?;
        let sd = rx.await.map_err(|_| {
            ChannelError::IceInitFailed("local description channel closed before SDP arrived".into())
        })?;
        let sdp = from_session_description(sd)?;
        if sdp.kind != SdpKind::Offer {
            return Err(ChannelError::InvalidSdp(format!(
                "expected offer, got {:?}",
                sdp.kind
            )));
        }
        Ok(sdp)
    }

    async fn accept_offer(
        &mut self,
        ice_servers: &[IceServer],
        remote: Sdp,
    ) -> ChannelResult<Sdp> {
        if self.closed {
            return Err(ChannelError::Closed);
        }
        if remote.kind != SdpKind::Offer {
            return Err(ChannelError::InvalidSdp(format!(
                "expected offer, got {:?}",
                remote.kind
            )));
        }
        self.ensure_pc(ice_servers)?;

        let sd = to_session_description(&remote)?;
        {
            let pc = self.pc.as_mut().expect("pc exists");
            pc.set_remote_description(&sd)
                .map_err(|e| ChannelError::InvalidSdp(format!("set_remote_description: {e}")))?;
        }
        self.remote_desc_set = true;
        self.drain_pending_candidates();

        // Auto-negotiation fires on_description with the local answer.
        let rx = self
            .local_desc_rx
            .take()
            .ok_or_else(|| ChannelError::Backend("accept_offer called twice".into()))?;
        let sd = rx.await.map_err(|_| {
            ChannelError::IceInitFailed("local description channel closed before SDP arrived".into())
        })?;
        let sdp = from_session_description(sd)?;
        if sdp.kind != SdpKind::Answer {
            return Err(ChannelError::InvalidSdp(format!(
                "expected answer, got {:?}",
                sdp.kind
            )));
        }
        Ok(sdp)
    }

    async fn accept_answer(&mut self, remote: Sdp) -> ChannelResult<()> {
        if self.closed {
            return Err(ChannelError::Closed);
        }
        if remote.kind != SdpKind::Answer {
            return Err(ChannelError::InvalidSdp(format!(
                "expected answer, got {:?}",
                remote.kind
            )));
        }
        // Check PC existence before parsing SDP so "accept_answer before
        // create_offer" is surfaced as a clear backend error rather than
        // being shadowed by an SDP parse error on an empty string.
        if self.pc.is_none() {
            return Err(ChannelError::Backend(
                "accept_answer before create_offer".into(),
            ));
        }
        let sd = to_session_description(&remote)?;
        {
            let pc = self.pc.as_mut().expect("pc checked above");
            pc.set_remote_description(&sd)
                .map_err(|e| ChannelError::InvalidSdp(format!("set_remote_description: {e}")))?;
        }
        self.remote_desc_set = true;
        self.drain_pending_candidates();
        Ok(())
    }

    async fn add_ice_candidate(&mut self, c: IceCandidate) -> ChannelResult<()> {
        if self.closed {
            return Err(ChannelError::Closed);
        }
        if !self.remote_desc_set || self.pc.is_none() {
            // Buffer until remote description is set (matches npm
            // behaviour in webrtc-channel-node.js:164-171).
            self.pending_candidates.push(c);
            return Ok(());
        }
        let raw = datachannel::IceCandidate {
            candidate: c.candidate,
            mid: c.sdp_mid.unwrap_or_default(),
        };
        let pc = self.pc.as_mut().expect("pc exists when remote_desc_set");
        pc.add_remote_candidate(&raw)
            .map_err(|e| ChannelError::InvalidCandidate(format!("{e}")))?;
        Ok(())
    }

    async fn restart_ice(&mut self, _new_ice_servers: &[IceServer]) -> ChannelResult<Sdp> {
        // libdatachannel-sys 0.23 does not expose `rtcSetConfiguration`.
        // Phase 5 is expected to interpret this as "tear down and
        // re-invite". See storm §4.1 "mid-call TURN refresh".
        Err(ChannelError::RestartIceUnsupported)
    }

    async fn send(&self, frame: &[u8]) -> ChannelResult<()> {
        if self.closed {
            return Err(ChannelError::Closed);
        }
        let mut guard = self.shared.dc.lock().expect("dc mutex poisoned");
        let dc = guard
            .as_mut()
            .ok_or_else(|| ChannelError::Backend("data channel not yet open".into()))?;
        // Cardinal rule: frame is already-encrypted bytes. Never log
        // contents. Only size would be safe; we omit it to keep the send
        // path silent.
        dc.send(frame)
            .map_err(|e| ChannelError::Backend(format!("send: {e}")))?;
        Ok(())
    }

    fn events(&mut self) -> &mut EventReceiver {
        &mut self.rx
    }

    async fn close(&mut self, reason: &str) -> ChannelResult<()> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        // Drop the DC first so libdatachannel delivers the Closed event
        // to the peer before we drop the PC.
        if let Ok(mut guard) = self.shared.dc.lock() {
            let _ = guard.take();
        }
        // Dropping the peer connection releases TURN allocations and
        // closes the socket.
        self.pc = None;
        // Emit a Closed event to local consumers so the Phase 5 driver
        // can observe the local close decision. Reason is metadata only.
        let _ = self.shared.tx.try_send(ChannelEvent::Closed {
            reason: reason.to_string(),
        });
        Ok(())
    }
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_config_passes_through_stun() {
        let servers = vec![IceServer {
            urls: vec!["stun:stun.example:3478".into()],
            username: None,
            credential: None,
        }];
        let cfg = NativeWebRtcChannel::build_config(&servers);
        assert_eq!(cfg.ice_servers.len(), 1);
    }

    #[test]
    fn build_config_injects_turn_credentials() {
        let servers = vec![IceServer {
            urls: vec!["turn:turn.example:3478".into()],
            username: Some("alice".into()),
            credential: Some("s3cret".into()),
        }];
        let cfg = NativeWebRtcChannel::build_config(&servers);
        assert_eq!(cfg.ice_servers.len(), 1);
        let s = cfg.ice_servers[0].to_str().unwrap().to_string();
        assert!(s.contains("alice"), "url should contain username: {s}");
        assert!(s.contains("s3cret"), "url should contain credential: {s}");
    }

    #[test]
    fn build_config_skips_unsupported_schemes() {
        let servers = vec![IceServer {
            urls: vec!["http://not-a-real-stun.example".into()],
            username: None,
            credential: None,
        }];
        let cfg = NativeWebRtcChannel::build_config(&servers);
        assert_eq!(cfg.ice_servers.len(), 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn restart_ice_returns_unsupported() {
        let mut ch = NativeWebRtcChannel::new();
        let err = ch.restart_ice(&[]).await.unwrap_err();
        assert!(matches!(err, ChannelError::RestartIceUnsupported));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn add_ice_candidate_before_remote_desc_buffers() {
        let mut ch = NativeWebRtcChannel::new();
        let cand = IceCandidate {
            candidate: "candidate:1 1 udp 2130706431 127.0.0.1 9 typ host".into(),
            sdp_mid: Some("0".into()),
            sdp_mline_index: Some(0),
        };
        // No remote description yet → should buffer (not error).
        ch.add_ice_candidate(cand.clone()).await.unwrap();
        assert_eq!(ch.pending_candidates.len(), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn close_before_create_is_noop() {
        let mut ch = NativeWebRtcChannel::new();
        ch.close("test").await.unwrap();
        // Second close is also fine.
        ch.close("test again").await.unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn send_before_open_errors() {
        let ch = NativeWebRtcChannel::new();
        let err = ch.send(b"\x00").await.unwrap_err();
        assert!(matches!(err, ChannelError::Backend(_)), "got {err:?}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn send_after_close_errors_with_closed() {
        let mut ch = NativeWebRtcChannel::new();
        ch.close("done").await.unwrap();
        let err = ch.send(b"x").await.unwrap_err();
        assert!(matches!(err, ChannelError::Closed), "got {err:?}");
    }

    // ---------- Handler translation tests ----------
    //
    // These exercise the callback -> ChannelEvent translation without
    // spinning up a real libdatachannel PeerConnection. Each test drives
    // the handler trait directly from the test thread and asserts the
    // emitted ChannelEvent.

    fn drain_one(rx: &mut EventReceiver) -> Option<ChannelEvent> {
        rx.try_recv().ok()
    }

    #[test]
    fn dc_handler_on_open_emits_open() {
        let (tx, mut rx) = event_channel();
        let mut h = DcHandler { tx };
        h.on_open();
        match drain_one(&mut rx) {
            Some(ChannelEvent::Open) => {}
            other => panic!("expected Open, got {other:?}"),
        }
    }

    #[test]
    fn dc_handler_on_closed_emits_closed() {
        let (tx, mut rx) = event_channel();
        let mut h = DcHandler { tx };
        h.on_closed();
        match drain_one(&mut rx) {
            Some(ChannelEvent::Closed { reason }) => assert!(!reason.is_empty()),
            other => panic!("expected Closed, got {other:?}"),
        }
    }

    #[test]
    fn dc_handler_on_error_emits_failure() {
        let (tx, mut rx) = event_channel();
        let mut h = DcHandler { tx };
        h.on_error("boom");
        match drain_one(&mut rx) {
            Some(ChannelEvent::Failure(s)) => assert!(s.contains("boom")),
            other => panic!("expected Failure, got {other:?}"),
        }
    }

    #[test]
    fn dc_handler_on_message_emits_message_with_bytes() {
        let (tx, mut rx) = event_channel();
        let mut h = DcHandler { tx };
        let payload = b"\x00\x01\x02encrypted\xff";
        h.on_message(payload);
        match drain_one(&mut rx) {
            Some(ChannelEvent::Message(b)) => assert_eq!(b.as_ref(), payload),
            other => panic!("expected Message, got {other:?}"),
        }
    }

    #[test]
    fn dc_handler_on_message_rejects_oversized_frame() {
        let (tx, mut rx) = event_channel();
        let mut h = DcHandler { tx };
        let big = vec![0u8; MAX_INBOUND_FRAME_SIZE + 1];
        h.on_message(&big);
        match drain_one(&mut rx) {
            Some(ChannelEvent::Failure(s)) => {
                assert!(s.contains("exceeds"), "got failure: {s}");
            }
            other => panic!("expected Failure, got {other:?}"),
        }
    }

    #[test]
    fn pc_handler_on_candidate_emits_local_ice() {
        let (tx, mut rx) = event_channel();
        let (oneshot_tx, _oneshot_rx) = tokio::sync::oneshot::channel();
        let shared = Arc::new(PcShared::new(tx, oneshot_tx));
        let mut h = PcHandler {
            shared: Arc::clone(&shared),
        };
        h.on_candidate(datachannel::IceCandidate {
            candidate: "candidate:1 1 udp 2130706431 127.0.0.1 9 typ host".into(),
            mid: "0".into(),
        });
        match drain_one(&mut rx) {
            Some(ChannelEvent::LocalIce(c)) => {
                assert!(c.candidate.contains("127.0.0.1"));
                assert_eq!(c.sdp_mid.as_deref(), Some("0"));
            }
            other => panic!("expected LocalIce, got {other:?}"),
        }
    }

    #[test]
    fn pc_handler_on_connection_state_failed_emits_failure() {
        let (tx, mut rx) = event_channel();
        let (oneshot_tx, _oneshot_rx) = tokio::sync::oneshot::channel();
        let shared = Arc::new(PcShared::new(tx, oneshot_tx));
        let mut h = PcHandler { shared };
        h.on_connection_state_change(ConnectionState::Failed);
        match drain_one(&mut rx) {
            Some(ChannelEvent::Failure(_)) => {}
            other => panic!("expected Failure, got {other:?}"),
        }
    }

    #[test]
    fn pc_handler_on_connection_state_closed_emits_closed() {
        let (tx, mut rx) = event_channel();
        let (oneshot_tx, _oneshot_rx) = tokio::sync::oneshot::channel();
        let shared = Arc::new(PcShared::new(tx, oneshot_tx));
        let mut h = PcHandler { shared };
        h.on_connection_state_change(ConnectionState::Closed);
        match drain_one(&mut rx) {
            Some(ChannelEvent::Closed { .. }) => {}
            other => panic!("expected Closed, got {other:?}"),
        }
    }

    #[test]
    fn pc_handler_on_ice_state_failed_emits_failure() {
        let (tx, mut rx) = event_channel();
        let (oneshot_tx, _oneshot_rx) = tokio::sync::oneshot::channel();
        let shared = Arc::new(PcShared::new(tx, oneshot_tx));
        let mut h = PcHandler { shared };
        h.on_ice_state_change(IceState::Failed);
        match drain_one(&mut rx) {
            Some(ChannelEvent::Failure(s)) => assert!(s.contains("ICE")),
            other => panic!("expected Failure, got {other:?}"),
        }
    }

    #[test]
    fn pc_handler_on_description_feeds_oneshot() {
        let (tx, _rx) = event_channel();
        let (oneshot_tx, mut oneshot_rx) = tokio::sync::oneshot::channel();
        let shared = Arc::new(PcShared::new(tx, oneshot_tx));
        let mut h = PcHandler {
            shared: Arc::clone(&shared),
        };
        // Build a minimal parseable SDP for the callback.
        let sdp_str = "v=0\r\no=- 4294967296 2 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\nm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\nc=IN IP4 0.0.0.0\r\na=mid:0\r\na=setup:actpass\r\na=ice-ufrag:abcd\r\na=ice-pwd:1234567890abcdef1234\r\na=fingerprint:sha-256 00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00\r\na=sctp-port:5000\r\n";
        let parsed = webrtc_sdp::parse_sdp(sdp_str, false).expect("test SDP must parse");
        let sd = SessionDescription {
            sdp: parsed,
            sdp_type: SdpType::Offer,
        };
        h.on_description(sd);
        // Try to read the oneshot (non-blocking).
        let got = oneshot_rx.try_recv();
        assert!(got.is_ok(), "oneshot should have received the SDP");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn accept_answer_without_offer_errors() {
        let mut ch = NativeWebRtcChannel::new();
        let sdp = Sdp {
            kind: SdpKind::Answer,
            sdp: "v=0\r\n".into(),
        };
        let err = ch.accept_answer(sdp).await.unwrap_err();
        assert!(matches!(err, ChannelError::Backend(_)), "got {err:?}");
    }
}
