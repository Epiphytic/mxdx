use mxdx_test_helpers::TuwunelInstance;

#[tokio::test]
async fn client_connects_and_initializes_crypto() {
    let mut hs = TuwunelInstance::start().await.unwrap();
    let client = mxdx_matrix::MatrixClient::register_and_connect(
        &format!("http://127.0.0.1:{}", hs.port),
        "testbot",
        "password123",
    )
    .await
    .unwrap();

    assert!(client.is_logged_in());
    assert!(client.crypto_enabled());
    hs.stop().await;
}

#[tokio::test]
async fn two_clients_exchange_encrypted_event() {
    let mut hs = TuwunelInstance::start().await.unwrap();
    let alice = mxdx_matrix::MatrixClient::register_and_connect(
        &format!("http://127.0.0.1:{}", hs.port),
        "alice",
        "pass",
    )
    .await
    .unwrap();
    let bob = mxdx_matrix::MatrixClient::register_and_connect(
        &format!("http://127.0.0.1:{}", hs.port),
        "bob",
        "pass",
    )
    .await
    .unwrap();

    let room_id = alice
        .create_encrypted_room(&[bob.user_id().to_owned()])
        .await
        .unwrap();
    bob.join_room(&room_id).await.unwrap();

    // Exchange keys via initial syncs
    alice.sync_once().await.unwrap();
    bob.sync_once().await.unwrap();

    let payload = serde_json::json!({
        "type": "org.mxdx.command",
        "content": {"uuid": "test-1"}
    });
    alice.send_event(&room_id, payload.clone()).await.unwrap();

    let events = bob
        .sync_and_collect_events(&room_id, std::time::Duration::from_secs(5))
        .await
        .unwrap();
    assert!(
        events.iter().any(|e| e["content"]["uuid"] == "test-1"),
        "Expected to find event with uuid=test-1 in: {:?}",
        events
    );
    hs.stop().await;
}
