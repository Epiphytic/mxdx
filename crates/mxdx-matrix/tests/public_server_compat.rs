//! Public Matrix server compatibility tests.
//!
//! Tests that verify MatrixClient works against a public homeserver
//! (e.g., matrix.org). Covers login, E2EE, encrypted rooms, custom events,
//! state events, sync, spaces, and tombstones.
//!
//! All tests are `#[ignore]` by default since they require real credentials.
//!
//! ## Setup
//!
//! Create `test-credentials.toml` in the repo root (gitignored):
//!
//! ```toml
//! [server]
//! url = "matrix.org"
//!
//! [account1]
//! username = "your-user"
//! password = "your-password"
//!
//! [account2]
//! username = "your-other-user"
//! password = "your-password"
//! ```
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

// ─── Login Tests (no room creation) ─────────────────────────────────────────

#[tokio::test]
#[ignore = "requires test-credentials.toml or env vars"]
async fn login_account1() {
    let (creds, _) = load_credentials();
    let client = MatrixClient::login_and_connect(&creds.hs_url, &creds.username, &creds.password)
        .await
        .expect("Account 1 login failed");
    assert!(client.is_logged_in());
    assert!(client.crypto_enabled().await);
}

#[tokio::test]
#[ignore = "requires test-credentials.toml or env vars"]
async fn login_account2() {
    let (_, creds2) = load_credentials();
    let creds = creds2.expect("Account 2 credentials required");
    let client = MatrixClient::login_and_connect(&creds.hs_url, &creds.username, &creds.password)
        .await
        .expect("Account 2 login failed");
    assert!(client.is_logged_in());
    assert!(client.crypto_enabled().await);
}

// ─── Room Tests (single test to minimize rate-limit pressure) ────────────────
//
// Public servers like matrix.org aggressively rate-limit room creation.
// We combine all room-related assertions into one test that creates rooms
// once with delays between operations.

#[tokio::test]
#[ignore = "requires test-credentials.toml or env vars — creates rooms on public server"]
async fn room_operations() {
    let (creds, _) = load_credentials();
    let mut client = MatrixClient::login_and_connect(&creds.hs_url, &creds.username, &creds.password)
        .await
        .expect("Login failed");
    client.set_room_creation_timeout(Duration::from_secs(120));

    // ── 1. Create encrypted room ──────────────────────────────────────
    eprintln!("[1/7] Creating encrypted room...");
    let room_id = client
        .create_encrypted_room(&[])
        .await
        .expect("Encrypted room creation should succeed");
    assert!(!room_id.as_str().is_empty());
    eprintln!("  Room: {room_id}");

    tokio::time::sleep(Duration::from_secs(2)).await;

    // ── 2. Send custom event ──────────────────────────────────────────
    eprintln!("[2/7] Sending custom event...");
    client.sync_once().await.unwrap();

    let test_uuid = format!("compat-{}", uuid::Uuid::new_v4());
    let content = serde_json::json!({
        "type": "org.mxdx.command",
        "content": {
            "uuid": test_uuid,
            "action": "exec",
            "cmd": "echo",
            "args": ["hello"],
        }
    });
    client
        .send_event(&room_id, content)
        .await
        .expect("Sending custom event should succeed");

    // ── 3. Sync and receive the event back ────────────────────────────
    eprintln!("[3/7] Syncing to receive event...");
    let events = client
        .sync_and_collect_events(&room_id, Duration::from_secs(15))
        .await
        .expect("sync_and_collect_events should succeed");

    let found = events.iter().any(|e| {
        e.get("content")
            .and_then(|c| c.get("uuid"))
            .and_then(|u| u.as_str())
            == Some(&test_uuid)
    });
    assert!(found, "Should find the custom event via sync_and_collect_events");

    tokio::time::sleep(Duration::from_secs(2)).await;

    // ── 4. Send state event ───────────────────────────────────────────
    eprintln!("[4/7] Sending state event...");
    let state_content = serde_json::json!({
        "status": "online",
        "version": "0.1.0"
    });
    client
        .send_state_event(&room_id, "org.mxdx.launcher.status", "", state_content)
        .await
        .expect("Sending custom state event should succeed");

    tokio::time::sleep(Duration::from_secs(2)).await;

    // ── 5. Create terminal session DM ─────────────────────────────────
    eprintln!("[5/7] Creating terminal session DM (waiting 5s for rate limit)...");
    tokio::time::sleep(Duration::from_secs(5)).await;
    let dm_room_id = client
        .create_terminal_session_dm(client.user_id())
        .await
        .expect("Terminal session DM creation should succeed");

    client.sync_once().await.unwrap();

    let state = client
        .get_room_state(&dm_room_id, "m.room.history_visibility")
        .await
        .expect("Should read room state");
    assert_eq!(
        state["history_visibility"], "joined",
        "history_visibility should be 'joined'"
    );

    tokio::time::sleep(Duration::from_secs(2)).await;

    // ── 6. Tombstone room ─────────────────────────────────────────────
    eprintln!("[6/7] Tombstoning room...");
    // Use the first room as old, the DM as the "replacement" (just for the test)
    client
        .tombstone_room(&room_id, &dm_room_id)
        .await
        .expect("Tombstone should succeed");

    // ── 7. Cleanup ────────────────────────────────────────────────────
    eprintln!("[7/7] Cleaning up...");
    for rid in [&room_id, &dm_room_id] {
        if let Some(room) = client.inner().get_room(rid) {
            let _ = room.leave().await;
            let _ = room.forget().await;
        }
    }

    eprintln!("[✓] All room operations passed");
}

#[tokio::test]
#[ignore = "requires test-credentials.toml or env vars — creates launcher space on public server"]
async fn launcher_space_operations() {
    let (creds, _) = load_credentials();
    let mut client = MatrixClient::login_and_connect(&creds.hs_url, &creds.username, &creds.password)
        .await
        .expect("Login failed");
    client.set_room_creation_delay(Some(Duration::from_secs(3)));
    client.set_room_creation_timeout(Duration::from_secs(120));

    let launcher_id = format!("compat-{}", uuid::Uuid::new_v4().as_simple());

    // ── 1. Create launcher space ──────────────────────────────────────
    eprintln!("[1/3] Creating launcher space '{launcher_id}'...");
    let topology = client
        .create_launcher_space(&launcher_id)
        .await
        .expect("Launcher space creation should succeed");

    assert!(!topology.space_id.as_str().is_empty());
    assert!(!topology.exec_room_id.as_str().is_empty());
    assert!(!topology.status_room_id.as_str().is_empty());
    assert!(!topology.logs_room_id.as_str().is_empty());
    eprintln!("  Space: {}", topology.space_id);

    // ── 2. Find launcher space ────────────────────────────────────────
    eprintln!("[2/3] Finding launcher space...");
    let found = client
        .find_launcher_space(&launcher_id)
        .await
        .expect("find_launcher_space should succeed")
        .expect("Should find the launcher space we just created");

    assert_eq!(found.space_id, topology.space_id);
    assert_eq!(found.exec_room_id, topology.exec_room_id);

    // ── 3. Cleanup ────────────────────────────────────────────────────
    eprintln!("[3/3] Cleaning up...");
    for rid in [&topology.space_id, &topology.exec_room_id, &topology.status_room_id, &topology.logs_room_id] {
        if let Some(room) = client.inner().get_room(rid) {
            let _ = room.leave().await;
            let _ = room.forget().await;
        }
    }

    eprintln!("[✓] Launcher space operations passed");
}
