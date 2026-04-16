/**
 * @deprecated This JS shim is superseded by the Rust signaling module in
 * `crates/mxdx-p2p/src/signaling/`. WASM P2PTransport will subsume signaling
 * orchestration. Scheduled for removal in T-C2 (cleanup phase).
 */

console.warn('[mxdx] p2p-signaling.js is deprecated — Rust P2PTransport will replace this');

/** Cross-platform random hex string (works in browser + Node 19+). */
function randomHex(byteCount) {
  const buf = new Uint8Array(byteCount);
  globalThis.crypto.getRandomValues(buf);
  return Array.from(buf, b => b.toString(16).padStart(2, '0')).join('');
}

/**
 * P2P signaling layer using standard Matrix VoIP call events (m.call.*).
 * Sends/receives SDP offers, answers, ICE candidates, and hangup events
 * through the Matrix room, enabling WebRTC peer connection establishment.
 */
export class P2PSignaling {
  #client;
  #roomId;
  #localUserId;
  #eventHandlers = new Map();
  #unsubscribe = null;

  /**
   * @param {object} client - Matrix client with sendEvent(roomId, type, contentJson) and onRoomEvent(roomId, cb)
   * @param {string} roomId - DM room for signaling
   * @param {string} localUserId - Our Matrix user ID (for glare resolution)
   */
  constructor(client, roomId, localUserId) {
    this.#client = client;
    this.#roomId = roomId;
    this.#localUserId = localUserId;
  }

  /** Generate a random call ID (16 hex chars). */
  static generateCallId() {
    return randomHex(8);
  }

  /** Generate a random party ID (8 hex chars). */
  static generatePartyId() {
    return randomHex(4);
  }

  /**
   * Send m.call.invite with SDP offer.
   *
   * The `mxdx_session_key` extension field (base64 AES-256 key) is protected
   * by room E2EE (Megolm + MSC4362). See ADR
   * docs/adr/2026-04-15-mcall-wire-format.md (and its 2026-04-16 addendum)
   * plus docs/adr/2026-04-16-coordinated-rust-npm-releases.md — this wire
   * shape is locked in step with crates/mxdx-p2p/src/signaling/events.rs
   * (Rust emitter). Default lifetime is 30000 ms on both sides.
   *
   * @param {{ callId: string, partyId: string, sdp: string, lifetime?: number, sessionKey?: string|null }} opts
   */
  async sendInvite({ callId, partyId, sdp, lifetime = 30000, sessionKey = null }) {
    const content = {
      call_id: callId,
      party_id: partyId,
      version: '1',
      lifetime,
      offer: { type: 'offer', sdp },
    };
    if (sessionKey) content.mxdx_session_key = sessionKey;
    await this.#send('m.call.invite', content);
  }

  /**
   * Send m.call.answer with SDP answer.
   * @param {{ callId: string, partyId: string, sdp: string }} opts
   */
  async sendAnswer({ callId, partyId, sdp }) {
    await this.#send('m.call.answer', {
      call_id: callId,
      party_id: partyId,
      version: '1',
      answer: { type: 'answer', sdp },
    });
  }

  /**
   * Send m.call.candidates with batched ICE candidates.
   * @param {{ callId: string, partyId: string, candidates: Array<{candidate: string, sdpMid: string}> }} opts
   */
  async sendCandidates({ callId, partyId, candidates }) {
    await this.#send('m.call.candidates', {
      call_id: callId,
      party_id: partyId,
      version: '1',
      candidates,
    });
  }

  /**
   * Send m.call.hangup to terminate the P2P channel.
   * @param {{ callId: string, partyId: string, reason?: string }} opts
   */
  async sendHangup({ callId, partyId, reason = 'user_hangup' }) {
    await this.#send('m.call.hangup', {
      call_id: callId,
      party_id: partyId,
      version: '1',
      reason,
    });
  }

  /**
   * Send m.call.select_answer for glare resolution.
   * @param {{ callId: string, partyId: string, selectedPartyId: string }} opts
   */
  async sendSelectAnswer({ callId, partyId, selectedPartyId }) {
    await this.#send('m.call.select_answer', {
      call_id: callId,
      party_id: partyId,
      version: '1',
      selected_party_id: selectedPartyId,
    });
  }

  /**
   * Resolve glare per Matrix spec: lower user_id (lexicographic) wins.
   * @param {string} remoteUserId
   * @returns {'win' | 'lose'}
   */
  resolveGlare(remoteUserId) {
    return this.#localUserId < remoteUserId ? 'win' : 'lose';
  }

  /**
   * Register a handler for incoming call events.
   * @param {string} eventType - e.g. 'm.call.invite'
   * @param {function} handler - Called with parsed event content
   */
  on(eventType, handler) {
    if (!this.#eventHandlers.has(eventType)) {
      this.#eventHandlers.set(eventType, []);
    }
    this.#eventHandlers.get(eventType).push(handler);
  }

  /**
   * Start listening for room events. Call once after registering handlers.
   */
  async startListening() {
    if (!this.#client) return;
    const result = await this.#client.onRoomEvent(this.#roomId, (type, content) => {
      const handlers = this.#eventHandlers.get(type);
      if (handlers) {
        const parsed = typeof content === 'string' ? JSON.parse(content) : content;
        for (const handler of handlers) {
          handler(parsed);
        }
      }
    });
    if (result && typeof result === 'function') {
      this.#unsubscribe = result;
    }
  }

  /**
   * Stop listening for room events.
   */
  stopListening() {
    if (this.#unsubscribe) {
      this.#unsubscribe();
      this.#unsubscribe = null;
    }
  }

  async #send(type, content) {
    await this.#client.sendEvent(this.#roomId, type, JSON.stringify(content));
  }
}
