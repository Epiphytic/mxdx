// Tests for the WASM-backed BatchedSender migration (P0-2).
//
// Exercises both `WasmBatchedSender` directly (via @mxdx/core) and the JS
// thin wrapper `BatchedSenderWasm` to confirm:
//   - basic flush via `takePayload` / `markSent`
//   - 429 retry: same payload re-emitted after `markRateLimited`
//   - coalesce-on-retry: data pushed during the rate-limit wait is bundled
//     into a single retry event (matches legacy JS BatchedSender)
//   - `parseRetryAfterMs` matches the JS regex behaviour
//
// These tests are integration-ish: they stand up a real WasmBatchedSender
// (Node-target WASM is loaded by @mxdx/core) but use a fake `sendEvent`
// to drive the rate-limit path deterministically.

import { describe, it } from 'node:test';
import assert from 'node:assert';

describe('WasmBatchedSender (P0-2 — Rust 429 retry)', () => {
  it('takePayload returns null on empty buffer', async () => {
    const { WasmBatchedSender } = await import('@mxdx/core');
    const w = new WasmBatchedSender('!room:example.com', undefined, undefined);
    assert.strictEqual(w.takePayload(), null, 'empty buffer → null');
    assert.strictEqual(w.pendingBytes, 0);
    assert.strictEqual(w.hasInFlight, false);
  });

  it('takePayload + markSent: basic flush, seq increments', async () => {
    const { WasmBatchedSender } = await import('@mxdx/core');
    const w = new WasmBatchedSender('!room:example.com', undefined, 'sess-1');
    w.push(new Uint8Array([0x68, 0x69])); // "hi"
    const p1 = w.takePayload();
    assert.ok(p1, 'first payload');
    const obj1 = JSON.parse(p1);
    assert.strictEqual(obj1.seq, 0, 'seq starts at 0');
    assert.strictEqual(obj1.session_id, 'sess-1');
    assert.ok(obj1.data, 'has data');
    assert.match(obj1.encoding, /base64/);
    assert.strictEqual(w.hasInFlight, true);

    w.markSent();
    assert.strictEqual(w.hasInFlight, false);
    assert.strictEqual(w.pendingBytes, 0);

    w.push(new Uint8Array([0x6f, 0x6b])); // "ok"
    const p2 = w.takePayload();
    assert.ok(p2);
    const obj2 = JSON.parse(p2);
    assert.strictEqual(obj2.seq, 1, 'seq increments after successful send');
  });

  it('markRateLimited: same payload re-emitted on next takePayload', async () => {
    const { WasmBatchedSender } = await import('@mxdx/core');
    const w = new WasmBatchedSender('!room:example.com', undefined, undefined);
    w.push(new Uint8Array([0x41, 0x42, 0x43])); // "ABC"
    const p1 = JSON.parse(w.takePayload());
    assert.strictEqual(p1.seq, 0);
    assert.strictEqual(w.hasInFlight, true);

    w.markRateLimited();
    assert.strictEqual(w.isRateLimited, true, 'rate-limited flag set');
    assert.strictEqual(w.hasInFlight, true, 'in-flight retained for retry');

    // No new push — should re-emit the same payload with the same seq.
    const p2 = JSON.parse(w.takePayload());
    assert.strictEqual(p2.seq, 0, 'retry reuses original seq');
    assert.strictEqual(p2.data, p1.data, 'retry reuses original encoded data');

    w.markSent();
    assert.strictEqual(w.isRateLimited, false, 'cleared after successful send');
    assert.strictEqual(w.hasInFlight, false);
  });

  it('coalesce-on-retry: data pushed during rate-limit wait is bundled', async () => {
    const { WasmBatchedSender } = await import('@mxdx/core');
    const w = new WasmBatchedSender('!room:example.com', undefined, undefined);
    w.push(new Uint8Array([0x41])); // "A"
    const p1 = JSON.parse(w.takePayload());
    assert.strictEqual(p1.seq, 0);

    w.markRateLimited();
    // Simulate "during wait" — JS pushes more data.
    w.push(new Uint8Array([0x42])); // "B"
    w.push(new Uint8Array([0x43])); // "C"

    const p2json = w.takePayload();
    const p2 = JSON.parse(p2json);
    assert.strictEqual(p2.seq, 0, 'retry seq stays at 0');
    // The combined payload should encode "ABC", which is 3 bytes — under
    // the 32-byte zlib threshold, so plain base64. Decode and check.
    const decoded = Buffer.from(p2.data, 'base64');
    // Encoding is "base64" (under 32 bytes); decoded bytes are "ABC".
    assert.strictEqual(p2.encoding, 'base64');
    assert.strictEqual(decoded.toString('utf8'), 'ABC');

    w.markSent();
    // Next push should now use seq 1 (we never bumped on retry).
    w.push(new Uint8Array([0x44]));
    const p3 = JSON.parse(w.takePayload());
    assert.strictEqual(p3.seq, 1);
  });

  it('markError: drops in-flight, no retry', async () => {
    const { WasmBatchedSender } = await import('@mxdx/core');
    const w = new WasmBatchedSender('!room:example.com', undefined, undefined);
    w.push(new Uint8Array([0x58]));
    JSON.parse(w.takePayload());
    assert.strictEqual(w.hasInFlight, true);
    w.markError();
    assert.strictEqual(w.hasInFlight, false);
    assert.strictEqual(w.pendingBytes, 0);
    // Empty after drop.
    assert.strictEqual(w.takePayload(), null);
  });

  it('parseRetryAfterMs: extracts retry_after_ms from error string', async () => {
    const { WasmBatchedSender } = await import('@mxdx/core');
    // Matches Synapse / Tuwunel error shapes.
    assert.strictEqual(
      WasmBatchedSender.parseRetryAfterMs('M_LIMIT_EXCEEDED: retry_after_ms: 5000'),
      5100,
      'returns ms + 100 safety margin',
    );
    assert.strictEqual(
      WasmBatchedSender.parseRetryAfterMs('{"errcode":"M_LIMIT_EXCEEDED","retry_after_ms":1500}'),
      1600,
    );
    assert.strictEqual(
      WasmBatchedSender.parseRetryAfterMs('429 Too Many Requests'),
      2000,
      'fallback when no retry_after_ms field',
    );
  });

  it('zlib threshold: payload >= 32 bytes is zlib+base64', async () => {
    const { WasmBatchedSender } = await import('@mxdx/core');
    const w = new WasmBatchedSender('!room:example.com', undefined, undefined);
    w.push(new Uint8Array(64).fill(0x41));
    const p = JSON.parse(w.takePayload());
    assert.strictEqual(p.encoding, 'zlib+base64');
  });
});

describe('BatchedSenderWasm (JS thin wrapper)', () => {
  it('drains a single push via fake sendEvent', async () => {
    const { BatchedSenderWasm } = await import('../src/batched-sender-wasm.js');
    const sent = [];
    const sender = new BatchedSenderWasm({
      sendEvent: async (rId, t, c) => { sent.push({ rId, t, c }); },
      roomId: '!r:example.com',
      batchMs: 5, // immediate flush path
    });
    sender.push('hello');
    await sender.flush();
    assert.strictEqual(sent.length, 1, 'sent one event');
    assert.strictEqual(sent[0].rId, '!r:example.com');
    assert.strictEqual(sent[0].t, 'org.mxdx.terminal.data');
    const parsed = JSON.parse(sent[0].c);
    assert.strictEqual(parsed.seq, 0);
    sender.destroy();
  });

  it('429 retry: coalesces during wait', async () => {
    const { BatchedSenderWasm } = await import('../src/batched-sender-wasm.js');
    const sent = [];
    let firstRespondsRateLimited = true;
    const sender = new BatchedSenderWasm({
      sendEvent: async (rId, t, c) => {
        if (firstRespondsRateLimited) {
          firstRespondsRateLimited = false;
          // Match a real Matrix rate-limit error format.
          throw new Error('429 M_LIMIT_EXCEEDED retry_after_ms: 30');
        }
        sent.push({ rId, t, c });
      },
      roomId: '!r:example.com',
      batchMs: 5,
    });
    sender.push('A');
    // Brief tick to let the first send fail and the wrapper start waiting.
    await new Promise((r) => setTimeout(r, 10));
    // Push more during the rate-limit wait.
    sender.push('BC');
    await sender.flush();
    assert.strictEqual(sent.length, 1, 'exactly one successful send (coalesced)');
    const parsed = JSON.parse(sent[0].c);
    assert.strictEqual(parsed.seq, 0, 'retry reused seq');
    const decoded = Buffer.from(parsed.data, 'base64');
    assert.strictEqual(decoded.toString('utf8'), 'ABC', 'coalesced payload contains all data');
    sender.destroy();
  });

  it('non-retryable error invokes onError and continues', async () => {
    const { BatchedSenderWasm } = await import('../src/batched-sender-wasm.js');
    const sent = [];
    const errors = [];
    let throwOnce = true;
    const sender = new BatchedSenderWasm({
      sendEvent: async (rId, t, c) => {
        if (throwOnce) { throwOnce = false; throw new Error('500 server error'); }
        sent.push({ rId, t, c });
      },
      roomId: '!r:example.com',
      batchMs: 5,
      onError: (err, seq) => { errors.push({ err: String(err), seq }); },
    });
    sender.push('first');
    await sender.flush();
    sender.push('second');
    await sender.flush();
    assert.strictEqual(errors.length, 1, 'onError called once');
    assert.strictEqual(sent.length, 1, 'second push delivered after error');
    sender.destroy();
  });

  it('onBuffering fires once on rate-limit, once on clear', async () => {
    const { BatchedSenderWasm } = await import('../src/batched-sender-wasm.js');
    const buffering = [];
    let throttle = 2;
    const sender = new BatchedSenderWasm({
      sendEvent: async () => {
        if (throttle > 0) {
          throttle--;
          throw new Error('429 retry_after_ms: 10');
        }
      },
      roomId: '!r:example.com',
      batchMs: 5,
      onBuffering: (b) => buffering.push(b),
    });
    sender.push('payload');
    await sender.flush();
    assert.deepStrictEqual(buffering, [true, false], 'buffering toggled true then false');
    sender.destroy();
  });
});
