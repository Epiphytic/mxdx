import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import { P2PTransport } from '../../../packages/core/p2p-transport.js';

function mockClient() {
  const sent = [];
  return {
    sent,
    sendEvent: async (roomId, type, content) => { sent.push({ roomId, type, content }); },
    onRoomEvent: async () => 'null',
    userId: () => '@test:example.com',
  };
}

/** Mock P2PCrypto that uses simple JSON wrapping (no real AES-GCM). */
function mockP2PCrypto() {
  return {
    async encrypt(plaintext) {
      return JSON.stringify({ mock_encrypted: true, original: plaintext });
    },
    async decrypt(ciphertextJson) {
      const parsed = JSON.parse(ciphertextJson);
      if (!parsed.mock_encrypted) throw new Error('Not mock-encrypted');
      return parsed.original;
    },
  };
}

/**
 * Create a mock channel that auto-completes peer verification.
 * When the transport sends a peer_verify frame, the mock responds
 * with the correct handshake to make verification succeed.
 */
function mockChannel() {
  let messageCallback = null;
  let closeCallback = null;
  const p2pSent = [];
  const channel = {
    p2pSent,
    isOpen: true,
    send: (data) => {
      p2pSent.push(data);
      // Auto-respond to peer verification frames
      try {
        const frame = JSON.parse(data);
        if (frame.type === 'peer_verify' && messageCallback) {
          // 1. Send back a peer_verify (their challenge to us)
          setTimeout(() => {
            messageCallback(JSON.stringify({
              type: 'peer_verify',
              nonce: 'remote_nonce_abc',
              device_id: 'REMOTE_DEVICE',
            }));
          }, 1);
          // 2. Send back a peer_verify_response (acknowledgement of our nonce)
          setTimeout(() => {
            messageCallback(JSON.stringify({
              type: 'peer_verify_response',
              nonce: frame.nonce,
              device_id: 'REMOTE_DEVICE',
            }));
          }, 2);
        }
      } catch { /* not JSON */ }
    },
    onMessage: (cb) => { messageCallback = cb; },
    onClose: (cb) => { closeCallback = cb; },
    close: () => {},
    simulateMessage: (msg) => messageCallback?.(msg),
    simulateClose: () => { closeCallback?.(); },
  };
  return channel;
}

/**
 * Create a mock channel that does NOT auto-verify.
 */
function mockChannelNoVerify() {
  let messageCallback = null;
  let closeCallback = null;
  const p2pSent = [];
  return {
    p2pSent,
    isOpen: true,
    send: (data) => p2pSent.push(data),
    onMessage: (cb) => { messageCallback = cb; },
    onClose: (cb) => { closeCallback = cb; },
    close: () => {},
    simulateMessage: (msg) => messageCallback?.(msg),
    simulateClose: () => { closeCallback?.(); },
  };
}

function createTransport(overrides = {}) {
  const client = mockClient();
  const transport = P2PTransport.create({
    matrixClient: client,
    p2pCrypto: mockP2PCrypto(),
    localDeviceId: 'TESTDEVICE',
    ...overrides,
  });
  return { client, transport };
}

/** Wait for verification to complete (auto-verify mock needs a few ms) */
async function waitForVerification() {
  await new Promise(r => setTimeout(r, 20));
}

describe('P2PTransport', () => {
  it('falls back to Matrix when no data channel exists', async () => {
    const { client, transport } = createTransport();
    assert.equal(transport.status, 'matrix');
    await transport.sendEvent('!room:ex', 'org.mxdx.terminal.data', '{"data":"aGk=","encoding":"base64","seq":0}');
    assert.equal(client.sent.length, 1);
    transport.close();
  });

  it('encrypts terminal data before sending over P2P', async () => {
    const { client, transport } = createTransport();
    const channel = mockChannel();
    transport.setDataChannel(channel);
    await waitForVerification();
    assert.equal(transport.status, 'p2p');
    await transport.sendEvent('!room:ex', 'org.mxdx.terminal.data', '{"data":"aGk=","encoding":"base64","seq":0}');
    assert.equal(client.sent.length, 0, 'should not send via Matrix');
    const encryptedFrames = channel.p2pSent.filter(d => {
      try { return JSON.parse(d).type === 'encrypted'; } catch { return false; }
    });
    assert.equal(encryptedFrames.length, 1, 'should send encrypted frame via P2P');
    const frame = JSON.parse(encryptedFrames[0]);
    assert.equal(frame.type, 'encrypted', 'frame must be encrypted');
    assert.ok(frame.ciphertext, 'must have ciphertext');
    transport.close();
  });

  it('does NOT send terminal data before peer verification', async () => {
    const { client, transport } = createTransport();
    const channel = mockChannelNoVerify();
    transport.setDataChannel(channel);
    await transport.sendEvent('!room:ex', 'org.mxdx.terminal.data', '{"data":"aGk=","encoding":"base64","seq":0}');
    assert.equal(client.sent.length, 1, 'should fall back to Matrix before verification');
    const encryptedFrames = channel.p2pSent.filter(d => {
      try { return JSON.parse(d).type === 'encrypted'; } catch { return false; }
    });
    assert.equal(encryptedFrames.length, 0, 'should not send encrypted data via P2P');
    transport.close();
  });

  it('delivers and decrypts incoming P2P messages via onRoomEvent', async () => {
    const { client, transport } = createTransport();
    const channel = mockChannel();
    transport.setDataChannel(channel);
    await waitForVerification();
    const p2pCrypto = mockP2PCrypto();
    const encrypted = await p2pCrypto.encrypt(
      JSON.stringify({ type: 'org.mxdx.terminal.data', content: { data: 'aGk=', encoding: 'base64', seq: 0 } }));
    channel.simulateMessage(JSON.stringify({
      type: 'encrypted',
      ciphertext: encrypted,
    }));
    // Allow async decryption to complete
    await new Promise(r => setTimeout(r, 10));
    const result = await transport.onRoomEvent('!room:ex', 'org.mxdx.terminal.data', 1);
    const parsed = JSON.parse(result);
    assert.equal(parsed.content.seq, 0);
    transport.close();
  });

  it('requeues unacked events to Matrix on channel close', async () => {
    const { client, transport } = createTransport();
    const channel = mockChannel();
    transport.setDataChannel(channel);
    await waitForVerification();
    await transport.sendEvent('!room:ex', 'org.mxdx.terminal.data', '{"data":"aGk=","encoding":"base64","seq":0}');
    channel.simulateClose();
    await new Promise(r => setTimeout(r, 100));
    assert.ok(client.sent.length >= 1, 'unacked event should be requeued via Matrix');
    assert.equal(transport.status, 'matrix');
    transport.close();
  });

  it('drops oversized frames (>64KB)', async () => {
    const { client, transport } = createTransport();
    const channel = mockChannel();
    transport.setDataChannel(channel);
    await waitForVerification();
    const oversized = 'x'.repeat(65 * 1024);
    channel.simulateMessage(oversized);
    const result = await transport.onRoomEvent('!room:ex', 'org.mxdx.terminal.data', 0.1);
    assert.equal(result, 'null', 'oversized frame should be dropped');
    transport.close();
  });

  it('tears down channel after idle timeout', async () => {
    const { client, transport } = createTransport({ idleTimeoutMs: 100 });
    const channel = mockChannel();
    transport.setDataChannel(channel);
    await waitForVerification();
    assert.equal(transport.status, 'p2p');
    await new Promise(r => setTimeout(r, 200));
    assert.equal(transport.status, 'matrix');
    transport.close();
  });

  it('resets idle timer on send', async () => {
    const { client, transport } = createTransport({ idleTimeoutMs: 150 });
    const channel = mockChannel();
    transport.setDataChannel(channel);
    await waitForVerification();
    await new Promise(r => setTimeout(r, 100));
    await transport.sendEvent('!room:ex', 'org.mxdx.terminal.data', '{"data":"aGk=","encoding":"base64","seq":0}');
    await new Promise(r => setTimeout(r, 100));
    assert.equal(transport.status, 'p2p');
    await new Promise(r => setTimeout(r, 200));
    assert.equal(transport.status, 'matrix');
    transport.close();
  });

  it('enforces exponential backoff on reconnect after idle hangup', async () => {
    let reconnectCount = 0;
    const { client, transport } = createTransport({
      idleTimeoutMs: 50,
      onReconnectNeeded: () => { reconnectCount++; },
    });
    const channel = mockChannel();
    transport.setDataChannel(channel);
    await waitForVerification();
    await new Promise(r => setTimeout(r, 100));
    assert.equal(transport.status, 'matrix');
    await transport.sendEvent('!room:ex', 'org.mxdx.terminal.data', '{"data":"aGk=","encoding":"base64","seq":0}');
    assert.equal(reconnectCount, 1);
    await transport.sendEvent('!room:ex', 'org.mxdx.terminal.data', '{"data":"aGk=","encoding":"base64","seq":1}');
    assert.equal(reconnectCount, 1, 'should respect backoff');
    transport.close();
  });

  it('completes peer verification via handshake protocol', async () => {
    const { client, transport } = createTransport();
    const channel = mockChannel();
    assert.equal(transport.status, 'matrix');
    transport.setDataChannel(channel);
    await waitForVerification();
    assert.equal(transport.status, 'p2p', 'should transition to p2p after verification');
    const verifyFrames = channel.p2pSent.filter(d => {
      try { return JSON.parse(d).type === 'peer_verify'; } catch { return false; }
    });
    assert.ok(verifyFrames.length >= 1, 'should have sent peer_verify challenge');
    transport.close();
  });

  it('drops encrypted frames with wrong session key', async () => {
    const { client, transport } = createTransport();
    const channel = mockChannel();
    transport.setDataChannel(channel);
    await waitForVerification();
    // Send a frame encrypted with a different "key" (not mock-encrypted format)
    channel.simulateMessage(JSON.stringify({
      type: 'encrypted',
      ciphertext: JSON.stringify({ wrong_format: true, data: 'hello' }),
    }));
    await new Promise(r => setTimeout(r, 10));
    const result = await transport.onRoomEvent('!room:ex', 'org.mxdx.terminal.data', 0.1);
    assert.equal(result, 'null', 'frame with wrong key should be dropped');
    transport.close();
  });
});
