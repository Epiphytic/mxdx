//! P2P transport integration for the client daemon (T-62 Phase 6 wiring).
//!
//! Symmetric to `mxdx-worker::p2p_integration` but adds the `--no-p2p`
//! CLI override: the flag in config.toml can be bypassed at runtime for
//! diagnostics. When `--no-p2p` is set, every `try_send` returns
//! `FallbackToMatrix`, even if `config.p2p.enabled = true`.
//!
//! # Non-regression contract (same as worker)
//!
//! - `ClientP2pSession::new(&cfg, no_p2p_cli) -> Option<Self>` returns
//!   `None` when `cfg.enabled == false` OR `no_p2p_cli == true`. Callers
//!   treat `None` as "route everything through Matrix".
//! - `try_send_via_p2p_or_matrix` is the single entry point. Non-blocking.
//!
//! # --no-p2p semantics (storm §4.7 / storm §2.9)
//!
//! Operators set `--no-p2p` during incident response to force Matrix-only
//! mode WITHOUT restarting the client or editing config. The flag has the
//! same observable effect as `config.p2p.enabled = false`: no transport
//! is spawned, no call-event sink allocated, all sends go through
//! `MatrixClient::send_megolm`.

use std::sync::Arc;

use mxdx_matrix::{Bytes, MatrixClient, Megolm, RoomId};
use mxdx_p2p::transport::driver::{P2PTransport, P2PTransportConfig};
use mxdx_p2p::transport::{P2PStateSnapshot, SendOutcome};
use mxdx_types::config::P2pConfig;

/// Per-session P2P wiring handle for the client daemon.
pub struct ClientP2pSession {
    transport: Option<P2PTransport>,
}

impl ClientP2pSession {
    /// Construct a transport if `cfg.enabled && !no_p2p_cli`. Returns
    /// `None` otherwise — callers route all sends through Matrix.
    pub fn new_if_enabled(
        cfg: &P2pConfig,
        no_p2p_cli: bool,
        our_user_id: impl Into<String>,
        our_device_id: impl Into<String>,
        room_id: impl Into<String>,
    ) -> (
        Option<Self>,
        Option<tokio::sync::mpsc::Sender<mxdx_p2p::signaling::parse::ParsedCallEvent>>,
    ) {
        if !cfg.enabled || no_p2p_cli {
            return (None, None);
        }
        let mut transport_cfg =
            P2PTransportConfig::new(our_user_id, our_device_id, room_id);
        transport_cfg.idle_window =
            std::time::Duration::from_secs(cfg.idle_timeout_seconds);
        let (transport, events_tx) = P2PTransport::spawn(transport_cfg);
        (
            Some(Self {
                transport: Some(transport),
            }),
            Some(events_tx),
        )
    }

    pub fn disabled() -> Self {
        Self { transport: None }
    }

    pub fn is_enabled(&self) -> bool {
        self.transport.is_some()
    }

    pub fn state(&self) -> Option<P2PStateSnapshot> {
        self.transport.as_ref().map(|t| t.state())
    }

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
/// else Matrix." Non-blocking; returns `Some(event_id)` on the Matrix
/// path, `None` if P2P took the payload.
pub async fn try_send_via_p2p_or_matrix(
    p2p: Option<&ClientP2pSession>,
    matrix_client: &MatrixClient,
    room_id: &RoomId,
    event_type: &str,
    payload: Megolm<Bytes>,
) -> Result<Option<String>, mxdx_matrix::MatrixClientError> {
    let session = match p2p {
        None => return send_matrix(matrix_client, room_id, event_type, payload).await,
        Some(s) => s,
    };

    let fallback_copy = payload.clone();
    match session.try_send_p2p(event_type.to_string(), payload) {
        SendOutcome::SentP2P => Ok(None),
        SendOutcome::FallbackToMatrix | SendOutcome::ChannelClosed => {
            send_matrix(matrix_client, room_id, event_type, fallback_copy).await
        }
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

/// Construction bundle (symmetric to the worker's `WorkerP2pSessionBundle`).
#[allow(clippy::too_many_arguments)]
pub fn build_p2p_session(
    cfg: &P2pConfig,
    no_p2p_cli: bool,
    matrix_client: Arc<MatrixClient>,
    our_user_id: impl Into<String>,
    our_device_id: impl Into<String>,
    room_id: impl Into<String>,
) -> ClientP2pSessionBundle {
    let our_device_id = our_device_id.into();
    let (session, events_tx) = ClientP2pSession::new_if_enabled(
        cfg,
        no_p2p_cli,
        our_user_id,
        our_device_id.clone(),
        room_id,
    );

    let effectively_enabled = cfg.enabled && !no_p2p_cli;

    let signer = if effectively_enabled {
        Some(Arc::new(
            mxdx_p2p::transport::matrix_signer::MatrixHandshakeSigner::new(
                matrix_client.clone(),
                our_device_id.clone(),
            ),
        ))
    } else {
        None
    };

    let peer_keys = if effectively_enabled {
        Some(Arc::new(
            mxdx_p2p::transport::matrix_signer::MatrixPeerKeySource::new(matrix_client),
        ))
    } else {
        None
    };

    ClientP2pSessionBundle {
        session,
        signer,
        peer_keys,
        call_event_tx: events_tx,
        no_p2p_cli,
    }
}

pub struct ClientP2pSessionBundle {
    pub session: Option<ClientP2pSession>,
    pub signer: Option<Arc<mxdx_p2p::transport::matrix_signer::MatrixHandshakeSigner>>,
    pub peer_keys: Option<Arc<mxdx_p2p::transport::matrix_signer::MatrixPeerKeySource>>,
    pub call_event_tx:
        Option<tokio::sync::mpsc::Sender<mxdx_p2p::signaling::parse::ParsedCallEvent>>,
    /// True iff the operator passed `--no-p2p`. Exposed so the daemon
    /// startup telemetry can log the effective flag.
    pub no_p2p_cli: bool,
}

impl ClientP2pSessionBundle {
    pub fn is_enabled(&self) -> bool {
        self.session.as_ref().map(|s| s.is_enabled()).unwrap_or(false)
    }
}

/// Picks the BatchedSender window for a given P2P state snapshot. Mirrors
/// the worker helper. The client uses the same flip rule: Open → 10ms,
/// everything else → 200ms.
pub fn batch_window_for_p2p_state(
    snap: Option<&P2PStateSnapshot>,
) -> std::time::Duration {
    match snap {
        Some(s) if s.is_open => std::time::Duration::from_millis(10),
        _ => std::time::Duration::from_millis(200),
    }
}
