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
) -> Result<(MatrixClient, bool)> {
    // 1. Try session restore
    let session_k = session_key(username, server);
    if let Ok(Some(data)) = keychain.get(&session_k) {
        if let Ok(session) = serde_json::from_slice::<SessionData>(&data) {
            tracing::info!(
                device_id = %session.device_id,
                user_id = %session.user_id,
                "attempting session restore"
            );
            match MatrixClient::connect_with_token_persistent(
                &session.homeserver_url,
                &session.access_token,
                &session.user_id,
                &session.device_id,
                store_path.clone(),
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
                        "session restore failed, falling back to fresh login"
                    );
                }
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
    let client = MatrixClient::login_and_connect_persistent(
        server,
        username,
        password,
        store_path,
        danger_accept_invalid_certs,
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
}
