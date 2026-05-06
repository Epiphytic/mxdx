import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import { P2PSignaling } from '../../../packages/core/p2p-signaling.js';

describe('P2PSignaling', () => {
  it('sends m.call.invite with correct fields', async () => {
    const sent = [];
    const mockClient = {
      sendEvent: async (roomId, type, content) => {
        sent.push({ roomId, type, content: JSON.parse(content) });
      },
      onRoomEvent: async () => null,
    };

    const sig = new P2PSignaling(mockClient, '!dm:ex', '@me:ex');
    await sig.sendInvite({ callId: 'c1', partyId: 'p1', sdp: 'v=0...', lifetime: 30000 });

    assert.equal(sent[0].type, 'm.call.invite');
    assert.equal(sent[0].content.call_id, 'c1');
    assert.equal(sent[0].content.party_id, 'p1');
    assert.equal(sent[0].content.version, '1');
    assert.equal(sent[0].content.lifetime, 30000);
    assert.equal(sent[0].content.offer.type, 'offer');
    assert.equal(sent[0].content.offer.sdp, 'v=0...');
  });

  it('sends m.call.answer', async () => {
    const sent = [];
    const mockClient = {
      sendEvent: async (r, t, c) => { sent.push({ type: t, content: JSON.parse(c) }); },
      onRoomEvent: async () => null,
    };

    const sig = new P2PSignaling(mockClient, '!dm:ex', '@me:ex');
    await sig.sendAnswer({ callId: 'c1', partyId: 'p2', sdp: 'v=0...' });

    assert.equal(sent[0].type, 'm.call.answer');
    assert.equal(sent[0].content.answer.type, 'answer');
    assert.equal(sent[0].content.version, '1');
  });

  it('sends m.call.candidates batched', async () => {
    const sent = [];
    const mockClient = {
      sendEvent: async (r, t, c) => { sent.push({ type: t, content: JSON.parse(c) }); },
      onRoomEvent: async () => null,
    };

    const sig = new P2PSignaling(mockClient, '!dm:ex', '@me:ex');
    await sig.sendCandidates({
      callId: 'c1', partyId: 'p1',
      candidates: [{ candidate: 'c1...', sdpMid: '0' }, { candidate: 'c2...', sdpMid: '0' }],
    });

    assert.equal(sent[0].type, 'm.call.candidates');
    assert.equal(sent[0].content.candidates.length, 2);
  });

  it('sends m.call.hangup with reason', async () => {
    const sent = [];
    const mockClient = {
      sendEvent: async (r, t, c) => { sent.push({ type: t, content: JSON.parse(c) }); },
      onRoomEvent: async () => null,
    };

    const sig = new P2PSignaling(mockClient, '!dm:ex', '@me:ex');
    await sig.sendHangup({ callId: 'c1', partyId: 'p1', reason: 'idle_timeout' });

    assert.equal(sent[0].type, 'm.call.hangup');
    assert.equal(sent[0].content.reason, 'idle_timeout');
  });

  it('resolves glare: lower user_id wins', () => {
    const sig = new P2PSignaling(null, '!dm:ex', '@alice:ex');
    assert.equal(sig.resolveGlare('@bob:ex'), 'win');
    assert.equal(sig.resolveGlare('@aaa:ex'), 'lose');
  });
});
