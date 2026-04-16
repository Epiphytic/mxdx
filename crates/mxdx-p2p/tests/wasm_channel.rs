#![cfg(target_arch = "wasm32")]
//! WASM-specific channel surface tests (Phase 8, T-84).
//!
//! These tests run under `wasm-pack test --headless --firefox` and verify
//! that the WASM WebRtcChannel implementation compiles and constructs
//! correctly. Full WebRTC connectivity requires a real browser runtime
//! with STUN/TURN infrastructure — the Playwright federated smoke test
//! covers that.

use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

use mxdx_p2p::channel::{
    event_channel, ChannelEvent, IceServer, SdpKind, WasmWebRtcChannel, WebRtcChannel,
    EVENT_QUEUE_DEPTH, MAX_INBOUND_FRAME_SIZE,
};

#[wasm_bindgen_test]
fn wasm_channel_constructs() {
    let ch = WasmWebRtcChannel::new();
    drop(ch);
}

#[wasm_bindgen_test]
fn wasm_channel_default_trait() {
    let ch = WasmWebRtcChannel::default();
    drop(ch);
}

#[wasm_bindgen_test]
fn event_channel_depth_matches() {
    let (tx, _rx) = event_channel();
    assert_eq!(tx.capacity(), EVENT_QUEUE_DEPTH);
}

#[wasm_bindgen_test]
fn max_frame_size_is_1mb() {
    assert_eq!(MAX_INBOUND_FRAME_SIZE, 1024 * 1024);
}

#[wasm_bindgen_test]
async fn wasm_channel_create_offer_without_pc() {
    let mut ch = WasmWebRtcChannel::new();
    // Creating an offer with empty ICE servers should succeed
    // (browser RTCPeerConnection allows this for local-only connections).
    let result = ch.create_offer(&[]).await;
    assert!(result.is_ok(), "create_offer with empty ICE servers should succeed");
    let sdp = result.unwrap();
    assert_eq!(sdp.kind, SdpKind::Offer);
    assert!(!sdp.sdp.is_empty(), "SDP should not be empty");
}

#[wasm_bindgen_test]
async fn wasm_channel_close_is_idempotent() {
    let mut ch = WasmWebRtcChannel::new();
    ch.close("test close").await.unwrap();
    // Second close should also succeed (idempotent)
    ch.close("test close again").await.unwrap();
}

#[wasm_bindgen_test]
async fn wasm_channel_send_on_closed_errors() {
    let mut ch = WasmWebRtcChannel::new();
    ch.close("test").await.unwrap();
    let result = ch.send(b"hello").await;
    assert!(result.is_err());
}
