import { connectWithSession } from './session.js';

export class MultiHsClient {
  #entries = [];        // Array<{client, server, userId, deviceId, latencyMs}>
  #preferredIndex = 0;
  #preferredOverride = null;
  #onPreferredChangeCb = null;
  #running = true;
  #seenEvents = new Map();  // eventId -> timestamp
  static #MAX_SEEN = 10_000;
  static #EVICT_BATCH = 2_000;
  #circuitBreakers = new Map(); // server -> { failures: number[], status: 'healthy'|'down' }
  #recoveryTimers = new Map();
  static #FAIL_WINDOW_MS = 5 * 60 * 1000;
  static #FAIL_THRESHOLD = 5;

  /**
   * Connect to multiple homeservers, measure latency, select preferred.
   * @param {Array<Object>} configs - Per-server connectWithSession configs
   * @param {Object} [options]
   * @param {string} [options.preferredServer] - Pin preferred to this server
   * @param {Function} [options.log] - Logger
   * @returns {Promise<MultiHsClient>}
   */
  static async connect(configs, options = {}) {
    const { preferredServer, log = () => {} } = options;

    const results = await Promise.all(
      configs.map(async (cfg) => {
        const { client } = await connectWithSession(cfg);
        return { client, server: cfg.server };
      }),
    );

    return MultiHsClient._createFromClients(results, { preferredServer, log });
  }

  /**
   * Create from pre-connected clients (used by tests and connect()).
   * Measures latency via syncOnce() on each client.
   */
  static async _createFromClients(clientEntries, options = {}) {
    const { preferredServer, log = () => {} } = options;
    const instance = new MultiHsClient();
    instance.#preferredOverride = preferredServer || null;

    // Measure latency for each client
    instance.#entries = await Promise.all(
      clientEntries.map(async ({ client, server }) => {
        const start = performance.now();
        try {
          await client.syncOnce();
        } catch {
          // Failed sync gets max latency
        }
        const latencyMs = performance.now() - start;
        return {
          client,
          server,
          userId: client.userId(),
          deviceId: client.deviceId(),
          latencyMs,
        };
      }),
    );

    // Select preferred
    if (instance.#preferredOverride) {
      const idx = instance.#entries.findIndex(e => e.server === instance.#preferredOverride);
      if (idx >= 0) instance.#preferredIndex = idx;
    } else {
      let minLatency = Infinity;
      for (let i = 0; i < instance.#entries.length; i++) {
        if (instance.#entries[i].latencyMs < minLatency) {
          minLatency = instance.#entries[i].latencyMs;
          instance.#preferredIndex = i;
        }
      }
    }

    // Initialize circuit breakers
    for (const entry of instance.#entries) {
      instance.#circuitBreakers.set(entry.server, { failures: [], status: 'healthy' });
    }

    log(`MultiHsClient: ${instance.#entries.length} server(s), preferred=${instance.preferred.server} (${Math.round(instance.preferred.latencyMs)}ms)`);
    return instance;
  }

  get preferred() { return this.#entries[this.#preferredIndex]; }
  userId() { return this.preferred.userId; }
  deviceId() { return this.preferred.deviceId; }
  get serverCount() { return this.#entries.length; }
  get isSingleServer() { return this.#entries.length <= 1; }

  onPreferredChange(cb) { this.#onPreferredChangeCb = cb; }

  // ── Sending API (routes through preferred) ──

  async sendEvent(roomId, type, contentJson) {
    try {
      const result = await this.preferred.client.sendEvent(roomId, type, contentJson);
      this._recordSuccess(this.#preferredIndex);
      return result;
    } catch (err) {
      this._recordFailure(this.#preferredIndex);
      throw err;
    }
  }

  async sendStateEvent(roomId, type, stateKey, contentJson) {
    try {
      const result = await this.preferred.client.sendStateEvent(roomId, type, stateKey, contentJson);
      this._recordSuccess(this.#preferredIndex);
      return result;
    } catch (err) {
      this._recordFailure(this.#preferredIndex);
      throw err;
    }
  }

  // ── Proxy methods (delegate to preferred) ──

  async syncOnce() { return this.preferred.client.syncOnce(); }
  async joinRoom(roomId) { return this.preferred.client.joinRoom(roomId); }
  async createDmRoom(userId) { return this.preferred.client.createDmRoom(userId); }
  async inviteUser(roomId, userId) { return this.preferred.client.inviteUser(roomId, userId); }
  invitedRoomIds() { return this.preferred.client.invitedRoomIds(); }
  async getOrCreateLauncherSpace(name) { return this.preferred.client.getOrCreateLauncherSpace(name); }
  async collectRoomEvents(roomId, limit) { return this.preferred.client.collectRoomEvents(roomId, limit); }
  async exportSession() { return this.preferred.client.exportSession(); }
  async bootstrapCrossSigningIfNeeded(pw) { return this.preferred.client.bootstrapCrossSigningIfNeeded(pw); }
  async verifyOwnIdentity() { return this.preferred.client.verifyOwnIdentity(); }
  async createRoom(config) { return this.preferred.client.createRoom(config); }
  async readRoomEvents(roomId) { return this.preferred.client.readRoomEvents(roomId); }
  async findRoomEvents(roomId, type, limit) { return this.preferred.client.findRoomEvents(roomId, type, limit); }
  async listLauncherSpaces() { return this.preferred.client.listLauncherSpaces(); }

  // ── Deduplication ──

  _isDuplicate(eventId) {
    if (this.isSingleServer) return false;
    if (this.#seenEvents.has(eventId)) return true;
    this.#seenEvents.set(eventId, Date.now());
    if (this.#seenEvents.size > MultiHsClient.#MAX_SEEN) {
      let count = 0;
      for (const key of this.#seenEvents.keys()) {
        if (count++ >= MultiHsClient.#EVICT_BATCH) break;
        this.#seenEvents.delete(key);
      }
    }
    return false;
  }

  // ── Circuit Breaker ──

  _recordFailure(serverIndex) {
    if (this.isSingleServer) return;
    const entry = this.#entries[serverIndex];
    const state = this.#circuitBreakers.get(entry.server);
    const now = Date.now();
    state.failures = state.failures.filter(t => now - t < MultiHsClient.#FAIL_WINDOW_MS);
    state.failures.push(now);

    if (state.failures.length >= MultiHsClient.#FAIL_THRESHOLD) {
      // Cross-server sanity: if ALL servers are failing, assume network issue
      const allFailing = this.#entries.every((_, i) => {
        const s = this.#circuitBreakers.get(this.#entries[i].server);
        return s.status === 'down' || s.failures.length >= MultiHsClient.#FAIL_THRESHOLD;
      });
      if (allFailing) {
        for (const [, s] of this.#circuitBreakers) {
          s.failures = [];
          s.status = 'healthy';
        }
        return;
      }
      state.status = 'down';
      if (serverIndex === this.#preferredIndex) {
        this.#triggerFailover();
      }
      this.#startRecoveryProbe(serverIndex);
    }
  }

  _recordSuccess(serverIndex) {
    if (this.isSingleServer) return;
    const state = this.#circuitBreakers.get(this.#entries[serverIndex].server);
    state.failures = [];
    if (state.status === 'down') {
      state.status = 'healthy';
      const timer = this.#recoveryTimers.get(this.#entries[serverIndex].server);
      if (timer) { clearTimeout(timer); this.#recoveryTimers.delete(this.#entries[serverIndex].server); }
    }
  }

  #triggerFailover() {
    // Synchronous failover: pick lowest-latency healthy server
    let bestIdx = -1;
    let bestLatency = Infinity;
    for (let i = 0; i < this.#entries.length; i++) {
      if (i === this.#preferredIndex) continue;
      const s = this.#circuitBreakers.get(this.#entries[i].server);
      if (s.status === 'down') continue;
      if (this.#entries[i].latencyMs < bestLatency) {
        bestLatency = this.#entries[i].latencyMs;
        bestIdx = i;
      }
    }
    if (bestIdx < 0) return;
    const old = this.#preferredIndex;
    this.#preferredIndex = bestIdx;
    if (this.#onPreferredChangeCb) {
      this.#onPreferredChangeCb(this.preferred, this.#entries[old]);
    }
  }

  serverHealth() {
    return new Map(this.#entries.map(entry => [
      entry.server,
      {
        status: this.#circuitBreakers.get(entry.server)?.status || 'healthy',
        latencyMs: entry.latencyMs,
      },
    ]));
  }

  allUserIds() {
    return this.#entries.map(e => e.userId);
  }

  // ── Recovery Probes ──

  _recoveryJitterMs() {
    return 60_000 + Math.floor(Math.random() * 100_001);
  }

  #startRecoveryProbe(serverIndex) {
    const server = this.#entries[serverIndex].server;
    if (this.#recoveryTimers.has(server)) return;
    const jitterMs = this._recoveryJitterMs();
    const timer = setTimeout(async () => {
      this.#recoveryTimers.delete(server);
      if (!this.#running) return;
      try {
        await this.#entries[serverIndex].client.syncOnce();
        this._recordSuccess(serverIndex);
      } catch {
        if (this.#running) this.#startRecoveryProbe(serverIndex);
      }
    }, jitterMs);
    timer.unref?.();
    this.#recoveryTimers.set(server, timer);
  }

  async shutdown() {
    this.#running = false;
    for (const [, timer] of this.#recoveryTimers) clearTimeout(timer);
    this.#recoveryTimers.clear();
  }
}
