//! Full system E2E test exercising all mxdx subsystems together.
//!
//! Requires Tuwunel and tmux (installed in CI integration job).

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use age::x25519::Identity;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;

use mxdx_launcher::config::*;
use mxdx_launcher::executor::*;
use mxdx_launcher::telemetry::collect_telemetry;
use mxdx_matrix::MatrixClient;
use mxdx_policy::appservice::{register_appservice, AppserviceRegistration};
use mxdx_policy::config::PolicyConfig;
use mxdx_policy::policy::{PolicyEngine, PolicyRejection};
use mxdx_secrets::coordinator::{decrypt_with_identity, SecretCoordinator};
use mxdx_secrets::store::SecretStore;
use mxdx_test_helpers::tuwunel::TuwunelInstance;
use mxdx_types::events::command::{CommandAction, CommandEvent};
use mxdx_types::events::secret::{SecretRequestEvent, SecretResponseEvent};

/// Full system E2E: starts Tuwunel, registers users, creates launcher topology,
/// sends a command over Matrix, executes it locally, sends output back, and
/// verifies the round-trip. Also tests telemetry, policy, terminal DM creation,
/// and secret store round-trip.
#[tokio::test]
async fn full_system_e2e() {
    // ── 1. Start Tuwunel ──────────────────────────────────────────────
    let mut hs = TuwunelInstance::start().await.unwrap();
    let base_url = format!("http://127.0.0.1:{}", hs.port);

    // ── 2. Register users ─────────────────────────────────────────────
    let admin = hs.register_user("admin", "adminpass").await.unwrap();

    let orchestrator =
        MatrixClient::register_and_connect(&base_url, "orchestrator", "pass", "mxdx-test-token")
            .await
            .unwrap();
    let launcher =
        MatrixClient::register_and_connect(&base_url, "launcher", "pass", "mxdx-test-token")
            .await
            .unwrap();

    assert!(orchestrator.is_logged_in());
    assert!(launcher.is_logged_in());

    // ── 3. Create launcher topology (space + rooms) ───────────────────
    let topology = launcher
        .create_launcher_space("e2e-launcher")
        .await
        .unwrap();
    assert!(!topology.space_id.as_str().is_empty());
    assert!(!topology.exec_room_id.as_str().is_empty());
    assert!(!topology.status_room_id.as_str().is_empty());
    assert!(!topology.logs_room_id.as_str().is_empty());

    // ── 4. Invite orchestrator to exec room and key exchange ──────────
    let cmd_room_id = orchestrator
        .create_encrypted_room(&[launcher.user_id().to_owned()])
        .await
        .unwrap();
    launcher.join_room(&cmd_room_id).await.unwrap();

    orchestrator.sync_once().await.unwrap();
    launcher.sync_once().await.unwrap();
    orchestrator.sync_once().await.unwrap();
    launcher.sync_once().await.unwrap();

    // ── 5. Send command event over Matrix ─────────────────────────────
    let cmd = CommandEvent {
        uuid: "e2e-full-001".into(),
        action: CommandAction::Exec,
        cmd: "echo".into(),
        args: vec!["full-system-e2e".into()],
        env: HashMap::new(),
        cwd: None,
        timeout_seconds: Some(10),
    };

    orchestrator
        .send_event(
            &cmd_room_id,
            serde_json::json!({
                "type": "org.mxdx.command",
                "content": serde_json::to_value(&cmd).unwrap()
            }),
        )
        .await
        .unwrap();

    // ── 6. Launcher receives command event ────────────────────────────
    let events = launcher
        .sync_and_collect_events(&cmd_room_id, Duration::from_secs(5))
        .await
        .unwrap();

    let cmd_event = events.iter().find(|e| {
        e.get("content")
            .and_then(|c| c.get("uuid"))
            .and_then(|u| u.as_str())
            == Some("e2e-full-001")
    });
    assert!(cmd_event.is_some(), "Launcher should receive command event");

    let content = cmd_event.unwrap().get("content").unwrap();
    let parsed: CommandEvent = serde_json::from_value(content.clone()).unwrap();
    assert_eq!(parsed.cmd, "echo");
    assert_eq!(parsed.args, vec!["full-system-e2e"]);

    // ── 7. Execute the command locally ────────────────────────────────
    let cap_config = CapabilitiesConfig {
        mode: CapabilityMode::Allowlist,
        allowed_commands: vec!["echo".to_string()],
        allowed_cwd_prefixes: vec!["/tmp".to_string()],
        max_sessions: 10,
    };

    let validated =
        validate_command(&cap_config, &parsed.cmd, &["full-system-e2e"], Some("/tmp")).unwrap();
    let result = execute_command(&validated).await.unwrap();

    assert_eq!(result.exit_code, Some(0));
    assert!(result
        .stdout_lines
        .iter()
        .any(|l| l.contains("full-system-e2e")));

    // ── 8. Verify telemetry collection ────────────────────────────────
    let telemetry = collect_telemetry(TelemetryDetail::Full);
    assert!(!telemetry.hostname.is_empty());
    assert!(!telemetry.os.is_empty());
    assert!(!telemetry.arch.is_empty());
    assert!(telemetry.uptime_seconds > 0);
    assert!(telemetry.cpu.cores > 0);
    assert!(telemetry.memory.total_bytes > 0);
    assert!(telemetry.network.is_some());

    let summary_telemetry = collect_telemetry(TelemetryDetail::Summary);
    assert!(summary_telemetry.network.is_none());

    // ── 9. Terminal session DM creation ───────────────────────────────
    let dm_room_id = launcher
        .create_terminal_session_dm(orchestrator.user_id())
        .await
        .unwrap();
    launcher.sync_once().await.unwrap();

    let state = launcher
        .get_room_state(&dm_room_id, "m.room.history_visibility")
        .await
        .unwrap();
    assert_eq!(state["history_visibility"], "joined");

    // ── 10. Policy engine: replay detection + authorization ───────────
    let mut policy = PolicyEngine::new();
    let orch_user_id = orchestrator.user_id().to_string();
    policy.authorize_user(&orch_user_id);

    assert!(policy
        .evaluate("$evt-e2e-1", &orch_user_id, "execute")
        .is_ok());
    assert_eq!(
        policy.evaluate("$evt-e2e-1", &orch_user_id, "execute"),
        Err(PolicyRejection::Replay)
    );
    assert_eq!(
        policy.evaluate("$evt-e2e-2", "@intruder:test.localhost", "execute"),
        Err(PolicyRejection::Unauthorized)
    );

    // ── 11. Appservice registration ───────────────────────────────────
    let policy_config = PolicyConfig {
        homeserver_url: base_url.clone(),
        as_token: "e2e_as_token".to_string(),
        hs_token: "e2e_hs_token".to_string(),
        server_name: hs.server_name.clone(),
        sender_localpart: "mxdx-policy".to_string(),
        user_prefix: "agent-".to_string(),
        appservice_port: 0,
    };

    let registration = AppserviceRegistration::from_config(&policy_config);
    register_appservice(&base_url, &admin.access_token, &registration)
        .await
        .expect("Appservice registration should succeed");

    // Verify agent namespace is claimed: registering @agent-test should fail
    let http_client = reqwest::Client::new();
    let reg_url = format!("{}/_matrix/client/v3/register", base_url);
    let body = serde_json::json!({
        "username": "agent-test",
        "password": "testpass",
        "auth": {
            "type": "m.login.registration_token",
            "token": "mxdx-test-token"
        }
    });
    let resp = http_client.post(&reg_url).json(&body).send().await.unwrap();
    assert!(
        !resp.status().is_success(),
        "Registration of @agent-test should fail when namespace is exclusively claimed"
    );

    // ── 12. Secret store + coordinator round-trip ─────────────────────
    let coord_identity = Identity::generate();
    let mut store = SecretStore::new(coord_identity);
    store.add("deploy.token", "tok_e2e_secret_value").unwrap();

    // Verify store round-trip
    let retrieved = store.get("deploy.token").unwrap().unwrap();
    assert_eq!(retrieved, "tok_e2e_secret_value");

    // Serialize/deserialize round-trip
    let serialized = store.serialize().unwrap();
    let store2 = SecretStore::deserialize(&serialized, store.key()).unwrap();
    assert_eq!(
        store2.get("deploy.token").unwrap().unwrap(),
        "tok_e2e_secret_value"
    );

    // ── 13. Secret request with double encryption over Matrix ─────────
    let coord_client =
        MatrixClient::register_and_connect(&base_url, "coordinator", "pass", "mxdx-test-token")
            .await
            .unwrap();
    let worker_client =
        MatrixClient::register_and_connect(&base_url, "worker", "pass", "mxdx-test-token")
            .await
            .unwrap();

    let secret_room_id = coord_client
        .create_encrypted_room(&[worker_client.user_id().to_owned()])
        .await
        .unwrap();
    worker_client.join_room(&secret_room_id).await.unwrap();

    coord_client.sync_once().await.unwrap();
    worker_client.sync_once().await.unwrap();
    coord_client.sync_once().await.unwrap();
    worker_client.sync_once().await.unwrap();

    let mut coord_store = SecretStore::new(Identity::generate());
    coord_store
        .add("deploy.token", "tok_e2e_secret_value")
        .unwrap();
    let authorized: HashSet<String> = ["deploy.token".to_string()].into();
    let coordinator = SecretCoordinator::new(coord_store, authorized);

    let ephemeral_identity = Identity::generate();
    let ephemeral_pubkey = ephemeral_identity.to_public().to_string();

    let request = SecretRequestEvent {
        request_id: "req-full-e2e-001".into(),
        scope: "deploy.token".into(),
        ttl_seconds: 300,
        reason: "full e2e test".into(),
        ephemeral_public_key: ephemeral_pubkey,
    };

    worker_client
        .send_event(
            &secret_room_id,
            serde_json::json!({
                "type": "org.mxdx.secret.request",
                "content": request,
            }),
        )
        .await
        .unwrap();

    let events = coord_client
        .sync_and_collect_events(&secret_room_id, Duration::from_secs(10))
        .await
        .unwrap();

    let request_event = events
        .iter()
        .find(|e| e.get("type").and_then(|t| t.as_str()) == Some("org.mxdx.secret.request"))
        .expect("coordinator should see secret request event");

    let received_request: SecretRequestEvent =
        serde_json::from_value(request_event["content"].clone()).unwrap();
    assert_eq!(received_request.request_id, "req-full-e2e-001");

    let response = coordinator.handle_secret_request(&received_request);
    assert!(response.granted);
    assert!(response.encrypted_value.is_some());

    coord_client
        .send_event(
            &secret_room_id,
            serde_json::json!({
                "type": "org.mxdx.secret.response",
                "content": response,
            }),
        )
        .await
        .unwrap();

    let events = worker_client
        .sync_and_collect_events(&secret_room_id, Duration::from_secs(10))
        .await
        .unwrap();

    let response_event = events
        .iter()
        .find(|e| e.get("type").and_then(|t| t.as_str()) == Some("org.mxdx.secret.response"))
        .expect("worker should see secret response event");

    let received_response: SecretResponseEvent =
        serde_json::from_value(response_event["content"].clone()).unwrap();
    assert!(received_response.granted);

    let ciphertext = BASE64
        .decode(received_response.encrypted_value.as_ref().unwrap())
        .unwrap();
    let plaintext = decrypt_with_identity(&ephemeral_identity, &ciphertext).unwrap();
    assert_eq!(
        String::from_utf8(plaintext).unwrap(),
        "tok_e2e_secret_value"
    );

    // ── Cleanup ───────────────────────────────────────────────────────
    hs.stop().await;
}

/// Focused test: config validation + command execution pipeline without Matrix.
#[tokio::test]
async fn config_and_executor_pipeline() {
    let config_toml = r#"
        [global]
        launcher_id = "e2e-test"
        data_dir = "/tmp/mxdx-e2e"

        [[homeservers]]
        url = "https://hs1.example.com"
        username = "launcher-1"
        password = "secret"

        [capabilities]
        mode = "allowlist"
        allowed_commands = ["echo", "seq"]
        allowed_cwd_prefixes = ["/tmp"]
        max_sessions = 5

        [telemetry]
        detail_level = "full"
        poll_interval_seconds = 30
    "#;

    let config: LauncherConfig = toml::from_str(config_toml).unwrap();
    assert_eq!(config.global.launcher_id, "e2e-test");
    assert_eq!(config.capabilities.allowed_commands, vec!["echo", "seq"]);
    assert_eq!(config.telemetry.detail_level, TelemetryDetail::Full);

    // Execute echo via the config-validated pipeline
    let validated = validate_command(
        &config.capabilities,
        "echo",
        &["config-pipeline-test"],
        Some("/tmp"),
    )
    .unwrap();
    let result = execute_command(&validated).await.unwrap();
    assert_eq!(result.exit_code, Some(0));
    assert!(result
        .stdout_lines
        .iter()
        .any(|l| l.contains("config-pipeline-test")));

    // Execute seq to verify ordered output
    let validated =
        validate_command(&config.capabilities, "seq", &["1", "10"], Some("/tmp")).unwrap();
    let result = execute_command(&validated).await.unwrap();
    assert_eq!(result.exit_code, Some(0));
    assert_eq!(result.stdout_lines.len(), 10);

    // Rejected command
    let rejected = validate_command(&config.capabilities, "rm", &["-rf", "/"], Some("/tmp"));
    assert!(rejected.is_err());

    // Rejected cwd
    let rejected_cwd = validate_command(&config.capabilities, "echo", &["hi"], Some("/etc"));
    assert!(rejected_cwd.is_err());
}

/// Focused test: policy engine replay + authorization in sequence.
#[tokio::test]
async fn policy_engine_integrated_flow() {
    let mut engine = PolicyEngine::new();
    let admin = "@admin:test.localhost";
    let intruder = "@intruder:test.localhost";

    engine.authorize_user(admin);

    // First command passes
    assert!(engine.evaluate("$cmd-1", admin, "execute").is_ok());
    // Replay of same event is blocked
    assert_eq!(
        engine.evaluate("$cmd-1", admin, "execute"),
        Err(PolicyRejection::Replay)
    );
    // Different event passes
    assert!(engine.evaluate("$cmd-2", admin, "execute").is_ok());
    // Unauthorized user is blocked
    assert_eq!(
        engine.evaluate("$cmd-3", intruder, "execute"),
        Err(PolicyRejection::Unauthorized)
    );
    // Revoked user is blocked
    engine.revoke_user(admin);
    assert_eq!(
        engine.evaluate("$cmd-4", admin, "execute"),
        Err(PolicyRejection::Unauthorized)
    );
}

/// Focused test: secret store encryption round-trip without Matrix.
#[tokio::test]
async fn secret_store_and_coordinator_round_trip() {
    let identity = Identity::generate();
    let mut store = SecretStore::new(identity);
    store.add("db.password", "super_secret_db_pass").unwrap();
    store.add("api.key", "sk_live_abc123").unwrap();

    assert_eq!(
        store.get("db.password").unwrap().unwrap(),
        "super_secret_db_pass"
    );
    assert_eq!(store.get("api.key").unwrap().unwrap(), "sk_live_abc123");
    assert!(store.get("nonexistent").unwrap().is_none());

    // Serialize and restore
    let serialized = store.serialize().unwrap();
    let restored = SecretStore::deserialize(&serialized, store.key()).unwrap();
    assert_eq!(
        restored.get("db.password").unwrap().unwrap(),
        "super_secret_db_pass"
    );

    // Coordinator double-encryption
    let authorized: HashSet<String> = ["db.password".to_string()].into();
    let coordinator = SecretCoordinator::new(restored, authorized);

    let worker_identity = Identity::generate();
    let request = SecretRequestEvent {
        request_id: "req-store-001".into(),
        scope: "db.password".into(),
        ttl_seconds: 300,
        reason: "e2e round-trip".into(),
        ephemeral_public_key: worker_identity.to_public().to_string(),
    };

    let response = coordinator.handle_secret_request(&request);
    assert!(response.granted);

    let ciphertext = BASE64
        .decode(response.encrypted_value.as_ref().unwrap())
        .unwrap();
    let plaintext = decrypt_with_identity(&worker_identity, &ciphertext).unwrap();
    assert_eq!(
        String::from_utf8(plaintext).unwrap(),
        "super_secret_db_pass"
    );

    // Unauthorized scope
    let bad_request = SecretRequestEvent {
        request_id: "req-store-002".into(),
        scope: "api.key".into(),
        ttl_seconds: 300,
        reason: "should fail".into(),
        ephemeral_public_key: worker_identity.to_public().to_string(),
    };
    let bad_response = coordinator.handle_secret_request(&bad_request);
    assert!(!bad_response.granted);
}

/// Focused test: telemetry at both detail levels.
#[tokio::test]
async fn telemetry_both_levels() {
    let full = collect_telemetry(TelemetryDetail::Full);
    assert!(!full.hostname.is_empty());
    assert!(full.cpu.cores > 0);
    assert!(full.memory.total_bytes > 0);
    assert!(full.network.is_some());

    let summary = collect_telemetry(TelemetryDetail::Summary);
    assert!(!summary.hostname.is_empty());
    assert!(summary.network.is_none());
    assert!(summary.services.is_none());
    assert!(summary.devices.is_none());
}
