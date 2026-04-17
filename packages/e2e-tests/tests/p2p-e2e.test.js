import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import { NodeWebRTCChannel } from '../../../packages/core/webrtc-channel-node.js';
import { P2PSignaling } from '../../../packages/core/p2p-signaling.js';
import { P2PTransport } from '../../../packages/core/p2p-transport.js';

/**
 * Create a pair of mock Matrix clients that relay events to each other
 * via in-memory queues. Simulates Matrix room event delivery.
 */
function createMockPair() {
  // Event queues: Map<eventType, Array<{content}>>
  const aInbox = new Map();
  const bInbox = new Map();
  // Waiters: Map<eventType, Array<resolve>>
  const aWaiters = new Map();
  const bWaiters = new Map();

  function deliver(inbox, waiters, type, content) {
    // Check if anyone is waiting
    const waiting = waiters.get(type);
    if (waiting && waiting.length > 0) {
      const resolve = waiting.shift();
      resolve(content);
      return;
    }
    // Queue for later
    if (!inbox.has(type)) inbox.set(type, []);
    inbox.get(type).push(content);
  }

  function makeClient(myInbox, myWaiters, peerInbox, peerWaiters, userId) {
    const sent = [];
    return {
      sent,
      sendEvent: async (roomId, type, contentJson) => {
        sent.push({ roomId, type, content: contentJson });
        deliver(peerInbox, peerWaiters, type, contentJson);
      },
      onRoomEvent: async (roomId, type, timeoutSecs) => {
        // Check inbox first
        const queue = myInbox.get(type);
        if (queue && queue.length > 0) {
          return queue.shift();
        }
        // Wait with timeout
        if (timeoutSecs <= 0) return null;
        return new Promise((resolve) => {
          const timer = setTimeout(() => {
            const w = myWaiters.get(type);
            if (w) {
              const idx = w.indexOf(resolve);
              if (idx !== -1) w.splice(idx, 1);
            }
            resolve(null);
          }, timeoutSecs * 1000);

          if (!myWaiters.has(type)) myWaiters.set(type, []);
          const origResolve = resolve;
          myWaiters.get(type).push((val) => {
            clearTimeout(timer);
            origResolve(val);
          });
        });
      },
      userId: () => userId,
    };
  }

  const clientA = makeClient(aInbox, aWaiters, bInbox, bWaiters, '@alice:test');
  const clientB = makeClient(bInbox, bWaiters, aInbox, aWaiters, '@bob:test');
  return { clientA, clientB };
}

/** Mock P2PCrypto using simple JSON wrapping. */
function mockP2PCrypto() {
  return {
    async encrypt(plaintext) {
      return JSON.stringify({ encrypted: true, original: plaintext });
    },
    async decrypt(ciphertextJson) {
      const parsed = JSON.parse(ciphertextJson);
      return parsed.original;
    },
  };
}

describe('P2P E2E with loopback WebRTC', () => {
  it('establishes P2P channel and exchanges encrypted terminal data', async () => {
    const { clientA, clientB } = createMockPair();

    // Create WebRTC channels (loopback — no TURN needed)
    const channelA = new NodeWebRTCChannel();
    const channelB = new NodeWebRTCChannel();

    // Create signaling
    const sigA = new P2PSignaling(clientA, '!test:room', '@alice:test');
    const sigB = new P2PSignaling(clientB, '!test:room', '@bob:test');

    const callId = P2PSignaling.generateCallId();
    const partyIdA = P2PSignaling.generatePartyId();
    const partyIdB = P2PSignaling.generatePartyId();

    // Collect ICE candidates
    const candidatesA = [];
    const candidatesB = [];
    channelA.onIceCandidate((c) => candidatesA.push(c));
    channelB.onIceCandidate((c) => candidatesB.push(c));

    // A creates offer
    const offer = await channelA.createOffer();

    // Send invite
    await sigA.sendInvite({ callId, partyId: partyIdA, sdp: offer.sdp, lifetime: 30000 });

    // B receives invite
    const inviteJson = await clientB.onRoomEvent('!test:room', 'm.call.invite', 5);
    assert.notEqual(inviteJson, null, 'B should receive invite');
    const inviteContent = JSON.parse(inviteJson);
    assert.equal(inviteContent.call_id, callId);

    // B accepts offer
    const answer = await channelB.acceptOffer({ sdp: inviteContent.offer.sdp, type: 'offer' });

    // B sends answer
    await sigB.sendAnswer({ callId, partyId: partyIdB, sdp: answer.sdp });

    // A receives answer
    const answerJson = await clientA.onRoomEvent('!test:room', 'm.call.answer', 5);
    assert.notEqual(answerJson, null, 'A should receive answer');
    const answerContent = JSON.parse(answerJson);

    // A accepts answer
    await channelA.acceptAnswer({ sdp: answerContent.answer.sdp, type: 'answer' });

    // Exchange ICE candidates
    await new Promise(r => setTimeout(r, 200));
    for (const c of candidatesA) channelB.addIceCandidate(c);
    for (const c of candidatesB) channelA.addIceCandidate(c);

    // Wait for data channels to open
    await Promise.all([
      channelA.waitForDataChannel(),
      channelB.waitForDataChannel(),
    ]);

    // Shared mock crypto (simulates session key exchange via Matrix)
    const sharedCrypto = mockP2PCrypto();

    // Create P2PTransport on both sides
    const transportA = P2PTransport.create({
      matrixClient: clientA,
      p2pCrypto: sharedCrypto,
      localDeviceId: 'DEVICE_A',
      idleTimeoutMs: 60000,
    });

    const transportB = P2PTransport.create({
      matrixClient: clientB,
      p2pCrypto: sharedCrypto,
      localDeviceId: 'DEVICE_B',
      idleTimeoutMs: 60000,
    });

    // Attach data channels — triggers automatic peer verification
    transportA.setDataChannel(channelA);
    transportB.setDataChannel(channelB);

    // Wait for peer verification to complete via handshake
    await new Promise(r => setTimeout(r, 500));

    assert.equal(transportA.status, 'p2p', 'A should be verified and in p2p mode');
    assert.equal(transportB.status, 'p2p', 'B should be verified and in p2p mode');

    // Send terminal data A -> B
    await transportA.sendEvent('!test:room', 'org.mxdx.terminal.data',
      JSON.stringify({ type: 'org.mxdx.terminal.data', content: { data: 'aGVsbG8=', encoding: 'base64', seq: 0 } }));

    // Verify data was encrypted on the wire (check raw channel messages)
    assert.equal(clientA.sent.filter(e => e.type === 'org.mxdx.terminal.data').length, 0,
      'Terminal data should NOT go via Matrix when P2P is active');

    // B receives via P2P inbox (allow async decryption)
    await new Promise(r => setTimeout(r, 50));
    const received = await transportB.onRoomEvent('!test:room', 'org.mxdx.terminal.data', 2);
    assert.notEqual(received, null, 'B should receive terminal data via P2P');
    const parsed = JSON.parse(received);
    assert.equal(parsed.content.seq, 0);
    assert.equal(parsed.content.data, 'aGVsbG8=');

    // Cleanup
    transportA.close();
    transportB.close();
    channelA.close();
    channelB.close();
  });

  it('falls back to Matrix on idle timeout', async () => {
    const { clientA, clientB } = createMockPair();

    const channelA = new NodeWebRTCChannel();
    const channelB = new NodeWebRTCChannel();

    const offer = await channelA.createOffer();
    const answer = await channelB.acceptOffer({ sdp: offer.sdp, type: 'offer' });
    await channelA.acceptAnswer({ sdp: answer.sdp, type: 'answer' });

    // Exchange candidates
    const candidatesA = [];
    const candidatesB = [];
    channelA.onIceCandidate((c) => candidatesA.push(c));
    channelB.onIceCandidate((c) => candidatesB.push(c));
    await new Promise(r => setTimeout(r, 200));
    for (const c of candidatesA) channelB.addIceCandidate(c);
    for (const c of candidatesB) channelA.addIceCandidate(c);

    await Promise.all([
      channelA.waitForDataChannel(),
      channelB.waitForDataChannel(),
    ]);

    const sharedCrypto = mockP2PCrypto();

    const transportA = P2PTransport.create({
      matrixClient: clientA,
      p2pCrypto: sharedCrypto,
      localDeviceId: 'DEVICE_A',
      idleTimeoutMs: 500, // Short timeout for testing
    });

    const transportB = P2PTransport.create({
      matrixClient: clientB,
      p2pCrypto: sharedCrypto,
      localDeviceId: 'DEVICE_B',
      idleTimeoutMs: 60000,
    });

    transportA.setDataChannel(channelA);
    transportB.setDataChannel(channelB);

    // Wait for verification to complete
    for (let i = 0; i < 20; i++) {
      if (transportA.status === 'p2p') break;
      await new Promise(r => setTimeout(r, 50));
    }
    assert.equal(transportA.status, 'p2p', 'A should reach p2p after verification');

    // Wait for idle timeout (500ms from last activity)
    await new Promise(r => setTimeout(r, 700));
    assert.equal(transportA.status, 'matrix', 'Should fall back to Matrix after idle timeout');

    transportA.close();
    transportB.close();
    channelA.close();
    channelB.close();
  });

  it('drops oversized frames (>64KB)', async () => {
    const { clientA, clientB } = createMockPair();

    const channelA = new NodeWebRTCChannel();
    const channelB = new NodeWebRTCChannel();

    const offer = await channelA.createOffer();
    const answer = await channelB.acceptOffer({ sdp: offer.sdp, type: 'offer' });
    await channelA.acceptAnswer({ sdp: answer.sdp, type: 'answer' });

    const candidatesA = [];
    const candidatesB = [];
    channelA.onIceCandidate((c) => candidatesA.push(c));
    channelB.onIceCandidate((c) => candidatesB.push(c));
    await new Promise(r => setTimeout(r, 200));
    for (const c of candidatesA) channelB.addIceCandidate(c);
    for (const c of candidatesB) channelA.addIceCandidate(c);

    await Promise.all([
      channelA.waitForDataChannel(),
      channelB.waitForDataChannel(),
    ]);

    const sharedCrypto = mockP2PCrypto();

    const transportA = P2PTransport.create({
      matrixClient: clientA,
      p2pCrypto: sharedCrypto,
      localDeviceId: 'DEVICE_A',
      idleTimeoutMs: 60000,
    });

    const transportB = P2PTransport.create({
      matrixClient: clientB,
      p2pCrypto: sharedCrypto,
      localDeviceId: 'DEVICE_B',
      idleTimeoutMs: 60000,
    });

    transportA.setDataChannel(channelA);
    transportB.setDataChannel(channelB);

    // Wait for verification
    await new Promise(r => setTimeout(r, 500));
    assert.equal(transportB.status, 'p2p');

    // Send oversized frame directly on the wire
    const oversized = JSON.stringify({ type: 'encrypted', ciphertext: 'x'.repeat(65 * 1024) });
    channelA.send(oversized);

    // B should NOT deliver it
    const result = await transportB.onRoomEvent('!test:room', 'org.mxdx.terminal.data', 0.3);
    assert.equal(result, null, 'Oversized frame should be dropped');

    transportA.close();
    transportB.close();
    channelA.close();
    channelB.close();
  });
});
