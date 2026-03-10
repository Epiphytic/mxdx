/**
 * P2P Transport: Public Matrix Server Signaling Tests.
 *
 * Verifies P2P signaling works over a real public Matrix homeserver (matrix.org).
 * Only tests signaling (m.call.invite / m.call.answer exchange), since actual
 * WebRTC data channel establishment requires TURN servers which may not be
 * available on public homeservers.
 *
 * Requires test-credentials.toml in repo root.
 *
 * Run with: node --test packages/e2e-tests/tests/p2p-public-server.test.js
 */
import { describe, it, before, after } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { WasmMatrixClient, P2PSignaling } from '@mxdx/core';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(__dirname, '../../..');

function loadCredentials() {
  const tomlPath = path.join(REPO_ROOT, 'test-credentials.toml');
  if (!fs.existsSync(tomlPath)) {
    throw new Error(
      'test-credentials.toml not found. See public-server.test.js for setup.'
    );
  }
  const content = fs.readFileSync(tomlPath, 'utf8');
  const lines = content.split('\n');
  const result = {};
  let section = null;
  for (const line of lines) {
    const trimmed = line.trim();
    const sectionMatch = trimmed.match(/^\[(\w+)\]$/);
    if (sectionMatch) {
      section = sectionMatch[1];
      result[section] = {};
      continue;
    }
    const kvMatch = trimmed.match(/^(\w+)\s*=\s*"(.+)"$/);
    if (kvMatch && section) {
      result[section][kvMatch[1]] = kvMatch[2];
    }
  }
  if (!result.server?.url) throw new Error('server.url missing');
  if (!result.account1?.username) throw new Error('account1 missing');
  if (!result.account2?.username) throw new Error('account2 missing');
  return {
    url: result.server.url,
    account1: result.account1,
    account2: result.account2,
  };
}

function sleep(ms) {
  return new Promise(r => setTimeout(r, ms));
}

describe('P2P Transport: Public Server Signaling', { timeout: 180000 }, () => {
  let creds;
  let client1;
  let client2;
  let dmRoomId;

  before(async () => {
    creds = loadCredentials();

    // Login both accounts
    client1 = await WasmMatrixClient.login(
      creds.url, creds.account1.username, creds.account1.password,
    );
    console.log(`[p2p-pub] Account1 logged in: ${client1.userId()}`);

    client2 = await WasmMatrixClient.login(
      creds.url, creds.account2.username, creds.account2.password,
    );
    console.log(`[p2p-pub] Account2 logged in: ${client2.userId()}`);

    // Sync both
    await client1.syncOnce();
    await client2.syncOnce();

    // Create DM room for signaling test
    dmRoomId = await client1.createDmRoom(client2.userId());
    console.log(`[p2p-pub] DM room created: ${dmRoomId}`);

    // Client2 joins
    await sleep(2000);
    await client2.syncOnce();
    await client2.joinRoom(dmRoomId);
    await client2.syncOnce();
    await client1.syncOnce();
    console.log('[p2p-pub] Both clients in DM room');
  });

  after(() => {
    if (client1) client1.free();
    if (client2) client2.free();
  });

  it('m.call.invite/answer round-trip over public homeserver', async () => {
    const callId = P2PSignaling.generateCallId();
    const partyId1 = P2PSignaling.generatePartyId();
    const partyId2 = P2PSignaling.generatePartyId();

    const sigA = new P2PSignaling(
      {
        sendEvent: (roomId, type, content) => client1.sendEvent(roomId, type, content),
        onRoomEvent: (roomId, cb) => client1.onRoomEvent(roomId, cb),
      },
      dmRoomId,
      client1.userId(),
    );

    const sigB = new P2PSignaling(
      {
        sendEvent: (roomId, type, content) => client2.sendEvent(roomId, type, content),
        onRoomEvent: (roomId, cb) => client2.onRoomEvent(roomId, cb),
      },
      dmRoomId,
      client2.userId(),
    );

    // Account1 sends invite
    const fakeSdp = 'v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=mxdx-test\r\nt=0 0\r\n';
    await sigA.sendInvite({ callId, partyId: partyId1, sdp: fakeSdp, lifetime: 60000 });
    console.log(`[p2p-pub] Invite sent: call_id=${callId}`);

    // Account2 receives invite
    await sleep(2000);
    await client2.syncOnce();
    const inviteJson = await client2.onRoomEvent(dmRoomId, 'm.call.invite', 30);
    assert.ok(inviteJson && inviteJson !== 'null', 'Account2 should receive m.call.invite');

    const invite = JSON.parse(inviteJson);
    const inviteContent = invite.content || invite;
    assert.equal(inviteContent.call_id, callId, 'call_id should match');
    assert.ok(inviteContent.offer.sdp.includes('mxdx-test'), 'SDP should contain test marker');
    console.log('[p2p-pub] Invite received and verified');

    // Account2 sends answer
    const answerSdp = 'v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=mxdx-answer\r\nt=0 0\r\n';
    await sigB.sendAnswer({ callId, partyId: partyId2, sdp: answerSdp });
    console.log('[p2p-pub] Answer sent');

    // Account1 receives answer
    await sleep(2000);
    await client1.syncOnce();
    const answerJson = await client1.onRoomEvent(dmRoomId, 'm.call.answer', 30);
    assert.ok(answerJson && answerJson !== 'null', 'Account1 should receive m.call.answer');

    const answer = JSON.parse(answerJson);
    const answerContent = answer.content || answer;
    assert.equal(answerContent.call_id, callId, 'Answer call_id should match');
    assert.ok(answerContent.answer.sdp.includes('mxdx-answer'), 'Answer SDP should contain marker');
    console.log('[p2p-pub] Answer received and verified');
    console.log('[p2p-pub] Full invite/answer round-trip over public homeserver succeeded');
  });

  it('m.call.candidates exchange over public homeserver', async () => {
    const callId = P2PSignaling.generateCallId();
    const partyId = P2PSignaling.generatePartyId();

    const sigA = new P2PSignaling(
      {
        sendEvent: (roomId, type, content) => client1.sendEvent(roomId, type, content),
        onRoomEvent: (roomId, cb) => client1.onRoomEvent(roomId, cb),
      },
      dmRoomId,
      client1.userId(),
    );

    // Send ICE candidates
    const testCandidates = [
      { candidate: 'candidate:1 1 UDP 2130706431 192.168.1.100 12345 typ host', sdpMid: '0' },
      { candidate: 'candidate:2 1 UDP 1694498815 203.0.113.1 54321 typ srflx', sdpMid: '0' },
    ];

    await sigA.sendCandidates({ callId, partyId, candidates: testCandidates });
    console.log('[p2p-pub] Candidates sent');

    // Account2 receives candidates
    await sleep(2000);
    await client2.syncOnce();
    const candJson = await client2.onRoomEvent(dmRoomId, 'm.call.candidates', 30);
    assert.ok(candJson && candJson !== 'null', 'Account2 should receive m.call.candidates');

    const candEvent = JSON.parse(candJson);
    const candContent = candEvent.content || candEvent;
    assert.equal(candContent.call_id, callId, 'call_id should match');
    assert.ok(Array.isArray(candContent.candidates), 'Should contain candidates array');
    assert.equal(candContent.candidates.length, 2, 'Should have 2 candidates');
    assert.ok(candContent.candidates[0].candidate.includes('192.168.1.100'), 'First candidate should match');
    console.log('[p2p-pub] ICE candidates exchange verified over public homeserver');
  });

  it('m.call.hangup delivered over public homeserver', async () => {
    const callId = P2PSignaling.generateCallId();
    const partyId = P2PSignaling.generatePartyId();

    const sigA = new P2PSignaling(
      {
        sendEvent: (roomId, type, content) => client1.sendEvent(roomId, type, content),
        onRoomEvent: (roomId, cb) => client1.onRoomEvent(roomId, cb),
      },
      dmRoomId,
      client1.userId(),
    );

    await sigA.sendHangup({ callId, partyId, reason: 'user_hangup' });
    console.log('[p2p-pub] Hangup sent');

    await sleep(2000);
    await client2.syncOnce();
    const hangupJson = await client2.onRoomEvent(dmRoomId, 'm.call.hangup', 30);
    assert.ok(hangupJson && hangupJson !== 'null', 'Account2 should receive m.call.hangup');

    const hangup = JSON.parse(hangupJson);
    const hangupContent = hangup.content || hangup;
    assert.equal(hangupContent.call_id, callId, 'call_id should match');
    assert.equal(hangupContent.reason, 'user_hangup', 'Reason should match');
    console.log('[p2p-pub] m.call.hangup verified over public homeserver');
  });
});
