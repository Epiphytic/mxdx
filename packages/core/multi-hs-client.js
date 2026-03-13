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

    log(`MultiHsClient: ${instance.#entries.length} server(s), preferred=${instance.preferred.server} (${Math.round(instance.preferred.latencyMs)}ms)`);
    return instance;
  }

  get preferred() { return this.#entries[this.#preferredIndex]; }
  userId() { return this.preferred.userId; }
  deviceId() { return this.preferred.deviceId; }
  get serverCount() { return this.#entries.length; }
  get isSingleServer() { return this.#entries.length <= 1; }

  onPreferredChange(cb) { this.#onPreferredChangeCb = cb; }

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

  async shutdown() {
    this.#running = false;
  }
}
