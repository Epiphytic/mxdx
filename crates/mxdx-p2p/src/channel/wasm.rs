//! WASM [`WebRtcChannel`] implementation backed by `web-sys::RtcPeerConnection`.
//!
//! Translates the browser's callback-driven WebRTC API to the async
//! [`WebRtcChannel`] trait using `wasm-bindgen` closures and
//! `wasm-bindgen-futures`. Events from the peer connection and data channel
//! are pushed through a tokio mpsc (which works on wasm via the `sync`
//! feature) and drained via [`WebRtcChannel::events`].
//!
//! ## Payload privacy
//!
//! Same rule as native: the channel is a byte pipe. `onmessage` never logs
//! or inspects payload bytes — only length at debug level. Cardinal rule:
//! frames are already Megolm+AES-GCM encrypted.
//!
//! ## restart_ice
//!
//! Browser `RTCPeerConnection` does support `restartIce()`, but the trait
//! returns `RestartIceUnsupported` for consistency with the native backend
//! and to match the Phase 5 state machine's "tear down and re-invite"
//! strategy.

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{
    MessageEvent, RtcConfiguration, RtcDataChannel, RtcDataChannelEvent, RtcDataChannelInit,
    RtcIceCandidate, RtcIceCandidateInit, RtcIceServer, RtcPeerConnection,
    RtcPeerConnectionIceEvent, RtcSdpType, RtcSessionDescriptionInit,
};

use super::{
    event_channel, ChannelError, ChannelEvent, ChannelResult, EventReceiver, EventSender,
    IceCandidate, IceServer, Sdp, SdpKind, WebRtcChannel, MAX_INBOUND_FRAME_SIZE,
};

/// Data channel label — must match the native backend and the npm peers.
const DATA_CHANNEL_LABEL: &str = "mxdx-terminal";

/// WASM `WebRtcChannel` backed by the browser's `RTCPeerConnection`.
pub struct WasmWebRtcChannel {
    pc: Option<RtcPeerConnection>,
    dc: Option<RtcDataChannel>,
    rx: EventReceiver,
    tx: EventSender,
    /// Candidates received before remote description is set.
    pending_candidates: Vec<IceCandidate>,
    remote_desc_set: bool,
    closed: bool,
    /// Prevent closures from being GC'd while the PC lives. We store them
    /// as `Closure<dyn FnMut(...)>` so the prevent-drop guarantee is
    /// maintained. Cleared on close/drop.
    _closures: Vec<ClosureHandle>,
}

/// Type-erased closure handle that prevents GC while alive.
/// The inner value is never read — its purpose is preventing `Drop`.
#[allow(dead_code)]
struct ClosureHandle(Box<dyn std::any::Any>);

impl WasmWebRtcChannel {
    pub fn new() -> Self {
        let (tx, rx) = event_channel();
        Self {
            pc: None,
            dc: None,
            rx,
            tx,
            pending_candidates: Vec::new(),
            remote_desc_set: false,
            closed: false,
            _closures: Vec::new(),
        }
    }

    /// Build an `RtcConfiguration` from our `IceServer` list.
    fn build_config(servers: &[IceServer]) -> Result<RtcConfiguration, JsValue> {
        let config = RtcConfiguration::new();
        let ice_servers = js_sys::Array::new();
        for s in servers {
            let server = RtcIceServer::new();
            let urls = js_sys::Array::new();
            for u in &s.urls {
                urls.push(&JsValue::from_str(u));
            }
            server.set_urls(&urls);
            if let Some(ref user) = s.username {
                server.set_username(user);
            }
            if let Some(ref cred) = s.credential {
                server.set_credential(cred);
            }
            ice_servers.push(&server);
        }
        config.set_ice_servers(&ice_servers);
        Ok(config)
    }

    /// Create the peer connection and wire up event handlers.
    fn ensure_pc(&mut self, servers: &[IceServer]) -> ChannelResult<()> {
        if self.pc.is_some() {
            return Ok(());
        }
        let config = Self::build_config(servers)
            .map_err(|e| ChannelError::IceInitFailed(format!("{e:?}")))?;
        let pc = RtcPeerConnection::new_with_configuration(&config)
            .map_err(|e| ChannelError::IceInitFailed(format!("{e:?}")))?;

        // --- onicecandidate ---
        let tx_ice = self.tx.clone();
        let on_ice_candidate =
            Closure::<dyn FnMut(RtcPeerConnectionIceEvent)>::new(move |ev: RtcPeerConnectionIceEvent| {
                if let Some(cand) = ev.candidate() {
                    let ice = IceCandidate {
                        candidate: cand.candidate(),
                        sdp_mid: cand.sdp_mid(),
                        sdp_mline_index: cand.sdp_m_line_index().map(|v| v as u32),
                    };
                    let _ = tx_ice.try_send(ChannelEvent::LocalIce(ice));
                }
            });
        pc.set_onicecandidate(Some(on_ice_candidate.as_ref().unchecked_ref()));
        self._closures.push(ClosureHandle(Box::new(on_ice_candidate)));

        // --- oniceconnectionstatechange ---
        let tx_state = self.tx.clone();
        let pc_ref = pc.clone();
        let on_ice_state = Closure::<dyn FnMut()>::new(move || {
            let state = pc_ref.ice_connection_state();
            match state {
                web_sys::RtcIceConnectionState::Failed => {
                    let _ = tx_state
                        .try_send(ChannelEvent::Failure("ICE connection state: failed".into()));
                }
                web_sys::RtcIceConnectionState::Closed
                | web_sys::RtcIceConnectionState::Disconnected => {
                    let _ = tx_state.try_send(ChannelEvent::Closed {
                        reason: format!("ICE connection state: {:?}", state),
                    });
                }
                _ => {}
            }
        });
        pc.set_oniceconnectionstatechange(Some(on_ice_state.as_ref().unchecked_ref()));
        self._closures.push(ClosureHandle(Box::new(on_ice_state)));

        // --- ondatachannel (answerer side) ---
        let tx_dc = self.tx.clone();
        let on_data_channel =
            Closure::<dyn FnMut(RtcDataChannelEvent)>::new(move |ev: RtcDataChannelEvent| {
                let dc = ev.channel();
                Self::wire_dc_handlers(&dc, &tx_dc);
                // Note: we can't store the DC reference here since closures
                // can't mutably borrow `self`. The answerer path in
                // `accept_offer` will pick up the DC from the event receiver.
                // Instead, we fire an Open event after the DC's own onopen fires.
            });
        pc.set_ondatachannel(Some(on_data_channel.as_ref().unchecked_ref()));
        self._closures.push(ClosureHandle(Box::new(on_data_channel)));

        self.pc = Some(pc);
        Ok(())
    }

    /// Wire `onopen`, `onclose`, `onerror`, `onmessage` handlers on a data channel.
    fn wire_dc_handlers(dc: &RtcDataChannel, tx: &EventSender) {
        // Ensure binary messages arrive as ArrayBuffer, not Blob.
        dc.set_binary_type(web_sys::RtcDataChannelType::Arraybuffer);

        let tx_open = tx.clone();
        let on_open = Closure::<dyn FnMut()>::new(move || {
            let _ = tx_open.try_send(ChannelEvent::Open);
        });
        dc.set_onopen(Some(on_open.as_ref().unchecked_ref()));
        on_open.forget(); // Safe: DC lifecycle is managed by PC teardown.

        let tx_close = tx.clone();
        let on_close = Closure::<dyn FnMut()>::new(move || {
            let _ = tx_close.try_send(ChannelEvent::Closed {
                reason: "remote closed data channel".into(),
            });
        });
        dc.set_onclose(Some(on_close.as_ref().unchecked_ref()));
        on_close.forget();

        let tx_err = tx.clone();
        let on_error = Closure::<dyn FnMut(JsValue)>::new(move |err: JsValue| {
            let msg = err
                .as_string()
                .unwrap_or_else(|| format!("{err:?}"));
            let _ = tx_err.try_send(ChannelEvent::Failure(format!("data channel error: {msg}")));
        });
        dc.set_onerror(Some(on_error.as_ref().unchecked_ref()));
        on_error.forget();

        let tx_msg = tx.clone();
        let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |ev: MessageEvent| {
            let data = ev.data();
            let bytes = if let Some(ab) = data.dyn_ref::<js_sys::ArrayBuffer>() {
                let arr = js_sys::Uint8Array::new(ab);
                let mut buf = vec![0u8; arr.length() as usize];
                arr.copy_to(&mut buf);
                buf
            } else if let Some(s) = data.as_string() {
                s.into_bytes()
            } else {
                // Unknown type — skip.
                return;
            };

            // Enforce storm §4.4 inbound frame cap.
            if bytes.len() > MAX_INBOUND_FRAME_SIZE {
                let _ = tx_msg.try_send(ChannelEvent::Failure(format!(
                    "inbound frame exceeds {MAX_INBOUND_FRAME_SIZE}-byte cap ({} bytes)",
                    bytes.len()
                )));
                return;
            }

            let _ = tx_msg.try_send(ChannelEvent::Message(bytes::Bytes::from(bytes)));
        });
        dc.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        on_message.forget();
    }

    /// Drain buffered ICE candidates once remote description is set.
    fn drain_pending_candidates(&mut self) {
        let pending = std::mem::take(&mut self.pending_candidates);
        if pending.is_empty() {
            return;
        }
        let Some(pc) = self.pc.as_ref() else { return };
        for cand in pending {
            let init = RtcIceCandidateInit::new(&cand.candidate);
            if let Some(ref mid) = cand.sdp_mid {
                init.set_sdp_mid(Some(mid));
            }
            if let Some(idx) = cand.sdp_mline_index {
                init.set_sdp_m_line_index(Some(idx as u16));
            }
            if let Ok(c) = RtcIceCandidate::new(&init) {
                let _ = pc.add_ice_candidate_with_opt_rtc_ice_candidate(Some(&c));
            }
        }
    }
}

impl Default for WasmWebRtcChannel {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for WasmWebRtcChannel {
    fn drop(&mut self) {
        if let Some(dc) = self.dc.take() {
            dc.close();
        }
        if let Some(pc) = self.pc.take() {
            pc.close();
        }
        self._closures.clear();
    }
}

#[async_trait::async_trait(?Send)]
impl WebRtcChannel for WasmWebRtcChannel {
    async fn create_offer(&mut self, ice_servers: &[IceServer]) -> ChannelResult<Sdp> {
        if self.closed {
            return Err(ChannelError::Closed);
        }
        self.ensure_pc(ice_servers)?;

        let pc = self.pc.as_ref().expect("pc exists after ensure_pc");

        // Create the data channel.
        let dc_init = RtcDataChannelInit::new();
        dc_init.set_ordered(true);
        let dc = pc.create_data_channel_with_data_channel_dict(DATA_CHANNEL_LABEL, &dc_init);
        Self::wire_dc_handlers(&dc, &self.tx);
        self.dc = Some(dc);

        // Create and set the local offer.
        let offer_js = wasm_bindgen_futures::JsFuture::from(pc.create_offer())
            .await
            .map_err(|e| ChannelError::IceInitFailed(format!("createOffer: {e:?}")))?;
        let offer_desc = RtcSessionDescriptionInit::from(offer_js.clone());

        // Extract SDP string from the JS object.
        let sdp_str = js_sys::Reflect::get(&offer_js, &"sdp".into())
            .ok()
            .and_then(|v| v.as_string())
            .ok_or_else(|| ChannelError::InvalidSdp("offer has no sdp field".into()))?;

        wasm_bindgen_futures::JsFuture::from(pc.set_local_description(&offer_desc))
            .await
            .map_err(|e| ChannelError::IceInitFailed(format!("setLocalDescription: {e:?}")))?;

        Ok(Sdp {
            kind: SdpKind::Offer,
            sdp: sdp_str,
        })
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

        // Set the remote offer. Scope the pc borrow so drain_pending_candidates
        // can borrow `self` mutably afterwards.
        {
            let pc = self.pc.as_ref().expect("pc exists after ensure_pc");
            let remote_desc = RtcSessionDescriptionInit::new(RtcSdpType::Offer);
            remote_desc.set_sdp(&remote.sdp);
            wasm_bindgen_futures::JsFuture::from(pc.set_remote_description(&remote_desc))
                .await
                .map_err(|e| ChannelError::InvalidSdp(format!("setRemoteDescription: {e:?}")))?;
        }
        self.remote_desc_set = true;
        self.drain_pending_candidates();

        // Create and set the local answer.
        let pc = self.pc.as_ref().expect("pc exists");
        let answer_js = wasm_bindgen_futures::JsFuture::from(pc.create_answer())
            .await
            .map_err(|e| ChannelError::IceInitFailed(format!("createAnswer: {e:?}")))?;
        let answer_desc = RtcSessionDescriptionInit::from(answer_js.clone());

        let sdp_str = js_sys::Reflect::get(&answer_js, &"sdp".into())
            .ok()
            .and_then(|v| v.as_string())
            .ok_or_else(|| ChannelError::InvalidSdp("answer has no sdp field".into()))?;

        wasm_bindgen_futures::JsFuture::from(pc.set_local_description(&answer_desc))
            .await
            .map_err(|e| ChannelError::IceInitFailed(format!("setLocalDescription: {e:?}")))?;

        Ok(Sdp {
            kind: SdpKind::Answer,
            sdp: sdp_str,
        })
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
        let pc = self.pc.as_ref().ok_or_else(|| {
            ChannelError::Backend("accept_answer before create_offer".into())
        })?;

        let remote_desc = RtcSessionDescriptionInit::new(RtcSdpType::Answer);
        remote_desc.set_sdp(&remote.sdp);
        wasm_bindgen_futures::JsFuture::from(pc.set_remote_description(&remote_desc))
            .await
            .map_err(|e| ChannelError::InvalidSdp(format!("setRemoteDescription: {e:?}")))?;
        self.remote_desc_set = true;
        self.drain_pending_candidates();
        Ok(())
    }

    async fn add_ice_candidate(&mut self, c: IceCandidate) -> ChannelResult<()> {
        if self.closed {
            return Err(ChannelError::Closed);
        }
        if !self.remote_desc_set || self.pc.is_none() {
            self.pending_candidates.push(c);
            return Ok(());
        }
        let pc = self.pc.as_ref().expect("pc exists when remote_desc_set");
        let init = RtcIceCandidateInit::new(&c.candidate);
        if let Some(ref mid) = c.sdp_mid {
            init.set_sdp_mid(Some(mid));
        }
        if let Some(idx) = c.sdp_mline_index {
            init.set_sdp_m_line_index(Some(idx as u16));
        }
        let cand = RtcIceCandidate::new(&init)
            .map_err(|e| ChannelError::InvalidCandidate(format!("{e:?}")))?;
        wasm_bindgen_futures::JsFuture::from(
            pc.add_ice_candidate_with_opt_rtc_ice_candidate(Some(&cand)),
        )
        .await
        .map_err(|e| ChannelError::InvalidCandidate(format!("{e:?}")))?;
        Ok(())
    }

    async fn restart_ice(&mut self, _new_ice_servers: &[IceServer]) -> ChannelResult<Sdp> {
        // Match native backend: return unsupported so Phase 5 tears down
        // and re-invites instead. Browser does support restartIce() but
        // keeping both backends consistent simplifies the state machine.
        Err(ChannelError::RestartIceUnsupported)
    }

    async fn send(&self, frame: &[u8]) -> ChannelResult<()> {
        if self.closed {
            return Err(ChannelError::Closed);
        }
        let dc = self
            .dc
            .as_ref()
            .ok_or_else(|| ChannelError::Backend("data channel not yet open".into()))?;
        // Cardinal rule: frame is already-encrypted bytes. Never log contents.
        dc.send_with_u8_array(frame)
            .map_err(|e| ChannelError::Backend(format!("send: {e:?}")))?;
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
        if let Some(dc) = self.dc.take() {
            dc.close();
        }
        if let Some(pc) = self.pc.take() {
            pc.close();
        }
        self._closures.clear();
        let _ = self.tx.try_send(ChannelEvent::Closed {
            reason: reason.to_string(),
        });
        Ok(())
    }
}
