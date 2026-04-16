//! Sealed newtype that marks bytes as Megolm-encrypted ciphertext.
//!
//! `Megolm<T>` has a package-private constructor. The only way an external
//! caller obtains a `Megolm<Bytes>` is by calling
//! [`MatrixClient::encrypt_for_room`](crate::MatrixClient::encrypt_for_room).
//!
//! This enforces at compile time the project cardinal rule: plaintext cannot
//! reach any send surface that accepts `Megolm<Bytes>`. See ADR
//! `2026-04-15-megolm-bytes-newtype.md` (second addendum: byte-identical
//! ciphertext restored via `OlmMachine::encrypt_room_event_raw`, per ADR
//! `2026-04-16-matrix-sdk-testing-feature.md`).

/// Byte alias matching the storm-spec public signature.
pub type Bytes = Vec<u8>;

/// Newtype marker: the wrapped value is Megolm-encrypted ciphertext.
/// Constructor is package-private to `mxdx-matrix`; only
/// [`MatrixClient::encrypt_for_room`](crate::MatrixClient::encrypt_for_room)
/// may construct one for external callers.
///
/// # Byte-identical ciphertext (second addendum)
///
/// Per ADR `2026-04-15-megolm-bytes-newtype.md` (second addendum, restored
/// by ADR `2026-04-16-matrix-sdk-testing-feature.md`), the inner bytes ARE
/// the actual Megolm-encrypted `m.room.encrypted` JSON produced by
/// `OlmMachine::encrypt_room_event_raw`. Both transport paths carry the
/// same ciphertext:
///
/// - [`MatrixClient::send_megolm`](crate::MatrixClient::send_megolm) sends
///   the already-encrypted content as `m.room.encrypted` without
///   re-encrypting.
/// - `P2PTransport::try_send` wraps the same bytes in an AES-GCM frame
///   whose session key was exchanged inside a Megolm-encrypted
///   `m.call.invite`.
///
/// The type-system marker enforces that these bytes CAN ONLY reach the
/// two authorized send surfaces — there is no public constructor.
///
/// **Callers: NEVER log, persist, or transmit `into_ciphertext_bytes()`
/// output outside of the two authorized send surfaces.** The bytes are
/// ciphertext; the accessor exists only so the send surfaces inside
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
