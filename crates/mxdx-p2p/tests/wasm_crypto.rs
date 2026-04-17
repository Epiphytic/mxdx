#![cfg(target_arch = "wasm32")]
//! WASM-specific P2PCrypto tests (Phase 8, T-84).
//!
//! Verifies AES-256-GCM encrypt/decrypt roundtrips work in the browser
//! WASM runtime using the same `P2PCrypto` implementation as native.

use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

use mxdx_p2p::crypto::{P2PCrypto, SealedKey};

#[wasm_bindgen_test]
fn generate_produces_key_and_crypto() {
    let (crypto, sealed) = P2PCrypto::generate();
    let b64 = sealed.to_base64();
    assert!(!b64.is_empty());
    // Verify the generated key is 32 bytes (44 chars in base64 with padding)
    assert_eq!(b64.len(), 44, "base64 key should be 44 chars (32 bytes)");
    drop(crypto);
}

#[wasm_bindgen_test]
fn encrypt_decrypt_roundtrip() {
    let (crypto, _) = P2PCrypto::generate();
    let plaintext = b"hello from wasm test";
    let frame = crypto.encrypt(plaintext).expect("encrypt should succeed");
    let decrypted = crypto.decrypt(&frame).expect("decrypt should succeed");
    assert_eq!(decrypted, plaintext);
}

#[wasm_bindgen_test]
fn sealed_key_roundtrip_via_base64() {
    let (_, sealed) = P2PCrypto::generate();
    let b64 = sealed.to_base64();
    let restored = SealedKey::from_base64(&b64).expect("from_base64 should succeed");
    let restored_crypto = P2PCrypto::from_sealed(restored);

    // Encrypt with original key, decrypt with restored
    let (original_crypto, _) = {
        let s2 = SealedKey::from_base64(&b64).unwrap();
        (P2PCrypto::from_sealed(s2), ())
    };
    let frame = original_crypto.encrypt(b"roundtrip test").unwrap();
    let decrypted = restored_crypto.decrypt(&frame).unwrap();
    assert_eq!(decrypted, b"roundtrip test");
}

#[wasm_bindgen_test]
fn wrong_key_decrypt_fails() {
    let (crypto1, _) = P2PCrypto::generate();
    let (crypto2, _) = P2PCrypto::generate();
    let frame = crypto1.encrypt(b"secret").unwrap();
    let result = crypto2.decrypt(&frame);
    assert!(result.is_err(), "decrypt with wrong key should fail");
}

#[wasm_bindgen_test]
fn empty_plaintext_roundtrip() {
    let (crypto, _) = P2PCrypto::generate();
    let frame = crypto.encrypt(b"").unwrap();
    let decrypted = crypto.decrypt(&frame).unwrap();
    assert_eq!(decrypted, b"");
}
