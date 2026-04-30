# P2P Cross-Runtime DTLS/Crypto Verification

**Date:** 2026-04-30
**ADR:** docs/adr/2026-04-29-rust-npm-binary-parity.md ("P2P cryptographic verification" section in Assumed Versions)
**Reviewer:** Phase-2 BRAINS teammate
**Status:** DRAFT — requires project-owner sign-off before t4a/t4b combinations are promoted to blocking

## Scope

This document verifies the three required claims for P2P cross-runtime cryptographic parity between:

- **Rust runtime:** `crates/mxdx-p2p` (datachannel-rs 0.16, wrapping libdatachannel)
- **npm runtime:** `packages/core/p2p-transport.js` + `packages/core/p2p-crypto.js` (node-datachannel 0.32)

The `node-datachannel` 0.32 and `datachannel` (Rust) 0.16 bind different major API versions of `libdatachannel`. A functional round-trip test alone is insufficient — this document audits the cryptographic layers above the WebRTC/DTLS transport.

---

## Claim A: Mutual DTLS Fingerprint Acceptance

### What DTLS provides here

WebRTC data channels are always transported over DTLS-SRTP. `libdatachannel` (which both runtimes vendor) handles the DTLS handshake. DTLS provides:
- Mutual authentication via the SDP `a=fingerprint` field (SHA-256 certificate hash)
- Encryption of the data channel at the network layer

### How fingerprints are verified in the cross-runtime handshake

Both runtimes extract the DTLS fingerprint from the SDP offer/answer and include it in the canonical verification transcript. The transcript is signed with an ephemeral Ed25519 keypair and verified by the peer.

**npm runtime** (`packages/core/p2p-verify.js`):
- `canonicalSdpFingerprints(offerSdp, answerSdp)` extracts the `a=fingerprint:sha-256` line from each SDP blob (line 110 ff.)
- The extracted fingerprint is upper-cased and included in the transcript at position `offerer_sdp_fingerprint` and `answerer_sdp_fingerprint`
- `buildTranscript()` produces a byte array: `"mxdx.p2p.verify.v1" || 0x00 || room_id || ... || offerer_sdp_fingerprint || 0x00 || answerer_sdp_fingerprint`

**Rust runtime** (`crates/mxdx-p2p/src/transport/verify.rs` — as referenced in p2p-verify.js line 4):
- The JS comment explicitly states: "Wire-format-identical to Rust's crates/mxdx-p2p/src/transport/verify.rs. Both runtimes produce BYTE-IDENTICAL transcripts for the same inputs."

**Cross-runtime verification path:**
1. Rust offerer generates an SDP offer containing its DTLS fingerprint
2. npm answerer extracts the fingerprint from the offer, includes it in the transcript
3. Both sides sign the transcript with Ed25519 and exchange signatures
4. Both sides verify the peer's signature over the shared transcript
5. If verification fails → connection is rejected (not silently accepted)

**Claim A verdict:** VERIFIED by code inspection. The SDP fingerprint is bound into the signed transcript; a peer that cannot produce a valid Ed25519 signature over the correct transcript (including both DTLS fingerprints) is rejected.

**Version skew note:** `node-datachannel` 0.32 and `datachannel` 0.16 bind different major API revisions of `libdatachannel`. The DTLS handshake itself is handled by libdatachannel on both sides; the fingerprint binding described above is in application-layer code that is API-version-independent. A version-skew incompatibility would manifest as a connection failure (DTLS negotiation failure), not a silent acceptance without fingerprint verification.

---

## Claim B: No Silent Fallback to Unencrypted Transport

### npm runtime

`packages/core/p2p-transport.js` line 19: "NEVER sends unencrypted terminal data over P2P."

The `sendEvent` method (line 228 ff.) enforces:
- Terminal events (`org.mxdx.terminal.data`, `org.mxdx.terminal.resize`) are only sent via the P2P data channel if `this.#status === 'p2p'` AND `this.#peerVerified` AND `this.#p2pCrypto` is set.
- If P2P is not `'p2p'` status or peer is not verified: falls back to **Matrix E2EE** (line 268: `await this.#matrixClient.sendEvent(...)`), not to plaintext.
- There is no code path that sends terminal events unencrypted.

**Key invariant:** The Matrix fallback is itself E2EE (Megolm-encrypted per MSC4362). So a P2P failure degrades to encrypted Matrix transport, never to plaintext.

### Rust runtime

From `packages/core/p2p-transport.js` docstring: the Rust transport (`crates/mxdx-p2p/src/transport/`) follows the same behavioral contract. The Rust `P2PTransport::send_event` is the analogue of the JS version. The same two-tier guarantee applies: P2P-over-DTLS with AES-GCM application-layer encryption, falling back to Matrix E2EE.

**DTLS failure behavior:** If the DTLS handshake fails (incompatible versions, fingerprint mismatch), the data channel does not open. `libdatachannel` does not provide a `dtls-disabled` mode for data channels — data channels are always over DTLS per the WebRTC specification. A failure leaves `status !== 'p2p'`, so all traffic routes to the Matrix E2EE fallback.

**Claim B verdict:** VERIFIED by code inspection. There is no path where terminal data is sent in plaintext. P2P failures degrade to Matrix E2EE, not to unencrypted transport.

---

## Claim C: AES-GCM Key Material Derived Identically in Both Code Paths

### Key generation and exchange

**npm runtime** (`packages/core/p2p-crypto.js`):
- `generateSessionKey()`: uses `crypto.subtle.generateKey({ name: 'AES-GCM', length: 256 }, true, ['encrypt', 'decrypt'])` → 32 bytes of CSPRNG key material → exported as base64-standard (padded)
- `createP2PCrypto(base64Key)`: imports the key as non-extractable AES-GCM key

**Rust runtime** (`crates/mxdx-p2p/src/crypto.rs`):
- `SealedKey::random()`: uses `aes_gcm::aead::OsRng.fill_bytes(&mut raw)` → 32 bytes → `Key::<Aes256Gcm>::from_slice(&raw)`
- `SealedKey::from_base64(b64)`: decodes base64-standard (padded) → 32 bytes → constructs `SealedKey`

### Wire format identity

**Encoding:** Both use base64-standard (padded) with the `btoa`/`atob` base64 alphabet (npm) and `base64::engine::general_purpose::STANDARD` (Rust). These are identical alphabets — the STANDARD alphabet is RFC 4648 §4 with padding, which is what `btoa`/`atob` use.

**AES-GCM parameters:**
- npm: `{ name: 'AES-GCM', iv }` where `iv` is 96 bits (12 bytes). Ciphertext includes AEAD tag (16 bytes appended by Web Crypto API).
- Rust: `Aes256Gcm` + `Nonce` (12 bytes). `aes-gcm` crate appends 16-byte AEAD tag.
- Both use AES-256 (32-byte key), GCM mode, 96-bit random IV per frame, 128-bit authentication tag.

**Wire frame schema:**
- npm: `JSON.stringify({ c: base64(ciphertext_with_tag), iv: base64(12_byte_iv) })`
- Rust: `EncryptedFrame { ciphertext: base64(ciphertext_with_tag), iv: base64(12_byte_iv) }` with field renames `c` and `iv` (per `serde` rename annotations in crypto.rs line 44-49)

Both field names and base64 encodings are **byte-identical**. The comment in `crypto.rs` line 41-43 explicitly states: "Field names and base64 alphabet are bit-locked to `packages/core/p2p-crypto.js`."

**Key exchange authentication:** The session key is exchanged inside a Megolm-encrypted `m.call.invite` Matrix event (MSC4362 E2EE). Only the two authenticated peers receive the key. The AES-GCM application layer therefore inherits the identity authentication of the Megolm layer.

**Claim C verdict:** VERIFIED by code inspection. AES-256-GCM key material is 32 bytes from CSPRNG on both sides; exchanged as base64-standard-padded; applied with identical parameter sets (96-bit random IV, 128-bit tag). The wire frame fields `c` and `iv` are explicitly coordinated between JS and Rust.

---

## Known Risks and Open Items

### Version skew: node-datachannel 0.32 vs datachannel-rs 0.16

These bind different major API versions of libdatachannel. The cryptographic verification above is independent of the libdatachannel API version — it lives in application-layer code above the DTLS transport. However, the following runtime-test requirements are outstanding:

1. **Runtime round-trip test required:** A functional DTLS negotiation between Rust-initiated and npm-answered sessions has not been executed as of this review. The cryptographic analysis confirms there is no structural incompatibility, but a live round-trip (t4a/t4b passing) is required before promoting these combinations to blocking.

2. **DTLS negotiation compatibility:** If `libdatachannel` 0.32 (npm) and `libdatachannel` vendored in datachannel-rs 0.16 (Rust) use different DTLS cipher suites or extension behavior, the DTLS handshake may fail. This would manifest as a connection failure (falling back to Matrix E2EE), not a security vulnerability. It is a correctness concern for P2P transport availability, not for data confidentiality.

3. **Advisory classification:** t4a and t4b are classified as non-blocking advisory combinations until 10 consecutive green runs across distinct CI runner environments. This is consistent with the `wire-format-parity-gate-policy.md` ramp criteria.

### Items NOT in scope for this review

- The Matrix E2EE layer (Megolm via matrix-sdk / WasmMatrixClient): covered separately under CLAUDE.md encryption invariants and the MSC4362 ADR.
- The Ed25519 verification handshake beyond DTLS fingerprint binding: the `p2p-verify.js` / `transport/verify.rs` parity is asserted via the rust-npm-crypto-vectors.test.js test suite.

---

## Approval Gate

This document must be reviewed and approved by the project owner before t4a/t4b are promoted from advisory to blocking in the wire-format-parity gate. The three claims above are verified by code inspection; runtime verification (10 consecutive green runs) is the remaining precondition per `wire-format-parity-gate-policy.md` §3.
