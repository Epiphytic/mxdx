# Key Backup, Re-Encryption, and REST-Based Room Discovery — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Eliminate duplicate-room creation, key-loss-on-restart, and unencrypted-space-room bugs by introducing server-side megolm key backup, room re-encryption self-healing, and a REST-based room discovery layer that bypasses the matrix-sdk's stale local cache.

**Architecture:** Three new `mxdx-matrix` modules: `rest.rs` (direct Matrix C-S API helpers, no SDK), `backup.rs` (matrix-sdk Encryption::backups facade with chained-keychain recovery key storage), `reencrypt.rs` (idempotent room replacement). Wired into worker `connect()` and client daemon startup. Diagnose tool gets a `--decrypt` opt-in flag using a throwaway temp-store SDK Client.

**Tech Stack:** Rust, matrix-sdk 0.16, reqwest, serde_json, mockito (test-only), MSC4362 encrypted state events.

**Spec:** `docs/superpowers/specs/2026-04-07-key-backup-and-encryption-fixes-design.md`

---

## Pre-flight

- [ ] **Read the spec.** `cat docs/superpowers/specs/2026-04-07-key-backup-and-encryption-fixes-design.md`
- [ ] **Verify clean working tree.** `git status` should show no unstaged changes that conflict with new work.
- [ ] **Confirm release binaries build.** `cargo build --release -p mxdx-worker -p mxdx-client 2>&1 | tail -5` should succeed.

---

## Task 1: `rest.rs` skeleton + `list_joined_rooms` / `list_invited_rooms`

**Files:**
- Create: `crates/mxdx-matrix/src/rest.rs`
- Modify: `crates/mxdx-matrix/src/lib.rs` (add `pub mod rest;`)
- Create: `crates/mxdx-matrix/tests/rest_test.rs`
- Modify: `crates/mxdx-matrix/Cargo.toml` (dev-deps: `mockito = "1"`)

- [ ] **Step 1: Add mockito dev-dep**

In `crates/mxdx-matrix/Cargo.toml`, under `[dev-dependencies]`:
```toml
mockito = "1"
tokio = { workspace = true, features = ["macros", "rt-multi-thread"] }
```

- [ ] **Step 2: Write the failing test**

Create `crates/mxdx-matrix/tests/rest_test.rs`:
```rust
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
```

- [ ] **Step 3: Run the test, expect failure**

`cargo test -p mxdx-matrix --test rest_test 2>&1 | tail -20`
Expected: compile error (`mxdx_matrix::rest` doesn't exist).

- [ ] **Step 4: Create the `rest.rs` module**

Create `crates/mxdx-matrix/src/rest.rs`:
```rust
//! Direct Matrix client-server REST API helpers.
//!
//! This module deliberately does NOT use matrix-sdk. It exists for two reasons:
//!   1. Workers/clients need to discover rooms even when the SDK's local cache
//!      is stale (after session restore, before initial sync).
//!   2. Diagnostic and cleanup tools must work without conflicting with a
//!      running daemon's crypto store.
//!
//! All requests have a 10s timeout. All errors are returned as `Result`.

use anyhow::{Context, Result};
use matrix_sdk::ruma::{OwnedRoomId, RoomId};
use serde::Deserialize;
use std::time::Duration;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

pub struct RestClient {
    homeserver: String,
    access_token: String,
    http: reqwest::Client,
}

#[derive(Debug, Clone)]
pub struct EncryptionState {
    pub algorithm: String,
    pub encrypt_state_events: bool,
}

impl RestClient {
    pub fn new(homeserver: &str, access_token: &str) -> Self {
        Self {
            homeserver: homeserver.trim_end_matches('/').to_string(),
            access_token: access_token.to_string(),
            http: reqwest::Client::builder()
                .timeout(DEFAULT_TIMEOUT)
                .build()
                .expect("reqwest client build"),
        }
    }

    pub async fn list_joined_rooms(&self) -> Result<Vec<OwnedRoomId>> {
        #[derive(Deserialize)]
        struct R { joined_rooms: Vec<String> }
        let url = format!("{}/_matrix/client/v3/joined_rooms", self.homeserver);
        let resp = self.http.get(&url).bearer_auth(&self.access_token).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("list_joined_rooms: HTTP {}", resp.status());
        }
        let r: R = resp.json().await?;
        r.joined_rooms.into_iter()
            .map(|s| RoomId::parse(&s).map(|id| id.to_owned()).context("invalid room id"))
            .collect()
    }

    pub async fn list_invited_rooms(&self) -> Result<Vec<OwnedRoomId>> {
        let url = format!("{}/_matrix/client/v3/sync?timeout=0", self.homeserver);
        let resp = self.http.get(&url).bearer_auth(&self.access_token).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("list_invited_rooms (sync): HTTP {}", resp.status());
        }
        let v: serde_json::Value = resp.json().await?;
        let invite = v.get("rooms").and_then(|r| r.get("invite")).and_then(|i| i.as_object());
        let mut out = Vec::new();
        if let Some(map) = invite {
            for (rid, _) in map {
                if let Ok(id) = RoomId::parse(rid) {
                    out.push(id.to_owned());
                }
            }
        }
        Ok(out)
    }
}
```

- [ ] **Step 5: Wire module into lib.rs**

In `crates/mxdx-matrix/src/lib.rs`, add `pub mod rest;` near the other `pub mod` lines.

- [ ] **Step 6: Run the tests, expect pass**

`cargo test -p mxdx-matrix --test rest_test 2>&1 | tail -20`
Expected: `2 passed`.

- [ ] **Step 7: Commit**

```bash
git add crates/mxdx-matrix/Cargo.toml crates/mxdx-matrix/src/lib.rs \
  crates/mxdx-matrix/src/rest.rs crates/mxdx-matrix/tests/rest_test.rs
git commit -m "feat(mxdx-matrix): add rest module with joined/invited room listing"
```

---

## Task 2: `rest.rs` room state helpers (topic, name, encryption, tombstone)

**Files:**
- Modify: `crates/mxdx-matrix/src/rest.rs`
- Modify: `crates/mxdx-matrix/tests/rest_test.rs`

- [ ] **Step 1: Write failing tests**

Append to `crates/mxdx-matrix/tests/rest_test.rs`:
```rust
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
```

- [ ] **Step 2: Run, confirm failure**

`cargo test -p mxdx-matrix --test rest_test 2>&1 | tail -20`
Expected: compile error (methods don't exist).

- [ ] **Step 3: Add methods to `RestClient`**

Append to `impl RestClient` in `crates/mxdx-matrix/src/rest.rs`:
```rust
    fn state_url(&self, room: &RoomId, event_type: &str) -> String {
        let encoded = urlencoding::encode(room.as_str());
        format!("{}/_matrix/client/v3/rooms/{}/state/{}/", self.homeserver, encoded, event_type)
    }

    pub async fn get_room_topic(&self, room: &RoomId) -> Result<Option<String>> {
        let url = self.state_url(room, "m.room.topic");
        let resp = self.http.get(&url).bearer_auth(&self.access_token).send().await?;
        if resp.status().as_u16() == 404 {
            return Ok(None);
        }
        if !resp.status().is_success() {
            anyhow::bail!("get_room_topic: HTTP {}", resp.status());
        }
        let v: serde_json::Value = resp.json().await?;
        Ok(v.get("topic").and_then(|t| t.as_str()).map(String::from))
    }

    pub async fn get_room_name(&self, room: &RoomId) -> Result<Option<String>> {
        let url = self.state_url(room, "m.room.name");
        let resp = self.http.get(&url).bearer_auth(&self.access_token).send().await?;
        if resp.status().as_u16() == 404 {
            return Ok(None);
        }
        if !resp.status().is_success() {
            anyhow::bail!("get_room_name: HTTP {}", resp.status());
        }
        let v: serde_json::Value = resp.json().await?;
        Ok(v.get("name").and_then(|n| n.as_str()).map(String::from))
    }

    pub async fn get_room_encryption(&self, room: &RoomId) -> Result<Option<EncryptionState>> {
        let url = self.state_url(room, "m.room.encryption");
        let resp = self.http.get(&url).bearer_auth(&self.access_token).send().await?;
        if resp.status().as_u16() == 404 {
            return Ok(None);
        }
        if !resp.status().is_success() {
            anyhow::bail!("get_room_encryption: HTTP {}", resp.status());
        }
        let v: serde_json::Value = resp.json().await?;
        let algorithm = v.get("algorithm").and_then(|a| a.as_str()).unwrap_or("").to_string();
        let encrypt_state_events = v.get("encrypt_state_events").and_then(|b| b.as_bool())
            .or_else(|| v.get("io.element.msc4362.encrypt_state_events").and_then(|b| b.as_bool()))
            .unwrap_or(false);
        Ok(Some(EncryptionState { algorithm, encrypt_state_events }))
    }

    pub async fn get_room_tombstone(&self, room: &RoomId) -> Result<Option<OwnedRoomId>> {
        let url = self.state_url(room, "m.room.tombstone");
        let resp = self.http.get(&url).bearer_auth(&self.access_token).send().await?;
        if resp.status().as_u16() == 404 {
            return Ok(None);
        }
        if !resp.status().is_success() {
            anyhow::bail!("get_room_tombstone: HTTP {}", resp.status());
        }
        let v: serde_json::Value = resp.json().await?;
        let replacement = v.get("replacement_room").and_then(|r| r.as_str());
        match replacement {
            Some(s) => Ok(Some(RoomId::parse(s)?.to_owned())),
            None => Ok(None),
        }
    }
```

Add `urlencoding = "2"` to `[dependencies]` in `crates/mxdx-matrix/Cargo.toml` if not already present.

- [ ] **Step 4: Run tests, expect pass**

`cargo test -p mxdx-matrix --test rest_test 2>&1 | tail -20`
Expected: 7 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/mxdx-matrix/src/rest.rs crates/mxdx-matrix/tests/rest_test.rs crates/mxdx-matrix/Cargo.toml
git commit -m "feat(mxdx-matrix): add room state REST helpers (topic/name/encryption/tombstone)"
```

---

## Task 3: Refactor `cleanup.rs` onto shared `rest::*` helpers

**Files:**
- Modify: `crates/mxdx-client/src/cleanup.rs`
- Modify: `crates/mxdx-client/Cargo.toml` (add `mxdx-matrix` if not present)

- [ ] **Step 1: Verify mxdx-client depends on mxdx-matrix**

`grep mxdx-matrix crates/mxdx-client/Cargo.toml`
If absent, add under `[dependencies]`: `mxdx-matrix = { path = "../mxdx-matrix", version = "1.1.0" }`

- [ ] **Step 2: Replace local list_joined_rooms with rest::list_joined_rooms**

In `crates/mxdx-client/src/cleanup.rs`:
1. Add `use mxdx_matrix::rest::RestClient;` near top.
2. DELETE the local `list_joined_rooms` and `list_invited_rooms` functions (lines defining each `async fn list_joined_rooms` and `async fn list_invited_rooms`).
3. In every site that calls those, construct `let rest = RestClient::new(homeserver, access_token);` once at the top of `run_cleanup` (after the args are parsed) and call `rest.list_joined_rooms().await?` / `rest.list_invited_rooms().await?` instead. Convert the returned `Vec<OwnedRoomId>` to `Vec<String>` via `.into_iter().map(|r| r.to_string()).collect()` if `leave_and_forget` still takes `&str`.

- [ ] **Step 3: cargo check**

`cargo check -p mxdx-client 2>&1 | tail -20`
Expected: clean.

- [ ] **Step 4: Manual smoke test**

```bash
cargo build --release -p mxdx-client 2>&1 | tail -5
./target/release/mxdx-client --no-daemon \
  --homeserver https://ca1-beta.mxdx.dev \
  --username e2etest-test1 --password 'mxdx-e2e-test-2026!' \
  cleanup rooms --force 2>&1 | tail -10
```
Expected: "Found N joined room(s) and M invited room(s)" line appears, exit 0.

- [ ] **Step 5: Commit**

```bash
git add crates/mxdx-client/src/cleanup.rs crates/mxdx-client/Cargo.toml
git commit -m "refactor(mxdx-client): cleanup uses shared mxdx-matrix REST helpers"
```

---

## Task 4: Refactor `find_launcher_space` to use REST + tombstone follow

**Files:**
- Modify: `crates/mxdx-matrix/src/rooms.rs`

- [ ] **Step 1: Read current implementation**

`sed -n '90,200p' crates/mxdx-matrix/src/rooms.rs` — confirm signature and the SDK-cache enumeration at the loop reading `self.inner().joined_rooms()`.

- [ ] **Step 2: Add REST-based discovery method**

Add a new method to the impl block alongside `find_launcher_space`:
```rust
    /// Find the launcher topology by querying the Matrix REST API directly,
    /// bypassing the SDK's local cache. Follows m.room.tombstone chains to
    /// the latest replacement room.
    pub async fn find_launcher_space_via_rest(
        &self,
        launcher_id: &str,
        homeserver: &str,
        access_token: &str,
    ) -> Result<Option<LauncherTopology>> {
        use crate::rest::RestClient;
        let rest = RestClient::new(homeserver, access_token);

        let expected_space_topic = format!("org.mxdx.launcher.space:{launcher_id}");
        let expected_exec_topic = format!("org.mxdx.launcher.exec:{launcher_id}");
        let expected_logs_topic = format!("org.mxdx.launcher.logs:{launcher_id}");

        // Auto-accept any pending invites first (worker may have been re-invited).
        for invited in rest.list_invited_rooms().await.unwrap_or_default() {
            if let Err(e) = self.join_room(&invited).await {
                tracing::debug!(room_id=%invited, error=%e, "could not auto-join invited room");
            }
        }

        let joined = rest.list_joined_rooms().await?;
        let mut space: Option<OwnedRoomId> = None;
        let mut exec: Option<OwnedRoomId> = None;
        let mut logs: Option<OwnedRoomId> = None;

        for rid in joined {
            // Follow tombstones to the latest replacement.
            let mut current = rid.clone();
            for _ in 0..10 {
                match rest.get_room_tombstone(&current).await {
                    Ok(Some(replacement)) => {
                        tracing::debug!(old=%current, new=%replacement, "following tombstone");
                        current = replacement;
                    }
                    _ => break,
                }
            }
            let topic = match rest.get_room_topic(&current).await {
                Ok(Some(t)) => t,
                _ => continue,
            };
            if topic == expected_space_topic && space.is_none() {
                space = Some(current);
            } else if topic == expected_exec_topic && exec.is_none() {
                exec = Some(current);
            } else if topic == expected_logs_topic && logs.is_none() {
                logs = Some(current);
            }
        }

        match (space, exec, logs) {
            (Some(s), Some(e), Some(l)) => Ok(Some(LauncherTopology {
                space_id: s, exec_room_id: e, logs_room_id: l,
            })),
            (_, Some(e), _) => {
                // Exec exists; fall back: derive missing pieces from exec as the canonical room.
                tracing::warn!(launcher_id=%launcher_id, "partial topology — only exec room found");
                Ok(Some(LauncherTopology {
                    space_id: e.clone(), exec_room_id: e.clone(), logs_room_id: e,
                }))
            }
            _ => Ok(None),
        }
    }
```

- [ ] **Step 3: cargo check**

`cargo check -p mxdx-matrix 2>&1 | tail -20`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/mxdx-matrix/src/rooms.rs
git commit -m "feat(mxdx-matrix): add REST-based launcher space discovery with tombstone following"
```

---

## Task 5: Keychain helper for backup recovery key

**Files:**
- Modify: `crates/mxdx-types/src/identity.rs` (or wherever the existing keychain key helpers live)

- [ ] **Step 1: Find the existing keychain key helper**

`grep -rn "mxdx:session\|mxdx:" crates/mxdx-types/src/ | head`

- [ ] **Step 2: Add backup keychain key function**

In the file that defines the existing key helpers, add:
```rust
/// Compute the chained-keychain key for a backup recovery key, scoped per
/// (homeserver, matrix_user, unix_user) so multiple unix users on the same host
/// independently store their copy of the recovery key.
pub fn backup_keychain_key(homeserver: &str, matrix_user: &str, unix_user: &str) -> String {
    let host = homeserver.trim_end_matches('/').trim_start_matches("https://").trim_start_matches("http://");
    format!("mxdx:backup:{host}:{matrix_user}:{unix_user}")
}

#[cfg(test)]
#[test]
fn backup_keychain_key_format() {
    assert_eq!(
        backup_keychain_key("https://ca1-beta.mxdx.dev", "@alice:ca1-beta.mxdx.dev", "bob"),
        "mxdx:backup:ca1-beta.mxdx.dev:@alice:ca1-beta.mxdx.dev:bob"
    );
}
```

- [ ] **Step 3: cargo test**

`cargo test -p mxdx-types backup_keychain_key 2>&1 | tail -10`
Expected: 1 passed.

- [ ] **Step 4: Commit**

```bash
git add crates/mxdx-types/src/identity.rs
git commit -m "feat(mxdx-types): add backup_keychain_key helper for per-launcher recovery key storage"
```

---

## Task 6: `backup.rs` skeleton — first-run create flow

**Files:**
- Create: `crates/mxdx-matrix/src/backup.rs`
- Modify: `crates/mxdx-matrix/src/lib.rs` (add `pub mod backup;`)

- [ ] **Step 1: Create the module skeleton**

Create `crates/mxdx-matrix/src/backup.rs`:
```rust
//! Server-side megolm key backup facade.
//!
//! Wraps matrix-sdk's `Encryption::backups()` and `Encryption::recovery()`.
//! Stores the recovery key in the chained keychain under a per-launcher key
//! computed via `mxdx_types::identity::backup_keychain_key()`.
//!
//! See `docs/superpowers/specs/2026-04-07-key-backup-and-encryption-fixes-design.md`
//! for the full design.

use anyhow::{Context, Result};
use matrix_sdk::Client;
use matrix_sdk::ruma::UserId;
use mxdx_types::identity::{backup_keychain_key, KeychainBackend};

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct BackupState {
    pub enabled: bool,
    pub version: Option<String>,
    pub keys_downloaded: u64,
    pub degraded: bool,
    pub error: Option<String>,
}

/// Ensure server-side backup is set up. On first run, creates a new backup
/// version and stores the recovery key in the keychain. On subsequent runs,
/// loads the recovery key (from keychain or secret storage) and connects to
/// the existing backup.
///
/// `is_first_run`: if `true`, any failure is fatal (returns Err). If `false`,
/// failures are logged as WARN and the returned `BackupState` has
/// `degraded: true`.
pub async fn ensure_backup(
    client: &Client,
    keychain: &dyn KeychainBackend,
    server: &str,
    matrix_user: &UserId,
    unix_user: &str,
    is_first_run: bool,
) -> Result<BackupState> {
    let key = backup_keychain_key(server, matrix_user.as_str(), unix_user);

    let backups = client.encryption().backups();

    // Does a backup already exist on the server?
    let exists_on_server = backups.exists_on_server().await.unwrap_or(false);

    if !exists_on_server {
        return create_new_backup(client, keychain, &key, is_first_run).await;
    }

    // Backup exists. Try local keychain first, then secret storage.
    match load_from_keychain(client, keychain, &key).await {
        Ok(state) => Ok(state),
        Err(e_local) => {
            tracing::info!(error=%e_local, "local recovery key unavailable, trying secret storage");
            match load_from_secret_storage(client, keychain, &key).await {
                Ok(state) => Ok(state),
                Err(e_ss) => {
                    let msg = format!("local: {e_local}; secret-storage: {e_ss}");
                    if is_first_run {
                        anyhow::bail!("backup setup failed (first run): {msg}");
                    }
                    tracing::warn!(error=%msg, "backup setup degraded");
                    Ok(BackupState {
                        enabled: false,
                        degraded: true,
                        error: Some(msg),
                        ..Default::default()
                    })
                }
            }
        }
    }
}

async fn create_new_backup(
    client: &Client,
    keychain: &dyn KeychainBackend,
    keychain_key: &str,
    is_first_run: bool,
) -> Result<BackupState> {
    let backups = client.encryption().backups();
    match backups.create().await {
        Ok(()) => {
            // Recovery key is now available via the recovery API.
            let recovery = client.encryption().recovery();
            let recovery_key = recovery
                .secret_storage_key()
                .await
                .context("no recovery key available after backup creation")?;
            keychain
                .set(keychain_key, recovery_key.as_bytes())
                .context("failed to persist recovery key to keychain")?;
            let version = backups.fetch_exists_on_server().await.ok().flatten();
            Ok(BackupState {
                enabled: true,
                version,
                keys_downloaded: 0,
                degraded: false,
                error: None,
            })
        }
        Err(e) => {
            if is_first_run {
                anyhow::bail!("failed to create backup version: {e}");
            }
            tracing::warn!(error=%e, "backup creation failed, continuing degraded");
            Ok(BackupState {
                enabled: false,
                degraded: true,
                error: Some(e.to_string()),
                ..Default::default()
            })
        }
    }
}

async fn load_from_keychain(
    client: &Client,
    keychain: &dyn KeychainBackend,
    keychain_key: &str,
) -> Result<BackupState> {
    let raw = keychain.get(keychain_key)
        .context("keychain get failed")?
        .ok_or_else(|| anyhow::anyhow!("no recovery key in keychain"))?;
    let recovery_key = String::from_utf8(raw).context("recovery key not utf-8")?;
    let recovery = client.encryption().recovery();
    recovery.recover(&recovery_key).await.context("recover() rejected stored recovery key")?;
    Ok(BackupState {
        enabled: true,
        version: None, // populated by caller after download
        keys_downloaded: 0,
        degraded: false,
        error: None,
    })
}

async fn load_from_secret_storage(
    client: &Client,
    keychain: &dyn KeychainBackend,
    keychain_key: &str,
) -> Result<BackupState> {
    let recovery = client.encryption().recovery();
    recovery.recover_from_secret_storage().await
        .context("recover_from_secret_storage failed")?;
    // Now the recovery key is loaded into the SDK; we can ask for it to cache.
    if let Ok(Some(key)) = recovery.secret_storage_key().await.map(Some) {
        let _ = keychain.set(keychain_key, key.as_bytes());
    }
    Ok(BackupState {
        enabled: true,
        version: None,
        keys_downloaded: 0,
        degraded: false,
        error: None,
    })
}

pub async fn download_all_keys(client: &Client) -> Result<u64> {
    let backups = client.encryption().backups();
    backups.download_all_room_keys().await.context("download_all_room_keys failed")?;
    // matrix-sdk doesn't return a count directly; we report 0 and rely on
    // SDK tracing for the actual number.
    Ok(0)
}
```

NOTE: matrix-sdk 0.16's exact backup API surface may differ slightly. If the
above method names don't compile, consult `cargo doc --open -p matrix-sdk` and
adjust to the closest equivalents — the contract (create / load / download) is
what matters; method names are not load-bearing.

- [ ] **Step 2: Wire into lib.rs**

In `crates/mxdx-matrix/src/lib.rs` add `pub mod backup;`.

- [ ] **Step 3: cargo check (compile-only)**

`cargo check -p mxdx-matrix 2>&1 | tail -30`
Expected: clean OR a small set of method-name mismatches against matrix-sdk that need adjustment per the note above.

- [ ] **Step 4: Commit**

```bash
git add crates/mxdx-matrix/src/backup.rs crates/mxdx-matrix/src/lib.rs
git commit -m "feat(mxdx-matrix): add backup facade with create/load/secret-storage flows"
```

---

## Task 7: Wire backup into worker `connect()`

**Files:**
- Modify: `crates/mxdx-worker/src/lib.rs`

- [ ] **Step 1: Locate the connect() function**

`grep -n "fn connect\|pub async fn connect" crates/mxdx-worker/src/lib.rs`

- [ ] **Step 2: Insert backup setup after Matrix login**

Right after the existing `tracing::info!("connected to Matrix ...")` line and BEFORE any room creation/discovery, add:
```rust
    // Server-side megolm key backup.
    let unix_user = whoami::username();
    let matrix_user = client.user_id().context("no user_id after login")?;
    let server = &config.homeserver;
    let keychain = mxdx_types::keychain_chain::ChainedKeychain::default_chain()
        .context("backup: keychain init failed")?;
    let is_first_run = !config.session_was_restored; // expose this from MultiHsClient if not already
    let backup_state = match mxdx_matrix::backup::ensure_backup(
        client.inner_sdk_client(), // accessor to the matrix_sdk::Client
        &keychain,
        server,
        matrix_user,
        &unix_user,
        is_first_run,
    ).await {
        Ok(state) => state,
        Err(e) if is_first_run => return Err(e),
        Err(e) => {
            tracing::warn!(error=%e, "backup setup failed (subsequent run); continuing degraded");
            mxdx_matrix::backup::BackupState {
                enabled: false,
                degraded: true,
                error: Some(e.to_string()),
                ..Default::default()
            }
        }
    };
    if backup_state.enabled {
        match mxdx_matrix::backup::download_all_keys(client.inner_sdk_client()).await {
            Ok(n) => tracing::info!(downloaded=n, "backup: room keys downloaded"),
            Err(e) => tracing::warn!(error=%e, "backup: download_all_room_keys failed"),
        }
    }
    tracing::info!(
        enabled=backup_state.enabled,
        degraded=backup_state.degraded,
        "backup state"
    );
```

- [ ] **Step 3: Add accessor on MultiHsClient if needed**

If `client.inner_sdk_client()` doesn't exist, add it to `crates/mxdx-matrix/src/multi_hs.rs`:
```rust
impl MultiHsClient {
    pub fn inner_sdk_client(&self) -> &matrix_sdk::Client {
        self.preferred.inner()  // adapt to actual struct
    }
}
```

- [ ] **Step 4: Add `session_was_restored` to MultiHsClient**

If not present, add a `bool` field set during connect, exposed via `pub fn session_was_restored(&self) -> bool`.

- [ ] **Step 5: cargo check**

`cargo check -p mxdx-worker 2>&1 | tail -30`
Expected: clean.

- [ ] **Step 6: Manual smoke test**

```bash
cargo build --release -p mxdx-worker 2>&1 | tail -5
RUST_LOG=mxdx_worker=info,mxdx_matrix=info ./target/release/mxdx-worker start \
  --homeserver https://ca1-beta.mxdx.dev \
  --username e2etest-test1 --password 'mxdx-e2e-test-2026!' \
  --authorized-user @e2etest-test2:ca1-beta.mxdx.dev \
  --allowed-command echo &
WORKER_PID=$!
sleep 30
kill $WORKER_PID
wait
```
Expected: log lines `backup: room keys downloaded` and `backup state enabled=true`.

- [ ] **Step 7: Verify keychain entry exists**

```bash
./target/release/mxdx-worker diagnose \
  --homeserver https://ca1-beta.mxdx.dev \
  --username e2etest-test1 --password 'mxdx-e2e-test-2026!' \
  | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('backup', 'NOT YET ADDED'))"
```
(`backup` field will show "NOT YET ADDED" until Task 14 — that's fine for now; the goal is just to confirm the worker survives startup.)

- [ ] **Step 8: Commit**

```bash
git add crates/mxdx-worker/src/lib.rs crates/mxdx-matrix/src/multi_hs.rs
git commit -m "feat(mxdx-worker): enable server-side key backup on connect"
```

---

## Task 8: Wire backup into client daemon

**Files:**
- Modify: `crates/mxdx-client/src/daemon/mod.rs`

- [ ] **Step 1: Locate the daemon's Matrix-connect path**

`grep -n "connect\|MultiHsClient::connect" crates/mxdx-client/src/daemon/mod.rs`

- [ ] **Step 2: Insert backup setup after Matrix connect**

Same code block as Task 7 step 2, but in the daemon's connect path. The daemon does NOT call room re-encryption (clients never modify rooms), so just the backup + download.

- [ ] **Step 3: cargo check + manual smoke test**

```bash
cargo check -p mxdx-client 2>&1 | tail -10
cargo build --release -p mxdx-client 2>&1 | tail -5
```

- [ ] **Step 4: Commit**

```bash
git add crates/mxdx-client/src/daemon/mod.rs
git commit -m "feat(mxdx-client): enable server-side key backup on daemon startup"
```

---

## Task 9: `reencrypt.rs` — verify_or_replace_topology

**Files:**
- Create: `crates/mxdx-matrix/src/reencrypt.rs`
- Modify: `crates/mxdx-matrix/src/lib.rs`

- [ ] **Step 1: Create the module**

Create `crates/mxdx-matrix/src/reencrypt.rs`:
```rust
//! Idempotent room re-encryption: detects rooms missing m.room.encryption
//! or missing encrypt_state_events, tombstones them, and creates encrypted
//! replacements with the same name/topic/power_levels/members.
//!
//! Per-room (not topology-wide) so a partially-broken topology only replaces
//! the broken pieces.
//!
//! See spec section "room re-encryption flow".

use anyhow::Result;
use matrix_sdk::ruma::{OwnedRoomId, OwnedUserId, RoomId};
use crate::rest::RestClient;
use crate::client::MatrixClient;
use crate::rooms::LauncherTopology;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RoomRole { Space, Exec, Logs }

pub async fn verify_or_replace_topology(
    matrix: &MatrixClient,
    rest: &RestClient,
    topology: LauncherTopology,
    launcher_id: &str,
    authorized_users: &[OwnedUserId],
) -> Result<LauncherTopology> {
    let mut new_space = topology.space_id.clone();
    let mut new_exec = topology.exec_room_id.clone();
    let mut new_logs = topology.logs_room_id.clone();

    new_space = ensure_encrypted(matrix, rest, &topology.space_id, RoomRole::Space, launcher_id, authorized_users).await?;
    new_exec = ensure_encrypted(matrix, rest, &topology.exec_room_id, RoomRole::Exec, launcher_id, authorized_users).await?;
    new_logs = ensure_encrypted(matrix, rest, &topology.logs_room_id, RoomRole::Logs, launcher_id, authorized_users).await?;

    Ok(LauncherTopology {
        space_id: new_space,
        exec_room_id: new_exec,
        logs_room_id: new_logs,
    })
}

async fn ensure_encrypted(
    matrix: &MatrixClient,
    rest: &RestClient,
    room: &RoomId,
    role: RoomRole,
    launcher_id: &str,
    authorized_users: &[OwnedUserId],
) -> Result<OwnedRoomId> {
    let enc = rest.get_room_encryption(room).await?;
    let needs_replace = match enc {
        None => true,
        Some(s) => s.algorithm != "m.megolm.v1.aes-sha2" || !s.encrypt_state_events,
    };
    if !needs_replace {
        return Ok(room.to_owned());
    }
    tracing::warn!(room=%room, ?role, "room not properly encrypted, replacing");

    let (name, topic) = match role {
        RoomRole::Space => (
            format!("mxdx: {launcher_id}"),
            format!("org.mxdx.launcher.space:{launcher_id}"),
        ),
        RoomRole::Exec => (
            format!("mxdx: {launcher_id} — exec"),
            format!("org.mxdx.launcher.exec:{launcher_id}"),
        ),
        RoomRole::Logs => (
            format!("mxdx: {launcher_id} — logs"),
            format!("org.mxdx.launcher.logs:{launcher_id}"),
        ),
    };

    let new_id = matrix
        .create_named_encrypted_room(&name, &topic, authorized_users)
        .await?;
    matrix.tombstone_room(room, &new_id).await?;
    if let Err(e) = matrix.leave_and_forget_room(room).await {
        tracing::warn!(room=%room, error=%e, "failed to leave old room (non-fatal)");
    }
    Ok(new_id)
}
```

- [ ] **Step 2: Add `pub mod reencrypt;` to `crates/mxdx-matrix/src/lib.rs`**

- [ ] **Step 3: Add a `leave_and_forget_room` method on MatrixClient if missing**

`grep -n "leave_and_forget" crates/mxdx-matrix/src/client.rs`
If not present, add:
```rust
    pub async fn leave_and_forget_room(&self, room_id: &RoomId) -> anyhow::Result<()> {
        if let Some(room) = self.inner.get_room(room_id) {
            room.leave().await?;
            room.forget().await?;
        }
        Ok(())
    }
```

- [ ] **Step 4: cargo check**

`cargo check -p mxdx-matrix 2>&1 | tail -20`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/mxdx-matrix/src/reencrypt.rs crates/mxdx-matrix/src/lib.rs crates/mxdx-matrix/src/client.rs
git commit -m "feat(mxdx-matrix): add reencrypt module for self-healing room replacement"
```

---

## Task 10: Wire reencrypt + REST find into worker `connect()`

**Files:**
- Modify: `crates/mxdx-worker/src/lib.rs`

- [ ] **Step 1: Replace `find_launcher_space` call with REST variant**

In `crates/mxdx-worker/src/lib.rs`, locate the existing call to `client.find_launcher_space(&launcher_id).await?` (or `get_or_create_launcher_space`). Replace with:
```rust
    let topology = match client
        .find_launcher_space_via_rest(&launcher_id, &config.homeserver, client.access_token())
        .await?
    {
        Some(t) => t,
        None => client.create_launcher_space(&launcher_id).await?,
    };
```

If `client.access_token()` doesn't exist, add an accessor on `MultiHsClient`/`MatrixClient` that returns the current session's access token as `&str`.

- [ ] **Step 2: Insert reencrypt call after backup download, after topology resolved**

```rust
    let rest = mxdx_matrix::rest::RestClient::new(&config.homeserver, client.access_token());
    let topology = mxdx_matrix::reencrypt::verify_or_replace_topology(
        client.matrix_client(),
        &rest,
        topology,
        &launcher_id,
        &config.authorized_users,
    ).await?;
```

- [ ] **Step 3: cargo check**

`cargo check -p mxdx-worker 2>&1 | tail -20`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/mxdx-worker/src/lib.rs crates/mxdx-matrix/src/multi_hs.rs crates/mxdx-matrix/src/client.rs
git commit -m "feat(mxdx-worker): use REST-based room discovery and self-heal unencrypted rooms"
```

---

## Task 11: Encrypt the launcher space room

**Files:**
- Modify: `crates/mxdx-matrix/src/rooms.rs`

- [ ] **Step 1: Find create_launcher_space**

`grep -n "fn create_launcher_space" crates/mxdx-matrix/src/rooms.rs`

- [ ] **Step 2: Add encryption to the space CreateRoomRequest**

Find the `create_space` or equivalent call in `create_launcher_space` and add `m.room.encryption` to its `initial_state`:
```rust
    use matrix_sdk::ruma::events::room::encryption::RoomEncryptionEventContent;
    use matrix_sdk::ruma::events::InitialStateEvent;
    use matrix_sdk::ruma::events::EmptyStateKey;

    let encryption_event = InitialStateEvent::new(
        EmptyStateKey,
        RoomEncryptionEventContent::with_recommended_defaults().with_encrypted_state(),
    );
    // ... add encryption_event.to_raw_any() to the existing initial_state vec for the space
```

- [ ] **Step 3: cargo check**

`cargo check -p mxdx-matrix 2>&1 | tail -10`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/mxdx-matrix/src/rooms.rs
git commit -m "fix(mxdx-matrix): encrypt launcher space room with MSC4362 state events"
```

---

## Task 12: Diagnose tool — fix `encrypt_state_events` parser

**Files:**
- Modify: `crates/mxdx-client/src/diagnose.rs`

- [ ] **Step 1: Locate the encryption parsing**

`grep -n "encrypt_state_events" crates/mxdx-client/src/diagnose.rs`

- [ ] **Step 2: Replace single-key lookup with two-key fallback**

Change:
```rust
let encrypt_state = content.get("encrypt_state_events").and_then(|b| b.as_bool());
```
to:
```rust
let encrypt_state = content.get("encrypt_state_events").and_then(|b| b.as_bool())
    .or_else(|| content.get("io.element.msc4362.encrypt_state_events").and_then(|b| b.as_bool()));
```

- [ ] **Step 3: cargo build**

```bash
cargo build --release -p mxdx-client 2>&1 | tail -5
```

- [ ] **Step 4: Manual verification against the live exec room**

```bash
./target/release/mxdx-client diagnose \
  --homeserver https://ca1-beta.mxdx.dev \
  --username e2etest-test2 --password 'mxdx-e2e-test-2026!' \
  | python3 -c "
import json,sys
d=json.load(sys.stdin)
for r in d['matrix']['joined_rooms']:
    print(r['type_hint'], r.get('encryption'))
"
```
Expected: exec/logs rooms show `encrypt_state_events: true` (no longer None).

- [ ] **Step 5: Commit**

```bash
git add crates/mxdx-client/src/diagnose.rs
git commit -m "fix(mxdx-client): diagnose accepts both encrypt_state_events keys"
```

---

## Task 13: Diagnose `--decrypt` flag

**Files:**
- Modify: `crates/mxdx-client/src/diagnose.rs`
- Modify: `crates/mxdx-client/src/cli/mod.rs` (add `--decrypt` to Diagnose subcommand)

- [ ] **Step 1: Add `--decrypt` to the CLI definition**

In `crates/mxdx-client/src/cli/mod.rs`, find the `Diagnose` variant and add:
```rust
    Diagnose {
        #[arg(long, default_value_t = false)]
        pretty: bool,
        #[arg(long, default_value_t = false)]
        decrypt: bool,
    },
```

Plumb the flag through to `run_diagnose(...)` in `main.rs` and `diagnose.rs`.

- [ ] **Step 2: Implement temp-store SDK Client**

In `crates/mxdx-client/src/diagnose.rs`, add a new function:
```rust
async fn decrypt_with_temp_client(
    homeserver: &str,
    matrix_user: &str,
    password: &str,
    rooms: &[matrix_sdk::ruma::OwnedRoomId],
) -> anyhow::Result<std::collections::HashMap<String, serde_json::Value>> {
    use matrix_sdk::Client;
    let temp_dir = std::env::temp_dir().join(format!("mxdx-diagnose-{}", std::process::id()));
    std::fs::create_dir_all(&temp_dir)?;

    let client = Client::builder()
        .homeserver_url(homeserver)
        .sqlite_store(&temp_dir, None)
        .build()
        .await?;
    client.matrix_auth().login_username(matrix_user, password).send().await?;

    // Sync once so the SDK knows about the rooms.
    client.sync_once(matrix_sdk::config::SyncSettings::default()).await?;

    // Set up backup using ensure_backup with is_first_run=false; we expect it to exist.
    let unix_user = whoami::username();
    let user_id = client.user_id().context("no user_id after login")?;
    let keychain = mxdx_types::keychain_chain::ChainedKeychain::default_chain()?;
    let _ = mxdx_matrix::backup::ensure_backup(&client, &keychain, homeserver, user_id, &unix_user, false).await?;
    let _ = mxdx_matrix::backup::download_all_keys(&client).await;

    let mut decrypted = std::collections::HashMap::new();
    for rid in rooms {
        if let Some(room) = client.get_room(rid) {
            // Walk recent timeline + state, attempt to decrypt encrypted state events.
            // matrix-sdk's room state API returns SyncStateEvent which is decrypted in encrypted rooms.
            let state = room.get_state_events_static::<matrix_sdk::ruma::events::room::encryption::RoomEncryptionEventContent>().await?;
            for ev in state {
                let key = format!("{}:{}", rid, ev.event_id());
                if let Ok(json) = serde_json::to_value(ev.deserialize()?) {
                    decrypted.insert(key, json);
                }
            }
        }
    }

    drop(client);
    let _ = std::fs::remove_dir_all(&temp_dir);
    Ok(decrypted)
}
```

NOTE: matrix-sdk's exact API for "list all decrypted state events of any type"
varies; use `get_state_events_for_keys` or iterate over the SDK's `Room::state`
helpers as appropriate. The contract: best-effort decrypt every encrypted state
event in the listed rooms, return a map of `event_id → decrypted JSON`.

- [ ] **Step 3: Wire decrypt output into the JSON document**

In `run_diagnose`, after building the existing `joined_rooms` array, if `decrypt` is true:
```rust
if decrypt {
    let room_ids: Vec<_> = joined_rooms.iter().filter_map(|r| RoomId::parse(&r.room_id).ok().map(|i| i.to_owned())).collect();
    match decrypt_with_temp_client(homeserver, &matrix_user, &password, &room_ids).await {
        Ok(map) => {
            // Merge decrypted contents into the per-room recent_state_event_types section.
            for r in joined_rooms.iter_mut() {
                let prefix = format!("{}:", r.room_id);
                let mut decrypted_for_room = serde_json::Map::new();
                for (k, v) in &map {
                    if let Some(ev_id) = k.strip_prefix(&prefix) {
                        decrypted_for_room.insert(ev_id.to_string(), v.clone());
                    }
                }
                r.decrypted_state_events = Some(decrypted_for_room.into());
            }
        }
        Err(e) => {
            output["decrypt_error"] = serde_json::json!(e.to_string());
        }
    }
}
```

Add a `pub decrypted_state_events: Option<serde_json::Value>` field on the joined-room struct.

- [ ] **Step 4: cargo build**

`cargo build --release -p mxdx-client 2>&1 | tail -10`

- [ ] **Step 5: Commit**

```bash
git add crates/mxdx-client/src/diagnose.rs crates/mxdx-client/src/cli/mod.rs crates/mxdx-client/src/main.rs
git commit -m "feat(mxdx-client): add --decrypt flag to diagnose for live state decryption"
```

---

## Task 14: Add backup section to diagnose JSON output

**Files:**
- Modify: `crates/mxdx-client/src/diagnose.rs`

- [ ] **Step 1: Add `backup` field to the top-level diagnose output struct**

```rust
pub struct DiagnoseReport {
    // ... existing fields ...
    pub backup: BackupReport,
}

pub struct BackupReport {
    pub keychain_present: bool,
    pub server_has_version: bool,
    pub version: Option<String>,
    pub local_recovery_key_present: bool,
    pub error: Option<String>,
}
```

- [ ] **Step 2: Populate via REST**

The diagnose tool already does direct REST. Add a function `gather_backup_report` that:
1. Constructs the keychain key via `backup_keychain_key`.
2. Reads from `ChainedKeychain::default_chain()` — `local_recovery_key_present = key.is_some()`.
3. `GET /_matrix/client/v3/room_keys/version` — populate `server_has_version` and `version` from the response.

- [ ] **Step 3: Manual verification**

```bash
./target/release/mxdx-client diagnose \
  --homeserver https://ca1-beta.mxdx.dev \
  --username e2etest-test2 --password 'mxdx-e2e-test-2026!' \
  | python3 -c "import json,sys; print(json.dumps(json.load(sys.stdin)['backup'], indent=2))"
```
Expected: a structured JSON object describing the backup state.

- [ ] **Step 4: Commit**

```bash
git add crates/mxdx-client/src/diagnose.rs
git commit -m "feat(mxdx-client): diagnose reports backup state (keychain, server version)"
```

---

## Task 15: E2E test — t11 backup round trip

**Files:**
- Modify: `crates/mxdx-worker/tests/e2e_profile.rs`

- [ ] **Step 1: Add the test**

Append to `crates/mxdx-worker/tests/e2e_profile.rs`:
```rust
#[test]
#[ignore = "requires beta credentials"]
fn t11_backup_round_trip() {
    skip_if_failed!();
    let creds = match load_creds() { Some(c) => c, None => return };
    // Phase 1: start a worker with store_dir A, post a session, kill the worker.
    let (store_a, kc_a) = persistent_test_dirs_named("t11-a");
    let mut worker_a = start_worker(&creds.server_url, &creds.worker_user, &creds.worker_pass,
        &creds.client_matrix_id(), &store_a, &kc_a);
    wait_ready(15);
    let _ = run_client_daemon_timeout("/bin/echo", &["round-trip-marker"], 40, &creds);
    let _ = worker_a.kill();
    let _ = worker_a.wait();

    // Phase 2: start a worker with a *different* store_dir B but the same Matrix account.
    let (store_b, kc_b) = persistent_test_dirs_named("t11-b");
    let _ = std::fs::remove_dir_all(&store_b);
    std::fs::create_dir_all(&store_b).unwrap();
    let mut worker_b = start_worker(&creds.server_url, &creds.worker_user, &creds.worker_pass,
        &creds.client_matrix_id(), &store_b, &kc_b);
    wait_ready(20);

    // worker_b should have downloaded the room keys from backup; sending an echo
    // should succeed without "missing key" decrypt errors.
    let out = run_client_daemon_timeout("/bin/echo", &["after-backup-restore"], 40, &creds);
    let _ = worker_b.kill();
    let _ = worker_b.wait();
    if !out.status.success() {
        fail_test!("t11: second worker failed to decrypt after backup restore: {:?}", out);
    }
}
```

Add a helper `persistent_test_dirs_named(label: &str)` that returns `(PathBuf, PathBuf)` under `~/.mxdx/e2e-{label}/`.

- [ ] **Step 2: Run only this test**

```bash
cargo test --release -p mxdx-worker --test e2e_profile -- --ignored --nocapture t11_backup_round_trip 2>&1 | tail -50
```
Expected: pass.

- [ ] **Step 3: Commit**

```bash
git add crates/mxdx-worker/tests/e2e_profile.rs
git commit -m "test(e2e): t11 backup round trip across distinct store dirs"
```

---

## Task 16: E2E test — t12 unencrypted room self-heal

**Files:**
- Modify: `crates/mxdx-worker/tests/e2e_profile.rs`

- [ ] **Step 1: Add the test**

```rust
#[test]
#[ignore = "requires beta credentials"]
fn t12_unencrypted_room_self_heal() {
    skip_if_failed!();
    let creds = match load_creds() { Some(c) => c, None => return };

    // Pre-create an UNENCRYPTED room with the launcher topic via raw REST.
    // Then start the worker; it should detect, tombstone, and replace.
    let launcher_id = format!("{}.{}.{}", hostname::get().unwrap().to_string_lossy(),
        whoami::username(), creds.worker_user);
    let topic = format!("org.mxdx.launcher.exec:{launcher_id}");

    // Login and create an unencrypted room via REST (omit m.room.encryption from initial_state).
    // ... helper: rest_create_unencrypted_room(creds, name, topic) -> room_id
    let bad_room = rest_create_unencrypted_room(&creds, "mxdx-test-bad", &topic).unwrap();

    let (store, kc) = persistent_test_dirs_named("t12");
    let mut worker = start_worker(&creds.server_url, &creds.worker_user, &creds.worker_pass,
        &creds.client_matrix_id(), &store, &kc);
    wait_ready(20);

    // The bad room should now be tombstoned (has m.room.tombstone state event).
    let tomb = rest_get_tombstone(&creds, &bad_room).unwrap();
    let _ = worker.kill();
    let _ = worker.wait();
    if tomb.is_none() {
        fail_test!("t12: worker did not tombstone the unencrypted room");
    }
}
```

Add helpers `rest_create_unencrypted_room` and `rest_get_tombstone` in the test file (small `reqwest` calls).

- [ ] **Step 2: Run**

```bash
cargo test --release -p mxdx-worker --test e2e_profile -- --ignored --nocapture t12_unencrypted_room_self_heal 2>&1 | tail -50
```

- [ ] **Step 3: Commit**

```bash
git add crates/mxdx-worker/tests/e2e_profile.rs
git commit -m "test(e2e): t12 worker self-heals unencrypted launcher rooms"
```

---

## Task 17: E2E test — t13 diagnose decrypts state

**Files:**
- Modify: `crates/mxdx-worker/tests/e2e_profile.rs`

- [ ] **Step 1: Add the test**

```rust
#[test]
#[ignore = "requires beta credentials"]
fn t13_diagnose_decrypts_state() {
    skip_if_failed!();
    let creds = match load_creds() { Some(c) => c, None => return };
    let state = SHARED_STATE.get().expect("requires t10 first");

    // Run a session so there's a session.completed encrypted state event.
    let _ = run_client_daemon_timeout("/bin/echo", &["t13-marker"], 40, &creds);

    // Run diagnose --decrypt and assert the decrypted output contains a session.completed.
    let out = std::process::Command::new(cargo_bin("mxdx-client"))
        .args(["diagnose", "--decrypt", "--homeserver", &creds.server_url,
               "--username", &creds.worker_user, "--password", &creds.worker_pass])
        .output()
        .expect("diagnose failed to run");
    if !out.status.success() {
        fail_test!("t13: diagnose --decrypt exit {:?}", out.status);
    }
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("diagnose output invalid JSON");
    let mut found = false;
    for room in json["matrix"]["joined_rooms"].as_array().unwrap_or(&vec![]) {
        if let Some(decrypted) = room.get("decrypted_state_events") {
            if decrypted.as_object().map(|o| !o.is_empty()).unwrap_or(false) {
                found = true;
                break;
            }
        }
    }
    if !found {
        fail_test!("t13: diagnose --decrypt produced no decrypted_state_events");
    }
}
```

- [ ] **Step 2: Run**

```bash
cargo test --release -p mxdx-worker --test e2e_profile -- --ignored --nocapture t13_diagnose_decrypts_state 2>&1 | tail -40
```

- [ ] **Step 3: Commit**

```bash
git add crates/mxdx-worker/tests/e2e_profile.rs
git commit -m "test(e2e): t13 diagnose --decrypt surfaces decrypted state events"
```

---

## Task 18: Full baseline E2E + final verification

- [ ] **Step 1: Kill stale processes**

```bash
pkill -f "mxdx-worker.*start" 2>/dev/null
pkill -f "mxdx-client.*_daemon" 2>/dev/null
sleep 2
```

- [ ] **Step 2: Clean test accounts**

```bash
for spec in \
  "https://ca1-beta.mxdx.dev e2etest-test1" \
  "https://ca2-beta.mxdx.dev e2etest-test1" \
  "https://ca1-beta.mxdx.dev e2etest-test2" \
  "https://ca2-beta.mxdx.dev e2etest-test2"; do
  read -r HS USER <<< "$spec"
  timeout 300 ./target/release/mxdx-client --no-daemon \
    --homeserver "$HS" --username "$USER" --password 'mxdx-e2e-test-2026!' \
    cleanup rooms --force 2>&1 | tail -5
done
```

- [ ] **Step 3: Run full baseline backgrounded**

```bash
rm -f e2e-results.jsonl /tmp/mxdx-worker-*.log
timeout 1500 cargo test --release -p mxdx-worker --test e2e_profile -- \
  --ignored --nocapture --test-threads=1 \
  > /tmp/mxdx-final.log 2>&1 &
wait $!
echo "exit: $?"
tail -150 /tmp/mxdx-final.log
cat e2e-results.jsonl
```

- [ ] **Step 4: Verify outcomes**

Expected:
- All 19 baseline tests pass
- t11, t12, t13 pass
- t20 echoes round-trip in <15s
- No "multiple rooms match launcher topic" warnings in worker logs
- No "Another worker instance owns" UUID lock errors
- diagnose against any test account shows backup.enabled=true and recent worker telemetry

- [ ] **Step 5: Commit if anything was tweaked during verification**

---

## Self-Review

After writing this plan, I checked it against the spec:

- ✅ Spec section "Components" → Tasks 1, 2, 6, 9 (rest, backup, reencrypt) + Task 11 (rooms.rs space encryption) + Task 12 (diagnose parser)
- ✅ "Worker startup" / "Client daemon startup" data flow → Tasks 7, 8, 10
- ✅ "Diagnose tool" updates → Tasks 12, 13, 14
- ✅ "Testing" section → Tasks 1, 2 (unit), 15, 16, 17 (E2E), 18 (baseline)
- ✅ "Build sequence" maps 1:1 to task ordering
- ✅ All keychain interactions use `mxdx_types::identity::backup_keychain_key` (Task 5)
- ✅ Per-room re-encryption (not topology-wide) → Task 9 `ensure_encrypted` per role
- ✅ Tombstone-following in find_launcher_space → Task 4 inner loop
- ✅ Cleanup converges on shared REST helpers → Task 3
- ✅ Failure mode (fatal first run, warn subsequent) → Task 6 `is_first_run` parameter

Type consistency check: `BackupState`, `EncryptionState`, `RestClient`, `LauncherTopology`, `RoomRole` — all defined in their first task and used consistently in later tasks. `ensure_backup` signature matches between Task 6 (definition), Task 7 (worker call site), Task 8 (client call site), and Task 13 (diagnose call site).

Known gap surfaced during planning: the matrix-sdk 0.16 backup API method names (`backups().create()`, `recovery().recover()`, `recovery().secret_storage_key()`) are based on the documented surface but may not match exactly. Task 6 step 3 includes a note instructing the implementer to consult `cargo doc -p matrix-sdk` and adjust. This is unavoidable without locking the implementer to a specific API surface that may have moved.

---

**Plan complete.** Saved to `docs/superpowers/plans/2026-04-07-key-backup-and-encryption-fixes.md`.

Two execution options:

1. **Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration, isolated context per task.

2. **Inline Execution** — Execute tasks sequentially in this session using executing-plans, with checkpoints for review.

Which approach?
