import { createRequire } from 'node:module';
const require = createRequire(import.meta.url);
const nodeDataChannel = require('node-datachannel');

/**
 * Thin wrapper around node-datachannel PeerConnection for P2P data channels.
 * Exposes a platform-agnostic interface matching BrowserWebRTCChannel.
 *
 * Note: node-datachannel does NOT expose `icecandidateerror` — TURN failures
 * can only be detected via state becoming 'failed'.
 *
 * Uses createRequire() because ESM default import wraps native callbacks.
 *
 * IMPORTANT: node-datachannel requires onLocalDescription to be registered
 * BEFORE createDataChannel — otherwise the callback never fires.
 */
export class NodeWebRTCChannel {
  #pc;
  #dc = null;
  #iceCandidateCallbacks = [];
  #messageCallbacks = [];
  #closeCallbacks = [];
  #stateChangeCallbacks = [];
  #dcOpenResolvers = [];
  #closed = false;
  #remoteDescSet = false;
  #pendingCandidates = [];

  constructor({ iceServers = [] } = {}) {
    const iceServerStrs = [];
    for (const server of iceServers) {
      const urls = Array.isArray(server.urls) ? server.urls : [server.urls];
      for (const url of urls) {
        iceServerStrs.push(url);
      }
    }

    this.#pc = new nodeDataChannel.PeerConnection('mxdx', { iceServers: iceServerStrs });

    this.#pc.onLocalCandidate((candidate, mid) => {
      for (const cb of this.#iceCandidateCallbacks) {
        cb({ candidate, sdpMid: mid });
      }
    });

    this.#pc.onStateChange((state) => {
      for (const cb of this.#stateChangeCallbacks) {
        cb(state);
      }
    });

    this.#pc.onIceStateChange((state) => {
      for (const cb of this.#stateChangeCallbacks) {
        cb(state);
      }
    });

    // Handle incoming data channel (answerer side)
    this.#pc.onDataChannel((dc) => {
      this.#setupDataChannel(dc);
    });
  }

  #setupDataChannel(dc) {
    this.#dc = dc;

    dc.onOpen(() => {
      for (const resolve of this.#dcOpenResolvers) {
        resolve();
      }
      this.#dcOpenResolvers = [];
    });

    dc.onMessage((msg) => {
      const str = typeof msg === 'string' ? msg : msg.toString('utf8');
      for (const cb of this.#messageCallbacks) {
        cb(str);
      }
    });

    dc.onClosed(() => {
      for (const cb of this.#closeCallbacks) {
        cb();
      }
    });

    dc.onError((err) => {
      for (const cb of this.#closeCallbacks) {
        cb(err);
      }
    });

    if (dc.isOpen()) {
      for (const resolve of this.#dcOpenResolvers) {
        resolve();
      }
      this.#dcOpenResolvers = [];
    }
  }

  #drainPendingCandidates() {
    for (const c of this.#pendingCandidates) {
      this.#pc.addRemoteCandidate(c.candidate, c.sdpMid || '0');
    }
    this.#pendingCandidates = [];
  }

  async createOffer() {
    return new Promise((resolve) => {
      // IMPORTANT: onLocalDescription must be registered BEFORE createDataChannel
      this.#pc.onLocalDescription((sdp, type) => {
        resolve({ sdp, type });
      });

      const dc = this.#pc.createDataChannel('mxdx-terminal');
      this.#setupDataChannel(dc);
      this.#pc.setLocalDescription();
    });
  }

  async acceptOffer(offer) {
    this.#pc.setRemoteDescription(offer.sdp, offer.type);
    this.#remoteDescSet = true;
    this.#drainPendingCandidates();

    // node-datachannel auto-generates the answer when setting a remote offer
    const desc = this.#pc.localDescription();
    return { sdp: desc.sdp, type: desc.type };
  }

  async acceptAnswer(answer) {
    this.#pc.setRemoteDescription(answer.sdp, answer.type);
    this.#remoteDescSet = true;
    this.#drainPendingCandidates();
  }

  addIceCandidate(candidate) {
    if (this.#closed) return;
    if (!this.#remoteDescSet) {
      this.#pendingCandidates.push(candidate);
      return;
    }
    this.#pc.addRemoteCandidate(candidate.candidate, candidate.sdpMid || '0');
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

  onStateChange(cb) {
    this.#stateChangeCallbacks.push(cb);
  }

  send(data) {
    if (!this.#dc || !this.#dc.isOpen()) {
      throw new Error('Data channel not open');
    }
    this.#dc.sendMessage(data);
  }

  waitForDataChannel() {
    if (this.#dc && this.#dc.isOpen()) {
      return Promise.resolve();
    }
    return new Promise((resolve) => {
      this.#dcOpenResolvers.push(resolve);
    });
  }

  get isOpen() {
    return this.#dc !== null && this.#dc.isOpen();
  }

  close() {
    this.#closed = true;
    try {
      if (this.#dc) this.#dc.close();
    } catch { /* already closed */ }
    try {
      this.#pc.close();
    } catch { /* already closed */ }
  }
}
