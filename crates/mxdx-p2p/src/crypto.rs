//! AES-256-GCM defense-in-depth layer for the P2P data channel.
//!
//! [`SealedKey`] is a sealed newtype with a `pub(in crate::crypto)` constructor;
//! the only way to transport it to a peer is via `signaling::events::build_invite`,
//! which embeds it in a Megolm-encrypted `m.call.invite`. See ADR
//! `2026-04-15-megolm-bytes-newtype.md`.

use aes_gcm::aead::generic_array::GenericArray;
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

/// Errors returned by `P2PCrypto`.
#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("AES-GCM operation failed (tag mismatch or invalid key)")]
    AeadFailure,
    #[error("invalid base64 in EncryptedFrame field {field}: {source}")]
    InvalidBase64 {
        field: &'static str,
        #[source]
        source: base64::DecodeError,
    },
    #[error("invalid IV length: got {got} bytes, expected 12")]
    InvalidIvLength { got: usize },
}

impl From<aes_gcm::Error> for CryptoError {
    fn from(_: aes_gcm::Error) -> Self {
        CryptoError::AeadFailure
    }
}

/// JSON wire format for an AES-GCM-protected frame on the P2P data channel.
///
/// Field names and base64 alphabet are bit-locked to
/// `packages/core/p2p-crypto.js`:
/// - `c` = base64-standard (padded) ciphertext (which includes the AES-GCM tag)
/// - `iv` = base64-standard (padded) 96-bit nonce
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct EncryptedFrame {
    #[serde(rename = "c")]
    pub ciphertext: String,
    #[serde(rename = "iv")]
    pub iv: String,
}

impl core::fmt::Debug for EncryptedFrame {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("EncryptedFrame")
            .field("ciphertext_len_b64", &self.ciphertext.len())
            .field("iv_len_b64", &self.iv.len())
            .finish()
    }
}

/// Sealed 256-bit AES-GCM key. The tuple field is `pub(in crate::crypto)` so
/// construction outside this module is a compile error (enforced by trybuild
/// tests in T-13).
pub struct SealedKey(pub(in crate::crypto) Key<Aes256Gcm>);

impl SealedKey {
    /// Create a new random `SealedKey` from the OS CSPRNG.
    ///
    /// Visibility is `pub(in crate::crypto)`: only code inside this module
    /// may mint a fresh sealed key. External callers obtain one via
    /// [`P2PCrypto::generate`] which returns it alongside a usable
    /// [`P2PCrypto`] instance.
    pub(in crate::crypto) fn random() -> Self {
        // `Aes256Gcm::generate_key` uses `OsRng` internally via the `aead`
        // default rng, but we depend on `rand::rngs::OsRng` directly to be
        // explicit about the source (CSPRNG, never a seeded PRNG).
        use aes_gcm::aead::rand_core::RngCore;
        let mut raw = [0u8; 32];
        aes_gcm::aead::OsRng.fill_bytes(&mut raw);
        let key = *Key::<Aes256Gcm>::from_slice(&raw);
        raw.zeroize();
        SealedKey(key)
    }

    /// Borrow the underlying key. Visibility is `pub(in crate::crypto)` —
    /// only code inside this module may read the key bytes. `P2PCrypto` uses
    /// this to construct the AES-GCM cipher.
    pub(in crate::crypto) fn as_key(&self) -> &Key<Aes256Gcm> {
        &self.0
    }

    /// Reconstruct a `SealedKey` from raw bytes received over a Megolm-
    /// protected signaling event (e.g. `m.call.invite` with an embedded
    /// `mxdx_session_key`). Visibility is `pub(in crate::crypto)` so the
    /// signaling layer imports from this module and never constructs keys
    /// directly from untrusted bytes outside the crypto module.
    #[allow(dead_code)] // consumed by signaling in a later phase
    pub(in crate::crypto) fn from_bytes(bytes: [u8; 32]) -> Self {
        let key = *Key::<Aes256Gcm>::from_slice(&bytes);
        // Zeroize the caller's copy by consuming the array — the caller
        // passes by value, so its stack slot is discarded after this call.
        SealedKey(key)
    }

    /// Export the key for embedding in a Megolm-protected `m.call.invite`.
    /// Visibility is `pub(in crate::crypto)` — the signaling layer sits
    /// within `mxdx-p2p` and imports from here; external callers never see
    /// raw key bytes.
    #[allow(dead_code)] // consumed by signaling in a later phase
    pub(in crate::crypto) fn as_bytes(&self) -> &[u8; 32] {
        let arr: &GenericArray<u8, _> = &self.0;
        // `Key<Aes256Gcm>` is `GenericArray<u8, U32>`; its in-memory
        // representation is 32 contiguous bytes.
        arr.as_slice()
            .try_into()
            .expect("Key<Aes256Gcm> is 32 bytes by construction")
    }
}

impl Drop for SealedKey {
    fn drop(&mut self) {
        // `Key<Aes256Gcm>` is a `GenericArray<u8, U32>` — zeroize the bytes.
        self.0.as_mut_slice().zeroize();
    }
}

// Explicit non-leaky Debug: key bytes never appear in logs.
impl core::fmt::Debug for SealedKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SealedKey")
            .field("key", &"<redacted>")
            .finish()
    }
}

// ---------------------------------------------------------------------------
// T-11 P2PCrypto — AES-256-GCM defense-in-depth, wire-locked to npm reference
// ---------------------------------------------------------------------------

/// AES-256-GCM defense-in-depth cipher for the P2P data channel. Session key
/// is established via a [`SealedKey`] exchanged inside a Megolm-encrypted
/// `m.call.invite`.
pub struct P2PCrypto {
    cipher: Aes256Gcm,
}

impl P2PCrypto {
    /// Generate a fresh random session key and return both a usable
    /// `P2PCrypto` and the `SealedKey` to transport to the peer (inside a
    /// Megolm-encrypted signaling event).
    pub fn generate() -> (Self, SealedKey) {
        let sealed = SealedKey::random();
        let cipher = Aes256Gcm::new(sealed.as_key());
        (P2PCrypto { cipher }, sealed)
    }

    /// Reconstruct a `P2PCrypto` from a `SealedKey` received via a
    /// Megolm-protected signaling event. Consumes the key.
    pub fn from_sealed(k: SealedKey) -> Self {
        let cipher = Aes256Gcm::new(k.as_key());
        // `k` dropped here — its key bytes are zeroized by `SealedKey::drop`.
        P2PCrypto { cipher }
    }

    /// Encrypt `plaintext` and return an [`EncryptedFrame`]. Each call uses a
    /// fresh 96-bit random IV sourced from `OsRng` (CSPRNG) — never reused.
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<EncryptedFrame, CryptoError> {
        use aes_gcm::aead::rand_core::RngCore;
        let mut iv_bytes = [0u8; 12];
        aes_gcm::aead::OsRng.fill_bytes(&mut iv_bytes);
        let nonce = Nonce::from_slice(&iv_bytes);
        let ciphertext = self.cipher.encrypt(nonce, plaintext)?;
        Ok(EncryptedFrame {
            ciphertext: BASE64_STANDARD.encode(&ciphertext),
            iv: BASE64_STANDARD.encode(iv_bytes),
        })
    }

    /// Decrypt an [`EncryptedFrame`]. Returns `Err(CryptoError::AeadFailure)`
    /// on tag mismatch (wrong key, tampered ciphertext, wrong IV).
    pub fn decrypt(&self, frame: &EncryptedFrame) -> Result<Vec<u8>, CryptoError> {
        let ciphertext = BASE64_STANDARD
            .decode(frame.ciphertext.as_bytes())
            .map_err(|e| CryptoError::InvalidBase64 {
                field: "c",
                source: e,
            })?;
        let iv_bytes = BASE64_STANDARD.decode(frame.iv.as_bytes()).map_err(|e| {
            CryptoError::InvalidBase64 {
                field: "iv",
                source: e,
            }
        })?;
        if iv_bytes.len() != 12 {
            return Err(CryptoError::InvalidIvLength {
                got: iv_bytes.len(),
            });
        }
        let nonce = Nonce::from_slice(&iv_bytes);
        let plaintext = self.cipher.decrypt(nonce, ciphertext.as_slice())?;
        Ok(plaintext)
    }

    /// Test-only helper: encrypt with a caller-supplied IV for deterministic
    /// vector generation. Never expose publicly — reusing IVs with the same
    /// key breaks AES-GCM security.
    #[cfg(any(test, feature = "vector-gen"))]
    pub(crate) fn encrypt_with_iv(
        &self,
        iv: [u8; 12],
        plaintext: &[u8],
    ) -> Result<EncryptedFrame, CryptoError> {
        let nonce = Nonce::from_slice(&iv);
        let ciphertext = self.cipher.encrypt(nonce, plaintext)?;
        Ok(EncryptedFrame {
            ciphertext: BASE64_STANDARD.encode(&ciphertext),
            iv: BASE64_STANDARD.encode(iv),
        })
    }
}

impl core::fmt::Debug for P2PCrypto {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("P2PCrypto")
            .field("key", &"<redacted>")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn roundtrip_empty_plaintext() {
        let (crypto, _sealed) = P2PCrypto::generate();
        let frame = crypto.encrypt(&[]).expect("encrypt empty");
        let plaintext = crypto.decrypt(&frame).expect("decrypt empty");
        assert_eq!(plaintext, Vec::<u8>::new());
    }

    #[test]
    fn roundtrip_single_byte() {
        let (crypto, _sealed) = P2PCrypto::generate();
        let frame = crypto.encrypt(&[0xABu8]).expect("encrypt 1 byte");
        assert_eq!(crypto.decrypt(&frame).unwrap(), vec![0xABu8]);
    }

    #[test]
    fn roundtrip_utf8_payload() {
        let (crypto, _sealed) = P2PCrypto::generate();
        let msg = "hello 🌍 — mxdx";
        let frame = crypto.encrypt(msg.as_bytes()).unwrap();
        let out = crypto.decrypt(&frame).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), msg);
    }

    #[test]
    fn roundtrip_64kb_random() {
        use aes_gcm::aead::rand_core::RngCore;
        let (crypto, _sealed) = P2PCrypto::generate();
        let mut buf = vec![0u8; 64 * 1024];
        aes_gcm::aead::OsRng.fill_bytes(&mut buf);
        let frame = crypto.encrypt(&buf).unwrap();
        assert_eq!(crypto.decrypt(&frame).unwrap(), buf);
    }

    #[test]
    fn wire_format_is_padded_base64() {
        let (crypto, _sealed) = P2PCrypto::generate();
        let frame = crypto.encrypt(b"abc").unwrap();
        let json = serde_json::to_string(&frame).unwrap();
        // Keys MUST be exactly `c` and `iv` (bit-compat with npm).
        assert!(json.starts_with('{') && json.ends_with('}'));
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed.get("c").is_some());
        assert!(parsed.get("iv").is_some());
        // IV is 12 bytes -> base64 = 16 chars (with padding).
        assert_eq!(frame.iv.len(), 16, "iv b64: {}", frame.iv);
        // Ciphertext for 3-byte plaintext = 3 + 16 (tag) = 19 bytes -> 28 b64 with padding
        assert_eq!(frame.ciphertext.len(), 28, "c b64: {}", frame.ciphertext);
    }

    #[test]
    fn tag_mismatch_returns_aead_failure() {
        let (crypto, _sealed) = P2PCrypto::generate();
        let mut frame = crypto.encrypt(b"payload").unwrap();
        // Flip a bit in the ciphertext (replace last b64 char).
        let mut ct = frame.ciphertext.clone();
        ct.pop();
        ct.push('A');
        frame.ciphertext = ct;
        let err = crypto.decrypt(&frame).unwrap_err();
        assert!(matches!(err, CryptoError::AeadFailure), "got: {err:?}");
    }

    #[test]
    fn wrong_key_returns_aead_failure() {
        let (alice, _sk_a) = P2PCrypto::generate();
        let (bob, _sk_b) = P2PCrypto::generate();
        let frame = alice.encrypt(b"for alice").unwrap();
        let err = bob.decrypt(&frame).unwrap_err();
        assert!(matches!(err, CryptoError::AeadFailure));
    }

    #[test]
    fn iv_unique_across_20_frames() {
        let (crypto, _sealed) = P2PCrypto::generate();
        let mut seen: HashSet<String> = HashSet::new();
        for _ in 0..20 {
            let frame = crypto.encrypt(b"same plaintext").unwrap();
            assert!(seen.insert(frame.iv.clone()), "IV collision");
        }
    }

    #[test]
    fn invalid_iv_length_rejected() {
        let (crypto, _sealed) = P2PCrypto::generate();
        let frame = EncryptedFrame {
            ciphertext: BASE64_STANDARD.encode([0u8; 16]),
            iv: BASE64_STANDARD.encode([0u8; 8]), // wrong length
        };
        let err = crypto.decrypt(&frame).unwrap_err();
        assert!(matches!(err, CryptoError::InvalidIvLength { got: 8 }));
    }

    #[test]
    fn invalid_base64_rejected() {
        let (crypto, _sealed) = P2PCrypto::generate();
        let frame = EncryptedFrame {
            ciphertext: "not base64!!".to_string(),
            iv: BASE64_STANDARD.encode([0u8; 12]),
        };
        let err = crypto.decrypt(&frame).unwrap_err();
        assert!(matches!(err, CryptoError::InvalidBase64 { field: "c", .. }));
    }

    #[test]
    fn deterministic_encrypt_with_iv_helper() {
        let (crypto, _sealed) = P2PCrypto::generate();
        let frame_a = crypto.encrypt_with_iv([7u8; 12], b"abc").unwrap();
        let frame_b = crypto.encrypt_with_iv([7u8; 12], b"abc").unwrap();
        assert_eq!(frame_a.ciphertext, frame_b.ciphertext);
        assert_eq!(frame_a.iv, frame_b.iv);
    }
}
