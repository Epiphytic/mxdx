use std::collections::HashSet;
use std::time::Duration;

use age::x25519::Identity;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;

use mxdx_matrix::MatrixClient;
use mxdx_secrets::coordinator::{decrypt_with_identity, SecretCoordinator};
use mxdx_secrets::store::SecretStore;
use mxdx_test_helpers::tuwunel::TuwunelInstance;
use mxdx_types::events::secret::{SecretRequestEvent, SecretResponseEvent};

#[tokio::test]
async fn worker_requests_secret_with_double_encryption() {
    let mut hs = TuwunelInstance::start().await.unwrap();
    let base_url = format!("http://127.0.0.1:{}", hs.port);

    let coordinator_client =
        MatrixClient::register_and_connect(&base_url, "coordinator", "pass", "mxdx-test-token")
            .await
            .unwrap();
    let worker_client =
        MatrixClient::register_and_connect(&base_url, "worker", "pass", "mxdx-test-token")
            .await
            .unwrap();

    let room_id = coordinator_client
        .create_encrypted_room(&[worker_client.user_id().to_owned()])
        .await
        .unwrap();
    worker_client.join_room(&room_id).await.unwrap();

    // Key exchange sync rounds
    coordinator_client.sync_once().await.unwrap();
    worker_client.sync_once().await.unwrap();
    coordinator_client.sync_once().await.unwrap();
    worker_client.sync_once().await.unwrap();

    // Set up coordinator with a secret
    let mut store = SecretStore::new(Identity::generate());
    store.add("github.token", "ghp_live_token_xyz").unwrap();
    let authorized: HashSet<String> = ["github.token".to_string()].into();
    let coordinator = SecretCoordinator::new(store, authorized);

    // Worker generates ephemeral keypair
    let ephemeral_identity = Identity::generate();
    let ephemeral_pubkey = ephemeral_identity.to_public().to_string();

    // Worker sends request over Matrix
    let request = SecretRequestEvent {
        request_id: "req-e2e-001".into(),
        scope: "github.token".into(),
        ttl_seconds: 300,
        reason: "e2e test".into(),
        ephemeral_public_key: ephemeral_pubkey,
    };

    worker_client
        .send_event(
            &room_id,
            serde_json::json!({
                "type": "org.mxdx.secret.request",
                "content": request,
            }),
        )
        .await
        .unwrap();

    // Coordinator syncs and picks up the request
    let events = coordinator_client
        .sync_and_collect_events(&room_id, Duration::from_secs(10))
        .await
        .unwrap();

    let request_event = events
        .iter()
        .find(|e| e.get("type").and_then(|t| t.as_str()) == Some("org.mxdx.secret.request"))
        .expect("coordinator should see secret request event");

    let received_request: SecretRequestEvent =
        serde_json::from_value(request_event["content"].clone()).unwrap();
    assert_eq!(received_request.request_id, "req-e2e-001");

    // Coordinator handles the request (double encryption)
    let response = coordinator.handle_secret_request(&received_request);
    assert!(response.granted);
    assert!(response.encrypted_value.is_some());

    // Coordinator sends response over Matrix
    coordinator_client
        .send_event(
            &room_id,
            serde_json::json!({
                "type": "org.mxdx.secret.response",
                "content": response,
            }),
        )
        .await
        .unwrap();

    // Worker syncs and picks up the response
    let events = worker_client
        .sync_and_collect_events(&room_id, Duration::from_secs(10))
        .await
        .unwrap();

    let response_event = events
        .iter()
        .find(|e| e.get("type").and_then(|t| t.as_str()) == Some("org.mxdx.secret.response"))
        .expect("worker should see secret response event");

    let received_response: SecretResponseEvent =
        serde_json::from_value(response_event["content"].clone()).unwrap();
    assert!(received_response.granted);

    // Worker decrypts with ephemeral private key
    let ciphertext = BASE64
        .decode(received_response.encrypted_value.as_ref().unwrap())
        .unwrap();
    let plaintext = decrypt_with_identity(&ephemeral_identity, &ciphertext).unwrap();
    assert_eq!(String::from_utf8(plaintext).unwrap(), "ghp_live_token_xyz");

    hs.stop().await;
}

#[tokio::test]
async fn unauthorized_worker_cannot_get_secret() {
    let mut hs = TuwunelInstance::start().await.unwrap();
    let base_url = format!("http://127.0.0.1:{}", hs.port);

    let coordinator_client =
        MatrixClient::register_and_connect(&base_url, "coord2", "pass", "mxdx-test-token")
            .await
            .unwrap();
    let worker_client =
        MatrixClient::register_and_connect(&base_url, "worker2", "pass", "mxdx-test-token")
            .await
            .unwrap();

    let room_id = coordinator_client
        .create_encrypted_room(&[worker_client.user_id().to_owned()])
        .await
        .unwrap();
    worker_client.join_room(&room_id).await.unwrap();

    // Key exchange sync rounds
    coordinator_client.sync_once().await.unwrap();
    worker_client.sync_once().await.unwrap();
    coordinator_client.sync_once().await.unwrap();
    worker_client.sync_once().await.unwrap();

    // Coordinator only authorizes "github.token", not "aws.secret_key"
    let mut store = SecretStore::new(Identity::generate());
    store.add("github.token", "ghp_live_token_xyz").unwrap();
    let authorized: HashSet<String> = ["github.token".to_string()].into();
    let coordinator = SecretCoordinator::new(store, authorized);

    let ephemeral_identity = Identity::generate();

    // Worker requests an unauthorized scope
    let request = SecretRequestEvent {
        request_id: "req-e2e-002".into(),
        scope: "aws.secret_key".into(),
        ttl_seconds: 300,
        reason: "e2e test unauthorized".into(),
        ephemeral_public_key: ephemeral_identity.to_public().to_string(),
    };

    worker_client
        .send_event(
            &room_id,
            serde_json::json!({
                "type": "org.mxdx.secret.request",
                "content": request,
            }),
        )
        .await
        .unwrap();

    // Coordinator syncs and picks up the request
    let events = coordinator_client
        .sync_and_collect_events(&room_id, Duration::from_secs(10))
        .await
        .unwrap();

    let request_event = events
        .iter()
        .find(|e| e.get("type").and_then(|t| t.as_str()) == Some("org.mxdx.secret.request"))
        .expect("coordinator should see secret request event");

    let received_request: SecretRequestEvent =
        serde_json::from_value(request_event["content"].clone()).unwrap();

    let response = coordinator.handle_secret_request(&received_request);
    assert!(!response.granted);
    assert!(response.encrypted_value.is_none());
    assert!(response
        .error
        .as_ref()
        .unwrap()
        .contains("unauthorized scope"));

    hs.stop().await;
}
