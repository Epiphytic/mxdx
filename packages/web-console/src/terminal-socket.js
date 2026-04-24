import { z } from 'zod';
import { BatchedSender } from '../../core/batched-sender.js';

const TerminalDataEvent = z.object({
  data: z.string(),
  encoding: z.string(),
  seq: z.number().int().nonnegative(),
});

const MAX_DECOMPRESSED_SIZE = 1024 * 1024; // 1MB zlib bomb protection

function base64Decode(str) {
  const binary = atob(str);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes;
}

async function decompress(data) {
  const ds = new DecompressionStream('deflate');
  const writer = ds.writable.getWriter();
  const reader = ds.readable.getReader();
  writer.write(data);
  writer.close();
  const chunks = [];
  let totalLength = 0;
  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    chunks.push(value);
    totalLength += value.length;
    if (totalLength > MAX_DECOMPRESSED_SIZE) {
      throw new Error('Decompressed data exceeds maximum size');
    }
  }
  const result = new Uint8Array(totalLength);
  let offset = 0;
  for (const chunk of chunks) {
    result.set(chunk, offset);
    offset += chunk.length;
  }
  return result;
}

/**
 * Browser-compatible TerminalSocket over Matrix events.
 */
export class TerminalSocket {
  binaryType = 'arraybuffer';

  onmessage = null;
  onclose = null;
  onerror = null;
  onbuffering = null;

  #client;
  #roomId;
  #expectedSeq = 0;
  #buffer = [];
  #closed = false;
  #pollTimer = null;
  #pollInterval;
  #gapTimer = null;
  #retransmitTimer = null;
  #sender = null;
  #sessionId = null;

  constructor(client, roomId, { pollIntervalMs = 200, batchMs = 200, sessionId = null } = {}) {
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
      onBuffering: (buffering) => {
        if (this.onbuffering) this.onbuffering(buffering);
      },
    });

    this.#startPolling();
  }

  #startPolling() {
    const poll = async () => {
      if (this.#closed) return;
      let gotData = false;
      try {
        const eventJson = await this.#client.onRoomEvent(
          this.#roomId,
          'org.mxdx.terminal.data',
          1,
        );
        if (eventJson != null) {
          gotData = true;
          const event = JSON.parse(eventJson);
          const content = event.content || event;
          this.#handleIncomingData(content);
        }
      } catch {
        // Sync error, retry
      }
      if (!this.#closed) {
        // Re-poll immediately when data was received (burst mode);
        // use pollInterval only when idle to avoid busy-spinning.
        this.#pollTimer = setTimeout(poll, gotData ? 0 : this.#pollInterval);
      }
    };
    this.#pollTimer = setTimeout(poll, 0);
  }

  #handleIncomingData(content) {
    const parsed = TerminalDataEvent.safeParse(content);
    if (!parsed.success) return;
    const { data, encoding, seq } = parsed.data;
    this.#decodeAndEmit(data, encoding, seq);
  }

  #decodeAndEmit(data, encoding, seq) {
    const raw = base64Decode(data);
    if (encoding === 'zlib+base64') {
      decompress(raw).then((decompressed) => {
        this.#enqueue(seq, decompressed);
        this.#flush();
      }).catch(() => {});
    } else {
      this.#enqueue(seq, raw);
      this.#flush();
    }
  }

  #enqueue(seq, data) {
    if (seq < this.#expectedSeq) return;
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
      if (firstBuffered <= this.#expectedSeq) { this.#flush(); return; }
      const from_seq = this.#expectedSeq;
      const to_seq = firstBuffered - 1;
      this.#client.sendEvent(
        this.#roomId, 'org.mxdx.terminal.retransmit',
        JSON.stringify({ from_seq, to_seq }),
      ).catch(() => {});
      this.#retransmitTimer = setTimeout(() => {
        this.#retransmitTimer = null;
        if (this.#closed || this.#buffer.length === 0) return;
        this.#expectedSeq = this.#buffer[0].seq;
        this.#flush();
      }, 500);
    }, 500);
  }

  #clearGapTimers() {
    if (this.#gapTimer) { clearTimeout(this.#gapTimer); this.#gapTimer = null; }
    if (this.#retransmitTimer) { clearTimeout(this.#retransmitTimer); this.#retransmitTimer = null; }
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
      this.#roomId, 'org.mxdx.terminal.resize',
      JSON.stringify(payload),
    );
  }

  close() {
    if (this.#closed) return;
    this.#closed = true;
    if (this.#sender) this.#sender.destroy();
    if (this.#pollTimer) { clearTimeout(this.#pollTimer); this.#pollTimer = null; }
    this.#clearGapTimers();
    if (this.onclose) this.onclose({ code: 1000, reason: 'Normal closure' });
  }

  /** Adjust send batching (lower = faster for P2P, higher = rate-limit friendly for Matrix). */
  set batchMs(ms) { if (this.#sender) this.#sender.batchMs = ms; }
  get batchMs() { return this.#sender ? this.#sender.batchMs : 0; }

  get connected() { return !this.#closed; }
  get closed() { return this.#closed; }
  get roomId() { return this.#roomId; }
}
