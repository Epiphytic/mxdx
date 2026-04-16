/**
 * @deprecated This JS transport class is superseded by the Rust P2PTransport
 * state machine in `crates/mxdx-p2p/src/transport/`. The native Rust driver
 * is functional; the WASM driver is a stub pending full Phase 8 integration.
 * Scheduled for removal in T-C2 (cleanup phase).
 *
 * P2PTransport — adapter between TerminalSocket/BatchedSender and WebRTC data channel.
 *
 * Implements the same sendEvent/onRoomEvent interface as the Matrix client.
 * All terminal data is AES-256-GCM encrypted (via P2PCrypto) before placement
 * on the data channel. The session key is exchanged via E2EE Matrix signaling,
 * so peer identity is authenticated by the Megolm layer.
 *
 * Handshake protocol (v=2, storm §3.1):
 *   Step 1: Both sides exchange verify_challenge frames with nonces + device_id
 *   Step 2: Both sides sign the full transcript (Ed25519) and exchange signatures
 *   If peer sends v=1 (legacy), falls back to simple nonce ping-pong
 *
 * NEVER sends unencrypted terminal data over P2P.
 * Falls back to Matrix transparently on any P2P failure.
 */

console.warn('[mxdx] p2p-transport.js is deprecated — Rust P2PTransport will replace this');

import {
  generateNonce,
  generateEphemeralKeypair,
  buildTranscript,
  buildChallengeFrame,
  buildResponseFrame,
  parseChallengeFrame,
  parseResponseFrame,
  canonicalOrdering,
  canonicalSdpFingerprints,
  verifyTranscript,
  b64encode,
} from './p2p-verify.js';

/** Cross-platform random hex string (works in browser + Node 19+). */
function randomHex(byteCount) {
  const buf = new Uint8Array(byteCount);
  globalThis.crypto.getRandomValues(buf);
  return Array.from(buf, b => b.toString(16).padStart(2, '0')).join('');
}

/** Current handshake protocol version. */
const VERIFY_PROTOCOL_VERSION = 2;

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

  // Handshake context (v=2 Ed25519, storm §3.1)
  #roomId;
  #sessionUuid;
  #callId;
  #offerSdp;
  #answerSdp;
  #weAreOfferer;
  #peerDeviceEd25519Lookup;  // async (userId, deviceId) => Uint8Array|null
  #ephemeralKeypair = null;  // { privateKey, publicKey, publicKeyBytes }

  #dataChannel = null;
  #peerVerified = false;
  #status = 'matrix';
  #closed = false;

  // Peer verification state
  #localNonce = null;          // Uint8Array (32 bytes) for v=2, hex string for v=1
  #remoteNonce = null;
  #peerDeviceId = null;
  #localVerified = false;   // Remote acknowledged our challenge
  #verifyTimer = null;
  #peerProtocolVersion = null; // null until first frame received

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
    // v=2 handshake context (optional for backwards compat)
    roomId = null,
    sessionUuid = null,
    callId = null,
    offerSdp = null,
    answerSdp = null,
    weAreOfferer = false,
    peerDeviceEd25519Lookup = null,
  }) {
    this.#matrixClient = matrixClient;
    this.#p2pCrypto = p2pCrypto;
    this.#localDeviceId = localDeviceId;
    this.#idleTimeoutMs = idleTimeoutMs;
    this.#onStatusChange = onStatusChange;
    this.#onReconnectNeeded = onReconnectNeeded;
    this.#onHangup = onHangup;
    this.#roomId = roomId;
    this.#sessionUuid = sessionUuid;
    this.#callId = callId;
    this.#offerSdp = offerSdp;
    this.#answerSdp = answerSdp;
    this.#weAreOfferer = weAreOfferer;
    this.#peerDeviceEd25519Lookup = peerDeviceEd25519Lookup;
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
   * Update handshake context for v=2 Ed25519 verification. Call after
   * signaling completes (offer/answer SDPs known) but before setDataChannel.
   */
  setHandshakeContext({ roomId, sessionUuid, callId, offerSdp, answerSdp, weAreOfferer }) {
    this.#roomId = roomId;
    this.#sessionUuid = sessionUuid;
    this.#callId = callId;
    this.#offerSdp = offerSdp;
    this.#answerSdp = answerSdp;
    this.#weAreOfferer = weAreOfferer;
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
    this.#peerDeviceId = null;
    this.#peerProtocolVersion = null;
    this.#ephemeralKeypair = null;

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

    // Start peer verification automatically (fire-and-forget; errors are
    // handled inside by falling back to Matrix)
    this.#startVerification().catch((err) => {
      this.#log('[p2p-transport] startVerification failed:', err.message || err);
    });
  }

  #log(...args) {
    // Only log when P2P debug is enabled (reduces noise in production)
    if (typeof localStorage !== 'undefined' && localStorage.getItem('mxdx-p2p-debug') === 'true') {
      console.log(...args);
    } else if (typeof process !== 'undefined' && process.env?.MXDX_P2P_DEBUG === 'true') {
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
   * When P2P is active, terminal events stay on P2P (no Matrix fallthrough).
   */
  async onRoomEvent(roomId, type, timeoutSecs) {
    // Check P2P inbox first (immediate return)
    const inbox = this.#p2pInbox.get(type);
    if (inbox && inbox.length > 0) {
      return inbox.shift();
    }

    // When P2P is active, only wait on P2P inbox — no Matrix fallthrough.
    // Terminal data arrives via data channel; falling through to Matrix
    // doubles wait times for no benefit (Matrix events aren't coming).
    if (this.#status === 'p2p' && timeoutSecs > 0) {
      return this.#waitForInbox(type, timeoutSecs);
    }

    // Matrix-only path (P2P not active)
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

  // --- Peer Verification (v=2: Ed25519 transcript per storm §3.1) ---
  //
  // Protocol negotiation:
  // - v=2: Ed25519-signed transcript. Both sides exchange verify_challenge
  //   frames (with nonce + device_id + v=2 tag), then verify_response
  //   frames (with signature + public key).
  // - v=1 (legacy): simple nonce ping-pong via peer_verify/peer_verify_response.
  //   Accepted when peer sends a v=1 frame (no `v` field = v=1).

  /** Whether we have the context needed for v=2 handshake. */
  #canDoV2() {
    return this.#roomId && this.#callId && this.#offerSdp && this.#answerSdp;
  }

  async #startVerification() {
    if (this.#canDoV2()) {
      // v=2: Ed25519 handshake
      this.#ephemeralKeypair = await generateEphemeralKeypair();
      this.#localNonce = generateNonce(); // Uint8Array(32)
      const challenge = buildChallengeFrame(this.#localNonce, this.#localDeviceId);
      challenge.v = VERIFY_PROTOCOL_VERSION;
      this.#log('[p2p-transport] sending verify_challenge (v=2)');
      this.#sendControlFrame(challenge);
    } else {
      // v=1 fallback: legacy nonce ping-pong
      this.#localNonce = randomHex(32);
      this.#log('[p2p-transport] sending peer_verify (v=1 legacy)');
      this.#sendControlFrame({
        type: 'peer_verify',
        nonce: this.#localNonce,
        device_id: this.#localDeviceId,
      });
    }

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

  async #handleVerifyChallenge(frame) {
    // v=2 Ed25519 challenge received
    this.#peerProtocolVersion = frame.v || 1;
    this.#log('[p2p-transport] received verify_challenge v=', this.#peerProtocolVersion);

    const { nonce: peerNonce, deviceId: peerDeviceIdFromFrame } = parseChallengeFrame(frame);
    this.#remoteNonce = peerNonce;
    this.#peerDeviceId = peerDeviceIdFromFrame;

    if (!this.#ephemeralKeypair) {
      this.#ephemeralKeypair = await generateEphemeralKeypair();
    }
    if (!this.#localNonce) {
      this.#localNonce = generateNonce();
      // Send our challenge too
      const ourChallenge = buildChallengeFrame(this.#localNonce, this.#localDeviceId);
      ourChallenge.v = VERIFY_PROTOCOL_VERSION;
      this.#sendControlFrame(ourChallenge);
    }

    // Now build and send our response (we have both nonces)
    await this.#sendVerifyResponse();
  }

  async #sendVerifyResponse() {
    if (!this.#localNonce || !this.#remoteNonce || !this.#ephemeralKeypair) return;
    if (!this.#canDoV2()) return;

    try {
      const [offererSdpFp, answererSdpFp] = canonicalSdpFingerprints(
        this.#offerSdp, this.#answerSdp,
      );
      const [offNonce, ansNonce] = canonicalOrdering(
        this.#localNonce, this.#remoteNonce, this.#weAreOfferer,
      );
      const [offParty, ansParty] = canonicalOrdering(
        this.#localDeviceId, this.#peerDeviceId || '', this.#weAreOfferer,
      );

      const transcript = buildTranscript({
        roomId: this.#roomId,
        sessionUuid: this.#sessionUuid || '',
        callId: this.#callId,
        offererNonce: offNonce,
        answererNonce: ansNonce,
        offererPartyId: offParty,
        answererPartyId: ansParty,
        offererSdpFingerprint: offererSdpFp,
        answererSdpFingerprint: answererSdpFp,
      });

      // OlmMachine::sign takes base64-encoded transcript (matching Rust)
      const transcriptB64 = b64encode(transcript);
      const response = await buildResponseFrame({
        privateKey: this.#ephemeralKeypair.privateKey,
        publicKeyBytes: this.#ephemeralKeypair.publicKeyBytes,
        transcript: new TextEncoder().encode(transcriptB64),
        ourDeviceId: this.#localDeviceId,
      });
      response.v = VERIFY_PROTOCOL_VERSION;
      this.#sendControlFrame(response);
    } catch (err) {
      this.#log('[p2p-transport] verify response build failed:', err.message);
    }
  }

  async #handleVerifyResponse(frame) {
    this.#log('[p2p-transport] received verify_response');
    if (!this.#localNonce || !this.#remoteNonce) return;

    try {
      const { signature, signerPk, deviceId } = parseResponseFrame(frame);

      // Look up peer's known Ed25519 key from Matrix crypto store
      let knownPk = null;
      if (this.#peerDeviceEd25519Lookup) {
        knownPk = await this.#peerDeviceEd25519Lookup(
          null, // user_id — the lookup impl knows which user
          deviceId,
        );
      }

      // If we can verify against a known key, do so
      if (knownPk) {
        // Compare wire pk against Matrix-known pk
        if (knownPk.length !== signerPk.length ||
            !knownPk.every((b, i) => b === signerPk[i])) {
          this.#log('[p2p-transport] SECURITY: peer public key mismatch — aborting');
          this.#handleChannelClose();
          return;
        }
      }

      // Rebuild transcript and verify signature
      const [offererSdpFp, answererSdpFp] = canonicalSdpFingerprints(
        this.#offerSdp, this.#answerSdp,
      );
      const [offNonce, ansNonce] = canonicalOrdering(
        this.#localNonce, this.#remoteNonce, this.#weAreOfferer,
      );
      const [offParty, ansParty] = canonicalOrdering(
        this.#localDeviceId, this.#peerDeviceId || deviceId, this.#weAreOfferer,
      );

      const transcript = buildTranscript({
        roomId: this.#roomId,
        sessionUuid: this.#sessionUuid || '',
        callId: this.#callId,
        offererNonce: offNonce,
        answererNonce: ansNonce,
        offererPartyId: offParty,
        answererPartyId: ansParty,
        offererSdpFingerprint: offererSdpFp,
        answererSdpFingerprint: answererSdpFp,
      });

      const transcriptB64 = b64encode(transcript);
      const valid = await verifyTranscript(
        signerPk, signature, new TextEncoder().encode(transcriptB64),
      );
      if (!valid) {
        this.#log('[p2p-transport] SECURITY: signature verification failed — aborting');
        this.#handleChannelClose();
        return;
      }

      this.#localVerified = true;
      this.#checkVerificationComplete();
    } catch (err) {
      this.#log('[p2p-transport] verify response handling failed:', err.message);
    }
  }

  // v=1 legacy handlers
  #handlePeerVerify(frame) {
    this.#log('[p2p-transport] received peer_verify (v=1 legacy) from device:', frame.device_id);
    this.#peerProtocolVersion = 1;
    this.#remoteNonce = frame.nonce;
    this.#peerDeviceId = frame.device_id;
    if (!this.#remoteNonce || !this.#peerDeviceId) return;

    this.#sendControlFrame({
      type: 'peer_verify_response',
      nonce: this.#remoteNonce,
      device_id: this.#localDeviceId,
    });

    this.#checkVerificationComplete();
  }

  #handlePeerVerifyResponse(frame) {
    this.#log('[p2p-transport] received peer_verify_response (v=1 legacy)');
    if (!frame.nonce || !frame.device_id) return;
    if (frame.nonce !== this.#localNonce) return;

    this.#localVerified = true;
    this.#checkVerificationComplete();
  }

  #checkVerificationComplete() {
    this.#log('[p2p-transport] checkVerification: localVerified=', this.#localVerified, 'remoteNonce=', !!this.#remoteNonce);
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
      // v=2 Ed25519 handshake frames
      case 'verify_challenge':
        this.#handleVerifyChallenge(frame);
        break;
      case 'verify_response':
        this.#handleVerifyResponse(frame);
        break;
      // v=1 legacy nonce ping-pong
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
