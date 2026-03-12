/**
 * P2PTransport — adapter between TerminalSocket/BatchedSender and WebRTC data channel.
 *
 * Implements the same sendEvent/onRoomEvent interface as the Matrix client.
 * All terminal data is AES-256-GCM encrypted (via P2PCrypto) before placement
 * on the data channel. The session key is exchanged via E2EE Matrix signaling,
 * so peer identity is authenticated by the Megolm layer.
 *
 * NEVER sends unencrypted terminal data over P2P.
 * Falls back to Matrix transparently on any P2P failure.
 */

/** Cross-platform random hex string (works in browser + Node 19+). */
function randomHex(byteCount) {
  const buf = new Uint8Array(byteCount);
  globalThis.crypto.getRandomValues(buf);
  return Array.from(buf, b => b.toString(16).padStart(2, '0')).join('');
}

const MAX_FRAME_SIZE = 64 * 1024; // 64KB
const INITIAL_BACKOFF_MS = 10_000;
const MAX_BACKOFF_MS = 300_000; // 5 minutes
const VERIFY_TIMEOUT_MS = 10_000;

// Event types that carry terminal content and MUST be encrypted
const ENCRYPTED_EVENT_TYPES = new Set([
  'org.mxdx.terminal.data',
  'org.mxdx.terminal.resize',
]);

export class P2PTransport {
  #matrixClient;
  #p2pCrypto;        // P2PCrypto instance for AES-256-GCM encryption
  #localDeviceId;
  #idleTimeoutMs;
  #onStatusChange;
  #onReconnectNeeded;
  #onHangup;

  #dataChannel = null;
  #peerVerified = false;
  #status = 'matrix';
  #closed = false;

  // Peer verification state
  #localNonce = null;
  #remoteNonce = null;
  #localVerified = false;   // Remote acknowledged our challenge
  #verifyTimer = null;

  // P2P inbox: Map<eventType, Array<resolvedJson>>
  #p2pInbox = new Map();
  // Waiters for inbox items: Map<eventType, Array<{resolve, timer}>>
  #inboxWaiters = new Map();

  // Unacked sent events for requeue on channel loss
  #pendingAcks = [];

  // Idle timeout
  #idleTimer = null;

  // Reconnect backoff
  #reconnectBackoffMs = INITIAL_BACKOFF_MS;
  #lastReconnectAt = 0;
  #hadSuccessfulP2P = false;

  constructor({
    matrixClient,
    p2pCrypto,
    localDeviceId,
    idleTimeoutMs = 300_000,
    onStatusChange = null,
    onReconnectNeeded = null,
    onHangup = null,
  }) {
    this.#matrixClient = matrixClient;
    this.#p2pCrypto = p2pCrypto;
    this.#localDeviceId = localDeviceId;
    this.#idleTimeoutMs = idleTimeoutMs;
    this.#onStatusChange = onStatusChange;
    this.#onReconnectNeeded = onReconnectNeeded;
    this.#onHangup = onHangup;
  }

  /**
   * Factory method — creates a fully configured P2PTransport.
   */
  static create(opts) {
    return new P2PTransport(opts);
  }

  get status() {
    return this.#status;
  }

  /**
   * Replace the P2P encryption key. Called when a shared key is negotiated
   * during signaling (the offerer's key, sent via E2EE Matrix invite).
   */
  setP2PCrypto(p2pCrypto) {
    this.#p2pCrypto = p2pCrypto;
  }

  /**
   * Attach a WebRTC data channel. Registers message/close handlers.
   * Automatically initiates peer verification handshake.
   * The channel is NOT used for terminal data until both sides verify.
   */
  setDataChannel(channel) {
    // Close old channel if being replaced (prevents stale onClose from killing new channel)
    if (this.#dataChannel && this.#dataChannel !== channel) {
      this.#log('[p2p-transport] replacing existing data channel');
      try { this.#dataChannel.close(); } catch { /* already closed */ }
    }

    this.#dataChannel = channel;
    this.#peerVerified = false;
    this.#localVerified = false;
    this.#localNonce = null;
    this.#remoteNonce = null;

    this.#log('[p2p-transport] setDataChannel called, isOpen:', channel.isOpen);

    channel.onMessage((msg) => {
      this.#handleDataChannelMessage(msg);
    });

    channel.onClose(() => {
      // Only handle close if this is still the current channel
      if (this.#dataChannel !== channel) {
        this.#log('[p2p-transport] stale channel closed (ignored)');
        return;
      }
      this.#log('[p2p-transport] channel closed');
      this.#handleChannelClose();
    });

    // Start peer verification automatically
    this.#startVerification();
  }

  #log(...args) {
    // Log to console (works in both browser and Node.js)
    if (typeof console !== 'undefined') {
      console.log(...args);
    }
  }

  /**
   * Send an event — routes to P2P (encrypted) or Matrix fallback.
   * Implements the same interface as the Matrix client's sendEvent.
   */
  async sendEvent(roomId, type, contentJson) {
    if (this.#closed) return;

    // Reset idle timer on any send activity
    if (this.#status === 'p2p') {
      this.#resetIdleTimer();
    }

    // If P2P is active, verified, and this is a terminal event — encrypt and send via data channel
    if (
      this.#status === 'p2p' &&
      this.#peerVerified &&
      this.#dataChannel &&
      this.#p2pCrypto &&
      ENCRYPTED_EVENT_TYPES.has(type)
    ) {
      try {
        const ciphertext = await this.#p2pCrypto.encrypt(contentJson);
        const frame = JSON.stringify({
          type: 'encrypted',
          ciphertext,
          event_type: type,
        });

        if (frame.length > MAX_FRAME_SIZE) {
          // Fall back to Matrix for oversized frames
          await this.#matrixClient.sendEvent(roomId, type, contentJson);
          return;
        }

        this.#log('[p2p-transport] sending encrypted frame via P2P, type=', type, 'len=', frame.length);
        this.#dataChannel.send(frame);
        // Track for potential requeue
        this.#pendingAcks.push({ roomId, type, contentJson });
        return;
      } catch (err) {
        this.#log('[p2p-transport] send via P2P failed:', err.message || err, '— falling back to Matrix');
      }
    }

    // Matrix fallback
    await this.#matrixClient.sendEvent(roomId, type, contentJson);

    // If we're on matrix and had P2P before, consider reconnecting
    if (this.#status === 'matrix' && this.#onReconnectNeeded) {
      this.#maybeReconnect();
    }
  }

  /**
   * Poll for incoming events — checks P2P inbox first, then Matrix.
   * Implements the same interface as the Matrix client's onRoomEvent.
   */
  async onRoomEvent(roomId, type, timeoutSecs) {
    // Check P2P inbox first
    const inbox = this.#p2pInbox.get(type);
    if (inbox && inbox.length > 0) {
      return inbox.shift();
    }

    // Wait for either P2P inbox push or timeout
    if (this.#status === 'p2p' && timeoutSecs > 0) {
      const result = await this.#waitForInbox(type, timeoutSecs);
      if (result !== null) {
        return result;
      }
    }

    // Fall through to Matrix polling
    return this.#matrixClient.onRoomEvent(roomId, type, timeoutSecs);
  }

  /**
   * Clean up all resources.
   */
  close() {
    this.#closed = true;
    this.#clearIdleTimer();
    this.#clearVerifyTimer();
    this.#clearAllWaiters();
    if (this.#dataChannel) {
      try { this.#dataChannel.close(); } catch { /* already closed */ }
    }
  }

  // --- Peer Verification ---
  // The session key (exchanged via E2EE Matrix signaling) provides authentication.
  // This handshake just confirms both sides are connected and ready.

  #startVerification() {
    this.#localNonce = randomHex(32);
    this.#log('[p2p-transport] sending peer_verify, channel isOpen:', this.#dataChannel?.isOpen);
    this.#sendControlFrame({
      type: 'peer_verify',
      nonce: this.#localNonce,
      device_id: this.#localDeviceId,
    });

    // Verification timeout — 10 seconds
    this.#verifyTimer = setTimeout(() => {
      this.#verifyTimer = null;
      if (!this.#peerVerified) {
        if (this.#dataChannel) {
          try { this.#dataChannel.close(); } catch { /* already closed */ }
          this.#dataChannel = null;
        }
        this.#peerVerified = false;
        this.#setStatus('matrix');
      }
    }, VERIFY_TIMEOUT_MS);
  }

  #handlePeerVerify(frame) {
    this.#log('[p2p-transport] received peer_verify from device:', frame.device_id);
    this.#remoteNonce = frame.nonce;
    const remoteDeviceId = frame.device_id;
    if (!this.#remoteNonce || !remoteDeviceId) return;

    // Acknowledge the remote's challenge
    this.#sendControlFrame({
      type: 'peer_verify_response',
      nonce: this.#remoteNonce,
      device_id: this.#localDeviceId,
    });

    this.#checkVerificationComplete();
  }

  #handlePeerVerifyResponse(frame) {
    this.#log('[p2p-transport] received peer_verify_response, nonce match:', frame.nonce === this.#localNonce);
    if (!frame.nonce || !frame.device_id) return;
    if (frame.nonce !== this.#localNonce) return;

    // Remote acknowledged our challenge
    this.#localVerified = true;
    this.#checkVerificationComplete();
  }

  #checkVerificationComplete() {
    this.#log('[p2p-transport] checkVerification: localVerified=', this.#localVerified, 'remoteNonce=', !!this.#remoteNonce);
    // Both sides must have exchanged: we got their response (localVerified)
    // AND they got our challenge (remoteNonce is set, meaning we responded)
    if (this.#localVerified && this.#remoteNonce) {
      this.#peerVerified = true;
      this.#clearVerifyTimer();
      this.#hadSuccessfulP2P = true;
      this.#reconnectBackoffMs = INITIAL_BACKOFF_MS;
      this.#setStatus('p2p');
      this.#resetIdleTimer();
    }
  }

  #clearVerifyTimer() {
    if (this.#verifyTimer) {
      clearTimeout(this.#verifyTimer);
      this.#verifyTimer = null;
    }
  }

  // --- Private methods ---

  #handleDataChannelMessage(msg) {
    // Frame size check — drop oversized messages
    if (typeof msg === 'string' && msg.length > MAX_FRAME_SIZE) {
      return; // Drop silently
    }
    if (typeof msg !== 'string') {
      const len = msg.byteLength || msg.length || 0;
      if (len > MAX_FRAME_SIZE) return;
    }

    // Reset idle timer on incoming data
    if (this.#status === 'p2p') {
      this.#resetIdleTimer();
    }

    let frame;
    try {
      frame = JSON.parse(msg);
    } catch {
      return; // Malformed frame, drop
    }

    this.#log('[p2p-transport] received frame type:', frame.type);

    switch (frame.type) {
      case 'encrypted':
        this.#handleEncryptedFrame(frame);
        break;
      case 'ack':
        this.#handleAck(frame);
        break;
      case 'ping':
        this.#sendControlFrame({ type: 'pong' });
        break;
      case 'pong':
        break;
      case 'peer_verify':
        this.#handlePeerVerify(frame);
        break;
      case 'peer_verify_response':
        this.#handlePeerVerifyResponse(frame);
        break;
      default:
        break;
    }
  }

  async #handleEncryptedFrame(frame) {
    if (!this.#peerVerified || !this.#p2pCrypto) {
      this.#log('[p2p-transport] encrypted frame dropped: verified=', this.#peerVerified, 'hasCrypto=', !!this.#p2pCrypto);
      return;
    }

    try {
      const plaintext = await this.#p2pCrypto.decrypt(frame.ciphertext);
      const event = JSON.parse(plaintext);
      const eventType = event.type || frame.event_type;

      this.#log('[p2p-transport] decrypted frame: eventType=', eventType, 'hasData=', !!event.data, 'seq=', event.seq);

      if (!eventType) return;

      // Push to inbox
      if (!this.#p2pInbox.has(eventType)) {
        this.#p2pInbox.set(eventType, []);
      }
      this.#p2pInbox.get(eventType).push(plaintext);

      // Resolve any waiters
      const waiters = this.#inboxWaiters.get(eventType);
      if (waiters && waiters.length > 0) {
        const waiter = waiters.shift();
        clearTimeout(waiter.timer);
        this.#log('[p2p-transport] resolving waiter for', eventType);
        waiter.resolve(this.#p2pInbox.get(eventType).shift());
      } else {
        this.#log('[p2p-transport] no waiter for', eventType, 'inbox size=', this.#p2pInbox.get(eventType).length);
      }
    } catch (err) {
      this.#log('[p2p-transport] decryption FAILED:', err.message || err);
    }
  }

  #handleAck(frame) {
    const ackedSeq = frame.seq;
    this.#pendingAcks = this.#pendingAcks.filter((evt) => {
      try {
        const content = JSON.parse(evt.contentJson);
        return (content.seq || 0) > ackedSeq;
      } catch {
        return true;
      }
    });
  }

  #handleChannelClose() {
    const wasP2P = this.#status === 'p2p';
    this.#dataChannel = null;
    this.#peerVerified = false;
    this.#localVerified = false;
    this.#clearVerifyTimer();
    this.#setStatus('matrix');
    this.#clearIdleTimer();

    // Requeue unacked events via Matrix
    if (wasP2P && this.#pendingAcks.length > 0) {
      const toRequeue = [...this.#pendingAcks];
      this.#pendingAcks = [];
      for (const evt of toRequeue) {
        this.#matrixClient.sendEvent(evt.roomId, evt.type, evt.contentJson).catch(() => {});
      }
    }
  }

  #sendControlFrame(frame) {
    if (this.#dataChannel && this.#dataChannel.isOpen) {
      try {
        this.#dataChannel.send(JSON.stringify(frame));
      } catch { /* channel may be closing */ }
    }
  }

  #setStatus(newStatus) {
    if (this.#status === newStatus) return;
    this.#status = newStatus;
    if (this.#onStatusChange) {
      try { this.#onStatusChange(newStatus); } catch { /* callback error */ }
    }
  }

  #resetIdleTimer() {
    this.#clearIdleTimer();
    if (this.#idleTimeoutMs > 0 && this.#status === 'p2p') {
      this.#idleTimer = setTimeout(() => {
        this.#handleIdleTimeout();
      }, this.#idleTimeoutMs);
    }
  }

  #clearIdleTimer() {
    if (this.#idleTimer) {
      clearTimeout(this.#idleTimer);
      this.#idleTimer = null;
    }
  }

  #handleIdleTimeout() {
    this.#clearIdleTimer();
    if (this.#dataChannel) {
      try { this.#dataChannel.close(); } catch { /* already closed */ }
      this.#dataChannel = null;
    }
    this.#peerVerified = false;
    this.#setStatus('matrix');

    if (this.#pendingAcks.length > 0) {
      const toRequeue = [...this.#pendingAcks];
      this.#pendingAcks = [];
      for (const evt of toRequeue) {
        this.#matrixClient.sendEvent(evt.roomId, evt.type, evt.contentJson).catch(() => {});
      }
    }

    if (this.#onHangup) {
      try { this.#onHangup('idle_timeout'); } catch { /* callback error */ }
    }
  }

  #maybeReconnect() {
    const now = Date.now();
    if (now - this.#lastReconnectAt < this.#reconnectBackoffMs) {
      return;
    }
    this.#lastReconnectAt = now;
    this.#reconnectBackoffMs = Math.min(this.#reconnectBackoffMs * 2, MAX_BACKOFF_MS);

    if (this.#onReconnectNeeded) {
      try { this.#onReconnectNeeded(); } catch { /* callback error */ }
    }
  }

  #waitForInbox(type, timeoutSecs) {
    return new Promise((resolve) => {
      const timer = setTimeout(() => {
        const waiters = this.#inboxWaiters.get(type);
        if (waiters) {
          const idx = waiters.findIndex((w) => w.timer === timer);
          if (idx !== -1) waiters.splice(idx, 1);
        }
        resolve(null);
      }, timeoutSecs * 1000);

      if (!this.#inboxWaiters.has(type)) {
        this.#inboxWaiters.set(type, []);
      }
      this.#inboxWaiters.get(type).push({ resolve, timer });
    });
  }

  #clearAllWaiters() {
    for (const [, waiters] of this.#inboxWaiters) {
      for (const w of waiters) {
        clearTimeout(w.timer);
        w.resolve(null);
      }
    }
    this.#inboxWaiters.clear();
  }
}
