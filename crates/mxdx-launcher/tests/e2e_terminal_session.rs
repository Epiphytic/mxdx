use std::time::Duration;

#[tokio::test]
async fn launcher_creates_terminal_dm_on_session_request() {
    use mxdx_matrix::MatrixClient;
    use mxdx_test_helpers::tuwunel::TuwunelInstance;

    let mut hs = TuwunelInstance::start().await.unwrap();
    let user = MatrixClient::register_and_connect(
        &format!("http://127.0.0.1:{}", hs.port),
        "user",
        "pass",
    )
    .await
    .unwrap();
    let launcher = MatrixClient::register_and_connect(
        &format!("http://127.0.0.1:{}", hs.port),
        "launcher",
        "pass",
    )
    .await
    .unwrap();

    // Launcher creates a terminal DM (simulating what the session handler would do)
    let dm_room_id = launcher
        .create_terminal_session_dm(user.user_id())
        .await
        .unwrap();

    // Verify the DM has history_visibility=joined (mxdx-aew)
    launcher.sync_once().await.unwrap();
    let state = launcher
        .get_room_state(&dm_room_id, "m.room.history_visibility")
        .await
        .unwrap();
    assert_eq!(state["history_visibility"], "joined");

    hs.stop().await;
}

#[tokio::test]
async fn tmux_session_bridges_input_to_output() {
    use mxdx_launcher::terminal::compression::{compress_encode, decode_decompress_bounded};
    use mxdx_launcher::terminal::tmux::TmuxSession;

    let name = format!("test-bridge-{}", std::process::id());
    let session = TmuxSession::create(&name, "/bin/bash", 80, 24)
        .await
        .unwrap();

    // Simulate: user sends terminal data (compressed input)
    let input = "echo bridge-test\n";
    let (encoded_input, encoding) = compress_encode(input.as_bytes());
    let decoded = decode_decompress_bounded(&encoded_input, &encoding, 1_048_576).unwrap();

    // Feed to tmux
    session
        .send_input(std::str::from_utf8(&decoded).unwrap())
        .await
        .unwrap();

    // Capture output
    let output = session
        .capture_pane_until("bridge-test", Duration::from_secs(2))
        .await
        .unwrap();
    assert!(output.contains("bridge-test"));

    // Compress output for sending back
    let (encoded_output, out_encoding) = compress_encode(output.as_bytes());
    assert!(!encoded_output.is_empty());
    assert!(out_encoding == "raw+base64" || out_encoding == "zlib+base64");

    session.kill().await.unwrap();
}

#[tokio::test]
async fn seq_counter_supports_u64_range() {
    use mxdx_launcher::terminal::ring_buffer::EventRingBuffer;

    let mut rb = EventRingBuffer::new(10);
    // Test near u64::MAX to verify no overflow issues
    let large_seq: u64 = u64::MAX - 5;
    for i in 0..5 {
        rb.push(large_seq + i, format!("event-{}", i));
    }
    assert_eq!(rb.get(large_seq), Some(&"event-0".to_string()));
    assert_eq!(rb.get(large_seq + 4), Some(&"event-4".to_string()));
}
