pub mod backup;
pub mod client;
pub mod crypto_envelope;
pub mod error;
pub mod multi_hs;
pub mod reencrypt;
pub mod rest;
pub mod rooms;
pub mod session;

pub use matrix_sdk;

/// The Matrix VoIP `m.call.*` event types recognized by the mxdx sync path.
///
/// Mirrors `mxdx_p2p::signaling::parse::CALL_EVENT_TYPES`; kept duplicated
/// to avoid a `mxdx-matrix → mxdx-p2p` dep (that would invert the normal
/// direction — `mxdx-p2p` depends on nothing Matrix-SDK-related, and the
/// npm/wasm targets for `mxdx-p2p` must not pull matrix-sdk transitively).
/// If you change this list, change the mxdx-p2p copy too — both should
/// reference the 2026-04-15 m.call wire-format ADR.
///
/// mxdx-matrix's `sync_and_collect_events` does not use an explicit
/// positive filter; it passes all event types through except a small
/// denylist (`m.room.encryption`, `m.room.member`, `m.room.power_levels`).
/// So `m.call.*` events flow to consumers by default. This constant is
/// exported so Phase 5's state-machine wiring has a canonical list.
pub const CALL_EVENT_TYPES: &[&str] = &[
    "m.call.invite",
    "m.call.answer",
    "m.call.candidates",
    "m.call.hangup",
    "m.call.select_answer",
];
pub use client::{default_store_base_path, short_hash, MatrixClient};
pub use crypto_envelope::{Bytes, Megolm};
pub use error::MatrixClientError;
pub use matrix_sdk::ruma::{OwnedRoomId, OwnedUserId, RoomId, UserId};
pub use multi_hs::{MultiHsClient, ServerAccount, ServerHealth, ServerStatus};
pub use rooms::LauncherTopology;
pub use session::{
    connect_with_session, normalize_server, password_key, session_key, store_key_key, SessionData,
};
