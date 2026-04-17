// Negative test: constructing `SealedKey` from outside the
// `mxdx_p2p::crypto` module must fail to compile. The tuple field is
// `pub(in crate::crypto)`, so neither the tuple-struct constructor
// `SealedKey(...)` nor any associated constructor is callable from
// external crates.
//
// See ADR `2026-04-15-megolm-bytes-newtype.md`. If this file starts
// compiling, the E2EE invariant has been weakened — re-seal the newtype.

use aes_gcm::aead::generic_array::GenericArray;
use aes_gcm::{Aes256Gcm, Key};
use mxdx_p2p::crypto::SealedKey;

fn main() {
    let raw: Key<Aes256Gcm> = *GenericArray::from_slice(&[0u8; 32]);
    let _leaked = SealedKey(raw);
}
