/**
 * Browser WebRTC channel wrapper using native RTCPeerConnection.
 * Same interface as NodeWebRTCChannel, plus onIceCandidateError for TURN error detection.
 *
 * TURN error codes (browser-only):
 *   486 — Allocation Quota Reached
 *   508 — Insufficient Capacity
 *   701 — Cannot reach TURN server
 */
export class BrowserWebRTCChannel {
  #pc;
  #dc = null;
  #iceCandidateCallbacks = [];
  #iceCandidateErrorCallbacks = [];
  #messageCallbacks = [];
  #messageBuffer = []; // Buffer messages before any handler is registered
  #closeCallbacks = [];
  #stateChangeCallbacks = [];
  #dcOpenResolvers = [];
  #closed = false;

  constructor({ iceServers = [], turnOnly = false } = {}) {
    const config = { iceServers };
    if (turnOnly) config.iceTransportPolicy = 'relay';
    this.#pc = new RTCPeerConnection(config);

    this.#pc.onicecandidate = (event) => {
      if (event.candidate) {
        for (const cb of this.#iceCandidateCallbacks) {
          cb({
            candidate: event.candidate.candidate,
            sdpMid: event.candidate.sdpMid,
            sdpMLineIndex: event.candidate.sdpMLineIndex,
          });
        }
      }
    };

    this.#pc.addEventListener('icecandidateerror', (event) => {
      for (const cb of this.#iceCandidateErrorCallbacks) {
        cb({
          errorCode: event.errorCode,
          errorText: event.errorText,
          url: event.url,
        });
      }
    });

    this.#pc.oniceconnectionstatechange = () => {
      for (const cb of this.#stateChangeCallbacks) {
        cb(this.#pc.iceConnectionState);
      }
    };

    // Handle incoming data channel (answerer side)
    this.#pc.ondatachannel = (event) => {
      this.#setupDataChannel(event.channel);
    };
  }

  #setupDataChannel(dc) {
    this.#dc = dc;

    dc.onopen = () => {
      for (const resolve of this.#dcOpenResolvers) {
        resolve();
      }
      this.#dcOpenResolvers = [];
    };

    dc.onmessage = (event) => {
      const data = typeof event.data === 'string' ? event.data : new TextDecoder().decode(event.data);
      if (this.#messageCallbacks.length === 0) {
        // Buffer messages until a handler is registered
        this.#messageBuffer.push(data);
      } else {
        for (const cb of this.#messageCallbacks) {
          cb(data);
        }
      }
    };

    dc.onclose = () => {
      for (const cb of this.#closeCallbacks) {
        cb();
      }
    };

    dc.onerror = (err) => {
      for (const cb of this.#closeCallbacks) {
        cb(err);
      }
    };

    if (dc.readyState === 'open') {
      for (const resolve of this.#dcOpenResolvers) {
        resolve();
      }
      this.#dcOpenResolvers = [];
    }
  }

  async createOffer() {
    const dc = this.#pc.createDataChannel('mxdx-terminal', {
      ordered: true,
    });
    this.#setupDataChannel(dc);

    const offer = await this.#pc.createOffer();
    await this.#pc.setLocalDescription(offer);
    return { sdp: offer.sdp, type: offer.type };
  }

  async acceptOffer(offer) {
    await this.#pc.setRemoteDescription(new RTCSessionDescription(offer));
    const answer = await this.#pc.createAnswer();
    await this.#pc.setLocalDescription(answer);
    return { sdp: answer.sdp, type: answer.type };
  }

  async acceptAnswer(answer) {
    await this.#pc.setRemoteDescription(new RTCSessionDescription(answer));
  }

  addIceCandidate(candidate) {
    if (this.#closed) return;
    this.#pc.addIceCandidate(new RTCIceCandidate({
      candidate: candidate.candidate,
      sdpMid: candidate.sdpMid,
      sdpMLineIndex: candidate.sdpMLineIndex,
    })).catch(() => { /* may be closed */ });
  }

  onIceCandidate(cb) {
    this.#iceCandidateCallbacks.push(cb);
  }

  /** Browser-only: TURN error detection with STUN error codes */
  onIceCandidateError(cb) {
    this.#iceCandidateErrorCallbacks.push(cb);
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
    if (!this.#dc || this.#dc.readyState !== 'open') {
      throw new Error('Data channel not open');
    }
    this.#dc.send(data);
  }

  waitForDataChannel() {
    if (this.#dc && this.#dc.readyState === 'open') {
      return Promise.resolve();
    }
    return new Promise((resolve) => {
      this.#dcOpenResolvers.push(resolve);
    });
  }

  get isOpen() {
    return this.#dc !== null && this.#dc.readyState === 'open';
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
