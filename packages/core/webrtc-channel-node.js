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
  #messageBuffer = []; // Buffer messages before any handler is registered
  #closeCallbacks = [];
  #stateChangeCallbacks = [];
  #dcOpenResolvers = [];
  #closed = false;
  #remoteDescSet = false;
  #pendingCandidates = [];

  constructor({ iceServers = [], turnOnly = false } = {}) {
    // Convert browser-format iceServers ({urls, username, credential}) to
    // node-datachannel format ({hostname, port, username, password, relayType})
    const ndcServers = [];
    for (const server of iceServers) {
      const urls = Array.isArray(server.urls) ? server.urls : [server.urls];
      for (const url of urls) {
        // In turnOnly mode, skip STUN servers — only use TURN (relay) servers
        if (turnOnly && !url.startsWith('turn:') && !url.startsWith('turns:')) continue;
        try {
          // Parse turn:host:port?transport=xxx or turns:host:port?transport=xxx
          const match = url.match(/^(turns?|stun):([^:?]+):(\d+)/);
          if (!match) continue;
          const [, scheme, hostname, port] = match;
          const transport = url.includes('transport=tcp') ? 'tcp' : 'udp';
          const relayType = scheme === 'turns' ? 'TurnTls'
            : transport === 'tcp' ? 'TurnTcp' : 'TurnUdp';
          ndcServers.push({
            hostname,
            port: parseInt(port, 10),
            username: server.username || '',
            password: server.credential || '',
            relayType: scheme === 'stun' ? undefined : relayType,
          });
        } catch { /* skip malformed */ }
      }
    }

    const config = { iceServers: ndcServers };
    if (turnOnly) config.iceTransportPolicy = 'relay';
    this.#pc = new nodeDataChannel.PeerConnection('mxdx', config);

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
      if (this.#messageCallbacks.length === 0) {
        // Buffer messages until a handler is registered
        this.#messageBuffer.push(str);
      } else {
        for (const cb of this.#messageCallbacks) {
          cb(str);
        }
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
    // Flush any messages that arrived before handler was registered
    if (this.#messageBuffer.length > 0) {
      const buffered = this.#messageBuffer.splice(0);
      for (const msg of buffered) {
        cb(msg);
      }
    }
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
