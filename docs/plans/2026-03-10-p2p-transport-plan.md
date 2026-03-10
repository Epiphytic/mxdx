# P2P Transport Layer Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add WebRTC P2P data channels between mxdx clients and launchers to bypass homeserver latency and rate limits for interactive terminal sessions, with transparent Matrix fallback.

**Architecture:** P2PTransport adapter wraps both a WebRTC data channel and the existing Matrix client behind the same `sendEvent`/`onRoomEvent` interface. Signaling (SDP/ICE) flows through Matrix exec room events. Sessions start on Matrix immediately; P2P upgrades in parallel. Application-level acks ensure delivery; unacked data falls back to Matrix.

**Tech Stack:** Platform-native WebRTC (browser `RTCPeerConnection`, `node-datachannel` for Node.js), Matrix signaling events, existing WASM E2EE layer, `smol-toml` for config.

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
  iceServers = ['stun:stun.l.google.com:19302'],
} = {}) {
  // ... existing assignments ...
  this.batchMs = batchMs;
  this.p2pEnabled = p2pEnabled;
  this.p2pBatchMs = p2pBatchMs;
  this.iceServers = iceServers;
}
```

In `fromArgs()`, add:

```javascript
p2pEnabled: args.p2pEnabled !== undefined ? args.p2pEnabled !== 'false' : true,
p2pBatchMs: args.p2pBatchMs ? parseInt(args.p2pBatchMs, 10) : 10,
iceServers: args.iceServers ? args.iceServers.split(',') : ['stun:stun.l.google.com:19302'],
```

In `save()`, add to the TOML object inside `launcher`:

```javascript
p2p_enabled: this.p2pEnabled,
p2p_batch_ms: this.p2pBatchMs,
ice_servers: this.iceServers,
```

In `load()`, add:

```javascript
p2pEnabled: l.p2p_enabled !== undefined ? l.p2p_enabled : true,
p2pBatchMs: l.p2p_batch_ms || 10,
iceServers: l.ice_servers || ['stun:stun.l.google.com:19302'],
```

**Step 2: Add P2P fields to ClientConfig**

In `packages/client/src/config.js`, same pattern:

Constructor:
```javascript
constructor({ username, server, password, registrationToken, batchMs = 200, p2pEnabled = true, p2pBatchMs = 10 } = {}) {
  // ... existing ...
  this.p2pEnabled = p2pEnabled;
  this.p2pBatchMs = p2pBatchMs;
}
```

`fromArgs()`:
```javascript
p2pEnabled: args.p2pEnabled !== undefined ? args.p2pEnabled !== 'false' : true,
p2pBatchMs: args.p2pBatchMs ? parseInt(args.p2pBatchMs, 10) : 10,
```

`save()` — add to `client` object:
```javascript
p2p_enabled: this.p2pEnabled,
p2p_batch_ms: this.p2pBatchMs,
```

`load()`:
```javascript
p2pEnabled: c.p2p_enabled !== undefined ? c.p2p_enabled : true,
p2pBatchMs: c.p2p_batch_ms || 10,
```

**Step 3: Add CLI flags**

In `packages/launcher/bin/mxdx-launcher.js`, add to the `start` command options:
```javascript
.option('--p2p-enabled <bool>', 'Enable P2P transport (default: true)')
.option('--p2p-batch-ms <ms>', 'P2P batch window in ms (default: 10)')
.option('--ice-servers <urls>', 'Comma-separated ICE server URLs')
```

In `packages/client/bin/mxdx-client.js`, add to global options:
```javascript
.option('--p2p-enabled <bool>', 'Enable P2P transport (default: true)')
.option('--p2p-batch-ms <ms>', 'P2P batch window in ms (default: 10)')
```

**Step 4: Verify config round-trips**

Run:
```bash
node -e "
const { LauncherConfig } = await import('./packages/launcher/src/config.js');
const c = new LauncherConfig({ username: 'test', servers: ['https://matrix.org'], p2pEnabled: true, p2pBatchMs: 10, iceServers: ['stun:stun.l.google.com:19302'] });
c.save('/tmp/test-launcher.toml');
const loaded = LauncherConfig.load('/tmp/test-launcher.toml');
console.log('p2pEnabled:', loaded.p2pEnabled, 'p2pBatchMs:', loaded.p2pBatchMs, 'iceServers:', loaded.iceServers);
"
```
Expected: `p2pEnabled: true p2pBatchMs: 10 iceServers: [ 'stun:stun.l.google.com:19302' ]`

**Step 5: Commit**

```bash
git add packages/launcher/src/config.js packages/client/src/config.js packages/launcher/bin/mxdx-launcher.js packages/client/bin/mxdx-client.js
git commit -m "feat: add P2P config fields to launcher and client configs"
```

---

## Task 4: WebRTC channel wrapper — Node.js

A thin wrapper around `node-datachannel` that exposes a consistent interface for P2PTransport.

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

    // A creates offer
    const offer = await channelA.createOffer();

    // B accepts offer, creates answer
    const answer = await channelB.acceptOffer(offer);

    // A accepts answer
    await channelA.acceptAnswer(answer);

    // Exchange ICE candidates
    channelA.onIceCandidate((candidate) => {
      channelB.addIceCandidate(candidate);
    });
    channelB.onIceCandidate((candidate) => {
      channelA.addIceCandidate(candidate);
    });

    // Wait for connection
    const connected = await Promise.race([
      Promise.all([channelA.waitForDataChannel(), channelB.waitForDataChannel()]),
      new Promise((_, reject) => setTimeout(() => reject(new Error('timeout')), 10000)),
    ]);

    // Send data A -> B
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

**Step 2: Run test to verify it fails**

Run: `node --test packages/e2e-tests/test/webrtc-channel-node.test.js`
Expected: FAIL — module not found.

**Step 3: Implement NodeWebRTCChannel**

Create `packages/core/webrtc-channel-node.js`:

```javascript
import nodeDatachannel from 'node-datachannel';

const { PeerConnection } = nodeDatachannel;

/**
 * Thin wrapper around node-datachannel for Node.js environments.
 * Exposes a consistent interface that P2PTransport consumes.
 */
export class NodeWebRTCChannel {
  #pc;
  #dc = null;
  #iceCandidateCallbacks = [];
  #messageCallbacks = [];
  #openResolvers = [];
  #closeCallbacks = [];
  #open = false;

  constructor({ iceServers = [] } = {}) {
    const config = {
      iceServers: iceServers.map(s => typeof s === 'string' ? s : s.urls),
    };
    this.#pc = new PeerConnection('mxdx-p2p', config);

    this.#pc.onLocalCandidate((candidate, mid) => {
      for (const cb of this.#iceCandidateCallbacks) {
        cb({ candidate, mid });
      }
    });
  }

  async createOffer() {
    this.#dc = this.#pc.createDataChannel('mxdx-terminal');
    this.#wireDataChannel(this.#dc);

    const sdp = await new Promise((resolve) => {
      this.#pc.onLocalDescription((sdp, type) => resolve({ sdp, type }));
    });

    return sdp;
  }

  async acceptOffer(offer) {
    this.#pc.onDataChannel((dc) => {
      this.#dc = dc;
      this.#wireDataChannel(dc);
    });

    this.#pc.setRemoteDescription(offer.sdp, offer.type);

    const answer = await new Promise((resolve) => {
      this.#pc.onLocalDescription((sdp, type) => resolve({ sdp, type }));
    });

    return answer;
  }

  async acceptAnswer(answer) {
    this.#pc.setRemoteDescription(answer.sdp, answer.type);
  }

  addIceCandidate(candidate) {
    this.#pc.addRemoteCandidate(candidate.candidate, candidate.mid);
  }

  onIceCandidate(cb) {
    this.#iceCandidateCallbacks.push(cb);
  }

  onMessage(cb) {
    this.#messageCallbacks.push(cb);
  }

  onClose(cb) {
    this.#closeCallbacks.push(cb);
  }

  send(data) {
    if (!this.#dc || !this.#open) throw new Error('Data channel not open');
    this.#dc.sendMessage(data);
  }

  waitForDataChannel() {
    if (this.#open) return Promise.resolve();
    return new Promise((resolve) => {
      this.#openResolvers.push(resolve);
    });
  }

  get isOpen() {
    return this.#open;
  }

  close() {
    if (this.#dc) {
      try { this.#dc.close(); } catch { /* best effort */ }
      this.#dc = null;
    }
    if (this.#pc) {
      try { this.#pc.close(); } catch { /* best effort */ }
      this.#pc = null;
    }
    this.#open = false;
  }

  #wireDataChannel(dc) {
    dc.onOpen(() => {
      this.#open = true;
      for (const resolve of this.#openResolvers) resolve();
      this.#openResolvers = [];
    });

    dc.onMessage((msg) => {
      for (const cb of this.#messageCallbacks) cb(msg);
    });

    dc.onClosed(() => {
      this.#open = false;
      for (const cb of this.#closeCallbacks) cb();
    });
  }
}
```

**Step 4: Run test to verify it passes**

Run: `node --test packages/e2e-tests/test/webrtc-channel-node.test.js`
Expected: PASS (both tests)

Note: The loopback peer test may need adjustment if `node-datachannel` API differs slightly. Consult `node-datachannel` docs and adjust constructor/method calls as needed. The interface contract (`createOffer`, `acceptOffer`, `acceptAnswer`, `addIceCandidate`, `onIceCandidate`, `onMessage`, `send`, `waitForDataChannel`, `close`, `isOpen`) must be preserved.

**Step 5: Commit**

```bash
git add packages/core/webrtc-channel-node.js packages/e2e-tests/test/webrtc-channel-node.test.js
git commit -m "feat: add NodeWebRTCChannel wrapper around node-datachannel"
```

---

## Task 5: WebRTC channel wrapper — Browser

Same interface as NodeWebRTCChannel but using native `RTCPeerConnection`.

**Files:**
- Create: `packages/web-console/src/webrtc-channel.js`

**Step 1: Implement BrowserWebRTCChannel**

Create `packages/web-console/src/webrtc-channel.js`:

```javascript
/**
 * Thin wrapper around browser RTCPeerConnection.
 * Same interface as NodeWebRTCChannel.
 */
export class BrowserWebRTCChannel {
  #pc;
  #dc = null;
  #iceCandidateCallbacks = [];
  #messageCallbacks = [];
  #openResolvers = [];
  #closeCallbacks = [];
  #open = false;

  constructor({ iceServers = [] } = {}) {
    const config = {
      iceServers: iceServers.map(s =>
        typeof s === 'string' ? { urls: s } : s
      ),
    };
    this.#pc = new RTCPeerConnection(config);

    this.#pc.onicecandidate = (event) => {
      if (event.candidate) {
        for (const cb of this.#iceCandidateCallbacks) {
          cb({
            candidate: event.candidate.candidate,
            mid: event.candidate.sdpMid,
          });
        }
      }
    };
  }

  async createOffer() {
    this.#dc = this.#pc.createDataChannel('mxdx-terminal', {
      ordered: true,
    });
    this.#wireDataChannel(this.#dc);

    const offer = await this.#pc.createOffer();
    await this.#pc.setLocalDescription(offer);
    return { sdp: offer.sdp, type: offer.type };
  }

  async acceptOffer(offer) {
    this.#pc.ondatachannel = (event) => {
      this.#dc = event.channel;
      this.#wireDataChannel(this.#dc);
    };

    await this.#pc.setRemoteDescription(new RTCSessionDescription({
      sdp: offer.sdp,
      type: offer.type,
    }));

    const answer = await this.#pc.createAnswer();
    await this.#pc.setLocalDescription(answer);
    return { sdp: answer.sdp, type: answer.type };
  }

  async acceptAnswer(answer) {
    await this.#pc.setRemoteDescription(new RTCSessionDescription({
      sdp: answer.sdp,
      type: answer.type,
    }));
  }

  addIceCandidate(candidate) {
    this.#pc.addIceCandidate(new RTCIceCandidate({
      candidate: candidate.candidate,
      sdpMid: candidate.mid,
    })).catch(() => { /* best effort */ });
  }

  onIceCandidate(cb) {
    this.#iceCandidateCallbacks.push(cb);
  }

  onMessage(cb) {
    this.#messageCallbacks.push(cb);
  }

  onClose(cb) {
    this.#closeCallbacks.push(cb);
  }

  send(data) {
    if (!this.#dc || !this.#open) throw new Error('Data channel not open');
    this.#dc.send(data);
  }

  waitForDataChannel() {
    if (this.#open) return Promise.resolve();
    return new Promise((resolve) => {
      this.#openResolvers.push(resolve);
    });
  }

  get isOpen() {
    return this.#open;
  }

  close() {
    if (this.#dc) {
      try { this.#dc.close(); } catch { /* best effort */ }
      this.#dc = null;
    }
    if (this.#pc) {
      try { this.#pc.close(); } catch { /* best effort */ }
      this.#pc = null;
    }
    this.#open = false;
  }

  #wireDataChannel(dc) {
    dc.onopen = () => {
      this.#open = true;
      for (const resolve of this.#openResolvers) resolve();
      this.#openResolvers = [];
    };

    dc.onmessage = (event) => {
      for (const cb of this.#messageCallbacks) cb(event.data);
    };

    dc.onclose = () => {
      this.#open = false;
      for (const cb of this.#closeCallbacks) cb();
    };
  }
}
```

**Step 2: Verify it loads in browser**

Start vite dev server and open browser console:
```bash
cd packages/web-console && npx vite --port 5173
```
In browser console:
```javascript
import('/src/webrtc-channel.js').then(m => console.log('BrowserWebRTCChannel:', typeof m.BrowserWebRTCChannel))
```
Expected: `BrowserWebRTCChannel: function`

**Step 3: Commit**

```bash
git add packages/web-console/src/webrtc-channel.js
git commit -m "feat: add BrowserWebRTCChannel wrapper for browser RTCPeerConnection"
```

---

## Task 6: P2P Signaling module

Handles SDP offer/answer exchange and ICE candidate trickle via Matrix events.

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
  it('sends offer via Matrix event', async () => {
    const sent = [];
    const mockClient = {
      sendEvent: async (roomId, type, content) => {
        sent.push({ roomId, type, content: JSON.parse(content) });
      },
      onRoomEvent: async () => 'null',
    };

    const signaling = new P2PSignaling(mockClient, '!exec:example.com');
    await signaling.sendOffer('req-1', 'client', { sdp: 'v=0...', type: 'offer' });

    assert.equal(sent.length, 1);
    assert.equal(sent[0].type, 'org.mxdx.p2p.offer');
    assert.equal(sent[0].content.role, 'client');
    assert.equal(sent[0].content.request_id, 'req-1');
    assert.ok(sent[0].content.sdp);
  });

  it('sends answer via Matrix event', async () => {
    const sent = [];
    const mockClient = {
      sendEvent: async (roomId, type, content) => {
        sent.push({ roomId, type, content: JSON.parse(content) });
      },
      onRoomEvent: async () => 'null',
    };

    const signaling = new P2PSignaling(mockClient, '!exec:example.com');
    await signaling.sendAnswer('req-1', 'launcher', { sdp: 'v=0...', type: 'answer' });

    assert.equal(sent.length, 1);
    assert.equal(sent[0].type, 'org.mxdx.p2p.answer');
    assert.equal(sent[0].content.role, 'launcher');
  });

  it('sends ICE candidate via Matrix event', async () => {
    const sent = [];
    const mockClient = {
      sendEvent: async (roomId, type, content) => {
        sent.push({ roomId, type, content: JSON.parse(content) });
      },
      onRoomEvent: async () => 'null',
    };

    const signaling = new P2PSignaling(mockClient, '!exec:example.com');
    await signaling.sendIceCandidate('req-1', 'client', { candidate: 'a=candidate...', mid: '0' });

    assert.equal(sent.length, 1);
    assert.equal(sent[0].type, 'org.mxdx.p2p.ice');
    assert.equal(sent[0].content.candidate.candidate, 'a=candidate...');
  });
});
```

**Step 2: Run test to verify it fails**

Run: `node --test packages/e2e-tests/test/p2p-signaling.test.js`
Expected: FAIL — module not found.

**Step 3: Implement P2PSignaling**

Create `packages/core/p2p-signaling.js`:

```javascript
/**
 * P2PSignaling — exchanges WebRTC SDP/ICE via Matrix events.
 *
 * Uses the exec room for signaling. Both peers send offers simultaneously
 * (bidirectional handshake). First data channel to open wins.
 */
export class P2PSignaling {
  #client;
  #roomId;
  #handlers = new Map(); // type -> [callbacks]

  constructor(client, execRoomId) {
    this.#client = client;
    this.#roomId = execRoomId;
  }

  async sendOffer(requestId, role, sdpObj) {
    await this.#client.sendEvent(
      this.#roomId,
      'org.mxdx.p2p.offer',
      JSON.stringify({
        request_id: requestId,
        role,
        sdp: sdpObj.sdp,
        type: sdpObj.type,
      }),
    );
  }

  async sendAnswer(requestId, role, sdpObj) {
    await this.#client.sendEvent(
      this.#roomId,
      'org.mxdx.p2p.answer',
      JSON.stringify({
        request_id: requestId,
        role,
        sdp: sdpObj.sdp,
        type: sdpObj.type,
      }),
    );
  }

  async sendIceCandidate(requestId, role, candidate) {
    await this.#client.sendEvent(
      this.#roomId,
      'org.mxdx.p2p.ice',
      JSON.stringify({
        request_id: requestId,
        role,
        candidate,
      }),
    );
  }

  /**
   * Poll for a specific P2P signaling event type.
   * Returns parsed content or null on timeout.
   */
  async waitForEvent(eventType, timeoutSecs = 5) {
    try {
      const json = await this.#client.onRoomEvent(
        this.#roomId,
        eventType,
        timeoutSecs,
      );
      if (!json || json === 'null') return null;
      const event = JSON.parse(json);
      return event.content || event;
    } catch {
      return null;
    }
  }

  async waitForOffer(timeoutSecs = 5) {
    return this.waitForEvent('org.mxdx.p2p.offer', timeoutSecs);
  }

  async waitForAnswer(timeoutSecs = 5) {
    return this.waitForEvent('org.mxdx.p2p.answer', timeoutSecs);
  }

  async waitForIceCandidate(timeoutSecs = 1) {
    return this.waitForEvent('org.mxdx.p2p.ice', timeoutSecs);
  }
}
```

**Step 4: Run test to verify it passes**

Run: `node --test packages/e2e-tests/test/p2p-signaling.test.js`
Expected: PASS

**Step 5: Commit**

```bash
git add packages/core/p2p-signaling.js packages/e2e-tests/test/p2p-signaling.test.js
git commit -m "feat: add P2PSignaling module for SDP/ICE exchange via Matrix"
```

---

## Task 7: P2PTransport adapter

The core adapter that sits between TerminalSocket/BatchedSender and the WebRTC data channel, with transparent Matrix fallback.

**Files:**
- Create: `packages/core/p2p-transport.js`
- Test: `packages/e2e-tests/test/p2p-transport.test.js`

**Step 1: Write the failing test**

Create `packages/e2e-tests/test/p2p-transport.test.js`:

```javascript
import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import { P2PTransport } from '../../../packages/core/p2p-transport.js';

describe('P2PTransport', () => {
  it('falls back to Matrix when no data channel exists', async () => {
    const sent = [];
    const mockClient = {
      sendEvent: async (roomId, type, content) => {
        sent.push({ roomId, type, content });
      },
      onRoomEvent: async (roomId, type, timeout) => {
        return 'null';
      },
      userId: () => '@test:example.com',
    };

    const transport = new P2PTransport(mockClient);
    assert.equal(transport.status, 'matrix');

    await transport.sendEvent('!room:ex', 'org.mxdx.terminal.data', '{"data":"aGk=","encoding":"base64","seq":0}');
    assert.equal(sent.length, 1);
    assert.equal(sent[0].roomId, '!room:ex');

    transport.close();
  });

  it('routes through data channel when P2P is active', async () => {
    const matrixSent = [];
    const p2pSent = [];

    const mockClient = {
      sendEvent: async (roomId, type, content) => {
        matrixSent.push({ roomId, type, content });
      },
      onRoomEvent: async () => 'null',
      userId: () => '@test:example.com',
    };

    const transport = new P2PTransport(mockClient);

    // Simulate a connected data channel
    const mockChannel = {
      isOpen: true,
      send: (data) => p2pSent.push(data),
      onMessage: () => {},
      onClose: () => {},
      close: () => {},
    };
    transport._setDataChannel(mockChannel);

    await transport.sendEvent('!room:ex', 'org.mxdx.terminal.data', '{"data":"aGk=","encoding":"base64","seq":0}');

    assert.equal(matrixSent.length, 0, 'should not send via Matrix');
    assert.equal(p2pSent.length, 1, 'should send via P2P');
    assert.equal(transport.status, 'p2p');

    transport.close();
  });

  it('delivers incoming P2P messages via onRoomEvent', async () => {
    const mockClient = {
      sendEvent: async () => {},
      onRoomEvent: async () => 'null',
      userId: () => '@test:example.com',
    };

    const transport = new P2PTransport(mockClient);

    let messageCallback = null;
    const mockChannel = {
      isOpen: true,
      send: () => {},
      onMessage: (cb) => { messageCallback = cb; },
      onClose: () => {},
      close: () => {},
    };
    transport._setDataChannel(mockChannel);

    // Simulate incoming P2P message
    const incoming = JSON.stringify({
      type: 'org.mxdx.terminal.data',
      content: { data: 'aGk=', encoding: 'base64', seq: 0 },
    });
    messageCallback(incoming);

    // onRoomEvent should return it
    const result = await transport.onRoomEvent('!room:ex', 'org.mxdx.terminal.data', 1);
    assert.ok(result);
    const parsed = JSON.parse(result);
    assert.equal(parsed.content.seq, 0);

    transport.close();
  });

  it('requeues unacked events to Matrix on channel close', async () => {
    const matrixSent = [];
    const mockClient = {
      sendEvent: async (roomId, type, content) => {
        matrixSent.push({ roomId, type, content });
      },
      onRoomEvent: async () => 'null',
      userId: () => '@test:example.com',
    };

    const transport = new P2PTransport(mockClient);

    let closeCallback = null;
    const mockChannel = {
      isOpen: true,
      send: () => {},
      onMessage: () => {},
      onClose: (cb) => { closeCallback = cb; },
      close: () => {},
    };
    transport._setDataChannel(mockChannel);

    // Send an event over P2P (unacked)
    await transport.sendEvent('!room:ex', 'org.mxdx.terminal.data', '{"data":"aGk=","encoding":"base64","seq":0}');

    // Simulate channel dropping
    closeCallback();

    // Give requeue time to process
    await new Promise(r => setTimeout(r, 100));

    // The unacked event should have been resent via Matrix
    assert.ok(matrixSent.length >= 1, 'unacked event should be requeued via Matrix');
    assert.equal(transport.status, 'matrix');

    transport.close();
  });
});
```

**Step 2: Run test to verify it fails**

Run: `node --test packages/e2e-tests/test/p2p-transport.test.js`
Expected: FAIL — module not found.

**Step 3: Implement P2PTransport**

Create `packages/core/p2p-transport.js`:

```javascript
/**
 * P2PTransport — adapter between TerminalSocket/BatchedSender and WebRTC.
 *
 * Implements the same sendEvent/onRoomEvent interface the existing code uses.
 * Routes events over WebRTC data channel when available, falls back to Matrix.
 * Application-level acks ensure delivery; unacked data falls back to Matrix.
 */
export class P2PTransport {
  #matrixClient;
  #dataChannel = null;
  #status = 'matrix'; // 'matrix' | 'p2p'
  #p2pInbox = new Map(); // type -> [{ resolve, timer }]
  #p2pQueue = new Map(); // type -> [eventJson]
  #pendingAcks = new Map(); // seq -> { roomId, type, content, timer }
  #statusCallbacks = [];
  #ackTimeout = 2000;
  #pingInterval = null;
  #pongTimer = null;
  #closed = false;

  constructor(matrixClient) {
    this.#matrixClient = matrixClient;
  }

  get status() {
    return this.#status;
  }

  onStatusChange(cb) {
    this.#statusCallbacks.push(cb);
    return () => {
      const idx = this.#statusCallbacks.indexOf(cb);
      if (idx >= 0) this.#statusCallbacks.splice(idx, 1);
    };
  }

  #setStatus(status) {
    if (this.#status === status) return;
    this.#status = status;
    for (const cb of this.#statusCallbacks) cb(status);
  }

  /**
   * Set the active WebRTC data channel. Called when P2P handshake succeeds.
   * Also used by tests (as _setDataChannel).
   */
  _setDataChannel(channel) {
    this.#dataChannel = channel;
    this.#setStatus('p2p');

    channel.onMessage((msg) => {
      this.#handleP2PMessage(msg);
    });

    channel.onClose(() => {
      this.#handleChannelClose();
    });

    this.#startKeepalive();
  }

  async sendEvent(roomId, type, content) {
    if (this.#closed) return;

    if (this.#dataChannel?.isOpen && this.#status === 'p2p') {
      // Send over P2P
      const frame = JSON.stringify({ type, content: JSON.parse(content) });
      try {
        this.#dataChannel.send(frame);
      } catch {
        // Channel broken, fall back
        this.#handleChannelClose();
        await this.#matrixClient.sendEvent(roomId, type, content);
        return;
      }

      // Track for ack (only terminal data events, which have seq)
      const parsed = JSON.parse(content);
      if (typeof parsed.seq === 'number') {
        const timer = setTimeout(() => {
          this.#requeueUnacked(parsed.seq);
        }, this.#ackTimeout);
        this.#pendingAcks.set(parsed.seq, { roomId, type, content, timer });
      }
    } else {
      // Fall back to Matrix
      await this.#matrixClient.sendEvent(roomId, type, content);
    }
  }

  async onRoomEvent(roomId, type, timeoutSecs) {
    if (this.#closed) return 'null';

    // Check P2P queue first
    const queue = this.#p2pQueue.get(type);
    if (queue && queue.length > 0) {
      return queue.shift();
    }

    if (this.#status === 'p2p') {
      // Wait for P2P message with timeout
      return new Promise((resolve) => {
        const timer = setTimeout(() => {
          // Remove waiter and try Matrix as fallback
          const waiters = this.#p2pInbox.get(type);
          if (waiters) {
            const idx = waiters.findIndex(w => w.resolve === resolve);
            if (idx >= 0) waiters.splice(idx, 1);
          }
          // Try Matrix poll with short timeout
          this.#matrixClient.onRoomEvent(roomId, type, Math.min(timeoutSecs, 1))
            .then(resolve)
            .catch(() => resolve('null'));
        }, timeoutSecs * 1000);

        if (!this.#p2pInbox.has(type)) this.#p2pInbox.set(type, []);
        this.#p2pInbox.get(type).push({ resolve, timer });
      });
    }

    // Matrix-only mode
    return this.#matrixClient.onRoomEvent(roomId, type, timeoutSecs);
  }

  /**
   * Proxy for any other client methods TerminalSocket might use.
   */
  userId() {
    return this.#matrixClient.userId();
  }

  #handleP2PMessage(msg) {
    let parsed;
    try {
      parsed = JSON.parse(msg);
    } catch {
      return;
    }

    // Handle acks
    if (parsed.type === 'org.mxdx.p2p.ack') {
      this.#handleAck(parsed.seq);
      return;
    }

    // Handle keepalive
    if (parsed.type === 'org.mxdx.p2p.ping') {
      try { this.#dataChannel.send(JSON.stringify({ type: 'org.mxdx.p2p.pong' })); } catch { /* */ }
      return;
    }
    if (parsed.type === 'org.mxdx.p2p.pong') {
      if (this.#pongTimer) { clearTimeout(this.#pongTimer); this.#pongTimer = null; }
      return;
    }

    // Send ack for terminal data events
    if (parsed.type === 'org.mxdx.terminal.data' && typeof parsed.content?.seq === 'number') {
      try {
        this.#dataChannel.send(JSON.stringify({ type: 'org.mxdx.p2p.ack', seq: parsed.content.seq }));
      } catch { /* best effort */ }
    }

    // Wrap as event JSON (matching what onRoomEvent returns from Matrix)
    const eventJson = JSON.stringify({ content: parsed.content });

    // Resolve any waiting onRoomEvent call
    const type = parsed.type;
    const waiters = this.#p2pInbox.get(type);
    if (waiters && waiters.length > 0) {
      const waiter = waiters.shift();
      clearTimeout(waiter.timer);
      waiter.resolve(eventJson);
      return;
    }

    // Queue for next onRoomEvent call
    if (!this.#p2pQueue.has(type)) this.#p2pQueue.set(type, []);
    this.#p2pQueue.get(type).push(eventJson);
  }

  #handleAck(seq) {
    // Ack is cumulative — clear this seq and all earlier
    for (const [pendingSeq, entry] of this.#pendingAcks) {
      if (pendingSeq <= seq) {
        clearTimeout(entry.timer);
        this.#pendingAcks.delete(pendingSeq);
      }
    }
  }

  #requeueUnacked(seq) {
    const entry = this.#pendingAcks.get(seq);
    if (!entry) return;

    clearTimeout(entry.timer);
    this.#pendingAcks.delete(seq);

    // Requeue via Matrix
    this.#matrixClient.sendEvent(entry.roomId, entry.type, entry.content).catch(() => {});
  }

  #handleChannelClose() {
    this.#dataChannel = null;
    this.#setStatus('matrix');
    this.#stopKeepalive();

    // Requeue all unacked events via Matrix
    for (const [seq, entry] of this.#pendingAcks) {
      clearTimeout(entry.timer);
      this.#matrixClient.sendEvent(entry.roomId, entry.type, entry.content).catch(() => {});
    }
    this.#pendingAcks.clear();

    // Resolve any waiting P2P inbox promises with null so they fall back to Matrix
    for (const [type, waiters] of this.#p2pInbox) {
      for (const waiter of waiters) {
        clearTimeout(waiter.timer);
        waiter.resolve('null');
      }
    }
    this.#p2pInbox.clear();
  }

  #startKeepalive() {
    this.#pingInterval = setInterval(() => {
      if (!this.#dataChannel?.isOpen) return;
      try {
        this.#dataChannel.send(JSON.stringify({ type: 'org.mxdx.p2p.ping' }));
      } catch {
        this.#handleChannelClose();
        return;
      }
      this.#pongTimer = setTimeout(() => {
        this.#handleChannelClose();
      }, 5000);
    }, 15000);
  }

  #stopKeepalive() {
    if (this.#pingInterval) { clearInterval(this.#pingInterval); this.#pingInterval = null; }
    if (this.#pongTimer) { clearTimeout(this.#pongTimer); this.#pongTimer = null; }
  }

  close() {
    this.#closed = true;
    this.#stopKeepalive();
    if (this.#dataChannel) {
      try { this.#dataChannel.close(); } catch { /* */ }
      this.#dataChannel = null;
    }
    // Clear pending ack timers
    for (const [, entry] of this.#pendingAcks) clearTimeout(entry.timer);
    this.#pendingAcks.clear();
    // Clear inbox timers
    for (const [, waiters] of this.#p2pInbox) {
      for (const w of waiters) clearTimeout(w.timer);
    }
    this.#p2pInbox.clear();
  }
}
```

**Step 4: Run test to verify it passes**

Run: `node --test packages/e2e-tests/test/p2p-transport.test.js`
Expected: PASS (all 4 tests)

**Step 5: Export from core**

In `packages/core/index.js`, add:

```javascript
export { P2PTransport } from './p2p-transport.js';
export { P2PSignaling } from './p2p-signaling.js';
```

**Step 6: Commit**

```bash
git add packages/core/p2p-transport.js packages/core/p2p-signaling.js packages/core/index.js packages/e2e-tests/test/p2p-transport.test.js
git commit -m "feat: add P2PTransport adapter with ack-based delivery and Matrix fallback"
```

---

## Task 8: Extend launcher telemetry with P2P discovery

**Files:**
- Modify: `packages/launcher/src/runtime.js` (the `#postTelemetry` method, ~line 718)

**Step 1: Add P2P block to telemetry**

In `#postTelemetry()`, after the `session_persistence` line and before the `sendStateEvent` call, add:

```javascript
    // P2P discovery info
    if (this.#config.p2pEnabled) {
      const nets = os.networkInterfaces();
      const internalIps = [];
      for (const ifaces of Object.values(nets)) {
        for (const iface of ifaces) {
          if (!iface.internal && iface.family === 'IPv4') {
            internalIps.push(iface.address);
          }
        }
      }
      telemetry.p2p = {
        enabled: true,
        ice_servers: (this.#config.iceServers || []).map(s => ({ urls: s })),
        internal_ips: internalIps,
        external_ip: null,
      };
    } else {
      telemetry.p2p = { enabled: false };
    }
```

Note: `os` is already imported at the top of `#postTelemetry()` via `const os = await import('node:os')`.

**Step 2: Verify telemetry includes P2P block**

Start launcher, then check the exec room state event. Alternatively, add a temporary log:

```bash
node -e "
import('./packages/launcher/src/config.js').then(({ LauncherConfig }) => {
  const c = new LauncherConfig({ username: 'test', servers: ['https://matrix.org'], p2pEnabled: true, iceServers: ['stun:stun.l.google.com:19302'] });
  console.log('p2pEnabled:', c.p2pEnabled);
});
"
```

**Step 3: Commit**

```bash
git add packages/launcher/src/runtime.js
git commit -m "feat: add P2P discovery block to launcher telemetry"
```

---

## Task 9: Wire P2P into launcher session handling

Add P2P signaling to `#handleInteractiveSession` and `#handleReconnect`. The launcher participates in bidirectional WebRTC handshake after starting the session.

**Files:**
- Modify: `packages/launcher/src/runtime.js`

**Step 1: Import P2P modules at top of runtime.js**

Add to existing imports:

```javascript
import { P2PTransport, P2PSignaling } from '@mxdx/core';
import { NodeWebRTCChannel } from '@mxdx/core/webrtc-channel-node.js';
```

**Step 2: Add P2P handshake helper method**

Add a new private method to LauncherRuntime class (after `#postTelemetry`):

```javascript
  /**
   * Attempt P2P handshake for an interactive session.
   * Runs in background — returns the P2PTransport immediately.
   * The transport starts in 'matrix' mode and upgrades to 'p2p' if handshake succeeds.
   */
  #attemptP2PHandshake(transport, signaling, requestId, iceServers) {
    const attempt = async () => {
      try {
        const channel = new NodeWebRTCChannel({ iceServers });

        // Create our offer (launcher role)
        const offer = await channel.createOffer();
        await signaling.sendOffer(requestId, 'launcher', offer);

        // Collect ICE candidates and send them
        channel.onIceCandidate((candidate) => {
          signaling.sendIceCandidate(requestId, 'launcher', candidate).catch(() => {});
        });

        // Wait for client's offer (bidirectional)
        const clientOffer = await signaling.waitForOffer(5);
        if (clientOffer && clientOffer.role === 'client') {
          // We also need to create a second channel to answer the client's offer
          const channel2 = new NodeWebRTCChannel({ iceServers });
          const answer = await channel2.acceptOffer(clientOffer);
          await signaling.sendAnswer(requestId, 'launcher', answer);

          channel2.onIceCandidate((candidate) => {
            signaling.sendIceCandidate(requestId, 'launcher', candidate).catch(() => {});
          });

          // Trickle ICE from client
          const iceLoop2 = async () => {
            for (let i = 0; i < 20; i++) {
              const ice = await signaling.waitForIceCandidate(1);
              if (ice && ice.role === 'client') channel2.addIceCandidate(ice.candidate);
            }
          };
          iceLoop2().catch(() => {});

          // Wait for either channel to open
          const winner = await Promise.race([
            channel.waitForDataChannel().then(() => ({ ch: channel, loser: channel2 })),
            channel2.waitForDataChannel().then(() => ({ ch: channel2, loser: channel })),
            new Promise((_, reject) => setTimeout(() => reject(new Error('P2P timeout')), 5000)),
          ]);

          winner.loser.close();
          transport._setDataChannel(winner.ch);
          this.#log.info('P2P data channel established', { request_id: requestId });
          return;
        }

        // No client offer — wait for client's answer to our offer
        const clientAnswer = await signaling.waitForAnswer(5);
        if (clientAnswer && clientAnswer.role === 'client') {
          await channel.acceptAnswer(clientAnswer);

          // Trickle ICE
          const iceLoop = async () => {
            for (let i = 0; i < 20; i++) {
              const ice = await signaling.waitForIceCandidate(1);
              if (ice && ice.role === 'client') channel.addIceCandidate(ice.candidate);
            }
          };
          iceLoop().catch(() => {});

          await Promise.race([
            channel.waitForDataChannel(),
            new Promise((_, reject) => setTimeout(() => reject(new Error('P2P timeout')), 5000)),
          ]);

          transport._setDataChannel(channel);
          this.#log.info('P2P data channel established', { request_id: requestId });
          return;
        }

        channel.close();
        this.#log.info('P2P handshake: no client response', { request_id: requestId });
      } catch (err) {
        this.#log.info('P2P handshake failed, staying on Matrix', { request_id: requestId, error: err.message });
      }
    };

    // Run in background — don't block session startup
    attempt().catch(() => {});
  }
```

**Step 3: Modify handleInteractiveSession to use P2PTransport**

In `#handleInteractiveSession`, after the DM room is created and before the `await this.#sendSessionResponse(...)` call, add the P2P transport setup. The key changes:

1. After `const dmRoomId = await this.#client.createDmRoom(sender);`, add:

```javascript
    // Set up P2P transport (wraps Matrix client with optional WebRTC upgrade)
    const transport = new P2PTransport(this.#client);
    const p2pEnabled = this.#config.p2pEnabled !== false;

    if (p2pEnabled) {
      const signaling = new P2PSignaling(this.#client, this.#topology.exec_room_id);
      const iceServers = (this.#config.iceServers || ['stun:stun.l.google.com:19302']).map(s => ({ urls: s }));
      this.#attemptP2PHandshake(transport, signaling, requestId, iceServers);
    }
```

2. In the session response, add `p2p: p2pEnabled`:

```javascript
    await this.#sendSessionResponse(requestId, 'started', dmRoomId, {
      session_id: sessionId,
      persistent: pty.persistent,
      batch_ms: negotiatedBatchMs,
      p2p_batch_ms: this.#config.p2pBatchMs || 10,
      p2p: p2pEnabled,
    });
```

3. Change the BatchedSender to use `transport` instead of direct `this.#client`:

```javascript
    const batchSender = new BatchedSender({
      sendEvent: (roomId, type, content) => transport.sendEvent(roomId, type, content),
      roomId: dmRoomId,
      batchMs: negotiatedBatchMs,
      onError: (err, seq) => this.#log.warn('terminal.data send failed', { seq, error: String(err) }),
    });
```

4. Add status change listener to switch batch window:

```javascript
    transport.onStatusChange((status) => {
      if (status === 'p2p') {
        batchSender.batchMs = this.#config.p2pBatchMs || 10;
        this.#log.info('Switched to P2P transport', { session_id: sessionId });
      } else {
        batchSender.batchMs = negotiatedBatchMs;
        this.#log.info('Fell back to Matrix transport', { session_id: sessionId });
      }
    });
```

5. Change the input polling to use `transport` instead of `this.#client`:

Replace `this.#client.onRoomEvent(dmRoomId, ...)` in the `pollForInput` loop with `transport.onRoomEvent(dmRoomId, ...)`.

6. In the `finally` block, add `transport.close()`:

```javascript
    pollForInput().finally(() => {
      transport.close();
      // ... existing cleanup ...
    });
```

**Step 4: Apply same pattern to handleReconnect**

Same changes as above but in `#handleReconnect`. The reconnect response should also include `p2p: p2pEnabled` and `p2p_batch_ms`.

**Step 5: Verify launcher starts without errors**

Run:
```bash
node packages/launcher/bin/mxdx-launcher.js &
sleep 10
kill %1
```
Expected: No crash, launcher starts and shuts down normally. P2P handshake will fail (no client) but should log gracefully.

**Step 6: Commit**

```bash
git add packages/launcher/src/runtime.js
git commit -m "feat: wire P2P transport into launcher session handling"
```

---

## Task 10: Wire P2P into browser terminal-view

**Files:**
- Modify: `packages/web-console/src/terminal-view.js`
- Modify: `packages/web-console/src/terminal-socket.js`

**Step 1: Add P2P handshake to setupTerminalView**

In `packages/web-console/src/terminal-view.js`, add import at top:

```javascript
import { P2PTransport, P2PSignaling } from '../../core/p2p-transport.js';
import { BrowserWebRTCChannel } from './webrtc-channel.js';
```

Note: Fix import paths if needed — the web-console imports core modules via relative paths (e.g., `../../core/batched-sender.js`). Check existing imports in the file for the pattern.

After the session response is received and `dmRoomId` is set (around line 134), add:

```javascript
    // Set up P2P transport
    const p2pEnabled = sessionContent.p2p && (localStorage.getItem('mxdx-p2p-enabled') !== 'false');
    const transport = new P2PTransport(client);

    if (p2pEnabled) {
      const signaling = new P2PSignaling(client, launcher.exec_room_id);
      const iceServers = sessionContent.ice_servers || [{ urls: 'stun:stun.l.google.com:19302' }];

      // Bidirectional P2P handshake (client role)
      (async () => {
        try {
          const channel = new BrowserWebRTCChannel({ iceServers });
          const offer = await channel.createOffer();
          await signaling.sendOffer(requestId, 'client', offer);

          channel.onIceCandidate((candidate) => {
            signaling.sendIceCandidate(requestId, 'client', candidate).catch(() => {});
          });

          // Wait for launcher's offer
          const launcherOffer = await signaling.waitForOffer(5);
          if (launcherOffer && launcherOffer.role === 'launcher') {
            const channel2 = new BrowserWebRTCChannel({ iceServers });
            const answer = await channel2.acceptOffer(launcherOffer);
            await signaling.sendAnswer(requestId, 'client', answer);

            channel2.onIceCandidate((candidate) => {
              signaling.sendIceCandidate(requestId, 'client', candidate).catch(() => {});
            });

            const iceLoop2 = async () => {
              for (let i = 0; i < 20; i++) {
                const ice = await signaling.waitForIceCandidate(1);
                if (ice && ice.role === 'launcher') channel2.addIceCandidate(ice.candidate);
              }
            };
            iceLoop2().catch(() => {});

            // First channel to open wins; client-initiated wins ties
            const winner = await Promise.race([
              channel.waitForDataChannel().then(() => ({ ch: channel, loser: channel2 })),
              channel2.waitForDataChannel().then(() => ({ ch: channel2, loser: channel })),
              new Promise((_, reject) => setTimeout(() => reject(new Error('P2P timeout')), 5000)),
            ]);

            winner.loser.close();
            transport._setDataChannel(winner.ch);
            return;
          }

          // No launcher offer — wait for answer to our offer
          const launcherAnswer = await signaling.waitForAnswer(5);
          if (launcherAnswer && launcherAnswer.role === 'launcher') {
            await channel.acceptAnswer(launcherAnswer);
            const iceLoop = async () => {
              for (let i = 0; i < 20; i++) {
                const ice = await signaling.waitForIceCandidate(1);
                if (ice && ice.role === 'launcher') channel.addIceCandidate(ice.candidate);
              }
            };
            iceLoop().catch(() => {});

            await Promise.race([
              channel.waitForDataChannel(),
              new Promise((_, reject) => setTimeout(() => reject(new Error('P2P timeout')), 5000)),
            ]);
            transport._setDataChannel(channel);
            return;
          }

          channel.close();
        } catch {
          // P2P failed, stay on Matrix — no user-visible error
        }
      })();
    }
```

Then change the TerminalSocket creation to use `transport` instead of `client`:

```javascript
    const socket = new TerminalSocket(transport, dmRoomId, { pollIntervalMs: 100, batchMs: negotiatedBatchMs });
```

Add batch window switching on P2P status change:

```javascript
    const p2pBatchMs = sessionContent.p2p_batch_ms || 10;
    transport.onStatusChange((status) => {
      if (socket._sender) {
        socket._sender.batchMs = status === 'p2p' ? p2pBatchMs : negotiatedBatchMs;
      }
    });
```

**Step 2: Update TerminalSocket to expose sender for batch window control**

In `packages/web-console/src/terminal-socket.js`, add a getter:

```javascript
  get _sender() { return this.#sender; }
```

Do the same in `packages/core/terminal-socket.js`.

**Step 3: Update status indicator**

Modify the `onbuffering` wiring to also show P2P status. After the socket creation:

```javascript
    // Wire: P2P status indicator
    transport.onStatusChange((status) => {
      if (statusEl) {
        if (status === 'p2p') {
          statusEl.textContent = 'P2P';
          statusEl.className = 'status-p2p';
          statusEl.hidden = false;
          // Hide after 3 seconds
          setTimeout(() => { if (transport.status === 'p2p') statusEl.hidden = true; }, 3000);
        } else {
          statusEl.textContent = 'Matrix (P2P lost)';
          statusEl.className = 'status-p2p-lost';
          statusEl.hidden = false;
          setTimeout(() => {
            if (transport.status === 'matrix') {
              statusEl.hidden = true;
              statusEl.className = '';
            }
          }, 5000);
        }
      }
    });
```

Update the existing `onbuffering` handler to show homeserver name:

```javascript
    socket.onbuffering = (buffering) => {
      if (statusEl) {
        if (buffering) {
          const homeserver = launcher.exec_room_id.split(':').pop();
          statusEl.textContent = `Rate-limited by ${homeserver}`;
          statusEl.className = 'status-rate-limited';
          statusEl.hidden = false;
        } else if (transport.status !== 'p2p') {
          statusEl.hidden = true;
          statusEl.className = '';
        }
      }
    };
```

**Step 4: Apply same P2P wiring to reconnectTerminalView**

Same pattern as setupTerminalView — set up P2PTransport, attempt handshake, use transport for TerminalSocket.

**Step 5: Add transport.close() to socket onclose handlers**

In both `setupTerminalView` and `reconnectTerminalView`, in the `socket.onclose` handler:

```javascript
    socket.onclose = () => {
      transport.close();
      // ... existing cleanup ...
    };
```

**Step 6: Commit**

```bash
git add packages/web-console/src/terminal-view.js packages/web-console/src/terminal-socket.js packages/core/terminal-socket.js
git commit -m "feat: wire P2P transport into browser terminal sessions"
```

---

## Task 11: UI indicator styles

**Files:**
- Modify: `packages/web-console/src/style.css`

**Step 1: Add P2P status indicator styles**

Find the existing `#terminal-status` styles and add after them:

```css
#terminal-status.status-p2p {
  color: #3fb950;
  background: rgba(63, 185, 80, 0.1);
}

#terminal-status.status-p2p-lost {
  color: #d29922;
  background: rgba(210, 153, 34, 0.1);
}

#terminal-status.status-rate-limited {
  color: #f85149;
  background: rgba(248, 81, 73, 0.1);
}

#terminal-status.status-connecting {
  color: #d29922;
  background: rgba(210, 153, 34, 0.1);
}
```

**Step 2: Verify styles render**

Start vite dev server, open terminal session, inspect `#terminal-status` element. It should show the appropriate color based on transport status.

**Step 3: Commit**

```bash
git add packages/web-console/src/style.css
git commit -m "feat: add P2P status indicator styles (green/amber/red)"
```

---

## Task 12: P2P Settings in web console

**Files:**
- Modify: `packages/web-console/src/settings.js` (if it exists — add P2P toggle)

**Step 1: Add P2P settings to the Settings page**

Add a "P2P Transport" section with:
- Enable/disable toggle (reads/writes `localStorage mxdx-p2p-enabled`)
- Batch window input (reads/writes `localStorage mxdx-p2p-batch-ms`, default 10)

This follows the same pattern as the existing batch-ms setting that reads from `localStorage.getItem('mxdx-batch-ms')`.

**Step 2: Commit**

```bash
git add packages/web-console/src/settings.js
git commit -m "feat: add P2P settings to web console settings page"
```

---

## Task 13: End-to-end P2P test

**Files:**
- Create: `packages/e2e-tests/test/p2p-e2e.test.js`

**Step 1: Write E2E test**

This test verifies the full P2P flow using two local Matrix clients (via Tuwunel) and loopback WebRTC:

```javascript
import { describe, it, before, after } from 'node:test';
import assert from 'node:assert/strict';
import { P2PTransport } from '../../../packages/core/p2p-transport.js';
import { NodeWebRTCChannel } from '../../../packages/core/webrtc-channel-node.js';

describe('P2P E2E', () => {
  it('two P2PTransports exchange data over WebRTC', async () => {
    const clientSent = [];
    const launcherSent = [];

    const mockClientMatrix = {
      sendEvent: async (r, t, c) => clientSent.push({ r, t, c }),
      onRoomEvent: async () => 'null',
      userId: () => '@client:test',
    };
    const mockLauncherMatrix = {
      sendEvent: async (r, t, c) => launcherSent.push({ r, t, c }),
      onRoomEvent: async () => 'null',
      userId: () => '@launcher:test',
    };

    const clientTransport = new P2PTransport(mockClientMatrix);
    const launcherTransport = new P2PTransport(mockLauncherMatrix);

    // Create WebRTC peers
    const clientCh = new NodeWebRTCChannel({ iceServers: [] });
    const launcherCh = new NodeWebRTCChannel({ iceServers: [] });

    const offer = await clientCh.createOffer();
    const answer = await launcherCh.acceptOffer(offer);
    await clientCh.acceptAnswer(answer);

    // Exchange ICE
    clientCh.onIceCandidate((c) => launcherCh.addIceCandidate(c));
    launcherCh.onIceCandidate((c) => clientCh.addIceCandidate(c));

    // Wait for connection
    await Promise.race([
      Promise.all([clientCh.waitForDataChannel(), launcherCh.waitForDataChannel()]),
      new Promise((_, r) => setTimeout(() => r(new Error('timeout')), 10000)),
    ]);

    // Wire channels into transports
    clientTransport._setDataChannel(clientCh);
    launcherTransport._setDataChannel(launcherCh);

    assert.equal(clientTransport.status, 'p2p');
    assert.equal(launcherTransport.status, 'p2p');

    // Send data client -> launcher over P2P
    const received = new Promise((resolve) => {
      // Launcher polls for data
      launcherTransport.onRoomEvent('!room:test', 'org.mxdx.terminal.data', 5).then(resolve);
    });

    await clientTransport.sendEvent(
      '!room:test',
      'org.mxdx.terminal.data',
      JSON.stringify({ data: 'aGVsbG8=', encoding: 'base64', seq: 0 }),
    );

    const result = await received;
    assert.ok(result && result !== 'null', 'should receive data via P2P');
    const parsed = JSON.parse(result);
    assert.equal(parsed.content.seq, 0);

    // Verify nothing went through Matrix
    const matrixTerminalEvents = clientSent.filter(s => s.t === 'org.mxdx.terminal.data');
    assert.equal(matrixTerminalEvents.length, 0, 'no terminal data should go through Matrix when P2P is active');

    clientTransport.close();
    launcherTransport.close();
    clientCh.close();
    launcherCh.close();
  });
});
```

**Step 2: Run test**

Run: `node --test packages/e2e-tests/test/p2p-e2e.test.js`
Expected: PASS

**Step 3: Commit**

```bash
git add packages/e2e-tests/test/p2p-e2e.test.js
git commit -m "test: add P2P end-to-end test with WebRTC loopback"
```

---

## Task 14: Full system E2E test with Tuwunel

**Files:**
- Modify: `packages/e2e-tests/test/terminal-e2e.test.js` (add P2P variant)

**Step 1: Add P2P terminal test**

Add a new test case to the existing terminal E2E test file that:

1. Starts two Tuwunel instances (existing helper)
2. Registers launcher and client accounts
3. Launcher starts with `p2pEnabled: true`
4. Client requests interactive session with `p2p: true`
5. Verify P2P handshake succeeds (both sides log "P2P data channel established")
6. Send test input, verify output arrives
7. Verify `transport.status === 'p2p'`

This test depends on `node-datachannel` being able to establish loopback WebRTC connections (which it can — no STUN needed for localhost).

**Step 2: Run the full test suite**

Run: `node --test packages/e2e-tests/test/`
Expected: All existing tests PASS, new P2P test PASS.

**Step 3: Commit**

```bash
git add packages/e2e-tests/test/terminal-e2e.test.js
git commit -m "test: add P2P terminal session E2E test with Tuwunel"
```

---

## Summary

| Task | Component | Description |
|:---|:---|:---|
| 1 | Dependencies | Install `node-datachannel` |
| 2 | BatchedSender | Add dynamic `batchMs` setter |
| 3 | Config | P2P config fields for launcher + client |
| 4 | WebRTC (Node) | `NodeWebRTCChannel` wrapper |
| 5 | WebRTC (Browser) | `BrowserWebRTCChannel` wrapper |
| 6 | Signaling | `P2PSignaling` for SDP/ICE via Matrix |
| 7 | Transport | `P2PTransport` adapter with acks + fallback |
| 8 | Telemetry | P2P discovery block in launcher telemetry |
| 9 | Launcher | Wire P2P into session handling |
| 10 | Browser | Wire P2P into terminal-view |
| 11 | Styles | P2P status indicator CSS |
| 12 | Settings | P2P toggle in web console settings |
| 13 | Test | P2P E2E test with loopback WebRTC |
| 14 | Test | Full system E2E with Tuwunel |
