import { describe, it, after } from 'node:test';
import assert from 'node:assert';
import { MultiHsClient } from '@mxdx/core';

// Mock WasmMatrixClient for unit testing
class MockClient {
  #userId; #deviceId; #latencyMs; #shouldFail;
  constructor({ userId, deviceId, latencyMs = 10, shouldFail = false }) {
    this.#userId = userId;
    this.#deviceId = deviceId;
    this.#latencyMs = latencyMs;
    this.#shouldFail = shouldFail;
  }
  async syncOnce() {
    await new Promise(r => setTimeout(r, this.#latencyMs));
    if (this.#shouldFail) throw new Error('sync failed');
  }
  userId() { return this.#userId; }
  deviceId() { return this.#deviceId; }
  async sendEvent(roomId, type, content) { return { event_id: `$${Date.now()}` }; }
  async sendStateEvent(roomId, type, stateKey, content) { return {}; }
  async onRoomEvent(roomId, type, timeout) { return null; }
  async collectRoomEvents(roomId, limit) { return '[]'; }
  invitedRoomIds() { return []; }
  async joinRoom(roomId) {}
  async findLauncherSpace(name) { return null; }
  async getOrCreateLauncherSpace(name) { return { space_id: '!s', exec_room_id: '!e', logs_room_id: '!l' }; }
  async createDmRoom(userId) { return '!dm'; }
  async exportSession() { return '{}'; }
  async bootstrapCrossSigningIfNeeded(pw) {}
  async verifyOwnIdentity() {}
  async inviteUser(roomId, userId) {}
  // Test helpers
  setShouldFail(v) { this.#shouldFail = v; }
}

// Factory that creates MultiHsClient from pre-built MockClients (bypass connectWithSession)
function createFromMocks(mocks, options = {}) {
  return MultiHsClient._createFromClients(
    mocks.map(m => ({ client: m.client, server: m.server })),
    options,
  );
}

describe('MultiHsClient: Core', () => {
  it('single server connects and returns serverCount 1', async () => {
    const mock = { client: new MockClient({ userId: '@u:hs1', deviceId: 'D1', latencyMs: 5 }), server: 'hs1' };
    const mhs = await createFromMocks([mock]);
    assert.strictEqual(mhs.serverCount, 1);
    assert.strictEqual(mhs.isSingleServer, true);
    assert.strictEqual(mhs.userId(), '@u:hs1');
    assert.strictEqual(mhs.deviceId(), 'D1');
    await mhs.shutdown();
  });

  it('two servers: preferred is lowest latency', async () => {
    const slow = { client: new MockClient({ userId: '@u:hs1', deviceId: 'D1', latencyMs: 100 }), server: 'hs1' };
    const fast = { client: new MockClient({ userId: '@u:hs2', deviceId: 'D2', latencyMs: 5 }), server: 'hs2' };
    const mhs = await createFromMocks([slow, fast]);
    assert.strictEqual(mhs.serverCount, 2);
    assert.strictEqual(mhs.isSingleServer, false);
    assert.strictEqual(mhs.preferred.server, 'hs2');
    assert.strictEqual(mhs.userId(), '@u:hs2');
    await mhs.shutdown();
  });

  it('preferredServer config override pins server', async () => {
    const slow = { client: new MockClient({ userId: '@u:hs1', deviceId: 'D1', latencyMs: 100 }), server: 'hs1' };
    const fast = { client: new MockClient({ userId: '@u:hs2', deviceId: 'D2', latencyMs: 5 }), server: 'hs2' };
    const mhs = await createFromMocks([slow, fast], { preferredServer: 'hs1' });
    assert.strictEqual(mhs.preferred.server, 'hs1');
    await mhs.shutdown();
  });

  it('shutdown is idempotent', async () => {
    const mock = { client: new MockClient({ userId: '@u:hs1', deviceId: 'D1' }), server: 'hs1' };
    const mhs = await createFromMocks([mock]);
    await mhs.shutdown();
    await mhs.shutdown(); // no throw
  });
});
