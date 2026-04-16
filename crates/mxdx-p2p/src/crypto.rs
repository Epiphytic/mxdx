//! AES-256-GCM defense-in-depth layer for the P2P data channel.
//!
//! [`SealedKey`] is a sealed newtype with a `pub(in crate::crypto)` constructor;
//! the only way to transport it to a peer is via `signaling::events::build_invite`,
//! which embeds it in a Megolm-encrypted `m.call.invite`. See ADR
//! `2026-04-15-megolm-bytes-newtype.md`.

use aes_gcm::aead::generic_array::GenericArray;
use aes_gcm::{Aes256Gcm, Key};
use zeroize::Zeroize;

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
// T-11 (P2PCrypto) lives below. Keeping SealedKey above so T-13 trybuild
// tests target a stable minimal surface.
// ---------------------------------------------------------------------------
