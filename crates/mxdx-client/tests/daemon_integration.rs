use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::time::Duration;

/// Helper: start a daemon server on a temp socket and return the socket path.
async fn start_test_daemon(profile: &str) -> (std::path::PathBuf, tokio::task::JoinHandle<()>) {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join(format!("{}.sock", profile));
    let sock_clone = sock.clone();

    let handler = Arc::new(mxdx_client::daemon::handler::Handler::new(profile));

    let server = tokio::spawn(async move {
        mxdx_client::daemon::transport::unix::serve(&sock_clone, handler)
            .await
            .unwrap();
    });

    // Wait for socket to be ready
    for _ in 0..50 {
        if sock.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Keep tempdir alive by leaking it (tests are short-lived)
    std::mem::forget(dir);

    (sock, server)
}

/// Helper: send a JSON-RPC request and read the response line.
async fn send_and_receive(
    reader: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    json: &str,
) -> String {
    writer.write_all(json.as_bytes()).await.unwrap();
    writer.write_all(b"\n").await.unwrap();
    writer.flush().await.unwrap();

    let mut line = String::new();
    reader.read_line(&mut line).await.unwrap();
    line
}

#[tokio::test]
async fn daemon_status_roundtrip() {
    let (sock, server) = start_test_daemon("test-status").await;
    let stream = UnixStream::connect(&sock).await.unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    let resp = send_and_receive(
        &mut reader,
        &mut writer,
        r#"{"jsonrpc":"2.0","id":1,"method":"daemon.status"}"#,
    )
    .await;

    let value: serde_json::Value = serde_json::from_str(&resp).unwrap();
    assert_eq!(value["jsonrpc"], "2.0");
    assert_eq!(value["id"], 1);
    assert_eq!(value["result"]["profile"], "test-status");
    assert!(value["result"]["uptime_seconds"].is_number());
    assert_eq!(value["result"]["active_sessions"], 0);

    server.abort();
}

#[tokio::test]
async fn daemon_shutdown_roundtrip() {
    let (sock, server) = start_test_daemon("test-shutdown").await;
    let stream = UnixStream::connect(&sock).await.unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    let resp = send_and_receive(
        &mut reader,
        &mut writer,
        r#"{"jsonrpc":"2.0","id":1,"method":"daemon.shutdown"}"#,
    )
    .await;

    let value: serde_json::Value = serde_json::from_str(&resp).unwrap();
    assert_eq!(value["result"]["status"], "shutting_down");

    server.abort();
}

#[tokio::test]
async fn session_run_returns_matrix_unavailable() {
    let (sock, server) = start_test_daemon("test-run").await;
    let stream = UnixStream::connect(&sock).await.unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    let resp = send_and_receive(
        &mut reader,
        &mut writer,
        r#"{"jsonrpc":"2.0","id":1,"method":"session.run","params":{"bin":"echo","args":["hello"]}}"#,
    )
    .await;

    let value: serde_json::Value = serde_json::from_str(&resp).unwrap();
    assert_eq!(value["error"]["code"], -7); // MATRIX_UNAVAILABLE
    assert!(value["error"]["message"].as_str().unwrap().contains("Matrix"));

    server.abort();
}

#[tokio::test]
async fn unknown_method_returns_error() {
    let (sock, server) = start_test_daemon("test-unknown").await;
    let stream = UnixStream::connect(&sock).await.unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    let resp = send_and_receive(
        &mut reader,
        &mut writer,
        r#"{"jsonrpc":"2.0","id":1,"method":"nonexistent.method"}"#,
    )
    .await;

    let value: serde_json::Value = serde_json::from_str(&resp).unwrap();
    assert_eq!(value["error"]["code"], -32601); // METHOD_NOT_FOUND

    server.abort();
}

#[tokio::test]
async fn invalid_json_returns_parse_error() {
    let (sock, server) = start_test_daemon("test-parse").await;
    let stream = UnixStream::connect(&sock).await.unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    let resp = send_and_receive(
        &mut reader,
        &mut writer,
        r#"this is not json"#,
    )
    .await;

    let value: serde_json::Value = serde_json::from_str(&resp).unwrap();
    assert_eq!(value["error"]["code"], -32700); // PARSE_ERROR

    server.abort();
}

#[tokio::test]
async fn multiple_requests_on_same_connection() {
    let (sock, server) = start_test_daemon("test-multi").await;
    let stream = UnixStream::connect(&sock).await.unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    // First request
    let resp1 = send_and_receive(
        &mut reader,
        &mut writer,
        r#"{"jsonrpc":"2.0","id":1,"method":"daemon.status"}"#,
    )
    .await;
    let v1: serde_json::Value = serde_json::from_str(&resp1).unwrap();
    assert_eq!(v1["id"], 1);

    // Second request
    let resp2 = send_and_receive(
        &mut reader,
        &mut writer,
        r#"{"jsonrpc":"2.0","id":2,"method":"worker.list"}"#,
    )
    .await;
    let v2: serde_json::Value = serde_json::from_str(&resp2).unwrap();
    assert_eq!(v2["id"], 2);
    assert!(v2["result"]["workers"].as_array().unwrap().is_empty());

    server.abort();
}

#[tokio::test]
async fn events_subscribe_roundtrip() {
    let (sock, server) = start_test_daemon("test-events").await;
    let stream = UnixStream::connect(&sock).await.unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    // Subscribe
    let resp = send_and_receive(
        &mut reader,
        &mut writer,
        r#"{"jsonrpc":"2.0","id":1,"method":"events.subscribe","params":{"events":["session.*"]}}"#,
    )
    .await;
    let value: serde_json::Value = serde_json::from_str(&resp).unwrap();
    let sub_id = value["result"]["subscription_id"].as_str().unwrap();
    assert!(sub_id.starts_with("sub-"));

    // Unsubscribe
    let unsub_json = format!(
        r#"{{"jsonrpc":"2.0","id":2,"method":"events.unsubscribe","params":{{"subscription_id":"{}"}}}}"#,
        sub_id
    );
    let resp = send_and_receive(&mut reader, &mut writer, &unsub_json).await;
    let value: serde_json::Value = serde_json::from_str(&resp).unwrap();
    assert_eq!(value["result"]["removed"], true);

    server.abort();
}
