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
