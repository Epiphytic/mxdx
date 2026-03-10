import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import { BatchedSender } from '../../../packages/core/batched-sender.js';

describe('BatchedSender', () => {
  it('allows dynamic batchMs changes', async () => {
    const sent = [];
    const sender = new BatchedSender({
      sendEvent: async (roomId, type, content) => { sent.push(content); },
      roomId: '!test:example.com',
      batchMs: 200,
    });

    assert.equal(sender.batchMs, 200);
    sender.batchMs = 10;
    assert.equal(sender.batchMs, 10);

    sender.destroy();
  });
});
