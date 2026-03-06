use mxdx_launcher::config::*;
use mxdx_launcher::executor::*;

#[tokio::test]
async fn execute_echo_captures_stdout() {
    let config = CapabilitiesConfig {
        mode: CapabilityMode::Allowlist,
        allowed_commands: vec!["echo".to_string()],
        allowed_cwd_prefixes: vec!["/tmp".to_string()],
        max_sessions: 10,
    };

    let validated = validate_command(&config, "echo", &["hello-world"], Some("/tmp")).unwrap();
    let result = execute_command(&validated).await.unwrap();

    assert_eq!(result.exit_code, Some(0));
    assert!(result.stdout_lines.iter().any(|l| l.contains("hello-world")));
}

#[tokio::test]
async fn execute_separates_stdout_and_stderr() {
    let config = CapabilitiesConfig {
        mode: CapabilityMode::Allowlist,
        allowed_commands: vec!["sh".to_string()],
        allowed_cwd_prefixes: vec!["/tmp".to_string()],
        max_sessions: 10,
    };

    let validated =
        validate_command(&config, "sh", &["-c", "echo out; echo err >&2"], Some("/tmp")).unwrap();
    let result = execute_command(&validated).await.unwrap();

    assert_eq!(result.exit_code, Some(0));
    assert!(result.stdout_lines.iter().any(|l| l.contains("out")));
    assert!(result.stderr_lines.iter().any(|l| l.contains("err")));
}

#[tokio::test]
async fn large_output_streams_in_order() {
    let config = CapabilitiesConfig {
        mode: CapabilityMode::Allowlist,
        allowed_commands: vec!["seq".to_string()],
        allowed_cwd_prefixes: vec!["/tmp".to_string()],
        max_sessions: 10,
    };

    let validated = validate_command(&config, "seq", &["1", "100"], Some("/tmp")).unwrap();
    let result = execute_command(&validated).await.unwrap();

    assert_eq!(result.exit_code, Some(0));
    assert_eq!(result.stdout_lines.len(), 100);
    for (i, line) in result.stdout_lines.iter().enumerate() {
        assert_eq!(line, &(i + 1).to_string());
    }
}

#[tokio::test]
async fn orchestrator_sends_command_launcher_receives_over_matrix() {
    use mxdx_matrix::MatrixClient;
    use mxdx_test_helpers::tuwunel::TuwunelInstance;
    use mxdx_types::events::command::{CommandAction, CommandEvent};
    use std::collections::HashMap;

    let mut hs = TuwunelInstance::start().await.unwrap();
    let orchestrator = MatrixClient::register_and_connect(
        &format!("http://127.0.0.1:{}", hs.port),
        "orchestrator",
        "pass",
        "mxdx-test-token",
    )
    .await
    .unwrap();
    let launcher_client = MatrixClient::register_and_connect(
        &format!("http://127.0.0.1:{}", hs.port),
        "launcher",
        "pass",
        "mxdx-test-token",
    )
    .await
    .unwrap();

    let room_id = orchestrator
        .create_encrypted_room(&[launcher_client.user_id().to_owned()])
        .await
        .unwrap();
    launcher_client.join_room(&room_id).await.unwrap();

    // Key exchange via sync cycles
    orchestrator.sync_once().await.unwrap();
    launcher_client.sync_once().await.unwrap();
    orchestrator.sync_once().await.unwrap();
    launcher_client.sync_once().await.unwrap();

    // Send command event
    let cmd = CommandEvent {
        uuid: "test-e2e-1".into(),
        action: CommandAction::Exec,
        cmd: "echo".into(),
        args: vec!["hello-e2e".into()],
        env: HashMap::new(),
        cwd: None,
        timeout_seconds: Some(10),
    };
    let payload = serde_json::json!({
        "type": "org.mxdx.command",
        "content": serde_json::to_value(&cmd).unwrap()
    });
    orchestrator.send_event(&room_id, payload).await.unwrap();

    // Launcher receives
    let events = launcher_client
        .sync_and_collect_events(&room_id, std::time::Duration::from_secs(5))
        .await
        .unwrap();

    let cmd_event = events.iter().find(|e| {
        e.get("content")
            .and_then(|c| c.get("uuid"))
            .and_then(|u| u.as_str())
            == Some("test-e2e-1")
    });
    assert!(
        cmd_event.is_some(),
        "Launcher should receive command event: {:?}",
        events
    );

    // Parse back to CommandEvent
    let content = cmd_event.unwrap().get("content").unwrap();
    let parsed: CommandEvent = serde_json::from_value(content.clone()).unwrap();
    assert_eq!(parsed.cmd, "echo");
    assert_eq!(parsed.args, vec!["hello-e2e"]);

    hs.stop().await;
}
