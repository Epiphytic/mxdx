#![cfg(not(target_arch = "wasm32"))]
//! Structural E2EE invariant: constructing `SealedKey` outside
//! `mxdx-p2p::crypto` MUST be a compile error. See ADR
//! `2026-04-15-megolm-bytes-newtype.md`.
//!
//! If this test turns red, some future change weakened the newtype. Re-seal
//! it — do not relax the test.

#[test]
fn sealedkey_constructor_compile_fail() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/trybuild/sealedkey-constructor-fails.rs");
}
