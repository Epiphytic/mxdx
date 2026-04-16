#![cfg(not(target_arch = "wasm32"))]
//! Cross-language crypto vectors: decrypt the committed JSON fixture in Rust
//! and assert the plaintexts match. The Node-side test lives at
//! `packages/e2e-tests/tests/rust-npm-crypto-vectors.test.js` and runs the
//! same fixture through `packages/core/p2p-crypto.js`.
//!
//! Gated on the `vector-gen` crate feature because injecting a known key into
//! a `P2PCrypto` requires a test-only constructor (`from_raw_key_for_testing`)
//! that is never exposed in production. Run with:
//!
//!     cargo test -p mxdx-p2p --features vector-gen --test crypto_vectors
//!
//! Regenerate the fixture with:
//!
//!     cargo test -p mxdx-p2p --features vector-gen --test crypto_vectors \
//!         -- --ignored generate_vectors --exact --nocapture
//!
//! …or via the npm helper:
//!
//!     node packages/e2e-tests/scripts/regenerate-p2p-vectors.mjs

#![cfg(feature = "vector-gen")]

use std::path::PathBuf;

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use mxdx_p2p::crypto::{EncryptedFrame, P2PCrypto};
use serde::{Deserialize, Serialize};

const FIXTURE_VERSION: u32 = 1;

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("crypto-vectors.json")
}

#[derive(Serialize, Deserialize)]
struct Vector {
    name: String,
    key_b64: String,
    iv_b64: String,
    plaintext_b64: String,
    ciphertext_b64: String,
    /// Optional human-readable plaintext for documentation/debug only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    plaintext_utf8: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Fixture {
    version: u32,
    description: String,
    vectors: Vec<Vector>,
}

fn load_fixture() -> Fixture {
    let path = fixture_path();
    let data = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read fixture {}: {e}", path.display()));
    serde_json::from_str(&data).expect("valid JSON fixture")
}

/// Load a `P2PCrypto` from a base64-encoded 32-byte key. Uses the gated
/// `from_raw_key_for_testing` constructor so we can pin keys for fixture
/// reproducibility; this entry point is never available in production.
fn crypto_from_b64(key_b64: &str) -> P2PCrypto {
    let key_bytes = BASE64_STANDARD.decode(key_b64).expect("b64 key");
    assert_eq!(key_bytes.len(), 32, "key must be 32 bytes");
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&key_bytes);
    P2PCrypto::from_raw_key_for_testing(arr)
}

#[test]
fn decrypt_all_vectors() {
    let fixture = load_fixture();
    assert_eq!(fixture.version, FIXTURE_VERSION);
    assert!(!fixture.vectors.is_empty(), "fixture has no vectors");
    for v in &fixture.vectors {
        let crypto = crypto_from_b64(&v.key_b64);
        let frame = EncryptedFrame {
            ciphertext: v.ciphertext_b64.clone(),
            iv: v.iv_b64.clone(),
        };
        let plaintext = crypto
            .decrypt(&frame)
            .unwrap_or_else(|e| panic!("decrypt vector {}: {e:?}", v.name));
        let expected = BASE64_STANDARD
            .decode(&v.plaintext_b64)
            .expect("b64 plaintext");
        assert_eq!(plaintext, expected, "vector {}", v.name);
    }
}

/// Regenerate the committed fixture. Ignored by default — run with
/// `cargo test -p mxdx-p2p --features vector-gen --test crypto_vectors -- \
///  --ignored generate_vectors --exact --nocapture`.
///
/// Uses a ChaCha20Rng seeded with a constant so regeneration is reproducible
/// (though AES-GCM ciphertext is deterministic given key+iv+plaintext, so
/// the fixture is byte-stable regardless of the seed choice).
#[test]
#[ignore = "regenerates the committed fixture; run explicitly when updating vectors"]
fn generate_vectors() {
    use rand::RngCore;
    use rand::SeedableRng;
    use rand_chacha::ChaCha20Rng;

    // Seed is a constant so the fixture is reproducible (though AES-GCM
    // ciphertext is already deterministic given key + IV + plaintext).
    let mut _rng = ChaCha20Rng::seed_from_u64(0x6D78_6478_7631); // "mxdxv1"
    let _ = (&mut _rng,);

    // Use a fixed key and fixed IVs across all vectors for determinism.
    let mut key = [0u8; 32];
    for (i, b) in key.iter_mut().enumerate() {
        *b = i as u8;
    }
    let crypto = mxdx_p2p::crypto::P2PCrypto::from_raw_key_for_testing(key);

    let mut vectors: Vec<Vector> = Vec::new();

    // (name, plaintext, iv_filler, optional utf8 preview)
    let inputs: Vec<(&str, Vec<u8>, u8, Option<&str>)> = vec![
        ("empty", vec![], 0x01, Some("")),
        ("one_byte", vec![0x2Au8], 0x02, None),
        (
            "1kb_random",
            {
                let mut b = vec![0u8; 1024];
                for (i, x) in b.iter_mut().enumerate() {
                    *x = (i as u8).wrapping_mul(17).wrapping_add(3);
                }
                b
            },
            0x03,
            None,
        ),
        (
            "64kb_random",
            {
                let mut b = vec![0u8; 64 * 1024];
                for (i, x) in b.iter_mut().enumerate() {
                    *x = (i as u8).wrapping_mul(37).wrapping_add(11);
                }
                b
            },
            0x04,
            None,
        ),
        (
            "framing_pitfall",
            br#"plain but looks like a frame: "c":"abc","iv":"xyz""#.to_vec(),
            0x05,
            Some(r#"plain but looks like a frame: "c":"abc","iv":"xyz""#),
        ),
    ];

    for (name, plaintext, iv_filler, utf8_preview) in inputs {
        let iv = [iv_filler; 12];
        let frame = crypto
            .encrypt_with_iv(iv, &plaintext)
            .expect("encrypt vector");
        vectors.push(Vector {
            name: name.to_string(),
            key_b64: BASE64_STANDARD.encode(key),
            iv_b64: frame.iv.clone(),
            plaintext_b64: BASE64_STANDARD.encode(&plaintext),
            ciphertext_b64: frame.ciphertext.clone(),
            plaintext_utf8: utf8_preview.map(|s| s.to_string()),
        });
    }

    let fixture = Fixture {
        version: FIXTURE_VERSION,
        description: "Cross-language AES-256-GCM vectors for mxdx P2P. Do not edit by hand."
            .to_string(),
        vectors,
    };

    let path = fixture_path();
    let json = serde_json::to_string_pretty(&fixture).expect("serialize fixture");
    std::fs::write(&path, format!("{}\n", json)).expect("write fixture");
    eprintln!("wrote fixture: {}", path.display());
}
