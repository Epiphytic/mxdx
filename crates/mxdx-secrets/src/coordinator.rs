use std::collections::HashSet;
use std::io::Write;

use age::x25519::Recipient;
use anyhow::{Context, Result};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;

use mxdx_types::events::secret::{SecretRequestEvent, SecretResponseEvent};

use crate::store::SecretStore;

/// Coordinates secret request/response protocol with double encryption (mxdx-adr2).
///
/// The coordinator holds a SecretStore and a set of authorized scopes per worker.
/// When a worker requests a secret, the coordinator:
/// 1. Checks authorization
/// 2. Retrieves the plaintext from the store
/// 3. Re-encrypts it to the worker's one-time age public key
/// 4. Returns the age ciphertext (base64-encoded)
///
/// This ensures that even if the Matrix Megolm session keys are compromised,
/// the secret value remains protected by the ephemeral age encryption layer.
pub struct SecretCoordinator {
    store: SecretStore,
    authorized_scopes: HashSet<String>,
}

impl SecretCoordinator {
    pub fn new(store: SecretStore, authorized_scopes: HashSet<String>) -> Self {
        Self {
            store,
            authorized_scopes,
        }
    }

    /// Handle a secret request, returning a response with double-encrypted value.
    pub fn handle_secret_request(&self, request: &SecretRequestEvent) -> SecretResponseEvent {
        if !self.authorized_scopes.contains(&request.scope) {
            return SecretResponseEvent {
                request_id: request.request_id.clone(),
                granted: false,
                encrypted_value: None,
                error: Some(format!("unauthorized scope: {}", request.scope)),
            };
        }

        let plaintext = match self.store.get(&request.scope) {
            Ok(Some(value)) => value,
            Ok(None) => {
                return SecretResponseEvent {
                    request_id: request.request_id.clone(),
                    granted: false,
                    encrypted_value: None,
                    error: Some(format!("secret not found: {}", request.scope)),
                };
            }
            Err(e) => {
                return SecretResponseEvent {
                    request_id: request.request_id.clone(),
                    granted: false,
                    encrypted_value: None,
                    error: Some(format!("store error: {e}")),
                };
            }
        };

        let recipient: Recipient = match request.ephemeral_public_key.parse() {
            Ok(r) => r,
            Err(e) => {
                return SecretResponseEvent {
                    request_id: request.request_id.clone(),
                    granted: false,
                    encrypted_value: None,
                    error: Some(format!("invalid ephemeral public key: {e}")),
                };
            }
        };

        match encrypt_to_recipient(&recipient, plaintext.as_bytes()) {
            Ok(ciphertext) => SecretResponseEvent {
                request_id: request.request_id.clone(),
                granted: true,
                encrypted_value: Some(BASE64.encode(&ciphertext)),
                error: None,
            },
            Err(e) => SecretResponseEvent {
                request_id: request.request_id.clone(),
                granted: false,
                encrypted_value: None,
                error: Some(format!("encryption failed: {e}")),
            },
        }
    }
}

/// Encrypt plaintext to a specific age x25519 recipient.
fn encrypt_to_recipient(recipient: &Recipient, plaintext: &[u8]) -> Result<Vec<u8>> {
    let encryptor =
        age::Encryptor::with_recipients(std::iter::once(recipient as &dyn age::Recipient))
            .map_err(|e| anyhow::anyhow!("encryption setup failed: {e}"))?;
    let mut encrypted = vec![];
    let mut writer = encryptor
        .wrap_output(&mut encrypted)
        .context("failed to create encryption writer")?;
    writer.write_all(plaintext)?;
    writer.finish()?;
    Ok(encrypted)
}

/// Decrypt ciphertext using an age x25519 identity (for worker-side decryption).
pub fn decrypt_with_identity(
    identity: &age::x25519::Identity,
    ciphertext: &[u8],
) -> Result<Vec<u8>> {
    use std::io::Read;
    let decryptor = age::Decryptor::new(ciphertext)
        .map_err(|e| anyhow::anyhow!("decryption setup failed: {e}"))?;
    let mut reader = decryptor
        .decrypt(std::iter::once(identity as &dyn age::Identity))
        .map_err(|e| anyhow::anyhow!("decryption failed: {e}"))?;
    let mut decrypted = vec![];
    reader.read_to_end(&mut decrypted)?;
    Ok(decrypted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use age::x25519::Identity;

    fn setup_coordinator(scopes: &[&str], secrets: &[(&str, &str)]) -> SecretCoordinator {
        let mut store = SecretStore::new(Identity::generate());
        for (key, value) in secrets {
            store.add(key, value).unwrap();
        }
        let authorized = scopes.iter().map(|s| s.to_string()).collect();
        SecretCoordinator::new(store, authorized)
    }

    #[test]
    fn double_encrypted_round_trip() {
        let coordinator = setup_coordinator(
            &["github.token"],
            &[("github.token", "ghp_secret123")],
        );

        let worker_identity = Identity::generate();
        let worker_pubkey = worker_identity.to_public().to_string();

        let request = SecretRequestEvent {
            request_id: "req-001".into(),
            scope: "github.token".into(),
            ttl_seconds: 3600,
            reason: "deploy".into(),
            ephemeral_public_key: worker_pubkey,
        };

        let response = coordinator.handle_secret_request(&request);
        assert!(response.granted);
        assert!(response.error.is_none());

        let ciphertext = BASE64
            .decode(response.encrypted_value.as_ref().unwrap())
            .unwrap();
        let plaintext = decrypt_with_identity(&worker_identity, &ciphertext).unwrap();
        assert_eq!(String::from_utf8(plaintext).unwrap(), "ghp_secret123");
    }

    #[test]
    fn unauthorized_scope_denied() {
        let coordinator = setup_coordinator(
            &["github.token"],
            &[("github.token", "ghp_secret123")],
        );

        let worker_identity = Identity::generate();
        let request = SecretRequestEvent {
            request_id: "req-002".into(),
            scope: "aws.secret_key".into(),
            ttl_seconds: 3600,
            reason: "deploy".into(),
            ephemeral_public_key: worker_identity.to_public().to_string(),
        };

        let response = coordinator.handle_secret_request(&request);
        assert!(!response.granted);
        assert!(response.encrypted_value.is_none());
        assert!(response.error.unwrap().contains("unauthorized scope"));
    }

    #[test]
    fn missing_secret_denied() {
        let coordinator = setup_coordinator(
            &["github.token"],
            &[], // no secrets stored
        );

        let worker_identity = Identity::generate();
        let request = SecretRequestEvent {
            request_id: "req-003".into(),
            scope: "github.token".into(),
            ttl_seconds: 3600,
            reason: "deploy".into(),
            ephemeral_public_key: worker_identity.to_public().to_string(),
        };

        let response = coordinator.handle_secret_request(&request);
        assert!(!response.granted);
        assert!(response.error.unwrap().contains("secret not found"));
    }

    #[test]
    fn invalid_public_key_returns_error() {
        let coordinator = setup_coordinator(
            &["github.token"],
            &[("github.token", "ghp_secret123")],
        );

        let request = SecretRequestEvent {
            request_id: "req-004".into(),
            scope: "github.token".into(),
            ttl_seconds: 3600,
            reason: "deploy".into(),
            ephemeral_public_key: "not-a-valid-key".into(),
        };

        let response = coordinator.handle_secret_request(&request);
        assert!(!response.granted);
        assert!(response.error.unwrap().contains("invalid ephemeral public key"));
    }

    #[test]
    fn wrong_private_key_cannot_decrypt() {
        let coordinator = setup_coordinator(
            &["github.token"],
            &[("github.token", "ghp_secret123")],
        );

        let worker_identity = Identity::generate();
        let request = SecretRequestEvent {
            request_id: "req-005".into(),
            scope: "github.token".into(),
            ttl_seconds: 3600,
            reason: "deploy".into(),
            ephemeral_public_key: worker_identity.to_public().to_string(),
        };

        let response = coordinator.handle_secret_request(&request);
        assert!(response.granted);

        let ciphertext = BASE64
            .decode(response.encrypted_value.as_ref().unwrap())
            .unwrap();

        // Try decrypting with a different identity
        let wrong_identity = Identity::generate();
        let result = decrypt_with_identity(&wrong_identity, &ciphertext);
        assert!(result.is_err());
    }
}
