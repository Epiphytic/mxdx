/**
 * P2PTransport — adapter between TerminalSocket/BatchedSender and WebRTC data channel.
 *
 * Implements the same sendEvent/onRoomEvent interface as the Matrix client.
 * All terminal data is Megolm-encrypted before placement on the data channel.
 * NEVER sends unencrypted terminal data over P2P.
 *
 * Peer verification must complete before any encrypted data flows.
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
const ACK_TIMEOUT_MS = 2_000;
const VERIFY_TIMEOUT_MS = 10_000;

// Event types that carry terminal content and MUST be encrypted
const ENCRYPTED_EVENT_TYPES = new Set([
  'org.mxdx.terminal.data',
  'org.mxdx.terminal.resize',
]);

export class P2PTransport {
  #matrixClient;
  #encryptFn;
  #decryptFn;
  #signFn;
  #verifySignatureFn;
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
  #localVerified = false;   // We verified the remote peer
  #remoteVerified = false;  // Remote peer verified us (they sent response, we confirmed)
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
    encryptFn,
    decryptFn,
    signFn,
    verifySignatureFn,
    localDeviceId,
    idleTimeoutMs = 300_000,
    onStatusChange = null,
    onReconnectNeeded = null,
    onHangup = null,
  }) {
    this.#matrixClient = matrixClient;
    this.#encryptFn = encryptFn;
    this.#decryptFn = decryptFn;
    this.#signFn = signFn;
    this.#verifySignatureFn = verifySignatureFn;
    this.#localDeviceId = localDeviceId;
    this.#idleTimeoutMs = idleTimeoutMs;
    this.#onStatusChange = onStatusChange;
    this.#onReconnectNeeded = onReconnectNeeded;
    this.#onHangup = onHangup;
  }

  /**
   * Factory method — creates a fully configured P2PTransport.
   * No post-construction mutation needed.
   */
  static create(opts) {
    return new P2PTransport(opts);
  }

  get status() {
    return this.#status;
  }

  /**
   * Attach a WebRTC data channel. Registers message/close handlers.
   * Automatically initiates peer verification challenge-response.
   * The channel is NOT used for terminal data until both sides verify.
   */
  setDataChannel(channel) {
    this.#dataChannel = channel;
    this.#peerVerified = false;
    this.#localVerified = false;
    this.#remoteVerified = false;
    this.#localNonce = null;
    this.#remoteNonce = null;

    channel.onMessage((msg) => {
      this.#handleDataChannelMessage(msg);
    });

    channel.onClose(() => {
      this.#handleChannelClose();
    });

    // Start peer verification automatically
    this.#startVerification();
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
      ENCRYPTED_EVENT_TYPES.has(type)
    ) {
      try {
        const ciphertext = this.#encryptFn(roomId, type, contentJson);
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

        this.#dataChannel.send(frame);
        // Track for potential requeue
        this.#pendingAcks.push({ roomId, type, contentJson });
        return;
      } catch {
        // Encryption or send failed — fall back to Matrix
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

  #startVerification() {
    // Generate a random nonce and send challenge
    this.#localNonce = randomHex(32);
    this.#sendControlFrame({
      type: 'peer_verify',
      nonce: this.#localNonce,
      device_id: this.#localDeviceId,
    });

    // Set verification timeout — 10 seconds
    this.#verifyTimer = setTimeout(() => {
      this.#verifyTimer = null;
      if (!this.#peerVerified) {
        // Verification timed out — close channel, fall back to Matrix
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
    // Remote sent us their nonce — sign it and respond
    this.#remoteNonce = frame.nonce;
    const remoteDeviceId = frame.device_id;

    if (!this.#remoteNonce || !remoteDeviceId) return;

    // Sign the remote's nonce with our device key
    const signature = this.#signFn(this.#remoteNonce);
    this.#sendControlFrame({
      type: 'peer_verify_response',
      nonce: this.#remoteNonce,
      device_id: this.#localDeviceId,
      signature,
    });
  }

  #handlePeerVerifyResponse(frame) {
    // Remote signed our nonce — verify their signature
    if (!frame.nonce || !frame.signature || !frame.device_id) return;

    // Must be responding to our nonce
    if (frame.nonce !== this.#localNonce) return;

    const valid = this.#verifySignatureFn(frame.nonce, frame.signature, frame.device_id);
    if (!valid) {
      // Verification failed — close channel
      if (this.#dataChannel) {
        try { this.#dataChannel.close(); } catch { /* already closed */ }
        this.#dataChannel = null;
      }
      this.#peerVerified = false;
      this.#clearVerifyTimer();
      this.#setStatus('matrix');
      return;
    }

    // We have verified the remote peer
    this.#localVerified = true;
    this.#checkVerificationComplete();
  }

  #checkVerificationComplete() {
    // Both sides must have completed: we verified them (localVerified)
    // AND they verified us (we sent our response when we got their nonce)
    // localVerified = we verified their signature on our nonce
    // remoteNonce being set = they sent us a challenge, and we responded
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
        // Keepalive response — nothing to do
        break;
      case 'peer_verify':
        this.#handlePeerVerify(frame);
        break;
      case 'peer_verify_response':
        this.#handlePeerVerifyResponse(frame);
        break;
      default:
        // Unknown frame type, ignore
        break;
    }
  }

  #handleEncryptedFrame(frame) {
    if (!this.#peerVerified) return; // Reject data before verification

    try {
      const plaintext = this.#decryptFn(frame.ciphertext);
      const event = JSON.parse(plaintext);
      const eventType = event.type || frame.event_type;

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
        waiter.resolve(this.#p2pInbox.get(eventType).shift());
      }
    } catch {
      // Decryption failed — drop frame
    }
  }

  #handleAck(frame) {
    const ackedSeq = frame.seq;
    // Remove acked events from pending buffer
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
    this.#remoteVerified = false;
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
    // Tear down P2P channel due to inactivity
    this.#clearIdleTimer();
    if (this.#dataChannel) {
      try { this.#dataChannel.close(); } catch { /* already closed */ }
      this.#dataChannel = null;
    }
    this.#peerVerified = false;
    this.#setStatus('matrix');

    // Requeue any pending
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
      return; // Within backoff window
    }
    this.#lastReconnectAt = now;
    // Increase backoff for next time
    this.#reconnectBackoffMs = Math.min(this.#reconnectBackoffMs * 2, MAX_BACKOFF_MS);

    if (this.#onReconnectNeeded) {
      try { this.#onReconnectNeeded(); } catch { /* callback error */ }
    }
  }

  #waitForInbox(type, timeoutSecs) {
    return new Promise((resolve) => {
      const timer = setTimeout(() => {
        // Remove this waiter
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
