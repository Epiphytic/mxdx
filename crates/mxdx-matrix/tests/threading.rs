use mxdx_test_helpers::tuwunel::TuwunelInstance;

#[tokio::test]
async fn send_event_returns_event_id() {
    let mut hs = TuwunelInstance::start().await.unwrap();
    let client = mxdx_matrix::MatrixClient::register_and_connect(
        &format!("http://127.0.0.1:{}", hs.port),
        "alice",
        "pass",
        "mxdx-test-token",
    )
    .await
    .unwrap();

    let room_id = client.create_encrypted_room(&[]).await.unwrap();
    client.sync_once().await.unwrap();

    let payload = serde_json::json!({
        "type": "org.mxdx.test",
        "content": {"hello": "world"}
    });
    let event_id = client.send_event(&room_id, payload).await.unwrap();

    assert!(
        event_id.starts_with('$'),
        "Event ID should start with '$', got: {event_id}"
    );
    assert!(!event_id.is_empty(), "Event ID should not be empty");

    hs.stop().await;
}

#[tokio::test]
async fn send_threaded_event_includes_relates_to() {
    let mut hs = TuwunelInstance::start().await.unwrap();
    let alice = mxdx_matrix::MatrixClient::register_and_connect(
        &format!("http://127.0.0.1:{}", hs.port),
        "alice",
        "pass",
        "mxdx-test-token",
    )
    .await
    .unwrap();
    let bob = mxdx_matrix::MatrixClient::register_and_connect(
        &format!("http://127.0.0.1:{}", hs.port),
        "bob",
        "pass",
        "mxdx-test-token",
    )
    .await
    .unwrap();

    let room_id = alice
        .create_encrypted_room(&[bob.user_id().to_owned()])
        .await
        .unwrap();
    bob.join_room(&room_id).await.unwrap();

    alice.sync_once().await.unwrap();
    bob.sync_once().await.unwrap();
    alice.sync_once().await.unwrap();
    bob.sync_once().await.unwrap();

    let root_payload = serde_json::json!({
        "type": "org.mxdx.fabric.task",
        "content": {"uuid": "task-root-1", "prompt": "do something"}
    });
    let root_event_id = alice.send_event(&room_id, root_payload).await.unwrap();

    assert!(
        root_event_id.starts_with('$'),
        "Root event ID should start with '$', got: {root_event_id}"
    );

    let thread_content = serde_json::json!({
        "task_uuid": "task-root-1",
        "status": "completed",
        "output": "done"
    });
    let thread_event_id = alice
        .send_threaded_event(
            &room_id,
            "org.mxdx.fabric.result",
            &root_event_id,
            thread_content,
        )
        .await
        .unwrap();

    assert!(
        thread_event_id.starts_with('$'),
        "Thread event ID should start with '$', got: {thread_event_id}"
    );
    assert_ne!(
        root_event_id, thread_event_id,
        "Thread event should have a different ID than the root"
    );

    let events = bob
        .sync_and_collect_events(&room_id, std::time::Duration::from_secs(5))
        .await
        .unwrap();

    let thread_event = events.iter().find(|e| {
        e.get("content")
            .and_then(|c| c.get("task_uuid"))
            .and_then(|u| u.as_str())
            == Some("task-root-1")
            && e.get("content")
                .and_then(|c| c.get("m.relates_to"))
                .is_some()
    });

    assert!(
        thread_event.is_some(),
        "Should find threaded event with m.relates_to in: {events:?}"
    );

    let relates_to = thread_event.unwrap()["content"]["m.relates_to"].clone();
    assert_eq!(
        relates_to["rel_type"], "m.thread",
        "rel_type should be m.thread"
    );
    assert_eq!(
        relates_to["event_id"], root_event_id,
        "event_id should match the root event"
    );

    hs.stop().await;
}

#[tokio::test]
async fn send_threaded_event_preserves_existing_content() {
    let mut hs = TuwunelInstance::start().await.unwrap();
    let alice = mxdx_matrix::MatrixClient::register_and_connect(
        &format!("http://127.0.0.1:{}", hs.port),
        "alice",
        "pass",
        "mxdx-test-token",
    )
    .await
    .unwrap();
    let bob = mxdx_matrix::MatrixClient::register_and_connect(
        &format!("http://127.0.0.1:{}", hs.port),
        "bob",
        "pass",
        "mxdx-test-token",
    )
    .await
    .unwrap();

    let room_id = alice
        .create_encrypted_room(&[bob.user_id().to_owned()])
        .await
        .unwrap();
    bob.join_room(&room_id).await.unwrap();

    alice.sync_once().await.unwrap();
    bob.sync_once().await.unwrap();
    alice.sync_once().await.unwrap();
    bob.sync_once().await.unwrap();

    let root_payload = serde_json::json!({
        "type": "org.mxdx.test.root",
        "content": {"marker": "root"}
    });
    let root_id = alice.send_event(&room_id, root_payload).await.unwrap();

    let content = serde_json::json!({
        "worker_id": "w1",
        "progress": 50,
        "chunks": ["line1", "line2"]
    });
    alice
        .send_threaded_event(&room_id, "org.mxdx.fabric.heartbeat", &root_id, content)
        .await
        .unwrap();

    let events = bob
        .sync_and_collect_events(&room_id, std::time::Duration::from_secs(5))
        .await
        .unwrap();

    let hb_event = events.iter().find(|e| {
        e.get("content")
            .and_then(|c| c.get("worker_id"))
            .and_then(|w| w.as_str())
            == Some("w1")
    });

    assert!(
        hb_event.is_some(),
        "Should find heartbeat event in: {events:?}"
    );

    let content = &hb_event.unwrap()["content"];
    assert_eq!(content["worker_id"], "w1");
    assert_eq!(content["progress"], 50);
    assert_eq!(content["chunks"][0], "line1");
    assert_eq!(content["chunks"][1], "line2");

    assert_eq!(content["m.relates_to"]["rel_type"], "m.thread");
    assert_eq!(content["m.relates_to"]["event_id"], root_id);

    hs.stop().await;
}
