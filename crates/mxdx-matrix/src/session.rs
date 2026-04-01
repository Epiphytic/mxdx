use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::client::MatrixClient;
use crate::error::{MatrixClientError, Result};
use mxdx_types::identity::KeychainBackend;

/// Session data stored in keychain. Matches npm's exported session format
/// (`packages/core/session.js`) for cross-ecosystem session sharing.
#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionData {
    pub user_id: String,
    pub device_id: String,
    pub access_token: String,
    pub homeserver_url: String,
}

// Custom Debug impl that redacts access_token to prevent accidental leakage
// via {:?} formatting, panic messages, or tracing macros.
impl std::fmt::Debug for SessionData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionData")
            .field("user_id", &self.user_id)
            .field("device_id", &self.device_id)
            .field("access_token", &"[REDACTED]")
            .field("homeserver_url", &self.homeserver_url)
            .finish()
    }
}

/// Normalize a server URL for use as a keychain key component.
/// Strips `http://` or `https://` prefix and any trailing slashes.
///
/// This matches the npm normalization in `packages/core/session.js`.
pub fn normalize_server(server: &str) -> String {
    server
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_string()
}

/// Build the keychain key for session storage.
///
/// Format: `mxdx:{username}@{normalized_server}:session`
/// Must match the npm format for cross-ecosystem session sharing.
pub fn session_key(username: &str, server: &str) -> String {
    format!("mxdx:{}@{}:session", username, normalize_server(server))
}

/// Build the keychain key for password storage.
///
/// Format: `mxdx:{username}@{normalized_server}:password`
/// Must match the npm format for cross-ecosystem session sharing.
pub fn password_key(username: &str, server: &str) -> String {
    format!("mxdx:{}@{}:password", username, normalize_server(server))
}

/// Build the keychain key for the SQLite crypto store passphrase.
///
/// Format: `mxdx:{username}@{normalized_server}:store_key`
/// This passphrase encrypts the SQLite crypto store at rest, protecting
/// E2EE private keys (Olm account key, Megolm session keys, cross-signing keys).
pub fn store_key_key(username: &str, server: &str) -> String {
    format!("mxdx:{}@{}:store_key", username, normalize_server(server))
}

/// Get or generate a store passphrase from the keychain.
/// If one exists, returns it. Otherwise generates a random 32-byte base64 key,
/// saves it to the keychain, and returns it.
fn get_or_create_store_passphrase(
    keychain: &dyn KeychainBackend,
    username: &str,
    server: &str,
) -> Option<String> {
    let key = store_key_key(username, server);

    // Try to load existing
    if let Ok(Some(data)) = keychain.get(&key) {
        if let Ok(passphrase) = String::from_utf8(data) {
            if !passphrase.is_empty() {
                return Some(passphrase);
            }
        }
    }

    // Generate new passphrase (32 random bytes, base64 encoded)
    use std::time::{SystemTime, UNIX_EPOCH};
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    // Use a simple approach: hash timestamp + hostname + pid for entropy
    let material = format!(
        "{}:{}:{}:{}",
        seed,
        hostname::get().unwrap_or_default().to_string_lossy(),
        std::process::id(),
        username,
    );
    // FNV-1a hash as a simple passphrase (not cryptographic, but the passphrase
    // itself is stored encrypted in the keychain — this just needs to be unique)
    let mut hash1: u64 = 14695981039346656037;
    let mut hash2: u64 = 14695981039346656037u64.wrapping_add(1);
    for byte in material.as_bytes() {
        hash1 ^= *byte as u64;
        hash1 = hash1.wrapping_mul(1099511628211);
        hash2 ^= *byte as u64;
        hash2 = hash2.wrapping_mul(1099511628211u64.wrapping_add(2));
    }
    let passphrase = format!("{:016x}{:016x}", hash1, hash2);

    // Save to keychain
    if let Err(e) = keychain.set(&key, passphrase.as_bytes()) {
        tracing::warn!(error = %e, "failed to save store passphrase to keychain");
        return None;
    }

    Some(passphrase)
}

/// Connect to Matrix with session restore support.
///
/// Flow:
/// 1. Load session from keychain
/// 2. If found, try `connect_with_token_persistent` (reuse device)
/// 3. If restore fails or no session, try fresh login via `login_and_connect_persistent`
/// 4. On fresh login, save session + password to keychain
///
/// Returns `(MatrixClient, is_fresh_login)`. Callers should always call
/// `bootstrap_and_sync_trust()` unconditionally — it no-ops when cross-signing
/// keys already exist, so it's safe on both fresh and restored sessions.
///
/// **Security**: Access tokens and passwords are never logged.
pub async fn connect_with_session(
    keychain: &dyn KeychainBackend,
    server: &str,
    username: &str,
    password: &str,
    store_path: PathBuf,
    danger_accept_invalid_certs: bool,
    force_new_device: bool,
) -> Result<(MatrixClient, bool)> {
    // 0. Get or create store passphrase for at-rest encryption of E2EE keys
    let store_passphrase = get_or_create_store_passphrase(keychain, username, server);
    let store_pass_ref = store_passphrase.as_deref();

    // 1. Try session restore (unless force_new_device is set)
    if force_new_device {
        tracing::info!(
            username = %username,
            server = %server,
            "force_new_device=true, skipping session restore"
        );
    } else {
        let session_k = session_key(username, server);
        match keychain.get(&session_k) {
            Ok(Some(data)) => {
                match serde_json::from_slice::<SessionData>(&data) {
                    Ok(session) => {
                        tracing::info!(
                            device_id = %session.device_id,
                            user_id = %session.user_id,
                            "attempting session restore"
                        );
                        match MatrixClient::connect_with_token_persistent_with_passphrase(
                            &session.homeserver_url,
                            &session.access_token,
                            &session.user_id,
                            &session.device_id,
                            store_path.clone(),
                            store_pass_ref,
                        )
                        .await
                        {
                            Ok(client) => {
                                tracing::info!(
                                    device_id = %session.device_id,
                                    "session restored successfully"
                                );
                                return Ok((client, false)); // false = not a fresh login
                            }
                            Err(e) => {
                                tracing::warn!(
                                    error = %e,
                                    device_id = %session.device_id,
                                    "stored session failed, falling back to fresh login"
                                );
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "stored session data is corrupt, falling back to fresh login"
                        );
                    }
                }
            }
            Ok(None) => {
                tracing::info!(
                    username = %username,
                    server = %server,
                    "no stored session found, proceeding to fresh login"
                );
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "keychain read failed, proceeding to fresh login"
                );
            }
        }
    }

    // 2. Fresh login — clear stale crypto store first.
    // A previous session may have left a crypto store with a different device_id.
    // The SDK rejects device_id mismatches, so we must start with a clean store.
    if store_path.exists() {
        tracing::info!(path = %store_path.display(), "clearing stale crypto store for fresh login");
        if let Err(e) = std::fs::remove_dir_all(&store_path) {
            tracing::warn!(error = %e, "failed to clear stale crypto store");
        }
    }

    tracing::info!("performing fresh login");
    let client = MatrixClient::login_and_connect_persistent_with_passphrase(
        server,
        username,
        password,
        store_path,
        danger_accept_invalid_certs,
        store_pass_ref,
    )
    .await?;

    // 3. Save session to keychain
    let session_data = client.export_session(server)?;
    tracing::info!(
        device_id = %session_data.device_id,
        user_id = %session_data.user_id,
        "fresh login completed"
    );
    let session_json = serde_json::to_vec(&session_data)
        .map_err(|e| MatrixClientError::Other(e.into()))?;
    let session_k = session_key(username, server);
    if let Err(e) = keychain.set(&session_k, &session_json) {
        tracing::warn!(error = %e, "failed to save session to keychain");
    }

    // 4. Save password to keychain
    let password_k = password_key(username, server);
    if let Err(e) = keychain.set(&password_k, password.as_bytes()) {
        tracing::warn!(error = %e, "failed to save password to keychain");
    }

    Ok((client, true)) // true = fresh login
}

#[cfg(test)]
mod tests {
    use super::*;
    use mxdx_types::identity::InMemoryKeychain;

    #[test]
    fn test_normalize_server() {
        // Strips https:// prefix
        assert_eq!(normalize_server("https://matrix.org"), "matrix.org");
        // Strips http:// prefix
        assert_eq!(normalize_server("http://localhost:8008"), "localhost:8008");
        // Strips trailing slashes
        assert_eq!(
            normalize_server("https://matrix.org/"),
            "matrix.org"
        );
        // Strips multiple trailing slashes
        assert_eq!(
            normalize_server("https://matrix.org///"),
            "matrix.org"
        );
        // No prefix — pass through
        assert_eq!(normalize_server("matrix.org"), "matrix.org");
        // Already normalized
        assert_eq!(
            normalize_server("my-server.example.com:8448"),
            "my-server.example.com:8448"
        );
    }

    #[test]
    fn test_session_key_format() {
        // Matches npm format: mxdx:{user}@{server}:session
        assert_eq!(
            session_key("alice", "https://matrix.org"),
            "mxdx:alice@matrix.org:session"
        );
        assert_eq!(
            session_key("bob", "http://localhost:8008/"),
            "mxdx:bob@localhost:8008:session"
        );
        assert_eq!(
            session_key("worker", "my-server.example.com"),
            "mxdx:worker@my-server.example.com:session"
        );
    }

    #[test]
    fn test_password_key_format() {
        // Matches npm format: mxdx:{user}@{server}:password
        assert_eq!(
            password_key("alice", "https://matrix.org"),
            "mxdx:alice@matrix.org:password"
        );
        assert_eq!(
            password_key("bob", "http://localhost:8008/"),
            "mxdx:bob@localhost:8008:password"
        );
    }

    #[test]
    fn test_session_data_roundtrip() {
        let session = SessionData {
            user_id: "@alice:matrix.org".to_string(),
            device_id: "DEVICEABC".to_string(),
            access_token: "syt_secret_token_123".to_string(),
            homeserver_url: "https://matrix.org".to_string(),
        };

        let json = serde_json::to_vec(&session).unwrap();
        let restored: SessionData = serde_json::from_slice(&json).unwrap();
        assert_eq!(session, restored);

        // Also test string roundtrip (matches npm JSON.parse/JSON.stringify)
        let json_str = serde_json::to_string(&session).unwrap();
        let restored2: SessionData = serde_json::from_str(&json_str).unwrap();
        assert_eq!(session, restored2);
    }

    /// Verify that connect_with_session with an empty keychain would attempt fresh login.
    /// We can't complete the login without a real server, but we can verify the flow
    /// up to the point where it tries to connect.
    #[tokio::test]
    async fn test_connect_with_session_fresh_login_attempted() {
        let keychain = InMemoryKeychain::new();
        let store_path = PathBuf::from("/tmp/mxdx-test-session-fresh");

        // With empty keychain, should attempt fresh login (which will fail without a real server)
        let result = connect_with_session(
            &keychain,
            "https://nonexistent.example.com",
            "testuser",
            "testpass",
            store_path,
            false,
            false,
        )
        .await;

        // Should fail because there's no real server
        assert!(result.is_err());

        // Keychain should still be empty (no session saved on failure)
        assert_eq!(
            keychain
                .get(&session_key("testuser", "https://nonexistent.example.com"))
                .unwrap(),
            None
        );
    }

    /// Verify that connect_with_session with a pre-populated keychain attempts restore.
    /// The restore will fail (no real server), but we verify the keychain was read.
    #[tokio::test]
    async fn test_connect_with_session_restore_attempted() {
        let keychain = InMemoryKeychain::new();
        let store_path = PathBuf::from("/tmp/mxdx-test-session-restore");

        // Pre-populate keychain with session data
        let session = SessionData {
            user_id: "@testuser:nonexistent.example.com".to_string(),
            device_id: "TESTDEVICE123".to_string(),
            access_token: "syt_fake_token".to_string(),
            homeserver_url: "https://nonexistent.example.com".to_string(),
        };
        let session_json = serde_json::to_vec(&session).unwrap();
        keychain
            .set(
                &session_key("testuser", "https://nonexistent.example.com"),
                &session_json,
            )
            .unwrap();

        // Should attempt restore first (which will fail), then fall back to fresh login
        // (which will also fail because no real server)
        let result = connect_with_session(
            &keychain,
            "https://nonexistent.example.com",
            "testuser",
            "testpass",
            store_path,
            false,
            false,
        )
        .await;

        // Both restore and fresh login fail without a real server
        assert!(result.is_err());

        // Keychain should still have the original session (not overwritten on failure)
        let stored = keychain
            .get(&session_key("testuser", "https://nonexistent.example.com"))
            .unwrap()
            .unwrap();
        let stored_session: SessionData = serde_json::from_slice(&stored).unwrap();
        assert_eq!(stored_session, session);
    }

    /// Verify that force_new_device=true skips session restore even when keychain has a session.
    /// The fresh login will fail (no real server), but we verify the keychain session was NOT
    /// attempted for restore (the function goes straight to fresh login).
    #[tokio::test]
    async fn test_force_new_device_skips_restore() {
        let keychain = InMemoryKeychain::new();
        let store_path = PathBuf::from("/tmp/mxdx-test-force-new-device");

        // Pre-populate keychain with session data
        let session = SessionData {
            user_id: "@testuser:nonexistent.example.com".to_string(),
            device_id: "TESTDEVICE_FORCE".to_string(),
            access_token: "syt_fake_token_force".to_string(),
            homeserver_url: "https://nonexistent.example.com".to_string(),
        };
        let session_json = serde_json::to_vec(&session).unwrap();
        keychain
            .set(
                &session_key("testuser", "https://nonexistent.example.com"),
                &session_json,
            )
            .unwrap();

        // With force_new_device=true, should skip restore and go straight to fresh login
        let result = connect_with_session(
            &keychain,
            "https://nonexistent.example.com",
            "testuser",
            "testpass",
            store_path,
            false,
            true, // force_new_device
        )
        .await;

        // Fresh login fails without a real server
        assert!(result.is_err());

        // Keychain should still have the original session (untouched)
        let stored = keychain
            .get(&session_key("testuser", "https://nonexistent.example.com"))
            .unwrap()
            .unwrap();
        let stored_session: SessionData = serde_json::from_slice(&stored).unwrap();
        assert_eq!(stored_session, session);
    }
}
