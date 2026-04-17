//! `mxdx-p2p` — platform-agnostic P2P transport for mxdx interactive sessions.
//!
//! This crate is the single home for:
//! - `WebRtcChannel` trait + cfg-gated native (`datachannel-rs`) and wasm (`web-sys`) impls
//! - `P2PTransport` state machine (shared across native and wasm)
//! - `P2PCrypto` (AES-256-GCM defense-in-depth layer over Megolm)
//! - `signaling` — Matrix VoIP `m.call.*` event de/serialization + glare resolver
//! - `turn` — `/_matrix/client/v3/voip/turnServer` client with active-call refresh
//!
//! Runtime wiring (SessionMux, BatchedSender configuration, session lifecycle, telemetry)
//! lives in `mxdx-worker` and `mxdx-client`. See ADR `2026-04-15-mxdx-p2p-crate.md`.

pub mod channel;
pub mod crypto;
pub mod signaling;
pub mod transport;
pub mod turn;
