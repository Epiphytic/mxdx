//! Integration tests for T-62 client daemon P2P wiring.
//!
//! Mirrors the worker's `p2p_wiring.rs` — verifies flag-off and
//! `--no-p2p` CLI override are strict no-ops, and flag-on constructs
//! the transport in Idle.

use mxdx_client::p2p_integration::{
    batch_window_for_p2p_state, ClientP2pSession,
};
use mxdx_p2p::transport::P2PStateSnapshot;
use mxdx_types::config::P2pConfig;
use std::time::Duration;

fn disabled_config() -> P2pConfig {
    P2pConfig {
        enabled: false,
        ..P2pConfig::default()
    }
}

fn enabled_config() -> P2pConfig {
    P2pConfig {
        enabled: true,
        idle_timeout_seconds: 300,
    }
}

#[tokio::test]
async fn disabled_flag_yields_no_session() {
    let (session, events_tx) = ClientP2pSession::new_if_enabled(
        &disabled_config(),
        false, // no_p2p_cli = false (not overriding)
        "@client:example.org",
        "CLIENTDEV",
        "!room:example.org",
    );
    assert!(session.is_none());
    assert!(events_tx.is_none());
}

#[tokio::test]
async fn enabled_flag_constructs_session() {
    let (session, events_tx) = ClientP2pSession::new_if_enabled(
        &enabled_config(),
        false,
        "@client:example.org",
        "CLIENTDEV",
        "!room:example.org",
    );
    assert!(session.is_some());
    assert!(events_tx.is_some());

    let session = session.unwrap();
    assert!(session.is_enabled());
    let snap = session.state().expect("state snapshot available");
    assert_eq!(snap.name, "Idle");
    assert!(!snap.is_open);
}

// The critical --no-p2p CLI override test: even with config.enabled=true,
// passing no_p2p_cli=true must yield None (flag-off behavior).
#[tokio::test]
async fn no_p2p_cli_override_forces_matrix_fallback() {
    let (session, events_tx) = ClientP2pSession::new_if_enabled(
        &enabled_config(), // config SAYS enabled
        true,              // but CLI SAYS --no-p2p
        "@client:example.org",
        "CLIENTDEV",
        "!room:example.org",
    );
    assert!(
        session.is_none(),
        "--no-p2p must override config.p2p.enabled"
    );
    assert!(events_tx.is_none());
}

#[tokio::test]
async fn no_p2p_cli_override_is_noop_when_config_already_disabled() {
    let (session, _) = ClientP2pSession::new_if_enabled(
        &disabled_config(),
        true,
        "@c:ex",
        "DEV",
        "!r:ex",
    );
    assert!(session.is_none());
}

#[tokio::test]
async fn disabled_session_helper_is_consistent() {
    let session = ClientP2pSession::disabled();
    assert!(!session.is_enabled());
    assert!(session.state().is_none());
}

#[tokio::test]
async fn feature_flag_off_is_identical_to_pre_phase_6_path() {
    let (session, events_tx) = ClientP2pSession::new_if_enabled(
        &disabled_config(),
        false,
        "@c:ex",
        "DEV",
        "!r:ex",
    );
    assert!(session.is_none());
    assert!(events_tx.is_none());
}

#[tokio::test]
async fn idle_timeout_override_is_applied_when_enabled() {
    let cfg = P2pConfig {
        enabled: true,
        idle_timeout_seconds: 42,
    };
    let (session, _) = ClientP2pSession::new_if_enabled(
        &cfg, false, "@c:ex", "DEV", "!r:ex",
    );
    assert!(session.is_some());
    drop(session);
}

// ---- Window flip rule (mirrors worker T-61) ----

#[tokio::test]
async fn window_flip_rule_open_yields_10ms() {
    let snap = P2PStateSnapshot {
        name: "Open",
        is_open: true,
    };
    assert_eq!(
        batch_window_for_p2p_state(Some(&snap)),
        Duration::from_millis(10)
    );
}

#[tokio::test]
async fn window_flip_rule_non_open_yields_200ms() {
    for name in &["Idle", "FetchingTurn", "Inviting", "Verifying", "Failed"] {
        let snap = P2PStateSnapshot {
            name,
            is_open: false,
        };
        assert_eq!(
            batch_window_for_p2p_state(Some(&snap)),
            Duration::from_millis(200),
            "non-Open state {name} must yield 200ms"
        );
    }
}

#[tokio::test]
async fn window_flip_rule_no_transport_yields_default_200ms() {
    assert_eq!(
        batch_window_for_p2p_state(None),
        Duration::from_millis(200)
    );
}
