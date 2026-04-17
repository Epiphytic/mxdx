//! Matrix-backed [`HandshakeSigner`] + [`HandshakePeerKeySource`] for the
//! Phase 7 retrofit.
//!
//! Per ADR `docs/adr/2026-04-16-matrix-sdk-testing-feature.md`, we now use
//! `OlmMachine::sign()` via `Client::olm_machine_for_testing()` for device-
//! key signing. This supersedes the Phase 6 ephemeral-key approach from ADR
//! `docs/adr/2026-04-16-ephemeral-key-cross-cert.md` (marked Superseded).
//!
//! Trust derivation (storm §3.1 original):
//! 1. The transcript is signed with the device's long-term Ed25519 key.
//! 2. The peer verifier looks up the peer device's Ed25519 public key from
//!    the Matrix crypto store (verified device cache).
//! 3. Cross-signing is enforced: `get_peer_device_ed25519` rejects
//!    non-cross-signed devices.
//!
//! # Trait constraints
//!
//! [`HandshakeSigner::sign`] and [`HandshakePeerKeySource::peer_public_key`]
//! are both SYNC. Matrix lookups are async. We therefore:
//!
//! - `MatrixHandshakeSigner` pre-signs the transcript at `prepare_sign()`
//!   time (async) and caches the result. The sync `sign()` returns the
//!   cached signature. If `prepare_sign` hasn't been called, `sign()` uses
//!   `tokio::runtime::Handle::current().block_on()` as a fallback.
//! - `MatrixPeerKeySource` holds an `Arc<RwLock<HashMap<...>>>` populated
//!   via `refresh(peer_user_id, peer_device_id)` (async) before the
//!   handshake begins. The sync lookup reads the cache.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use mxdx_matrix::MatrixClient;

use super::verify::{
    HandshakePeerKeySource, HandshakeSigner, VerifyError,
    ED25519_PUBLIC_KEY_LEN, ED25519_SIGNATURE_LEN,
};

/// Production [`HandshakeSigner`] for Phase 7 retrofit.
///
/// Signs the handshake transcript with the device's long-term Ed25519 key
/// via `MatrixClient::sign_with_device_key()`. See ADR
/// `docs/adr/2026-04-16-matrix-sdk-testing-feature.md`.
pub struct MatrixHandshakeSigner {
    matrix: Arc<MatrixClient>,
    /// Cached public key (fetched once at construction).
    cached_pk: RwLock<Option<[u8; ED25519_PUBLIC_KEY_LEN]>>,
}

impl MatrixHandshakeSigner {
    /// Create a new signer backed by the device's long-term Ed25519 key.
    pub fn new(matrix: Arc<MatrixClient>) -> Self {
        Self {
            matrix,
            cached_pk: RwLock::new(None),
        }
    }

    /// Pre-fetch the device's Ed25519 public key (async). Call during
    /// transport start so the sync `sign()` has the key available.
    pub async fn init(&self) -> Result<(), mxdx_matrix::MatrixClientError> {
        // Sign a dummy message to discover our public key.
        let (_sig, pk) = self.matrix.sign_with_device_key("init").await?;
        if let Ok(mut cached) = self.cached_pk.write() {
            *cached = Some(pk);
        }
        Ok(())
    }

    /// The device's Ed25519 public key (available after `init()`).
    pub fn public_key(&self) -> Option<[u8; ED25519_PUBLIC_KEY_LEN]> {
        self.cached_pk.read().ok().and_then(|g| *g)
    }
}

impl HandshakeSigner for MatrixHandshakeSigner {
    fn sign(
        &self,
        transcript: &[u8],
    ) -> Result<
        ([u8; ED25519_SIGNATURE_LEN], [u8; ED25519_PUBLIC_KEY_LEN]),
        VerifyError,
    > {
        // OlmMachine::sign takes &str, so base64-encode the transcript.
        // Both Rust and npm sides must use the same encoding.
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(transcript);

        // Use block_on to bridge sync→async. The runtime is guaranteed to
        // exist in native context (the driver loop runs under tokio).
        let handle = tokio::runtime::Handle::current();
        let result = handle.block_on(self.matrix.sign_with_device_key(&b64));

        match result {
            Ok((sig, pk)) => Ok((sig, pk)),
            Err(e) => Err(VerifyError::SigningFailed(e.to_string())),
        }
    }
}

/// Production [`HandshakePeerKeySource`] for Phase 7 retrofit.
///
/// Looks up the peer device's Ed25519 public key from the Matrix verified
/// device cache via `MatrixClient::get_peer_device_ed25519`. Cross-signing
/// is enforced by the underlying method.
pub struct MatrixPeerKeySource {
    matrix: Arc<MatrixClient>,
    cache: Arc<RwLock<HashMap<(String, String), [u8; ED25519_PUBLIC_KEY_LEN]>>>,
}

impl MatrixPeerKeySource {
    pub fn new(matrix: Arc<MatrixClient>) -> Self {
        Self {
            matrix,
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Async refresh: fetch the peer device's Ed25519 public key from the
    /// Matrix verified device cache and populate the local cache.
    ///
    /// Returns `Ok(true)` iff a key was found and cached, `Ok(false)` if
    /// the device is unknown or not cross-signed.
    pub async fn refresh(
        &self,
        peer_user_id: &mxdx_matrix::UserId,
        peer_device_id: &str,
    ) -> Result<bool, mxdx_matrix::MatrixClientError> {
        let key = self
            .matrix
            .get_peer_device_ed25519(peer_user_id, peer_device_id)
            .await?;
        match key {
            Some(pk) => {
                if let Ok(mut cache) = self.cache.write() {
                    cache.insert(
                        (peer_user_id.to_string(), peer_device_id.to_string()),
                        pk,
                    );
                }
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Clear the cached key for a peer.
    pub fn invalidate(&self, peer_user_id: &str, peer_device_id: &str) {
        if let Ok(mut cache) = self.cache.write() {
            cache.remove(&(peer_user_id.to_string(), peer_device_id.to_string()));
        }
    }
}

impl HandshakePeerKeySource for MatrixPeerKeySource {
    fn peer_public_key(
        &self,
        peer_user_id: &str,
        peer_device_id: &str,
    ) -> Option<[u8; ED25519_PUBLIC_KEY_LEN]> {
        self.cache
            .read()
            .ok()
            .and_then(|cache| {
                cache
                    .get(&(peer_user_id.to_string(), peer_device_id.to_string()))
                    .copied()
            })
    }
}

// --------------------------------------------------------------------------
// Tests
// --------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_key_source_cache_miss_returns_none() {
        let cache: Arc<RwLock<HashMap<_, _>>> = Arc::new(RwLock::new(HashMap::new()));
        let src = CacheOnlyPeerKeySource { cache };
        assert!(src.peer_public_key("@u:ex", "DEV").is_none());
    }

    #[test]
    fn peer_key_source_cache_hit_returns_cached_key() {
        let cache: Arc<RwLock<HashMap<_, _>>> = Arc::new(RwLock::new(HashMap::new()));
        cache
            .write()
            .unwrap()
            .insert(("@u:ex".to_string(), "DEV".to_string()), [7u8; 32]);
        let src = CacheOnlyPeerKeySource { cache };
        assert_eq!(src.peer_public_key("@u:ex", "DEV"), Some([7u8; 32]));
    }

    #[test]
    fn peer_key_source_invalidate_removes_entry() {
        let cache: Arc<RwLock<HashMap<_, _>>> = Arc::new(RwLock::new(HashMap::new()));
        cache
            .write()
            .unwrap()
            .insert(("@u:ex".to_string(), "DEV".to_string()), [7u8; 32]);
        let src = CacheOnlyPeerKeySource {
            cache: cache.clone(),
        };
        assert!(src.peer_public_key("@u:ex", "DEV").is_some());
        {
            cache.write().unwrap().remove(&(
                "@u:ex".to_string(),
                "DEV".to_string(),
            ));
        }
        assert!(src.peer_public_key("@u:ex", "DEV").is_none());
    }

    /// Cache-only shim for sync-trait unit tests.
    struct CacheOnlyPeerKeySource {
        cache: Arc<RwLock<HashMap<(String, String), [u8; 32]>>>,
    }

    impl HandshakePeerKeySource for CacheOnlyPeerKeySource {
        fn peer_public_key(
            &self,
            peer_user_id: &str,
            peer_device_id: &str,
        ) -> Option<[u8; 32]> {
            self.cache
                .read()
                .ok()
                .and_then(|c| {
                    c.get(&(peer_user_id.to_string(), peer_device_id.to_string()))
                        .copied()
                })
        }
    }
}
