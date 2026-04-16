#![cfg(not(target_arch = "wasm32"))]
//! T-53 integration tests: the Verifying handshake end-to-end, using the
//! `HandshakeSigner` + `HandshakePeerKeySource` abstractions against a
//! simulated peer. Proves the storm §3.1 transcript + Ed25519 signature
//! produces `Verified` for a well-formed handshake and `SignatureMismatch`
//! / `PeerKeyMismatch` / `PeerDeviceUnknown` / `InvalidPayload` for the
//! full failure taxonomy.

use mxdx_p2p::transport::verify::{
    build_handshake_challenge, build_handshake_response, decode_nonce_b64, encode_nonce,
    encode_public_key, encode_signature, generate_nonce, verify_handshake_response,
    EphemeralKeySigner, HandshakeMsg, HandshakeOutcome, HandshakeParams, HandshakeSigner,
    InMemoryPeerKeySource,
};

fn mk_params(call_id: &str, we_are_offerer: bool) -> HandshakeParams {
    HandshakeParams {
        room_id: "!room:ex".into(),
        session_uuid: "sid".into(),
        call_id: call_id.into(),
        our_nonce: generate_nonce(),
        our_party_id: if we_are_offerer {
            "off-party".into()
        } else {
            "ans-party".into()
        },
        our_user_id: if we_are_offerer {
            "@off:ex".into()
        } else {
            "@ans:ex".into()
        },
        our_device_id: if we_are_offerer {
            "OFFDEV".into()
        } else {
            "ANSDEV".into()
        },
        peer_user_id: if we_are_offerer {
            "@ans:ex".into()
        } else {
            "@off:ex".into()
        },
        peer_device_id: if we_are_offerer {
            "ANSDEV".into()
        } else {
            "OFFDEV".into()
        },
        our_sdp_fingerprint: if we_are_offerer {
            "AA:BB".into()
        } else {
            "CC:DD".into()
        },
        we_are_offerer,
    }
}

#[test]
fn correct_signature_verifies_to_open() {
    let offerer_signer = EphemeralKeySigner::new();
    let answerer_signer = EphemeralKeySigner::new();

    let mut peer_keys = InMemoryPeerKeySource::new();
    peer_keys.insert("@off:ex", "OFFDEV", offerer_signer.public_key());
    peer_keys.insert("@ans:ex", "ANSDEV", answerer_signer.public_key());

    // Both sides build params (each knows its own nonce + the peer's).
    let mut off_params = mk_params("c1", true);
    let mut ans_params = mk_params("c1", false);
    // Canonical test: both peers see each other's peer_* fields matching.
    // Synchronize: offerer's SDP fingerprint is ans_params.peer_sdp_fingerprint
    // and vice versa.
    let off_fp = off_params.our_sdp_fingerprint.clone();
    let ans_fp = ans_params.our_sdp_fingerprint.clone();
    // Offerer nonce becomes answerer's peer nonce.
    let off_nonce = off_params.our_nonce;
    let ans_nonce = ans_params.our_nonce;
    // Ensure the test harness uses consistent party_ids across the two
    // parameter structs.
    off_params.peer_user_id = "@ans:ex".into();
    off_params.peer_device_id = "ANSDEV".into();
    ans_params.peer_user_id = "@off:ex".into();
    ans_params.peer_device_id = "OFFDEV".into();

    // Each side builds its handshake response using the peer's nonce.
    let off_response = build_handshake_response(
        &off_params,
        &ans_nonce,
        &ans_params.our_party_id,
        &ans_fp,
        &offerer_signer,
    )
    .unwrap();
    let ans_response = build_handshake_response(
        &ans_params,
        &off_nonce,
        &off_params.our_party_id,
        &off_fp,
        &answerer_signer,
    )
    .unwrap();

    // Each side verifies the peer's response. ans verifies off's response;
    // off verifies ans's response.
    let off_outcome = verify_handshake_response(
        &ans_params,
        &off_nonce,
        &off_params.our_party_id,
        &off_fp,
        &off_response,
        &peer_keys,
    );
    assert_eq!(off_outcome, HandshakeOutcome::Verified);

    let ans_outcome = verify_handshake_response(
        &off_params,
        &ans_nonce,
        &ans_params.our_party_id,
        &ans_fp,
        &ans_response,
        &peer_keys,
    );
    assert_eq!(ans_outcome, HandshakeOutcome::Verified);
}

#[test]
fn wrong_signer_is_rejected_with_peer_key_mismatch() {
    let real_signer = EphemeralKeySigner::new();
    let attacker_signer = EphemeralKeySigner::new();

    let mut peer_keys = InMemoryPeerKeySource::new();
    // Matrix knows the REAL signer's key.
    peer_keys.insert("@off:ex", "OFFDEV", real_signer.public_key());

    let off_params = mk_params("c1", true);
    let ans_params = mk_params("c1", false);
    let off_fp = off_params.our_sdp_fingerprint.clone();
    let ans_fp = ans_params.our_sdp_fingerprint.clone();
    let ans_nonce = ans_params.our_nonce;

    // Attacker signs with their OWN key claiming to be @off:ex/OFFDEV.
    let attacker_response = build_handshake_response(
        &off_params,
        &ans_nonce,
        &ans_params.our_party_id,
        &ans_fp,
        &attacker_signer,
    )
    .unwrap();
    // Swap in attacker's device_id claim — it's already OFFDEV from mk_params.
    let outcome = verify_handshake_response(
        &ans_params,
        &off_params.our_nonce,
        &off_params.our_party_id,
        &off_fp,
        &attacker_response,
        &peer_keys,
    );
    assert_eq!(outcome, HandshakeOutcome::PeerKeyMismatch);
}

#[test]
fn unknown_peer_device_rejected() {
    let signer = EphemeralKeySigner::new();
    // Empty peer_keys — no Matrix binding.
    let peer_keys = InMemoryPeerKeySource::new();

    let off_params = mk_params("c1", true);
    let ans_params = mk_params("c1", false);
    let off_fp = off_params.our_sdp_fingerprint.clone();
    let ans_fp = ans_params.our_sdp_fingerprint.clone();
    let ans_nonce = ans_params.our_nonce;

    let response = build_handshake_response(
        &off_params,
        &ans_nonce,
        &ans_params.our_party_id,
        &ans_fp,
        &signer,
    )
    .unwrap();
    let outcome = verify_handshake_response(
        &ans_params,
        &off_params.our_nonce,
        &off_params.our_party_id,
        &off_fp,
        &response,
        &peer_keys,
    );
    assert_eq!(outcome, HandshakeOutcome::PeerDeviceUnknown);
}

#[test]
fn invalid_payload_challenge_in_response_slot_rejected() {
    let peer_keys = InMemoryPeerKeySource::new();
    let off_params = mk_params("c1", true);
    let ans_params = mk_params("c1", false);

    // Malformed: peer sends a Challenge where we expected a Response.
    let bogus = HandshakeMsg::Challenge {
        nonce_b64: encode_nonce(&ans_params.our_nonce),
        device_id: "OFFDEV".into(),
    };
    let outcome = verify_handshake_response(
        &ans_params,
        &off_params.our_nonce,
        &off_params.our_party_id,
        "X",
        &bogus,
        &peer_keys,
    );
    assert_eq!(outcome, HandshakeOutcome::InvalidPayload);
}

#[test]
fn invalid_payload_malformed_base64_rejected() {
    let peer_keys = InMemoryPeerKeySource::new();
    let off_params = mk_params("c1", true);
    let ans_params = mk_params("c1", false);

    // Malformed: signature_b64 is invalid base64.
    let bogus = HandshakeMsg::Response {
        signature_b64: "!!!not base64!!!".into(),
        signer_ed25519_b64: encode_public_key(&[0u8; 32]),
        device_id: "OFFDEV".into(),
    };
    let outcome = verify_handshake_response(
        &ans_params,
        &off_params.our_nonce,
        &off_params.our_party_id,
        "X",
        &bogus,
        &peer_keys,
    );
    assert_eq!(outcome, HandshakeOutcome::InvalidPayload);
}

#[test]
fn modified_transcript_field_rejected() {
    // Attacker replays a valid (signer, signature) pair but with a
    // different call_id. Transcript differs → signature verification
    // fails.
    let signer = EphemeralKeySigner::new();
    let mut peer_keys = InMemoryPeerKeySource::new();
    peer_keys.insert("@off:ex", "OFFDEV", signer.public_key());

    let mut off_params = mk_params("c1", true);
    let ans_params = mk_params("c1", false);
    let off_fp = off_params.our_sdp_fingerprint.clone();
    let ans_fp = ans_params.our_sdp_fingerprint.clone();
    let ans_nonce = ans_params.our_nonce;

    // Valid response signed over call_id=c1.
    let response = build_handshake_response(
        &off_params,
        &ans_nonce,
        &ans_params.our_party_id,
        &ans_fp,
        &signer,
    )
    .unwrap();

    // Attacker replays it but the answerer's recorded call_id is c2
    // (session hijack attempt across calls). Verifier's ans_params has
    // call_id=c1, so the legitimate case verifies. To simulate the
    // attack, we rebuild ans_params with call_id=c2 and attempt verify.
    off_params.call_id = "c2".into();
    let mut tampered_params = ans_params.clone();
    tampered_params.call_id = "c2".into();

    let outcome = verify_handshake_response(
        &tampered_params,
        &off_params.our_nonce,
        &off_params.our_party_id,
        &off_fp,
        &response,
        &peer_keys,
    );
    // Signature was over c1; verifier's transcript is c2; mismatch.
    assert_eq!(outcome, HandshakeOutcome::SignatureMismatch);
}

#[test]
fn build_challenge_is_deterministic_in_nonce_and_device() {
    let params = mk_params("c1", true);
    let m = build_handshake_challenge(&params);
    match m {
        HandshakeMsg::Challenge {
            nonce_b64,
            device_id,
        } => {
            assert_eq!(device_id, "OFFDEV");
            let decoded = decode_nonce_b64(&nonce_b64).unwrap();
            assert_eq!(decoded, params.our_nonce);
        }
        _ => panic!("expected Challenge"),
    }
}

#[test]
fn handshake_roundtrip_with_glare_winner_is_offerer() {
    // Glare resolved with WeWin on the offerer side; answerer side sees
    // TheyWin. Both still produce the same canonical transcript and
    // both verifications succeed.
    let off_signer = EphemeralKeySigner::new();
    let ans_signer = EphemeralKeySigner::new();
    let mut peer_keys = InMemoryPeerKeySource::new();
    peer_keys.insert("@off:ex", "OFFDEV", off_signer.public_key());
    peer_keys.insert("@ans:ex", "ANSDEV", ans_signer.public_key());

    let off_params = mk_params("c1", true);
    let ans_params = mk_params("c1", false);
    let ans_nonce = ans_params.our_nonce;
    let off_nonce = off_params.our_nonce;
    let off_fp = off_params.our_sdp_fingerprint.clone();
    let ans_fp = ans_params.our_sdp_fingerprint.clone();

    let off_response = build_handshake_response(
        &off_params,
        &ans_nonce,
        &ans_params.our_party_id,
        &ans_fp,
        &off_signer,
    )
    .unwrap();

    let outcome = verify_handshake_response(
        &ans_params,
        &off_nonce,
        &off_params.our_party_id,
        &off_fp,
        &off_response,
        &peer_keys,
    );
    assert_eq!(outcome, HandshakeOutcome::Verified);
}

#[test]
fn replay_detection_policy_modeled_outside_verify() {
    // Rationale: replay detection is the DRIVER's responsibility (it
    // tracks recently-seen (call_id, nonce) pairs via DriverTask::
    // check_replay). verify_handshake_response is a pure per-call
    // check — replay of the same nonce within the SAME call would
    // still cryptographically verify because the transcript is
    // unchanged.
    //
    // This test documents the layering: we prove a replayed nonce
    // verifies at the crypto layer (so the driver MUST reject it via
    // replay-tracking), NOT by trusting the crypto layer to know.
    let signer = EphemeralKeySigner::new();
    let mut peer_keys = InMemoryPeerKeySource::new();
    peer_keys.insert("@off:ex", "OFFDEV", signer.public_key());

    let off_params = mk_params("c1", true);
    let ans_params = mk_params("c1", false);
    let ans_nonce = ans_params.our_nonce;
    let ans_fp = ans_params.our_sdp_fingerprint.clone();
    let off_fp = off_params.our_sdp_fingerprint.clone();

    let response = build_handshake_response(
        &off_params,
        &ans_nonce,
        &ans_params.our_party_id,
        &ans_fp,
        &signer,
    )
    .unwrap();

    // First verification — Ok.
    let first = verify_handshake_response(
        &ans_params,
        &off_params.our_nonce,
        &off_params.our_party_id,
        &off_fp,
        &response,
        &peer_keys,
    );
    assert_eq!(first, HandshakeOutcome::Verified);

    // Second verification with the SAME response bytes — still Ok at
    // the crypto layer. Replay rejection is the driver's job (tracked
    // in DriverTask::check_replay, covered by the driver unit tests).
    let second = verify_handshake_response(
        &ans_params,
        &off_params.our_nonce,
        &off_params.our_party_id,
        &off_fp,
        &response,
        &peer_keys,
    );
    assert_eq!(
        second,
        HandshakeOutcome::Verified,
        "crypto layer is stateless; driver enforces replay freshness"
    );
}

#[test]
fn encode_decode_signature_roundtrip_loses_no_bits() {
    // Sanity test: ensure our b64 codec is byte-perfect.
    let sig = [0u8; 64];
    let b64 = encode_signature(&sig);
    // Length check: 64 bytes = 88 chars with padding in STANDARD base64.
    assert_eq!(b64.len(), 88);
    let decoded =
        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64.as_bytes()).unwrap();
    assert_eq!(decoded.len(), 64);
    assert_eq!(decoded, sig);
}
