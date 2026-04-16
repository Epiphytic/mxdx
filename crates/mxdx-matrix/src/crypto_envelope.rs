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
/// The bytes inside a `Megolm<Bytes>` are intended to be delivered to a send
/// surface (`MatrixClient::send_megolm` for the Matrix fallback path, or
/// `P2PTransport::try_send` for the P2P path) which treats them as an opaque,
/// already-Megolm-protected payload.
pub struct Megolm<T>(pub(crate) T);

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
