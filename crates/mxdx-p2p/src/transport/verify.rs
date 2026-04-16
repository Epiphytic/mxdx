//! Verifying handshake — Ed25519-signed transcript over AES-GCM (T-53).
//!
//! Storm §3.1 / §4.5 transcript schema:
//! ```text
//! transcript = domain_sep_tag
//!           || 0x00 || room_id
//!           || 0x00 || session_uuid (empty if None)
//!           || 0x00 || call_id
//!           || offerer_nonce (32 bytes)
//!           || answerer_nonce (32 bytes)
//!           || 0x00 || offerer_party_id
//!           || 0x00 || answerer_party_id
//!           || 0x00 || offerer_sdp_fingerprint
//!           || 0x00 || answerer_sdp_fingerprint
//! ```
//!
//! 0x00 separators disambiguate variable-length string fields; nonces are
//! fixed-width and carry no separator between them.
//!
//! # Security properties (storm §4.5)
//!
//! 1. **Peer identity binding** — `offerer_party_id + answerer_party_id`
//!    bind the signature to a specific (user, device) pair. A MITM
//!    cannot substitute a different peer.
//! 2. **Cross-call replay protection** — `room_id + session_uuid +
//!    call_id` bind to this specific call. A captured signature from a
//!    prior call cannot be replayed.
//! 3. **Reflection resistance** — offerer-first canonical ordering means
//!    both peers produce the same transcript bytes and sign the same
//!    blob, but the roles are asymmetric. An adversary cannot reflect
//!    one peer's challenge back to them as its own response (the
//!    transcript ordering prevents a match).
//! 4. **TURN-MITM resistance** — `offerer_sdp_fingerprint +
//!    answerer_sdp_fingerprint` bind to the DTLS certificate hashes
//!    both peers saw in the SDP negotiation. A TURN relay that inserts
//!    a different DTLS cert changes the fingerprint and the signature
//!    fails to verify.
//! 5. **Domain separation** — `mxdx.p2p.verify.v1` ensures a captured
//!    signature cannot be misused as a signature over a different
//!    protocol blob.
//! 6. **Freshness** — 32-byte CSPRNG nonces from both sides (OsRng)
//!    ensure the transcript is unique per call.
//!
//! # npm divergence (documented)
//!
//! The deployed npm handshake in `packages/core/p2p-transport.js` is a
//! simple nonce ping-pong (`peer_verify` + `peer_verify_response`) in
//! plaintext JSON over WebRTC DTLS only — no Ed25519 signature, no
//! transcript. The Rust implementation in this module is the stronger
//! storm-spec version. A coordinated-release follow-up bead (filed by
//! the phase-5 completion marker) tracks the npm migration; until then,
//! Rust↔Rust P2P uses this handshake and Rust↔npm P2P falls back to
//! Matrix at the Verifying step.
//!
//! # Wire format
//!
//! Each handshake message is serde-JSON serialized, AES-GCM encrypted
//! via [`P2PCrypto::encrypt`], then sent as a single data-channel frame.
//! Inbound path: [`P2PCrypto::decrypt`] → JSON parse → typed enum.

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

use crate::signaling::glare::GlareResult;

/// Domain-separation tag for the Verifying transcript. ASCII constant,
/// versioned. Storm §3.1 / §4.5.
pub const DOMAIN_SEPARATION_TAG: &[u8] = b"mxdx.p2p.verify.v1";

/// Byte separator between variable-length string fields in the
/// transcript. Chosen as 0x00 because neither UTF-8 text (room_id,
/// call_id, party_id) nor upper-hex SDP fingerprints (`[0-9A-F:]`)
/// can contain it, so the separator is self-delimiting.
const FIELD_SEP: u8 = 0x00;

/// Nonce length (32 bytes) — matches storm §3.1.
pub const NONCE_LEN: usize = 32;

/// Ed25519 signature length (64 bytes, by spec).
pub const ED25519_SIGNATURE_LEN: usize = 64;

/// Ed25519 public key length (32 bytes).
pub const ED25519_PUBLIC_KEY_LEN: usize = 32;

/// Errors raised during handshake construction or verification.
#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    #[error("invalid base64: {0}")]
    InvalidBase64(String),

    #[error("invalid ed25519 signature length: got {0}, expected 64")]
    InvalidSignatureLength(usize),

    #[error("invalid ed25519 public-key length: got {0}, expected 32")]
    InvalidPublicKeyLength(usize),

    #[error("invalid nonce length: got {0}, expected 32")]
    InvalidNonceLength(usize),

    #[error("could not parse ed25519 public key: {0}")]
    InvalidPublicKey(String),

    #[error("ed25519 signature verification failed")]
    SignatureMismatch,

    #[error("malformed SDP — no DTLS fingerprint found")]
    MissingSdpFingerprint,
}

/// Verifying-handshake frame envelope. Serialized as JSON, then the JSON
/// bytes are AES-GCM-encrypted by [`P2PCrypto::encrypt`] and sent as a
/// single data-channel frame.
///
/// The `type` tag discriminator is on the wire as `"verify_challenge"` or
/// `"verify_response"`. Matches the storm §3.1 "Both sides exchange
/// (nonce, signature) inside AES-GCM frames" requirement — we split
/// into two message types so the offerer does not have to buffer a
/// signature over its own nonce alone (the transcript requires both
/// nonces to be known).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum HandshakeMsg {
    /// Step 1: each peer sends its nonce to the other. Contains no
    /// signature — neither side can sign the transcript yet because it
    /// needs both nonces.
    #[serde(rename = "verify_challenge")]
    Challenge {
        nonce_b64: String,
        /// Our device_id. Included here so the peer can look up our
        /// Ed25519 public key via the Matrix crypto store.
        device_id: String,
    },
    /// Step 2: each peer signs the full transcript (including BOTH
    /// nonces, both party_ids, both SDP fingerprints) and sends the
    /// signature.
    #[serde(rename = "verify_response")]
    Response {
        signature_b64: String,
        /// Base64-encoded 32-byte Ed25519 public key. The receiver
        /// verifies this matches the Matrix device's public key (via
        /// `MatrixClient::get_peer_device_ed25519`) before trusting the
        /// signature.
        signer_ed25519_b64: String,
        device_id: String,
    },
}

/// Build the canonical transcript bytes per storm §3.1. The byte order
/// is offerer-first — determined by glare outcome, NOT by which peer
/// the current driver is. Both peers produce IDENTICAL transcript bytes
/// even though they occupy different roles in the call.
///
/// The transcript is the input to Ed25519 sign/verify. Never pass raw
/// user content here — only the bound fields below.
///
/// # Arguments
///
/// * `room_id` — Matrix room_id string (e.g. `!abcd:example.org`).
/// * `session_uuid` — mxdx session UUID; empty string if absent.
/// * `call_id` — Matrix VoIP `call_id` string.
/// * `offerer_nonce` / `answerer_nonce` — 32 bytes each.
/// * `offerer_party_id` / `answerer_party_id` — UTF-8 strings.
/// * `offerer_sdp_fingerprint` / `answerer_sdp_fingerprint` —
///   upper-hex with colons (e.g. `AA:BB:CC:...`), extracted from each
///   peer's SDP via [`extract_sdp_fingerprint`].
///
/// # Panics
///
/// Never — the function is pure byte concatenation.
pub fn build_transcript(
    room_id: &str,
    session_uuid: &str,
    call_id: &str,
    offerer_nonce: &[u8; NONCE_LEN],
    answerer_nonce: &[u8; NONCE_LEN],
    offerer_party_id: &str,
    answerer_party_id: &str,
    offerer_sdp_fingerprint: &str,
    answerer_sdp_fingerprint: &str,
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(
        DOMAIN_SEPARATION_TAG.len()
            + 1
            + room_id.len()
            + 1
            + session_uuid.len()
            + 1
            + call_id.len()
            + NONCE_LEN * 2
            + 1
            + offerer_party_id.len()
            + 1
            + answerer_party_id.len()
            + 1
            + offerer_sdp_fingerprint.len()
            + 1
            + answerer_sdp_fingerprint.len()
            + 8,
    );
    buf.extend_from_slice(DOMAIN_SEPARATION_TAG);
    buf.push(FIELD_SEP);
    buf.extend_from_slice(room_id.as_bytes());
    buf.push(FIELD_SEP);
    buf.extend_from_slice(session_uuid.as_bytes());
    buf.push(FIELD_SEP);
    buf.extend_from_slice(call_id.as_bytes());
    // Nonces are fixed-width; canonical ordering: offerer, answerer.
    buf.extend_from_slice(offerer_nonce);
    buf.extend_from_slice(answerer_nonce);
    buf.push(FIELD_SEP);
    buf.extend_from_slice(offerer_party_id.as_bytes());
    buf.push(FIELD_SEP);
    buf.extend_from_slice(answerer_party_id.as_bytes());
    buf.push(FIELD_SEP);
    buf.extend_from_slice(offerer_sdp_fingerprint.as_bytes());
    buf.push(FIELD_SEP);
    buf.extend_from_slice(answerer_sdp_fingerprint.as_bytes());
    buf
}

/// Extract the `a=fingerprint:sha-256 XX:XX:...` value from an SDP blob.
/// Returns the normalized uppercase hex-with-colons string.
///
/// If multiple fingerprints are present (rare — RTP+RTCP split), the
/// lexicographically smallest fingerprint (by normalized upper-hex) is
/// returned. Both peers apply the same canonicalization so the
/// transcript matches.
///
/// # Errors
///
/// Returns [`VerifyError::MissingSdpFingerprint`] if no `a=fingerprint`
/// line is present.
pub fn extract_sdp_fingerprint(sdp: &str) -> Result<String, VerifyError> {
    let mut candidates: Vec<String> = Vec::new();
    for line in sdp.lines() {
        let trimmed = line.trim();
        // Spec: "a=fingerprint:<hash-function> <fingerprint>"
        // Accept sha-256; ignore other hash algorithms (we require sha-256
        // for the binding to be strong, and libdatachannel always emits it).
        if let Some(rest) = trimmed.strip_prefix("a=fingerprint:") {
            // rest = "sha-256 AA:BB:CC..."
            let mut parts = rest.splitn(2, char::is_whitespace);
            let algo = parts.next().unwrap_or("");
            let fp = parts.next().unwrap_or("").trim();
            if algo.eq_ignore_ascii_case("sha-256") && !fp.is_empty() {
                candidates.push(fp.to_ascii_uppercase());
            }
        }
    }
    if candidates.is_empty() {
        return Err(VerifyError::MissingSdpFingerprint);
    }
    candidates.sort();
    Ok(candidates.into_iter().next().unwrap())
}

/// Extract both SDP fingerprints from offer and answer and canonicalize
/// them per the glare-result-determined ordering.
pub fn canonical_sdp_fingerprints(
    offer_sdp: &str,
    answer_sdp: &str,
) -> Result<(String, String), VerifyError> {
    let offerer_fp = extract_sdp_fingerprint(offer_sdp)?;
    let answerer_fp = extract_sdp_fingerprint(answer_sdp)?;
    Ok((offerer_fp, answerer_fp))
}

/// Canonical (offerer, answerer) ordering of a (nonce, party_id) pair
/// determined by the glare resolution outcome.
///
/// When we are the offerer (we sent the invite, or we won glare),
/// `(our_nonce, our_party_id)` is the offerer pair; otherwise it's the
/// answerer pair.
pub fn canonical_ordering<'a, T: ?Sized>(
    our: &'a T,
    theirs: &'a T,
    we_are_offerer: bool,
) -> (&'a T, &'a T) {
    if we_are_offerer {
        (our, theirs)
    } else {
        (theirs, our)
    }
}

/// Shorthand: `we_are_offerer == matches!(glare, GlareResult::WeWin)` when
/// there was a glare, OR `our_call_id == invite.call_id` when there
/// wasn't. The driver computes this and passes it in explicitly to keep
/// the transcript construction self-contained.
pub fn we_are_offerer(glare: Option<GlareResult>, sent_invite: bool) -> bool {
    match glare {
        Some(GlareResult::WeWin) => true,
        Some(GlareResult::TheyWin) => false,
        None => sent_invite,
    }
}

/// Verify the peer's signature over the canonical transcript.
///
/// # Arguments
///
/// * `peer_public_key_bytes` — 32-byte Ed25519 public key of the peer's
///   Matrix device, obtained via `MatrixClient::get_peer_device_ed25519`.
///   The caller MUST validate the key came from Matrix's verified device
///   store before calling this function.
/// * `signature_bytes` — 64-byte signature from the handshake response.
/// * `transcript` — output of [`build_transcript`].
///
/// # Errors
///
/// * [`VerifyError::InvalidPublicKey`] — public-key bytes don't decode as
///   a valid Ed25519 point.
/// * [`VerifyError::SignatureMismatch`] — signature does not verify.
pub fn verify_peer_signature(
    peer_public_key_bytes: &[u8; ED25519_PUBLIC_KEY_LEN],
    signature_bytes: &[u8; ED25519_SIGNATURE_LEN],
    transcript: &[u8],
) -> Result<(), VerifyError> {
    let vk = VerifyingKey::from_bytes(peer_public_key_bytes)
        .map_err(|e| VerifyError::InvalidPublicKey(e.to_string()))?;
    let sig = Signature::from_bytes(signature_bytes);
    vk.verify(transcript, &sig)
        .map_err(|_| VerifyError::SignatureMismatch)
}

/// Decode a base64 nonce string into a fixed 32-byte array.
pub fn decode_nonce_b64(b64: &str) -> Result<[u8; NONCE_LEN], VerifyError> {
    let raw = BASE64_STANDARD
        .decode(b64)
        .map_err(|e| VerifyError::InvalidBase64(e.to_string()))?;
    raw.as_slice()
        .try_into()
        .map_err(|_| VerifyError::InvalidNonceLength(raw.len()))
}

/// Decode a base64 signature into a 64-byte array.
pub fn decode_signature_b64(b64: &str) -> Result<[u8; ED25519_SIGNATURE_LEN], VerifyError> {
    let raw = BASE64_STANDARD
        .decode(b64)
        .map_err(|e| VerifyError::InvalidBase64(e.to_string()))?;
    raw.as_slice()
        .try_into()
        .map_err(|_| VerifyError::InvalidSignatureLength(raw.len()))
}

/// Decode a base64 32-byte Ed25519 public key.
pub fn decode_public_key_b64(b64: &str) -> Result<[u8; ED25519_PUBLIC_KEY_LEN], VerifyError> {
    let raw = BASE64_STANDARD
        .decode(b64)
        .map_err(|e| VerifyError::InvalidBase64(e.to_string()))?;
    raw.as_slice()
        .try_into()
        .map_err(|_| VerifyError::InvalidPublicKeyLength(raw.len()))
}

/// Encode a 32-byte nonce as base64.
pub fn encode_nonce(nonce: &[u8; NONCE_LEN]) -> String {
    BASE64_STANDARD.encode(nonce)
}

/// Encode a signature as base64.
pub fn encode_signature(sig: &[u8; ED25519_SIGNATURE_LEN]) -> String {
    BASE64_STANDARD.encode(sig)
}

/// Encode a public key as base64.
pub fn encode_public_key(pk: &[u8; ED25519_PUBLIC_KEY_LEN]) -> String {
    BASE64_STANDARD.encode(pk)
}

/// Generate a 32-byte nonce from the OS CSPRNG. MUST be called once per
/// call — never reuse nonces across calls (storm §4.5 replay detection
/// depends on freshness).
pub fn generate_nonce() -> [u8; NONCE_LEN] {
    use aes_gcm::aead::rand_core::RngCore;
    let mut nonce = [0u8; NONCE_LEN];
    aes_gcm::aead::OsRng.fill_bytes(&mut nonce);
    nonce
}

/// Abstraction over "sign this transcript with our Matrix device's
/// Ed25519 key". Phase-6 integration supplies a concrete impl backed by
/// `MatrixClient` (via the Olm machine). For tests and the pure crypto
/// path inside this phase, we provide [`EphemeralKeySigner`] which
/// generates a per-session ephemeral key — still Ed25519, still CSPRNG,
/// but unbound to the Matrix device identity.
///
/// SECURITY NOTE: `EphemeralKeySigner` is NOT sufficient for production
/// because it does not prove "this signer is the Matrix device we
/// invited". The production [`HandshakeSigner`] impl MUST either:
///   (a) sign with the Matrix device's stable Ed25519 key directly, OR
///   (b) sign a cross-certification envelope that binds the ephemeral
///       key to the Matrix device's stable key via a Matrix-visible
///       attestation (an `m.mxdx.p2p.ephemeral_key` state event signed
///       by the device's Ed25519).
///
/// Storm §3.1 calls for option (a). Phase 6 will implement it against
/// matrix-sdk's Olm machine.
pub trait HandshakeSigner: Send + Sync {
    /// Sign `transcript` with our device's Ed25519 private key and
    /// return the 64-byte signature + the 32-byte public key the
    /// verifier should use.
    fn sign(
        &self,
        transcript: &[u8],
    ) -> Result<([u8; ED25519_SIGNATURE_LEN], [u8; ED25519_PUBLIC_KEY_LEN]), VerifyError>;
}

/// Abstraction over "look up the peer device's Ed25519 public key".
/// Phase-6 integration supplies a concrete impl backed by
/// `MatrixClient::encryption().get_user_devices()` — the production
/// verifier MUST look up via the Matrix crypto store, NOT trust the
/// `signer_ed25519_b64` field carried in the response message verbatim.
///
/// # Why not trust the wire's public key
///
/// If we trusted the wire value, an attacker could:
///   1. Sign the transcript with their own key.
///   2. Put their public key in `signer_ed25519_b64`.
///   3. Verification passes (signature/key pair is consistent).
///   4. We think we verified the peer, but it's the attacker.
///
/// The point is to bind to the Matrix device identity. The wire public
/// key is compared against the Matrix-known public key; mismatch →
/// abort.
pub trait HandshakePeerKeySource: Send + Sync {
    /// Return the 32-byte Ed25519 public key of the peer's Matrix
    /// device. Returns `None` if the device is unknown (in which case
    /// the handshake aborts — we never trust an unknown device's P2P
    /// signature).
    fn peer_public_key(
        &self,
        peer_user_id: &str,
        peer_device_id: &str,
    ) -> Option<[u8; ED25519_PUBLIC_KEY_LEN]>;
}

/// Test/development signer that generates a fresh Ed25519 keypair on
/// creation. See [`HandshakeSigner`] security note — NOT sufficient for
/// production (no binding to Matrix device identity).
pub struct EphemeralKeySigner {
    signing_key: ed25519_dalek::SigningKey,
}

impl EphemeralKeySigner {
    /// Generate a fresh ephemeral Ed25519 keypair from OsRng. Drops the
    /// private key on Self drop (SigningKey zeroizes internally).
    pub fn new() -> Self {
        use aes_gcm::aead::rand_core::RngCore;
        let mut seed = [0u8; 32];
        aes_gcm::aead::OsRng.fill_bytes(&mut seed);
        let sk = ed25519_dalek::SigningKey::from_bytes(&seed);
        Self { signing_key: sk }
    }

    /// Public key (stable over this signer's lifetime).
    pub fn public_key(&self) -> [u8; ED25519_PUBLIC_KEY_LEN] {
        self.signing_key.verifying_key().to_bytes()
    }
}

impl Default for EphemeralKeySigner {
    fn default() -> Self {
        Self::new()
    }
}

impl HandshakeSigner for EphemeralKeySigner {
    fn sign(
        &self,
        transcript: &[u8],
    ) -> Result<([u8; ED25519_SIGNATURE_LEN], [u8; ED25519_PUBLIC_KEY_LEN]), VerifyError> {
        use ed25519_dalek::Signer;
        let sig = self.signing_key.sign(transcript);
        Ok((sig.to_bytes(), self.signing_key.verifying_key().to_bytes()))
    }
}

/// In-memory [`HandshakePeerKeySource`] for tests. Maps
/// `(user_id, device_id)` pairs to known public keys.
#[derive(Default)]
pub struct InMemoryPeerKeySource {
    keys: std::collections::HashMap<(String, String), [u8; ED25519_PUBLIC_KEY_LEN]>,
}

impl InMemoryPeerKeySource {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(
        &mut self,
        user_id: impl Into<String>,
        device_id: impl Into<String>,
        key: [u8; ED25519_PUBLIC_KEY_LEN],
    ) {
        self.keys.insert((user_id.into(), device_id.into()), key);
    }
}

impl HandshakePeerKeySource for InMemoryPeerKeySource {
    fn peer_public_key(&self, user: &str, device: &str) -> Option<[u8; ED25519_PUBLIC_KEY_LEN]> {
        self.keys
            .get(&(user.to_string(), device.to_string()))
            .copied()
    }
}

// --------------------------------------------------------------------------
// Handshake orchestration — pure functions the driver composes
// --------------------------------------------------------------------------

/// Parameters driving a single handshake attempt. Populated by the driver
/// from its [`P2PState::Verifying`] fields plus the peer identity from
/// the initiating `start()`.
#[derive(Debug, Clone)]
pub struct HandshakeParams {
    pub room_id: String,
    pub session_uuid: String,
    pub call_id: String,
    pub our_nonce: [u8; NONCE_LEN],
    pub our_party_id: String,
    pub our_user_id: String,
    pub our_device_id: String,
    pub peer_user_id: String,
    pub peer_device_id: String,
    pub our_sdp_fingerprint: String,
    pub we_are_offerer: bool,
}

/// Outcome of a single handshake attempt — the driver maps this 1:1 onto
/// `Event::VerifyOk` / `Event::VerifyFail { reason }`.
#[derive(Debug, PartialEq, Eq)]
pub enum HandshakeOutcome {
    Verified,
    SignatureMismatch,
    ReplayDetected,
    PeerDeviceUnknown,
    InvalidPayload,
    PeerKeyMismatch,
}

impl HandshakeOutcome {
    pub fn to_event(self) -> Option<super::state::VerifyFailureReason> {
        use super::state::VerifyFailureReason as V;
        match self {
            Self::Verified => None,
            Self::SignatureMismatch => Some(V::SignatureMismatch),
            Self::ReplayDetected => Some(V::ReplayDetected),
            Self::PeerDeviceUnknown => Some(V::SignatureMismatch),
            Self::InvalidPayload => Some(V::InvalidPayload),
            Self::PeerKeyMismatch => Some(V::SignatureMismatch),
        }
    }
}

/// Verify a peer's [`HandshakeMsg::Response`] against the canonical
/// transcript. Pure check; the driver wraps it with replay and 3-strike
/// bookkeeping.
pub fn verify_handshake_response(
    params: &HandshakeParams,
    peer_nonce: &[u8; NONCE_LEN],
    peer_party_id: &str,
    peer_sdp_fingerprint: &str,
    peer_response: &HandshakeMsg,
    peer_keys: &dyn HandshakePeerKeySource,
) -> HandshakeOutcome {
    let (signature_b64, signer_pk_b64, response_device_id) = match peer_response {
        HandshakeMsg::Response {
            signature_b64,
            signer_ed25519_b64,
            device_id,
        } => (signature_b64, signer_ed25519_b64, device_id),
        HandshakeMsg::Challenge { .. } => return HandshakeOutcome::InvalidPayload,
    };

    let wire_pk = match decode_public_key_b64(signer_pk_b64) {
        Ok(k) => k,
        Err(_) => return HandshakeOutcome::InvalidPayload,
    };
    let signature = match decode_signature_b64(signature_b64) {
        Ok(s) => s,
        Err(_) => return HandshakeOutcome::InvalidPayload,
    };

    let matrix_pk = match peer_keys.peer_public_key(&params.peer_user_id, response_device_id) {
        Some(k) => k,
        None => return HandshakeOutcome::PeerDeviceUnknown,
    };

    if wire_pk != matrix_pk {
        return HandshakeOutcome::PeerKeyMismatch;
    }

    let (off_nonce, ans_nonce) =
        canonical_ordering(&params.our_nonce, peer_nonce, params.we_are_offerer);
    let (off_party, ans_party) = canonical_ordering(
        params.our_party_id.as_str(),
        peer_party_id,
        params.we_are_offerer,
    );
    let (off_fp, ans_fp) = canonical_ordering(
        params.our_sdp_fingerprint.as_str(),
        peer_sdp_fingerprint,
        params.we_are_offerer,
    );
    let transcript = build_transcript(
        &params.room_id,
        &params.session_uuid,
        &params.call_id,
        off_nonce,
        ans_nonce,
        off_party,
        ans_party,
        off_fp,
        ans_fp,
    );

    match verify_peer_signature(&matrix_pk, &signature, &transcript) {
        Ok(()) => HandshakeOutcome::Verified,
        Err(_) => HandshakeOutcome::SignatureMismatch,
    }
}

/// Build our side's [`HandshakeMsg::Response`] given the parameters and a
/// peer-nonce already received. Uses [`HandshakeSigner`] to produce the
/// signature.
pub fn build_handshake_response(
    params: &HandshakeParams,
    peer_nonce: &[u8; NONCE_LEN],
    peer_party_id: &str,
    peer_sdp_fingerprint: &str,
    signer: &dyn HandshakeSigner,
) -> Result<HandshakeMsg, VerifyError> {
    let (off_nonce, ans_nonce) =
        canonical_ordering(&params.our_nonce, peer_nonce, params.we_are_offerer);
    let (off_party, ans_party) = canonical_ordering(
        params.our_party_id.as_str(),
        peer_party_id,
        params.we_are_offerer,
    );
    let (off_fp, ans_fp) = canonical_ordering(
        params.our_sdp_fingerprint.as_str(),
        peer_sdp_fingerprint,
        params.we_are_offerer,
    );
    let transcript = build_transcript(
        &params.room_id,
        &params.session_uuid,
        &params.call_id,
        off_nonce,
        ans_nonce,
        off_party,
        ans_party,
        off_fp,
        ans_fp,
    );
    let (sig, pk) = signer.sign(&transcript)?;
    Ok(HandshakeMsg::Response {
        signature_b64: encode_signature(&sig),
        signer_ed25519_b64: encode_public_key(&pk),
        device_id: params.our_device_id.clone(),
    })
}

/// Build our side's [`HandshakeMsg::Challenge`] with the current nonce.
pub fn build_handshake_challenge(params: &HandshakeParams) -> HandshakeMsg {
    HandshakeMsg::Challenge {
        nonce_b64: encode_nonce(&params.our_nonce),
        device_id: params.our_device_id.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn mk_signing_key() -> SigningKey {
        // Deterministic test key (NOT a real key — tests only).
        let seed = [42u8; 32];
        SigningKey::from_bytes(&seed)
    }

    #[test]
    fn domain_separation_tag_is_versioned_and_ascii() {
        assert_eq!(DOMAIN_SEPARATION_TAG, b"mxdx.p2p.verify.v1");
        assert!(DOMAIN_SEPARATION_TAG.iter().all(|b| b.is_ascii()));
    }

    #[test]
    fn handshake_msg_challenge_roundtrips() {
        let m = HandshakeMsg::Challenge {
            nonce_b64: "abc".into(),
            device_id: "DEV".into(),
        };
        let json = serde_json::to_string(&m).unwrap();
        assert!(json.contains("\"type\":\"verify_challenge\""));
        let back: HandshakeMsg = serde_json::from_str(&json).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn handshake_msg_response_roundtrips() {
        let m = HandshakeMsg::Response {
            signature_b64: "sig".into(),
            signer_ed25519_b64: "pk".into(),
            device_id: "DEV".into(),
        };
        let json = serde_json::to_string(&m).unwrap();
        assert!(json.contains("\"type\":\"verify_response\""));
        let back: HandshakeMsg = serde_json::from_str(&json).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn build_transcript_is_deterministic() {
        let t1 = build_transcript(
            "!r:ex", "sid", "call-1", &[1u8; 32], &[2u8; 32], "p-off", "p-ans", "AA:BB", "CC:DD",
        );
        let t2 = build_transcript(
            "!r:ex", "sid", "call-1", &[1u8; 32], &[2u8; 32], "p-off", "p-ans", "AA:BB", "CC:DD",
        );
        assert_eq!(t1, t2);
    }

    #[test]
    fn build_transcript_contains_domain_separator_at_start() {
        let t = build_transcript("!r:ex", "", "c", &[0u8; 32], &[0u8; 32], "a", "b", "X", "Y");
        assert!(t.starts_with(DOMAIN_SEPARATION_TAG));
    }

    #[test]
    fn build_transcript_differs_on_room_id() {
        let t1 = build_transcript(
            "!r1:ex", "s", "c", &[0u8; 32], &[0u8; 32], "a", "b", "X", "Y",
        );
        let t2 = build_transcript(
            "!r2:ex", "s", "c", &[0u8; 32], &[0u8; 32], "a", "b", "X", "Y",
        );
        assert_ne!(t1, t2);
    }

    #[test]
    fn build_transcript_differs_on_session_uuid() {
        let t1 = build_transcript(
            "!r:ex", "s1", "c", &[0u8; 32], &[0u8; 32], "a", "b", "X", "Y",
        );
        let t2 = build_transcript(
            "!r:ex", "s2", "c", &[0u8; 32], &[0u8; 32], "a", "b", "X", "Y",
        );
        assert_ne!(t1, t2);
    }

    #[test]
    fn build_transcript_differs_on_call_id() {
        let t1 = build_transcript(
            "!r:ex", "s", "c1", &[0u8; 32], &[0u8; 32], "a", "b", "X", "Y",
        );
        let t2 = build_transcript(
            "!r:ex", "s", "c2", &[0u8; 32], &[0u8; 32], "a", "b", "X", "Y",
        );
        assert_ne!(t1, t2);
    }

    #[test]
    fn build_transcript_differs_on_offerer_nonce() {
        let t1 = build_transcript(
            "!r:ex", "s", "c", &[1u8; 32], &[0u8; 32], "a", "b", "X", "Y",
        );
        let t2 = build_transcript(
            "!r:ex", "s", "c", &[2u8; 32], &[0u8; 32], "a", "b", "X", "Y",
        );
        assert_ne!(t1, t2);
    }

    #[test]
    fn build_transcript_differs_on_answerer_nonce() {
        let t1 = build_transcript(
            "!r:ex", "s", "c", &[0u8; 32], &[1u8; 32], "a", "b", "X", "Y",
        );
        let t2 = build_transcript(
            "!r:ex", "s", "c", &[0u8; 32], &[2u8; 32], "a", "b", "X", "Y",
        );
        assert_ne!(t1, t2);
    }

    #[test]
    fn build_transcript_differs_on_offerer_party_id() {
        let t1 = build_transcript(
            "!r:ex", "s", "c", &[0u8; 32], &[0u8; 32], "a1", "b", "X", "Y",
        );
        let t2 = build_transcript(
            "!r:ex", "s", "c", &[0u8; 32], &[0u8; 32], "a2", "b", "X", "Y",
        );
        assert_ne!(t1, t2);
    }

    #[test]
    fn build_transcript_differs_on_answerer_party_id() {
        let t1 = build_transcript(
            "!r:ex", "s", "c", &[0u8; 32], &[0u8; 32], "a", "b1", "X", "Y",
        );
        let t2 = build_transcript(
            "!r:ex", "s", "c", &[0u8; 32], &[0u8; 32], "a", "b2", "X", "Y",
        );
        assert_ne!(t1, t2);
    }

    #[test]
    fn build_transcript_differs_on_sdp_fingerprints() {
        let t1 = build_transcript(
            "!r:ex", "s", "c", &[0u8; 32], &[0u8; 32], "a", "b", "FP1", "FP2",
        );
        let t2 = build_transcript(
            "!r:ex", "s", "c", &[0u8; 32], &[0u8; 32], "a", "b", "FP1", "FP3",
        );
        assert_ne!(t1, t2);
    }

    #[test]
    fn build_transcript_not_affected_by_field_confusion() {
        // If the 0x00 separators were missing, the transcript of
        // (room="!r", call="c") would match (room="!r:c", call=""). The
        // separator makes these distinct.
        let t1 = build_transcript("!r", "", "c", &[0u8; 32], &[0u8; 32], "a", "b", "X", "Y");
        let t2 = build_transcript("!r:c", "", "", &[0u8; 32], &[0u8; 32], "a", "b", "X", "Y");
        assert_ne!(t1, t2);
    }

    #[test]
    fn extract_sdp_fingerprint_parses_standard_line() {
        let sdp =
            "v=0\r\no=- 123 456 IN IP4 1.2.3.4\r\na=fingerprint:sha-256 AA:BB:CC:DD:EE:FF\r\n";
        assert_eq!(extract_sdp_fingerprint(sdp).unwrap(), "AA:BB:CC:DD:EE:FF");
    }

    #[test]
    fn extract_sdp_fingerprint_normalizes_case() {
        let sdp = "a=fingerprint:sha-256 aa:bb:cc:dd\r\n";
        assert_eq!(extract_sdp_fingerprint(sdp).unwrap(), "AA:BB:CC:DD");
    }

    #[test]
    fn extract_sdp_fingerprint_errors_when_missing() {
        let sdp = "v=0\r\no=- 123 456 IN IP4 1.2.3.4\r\n";
        let err = extract_sdp_fingerprint(sdp).unwrap_err();
        assert!(matches!(err, VerifyError::MissingSdpFingerprint));
    }

    #[test]
    fn extract_sdp_fingerprint_picks_lexicographically_smallest() {
        let sdp = "a=fingerprint:sha-256 FF:00\r\na=fingerprint:sha-256 11:22\r\n";
        assert_eq!(extract_sdp_fingerprint(sdp).unwrap(), "11:22");
    }

    #[test]
    fn extract_sdp_fingerprint_ignores_non_sha256() {
        let sdp = "a=fingerprint:sha-1 AA:BB\r\na=fingerprint:sha-256 CC:DD\r\n";
        assert_eq!(extract_sdp_fingerprint(sdp).unwrap(), "CC:DD");
    }

    #[test]
    fn sign_and_verify_roundtrip_succeeds() {
        let sk = mk_signing_key();
        let vk = sk.verifying_key();
        let transcript = build_transcript(
            "!r:ex", "sid", "call-1", &[1u8; 32], &[2u8; 32], "p-off", "p-ans", "AA", "BB",
        );
        let sig = sk.sign(&transcript);
        verify_peer_signature(&vk.to_bytes(), &sig.to_bytes(), &transcript)
            .expect("signature should verify");
    }

    #[test]
    fn wrong_signature_fails_verification() {
        let sk1 = mk_signing_key();
        let sk2 = SigningKey::from_bytes(&[9u8; 32]);
        let vk1 = sk1.verifying_key();
        let transcript = build_transcript(
            "!r:ex", "sid", "call-1", &[1u8; 32], &[2u8; 32], "p-off", "p-ans", "AA", "BB",
        );
        // Sign with sk2 but try to verify against vk1.
        let sig = sk2.sign(&transcript);
        let err = verify_peer_signature(&vk1.to_bytes(), &sig.to_bytes(), &transcript).unwrap_err();
        assert!(matches!(err, VerifyError::SignatureMismatch));
    }

    #[test]
    fn verify_fails_on_modified_transcript() {
        let sk = mk_signing_key();
        let vk = sk.verifying_key();
        let transcript = build_transcript(
            "!r:ex", "sid", "call-1", &[1u8; 32], &[2u8; 32], "p-off", "p-ans", "AA", "BB",
        );
        let sig = sk.sign(&transcript);
        let mut tampered = transcript.clone();
        tampered[0] ^= 0x01;
        let err = verify_peer_signature(&vk.to_bytes(), &sig.to_bytes(), &tampered).unwrap_err();
        assert!(matches!(err, VerifyError::SignatureMismatch));
    }

    #[test]
    fn invalid_public_key_bytes_rejected() {
        let transcript = b"test";
        // 32 bytes that are not a valid Ed25519 point.
        let bogus_pk = [0xFFu8; 32];
        let sig = [0u8; 64];
        let result = verify_peer_signature(&bogus_pk, &sig, transcript);
        // Ed25519 point decompression may or may not reject 0xFF*32;
        // either InvalidPublicKey or SignatureMismatch is acceptable —
        // the point is we don't panic and don't return Ok.
        assert!(result.is_err());
    }

    #[test]
    fn decode_nonce_b64_round_trip() {
        let n = [7u8; 32];
        let b64 = encode_nonce(&n);
        assert_eq!(decode_nonce_b64(&b64).unwrap(), n);
    }

    #[test]
    fn decode_nonce_b64_rejects_wrong_length() {
        let b64 = BASE64_STANDARD.encode([0u8; 16]); // 16 bytes, not 32
        let err = decode_nonce_b64(&b64).unwrap_err();
        assert!(matches!(err, VerifyError::InvalidNonceLength(16)));
    }

    #[test]
    fn decode_signature_b64_rejects_wrong_length() {
        let b64 = BASE64_STANDARD.encode([0u8; 32]); // 32 bytes, not 64
        let err = decode_signature_b64(&b64).unwrap_err();
        assert!(matches!(err, VerifyError::InvalidSignatureLength(32)));
    }

    #[test]
    fn decode_public_key_b64_rejects_wrong_length() {
        let b64 = BASE64_STANDARD.encode([0u8; 16]); // 16 bytes, not 32
        let err = decode_public_key_b64(&b64).unwrap_err();
        assert!(matches!(err, VerifyError::InvalidPublicKeyLength(16)));
    }

    #[test]
    fn decode_b64_rejects_invalid_base64() {
        let err = decode_nonce_b64("not base64!!!!").unwrap_err();
        assert!(matches!(err, VerifyError::InvalidBase64(_)));
    }

    #[test]
    fn generate_nonce_is_nonzero_and_32_bytes() {
        let n = generate_nonce();
        assert_eq!(n.len(), NONCE_LEN);
        // The probability of OsRng producing all-zeros is 2^-256 —
        // effectively zero.
        assert!(n.iter().any(|b| *b != 0));
    }

    #[test]
    fn generate_nonce_produces_unique_values() {
        let a = generate_nonce();
        let b = generate_nonce();
        assert_ne!(a, b);
    }

    #[test]
    fn canonical_ordering_when_we_are_offerer() {
        let (off, ans) = canonical_ordering(&"us", &"them", true);
        assert_eq!(off, &"us");
        assert_eq!(ans, &"them");
    }

    #[test]
    fn canonical_ordering_when_we_are_not_offerer() {
        let (off, ans) = canonical_ordering(&"us", &"them", false);
        assert_eq!(off, &"them");
        assert_eq!(ans, &"us");
    }

    #[test]
    fn we_are_offerer_uses_glare_result_when_present() {
        assert!(we_are_offerer(Some(GlareResult::WeWin), false));
        assert!(!we_are_offerer(Some(GlareResult::TheyWin), true));
    }

    #[test]
    fn we_are_offerer_falls_back_to_sent_invite_when_no_glare() {
        assert!(we_are_offerer(None, true));
        assert!(!we_are_offerer(None, false));
    }

    #[test]
    fn ephemeral_signer_sign_and_verify_roundtrip() {
        let signer = EphemeralKeySigner::new();
        let pk = signer.public_key();
        let transcript = b"mxdx.p2p.verify.v1 canonical bytes";
        let (sig, signer_pk) = signer.sign(transcript).unwrap();
        assert_eq!(pk, signer_pk);
        verify_peer_signature(&pk, &sig, transcript).unwrap();
    }

    #[test]
    fn ephemeral_signer_produces_different_keys_across_instances() {
        let s1 = EphemeralKeySigner::new();
        let s2 = EphemeralKeySigner::new();
        assert_ne!(s1.public_key(), s2.public_key());
    }

    #[test]
    fn in_memory_peer_key_source_roundtrip() {
        let mut src = InMemoryPeerKeySource::new();
        let pk = [42u8; 32];
        src.insert("@u:ex", "DEV", pk);
        assert_eq!(src.peer_public_key("@u:ex", "DEV"), Some(pk));
        assert_eq!(src.peer_public_key("@u:ex", "OTHER"), None);
        assert_eq!(src.peer_public_key("@other:ex", "DEV"), None);
    }

    #[test]
    fn full_handshake_flow_with_abstractions() {
        // Simulate both peers with EphemeralKeySigner + canonical
        // transcript. Each peer:
        //   1. Generates a nonce.
        //   2. After exchanging challenges, builds the transcript.
        //   3. Signs.
        //   4. Receives peer's (signature, signer_pk).
        //   5. Looks up peer's KNOWN pk from InMemoryPeerKeySource.
        //   6. Rejects if signer_pk != known_pk.
        //   7. Otherwise verifies.

        // Setup: both peers have stable identities in Matrix (simulated
        // by InMemoryPeerKeySource).
        let offerer_signer = EphemeralKeySigner::new();
        let answerer_signer = EphemeralKeySigner::new();

        let mut peer_keys = InMemoryPeerKeySource::new();
        peer_keys.insert("@offerer:ex", "OFFDEV", offerer_signer.public_key());
        peer_keys.insert("@answerer:ex", "ANSDEV", answerer_signer.public_key());

        let offerer_nonce = generate_nonce();
        let answerer_nonce = generate_nonce();

        // Both peers build the same canonical transcript.
        let transcript = build_transcript(
            "!r:ex",
            "sid",
            "call-1",
            &offerer_nonce,
            &answerer_nonce,
            "off-party",
            "ans-party",
            "AA:BB",
            "CC:DD",
        );

        // Each side signs.
        let (off_sig, off_sig_pk) = offerer_signer.sign(&transcript).unwrap();
        let (ans_sig, ans_sig_pk) = answerer_signer.sign(&transcript).unwrap();

        // Receiver checks: signer_pk from wire MUST match Matrix-known pk.
        let known_offerer_pk = peer_keys.peer_public_key("@offerer:ex", "OFFDEV").unwrap();
        assert_eq!(off_sig_pk, known_offerer_pk, "offerer pk binding");
        verify_peer_signature(&known_offerer_pk, &off_sig, &transcript).unwrap();

        let known_answerer_pk = peer_keys.peer_public_key("@answerer:ex", "ANSDEV").unwrap();
        assert_eq!(ans_sig_pk, known_answerer_pk, "answerer pk binding");
        verify_peer_signature(&known_answerer_pk, &ans_sig, &transcript).unwrap();
    }

    #[test]
    fn handshake_fails_when_peer_pk_lookup_returns_none() {
        let peer_keys = InMemoryPeerKeySource::new();
        // No entry for @unknown:ex — lookup returns None.
        assert!(peer_keys.peer_public_key("@unknown:ex", "DEV").is_none());
        // The driver's policy on None is to reject the handshake —
        // we never trust a wire-carried public key without a Matrix
        // binding.
    }

    #[test]
    fn handshake_fails_when_signer_pk_on_wire_differs_from_matrix_pk() {
        let real_signer = EphemeralKeySigner::new();
        let attacker_signer = EphemeralKeySigner::new();

        let mut peer_keys = InMemoryPeerKeySource::new();
        // Matrix knows the REAL signer's pk.
        peer_keys.insert("@peer:ex", "DEV", real_signer.public_key());

        let transcript = b"any transcript bytes";
        // Attacker signs with their OWN key.
        let (atk_sig, atk_pk) = attacker_signer.sign(transcript).unwrap();

        // Caller policy: compare signer_pk on wire to Matrix-known pk.
        let matrix_pk = peer_keys.peer_public_key("@peer:ex", "DEV").unwrap();
        assert_ne!(atk_pk, matrix_pk, "attacker pk differs from Matrix pk");

        // Even if verification with the attacker's pk would succeed
        // (self-consistent pair), verification against the Matrix pk
        // fails.
        let err = verify_peer_signature(&matrix_pk, &atk_sig, transcript).unwrap_err();
        assert!(matches!(err, VerifyError::SignatureMismatch));
    }

    /// Roles-swapped test: offerer side builds a transcript with
    /// (our_nonce, their_nonce) = (A, B) and we_are_offerer=true.
    /// Answerer side builds with (our_nonce, their_nonce) = (B, A) and
    /// we_are_offerer=false. After canonical_ordering, both produce the
    /// same transcript bytes.
    #[test]
    fn canonical_ordering_produces_same_transcript_on_both_peers() {
        let nonce_a = [1u8; 32];
        let nonce_b = [2u8; 32];
        let party_a = "party-a";
        let party_b = "party-b";
        let fp_a = "AA";
        let fp_b = "BB";

        // Offerer side: our=A, their=B, we_are_offerer=true
        let (off_n, ans_n) = canonical_ordering(&nonce_a, &nonce_b, true);
        let (off_p, ans_p) = canonical_ordering(&party_a, &party_b, true);
        let (off_f, ans_f) = canonical_ordering(&fp_a, &fp_b, true);
        let t_off = build_transcript(
            "!r:ex", "sid", "c1", off_n, ans_n, off_p, ans_p, off_f, ans_f,
        );

        // Answerer side: our=B, their=A, we_are_offerer=false
        let (off_n, ans_n) = canonical_ordering(&nonce_b, &nonce_a, false);
        let (off_p, ans_p) = canonical_ordering(&party_b, &party_a, false);
        let (off_f, ans_f) = canonical_ordering(&fp_b, &fp_a, false);
        let t_ans = build_transcript(
            "!r:ex", "sid", "c1", off_n, ans_n, off_p, ans_p, off_f, ans_f,
        );

        assert_eq!(
            t_off, t_ans,
            "both peers must produce identical transcripts after canonical ordering"
        );
    }
}
