//! True end-to-end test over a public Matrix server.
//!
//! This test verifies the complete mxdx workflow against a real public homeserver:
//!
//!   Client (account1) ──send command──▶ matrix.org ──▶ Launcher (account2)
//!                                                          │
//!                                                     execute locally
//!                                                          │
//!   Client (account1) ◀──receive output── matrix.org ◀──send output──┘
//!
//! ## Setup
//!
//! Create `test-credentials.toml` in the repo root (gitignored):
//!
//! ```toml
//! [server]
//! url = "matrix.org"
//!
//! [account1]
//! username = "your-client-user"
//! password = "your-password"
//!
//! [account2]
//! username = "your-launcher-user"
//! password = "your-password"
//! ```
//!
//! Run with: cargo test -p mxdx-launcher --test e2e_public_server -- --ignored --nocapture

use std::collections::HashMap;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;

use mxdx_launcher::config::*;
use mxdx_launcher::executor::*;
use mxdx_matrix::MatrixClient;
use mxdx_types::events::command::{CommandAction, CommandEvent};
use mxdx_types::events::output::{OutputEvent, OutputStream};

/// Load two-account credentials from test-credentials.toml.
fn load_credentials() -> (String, String, String, String, String) {
    let toml_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("test-credentials.toml");

    let content = std::fs::read_to_string(&toml_path)
        .expect("test-credentials.toml not found — see test file header for setup");
    let parsed: toml::Value = content.parse().expect("Invalid TOML");

    let url = parsed["server"]["url"].as_str().unwrap().to_string();
    let u1 = parsed["account1"]["username"].as_str().unwrap().to_string();
    let p1 = parsed["account1"]["password"].as_str().unwrap().to_string();
    let u2 = parsed["account2"]["username"].as_str().unwrap().to_string();
    let p2 = parsed["account2"]["password"].as_str().unwrap().to_string();

    (url, u1, p1, u2, p2)
}

/// Full end-to-end: client sends a command over matrix.org, launcher executes it,
/// sends output back, and client receives the result.
#[tokio::test]
#[ignore = "requires two accounts on a public Matrix server (test-credentials.toml)"]
async fn public_server_command_round_trip() {
    let (url, u1, p1, u2, p2) = load_credentials();

    // ── 1. Connect both accounts ────────────────────────────────────
    eprintln!("[1] Connecting client ({u1}) and launcher ({u2}) to {url}...");
    let client = MatrixClient::login_and_connect(&url, &u1, &p1)
        .await
        .expect("Client login failed");
    let launcher = MatrixClient::login_and_connect(&url, &u2, &p2)
        .await
        .expect("Launcher login failed");

    assert!(client.is_logged_in());
    assert!(launcher.is_logged_in());
    eprintln!("[1] Both accounts connected.");

    // ── 2. Client creates encrypted room and invites launcher ───────
    eprintln!("[2] Creating encrypted command room...");
    let cmd_room_id = client
        .create_encrypted_room(&[launcher.user_id().to_owned()])
        .await
        .expect("Failed to create command room");
    eprintln!("[2] Room created: {cmd_room_id}");

    // ── 3. Launcher joins the room ──────────────────────────────────
    eprintln!("[3] Launcher joining room...");
    // Launcher needs to sync to see the invite
    launcher.sync_once().await.unwrap();
    launcher
        .join_room(&cmd_room_id)
        .await
        .expect("Launcher failed to join command room");
    eprintln!("[3] Launcher joined.");

    // ── 4. Key exchange — sync both sides multiple times ────────────
    eprintln!("[4] Exchanging E2EE keys...");
    for i in 0..4 {
        client.sync_once().await.unwrap();
        launcher.sync_once().await.unwrap();
        eprintln!("[4] Key exchange sync round {}/4", i + 1);
    }
    eprintln!("[4] Key exchange complete.");

    // ── 5. Client sends a command event ─────────────────────────────
    let test_uuid = format!("pub-e2e-{}", uuid::Uuid::new_v4());
    let cmd = CommandEvent {
        uuid: test_uuid.clone(),
        action: CommandAction::Exec,
        cmd: "echo".into(),
        args: vec!["hello-from-matrix-org".into()],
        env: HashMap::new(),
        cwd: None,
        timeout_seconds: Some(10),
    };

    eprintln!("[5] Client sending command: echo hello-from-matrix-org (uuid={test_uuid})");
    client
        .send_event(
            &cmd_room_id,
            serde_json::json!({
                "type": "org.mxdx.command",
                "content": serde_json::to_value(&cmd).unwrap()
            }),
        )
        .await
        .expect("Client failed to send command");
    eprintln!("[5] Command sent.");

    // ── 6. Launcher receives the command ────────────────────────────
    eprintln!("[6] Launcher waiting for command event...");
    let events = launcher
        .sync_and_collect_events(&cmd_room_id, Duration::from_secs(30))
        .await
        .expect("Launcher failed to sync");

    let cmd_event = events.iter().find(|e| {
        e.get("content")
            .and_then(|c| c.get("uuid"))
            .and_then(|u| u.as_str())
            == Some(&test_uuid)
    });
    assert!(cmd_event.is_some(), "Launcher should receive command event");

    let content = cmd_event.unwrap().get("content").unwrap();
    let parsed_cmd: CommandEvent =
        serde_json::from_value(content.clone()).expect("Failed to parse command event");
    assert_eq!(parsed_cmd.cmd, "echo");
    assert_eq!(parsed_cmd.args, vec!["hello-from-matrix-org"]);
    eprintln!(
        "[6] Launcher received command: {} {:?}",
        parsed_cmd.cmd, parsed_cmd.args
    );

    // ── 7. Launcher validates and executes the command locally ──────
    eprintln!("[7] Launcher validating and executing command...");
    let cap_config = CapabilitiesConfig {
        mode: CapabilityMode::Allowlist,
        allowed_commands: vec!["echo".to_string()],
        allowed_cwd_prefixes: vec!["/tmp".to_string()],
        max_sessions: 10,
    };

    let validated = validate_command(
        &cap_config,
        &parsed_cmd.cmd,
        &["hello-from-matrix-org"],
        Some("/tmp"),
    )
    .expect("Command validation failed");
    let result = execute_command(&validated)
        .await
        .expect("Command execution failed");

    assert_eq!(result.exit_code, Some(0));
    assert!(result
        .stdout_lines
        .iter()
        .any(|l| l.contains("hello-from-matrix-org")));
    eprintln!(
        "[7] Command executed. exit_code=0, stdout={:?}",
        result.stdout_lines
    );

    // ── 8. Launcher sends output event back over Matrix ─────────────
    let output = OutputEvent {
        uuid: test_uuid.clone(),
        stream: OutputStream::Stdout,
        data: BASE64.encode(result.stdout_lines.join("\n")),
        encoding: "raw+base64".into(),
        seq: 0,
    };

    eprintln!("[8] Launcher sending output event...");
    launcher
        .send_event(
            &cmd_room_id,
            serde_json::json!({
                "type": "org.mxdx.output",
                "content": serde_json::to_value(&output).unwrap()
            }),
        )
        .await
        .expect("Launcher failed to send output");
    eprintln!("[8] Output event sent.");

    // ── 9. Client receives the output ───────────────────────────────
    eprintln!("[9] Client waiting for output event...");
    let events = client
        .sync_and_collect_events(&cmd_room_id, Duration::from_secs(30))
        .await
        .expect("Client failed to sync");

    let output_event = events.iter().find(|e| {
        e.get("type").and_then(|t| t.as_str()) == Some("org.mxdx.output")
            && e.get("content")
                .and_then(|c| c.get("uuid"))
                .and_then(|u| u.as_str())
                == Some(&test_uuid)
    });
    assert!(
        output_event.is_some(),
        "Client should receive output event from launcher"
    );

    let output_content: OutputEvent =
        serde_json::from_value(output_event.unwrap()["content"].clone())
            .expect("Failed to parse output event");
    assert_eq!(output_content.uuid, test_uuid);
    assert_eq!(output_content.stream, OutputStream::Stdout);

    let decoded = String::from_utf8(BASE64.decode(&output_content.data).unwrap()).unwrap();
    assert!(
        decoded.contains("hello-from-matrix-org"),
        "Decoded output should contain 'hello-from-matrix-org', got: {decoded}"
    );
    eprintln!("[9] Client received output: {decoded}");

    eprintln!("[✓] Full round-trip complete: client → matrix.org → launcher → execute → matrix.org → client");

    // ── Cleanup: leave the room ─────────────────────────────────────
    if let Some(r) = client.inner().get_room(&cmd_room_id) {
        let _ = r.leave().await;
    }
    if let Some(r) = launcher.inner().get_room(&cmd_room_id) {
        let _ = r.leave().await;
    }
}
