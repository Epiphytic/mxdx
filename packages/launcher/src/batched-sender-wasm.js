// Rust equivalent: crates/mxdx-core-wasm/src/lib.rs::WasmBatchedSender
//
// Thin OS-bound JS shim around `WasmBatchedSender`. WASM owns:
//   - PTY chunk buffering, zlib+base64 compression, seq numbering
//   - 429 retry-with-coalesce state (in-flight payload, rate-limited flag,
//     `parseRetryAfterMs` for retry-after extraction)
//
// This shim owns:
//   - The `setTimeout` driving the batch window (Node/browser API, not in WASM)
//   - The actual Matrix `sendEvent` call (E2EE send path stays in
//     `WasmMatrixClient`, which this module is not allowed to bypass —
//     CLAUDE.md "EVERY MATRIX EVENT MUST BE END-TO-END ENCRYPTED")
//   - The drain serialisation lock (no concurrent sends)
//   - `onError` / `onBuffering` user callbacks (host-side telemetry)
//
// Implements the same JS surface as the legacy
// `packages/core/batched-sender.js::BatchedSender` so callers can swap
// imports with no other changes. The legacy module is retained for
// `packages/core/terminal-socket.js` and `packages/web-console/src/terminal-socket.js`,
// which still consume it; migrating those is a follow-up (filed as
// `brains:cleanup`).

import { WasmBatchedSender } from '@mxdx/core';

const textEncoder = new TextEncoder();

/**
 * Drop-in replacement for the legacy JS `BatchedSender` that delegates
 * compression, sequencing, and 429 retry/coalesce semantics to
 * `WasmBatchedSender`.
 *
 * @param {object} options
 * @param {function} options.sendEvent - async (roomId, eventType, contentJson) => void
 * @param {string} options.roomId - Matrix room ID
 * @param {string} [options.eventType='org.mxdx.terminal.data']
 * @param {number} [options.batchMs=200]
 * @param {string} [options.sessionId]
 * @param {function} [options.onError] - (err, seq) => void  for non-retryable errors
 * @param {function} [options.onBuffering] - (isBuffering: boolean) => void
 */
export class BatchedSenderWasm {
  #wasm;
  #sendEvent;
  #onError;
  #onBuffering;
  #buffering = false;
  #flushTimer = null;
  #sending = false;
  #destroyed = false;
  #batchMs;
  #drainKick = null;

  constructor({
    sendEvent,
    roomId,
    eventType = 'org.mxdx.terminal.data',
    batchMs = 200,
    sessionId = null,
    onError = null,
    onBuffering = null,
  }) {
    this.#sendEvent = sendEvent;
    this.#batchMs = batchMs;
    this.#onError = onError;
    this.#onBuffering = onBuffering;
    this.#wasm = new WasmBatchedSender(
      roomId,
      eventType,
      sessionId == null ? undefined : String(sessionId),
    );
  }

  push(data) {
    if (this.#destroyed) return;
    const bytes = typeof data === 'string'
      ? textEncoder.encode(data)
      : data instanceof Uint8Array
        ? data
        : new Uint8Array(data);
    this.#wasm.push(bytes);

    if (this.#batchMs <= 10) {
      if (this.#flushTimer) {
        clearTimeout(this.#flushTimer);
        this.#flushTimer = null;
      }
      this.#kickDrain();
      return;
    }
    if (!this.#flushTimer) {
      this.#flushTimer = setTimeout(() => {
        this.#flushTimer = null;
        this.#kickDrain();
      }, this.#batchMs);
    }
  }

  #kickDrain() {
    // Coalesce concurrent kicks; the drain loop runs to exhaustion.
    if (this.#sending) return;
    this.#sending = true;
    this.#drainKick = this.#drain().finally(() => {
      this.#sending = false;
      this.#drainKick = null;
    });
  }

  async #drain() {
    while (!this.#destroyed) {
      let payloadJson;
      try {
        payloadJson = this.#wasm.takePayload();
      } catch (err) {
        // Serialization failure inside WASM — surface as non-retryable error.
        if (this.#onError) this.#onError(err, -1);
        return;
      }
      if (payloadJson == null) return;

      // The seq is embedded in the payload — parse for the onError callback.
      let seq = -1;
      try { seq = JSON.parse(payloadJson).seq ?? -1; } catch { /* keep -1 */ }

      try {
        await this.#sendEvent(
          this.#wasm.roomId,
          this.#wasm.eventType,
          payloadJson,
        );
        this.#wasm.markSent();
        if (this.#buffering) {
          this.#buffering = false;
          if (this.#onBuffering) this.#onBuffering(false);
        }
      } catch (err) {
        const errStr = String(err);
        if (errStr.includes('429') || errStr.includes('M_LIMIT_EXCEEDED')) {
          this.#wasm.markRateLimited();
          if (!this.#buffering) {
            this.#buffering = true;
            if (this.#onBuffering) this.#onBuffering(true);
          }
          const wait = WasmBatchedSender.parseRetryAfterMs(errStr);
          await new Promise((r) => setTimeout(r, wait));
          // Loop continues — takePayload() will coalesce in-flight + new buffer.
        } else {
          this.#wasm.markError();
          if (this.#onError) this.#onError(err, seq);
          // Drop and continue; matches JS BatchedSender's drop-and-report behavior.
        }
      }
    }
  }

  /** Flush any pending data and wait for drain. */
  async flush() {
    if (this.#flushTimer) {
      clearTimeout(this.#flushTimer);
      this.#flushTimer = null;
    }
    if (this.#destroyed) return;
    this.#kickDrain();
    if (this.#drainKick) {
      try { await this.#drainKick; } catch { /* drain exits cleanly on destroy */ }
    }
  }

  destroy() {
    this.#destroyed = true;
    if (this.#flushTimer) {
      clearTimeout(this.#flushTimer);
      this.#flushTimer = null;
    }
    // Best-effort: tell WASM there's nothing pending. WASM has no destroy hook
    // beyond Drop; the struct is collected when the JS reference is GC'd.
    try { this.#wasm.markError(); } catch { /* ignore */ }
  }

  get batchMs() { return this.#batchMs; }
  set batchMs(ms) { this.#batchMs = ms; }
  get queueLength() { return this.#wasm.hasInFlight ? 1 : 0; }
  get pendingBytes() { return this.#wasm.pendingBytes; }
}
