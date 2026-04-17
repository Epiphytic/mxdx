#![cfg(not(target_arch = "wasm32"))]
//! Cross-runtime verify-handshake vectors (bead mxdx-fqt).
//!
//! Locks byte-exact transcript-layout parity between Rust
//! `mxdx_p2p::transport::verify::build_transcript` and npm
//! `packages/core/p2p-verify.js::buildTranscript`.
//!
//! Also checks the SDP fingerprint extractor normalizations.
//!
//! Regenerate the fixture with:
//!
//!     cargo test -p mxdx-p2p --test verify_vectors -- \
//!         --ignored generate_verify_vectors --exact --nocapture

use base64::Engine as _;
use mxdx_p2p::transport::verify::{build_transcript, extract_sdp_fingerprint, NONCE_LEN};
use serde_json::Value;
use std::path::PathBuf;

const FIXTURE_VERSION: u32 = 1;

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("verify-vectors.json")
}

fn load_fixture() -> Value {
    let data = std::fs::read_to_string(fixture_path()).expect("fixture file");
    let v: Value = serde_json::from_str(&data).expect("valid JSON");
    assert_eq!(
        v["version"].as_u64().unwrap() as u32,
        FIXTURE_VERSION,
        "fixture version mismatch"
    );
    v
}

fn b64(s: &str) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .expect("valid base64")
}

fn hex_encode(b: &[u8]) -> String {
    let mut out = String::with_capacity(b.len() * 2);
    for byte in b {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn hex_decode(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn inputs_to_transcript(inputs: &Value) -> Vec<u8> {
    let offerer_nonce_vec = b64(inputs["offerer_nonce_b64"].as_str().unwrap());
    let answerer_nonce_vec = b64(inputs["answerer_nonce_b64"].as_str().unwrap());
    assert_eq!(offerer_nonce_vec.len(), NONCE_LEN);
    assert_eq!(answerer_nonce_vec.len(), NONCE_LEN);
    let mut o = [0u8; NONCE_LEN];
    let mut a = [0u8; NONCE_LEN];
    o.copy_from_slice(&offerer_nonce_vec);
    a.copy_from_slice(&answerer_nonce_vec);

    build_transcript(
        inputs["room_id"].as_str().unwrap(),
        inputs["session_uuid"].as_str().unwrap(),
        inputs["call_id"].as_str().unwrap(),
        &o,
        &a,
        inputs["offerer_party_id"].as_str().unwrap(),
        inputs["answerer_party_id"].as_str().unwrap(),
        inputs["offerer_sdp_fingerprint"].as_str().unwrap(),
        inputs["answerer_sdp_fingerprint"].as_str().unwrap(),
    )
}

#[test]
fn rust_transcript_matches_fixture_bytes() {
    let fixture = load_fixture();
    let vec = &fixture["transcript_vector_basic"];
    let expected_hex = vec["transcript_hex"].as_str().unwrap();
    let computed = inputs_to_transcript(&vec["inputs"]);
    let computed_hex = hex_encode(&computed);
    assert_eq!(
        computed_hex, expected_hex,
        "Rust-computed transcript bytes must match fixture exactly.\n\
         Expected: {expected_hex}\n\
         Computed: {computed_hex}\n\
         If this test fails after a schema change, regenerate the fixture via\n\
         `cargo test -p mxdx-p2p --test verify_vectors -- --ignored generate_verify_vectors --exact --nocapture`\n\
         AND also update `packages/core/p2p-verify.js` in a coordinated release."
    );

    // Sanity: computed bytes decode into valid input-bound form.
    let expected_bytes = hex_decode(expected_hex);
    assert_eq!(computed, expected_bytes);
}

#[test]
fn sdp_fingerprint_normalization_matches_fixture() {
    let fixture = load_fixture();
    let cases = fixture["sdp_fingerprint_normalization"]["cases"]
        .as_array()
        .unwrap();
    for case in cases {
        let sdp = case["sdp"].as_str().unwrap();
        let expected = case["expected"].as_str().unwrap();
        let got = extract_sdp_fingerprint(sdp).expect("fingerprint extraction");
        assert_eq!(got, expected, "SDP fingerprint normalization for {sdp:?}");
    }
}

/// Regenerate the fixture. Run via
/// `cargo test -p mxdx-p2p --test verify_vectors -- --ignored generate_verify_vectors --exact --nocapture`.
#[test]
#[ignore]
fn generate_verify_vectors() {
    let mut fixture = load_fixture();
    let computed = inputs_to_transcript(&fixture["transcript_vector_basic"]["inputs"]);
    fixture["transcript_vector_basic"]["transcript_hex"] = Value::String(hex_encode(&computed));
    let pretty = serde_json::to_string_pretty(&fixture).unwrap();
    std::fs::write(fixture_path(), pretty).expect("write fixture");
    eprintln!("regenerated fixture at {}", fixture_path().display());
}
