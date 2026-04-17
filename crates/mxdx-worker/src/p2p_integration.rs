//! P2P transport integration for the worker (T-60 Phase 6 wiring).
//!
//! This module encapsulates the optional per-session [`P2PTransport`] and
//! the Megolm-wrapped non-blocking send helper. It is constructed per
//! session only when `worker_config.p2p.enabled == true`. With the flag
//! off, construction is skipped entirely and every send goes through the
//! existing [`MatrixClient::send_megolm`] path (the pre-Phase-6 behavior).
//!
//! # Non-regression contract
//!
//! - `WorkerP2pSession::new_if_enabled(cfg) -> Option<Self>` returns
//!   `None` when the flag is off. Callers treat `None` as "no P2P; always
//!   go through Matrix". The on-path behavior with `None` is
//!   byte-for-byte identical to what the worker did before Phase 6.
//! - `try_send_via_p2p_or_matrix` is the single entry point that chooses
//!   P2P or Matrix. If the session is `None`, it immediately routes
//!   through Matrix. If `Some` but state != Open, it also routes through
//!   Matrix (via the non-blocking [`P2PTransport::try_send`] contract).
//!
//! # Cardinal rule (E2EE)
//!
//! The payload argument is `Megolm<Bytes>` (structurally un-constructible
//! outside `mxdx-matrix`). Callers MUST build it via
//! [`MatrixClient::encrypt_for_room`] first. The fallback path calls
//! [`MatrixClient::send_megolm`] with the SAME `Megolm<Bytes>` — the
//! receiver sees a Megolm-encrypted event either way.

use std::sync::Arc;

use mxdx_matrix::{Bytes, MatrixClient, Megolm, RoomId};
use mxdx_p2p::transport::driver::{P2PTransport, P2PTransportConfig};
use mxdx_p2p::transport::{P2PStateSnapshot, SendOutcome};
use mxdx_types::config::P2pConfig;

/// Per-session P2P wiring handle. Wraps an optional [`P2PTransport`] so
/// the caller can use the same non-blocking send surface regardless of
/// whether P2P is enabled.
pub struct WorkerP2pSession {
    /// `None` when `p2p.enabled == false`. `Some` when the feature flag is
    /// on (the default since Phase-9 T-91) and a transport has been constructed.
    transport: Option<P2PTransport>,
}

impl WorkerP2pSession {
    /// Construct a transport for this session iff `p2p_config.enabled`.
    /// Returns `None` otherwise — callers route all sends through Matrix.
    ///
    /// The `call_event_tx` returned with Some is the inbound-signaling
    /// sink: the caller hands it to the mxdx-matrix sync path so
    /// `m.call.*` events parsed out of the session room feed the driver.
    /// With `None`, the caller does not need a call-event tx at all.
    pub fn new_if_enabled(
        p2p_config: &P2pConfig,
        our_user_id: impl Into<String>,
        our_device_id: impl Into<String>,
        room_id: impl Into<String>,
    ) -> (
        Option<Self>,
        Option<tokio::sync::mpsc::Sender<mxdx_p2p::signaling::parse::ParsedCallEvent>>,
    ) {
        if !p2p_config.enabled {
            return (None, None);
        }
        let mut cfg =
            P2PTransportConfig::new(our_user_id, our_device_id, room_id);
        cfg.idle_window =
            std::time::Duration::from_secs(p2p_config.idle_timeout_seconds);
        let (transport, events_tx) = P2PTransport::spawn(cfg);
        (
            Some(Self {
                transport: Some(transport),
            }),
            Some(events_tx),
        )
    }

    /// Construction helper for tests — builds a handle with no transport.
    /// Matches the behavior of `new_if_enabled` when the flag is off.
    pub fn disabled() -> Self {
        Self { transport: None }
    }

    /// True iff a P2PTransport is actively spawned (flag is on).
    pub fn is_enabled(&self) -> bool {
        self.transport.is_some()
    }

    /// Cheap current-state snapshot. `None` if the transport is not
    /// constructed (flag off).
    pub fn state(&self) -> Option<P2PStateSnapshot> {
        self.transport.as_ref().map(|t| t.state())
    }

    /// Non-blocking send: if a transport exists AND is Open AND the
    /// outbound queue accepts, returns `SendOutcome::SentP2P`; otherwise
    /// (including `None` transport) returns `SendOutcome::FallbackToMatrix`.
    ///
    /// The caller is always responsible for honoring
    /// `FallbackToMatrix`/`ChannelClosed` by invoking
    /// [`MatrixClient::send_megolm`] with the same `Megolm<Bytes>`.
    pub fn try_send_p2p(
        &self,
        event_type: String,
        payload: Megolm<Bytes>,
    ) -> SendOutcome {
        match self.transport.as_ref() {
            None => SendOutcome::FallbackToMatrix,
            Some(t) => t.try_send(event_type, payload),
        }
    }
}

/// Single entry point for "send this Megolm payload — P2P if available,
/// else Matrix." This wraps the two-step pattern documented in storm §3.2
/// / §4.3 so callers don't forget the fallback.
///
/// Semantics:
/// - `p2p.is_none()` → Matrix only (existing worker path, no change).
/// - `p2p.is_some()` + state != Open → Matrix (non-blocking fallback).
/// - `p2p.is_some()` + state == Open → P2P, with Matrix as a safety net
///   if the outbound queue is full or the channel was just closed.
///
/// Returns the Matrix event_id on the Matrix path, or `None` if the P2P
/// path took the payload (the P2P path does not produce an event_id —
/// the receiver reconstructs the same event via the decrypted frame).
pub async fn try_send_via_p2p_or_matrix(
    p2p: Option<&WorkerP2pSession>,
    matrix_client: &MatrixClient,
    room_id: &RoomId,
    event_type: &str,
    payload: Megolm<Bytes>,
) -> Result<Option<String>, mxdx_matrix::MatrixClientError> {
    // With flag off OR no session, Matrix is the only path. This is the
    // pre-Phase-6 behavior and the default in production.
    let session = match p2p {
        None => return send_matrix(matrix_client, room_id, event_type, payload).await,
        Some(s) => s,
    };

    // Clone the Megolm wrapper so we can fall back to Matrix with the
    // same payload if P2P rejects the send. Duplicating the sealed bytes
    // does not create a new external constructor (Megolm's constructor is
    // still pub(crate)) — the clone is confined to this two-send-path
    // integration point.
    let fallback_copy = payload.clone();
    match session.try_send_p2p(event_type.to_string(), payload) {
        SendOutcome::SentP2P => {
            // P2P took it; no Matrix event_id to return. Receiver decrypts
            // the AES-GCM frame to recover the same Megolm bytes.
            Ok(None)
        }
        SendOutcome::FallbackToMatrix | SendOutcome::ChannelClosed => {
            // Non-blocking fallback — post via Matrix with the same payload.
            send_matrix(matrix_client, room_id, event_type, fallback_copy).await
        }
    }
}

/// T-61: apply the BatchedSender window flip rule for a given P2P state
/// snapshot. Returns the window the caller should pass to
/// [`BatchedSender::set_batch_window`]. Called by the worker event loop
/// after every `transport.state()` observation.
///
/// Storm §2.8: Open → 10ms, non-Open → 200ms.
///
/// When `snap` is `None` (flag off, no transport), returns
/// `DEFAULT_BATCH_WINDOW` (200ms) — the pre-Phase-6 behavior.
pub fn batch_window_for_p2p_state(
    snap: Option<&P2PStateSnapshot>,
) -> std::time::Duration {
    match snap {
        Some(s) if s.is_open => crate::batched_sender::P2P_OPEN_BATCH_WINDOW,
        _ => crate::batched_sender::DEFAULT_BATCH_WINDOW,
    }
}

async fn send_matrix(
    matrix_client: &MatrixClient,
    room_id: &RoomId,
    event_type: &str,
    payload: Megolm<Bytes>,
) -> Result<Option<String>, mxdx_matrix::MatrixClientError> {
    matrix_client
        .send_megolm(room_id, event_type, payload)
        .await
        .map(Some)
}

/// Top-level construction helper: given a loaded [`P2pConfig`], returns a
/// `WorkerP2pSession` if the flag is on, else `None`. Also returns the
/// optional `Arc<MatrixClient>` handle pre-wired into the integration —
/// matching the signer/peer-source design from Phase-5 mxdx-btk.
///
/// Phase-7 E2E tests exercise this constructor directly.
#[allow(clippy::too_many_arguments)]
pub fn build_p2p_session(
    p2p_config: &P2pConfig,
    matrix_client: Arc<MatrixClient>,
    our_user_id: impl Into<String>,
    our_device_id: impl Into<String>,
    room_id: impl Into<String>,
) -> WorkerP2pSessionBundle {
    let our_device_id = our_device_id.into();
    let (session, events_tx) = WorkerP2pSession::new_if_enabled(
        p2p_config,
        our_user_id,
        our_device_id.clone(),
        room_id,
    );

    let signer = if p2p_config.enabled {
        Some(Arc::new(
            mxdx_p2p::transport::matrix_signer::MatrixHandshakeSigner::new(
                matrix_client.clone(),
            ),
        ))
    } else {
        None
    };

    let peer_keys = if p2p_config.enabled {
        Some(Arc::new(
            mxdx_p2p::transport::matrix_signer::MatrixPeerKeySource::new(matrix_client),
        ))
    } else {
        None
    };

    WorkerP2pSessionBundle {
        session,
        signer,
        peer_keys,
        call_event_tx: events_tx,
    }
}

/// Bundle of per-session P2P integration handles. Holds the optional
/// transport session plus the Matrix-backed signer/peer-key source that
/// the driver will consult during the Verifying handshake.
pub struct WorkerP2pSessionBundle {
    pub session: Option<WorkerP2pSession>,
    pub signer: Option<Arc<mxdx_p2p::transport::matrix_signer::MatrixHandshakeSigner>>,
    pub peer_keys: Option<Arc<mxdx_p2p::transport::matrix_signer::MatrixPeerKeySource>>,
    pub call_event_tx:
        Option<tokio::sync::mpsc::Sender<mxdx_p2p::signaling::parse::ParsedCallEvent>>,
}

impl WorkerP2pSessionBundle {
    pub fn is_enabled(&self) -> bool {
        self.session.as_ref().map(|s| s.is_enabled()).unwrap_or(false)
    }
}

