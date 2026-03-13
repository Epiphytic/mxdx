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

describe('MultiHsClient: Deduplication', () => {
  it('first event is not a duplicate', async () => {
    const mock = { client: new MockClient({ userId: '@u:hs1', deviceId: 'D1' }), server: 'hs1' };
    const fast = { client: new MockClient({ userId: '@u:hs2', deviceId: 'D2' }), server: 'hs2' };
    const mhs = await createFromMocks([mock, fast]);
    assert.strictEqual(mhs._isDuplicate('$event1'), false);
    await mhs.shutdown();
  });

  it('second occurrence of same event ID is a duplicate', async () => {
    const mock = { client: new MockClient({ userId: '@u:hs1', deviceId: 'D1' }), server: 'hs1' };
    const fast = { client: new MockClient({ userId: '@u:hs2', deviceId: 'D2' }), server: 'hs2' };
    const mhs = await createFromMocks([mock, fast]);
    mhs._isDuplicate('$event1');
    assert.strictEqual(mhs._isDuplicate('$event1'), true);
    await mhs.shutdown();
  });

  it('evicts oldest entries when seen set exceeds max size', async () => {
    const mock = { client: new MockClient({ userId: '@u:hs1', deviceId: 'D1' }), server: 'hs1' };
    const fast = { client: new MockClient({ userId: '@u:hs2', deviceId: 'D2' }), server: 'hs2' };
    const mhs = await createFromMocks([mock, fast]);
    for (let i = 0; i < 10001; i++) {
      mhs._isDuplicate(`$evt${i}`);
    }
    assert.strictEqual(mhs._isDuplicate('$evt0'), false); // was evicted
    assert.strictEqual(mhs._isDuplicate('$evt10000'), true); // still in set
    await mhs.shutdown();
  });

  it('single-server mode: always returns false', async () => {
    const mock = { client: new MockClient({ userId: '@u:hs1', deviceId: 'D1' }), server: 'hs1' };
    const mhs = await createFromMocks([mock]);
    assert.strictEqual(mhs._isDuplicate('$event1'), false);
    assert.strictEqual(mhs._isDuplicate('$event1'), false); // still false
    await mhs.shutdown();
  });
});

describe('MultiHsClient: Circuit Breaker', () => {
  it('4 failures in window: server stays healthy', async () => {
    const s1 = { client: new MockClient({ userId: '@u:hs1', deviceId: 'D1', latencyMs: 5 }), server: 'hs1' };
    const s2 = { client: new MockClient({ userId: '@u:hs2', deviceId: 'D2', latencyMs: 10 }), server: 'hs2' };
    const mhs = await createFromMocks([s1, s2]);
    for (let i = 0; i < 4; i++) mhs._recordFailure(0);
    const health = mhs.serverHealth();
    assert.strictEqual(health.get('hs1').status, 'healthy');
    await mhs.shutdown();
  });

  it('5 failures in window: server marked down, failover triggers', async () => {
    const s1 = { client: new MockClient({ userId: '@u:hs1', deviceId: 'D1', latencyMs: 5 }), server: 'hs1' };
    const s2 = { client: new MockClient({ userId: '@u:hs2', deviceId: 'D2', latencyMs: 10 }), server: 'hs2' };
    const mhs = await createFromMocks([s1, s2]);
    assert.strictEqual(mhs.preferred.server, 'hs1');
    for (let i = 0; i < 5; i++) mhs._recordFailure(0);
    const health = mhs.serverHealth();
    assert.strictEqual(health.get('hs1').status, 'down');
    assert.strictEqual(mhs.preferred.server, 'hs2');
    await mhs.shutdown();
  });

  it('all servers failing: no circuit break (network sanity)', async () => {
    const s1 = { client: new MockClient({ userId: '@u:hs1', deviceId: 'D1', shouldFail: true }), server: 'hs1' };
    const s2 = { client: new MockClient({ userId: '@u:hs2', deviceId: 'D2', shouldFail: true }), server: 'hs2' };
    const mhs = await createFromMocks([s1, s2]);
    for (let i = 0; i < 5; i++) {
      mhs._recordFailure(0);
      mhs._recordFailure(1);
    }
    const health = mhs.serverHealth();
    assert.strictEqual(health.get('hs1').status, 'healthy');
    assert.strictEqual(health.get('hs2').status, 'healthy');
    await mhs.shutdown();
  });

  it('single-server: no circuit breaking', async () => {
    const s1 = { client: new MockClient({ userId: '@u:hs1', deviceId: 'D1' }), server: 'hs1' };
    const mhs = await createFromMocks([s1]);
    for (let i = 0; i < 10; i++) mhs._recordFailure(0);
    const health = mhs.serverHealth();
    assert.strictEqual(health.get('hs1').status, 'healthy');
    await mhs.shutdown();
  });

  it('recordSuccess resets failure count', async () => {
    const s1 = { client: new MockClient({ userId: '@u:hs1', deviceId: 'D1', latencyMs: 5 }), server: 'hs1' };
    const s2 = { client: new MockClient({ userId: '@u:hs2', deviceId: 'D2', latencyMs: 10 }), server: 'hs2' };
    const mhs = await createFromMocks([s1, s2]);
    for (let i = 0; i < 4; i++) mhs._recordFailure(0);
    mhs._recordSuccess(0);
    for (let i = 0; i < 4; i++) mhs._recordFailure(0);
    assert.strictEqual(mhs.serverHealth().get('hs1').status, 'healthy');
    await mhs.shutdown();
  });
});
