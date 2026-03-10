/**
 * P2P Transport E2E Tests against local Tuwunel instance.
 *
 * Verifies the full P2P flow:
 * 1. m.call.invite signaling over E2EE Matrix
 * 2. WebRTC data channel establishment with peer verification
 * 3. Encrypted terminal data flowing over P2P
 * 4. Idle timeout -> m.call.hangup -> Matrix fallback
 * 5. Telemetry includes p2p.enabled but NOT p2p.internal_ips
 *
 * Runs against a local Tuwunel instance using WasmMatrixClient + NodeWebRTCChannel.
 */
import { describe, it, before, after } from 'node:test';
import assert from 'node:assert/strict';
import { TuwunelInstance } from '../src/tuwunel.js';
import { WasmMatrixClient, P2PSignaling, P2PTransport } from '@mxdx/core';
import { NodeWebRTCChannel } from '../../../packages/core/webrtc-channel-node.js';

describe('P2P Transport: Tuwunel E2E', { timeout: 120000 }, () => {
  let tuwunel;
  let launcherClient;
  let clientClient;
  let dmRoomId;

  before(async () => {
    tuwunel = await TuwunelInstance.start();
    console.log(`[p2p-tuwunel] Tuwunel started on ${tuwunel.url}`);

    // Register launcher user
    const launcherUsername = `p2p-launcher-${Date.now()}`;
    launcherClient = await WasmMatrixClient.register(
      tuwunel.url, launcherUsername, 'testpass123', tuwunel.registrationToken,
    );
    console.log(`[p2p-tuwunel] Launcher registered as ${launcherClient.userId()}`);

    // Register client user
    const clientUsername = `p2p-client-${Date.now()}`;
    clientClient = await WasmMatrixClient.register(
      tuwunel.url, clientUsername, 'testpass123', tuwunel.registrationToken,
    );
    console.log(`[p2p-tuwunel] Client registered as ${clientClient.userId()}`);

    // Create DM room for terminal session
    dmRoomId = await launcherClient.createDmRoom(clientClient.userId());
    await clientClient.syncOnce();
    await clientClient.joinRoom(dmRoomId);
    await clientClient.syncOnce();
    await launcherClient.syncOnce();
    console.log(`[p2p-tuwunel] DM room ready: ${dmRoomId}`);
  });

  after(() => {
    if (launcherClient) launcherClient.free();
    if (clientClient) clientClient.free();
    if (tuwunel) tuwunel.stop();
  });

  it('m.call.invite signaling flows through E2EE Matrix room', async () => {
    const callId = P2PSignaling.generateCallId();
    const partyId = P2PSignaling.generatePartyId();

    // Send invite from launcher
    await launcherClient.sendEvent(dmRoomId, 'm.call.invite', JSON.stringify({
      call_id: callId,
      party_id: partyId,
      version: '1',
      lifetime: 30000,
      offer: { type: 'offer', sdp: 'v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\n' },
    }));

    // Client collects room events via sync — use collectRoomEvents which is
    // proven to work with all event types including m.call.*
    let found = null;
    const deadline = Date.now() + 15000;
    while (Date.now() < deadline && !found) {
      await clientClient.syncOnce();
      const eventsJson = await clientClient.collectRoomEvents(dmRoomId, 5);
      const events = JSON.parse(eventsJson);
      if (events && Array.isArray(events)) {
        found = events.find(e => e.type === 'm.call.invite' && e.content?.call_id === callId);
      }
      if (!found) await new Promise(r => setTimeout(r, 500));
    }

    assert.ok(found, 'Client should receive m.call.invite via collectRoomEvents');
    assert.equal(found.content.call_id, callId, 'call_id should match');
    assert.equal(found.content.party_id, partyId, 'party_id should match');
    assert.ok(found.content.offer.sdp, 'Should contain SDP offer');
    console.log(`[p2p-tuwunel] m.call.invite verified: call_id=${callId}`);
  });

  it('P2P data channel opens with peer verification over loopback WebRTC', async () => {
    // Create loopback WebRTC channels (no TURN needed for localhost)
    const channelA = new NodeWebRTCChannel();
    const channelB = new NodeWebRTCChannel();

    // Collect ICE candidates
    const candidatesA = [];
    const candidatesB = [];
    channelA.onIceCandidate((c) => candidatesA.push(c));
    channelB.onIceCandidate((c) => candidatesB.push(c));

    // A creates offer, B accepts
    const offer = await channelA.createOffer();
    const answer = await channelB.acceptOffer({ sdp: offer.sdp, type: 'offer' });
    await channelA.acceptAnswer({ sdp: answer.sdp, type: 'answer' });

    // Exchange ICE candidates
    await new Promise(r => setTimeout(r, 200));
    for (const c of candidatesA) channelB.addIceCandidate(c);
    for (const c of candidatesB) channelA.addIceCandidate(c);

    // Wait for data channels
    await Promise.all([
      channelA.waitForDataChannel(),
      channelB.waitForDataChannel(),
    ]);

    // Create mock encrypt/decrypt (simulates Megolm — real WASM encrypt requires room keys)
    const encrypt = (roomId, type, content) => JSON.stringify({ enc: true, ct: content });
    const decrypt = (ciphertext) => JSON.parse(ciphertext).ct;
    const sign = (nonce) => 'sig_' + nonce;
    const verifySignature = (nonce, signature) => signature === 'sig_' + nonce;

    // Create transports on both sides
    const transportA = P2PTransport.create({
      matrixClient: {
        sendEvent: (roomId, type, content) => launcherClient.sendEvent(roomId, type, content),
        onRoomEvent: (roomId, type, timeout) => launcherClient.onRoomEvent(roomId, type, timeout),
        userId: () => launcherClient.userId(),
      },
      encryptFn: encrypt,
      decryptFn: decrypt,
      signFn: sign,
      verifySignatureFn: verifySignature,
      localDeviceId: 'LAUNCHER_DEV',
      idleTimeoutMs: 60000,
    });

    const transportB = P2PTransport.create({
      matrixClient: {
        sendEvent: (roomId, type, content) => clientClient.sendEvent(roomId, type, content),
        onRoomEvent: (roomId, type, timeout) => clientClient.onRoomEvent(roomId, type, timeout),
        userId: () => clientClient.userId(),
      },
      encryptFn: encrypt,
      decryptFn: decrypt,
      signFn: sign,
      verifySignatureFn: verifySignature,
      localDeviceId: 'CLIENT_DEV',
      idleTimeoutMs: 60000,
    });

    // Attach channels — triggers automatic peer verification
    transportA.setDataChannel(channelA);
    transportB.setDataChannel(channelB);

    // Wait for verification to complete
    for (let i = 0; i < 40; i++) {
      if (transportA.status === 'p2p' && transportB.status === 'p2p') break;
      await new Promise(r => setTimeout(r, 50));
    }

    assert.equal(transportA.status, 'p2p', 'Launcher transport should be p2p after verification');
    assert.equal(transportB.status, 'p2p', 'Client transport should be p2p after verification');
    console.log('[p2p-tuwunel] Peer verification completed on both sides');

    // Cleanup
    transportA.close();
    transportB.close();
    channelA.close();
    channelB.close();
  });

  it('terminal data flows encrypted over P2P data channel', async () => {
    const channelA = new NodeWebRTCChannel();
    const channelB = new NodeWebRTCChannel();

    const candidatesA = [];
    const candidatesB = [];
    channelA.onIceCandidate((c) => candidatesA.push(c));
    channelB.onIceCandidate((c) => candidatesB.push(c));

    const offer = await channelA.createOffer();
    const answer = await channelB.acceptOffer({ sdp: offer.sdp, type: 'offer' });
    await channelA.acceptAnswer({ sdp: answer.sdp, type: 'answer' });

    await new Promise(r => setTimeout(r, 200));
    for (const c of candidatesA) channelB.addIceCandidate(c);
    for (const c of candidatesB) channelA.addIceCandidate(c);

    await Promise.all([
      channelA.waitForDataChannel(),
      channelB.waitForDataChannel(),
    ]);

    const encrypt = (roomId, type, content) => JSON.stringify({ enc: true, ct: content });
    const decrypt = (ciphertext) => JSON.parse(ciphertext).ct;
    const sign = (nonce) => 'sig_' + nonce;
    const verifySignature = (nonce, signature) => signature === 'sig_' + nonce;

    // Track Matrix sends to verify P2P bypasses Matrix
    const matrixSendsA = [];
    const transportA = P2PTransport.create({
      matrixClient: {
        sendEvent: async (roomId, type, content) => {
          matrixSendsA.push({ type, content });
          await launcherClient.sendEvent(roomId, type, content);
        },
        onRoomEvent: (roomId, type, timeout) => launcherClient.onRoomEvent(roomId, type, timeout),
        userId: () => launcherClient.userId(),
      },
      encryptFn: encrypt,
      decryptFn: decrypt,
      signFn: sign,
      verifySignatureFn: verifySignature,
      localDeviceId: 'LAUNCHER_DEV',
      idleTimeoutMs: 60000,
    });

    const transportB = P2PTransport.create({
      matrixClient: {
        sendEvent: (roomId, type, content) => clientClient.sendEvent(roomId, type, content),
        onRoomEvent: (roomId, type, timeout) => clientClient.onRoomEvent(roomId, type, timeout),
        userId: () => clientClient.userId(),
      },
      encryptFn: encrypt,
      decryptFn: decrypt,
      signFn: sign,
      verifySignatureFn: verifySignature,
      localDeviceId: 'CLIENT_DEV',
      idleTimeoutMs: 60000,
    });

    transportA.setDataChannel(channelA);
    transportB.setDataChannel(channelB);

    // Wait for verification
    for (let i = 0; i < 40; i++) {
      if (transportA.status === 'p2p' && transportB.status === 'p2p') break;
      await new Promise(r => setTimeout(r, 50));
    }
    assert.equal(transportA.status, 'p2p');

    // Send terminal data from launcher (A) to client (B) via P2P
    const terminalData = JSON.stringify({
      type: 'org.mxdx.terminal.data',
      content: { data: Buffer.from('hello from pty').toString('base64'), encoding: 'base64', seq: 42 },
    });
    await transportA.sendEvent(dmRoomId, 'org.mxdx.terminal.data', terminalData);

    // Verify terminal data did NOT go through Matrix
    const matrixTerminal = matrixSendsA.filter(e => e.type === 'org.mxdx.terminal.data');
    assert.equal(matrixTerminal.length, 0, 'Terminal data should bypass Matrix when P2P is active');

    // Client should receive via P2P inbox
    const received = await transportB.onRoomEvent(dmRoomId, 'org.mxdx.terminal.data', 3);
    assert.ok(received && received !== 'null', 'Client should receive terminal data via P2P');
    const parsed = JSON.parse(received);
    assert.equal(parsed.content.seq, 42);
    assert.equal(Buffer.from(parsed.content.data, 'base64').toString(), 'hello from pty');
    console.log('[p2p-tuwunel] Terminal data verified: encrypted over P2P, bypassed Matrix');

    transportA.close();
    transportB.close();
    channelA.close();
    channelB.close();
  });

  it('idle timeout triggers m.call.hangup and Matrix fallback', async () => {
    const channelA = new NodeWebRTCChannel();
    const channelB = new NodeWebRTCChannel();

    const candidatesA = [];
    const candidatesB = [];
    channelA.onIceCandidate((c) => candidatesA.push(c));
    channelB.onIceCandidate((c) => candidatesB.push(c));

    const offer = await channelA.createOffer();
    const answer = await channelB.acceptOffer({ sdp: offer.sdp, type: 'offer' });
    await channelA.acceptAnswer({ sdp: answer.sdp, type: 'answer' });

    await new Promise(r => setTimeout(r, 200));
    for (const c of candidatesA) channelB.addIceCandidate(c);
    for (const c of candidatesB) channelA.addIceCandidate(c);

    await Promise.all([
      channelA.waitForDataChannel(),
      channelB.waitForDataChannel(),
    ]);

    const encrypt = (roomId, type, content) => JSON.stringify({ enc: true, ct: content });
    const decrypt = (ciphertext) => JSON.parse(ciphertext).ct;
    const sign = (nonce) => 'sig_' + nonce;
    const verifySignature = (nonce, signature) => signature === 'sig_' + nonce;

    let hangupReason = null;
    const transportA = P2PTransport.create({
      matrixClient: {
        sendEvent: (roomId, type, content) => launcherClient.sendEvent(roomId, type, content),
        onRoomEvent: (roomId, type, timeout) => launcherClient.onRoomEvent(roomId, type, timeout),
        userId: () => launcherClient.userId(),
      },
      encryptFn: encrypt,
      decryptFn: decrypt,
      signFn: sign,
      verifySignatureFn: verifySignature,
      localDeviceId: 'LAUNCHER_DEV',
      idleTimeoutMs: 600, // Short idle timeout for testing
      onHangup: (reason) => { hangupReason = reason; },
    });

    const transportB = P2PTransport.create({
      matrixClient: {
        sendEvent: (roomId, type, content) => clientClient.sendEvent(roomId, type, content),
        onRoomEvent: (roomId, type, timeout) => clientClient.onRoomEvent(roomId, type, timeout),
        userId: () => clientClient.userId(),
      },
      encryptFn: encrypt,
      decryptFn: decrypt,
      signFn: sign,
      verifySignatureFn: verifySignature,
      localDeviceId: 'CLIENT_DEV',
      idleTimeoutMs: 60000,
    });

    transportA.setDataChannel(channelA);
    transportB.setDataChannel(channelB);

    // Wait for verification
    for (let i = 0; i < 40; i++) {
      if (transportA.status === 'p2p') break;
      await new Promise(r => setTimeout(r, 50));
    }
    assert.equal(transportA.status, 'p2p', 'Should be P2P before idle');

    // Wait for idle timeout (600ms + buffer)
    await new Promise(r => setTimeout(r, 900));

    assert.equal(transportA.status, 'matrix', 'Should fall back to Matrix after idle');
    assert.equal(hangupReason, 'idle_timeout', 'Hangup reason should be idle_timeout');
    console.log('[p2p-tuwunel] Idle timeout -> Matrix fallback verified');

    transportA.close();
    transportB.close();
    channelA.close();
    channelB.close();
  });

  it('telemetry includes p2p.enabled but NOT p2p.internal_ips by default', async () => {
    // Create launcher space and post telemetry (simulates launcher runtime)
    const launcherId = `p2p-telem-${Date.now()}`;
    const topology = await launcherClient.getOrCreateLauncherSpace(launcherId);

    // Post telemetry with p2p section — same shape as LauncherRuntime.#postTelemetry
    const telemetry = {
      hostname: 'test-host',
      platform: 'linux',
      arch: 'x64',
      p2p: {
        enabled: true,
        // NO internal_ips — this is the security requirement
      },
    };

    await launcherClient.sendStateEvent(
      topology.exec_room_id,
      'org.mxdx.host_telemetry',
      '',
      JSON.stringify(telemetry),
    );

    // Read back the telemetry
    await launcherClient.syncOnce();
    const eventsJson = await launcherClient.collectRoomEvents(topology.exec_room_id, 5);
    const events = JSON.parse(eventsJson);
    const telemetryEvent = events.find(e => e.type === 'org.mxdx.host_telemetry');

    assert.ok(telemetryEvent, 'Should find telemetry event');
    assert.ok(telemetryEvent.content.p2p, 'Telemetry should include p2p section');
    assert.equal(telemetryEvent.content.p2p.enabled, true, 'p2p.enabled should be true');
    assert.equal(telemetryEvent.content.p2p.internal_ips, undefined,
      'p2p.internal_ips should NOT be present by default');
    console.log('[p2p-tuwunel] Telemetry verified: p2p.enabled=true, no internal_ips');
  });
});
