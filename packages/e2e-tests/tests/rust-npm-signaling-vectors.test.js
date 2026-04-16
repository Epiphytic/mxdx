// Cross-runtime signaling-vector verification for T-40 / T-44 coordinated
// release (ADR docs/adr/2026-04-15-mcall-wire-format.md addendum 2026-04-16 +
// docs/adr/2026-04-16-coordinated-rust-npm-releases.md). Loads the same
// committed fixture that `crates/mxdx-p2p/tests/signaling_vectors.rs` emits,
// runs two byte-exact checks:
//
// 1. PARSE:  every Rust-emitted JSON fixture parses cleanly through
//            JSON.parse and carries the expected fields in the npm runtime.
//            Guarantees "npm can read Rust wire output".
//
// 2. EMIT:   invoking `P2PSignaling.sendInvite/sendAnswer/...` through a
//            mock sendEvent captures the JSON string npm emits and
//            compares it byte-for-byte with the Rust-emitted fixture for
//            the same logical inputs. Guarantees "Rust can read npm wire
//            output" because serde parses any superset of what we emit.
//
// Run: `node --test packages/e2e-tests/tests/rust-npm-signaling-vectors.test.js`

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';

import { P2PSignaling } from '../../core/p2p-signaling.js';

const __dirname = dirname(fileURLToPath(import.meta.url));
const FIXTURE_PATH = resolve(
  __dirname,
  '../../../crates/mxdx-p2p/tests/fixtures/signaling-vectors.json',
);

function loadFixture() {
  const fixture = JSON.parse(readFileSync(FIXTURE_PATH, 'utf8'));
  assert.equal(fixture.version, 1, 'signaling fixture version mismatch');
  return fixture;
}

// ---------------------------------------------------------------------------
// PASS 1 — npm can parse Rust-emitted JSON
// ---------------------------------------------------------------------------

test('npm parses Rust-emitted invite with mxdx_session_key', () => {
  const fixture = loadFixture();
  const parsed = JSON.parse(fixture.invite_with_session_key.json);
  assert.equal(parsed.call_id, 'c1');
  assert.equal(parsed.party_id, 'p1');
  assert.equal(parsed.version, '1');
  assert.equal(parsed.lifetime, 30000);
  assert.equal(parsed.offer.type, 'offer');
  assert.equal(parsed.offer.sdp, 'v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\n');
  assert.equal(
    parsed.mxdx_session_key,
    'AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=',
  );
  assert.equal(parsed.session_uuid, undefined, 'session_uuid should be absent');
  assert.equal(
    parsed.session_key,
    undefined,
    'legacy session_key must be absent — coordinated-release migration must not regress',
  );
});

test('npm parses Rust-emitted invite without mxdx_session_key', () => {
  const fixture = loadFixture();
  const parsed = JSON.parse(fixture.invite_without_session_key.json);
  assert.equal(parsed.call_id, 'c2');
  assert.equal(parsed.mxdx_session_key, undefined);
  assert.equal(parsed.session_uuid, undefined);
});

test('npm parses Rust-emitted invite with session_uuid', () => {
  const fixture = loadFixture();
  const parsed = JSON.parse(fixture.invite_with_session_uuid.json);
  assert.equal(parsed.call_id, 'c3');
  assert.equal(parsed.session_uuid, 'sess-abc-123');
  assert.equal(
    parsed.mxdx_session_key,
    'AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=',
  );
});

test('npm parses Rust-emitted answer', () => {
  const fixture = loadFixture();
  const parsed = JSON.parse(fixture.answer.json);
  assert.equal(parsed.call_id, 'c1');
  assert.equal(parsed.party_id, 'p2');
  assert.equal(parsed.version, '1');
  assert.equal(parsed.answer.type, 'answer');
  assert.equal(parsed.answer.sdp, 'v=0\r\na=answer\r\n');
});

test('npm parses Rust-emitted candidates (node shape: no sdpMLineIndex)', () => {
  const fixture = loadFixture();
  const parsed = JSON.parse(fixture.candidates_node_shape.json);
  assert.equal(parsed.candidates.length, 2);
  assert.equal(parsed.candidates[0].sdpMid, '0');
  assert.equal(parsed.candidates[0].sdpMLineIndex, undefined);
  assert.equal(parsed.candidates[1].sdpMid, '0');
});

test('npm parses Rust-emitted candidates (browser shape: with sdpMLineIndex)', () => {
  const fixture = loadFixture();
  const parsed = JSON.parse(fixture.candidates_browser_shape.json);
  assert.equal(parsed.candidates.length, 1);
  assert.equal(parsed.candidates[0].sdpMid, '0');
  assert.equal(parsed.candidates[0].sdpMLineIndex, 0);
});

test('npm parses Rust-emitted hangup with and without reason', () => {
  const fixture = loadFixture();
  const withReason = JSON.parse(fixture.hangup_with_reason.json);
  assert.equal(withReason.reason, 'idle_timeout');
  const withoutReason = JSON.parse(fixture.hangup_without_reason.json);
  assert.equal(withoutReason.reason, undefined);
});

test('npm parses Rust-emitted select_answer', () => {
  const fixture = loadFixture();
  const parsed = JSON.parse(fixture.select_answer.json);
  assert.equal(parsed.selected_party_id, 'remote-party-7');
});

// ---------------------------------------------------------------------------
// PASS 2 — npm emitter is byte-for-byte identical to the Rust fixture for the
// same logical inputs. Captures the JSON string via a mock sendEvent.
// ---------------------------------------------------------------------------

function mockSignaling() {
  const sent = [];
  return [
    new P2PSignaling(
      {
        sendEvent: async (_roomId, type, contentJson) => {
          // `contentJson` is already a JSON string (p2p-signaling.js line 160
          // does `JSON.stringify(content)` before calling sendEvent).
          sent.push({ type, contentJson });
        },
        onRoomEvent: async () => 'null',
      },
      '!dm:ex',
      '@me:ex',
    ),
    sent,
  ];
}

test('npm sendInvite emits byte-for-byte the Rust fixture (with session key)', async () => {
  const fixture = loadFixture();
  const [sig, sent] = mockSignaling();
  await sig.sendInvite({
    callId: 'c1',
    partyId: 'p1',
    sdp: 'v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\n',
    lifetime: 30000,
    sessionKey: 'AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=',
  });
  assert.equal(sent[0].type, 'm.call.invite');
  assert.equal(
    sent[0].contentJson,
    fixture.invite_with_session_key.json,
    'npm emitter diverged from Rust fixture — coordinated-release wire contract broken',
  );
});

test('npm sendInvite emits byte-for-byte the Rust fixture (without session key)', async () => {
  const fixture = loadFixture();
  const [sig, sent] = mockSignaling();
  await sig.sendInvite({
    callId: 'c2',
    partyId: 'p2',
    sdp: 'sdp-placeholder',
    lifetime: 30000,
  });
  assert.equal(
    sent[0].contentJson,
    fixture.invite_without_session_key.json,
  );
});

test('npm sendAnswer emits byte-for-byte the Rust fixture', async () => {
  const fixture = loadFixture();
  const [sig, sent] = mockSignaling();
  await sig.sendAnswer({
    callId: 'c1',
    partyId: 'p2',
    sdp: 'v=0\r\na=answer\r\n',
  });
  assert.equal(sent[0].contentJson, fixture.answer.json);
});

test('npm sendCandidates emits byte-for-byte the Rust fixture (node shape)', async () => {
  const fixture = loadFixture();
  const [sig, sent] = mockSignaling();
  await sig.sendCandidates({
    callId: 'c1',
    partyId: 'p1',
    candidates: [
      { candidate: 'candidate:1 1 UDP 2130706431 192.168.1.100 12345 typ host', sdpMid: '0' },
      { candidate: 'candidate:2 1 UDP 1694498815 203.0.113.1 54321 typ srflx', sdpMid: '0' },
    ],
  });
  assert.equal(sent[0].contentJson, fixture.candidates_node_shape.json);
});

test('npm sendHangup emits byte-for-byte the Rust fixture (with reason)', async () => {
  const fixture = loadFixture();
  const [sig, sent] = mockSignaling();
  await sig.sendHangup({ callId: 'c1', partyId: 'p1', reason: 'idle_timeout' });
  assert.equal(sent[0].contentJson, fixture.hangup_with_reason.json);
});

test('npm sendSelectAnswer emits byte-for-byte the Rust fixture', async () => {
  const fixture = loadFixture();
  const [sig, sent] = mockSignaling();
  await sig.sendSelectAnswer({
    callId: 'c1',
    partyId: 'p1',
    selectedPartyId: 'remote-party-7',
  });
  assert.equal(sent[0].contentJson, fixture.select_answer.json);
});

// ---------------------------------------------------------------------------
// PASS 3 — explicit coordinated-release regression guards
// ---------------------------------------------------------------------------

test('P2PSignaling default lifetime is 30000 (coordinated-release ADR)', async () => {
  const [sig, sent] = mockSignaling();
  await sig.sendInvite({ callId: 'c1', partyId: 'p1', sdp: 'x' });
  const parsed = JSON.parse(sent[0].contentJson);
  assert.equal(parsed.lifetime, 30000);
});

test('P2PSignaling never emits legacy `session_key` field name', async () => {
  const [sig, sent] = mockSignaling();
  await sig.sendInvite({
    callId: 'c1',
    partyId: 'p1',
    sdp: 'x',
    sessionKey: 'AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=',
  });
  assert.ok(
    !sent[0].contentJson.includes('"session_key"'),
    `legacy session_key must not appear on the wire, got: ${sent[0].contentJson}`,
  );
  assert.ok(sent[0].contentJson.includes('"mxdx_session_key"'));
});
