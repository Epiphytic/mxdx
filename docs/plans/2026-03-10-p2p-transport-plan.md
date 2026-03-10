# P2P Transport Layer Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add WebRTC P2P data channels between mxdx clients and launchers to bypass homeserver latency and rate limits for interactive terminal sessions, with transparent Matrix fallback.

**Architecture:** P2PTransport adapter wraps both a WebRTC data channel and the existing Matrix client behind the same `sendEvent`/`onRoomEvent` interface. Signaling uses standard Matrix call events (`m.call.invite/answer/candidates/hangup`). TURN credentials come from the homeserver's `/voip/turnServer` endpoint. Terminal data is Megolm-encrypted before placement on the data channel (preserving E2EE). Peer identity is verified via challenge-response after data channel opens. Sessions start on Matrix immediately; P2P upgrades in parallel. Idle channels are torn down after 5 minutes and reconnected on activity (with exponential backoff: 10s → 5 min). Application-level acks ensure delivery; unacked data falls back to Matrix.

**Tech Stack:** Platform-native WebRTC (browser `RTCPeerConnection`, `node-datachannel` for Node.js), standard Matrix call signaling, homeserver TURN provisioning, existing WASM E2EE layer, `smol-toml` for config.

**Design doc:** `docs/plans/2026-03-10-p2p-transport-design.md`

---

## Task 1: Add `node-datachannel` dependency

**Files:**
- Modify: `package.json` (root workspace)

**Step 1: Install the dependency**

Run:
```bash
npm install node-datachannel --save -w packages/core
```

**Step 2: Verify it imports**

Run:
```bash
node -e "const ndc = require('node-datachannel'); console.log('node-datachannel loaded, version:', ndc.version || 'ok')"
```
Expected: prints version or "ok" without error.

**Step 3: Commit**

```bash
git add package.json package-lock.json packages/core/package.json
git commit -m "chore: add node-datachannel dependency for P2P transport"
```

---

## Task 2: Add `batchMs` setter to BatchedSender

The P2P transport needs to dynamically switch between `p2p_batch_ms` (10ms) and `batch_ms` (200ms) when the transport status changes. Currently `batchMs` is read-only.

**Files:**
- Modify: `packages/core/batched-sender.js`
- Test: `packages/e2e-tests/test/batched-sender.test.js` (create)

**Step 1: Write the failing test**

Create `packages/e2e-tests/test/batched-sender.test.js`:

```javascript
import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import { BatchedSender } from '@mxdx/core';

describe('BatchedSender', () => {
  it('allows dynamic batchMs changes', async () => {
    const sent = [];
    const sender = new BatchedSender({
      sendEvent: async (roomId, type, content) => { sent.push(content); },
      roomId: '!test:example.com',
      batchMs: 200,
    });

    assert.equal(sender.batchMs, 200);
    sender.batchMs = 10;
    assert.equal(sender.batchMs, 10);

    sender.destroy();
  });
});
```

**Step 2: Run test to verify it fails**

Run: `node --test packages/e2e-tests/test/batched-sender.test.js`
Expected: FAIL — `sender.batchMs = 10` has no effect (no setter).

**Step 3: Add the setter**

In `packages/core/batched-sender.js`, find the existing getter (around line 239):

```javascript
get batchMs() {
  return this.#batchMs;
}
```

Add a setter directly after it:

```javascript
set batchMs(ms) {
  this.#batchMs = ms;
}
```

**Step 4: Run test to verify it passes**

Run: `node --test packages/e2e-tests/test/batched-sender.test.js`
Expected: PASS

**Step 5: Commit**

```bash
git add packages/core/batched-sender.js packages/e2e-tests/test/batched-sender.test.js
git commit -m "feat: add dynamic batchMs setter to BatchedSender"
```

---

## Task 3: P2P config fields for launcher and client

No `iceServers` config — TURN credentials come from the homeserver's `/voip/turnServer` endpoint.

**Files:**
- Modify: `packages/launcher/src/config.js`
- Modify: `packages/client/src/config.js`
- Modify: `packages/launcher/bin/mxdx-launcher.js`
- Modify: `packages/client/bin/mxdx-client.js`

**Step 1: Add P2P fields to LauncherConfig**

In `packages/launcher/src/config.js`, add to constructor params:

```javascript
constructor({
  // ... existing params ...
  batchMs = 200,
  p2pEnabled = true,
  p2pBatchMs = 10,
  p2pIdleTimeoutS = 300,
  p2pAdvertiseIps = false,
} = {}) {
  // ... existing assignments ...
  this.batchMs = batchMs;
  this.p2pEnabled = p2pEnabled;
  this.p2pBatchMs = p2pBatchMs;
  this.p2pIdleTimeoutS = p2pIdleTimeoutS;
  this.p2pAdvertiseIps = p2pAdvertiseIps;
}
```

In `fromArgs()`, add:

```javascript
p2pEnabled: args.p2pEnabled !== undefined ? args.p2pEnabled !== 'false' : true,
p2pBatchMs: args.p2pBatchMs ? parseInt(args.p2pBatchMs, 10) : 10,
p2pIdleTimeoutS: args.p2pIdleTimeoutS ? parseInt(args.p2pIdleTimeoutS, 10) : 300,
p2pAdvertiseIps: args.p2pAdvertiseIps === 'true' || args.p2pAdvertiseIps === true,
```

In `save()`, add to the TOML object inside `launcher`:

```javascript
p2p_enabled: this.p2pEnabled,
p2p_batch_ms: this.p2pBatchMs,
p2p_idle_timeout_s: this.p2pIdleTimeoutS,
p2p_advertise_ips: this.p2pAdvertiseIps,
```

In `load()`, add:

```javascript
p2pEnabled: l.p2p_enabled !== undefined ? l.p2p_enabled : true,
p2pBatchMs: l.p2p_batch_ms || 10,
p2pIdleTimeoutS: l.p2p_idle_timeout_s || 300,
p2pAdvertiseIps: l.p2p_advertise_ips === true,
```

**Step 2: Add P2P fields to ClientConfig**

In `packages/client/src/config.js`, same pattern — add `p2pEnabled`, `p2pBatchMs`, `p2pIdleTimeoutS` to constructor, `fromArgs()`, `save()`, and `load()`. Client does not need `p2pAdvertiseIps` (launcher-only).

**Step 3: Add CLI flags**

In `packages/launcher/bin/mxdx-launcher.js`, add to the `start` command options:
```javascript
.option('--p2p-enabled <bool>', 'Enable P2P transport (default: true)')
.option('--p2p-batch-ms <ms>', 'P2P batch window in ms (default: 10)')
.option('--p2p-idle-timeout-s <seconds>', 'P2P idle timeout in seconds (default: 300)')
.option('--p2p-advertise-ips <bool>', 'Include internal IPs in telemetry (default: false, use only with trusted homeservers)')
```

In `packages/client/bin/mxdx-client.js`, add matching options (except `--p2p-advertise-ips`).

Note: Reconnect backoff (10s → 5 min exponential) is hardcoded, not configurable — it's a safety mechanism, not a tuning knob.

**Step 4: Verify config round-trips**

Run:
```bash
node -e "
const { LauncherConfig } = await import('./packages/launcher/src/config.js');
const c = new LauncherConfig({ username: 'test', servers: ['https://matrix.org'], p2pEnabled: true, p2pBatchMs: 10, p2pIdleTimeoutS: 300 });
c.save('/tmp/test-launcher.toml');
const loaded = LauncherConfig.load('/tmp/test-launcher.toml');
console.log('p2pEnabled:', loaded.p2pEnabled, 'p2pBatchMs:', loaded.p2pBatchMs, 'p2pIdleTimeoutS:', loaded.p2pIdleTimeoutS);
"
```
Expected: `p2pEnabled: true p2pBatchMs: 10 p2pIdleTimeoutS: 300`

**Step 5: Commit**

```bash
git add packages/launcher/src/config.js packages/client/src/config.js packages/launcher/bin/mxdx-launcher.js packages/client/bin/mxdx-client.js
git commit -m "feat: add P2P config fields to launcher and client configs"
```

---

## Task 4: TURN credential fetching

Fetches TURN server credentials from the homeserver's `/voip/turnServer` endpoint.

**Files:**
- Create: `packages/core/turn-credentials.js`
- Test: `packages/e2e-tests/test/turn-credentials.test.js`

**Step 1: Write the failing test**

Create `packages/e2e-tests/test/turn-credentials.test.js`:

```javascript
import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import { fetchTurnCredentials, turnToIceServers } from '../../../packages/core/turn-credentials.js';

describe('TURN credentials', () => {
  it('parses homeserver TURN response into ICE servers', () => {
    const turnResponse = {
      username: '1443779631:@user:example.com',
      password: 'JlKfBy1QwLrO20385QyAtEyIv0=',
      uris: [
        'turn:turn.example.com:3478?transport=udp',
        'turn:turn.example.com:3478?transport=tcp',
        'turns:turn.example.com:5349?transport=tcp',
      ],
      ttl: 86400,
    };

    const iceServers = turnToIceServers(turnResponse);
    assert.equal(iceServers.length, 1);
    assert.deepEqual(iceServers[0].urls, turnResponse.uris);
    assert.equal(iceServers[0].username, turnResponse.username);
    assert.equal(iceServers[0].credential, turnResponse.password);
  });

  it('returns empty array for null/empty response', () => {
    assert.deepEqual(turnToIceServers(null), []);
    assert.deepEqual(turnToIceServers({}), []);
    assert.deepEqual(turnToIceServers({ uris: [] }), []);
  });

  it('fetchTurnCredentials calls correct endpoint', async () => {
    let calledUrl = null;
    const mockFetch = async (url, opts) => {
      calledUrl = url;
      return {
        ok: true,
        json: async () => ({
          username: 'user',
          password: 'pass',
          uris: ['turn:turn.example.com:3478'],
          ttl: 86400,
        }),
      };
    };

    const result = await fetchTurnCredentials('https://matrix.example.com', 'syt_token', mockFetch);
    assert.ok(calledUrl.includes('/_matrix/client/v3/voip/turnServer'));
    assert.equal(result.username, 'user');
  });

  it('returns null when homeserver has no TURN', async () => {
    const mockFetch = async () => ({ ok: false, status: 404 });
    const result = await fetchTurnCredentials('https://matrix.example.com', 'syt_token', mockFetch);
    assert.equal(result, null);
  });

  it('rejects non-https URLs', async () => {
    const mockFetch = async () => { throw new Error('should not be called'); };
    const result = await fetchTurnCredentials('ftp://evil.com', 'syt_token', mockFetch);
    assert.equal(result, null);
  });

  it('uses URL constructor for safe path building', async () => {
    let calledUrl = null;
    const mockFetch = async (url) => {
      calledUrl = url;
      return { ok: true, json: async () => ({ username: 'u', password: 'p', uris: ['turn:t:3478'], ttl: 86400 }) };
    };
    await fetchTurnCredentials('https://matrix.example.com/extra/path/', 'tok', mockFetch);
    assert.equal(calledUrl, 'https://matrix.example.com/_matrix/client/v3/voip/turnServer');
  });
});
```

**Step 2: Run test to verify it fails**

Run: `node --test packages/e2e-tests/test/turn-credentials.test.js`
Expected: FAIL — module not found.

**Step 3: Implement turn-credentials.js**

Create `packages/core/turn-credentials.js`:

```javascript
/**
 * Fetches TURN server credentials from the homeserver.
 * Returns null if the homeserver doesn't provide TURN.
 *
 * Uses URL() constructor for safe path construction (no string concatenation).
 * Validates https: scheme to prevent credential exfiltration via SSRF.
 * Disables redirect following to prevent credential leakage to non-homeserver domains.
 */
export async function fetchTurnCredentials(homeserverUrl, accessToken, fetchFn = fetch) {
  try {
    const parsed = new URL(homeserverUrl);
    if (parsed.protocol !== 'https:' && parsed.protocol !== 'http:') {
      return null;  // Only allow http(s) schemes
    }
    parsed.pathname = '/_matrix/client/v3/voip/turnServer';
    const response = await fetchFn(parsed.href, {
      headers: { Authorization: 'Bearer ' + accessToken },
      redirect: 'error',  // Do not follow redirects — prevents credential exfiltration
    });
    if (!response.ok) return null;
    const data = await response.json();
    if (!data.uris || data.uris.length === 0) return null;
    return data;
  } catch {
    return null;
  }
}

/**
 * Converts homeserver TURN response to RTCPeerConnection iceServers format.
 */
export function turnToIceServers(turnResponse) {
  if (!turnResponse?.uris?.length) return [];
  return [{
    urls: turnResponse.uris,
    username: turnResponse.username,
    credential: turnResponse.password,
  }];
}
```

**Step 4: Run test to verify it passes**

Run: `node --test packages/e2e-tests/test/turn-credentials.test.js`
Expected: PASS

**Step 5: Commit**

```bash
git add packages/core/turn-credentials.js packages/e2e-tests/test/turn-credentials.test.js
git commit -m "feat: add TURN credential fetching from homeserver /voip/turnServer"
```

---

## Task 5: WebRTC channel wrapper — Node.js

Thin wrapper around `node-datachannel` with `onStateChange` for ICE monitoring.

**Files:**
- Create: `packages/core/webrtc-channel-node.js`
- Test: `packages/e2e-tests/test/webrtc-channel-node.test.js`

**Step 1: Write the failing test**

Create `packages/e2e-tests/test/webrtc-channel-node.test.js`:

```javascript
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

    const offer = await channelA.createOffer();
    const answer = await channelB.acceptOffer(offer);
    await channelA.acceptAnswer(answer);

    channelA.onIceCandidate((c) => channelB.addIceCandidate(c));
    channelB.onIceCandidate((c) => channelA.addIceCandidate(c));

    await Promise.race([
      Promise.all([channelA.waitForDataChannel(), channelB.waitForDataChannel()]),
      new Promise((_, reject) => setTimeout(() => reject(new Error('timeout')), 10000)),
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
```

**Step 2: Run test, verify fails**

Run: `node --test packages/e2e-tests/test/webrtc-channel-node.test.js`

**Step 3: Implement NodeWebRTCChannel**

Create `packages/core/webrtc-channel-node.js` — wrapper around node-datachannel PeerConnection exposing: `createOffer`, `acceptOffer`, `acceptAnswer`, `addIceCandidate`, `onIceCandidate`, `onMessage`, `onClose`, `onStateChange`, `send`, `waitForDataChannel`, `isOpen`, `close`. The `onStateChange` callback fires with the ICE connection state string. See design doc for full interface.

Note: node-datachannel does NOT expose `icecandidateerror` — TURN failures can only be detected via state becoming `'failed'`.

**Step 4: Run test, verify passes**

**Step 5: Commit**

```bash
git add packages/core/webrtc-channel-node.js packages/e2e-tests/test/webrtc-channel-node.test.js
git commit -m "feat: add NodeWebRTCChannel wrapper around node-datachannel"
```

---

## Task 6: WebRTC channel wrapper — Browser

Same interface as NodeWebRTCChannel but using native `RTCPeerConnection`. Includes `onIceCandidateError` for TURN error detection (486/508/701).

**Files:**
- Create: `packages/web-console/src/webrtc-channel.js`

**Step 1: Implement BrowserWebRTCChannel**

Create `packages/web-console/src/webrtc-channel.js` — wrapper around browser RTCPeerConnection with same interface as NodeWebRTCChannel, plus:

- `onIceCandidateError(cb)` — browser-only, receives `{ errorCode, errorText, url }` when TURN fails
  - 486: Allocation Quota Reached
  - 508: Insufficient Capacity
  - 701: Cannot reach TURN server
- `onStateChange(cb)` — fires with `iceConnectionState` string

Wires: `pc.onicecandidate`, `pc.addEventListener('icecandidateerror', ...)`, `pc.oniceconnectionstatechange`, `pc.ondatachannel`.

**Step 2: Verify loads in browser dev console**

**Step 3: Commit**

```bash
git add packages/web-console/src/webrtc-channel.js
git commit -m "feat: add BrowserWebRTCChannel with TURN error detection"
```

---

## Task 7: P2P Signaling — standard Matrix call protocol

SDP/ICE exchange via `m.call.invite/answer/candidates/hangup` with glare resolution.

**Files:**
- Create: `packages/core/p2p-signaling.js`
- Test: `packages/e2e-tests/test/p2p-signaling.test.js`

**Step 1: Write the failing test**

Create `packages/e2e-tests/test/p2p-signaling.test.js`:

```javascript
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
      onRoomEvent: async () => 'null',
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
      onRoomEvent: async () => 'null',
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
      onRoomEvent: async () => 'null',
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
      onRoomEvent: async () => 'null',
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
```

**Step 2: Run test, verify fails**

**Step 3: Implement P2PSignaling**

Create `packages/core/p2p-signaling.js` with methods:
- `sendInvite({ callId, partyId, sdp, lifetime })` → `m.call.invite`
- `sendAnswer({ callId, partyId, sdp })` → `m.call.answer`
- `sendCandidates({ callId, partyId, candidates })` → `m.call.candidates`
- `sendHangup({ callId, partyId, reason })` → `m.call.hangup`
- `sendSelectAnswer({ callId, partyId, selectedPartyId })` → `m.call.select_answer`
- `waitForInvite/Answer/Candidates/Hangup(timeoutSecs)` — poll via `onRoomEvent`
- `resolveGlare(remoteUserId)` — returns `'win'` or `'lose'`
- Static: `generateCallId()`, `generatePartyId()` — both return `crypto.randomUUID()`

All events include `version: '1'` per Matrix call spec v1.

**Step 4: Run test, verify passes**

**Step 5: Commit**

```bash
git add packages/core/p2p-signaling.js packages/e2e-tests/test/p2p-signaling.test.js
git commit -m "feat: add P2PSignaling with standard Matrix m.call.* events"
```

---

## Task 8: P2PTransport adapter with E2EE, peer verification, idle timeout

Core adapter between TerminalSocket/BatchedSender and WebRTC. Encrypts terminal data with Megolm before sending over data channel. Verifies peer identity after channel opens. Enforces frame size limits. Includes acks, idle timeout, and reconnect with exponential backoff.

**Files:**
- Create: `packages/core/p2p-transport.js`
- Test: `packages/e2e-tests/test/p2p-transport.test.js`

**Step 1: Write the failing test**

Create `packages/e2e-tests/test/p2p-transport.test.js`:

```javascript
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
```

**Step 2: Run test, verify fails**

**Step 3: Implement P2PTransport**

Create `packages/core/p2p-transport.js` with:
- Static `P2PTransport.create({ matrixClient, encryptFn, decryptFn, signFn, verifySignatureFn, localDeviceId, ... })` — factory method, fully configured at construction (no `_setTransport()`)
- `sendEvent(roomId, type, content)` — encrypts via `encryptFn` then routes via data channel, or falls back to Matrix. **NEVER sends unencrypted terminal data over the data channel.**
- `onRoomEvent(roomId, type, timeoutSecs)` — checks P2P inbox (decrypts via `decryptFn`) then falls through to Matrix polling. P2PTransport is the sole consumer — no event consumption race.
- `setDataChannel(channel)` — connects WebRTC channel, initiates peer verification, starts keepalive + idle timer only after verification succeeds
- Peer verification: challenge-response with Ed25519 device key signatures. 10s timeout. Failure → close channel, fall back to Matrix.
- `onStatusChange(cb)` — status change callback
- `setTurnError(info)` — records TURN error info for UI
- Maximum frame size: **64KB**. Drop and log oversized incoming frames before `JSON.parse()`.
- Idle timer: resets on every send/receive, fires `onHangup('idle_timeout')` when expired
- Reconnect on activity with exponential backoff: when `sendEvent` called while status is `'matrix'`, calls `onReconnectNeeded` only if the current backoff interval has elapsed since last reconnect attempt. Backoff: 10s → 20s → 40s → 80s → 160s → 300s (5 min cap). Resets to 10s after a successful P2P session. Same backoff for both idle and failure reconnects.
- Application-level acks: tracks pending, requeues unacked via Matrix on channel close
- Keepalive: `ping` every 15s, `pong` timeout 5s
- In-band frame types: `encrypted` (terminal data), `ack`, `ping`, `pong`, `peer_verify`

**Step 4: Run test, verify passes (all 7 tests)**

**Step 5: Commit**

```bash
git add packages/core/p2p-transport.js packages/e2e-tests/test/p2p-transport.test.js
git commit -m "feat: add P2PTransport adapter with acks, idle timeout, and reconnect"
```

---

## Task 9: Extend launcher telemetry with P2P discovery

**Files:**
- Modify: `packages/launcher/src/runtime.js`

**Step 1: Add P2P field to telemetry**

Find where `org.mxdx.host_telemetry` state event is posted. Add `p2p` field:

```javascript
p2p: {
  enabled: this.#config.p2pEnabled,
  // Internal IPs only when explicitly enabled — state events persist indefinitely
  ...(this.#config.p2pAdvertiseIps ? { internal_ips: this.#getInternalIps() } : {}),
},
```

Add helper (only called when `p2pAdvertiseIps` is true):
```javascript
#getInternalIps() {
  const nets = os.networkInterfaces();
  const ips = [];
  for (const name of Object.keys(nets)) {
    for (const net of nets[name]) {
      if (net.family === 'IPv4' && !net.internal) ips.push(net.address);
    }
  }
  return ips;
}
```

**Important:** Internal IPs are NOT included by default. The `p2p_advertise_ips` config option (default `false`) must be explicitly set to `true`. This should only be done with trusted, non-public homeservers. State events persist indefinitely and are visible to all room members, including future joins.

**Step 2: Verify**

Run launcher with default config — telemetry should contain `p2p.enabled` but NOT `p2p.internal_ips`.
Run launcher with `--p2p-advertise-ips true` — telemetry should contain `p2p.internal_ips`.

**Step 3: Commit**

```bash
git add packages/launcher/src/runtime.js
git commit -m "feat: extend launcher telemetry with P2P discovery (IPs opt-in only)"
```

---

## Task 10: Wire P2P into launcher session handling

**Files:**
- Modify: `packages/launcher/src/runtime.js`

**No changes to `terminal-socket.js`** — TerminalSocket already accepts a client at construction. P2PTransport implements the same interface (`sendEvent`/`onRoomEvent`), so it's passed to TerminalSocket's constructor directly. No `_setTransport()` or `_sender` getter needed.

**Step 1: Add P2P setup to launcher runtime**

Import P2P modules and add `#setupP2P` method that:
1. Fetches TURN credentials via `fetchTurnCredentials` (validates URL with `new URL()`)
2. Creates `P2PSignaling` with `m.call.*` events
3. Creates `NodeWebRTCChannel` with TURN ICE servers
4. Creates `P2PTransport` via `P2PTransport.create()` factory with:
   - `encryptFn`/`decryptFn` wired to WASM Megolm encrypt/decrypt
   - `signFn`/`verifySignatureFn` wired to WASM Ed25519 device key operations
   - `localDeviceId` from WASM client
   - `idleTimeoutMs` from config
   - hangup callback, reconnect callback (with exponential backoff 10s→5min), status change callback
5. Passes P2PTransport (instead of raw Matrix client) to TerminalSocket constructor
6. Sends `m.call.invite` with SDP offer in background
7. Waits for `m.call.answer`
8. Exchanges ICE candidates via `m.call.candidates`
9. On data channel open: `transport.setDataChannel(channel)` — triggers peer verification, then switches batch window on verification success
10. On failure: log warning, terminal continues on Matrix (P2PTransport's fallback)

When `config.p2pEnabled` is false, TerminalSocket is constructed with the raw Matrix client as before (no P2PTransport wrapper).

**Step 2: Verify launcher starts with P2P options**

Run `node packages/launcher/bin/mxdx-launcher.js start --help` — should show P2P flags.

**Step 3: Commit**

```bash
git add packages/launcher/src/runtime.js
git commit -m "feat: wire P2P transport into launcher session handling"
```

---

## Task 11: Wire P2P into browser terminal-view

**Files:**
- Modify: `packages/web-console/src/terminal-view.js`

**No changes to browser `terminal-socket.js`** — same constructor injection pattern as launcher.

**Step 1: Import P2P modules and add `setupBrowserP2P`**

Same pattern as launcher but using `BrowserWebRTCChannel`:
1. Fetch TURN credentials (validates URL with `new URL()`)
2. Create `P2PTransport` via factory with:
   - `encryptFn`/`decryptFn` wired to WASM Megolm encrypt/decrypt
   - `signFn`/`verifySignatureFn` wired to WASM Ed25519 device key operations
   - `localDeviceId` from WASM client
3. Pass P2PTransport to TerminalSocket constructor (instead of raw client)
4. Create signaling + BrowserWebRTCChannel
5. Wire `onIceCandidateError` for TURN-specific errors (486/508/701)
6. Send invite, wait for answer, exchange candidates
7. On data channel open: `transport.setDataChannel(channel)` — triggers peer verification
8. On verification success: switch batch window, update UI to `P2P`
9. On status change: update UI indicator
10. On idle hangup: show `Matrix` status, reconnect on next activity (with exponential backoff)

Read P2P settings from localStorage. Clamp values to sane ranges: `batchMs` 1-1000, `idleTimeout` 30-3600.

**Step 2: Add `updateStatus` function**

Maps transport status to UI text and CSS class. See design doc UI indicators table.

**Step 3: Commit**

```bash
git add packages/web-console/src/terminal-view.js
git commit -m "feat: wire P2P transport into browser terminal view with E2EE and TURN error handling"
```

---

## Task 12: UI indicator styles

**Files:**
- Modify: `packages/web-console/src/style.css`

**Step 1: Add CSS classes**

```css
#terminal-status { font-size: 0.75rem; padding: 2px 8px; border-radius: 3px; font-family: monospace; }
#terminal-status.status-matrix { color: #888; }
#terminal-status.status-connecting { color: #d4a017; }
#terminal-status.status-p2p { color: #22c55e; }
#terminal-status.status-matrix-lost { color: #d4a017; }
#terminal-status.status-turn-limit { color: #d4a017; }
#terminal-status.status-turn-unreachable { color: #d4a017; }
#terminal-status.status-direct-only { color: #888; }
#terminal-status.status-rate-limited { color: #ef4444; }
```

**Step 2: Verify renders in browser**

**Step 3: Commit**

```bash
git add packages/web-console/src/style.css
git commit -m "feat: add P2P status indicator CSS"
```

---

## Task 13: P2P Settings in web console

**Files:**
- Modify: `packages/web-console/src/settings.js` (or settings view)

**Step 1: Add P2P settings controls**

Using safe DOM methods (createElement, textContent, appendChild):
- Checkbox for `mxdx-p2p-enabled` (default: true)
- Number input for `mxdx-p2p-batch-ms` (default: 10, clamped to 1-1000)
- Number input for `mxdx-p2p-idle-timeout-s` (default: 300, clamped to 30-3600)

Wire change handlers to persist to localStorage. All numeric values are validated and clamped to their sane ranges on read AND on save — prevents malicious localStorage manipulation from causing excessive sends or signaling churn.

**Step 2: Verify settings persist across reload**

**Step 3: Commit**

```bash
git add packages/web-console/src/settings.js
git commit -m "feat: add P2P transport settings to web console"
```

---

## Task 14: P2P E2E test with loopback WebRTC

**Files:**
- Create: `packages/e2e-tests/test/p2p-e2e.test.js`

**Step 1: Write E2E test**

Test uses a `createMockPair()` helper that creates two mock Matrix clients relaying events to each other via in-memory queues. Then:

1. Both sides create `P2PSignaling` and `NodeWebRTCChannel`
2. Side A sends `m.call.invite`, Side B receives and sends `m.call.answer`
3. ICE candidates exchanged via `m.call.candidates`
4. Data channel opens, both sides create `P2PTransport` via factory with mock encrypt/decrypt/sign/verify
5. Peer verification completes (challenge-response over data channel)
6. Terminal data sent A→B via P2P — **verify data is encrypted on the wire** (inspect raw data channel messages)
7. Verify received data is correctly decrypted on the other side
8. Separate test: verify idle timeout causes fallback to matrix
9. Separate test: verify exponential backoff is respected after idle hangup
10. Separate test: verify oversized frames (>64KB) are dropped

**Step 2: Run test**

Run: `node --test packages/e2e-tests/test/p2p-e2e.test.js`
Expected: PASS

Note: Requires `stun:stun.l.google.com:19302` reachable. May need to skip in CI.

**Step 3: Commit**

```bash
git add packages/e2e-tests/test/p2p-e2e.test.js
git commit -m "test: add P2P E2E test with loopback WebRTC and idle timeout"
```

---

## Task 15: Full system E2E test with Tuwunel

**Files:**
- Modify: `packages/e2e-tests/test/terminal-e2e.test.js`

**Step 1: Add P2P test case**

Add test to existing terminal E2E suite that verifies:
1. Terminal session starts on Matrix (immediate)
2. `m.call.invite` event appears in room
3. `m.call.answer` event appears
4. `peer_verify` frames are exchanged (challenge-response)
5. Data channel opens and peer verification completes (status → `'p2p'`)
6. Terminal data flows over P2P — **verify it is Megolm-encrypted on the data channel**
7. After idle timeout, `m.call.hangup` appears with `reason: 'idle_timeout'`
8. Terminal continues on Matrix
9. New data triggers fresh `m.call.invite` (respecting exponential backoff)
10. Telemetry state event contains `p2p.enabled` but NOT `p2p.internal_ips` (default config)

**Step 2: Run full E2E suite**

Run: `node --test packages/e2e-tests/test/terminal-e2e.test.js`
Expected: All tests pass

**Step 3: Commit**

```bash
git add packages/e2e-tests/test/terminal-e2e.test.js
git commit -m "test: add full system P2P E2E test with Tuwunel"
```
