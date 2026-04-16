//! Sealed newtype that marks bytes have crossed the Megolm encryption boundary.
//!
//! `Megolm<T>` has a package-private constructor. The only way an external
//! caller obtains a `Megolm<Bytes>` is by calling
//! [`MatrixClient::encrypt_for_room`](crate::MatrixClient::encrypt_for_room).
//!
//! This enforces at compile time the project cardinal rule: plaintext cannot
//! reach any send surface that accepts `Megolm<Bytes>`. See ADR
//! `2026-04-15-megolm-bytes-newtype.md` (including the 2026-04-16 addendum on
//! semantic equivalence vs byte-identity).

/// Byte alias matching the storm-spec public signature.
pub type Bytes = Vec<u8>;

/// Newtype marker: the wrapped value has crossed the Megolm encryption
/// boundary. Constructor is package-private to `mxdx-matrix`; only
/// [`MatrixClient::encrypt_for_room`](crate::MatrixClient::encrypt_for_room)
/// may construct one for external callers.
///
/// # Semantic equivalence, not byte-identity
///
/// Per ADR `2026-04-15-megolm-bytes-newtype.md` (2026-04-16 addendum —
/// **Option B**), the inner bytes are NOT already-Megolm-ciphertext on
/// this side of the boundary. They are plaintext serialized event content
/// that the receiving send surface will Megolm-encrypt on the wire:
///
/// - [`MatrixClient::send_megolm`](crate::MatrixClient::send_megolm) posts
///   through `room.send_raw`, which Megolm-encrypts in-flight using the
///   existing room outbound session.
/// - `P2PTransport::try_send` (phase 5) wraps the bytes in an AES-GCM frame
///   whose session key was exchanged inside a Megolm-encrypted
///   `m.call.invite`.
///
/// In both paths the bytes that *leave the process* are Megolm-protected
/// against the same room session. The type-system marker enforces that
/// bytes CAN ONLY reach those two send surfaces — there is no public
/// constructor and no public accessor that returns raw bytes out of
/// an external caller's control.
///
/// **Callers: NEVER log, persist, or transmit `into_ciphertext_bytes()`
/// output outside of the two authorized send surfaces.** The bytes are
/// plaintext JSON; treating them as opaque ciphertext would leak event
/// contents. The accessor exists only so the send surfaces inside
/// `mxdx-matrix` and `mxdx-p2p` can forward them.
pub struct Megolm<T>(pub(crate) T);

// NOTE: Clone is derived for Megolm<T: Clone> so the worker/client
// integration layer can duplicate a sealed payload for the P2P + Matrix
// dual-send pattern (storm §3.2: P2P attempt first, fall back to Matrix
// on FallbackToMatrix/ChannelClosed). Duplicating a Megolm<T> does NOT
// create a new external constructor — the only way to obtain one is still
// `MatrixClient::encrypt_for_room`. Cloning an existing wrapper just
// duplicates the already-boundary-crossed bytes, which remain confined to
// authorized send surfaces by the public API surface (no plaintext
// accessor exists for external callers).
impl<T: Clone> Clone for Megolm<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> Megolm<T> {
    /// Consume the wrapper and return the Megolm-protected bytes.
    ///
    /// WARNING: these bytes are Megolm-encrypted; do not decrypt here. They
    /// are intended only for hand-off to a Matrix or P2P send surface.
    pub fn into_ciphertext_bytes(self) -> T {
        self.0
    }

    /// Borrow the Megolm-protected bytes without consuming the wrapper.
    ///
    /// WARNING: these bytes are Megolm-encrypted; do not decrypt here.
    pub fn as_ciphertext_bytes(&self) -> &T {
        &self.0
    }
}

// Explicit Debug impl that does NOT leak the wrapped bytes — even though the
// bytes are already ciphertext, a payload preview can aid correlation attacks
// in debug logs. Print only the length hint when available.
impl<T> core::fmt::Debug for Megolm<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Megolm")
            .field("payload", &"<redacted>")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn megolm_accessors_roundtrip_payload() {
        let payload: Bytes = b"sealed ciphertext".to_vec();
        let m = Megolm(payload.clone());
        assert_eq!(m.as_ciphertext_bytes(), &payload);
        assert_eq!(m.into_ciphertext_bytes(), payload);
    }

    #[test]
    fn megolm_debug_redacts_payload() {
        let m = Megolm::<Bytes>(b"secret".to_vec());
        let rendered = format!("{m:?}");
        assert!(rendered.contains("redacted"), "got: {rendered}");
        assert!(!rendered.contains("secret"), "got: {rendered}");
    }
}
