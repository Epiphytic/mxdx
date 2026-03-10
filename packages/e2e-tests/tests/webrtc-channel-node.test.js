import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import { NodeWebRTCChannel } from '../../../packages/core/webrtc-channel-node.js';

describe('NodeWebRTCChannel', () => {
  it('creates an offer and generates SDP', async () => {
    const channel = new NodeWebRTCChannel({
      iceServers: [{ urls: 'stun:stun.l.google.com:19302' }],
    });
    const offer = await channel.createOffer();
    assert.ok(offer.sdp, 'offer should have sdp');
    assert.equal(offer.type, 'offer');
    channel.close();
  });

  it('exchanges data between two peers', async () => {
    const channelA = new NodeWebRTCChannel({
      iceServers: [{ urls: 'stun:stun.l.google.com:19302' }],
    });
    const channelB = new NodeWebRTCChannel({
      iceServers: [{ urls: 'stun:stun.l.google.com:19302' }],
    });

    // Wire ICE candidates BEFORE SDP exchange to avoid races
    channelA.onIceCandidate((c) => channelB.addIceCandidate(c));
    channelB.onIceCandidate((c) => channelA.addIceCandidate(c));

    const offer = await channelA.createOffer();
    const answer = await channelB.acceptOffer(offer);
    await channelA.acceptAnswer(answer);

    await Promise.race([
      Promise.all([channelA.waitForDataChannel(), channelB.waitForDataChannel()]),
      new Promise((_, reject) => setTimeout(() => reject(new Error('timeout waiting for data channel')), 10000)),
    ]);

    const received = new Promise((resolve) => {
      channelB.onMessage((msg) => resolve(msg));
    });

    channelA.send('hello from A');
    const msg = await received;
    assert.equal(msg, 'hello from A');

    channelA.close();
    channelB.close();
  });
});
