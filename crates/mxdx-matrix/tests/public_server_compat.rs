//! Public Matrix server compatibility tests.
//!
//! These tests verify that mxdx's MatrixClient facade works against public
//! Matrix homeservers (e.g., matrix.org) rather than just local Tuwunel instances.
//!
//! All tests are `#[ignore]` by default since they require real credentials.
//!
//! ## Setup
//!
//! 1. Register two accounts on a public Matrix server (e.g., matrix.org)
//! 2. Create `test-credentials.toml` in the repo root (gitignored):
//!
//!    ```toml
//!    [server]
//!    url = "https://matrix-client.matrix.org"
//!
//!    [account1]
//!    username = "mxdx-test-user1"
//!    password = "your-password-here"
//!
//!    [account2]
//!    username = "mxdx-test-user2"
//!    password = "your-password-here"
//!    ```
//!
//! 3. Or set environment variables:
//!    - MXDX_PUBLIC_HS_URL
//!    - MXDX_PUBLIC_USERNAME / MXDX_PUBLIC_PASSWORD
//!    - MXDX_PUBLIC_USERNAME2 / MXDX_PUBLIC_PASSWORD2
//!
//! Run with: cargo test -p mxdx-matrix --test public_server_compat -- --ignored

use std::time::Duration;
use mxdx_matrix::MatrixClient;

/// Credentials for a single account.
struct Credentials {
    hs_url: String,
    username: String,
    password: String,
}

/// Load credentials from test-credentials.toml or environment variables.
fn load_credentials() -> (Credentials, Option<Credentials>) {
    // Try TOML file first
    let toml_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("test-credentials.toml");

    if toml_path.exists() {
        let content = std::fs::read_to_string(&toml_path)
            .expect("Failed to read test-credentials.toml");
        let parsed: toml::Value = content.parse()
            .expect("Failed to parse test-credentials.toml");

        let hs_url = parsed["server"]["url"].as_str()
            .expect("server.url missing in test-credentials.toml")
            .to_string();

        let account1 = Credentials {
            hs_url: hs_url.clone(),
            username: parsed["account1"]["username"].as_str()
                .expect("account1.username missing").to_string(),
            password: parsed["account1"]["password"].as_str()
                .expect("account1.password missing").to_string(),
        };

        let account2 = parsed.get("account2").map(|a| Credentials {
            hs_url: hs_url.clone(),
            username: a["username"].as_str()
                .expect("account2.username missing").to_string(),
            password: a["password"].as_str()
                .expect("account2.password missing").to_string(),
        });

        return (account1, account2);
    }

    // Fall back to environment variables
    let hs_url = std::env::var("MXDX_PUBLIC_HS_URL")
        .expect("Set MXDX_PUBLIC_HS_URL or create test-credentials.toml");
    let account1 = Credentials {
        hs_url: hs_url.clone(),
        username: std::env::var("MXDX_PUBLIC_USERNAME")
            .expect("Set MXDX_PUBLIC_USERNAME"),
        password: std::env::var("MXDX_PUBLIC_PASSWORD")
            .expect("Set MXDX_PUBLIC_PASSWORD"),
    };
    let account2 = std::env::var("MXDX_PUBLIC_USERNAME2").ok().map(|u| Credentials {
        hs_url,
        username: u,
        password: std::env::var("MXDX_PUBLIC_PASSWORD2")
            .expect("Set MXDX_PUBLIC_PASSWORD2 if MXDX_PUBLIC_USERNAME2 is set"),
    });

    (account1, account2)
}

/// Connect account1 via MatrixClient::login_and_connect.
async fn connect_account1() -> MatrixClient {
    let (creds, _) = load_credentials();
    MatrixClient::login_and_connect(&creds.hs_url, &creds.username, &creds.password)
        .await
        .expect("Account 1 login failed — check credentials")
}

/// Connect both accounts.
async fn connect_both() -> (MatrixClient, MatrixClient) {
    let (creds1, creds2) = load_credentials();
    let creds2 = creds2.expect("Account 2 credentials required for this test");

    let client1 = MatrixClient::login_and_connect(&creds1.hs_url, &creds1.username, &creds1.password)
        .await
        .expect("Account 1 login failed");
    let client2 = MatrixClient::login_and_connect(&creds2.hs_url, &creds2.username, &creds2.password)
        .await
        .expect("Account 2 login failed");

    (client1, client2)
}

// ─── Single-account tests ────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires test-credentials.toml or env vars"]
async fn login_and_connect_succeeds() {
    let client = connect_account1().await;
    assert!(client.is_logged_in());
    assert!(client.crypto_enabled().await);
}

#[tokio::test]
#[ignore = "requires test-credentials.toml or env vars"]
async fn create_encrypted_room() {
    let client = connect_account1().await;

    let room_id = client.create_encrypted_room(&[]).await
        .expect("Should create encrypted room on public server");
    assert!(!room_id.as_str().is_empty());

    // Leave room to clean up
    let _ = client.inner().get_room(&room_id).map(|r| {
        tokio::spawn(async move { let _ = r.leave().await; })
    });
}

#[tokio::test]
#[ignore = "requires test-credentials.toml or env vars"]
async fn send_custom_event_in_encrypted_room() {
    let client = connect_account1().await;

    let room_id = client.create_encrypted_room(&[]).await.unwrap();

    // Sync so we know about the room
    client.sync_once().await.unwrap();

    // Send a custom org.mxdx.command event
    let payload = serde_json::json!({
        "type": "org.mxdx.command",
        "content": {
            "uuid": "compat-test-001",
            "action": "exec",
            "cmd": "echo",
            "args": ["hello"],
            "env": {},
            "timeout_seconds": 10
        }
    });

    client.send_event(&room_id, payload).await
        .expect("Sending custom event should succeed on public server");

    let _ = client.inner().get_room(&room_id).map(|r| {
        tokio::spawn(async move { let _ = r.leave().await; })
    });
}

#[tokio::test]
#[ignore = "requires test-credentials.toml or env vars"]
async fn send_custom_state_event() {
    let client = connect_account1().await;

    let room_id = client.create_encrypted_room(&[]).await.unwrap();
    client.sync_once().await.unwrap();

    client.send_state_event(
        &room_id,
        "org.mxdx.launcher.status",
        "",
        serde_json::json!({ "status": "online", "version": "0.1.0" }),
    ).await.expect("Custom state event should succeed on public server");

    let _ = client.inner().get_room(&room_id).map(|r| {
        tokio::spawn(async move { let _ = r.leave().await; })
    });
}

#[tokio::test]
#[ignore = "requires test-credentials.toml or env vars"]
async fn sync_and_receive_own_custom_events() {
    let client = connect_account1().await;

    let room_id = client.create_encrypted_room(&[]).await.unwrap();
    client.sync_once().await.unwrap();

    let test_uuid = format!("compat-recv-{}", uuid::Uuid::new_v4());
    let payload = serde_json::json!({
        "type": "org.mxdx.command",
        "content": {
            "uuid": test_uuid,
            "action": "exec",
            "cmd": "echo",
            "args": ["test"],
            "env": {}
        }
    });
    client.send_event(&room_id, payload).await.unwrap();

    // Collect events — should find our custom event
    let events = client.sync_and_collect_events(&room_id, Duration::from_secs(10)).await.unwrap();
    let found = events.iter().any(|e| {
        e.get("content")
            .and_then(|c| c.get("uuid"))
            .and_then(|u| u.as_str())
            == Some(&test_uuid)
    });

    assert!(found, "Should receive custom event back via sync_and_collect_events");

    let _ = client.inner().get_room(&room_id).map(|r| {
        tokio::spawn(async move { let _ = r.leave().await; })
    });
}

// ─── Two-account tests ───────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires two accounts in test-credentials.toml or env vars"]
async fn two_users_encrypted_room_with_invite() {
    let (client1, client2) = connect_both().await;

    // Client 1 creates encrypted room and invites client 2
    let room_id = client1
        .create_encrypted_room(&[client2.user_id().to_owned()])
        .await
        .expect("Should create room with invite on public server");

    // Client 2 joins
    client2.join_room(&room_id).await
        .expect("Invited user should join successfully");

    // Both sync to exchange keys
    client1.sync_once().await.unwrap();
    client2.sync_once().await.unwrap();
    client1.sync_once().await.unwrap();
    client2.sync_once().await.unwrap();

    // Client 1 sends a custom event
    let test_uuid = format!("two-user-{}", uuid::Uuid::new_v4());
    let payload = serde_json::json!({
        "type": "org.mxdx.command",
        "content": {
            "uuid": test_uuid,
            "action": "exec",
            "cmd": "echo",
            "args": ["cross-user-test"],
            "env": {}
        }
    });
    client1.send_event(&room_id, payload).await
        .expect("Sender should send custom event");

    // Client 2 receives it
    let events = client2.sync_and_collect_events(&room_id, Duration::from_secs(15)).await.unwrap();
    let found = events.iter().any(|e| {
        e.get("content")
            .and_then(|c| c.get("uuid"))
            .and_then(|u| u.as_str())
            == Some(&test_uuid)
    });

    assert!(found, "Second user should receive the E2EE custom event from first user");

    // Cleanup
    let _ = client1.inner().get_room(&room_id).map(|r| {
        tokio::spawn(async move { let _ = r.leave().await; })
    });
    let _ = client2.inner().get_room(&room_id).map(|r| {
        tokio::spawn(async move { let _ = r.leave().await; })
    });
}

#[tokio::test]
#[ignore = "requires two accounts in test-credentials.toml or env vars"]
async fn terminal_dm_with_history_visibility() {
    let (client1, client2) = connect_both().await;

    // Client 1 creates a terminal session DM (encrypted, direct, history_visibility=joined)
    let room_id = client1
        .create_terminal_session_dm(client2.user_id())
        .await
        .expect("Should create terminal DM on public server");

    client2.join_room(&room_id).await.unwrap();
    client1.sync_once().await.unwrap();

    // Verify history_visibility via get_room_state
    let state = client1.get_room_state(&room_id, "m.room.history_visibility").await.unwrap();
    assert_eq!(state["history_visibility"], "joined");

    // Cleanup
    let _ = client1.inner().get_room(&room_id).map(|r| {
        tokio::spawn(async move { let _ = r.leave().await; })
    });
    let _ = client2.inner().get_room(&room_id).map(|r| {
        tokio::spawn(async move { let _ = r.leave().await; })
    });
}
