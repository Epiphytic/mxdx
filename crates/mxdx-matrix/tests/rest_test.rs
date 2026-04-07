use mxdx_matrix::rest::RestClient;

#[tokio::test]
async fn list_joined_rooms_returns_room_ids() {
    let mut server = mockito::Server::new_async().await;
    let _m = server
        .mock("GET", "/_matrix/client/v3/joined_rooms")
        .match_header("authorization", "Bearer test-token")
        .with_status(200)
        .with_body(r#"{"joined_rooms":["!aaa:example.org","!bbb:example.org"]}"#)
        .create_async()
        .await;

    let client = RestClient::new(&server.url(), "test-token");
    let rooms = client.list_joined_rooms().await.unwrap();
    assert_eq!(rooms.len(), 2);
    assert_eq!(rooms[0].as_str(), "!aaa:example.org");
}

#[tokio::test]
async fn list_invited_rooms_returns_invite_keys() {
    let mut server = mockito::Server::new_async().await;
    let body = r#"{"rooms":{"invite":{"!inv1:example.org":{},"!inv2:example.org":{}},"join":{},"leave":{}}}"#;
    let _m = server
        .mock("GET", mockito::Matcher::Regex(r"^/_matrix/client/v3/sync".to_string()))
        .with_status(200)
        .with_body(body)
        .create_async()
        .await;

    let client = RestClient::new(&server.url(), "test-token");
    let rooms = client.list_invited_rooms().await.unwrap();
    assert_eq!(rooms.len(), 2);
}

#[tokio::test]
async fn get_room_topic_returns_topic() {
    let mut server = mockito::Server::new_async().await;
    let _m = server
        .mock("GET", "/_matrix/client/v3/rooms/%21abc%3Aexample.org/state/m.room.topic/")
        .with_status(200)
        .with_body(r#"{"topic":"org.mxdx.launcher.exec:test"}"#)
        .create_async().await;
    let client = RestClient::new(&server.url(), "tok");
    let rid = matrix_sdk::ruma::RoomId::parse("!abc:example.org").unwrap();
    let topic = client.get_room_topic(&rid).await.unwrap();
    assert_eq!(topic.as_deref(), Some("org.mxdx.launcher.exec:test"));
}

#[tokio::test]
async fn get_room_topic_404_returns_none() {
    let mut server = mockito::Server::new_async().await;
    let _m = server
        .mock("GET", mockito::Matcher::Regex(r"^/_matrix/client/v3/rooms/.*/state/m.room.topic/".into()))
        .with_status(404)
        .with_body(r#"{"errcode":"M_NOT_FOUND"}"#)
        .create_async().await;
    let client = RestClient::new(&server.url(), "tok");
    let rid = matrix_sdk::ruma::RoomId::parse("!abc:example.org").unwrap();
    assert!(client.get_room_topic(&rid).await.unwrap().is_none());
}

#[tokio::test]
async fn get_room_encryption_accepts_canonical_key() {
    let mut server = mockito::Server::new_async().await;
    let _m = server
        .mock("GET", mockito::Matcher::Regex(r"^/_matrix/client/v3/rooms/.*/state/m.room.encryption/".into()))
        .with_status(200)
        .with_body(r#"{"algorithm":"m.megolm.v1.aes-sha2","encrypt_state_events":true}"#)
        .create_async().await;
    let client = RestClient::new(&server.url(), "tok");
    let rid = matrix_sdk::ruma::RoomId::parse("!abc:example.org").unwrap();
    let enc = client.get_room_encryption(&rid).await.unwrap().unwrap();
    assert_eq!(enc.algorithm, "m.megolm.v1.aes-sha2");
    assert!(enc.encrypt_state_events);
}

#[tokio::test]
async fn get_room_encryption_accepts_msc4362_key() {
    let mut server = mockito::Server::new_async().await;
    let _m = server
        .mock("GET", mockito::Matcher::Regex(r"^/_matrix/client/v3/rooms/.*/state/m.room.encryption/".into()))
        .with_status(200)
        .with_body(r#"{"algorithm":"m.megolm.v1.aes-sha2","io.element.msc4362.encrypt_state_events":true}"#)
        .create_async().await;
    let client = RestClient::new(&server.url(), "tok");
    let rid = matrix_sdk::ruma::RoomId::parse("!abc:example.org").unwrap();
    let enc = client.get_room_encryption(&rid).await.unwrap().unwrap();
    assert!(enc.encrypt_state_events);
}

#[tokio::test]
async fn get_room_tombstone_returns_replacement() {
    let mut server = mockito::Server::new_async().await;
    let _m = server
        .mock("GET", mockito::Matcher::Regex(r"^/_matrix/client/v3/rooms/.*/state/m.room.tombstone/".into()))
        .with_status(200)
        .with_body(r#"{"replacement_room":"!new:example.org","body":"replaced"}"#)
        .create_async().await;
    let client = RestClient::new(&server.url(), "tok");
    let rid = matrix_sdk::ruma::RoomId::parse("!old:example.org").unwrap();
    let r = client.get_room_tombstone(&rid).await.unwrap();
    assert_eq!(r.unwrap().as_str(), "!new:example.org");
}
