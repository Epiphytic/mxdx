// Cross-runtime verify-handshake transcript parity (bead mxdx-fqt,
// coordinated release per ADR 2026-04-16-coordinated-rust-npm-releases.md).
//
// Loads the same fixture emitted by
// `crates/mxdx-p2p/tests/verify_vectors.rs` and asserts that npm's
// `buildTranscript` produces byte-identical output for the same inputs.
// If this test fails, Rust↔npm P2P handshake will fail at Verifying —
// both runtimes must sign over the same bytes.
//
// Run: `node --test packages/e2e-tests/tests/rust-npm-verify-transcript-vectors.test.js`

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';

import {
  buildTranscript,
  extractSdpFingerprint,
  b64decode,
  NONCE_LEN,
  DOMAIN_SEPARATION_TAG,
} from '../../core/p2p-verify.js';

const __dirname = dirname(fileURLToPath(import.meta.url));
const FIXTURE_PATH = resolve(
  __dirname,
  '../../../crates/mxdx-p2p/tests/fixtures/verify-vectors.json',
);

function loadFixture() {
  const fixture = JSON.parse(readFileSync(FIXTURE_PATH, 'utf8'));
  assert.equal(fixture.version, 1, 'verify fixture version mismatch');
  return fixture;
}

function hexEncode(bytes) {
  return Array.from(bytes)
    .map((b) => b.toString(16).padStart(2, '0'))
    .join('');
}

test('npm transcript matches Rust fixture bytes (coordinated release contract)', () => {
  const fixture = loadFixture();
  const v = fixture.transcript_vector_basic;
  const inputs = v.inputs;

  const offererNonce = b64decode(inputs.offerer_nonce_b64);
  const answererNonce = b64decode(inputs.answerer_nonce_b64);
  assert.equal(offererNonce.length, NONCE_LEN);
  assert.equal(answererNonce.length, NONCE_LEN);

  const transcript = buildTranscript({
    roomId: inputs.room_id,
    sessionUuid: inputs.session_uuid,
    callId: inputs.call_id,
    offererNonce,
    answererNonce,
    offererPartyId: inputs.offerer_party_id,
    answererPartyId: inputs.answerer_party_id,
    offererSdpFingerprint: inputs.offerer_sdp_fingerprint,
    answererSdpFingerprint: inputs.answerer_sdp_fingerprint,
  });

  const computedHex = hexEncode(transcript);
  assert.equal(
    computedHex,
    v.transcript_hex,
    `npm transcript bytes diverged from Rust fixture.
Expected: ${v.transcript_hex}
Computed: ${computedHex}

If this test fails after a schema change, regenerate the fixture
(cargo test -p mxdx-p2p --test verify_vectors -- --ignored generate_verify_vectors --exact --nocapture)
AND update BOTH Rust (crates/mxdx-p2p/src/transport/verify.rs) AND npm
(packages/core/p2p-verify.js) in the same coordinated release.`,
  );
});

test('npm SDP fingerprint normalization matches Rust fixture', () => {
  const fixture = loadFixture();
  const cases = fixture.sdp_fingerprint_normalization.cases;
  for (const c of cases) {
    const got = extractSdpFingerprint(c.sdp);
    assert.equal(
      got,
      c.expected,
      `SDP fingerprint mismatch for ${JSON.stringify(c.sdp)}: expected ${c.expected}, got ${got}`,
    );
  }
});

test('npm domain separation tag matches Rust', () => {
  // Soft assertion — the tag is load-bearing for domain separation.
  // Storm §3.1: "mxdx.p2p.verify.v1"
  assert.equal(DOMAIN_SEPARATION_TAG, 'mxdx.p2p.verify.v1');
});

test('nonce length constant matches Rust (32 bytes)', () => {
  assert.equal(NONCE_LEN, 32);
});

test('transcript is deterministic — same inputs always produce same bytes', () => {
  const inputs = {
    roomId: '!r:ex',
    sessionUuid: null,
    callId: 'c1',
    offererNonce: new Uint8Array(32).fill(1),
    answererNonce: new Uint8Array(32).fill(2),
    offererPartyId: 'A',
    answererPartyId: 'B',
    offererSdpFingerprint: 'AA:BB',
    answererSdpFingerprint: 'CC:DD',
  };
  const t1 = buildTranscript(inputs);
  const t2 = buildTranscript(inputs);
  assert.deepEqual(t1, t2);
});

test('transcript with null session_uuid renders as empty', () => {
  // Rust: empty string segment between session_uuid separators.
  const inputs = {
    roomId: '!r:ex',
    sessionUuid: null,
    callId: 'c1',
    offererNonce: new Uint8Array(32),
    answererNonce: new Uint8Array(32),
    offererPartyId: 'A',
    answererPartyId: 'B',
    offererSdpFingerprint: 'AA',
    answererSdpFingerprint: 'BB',
  };
  const tNull = buildTranscript(inputs);
  const tEmpty = buildTranscript({ ...inputs, sessionUuid: '' });
  assert.deepEqual(tNull, tEmpty, 'null and empty session_uuid must produce identical transcripts');
});
