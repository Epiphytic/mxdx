import { TerminalDataEvent } from './terminal-types.js';
import { BatchedSender } from './batched-sender.js';

const MAX_DECOMPRESSED_SIZE = 1024 * 1024; // 1MB zlib bomb protection

function base64Decode(str) {
  if (typeof Buffer !== 'undefined') {
    return new Uint8Array(Buffer.from(str, 'base64'));
  }
  const binary = atob(str);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes;
}

async function decompress(data) {
  // Use node:zlib with bounded decompression for safety
  const { inflateSync } = await import('node:zlib');
  const result = inflateSync(Buffer.from(data), { maxOutputLength: MAX_DECOMPRESSED_SIZE });
  return new Uint8Array(result);
}

/**
 * TerminalSocket — WebSocket-like interface over Matrix events.
 *
 * Wraps a WasmMatrixClient to send/receive terminal data in a DM room.
 * Uses BatchedSender for rate-limit-aware sending with coalescing,
 * sequence numbers for ordering, and gap detection with retransmit requests.
 */
export class TerminalSocket {
  binaryType = 'arraybuffer';

  onmessage = null;
  onclose = null;
  onerror = null;

  #client;
  #roomId;
  #sessionId;
  #expectedSeq = 0;
  #buffer = [];
  #closed = false;
  #pollTimer = null;
  #pollInterval;
  #gapTimer = null;
  #retransmitTimer = null;
  #sender = null;

  /**
   * @param {object} client - WasmMatrixClient instance with sendEvent/onRoomEvent
   * @param {string} roomId - Matrix room ID for terminal I/O
   * @param {object} [options]
   * @param {number} [options.pollIntervalMs=200] - Poll interval for incoming events
   * @param {number} [options.batchMs=200] - Send batch window in ms
   * @param {string} [options.sessionId] - Session ID for multiplexing (filter incoming, tag outgoing)
   * @param {function} [options.onBuffering] - Called with true/false when rate-limit buffering starts/stops
   */
  constructor(client, roomId, { pollIntervalMs = 200, batchMs = 200, sessionId = null, onBuffering = null } = {}) {
    this.#client = client;
    this.#roomId = roomId;
    this.#sessionId = sessionId;
    this.#pollInterval = pollIntervalMs;

    this.#sender = new BatchedSender({
      sendEvent: (rid, type, content) => client.sendEvent(rid, type, content),
      roomId,
      batchMs,
      sessionId,
      onError: (err) => {
        if (this.onerror) this.onerror(err);
      },
    });

    this.#startPolling();
  }

  #startPolling() {
    const poll = async () => {
      if (this.#closed) return;
      try {
        const eventJson = await this.#client.onRoomEvent(
          this.#roomId,
          'org.mxdx.terminal.data',
          1,
        );
        if (eventJson != null) {
          const event = JSON.parse(eventJson);
          const content = event.content || event;
          this.#handleIncomingData(content);
        }
      } catch {
        // Sync error, will retry on next poll
      }
      if (!this.#closed) {
        this.#pollTimer = setTimeout(poll, this.#pollInterval);
      }
    };
    this.#pollTimer = setTimeout(poll, 0);
  }

  #handleIncomingData(content) {
    const parsed = TerminalDataEvent.safeParse(content);
    if (!parsed.success) return;

    // Filter by session_id when multiplexing
    if (this.#sessionId && parsed.data.session_id && parsed.data.session_id !== this.#sessionId) return;

    const { data, encoding, seq } = parsed.data;
    this.#decodeAndEmit(data, encoding, seq);
  }

  #decodeAndEmit(data, encoding, seq) {
    const raw = base64Decode(data);

    if (encoding === 'zlib+base64') {
      decompress(raw).then((decompressed) => {
        this.#enqueue(seq, decompressed);
        this.#flush();
      }).catch(() => {
        // Decompression failed (possibly zlib bomb), skip event
      });
    } else {
      this.#enqueue(seq, raw);
      this.#flush();
    }
  }

  #enqueue(seq, data) {
    if (seq < this.#expectedSeq) return; // duplicate
    this.#buffer.push({ seq, data });
    this.#buffer.sort((a, b) => a.seq - b.seq);
  }

  #flush() {
    if (this.#buffer.length > 0 && this.#buffer[0].seq === this.#expectedSeq) {
      this.#clearGapTimers();
    }

    while (this.#buffer.length > 0 && this.#buffer[0].seq === this.#expectedSeq) {
      const event = this.#buffer.shift();
      this.#expectedSeq++;
      if (this.onmessage) {
        this.onmessage({ data: event.data.buffer.slice(
          event.data.byteOffset,
          event.data.byteOffset + event.data.byteLength,
        ) });
      }
    }

    if (this.#buffer.length > 0 && this.#buffer[0].seq > this.#expectedSeq && !this.#gapTimer && !this.#retransmitTimer) {
      this.#startGapTimer();
    }
  }

  #startGapTimer() {
    this.#gapTimer = setTimeout(() => {
      this.#gapTimer = null;
      if (this.#closed || this.#buffer.length === 0) return;

      const firstBuffered = this.#buffer[0].seq;
      if (firstBuffered <= this.#expectedSeq) {
        this.#flush();
        return;
      }

      const from_seq = this.#expectedSeq;
      const to_seq = firstBuffered - 1;
      this.#client.sendEvent(
        this.#roomId,
        'org.mxdx.terminal.retransmit',
        JSON.stringify({ from_seq, to_seq }),
      ).catch(() => {});

      this.#retransmitTimer = setTimeout(() => {
        this.#retransmitTimer = null;
        if (this.#closed || this.#buffer.length === 0) return;
        this.#acceptGap();
      }, 500);
    }, 500);
  }

  #acceptGap() {
    if (this.#buffer.length === 0) return;
    this.#expectedSeq = this.#buffer[0].seq;
    this.#flush();
  }

  #clearGapTimers() {
    if (this.#gapTimer) {
      clearTimeout(this.#gapTimer);
      this.#gapTimer = null;
    }
    if (this.#retransmitTimer) {
      clearTimeout(this.#retransmitTimer);
      this.#retransmitTimer = null;
    }
  }

  async send(data) {
    if (this.#closed) throw new Error('TerminalSocket is closed');
    this.#sender.push(data);
  }

  async resize(cols, rows) {
    if (this.#closed) throw new Error('TerminalSocket is closed');

    const payload = { cols, rows };
    if (this.#sessionId) payload.session_id = this.#sessionId;
    await this.#client.sendEvent(
      this.#roomId,
      'org.mxdx.terminal.resize',
      JSON.stringify(payload),
    );
  }

  close() {
    if (this.#closed) return;
    this.#closed = true;

    if (this.#sender) this.#sender.destroy();

    if (this.#pollTimer) {
      clearTimeout(this.#pollTimer);
      this.#pollTimer = null;
    }

    this.#clearGapTimers();

    if (this.onclose) {
      this.onclose({ code: 1000, reason: 'Normal closure' });
    }
  }

  get connected() {
    return !this.#closed;
  }

  get closed() {
    return this.#closed;
  }

  get roomId() {
    return this.#roomId;
  }
}
