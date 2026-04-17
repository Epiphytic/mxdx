/**
 * @mxdx/core — Ed25519-signed Verifying handshake for P2P transport.
 *
 * Wire-format-identical to Rust's crates/mxdx-p2p/src/transport/verify.rs.
 * Both runtimes produce BYTE-IDENTICAL transcripts for the same inputs —
 * this is the coordinated-release contract per ADR
 * 2026-04-16-coordinated-rust-npm-releases.md (bead mxdx-fqt).
 *
 * Transcript layout (storm §3.1):
 *
 *   transcript = "mxdx.p2p.verify.v1"
 *              || 0x00 || room_id
 *              || 0x00 || session_uuid (empty if null)
 *              || 0x00 || call_id
 *              || offerer_nonce (32 bytes, no separator)
 *              || answerer_nonce (32 bytes, no separator)
 *              || 0x00 || offerer_party_id
 *              || 0x00 || answerer_party_id
 *              || 0x00 || offerer_sdp_fingerprint
 *              || 0x00 || answerer_sdp_fingerprint
 *
 * Nonces are fixed-width (32 bytes each) with NO separator between them.
 * All other variable-length fields get a preceding 0x00 separator.
 */

import { webcrypto } from 'node:crypto';

export const DOMAIN_SEPARATION_TAG = 'mxdx.p2p.verify.v1';
export const NONCE_LEN = 32;
export const ED25519_SIGNATURE_LEN = 64;
export const ED25519_PUBLIC_KEY_LEN = 32;
const FIELD_SEP = 0x00;

/**
 * Build the canonical transcript bytes — byte-identical to Rust's
 * `build_transcript`.
 *
 * @param {object} params
 * @param {string} params.roomId
 * @param {string|null} params.sessionUuid — empty string if null
 * @param {string} params.callId
 * @param {Uint8Array} params.offererNonce — 32 bytes
 * @param {Uint8Array} params.answererNonce — 32 bytes
 * @param {string} params.offererPartyId
 * @param {string} params.answererPartyId
 * @param {string} params.offererSdpFingerprint — upper-case, e.g. "AA:BB:..."
 * @param {string} params.answererSdpFingerprint
 * @returns {Uint8Array}
 */
export function buildTranscript(params) {
  const {
    roomId,
    sessionUuid,
    callId,
    offererNonce,
    answererNonce,
    offererPartyId,
    answererPartyId,
    offererSdpFingerprint,
    answererSdpFingerprint,
  } = params;

  if (offererNonce.length !== NONCE_LEN) {
    throw new Error(`offerer_nonce must be ${NONCE_LEN} bytes`);
  }
  if (answererNonce.length !== NONCE_LEN) {
    throw new Error(`answerer_nonce must be ${NONCE_LEN} bytes`);
  }

  const enc = new TextEncoder();
  const parts = [
    enc.encode(DOMAIN_SEPARATION_TAG),
    new Uint8Array([FIELD_SEP]),
    enc.encode(roomId),
    new Uint8Array([FIELD_SEP]),
    enc.encode(sessionUuid ?? ''),
    new Uint8Array([FIELD_SEP]),
    enc.encode(callId),
    offererNonce, // no separator — fixed-width
    answererNonce, // no separator — fixed-width
    new Uint8Array([FIELD_SEP]),
    enc.encode(offererPartyId),
    new Uint8Array([FIELD_SEP]),
    enc.encode(answererPartyId),
    new Uint8Array([FIELD_SEP]),
    enc.encode(offererSdpFingerprint),
    new Uint8Array([FIELD_SEP]),
    enc.encode(answererSdpFingerprint),
  ];

  const total = parts.reduce((acc, p) => acc + p.length, 0);
  const out = new Uint8Array(total);
  let off = 0;
  for (const p of parts) {
    out.set(p, off);
    off += p.length;
  }
  return out;
}

/**
 * Canonical ordering — which of (ours, theirs) goes offerer-first.
 * Mirrors Rust's `canonical_ordering`.
 */
export function canonicalOrdering(ours, theirs, weAreOfferer) {
  return weAreOfferer ? [ours, theirs] : [theirs, ours];
}

/**
 * Extract the `a=fingerprint:sha-256 AA:BB:...` value from an SDP blob.
 * Returns the upper-cased colon-hex form. On multiple matches, returns
 * lexicographically smallest (matches Rust). Throws if none found.
 */
export function extractSdpFingerprint(sdp) {
  const candidates = [];
  for (const line of sdp.split(/\r?\n/)) {
    const trimmed = line.trim();
    const idx = trimmed.indexOf('a=fingerprint:');
    if (idx !== 0) continue;
    const rest = trimmed.slice('a=fingerprint:'.length);
    const ws = rest.search(/\s/);
    if (ws < 0) continue;
    const algo = rest.slice(0, ws);
    const fp = rest.slice(ws + 1).trim();
    if (algo.toLowerCase() === 'sha-256' && fp.length > 0) {
      candidates.push(fp.toUpperCase());
    }
  }
  if (candidates.length === 0) {
    throw new Error('missing_sdp_fingerprint');
  }
  candidates.sort();
  return candidates[0];
}

/** Pair helper: call extractSdpFingerprint on offer + answer. */
export function canonicalSdpFingerprints(offerSdp, answerSdp) {
  return [extractSdpFingerprint(offerSdp), extractSdpFingerprint(answerSdp)];
}

/**
 * Generate a fresh 32-byte CSPRNG nonce.
 */
export function generateNonce() {
  const out = new Uint8Array(NONCE_LEN);
  webcrypto.getRandomValues(out);
  return out;
}

// --- base64 url-safe encoding without padding (matches Rust's
//     STANDARD_NO_PAD via base64::engine::general_purpose). We use
//     standard (not urlsafe) to match Rust's default. ---

export function b64encode(bytes) {
  return Buffer.from(bytes).toString('base64').replace(/=+$/, '');
}

export function b64decode(s) {
  const padded = s + '='.repeat((4 - (s.length % 4)) % 4);
  return new Uint8Array(Buffer.from(padded, 'base64'));
}

/**
 * Generate an ephemeral Ed25519 keypair via Node's webcrypto subtle API.
 * Returns { privateKey, publicKey } where both are CryptoKey handles
 * suitable for webcrypto sign/verify, and publicKeyBytes is the raw
 * 32-byte public key.
 */
export async function generateEphemeralKeypair() {
  const kp = await webcrypto.subtle.generateKey(
    { name: 'Ed25519' },
    true,
    ['sign', 'verify'],
  );
  const spki = await webcrypto.subtle.exportKey('raw', kp.publicKey);
  return {
    privateKey: kp.privateKey,
    publicKey: kp.publicKey,
    publicKeyBytes: new Uint8Array(spki),
  };
}

/** Sign a transcript with an Ed25519 private CryptoKey. Returns 64-byte sig. */
export async function signTranscript(privateKey, transcript) {
  const sig = await webcrypto.subtle.sign('Ed25519', privateKey, transcript);
  const out = new Uint8Array(sig);
  if (out.length !== ED25519_SIGNATURE_LEN) {
    throw new Error(`unexpected signature length ${out.length}`);
  }
  return out;
}

/** Verify a transcript signature against a raw 32-byte Ed25519 public key. */
export async function verifyTranscript(publicKeyBytes, signature, transcript) {
  if (publicKeyBytes.length !== ED25519_PUBLIC_KEY_LEN) {
    throw new Error(`invalid public key length ${publicKeyBytes.length}`);
  }
  if (signature.length !== ED25519_SIGNATURE_LEN) {
    throw new Error(`invalid signature length ${signature.length}`);
  }
  const key = await webcrypto.subtle.importKey(
    'raw',
    publicKeyBytes,
    { name: 'Ed25519' },
    false,
    ['verify'],
  );
  return webcrypto.subtle.verify('Ed25519', key, signature, transcript);
}

/**
 * Build a `verify_challenge` frame (step 1 of storm §3.1 handshake).
 * Each peer sends its nonce + device_id. No signature yet — both nonces
 * are needed for the transcript.
 */
export function buildChallengeFrame(ourNonce, ourDeviceId) {
  return {
    type: 'verify_challenge',
    nonce_b64: b64encode(ourNonce),
    device_id: ourDeviceId,
  };
}

/**
 * Build a `verify_response` frame (step 2): each peer signs the full
 * transcript and sends the signature + public key.
 */
export async function buildResponseFrame({
  privateKey,
  publicKeyBytes,
  transcript,
  ourDeviceId,
}) {
  const sig = await signTranscript(privateKey, transcript);
  return {
    type: 'verify_response',
    signature_b64: b64encode(sig),
    signer_ed25519_b64: b64encode(publicKeyBytes),
    device_id: ourDeviceId,
  };
}

/**
 * Parse a `verify_challenge` frame. Returns { nonce, deviceId }. Throws
 * on malformed input.
 */
export function parseChallengeFrame(frame) {
  if (!frame || frame.type !== 'verify_challenge') {
    throw new Error('not a verify_challenge frame');
  }
  if (typeof frame.nonce_b64 !== 'string' || typeof frame.device_id !== 'string') {
    throw new Error('malformed verify_challenge');
  }
  const nonce = b64decode(frame.nonce_b64);
  if (nonce.length !== NONCE_LEN) {
    throw new Error(`invalid nonce length ${nonce.length}`);
  }
  return { nonce, deviceId: frame.device_id };
}

/**
 * Parse a `verify_response` frame. Returns { signature, signerPk, deviceId }.
 * Throws on malformed input. The caller is responsible for gating
 * trust on `signerPk === matrix_known_device_key` BEFORE calling
 * verifyTranscript — the wire-carried signer_ed25519_b64 must NEVER be
 * trusted on its own.
 */
export function parseResponseFrame(frame) {
  if (!frame || frame.type !== 'verify_response') {
    throw new Error('not a verify_response frame');
  }
  if (
    typeof frame.signature_b64 !== 'string' ||
    typeof frame.signer_ed25519_b64 !== 'string' ||
    typeof frame.device_id !== 'string'
  ) {
    throw new Error('malformed verify_response');
  }
  const signature = b64decode(frame.signature_b64);
  const signerPk = b64decode(frame.signer_ed25519_b64);
  if (signature.length !== ED25519_SIGNATURE_LEN) {
    throw new Error(`invalid signature length ${signature.length}`);
  }
  if (signerPk.length !== ED25519_PUBLIC_KEY_LEN) {
    throw new Error(`invalid signer pk length ${signerPk.length}`);
  }
  return {
    signature,
    signerPk,
    deviceId: frame.device_id,
  };
}
