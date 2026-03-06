use mxdx_test_helpers::tuwunel::TuwunelInstance;

#[tokio::test]
async fn create_launcher_space_creates_space_with_child_rooms() {
    let mut hs = TuwunelInstance::start().await.unwrap();
    let client = mxdx_matrix::MatrixClient::register_and_connect(
        &format!("http://127.0.0.1:{}", hs.port),
        "launcher",
        "pass",
    )
    .await
    .unwrap();

    let topology = client.create_launcher_space("belthanior").await.unwrap();

    assert!(!topology.space_id.as_str().is_empty());
    assert!(!topology.exec_room_id.as_str().is_empty());
    assert!(!topology.status_room_id.as_str().is_empty());
    assert!(!topology.logs_room_id.as_str().is_empty());

    // Verify space has m.space.child state events for each child room
    client.sync_once().await.unwrap();

    let exec_child = client
        .get_room_state(&topology.space_id, &format!("m.space.child/{}", topology.exec_room_id))
        .await;
    assert!(exec_child.is_ok(), "exec room should be linked as space child");

    let status_child = client
        .get_room_state(&topology.space_id, &format!("m.space.child/{}", topology.status_room_id))
        .await;
    assert!(status_child.is_ok(), "status room should be linked as space child");

    let logs_child = client
        .get_room_state(&topology.space_id, &format!("m.space.child/{}", topology.logs_room_id))
        .await;
    assert!(logs_child.is_ok(), "logs room should be linked as space child");

    hs.stop().await;
}

#[tokio::test]
async fn terminal_dm_has_joined_history_visibility_from_creation() {
    // SECURITY TEST (mxdx-aew)
    let mut hs = TuwunelInstance::start().await.unwrap();
    let launcher = mxdx_matrix::MatrixClient::register_and_connect(
        &format!("http://127.0.0.1:{}", hs.port),
        "launcher",
        "pass",
    )
    .await
    .unwrap();
    let user = mxdx_matrix::MatrixClient::register_and_connect(
        &format!("http://127.0.0.1:{}", hs.port),
        "user",
        "pass",
    )
    .await
    .unwrap();

    let room_id = launcher
        .create_terminal_session_dm(user.user_id())
        .await
        .unwrap();

    launcher.sync_once().await.unwrap();

    let state = launcher
        .get_room_state(&room_id, "m.room.history_visibility")
        .await
        .unwrap();
    assert_eq!(
        state["history_visibility"], "joined",
        "Terminal DM must have history_visibility=joined (mxdx-aew)"
    );

    hs.stop().await;
}

#[tokio::test]
async fn tombstone_room_marks_room_replaced() {
    let mut hs = TuwunelInstance::start().await.unwrap();
    let client = mxdx_matrix::MatrixClient::register_and_connect(
        &format!("http://127.0.0.1:{}", hs.port),
        "testbot",
        "pass",
    )
    .await
    .unwrap();

    let old_room = client.create_encrypted_room(&[]).await.unwrap();
    let new_room = client.create_encrypted_room(&[]).await.unwrap();

    // Need to sync so rooms are known locally before sending state events
    client.sync_once().await.unwrap();

    client.tombstone_room(&old_room, &new_room).await.unwrap();

    client.sync_once().await.unwrap();
    let state = client
        .get_room_state(&old_room, "m.room.tombstone")
        .await
        .unwrap();
    assert_eq!(state["replacement_room"], new_room.to_string());

    hs.stop().await;
}
