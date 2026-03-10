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

function mockEncrypt(roomId, type, content) {
  return JSON.stringify({ encrypted: true, original: content });
}

function mockDecrypt(ciphertext) {
  const parsed = JSON.parse(ciphertext);
  return parsed.original;
}

function mockSign(nonce) {
  return 'sig_' + nonce;
}

function mockVerifySignature(nonce, signature, deviceId) {
  return signature === 'sig_' + nonce;
}

function mockChannel() {
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
    encryptFn: mockEncrypt,
    decryptFn: mockDecrypt,
    signFn: mockSign,
    verifySignatureFn: mockVerifySignature,
    localDeviceId: 'TESTDEVICE',
    ...overrides,
  });
  return { client, transport };
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
    // Simulate peer verification completing
    transport._setPeerVerified(true);
    await transport.sendEvent('!room:ex', 'org.mxdx.terminal.data', '{"data":"aGk=","encoding":"base64","seq":0}');
    assert.equal(client.sent.length, 0, 'should not send via Matrix');
    assert.equal(channel.p2pSent.length, 1, 'should send via P2P');
    const frame = JSON.parse(channel.p2pSent[0]);
    assert.equal(frame.type, 'encrypted', 'frame must be encrypted');
    assert.ok(frame.ciphertext, 'must have ciphertext');
    assert.equal(transport.status, 'p2p');
    transport.close();
  });

  it('does NOT send terminal data before peer verification', async () => {
    const { client, transport } = createTransport();
    const channel = mockChannel();
    transport.setDataChannel(channel);
    // Do NOT call _setPeerVerified — verification not complete
    await transport.sendEvent('!room:ex', 'org.mxdx.terminal.data', '{"data":"aGk=","encoding":"base64","seq":0}');
    assert.equal(client.sent.length, 1, 'should fall back to Matrix before verification');
    assert.equal(channel.p2pSent.length, 0, 'should not send via P2P');
    transport.close();
  });

  it('delivers and decrypts incoming P2P messages via onRoomEvent', async () => {
    const { client, transport } = createTransport();
    const channel = mockChannel();
    transport.setDataChannel(channel);
    transport._setPeerVerified(true);
    const encrypted = mockEncrypt('!room:ex', 'org.mxdx.terminal.data',
      JSON.stringify({ type: 'org.mxdx.terminal.data', content: { data: 'aGk=', encoding: 'base64', seq: 0 } }));
    channel.simulateMessage(JSON.stringify({
      type: 'encrypted',
      ciphertext: encrypted,
    }));
    const result = await transport.onRoomEvent('!room:ex', 'org.mxdx.terminal.data', 1);
    const parsed = JSON.parse(result);
    assert.equal(parsed.content.seq, 0);
    transport.close();
  });

  it('requeues unacked events to Matrix on channel close', async () => {
    const { client, transport } = createTransport();
    const channel = mockChannel();
    transport.setDataChannel(channel);
    transport._setPeerVerified(true);
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
    transport._setPeerVerified(true);
    const oversized = 'x'.repeat(65 * 1024);  // 65KB
    channel.simulateMessage(oversized);
    // Should not crash, should not deliver
    const result = await transport.onRoomEvent('!room:ex', 'org.mxdx.terminal.data', 0.1);
    assert.equal(result, 'null', 'oversized frame should be dropped');
    transport.close();
  });

  it('tears down channel after idle timeout', async () => {
    const { client, transport } = createTransport({ idleTimeoutMs: 100 });
    const channel = mockChannel();
    transport.setDataChannel(channel);
    transport._setPeerVerified(true);
    assert.equal(transport.status, 'p2p');
    await new Promise(r => setTimeout(r, 200));
    assert.equal(transport.status, 'matrix');
    transport.close();
  });

  it('resets idle timer on send', async () => {
    const { client, transport } = createTransport({ idleTimeoutMs: 150 });
    const channel = mockChannel();
    transport.setDataChannel(channel);
    transport._setPeerVerified(true);
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
    transport._setPeerVerified(true);
    await new Promise(r => setTimeout(r, 100));  // idle hangup
    assert.equal(transport.status, 'matrix');
    // First send triggers reconnect (backoff starts at 10s)
    await transport.sendEvent('!room:ex', 'org.mxdx.terminal.data', '{"data":"aGk=","encoding":"base64","seq":0}');
    assert.equal(reconnectCount, 1);
    // Second send within backoff window should NOT trigger reconnect
    await transport.sendEvent('!room:ex', 'org.mxdx.terminal.data', '{"data":"aGk=","encoding":"base64","seq":1}');
    assert.equal(reconnectCount, 1, 'should respect backoff');
    transport.close();
  });
});
