const COMPRESSION_THRESHOLD = 32;

const textEncoder = new TextEncoder();

function concatBytes(arrays) {
  let total = 0;
  for (const a of arrays) total += a.byteLength;
  const result = new Uint8Array(total);
  let offset = 0;
  for (const a of arrays) {
    result.set(a, offset);
    offset += a.byteLength;
  }
  return result;
}

function base64Encode(data) {
  if (typeof Buffer !== 'undefined') {
    return Buffer.from(data).toString('base64');
  }
  let binary = '';
  for (let i = 0; i < data.length; i++) {
    binary += String.fromCharCode(data[i]);
  }
  return btoa(binary);
}

function parseRetryAfter(errStr) {
  const match = errStr.match(/retry_after_ms["\s:]+(\d+)/);
  return match ? parseInt(match[1], 10) + 100 : 2000;
}

/**
 * Default compress function — uses CompressionStream (browser) or node:zlib.
 * Returns { encoded: string, encoding: string }.
 */
async function defaultCompress(data) {
  if (data.byteLength < COMPRESSION_THRESHOLD) {
    return { encoded: base64Encode(data), encoding: 'base64' };
  }

  let compressed;
  if (typeof globalThis.CompressionStream !== 'undefined') {
    const cs = new CompressionStream('deflate');
    const writer = cs.writable.getWriter();
    const reader = cs.readable.getReader();
    writer.write(data);
    writer.close();
    const chunks = [];
    let totalLength = 0;
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      chunks.push(value);
      totalLength += value.length;
    }
    compressed = new Uint8Array(totalLength);
    let offset = 0;
    for (const chunk of chunks) {
      compressed.set(chunk, offset);
      offset += chunk.length;
    }
  } else {
    const { deflateSync } = await import('node:zlib');
    compressed = new Uint8Array(deflateSync(Buffer.from(data)));
  }

  return { encoded: base64Encode(compressed), encoding: 'zlib+base64' };
}

/**
 * BatchedSender — rate-limit-aware event batching for Matrix.
 *
 * Collects raw byte chunks over a configurable time window, concatenates them,
 * compresses, and sends as one Matrix event. On 429 (rate limit), waits then
 * coalesces all queued + newly buffered data into a single message before retry.
 *
 * Works in both Node.js and browser environments.
 *
 * @param {object} options
 * @param {function} options.sendEvent - async (roomId, eventType, contentJson) => void
 * @param {string} options.roomId - Matrix room ID
 * @param {string} [options.eventType='org.mxdx.terminal.data'] - Event type
 * @param {number} [options.batchMs=200] - Batch window in milliseconds
 * @param {function} [options.compress] - async (Uint8Array) => { encoded, encoding }
 * @param {function} [options.onError] - Called on non-retryable send errors
 * @param {function} [options.onBuffering] - Called with (true) when rate-limited, (false) when cleared
 */
export class BatchedSender {
  #sendEvent;
  #compress;
  #roomId;
  #eventType;
  #seq = 0;
  #buffer = [];
  #flushTimer = null;
  #sending = false;
  #queue = [];
  #batchMs;
  #destroyed = false;
  #onError;
  #onBuffering;
  #buffering = false;

  constructor({
    sendEvent,
    roomId,
    eventType = 'org.mxdx.terminal.data',
    batchMs = 200,
    compress = defaultCompress,
    onError = null,
    onBuffering = null,
  }) {
    this.#sendEvent = sendEvent;
    this.#roomId = roomId;
    this.#eventType = eventType;
    this.#batchMs = batchMs;
    this.#compress = compress;
    this.#onError = onError;
    this.#onBuffering = onBuffering;
  }

  /**
   * Push data to be sent. Accepts string, Buffer, or Uint8Array.
   * Data is buffered and sent after the batch window expires.
   */
  push(data) {
    if (this.#destroyed) return;
    const bytes = typeof data === 'string'
      ? textEncoder.encode(data)
      : new Uint8Array(data);
    this.#buffer.push(bytes);
    if (!this.#flushTimer) {
      this.#flushTimer = setTimeout(() => this.#flush(), this.#batchMs);
    }
  }

  #flush() {
    this.#flushTimer = null;
    if (this.#buffer.length === 0) return;
    const combined = concatBytes(this.#buffer);
    this.#buffer = [];
    const seq = this.#seq++;
    this.#queue.push({ data: combined, seq });
    this.#drain();
  }

  async #drain() {
    if (this.#sending || this.#destroyed) return;
    this.#sending = true;

    while (this.#queue.length > 0 && !this.#destroyed) {
      // Coalesce: if multiple items queued, combine into one event
      if (this.#queue.length > 1) {
        const combined = concatBytes(this.#queue.map((q) => q.data));
        const lastSeq = this.#queue[this.#queue.length - 1].seq;
        this.#queue = [{ data: combined, seq: lastSeq }];
      }

      const { data, seq } = this.#queue[0];
      const { encoded, encoding } = await this.#compress(data);

      try {
        await this.#sendEvent(
          this.#roomId,
          this.#eventType,
          JSON.stringify({ data: encoded, encoding, seq }),
        );
        this.#queue.shift();
        // Clear buffering state after successful send
        if (this.#buffering) {
          this.#buffering = false;
          if (this.#onBuffering) this.#onBuffering(false);
        }
      } catch (err) {
        const errStr = String(err);
        if (errStr.includes('429') || errStr.includes('M_LIMIT_EXCEEDED')) {
          const wait = parseRetryAfter(errStr);

          // Signal buffering state
          if (!this.#buffering) {
            this.#buffering = true;
            if (this.#onBuffering) this.#onBuffering(true);
          }

          // Wait for rate limit to clear
          await new Promise((r) => setTimeout(r, wait));

          // After waiting, pull any new buffer data into queue for coalescing
          if (this.#flushTimer) {
            clearTimeout(this.#flushTimer);
            this.#flushTimer = null;
          }
          if (this.#buffer.length > 0) {
            const newData = concatBytes(this.#buffer);
            this.#buffer = [];
            this.#queue.push({ data: newData, seq: this.#seq++ });
          }
          // Next loop iteration will coalesce all queue items
        } else {
          // Non-retryable error — drop and report
          this.#queue.shift();
          if (this.#onError) this.#onError(err, seq);
        }
      }
    }

    this.#sending = false;
  }

  /** Flush any remaining buffer immediately and wait for drain. */
  async flush() {
    if (this.#flushTimer) {
      clearTimeout(this.#flushTimer);
      this.#flushTimer = null;
    }
    if (this.#buffer.length > 0) {
      const combined = concatBytes(this.#buffer);
      this.#buffer = [];
      const seq = this.#seq++;
      this.#queue.push({ data: combined, seq });
    }
    // Wait for drain to complete
    if (this.#queue.length > 0 && !this.#sending) {
      await this.#drain();
    }
  }

  destroy() {
    this.#destroyed = true;
    if (this.#flushTimer) {
      clearTimeout(this.#flushTimer);
      this.#flushTimer = null;
    }
    this.#buffer = [];
    this.#queue = [];
  }

  get batchMs() { return this.#batchMs; }
  set batchMs(ms) { this.#batchMs = ms; }
  get queueLength() { return this.#queue.length; }
}
