// npm-only unit test for the Ed25519-signed Verifying handshake
// (bead mxdx-fqt). Exercises the sign/verify round-trip over the
// canonical transcript, and the HandshakeMsg-frame parse/emit contract.

import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  generateEphemeralKeypair,
  signTranscript,
  verifyTranscript,
  buildTranscript,
  buildChallengeFrame,
  buildResponseFrame,
  parseChallengeFrame,
  parseResponseFrame,
  generateNonce,
  b64encode,
  b64decode,
  NONCE_LEN,
  ED25519_PUBLIC_KEY_LEN,
  ED25519_SIGNATURE_LEN,
} from '../../core/p2p-verify.js';

test('ephemeral keypair has correct shape', async () => {
  const { publicKeyBytes } = await generateEphemeralKeypair();
  assert.equal(publicKeyBytes.length, ED25519_PUBLIC_KEY_LEN);
});

test('sign/verify round-trip succeeds on valid transcript', async () => {
  const { privateKey, publicKeyBytes } = await generateEphemeralKeypair();
  const transcript = new TextEncoder().encode('hello transcript');
  const sig = await signTranscript(privateKey, transcript);
  assert.equal(sig.length, ED25519_SIGNATURE_LEN);

  const ok = await verifyTranscript(publicKeyBytes, sig, transcript);
  assert.ok(ok, 'signature must verify against signer public key');
});

test('verify rejects tampered transcript', async () => {
  const { privateKey, publicKeyBytes } = await generateEphemeralKeypair();
  const transcript = new TextEncoder().encode('hello transcript');
  const sig = await signTranscript(privateKey, transcript);

  const tampered = new TextEncoder().encode('hello transcripX'); // last byte changed
  const ok = await verifyTranscript(publicKeyBytes, sig, tampered);
  assert.equal(ok, false, 'tampered transcript must NOT verify');
});

test('verify rejects wrong signer public key', async () => {
  const { privateKey } = await generateEphemeralKeypair();
  const { publicKeyBytes: otherPk } = await generateEphemeralKeypair();
  const transcript = new TextEncoder().encode('hello');
  const sig = await signTranscript(privateKey, transcript);

  const ok = await verifyTranscript(otherPk, sig, transcript);
  assert.equal(ok, false, 'wrong public key must NOT verify');
});

test('generated nonce is 32 random bytes', () => {
  const a = generateNonce();
  const b = generateNonce();
  assert.equal(a.length, NONCE_LEN);
  assert.equal(b.length, NONCE_LEN);
  assert.notDeepEqual(a, b, 'two CSPRNG nonces should differ');
});

test('challenge frame round-trip', () => {
  const nonce = generateNonce();
  const frame = buildChallengeFrame(nonce, 'DEVICE_A');
  const json = JSON.stringify(frame);
  const parsed = parseChallengeFrame(JSON.parse(json));
  assert.deepEqual(parsed.nonce, nonce);
  assert.equal(parsed.deviceId, 'DEVICE_A');
});

test('response frame round-trip', async () => {
  const { privateKey, publicKeyBytes } = await generateEphemeralKeypair();
  const transcript = new TextEncoder().encode('some transcript bytes');
  const frame = await buildResponseFrame({
    privateKey,
    publicKeyBytes,
    transcript,
    ourDeviceId: 'DEVICE_B',
  });
  assert.equal(frame.type, 'verify_response');

  const parsed = parseResponseFrame(JSON.parse(JSON.stringify(frame)));
  assert.equal(parsed.deviceId, 'DEVICE_B');
  assert.equal(parsed.signature.length, ED25519_SIGNATURE_LEN);
  assert.equal(parsed.signerPk.length, ED25519_PUBLIC_KEY_LEN);

  const ok = await verifyTranscript(parsed.signerPk, parsed.signature, transcript);
  assert.ok(ok);
});

test('parseChallengeFrame rejects malformed frames', () => {
  assert.throws(() => parseChallengeFrame({ type: 'other' }));
  assert.throws(() => parseChallengeFrame({ type: 'verify_challenge' }));
  assert.throws(() =>
    parseChallengeFrame({ type: 'verify_challenge', nonce_b64: b64encode(new Uint8Array(10)), device_id: 'D' }),
  );
});

test('parseResponseFrame rejects malformed frames', () => {
  assert.throws(() => parseResponseFrame({ type: 'other' }));
  assert.throws(() => parseResponseFrame({ type: 'verify_response' }));
});

test('b64 encode/decode round-trip', () => {
  const bytes = new Uint8Array([1, 2, 3, 4, 5, 253, 254, 255]);
  const b64 = b64encode(bytes);
  const back = b64decode(b64);
  assert.deepEqual(back, bytes);
});

test('full end-to-end handshake with valid transcript', async () => {
  // Two peers; both generate keypairs + nonces; build transcript with
  // both nonces; each signs the same transcript bytes; verify.
  const alice = await generateEphemeralKeypair();
  const bob = await generateEphemeralKeypair();
  const aliceNonce = generateNonce();
  const bobNonce = generateNonce();

  // Canonical ordering: alice is offerer.
  const transcript = buildTranscript({
    roomId: '!r:ex',
    sessionUuid: 'session-1',
    callId: 'c1',
    offererNonce: aliceNonce,
    answererNonce: bobNonce,
    offererPartyId: 'alice',
    answererPartyId: 'bob',
    offererSdpFingerprint: 'AA:BB',
    answererSdpFingerprint: 'CC:DD',
  });

  const aliceResp = await buildResponseFrame({
    privateKey: alice.privateKey,
    publicKeyBytes: alice.publicKeyBytes,
    transcript,
    ourDeviceId: 'ALICE',
  });
  const bobResp = await buildResponseFrame({
    privateKey: bob.privateKey,
    publicKeyBytes: bob.publicKeyBytes,
    transcript,
    ourDeviceId: 'BOB',
  });

  // Bob verifies Alice's signature (against Alice's Matrix-known pk):
  const aliceParsed = parseResponseFrame(aliceResp);
  assert.ok(
    await verifyTranscript(alice.publicKeyBytes, aliceParsed.signature, transcript),
    'Bob must be able to verify Alice',
  );
  // Alice verifies Bob's signature (against Bob's Matrix-known pk):
  const bobParsed = parseResponseFrame(bobResp);
  assert.ok(
    await verifyTranscript(bob.publicKeyBytes, bobParsed.signature, transcript),
    'Alice must be able to verify Bob',
  );
});
