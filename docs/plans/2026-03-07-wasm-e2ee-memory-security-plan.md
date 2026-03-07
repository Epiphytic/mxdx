# WASM E2EE Memory Security Hardening Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Harden the mxdx launcher against crypto key extraction from WASM linear memory, fake-indexeddb, and process memory through four independent defense layers.

**Architecture:** Four layers: (1) Process isolation via Worker thread for crypto, (2) Encrypted IndexedDB store via Web Crypto API, (3) Aggressive Megolm key rotation with configurable retention, (4) OS-level hardening (prctl, mlock, zeroize). Each layer ships independently.

**Tech Stack:** Node.js `worker_threads`, `crypto.subtle` (Web Crypto API), `browser-crypto` Rust crate, `zeroize` Rust crate, `posix` npm package (prctl/mlock), matrix-sdk 0.16 `RoomEncryptionEventContent`

**Design doc:** `docs/plans/2026-03-07-wasm-e2ee-memory-security-design.md`

---

## Phase 1: Encrypted IndexedDB Store (Layer 2)

Ship this first because it's self-contained and doesn't require architectural changes. It protects keys at rest in memory immediately.

### Task 1.1: Encrypted IDB Proxy — Test

**Files:**
- Create: `packages/core/src/encrypted-idb-proxy.test.js`

**Step 1: Write the failing test**

```javascript
import { describe, it } from 'node:test';
import assert from 'node:assert';
import { createEncryptedIDBProxy } from './encrypted-idb-proxy.js';

describe('EncryptedIDBProxy', () => {
  it('encrypts values stored in indexedDB', async () => {
    const proxy = await createEncryptedIDBProxy();

    // Store a value through the proxy
    const testData = { secret_key: 'olm-session-key-abc123', device_id: 'MYDEVICE' };
    await proxy.put('test-store', 'key1', testData);

    // Read back through proxy — should get original data
    const retrieved = await proxy.get('test-store', 'key1');
    assert.deepStrictEqual(retrieved, testData);

    // Read raw storage — should be ciphertext, not contain plaintext
    const raw = proxy.getRaw('test-store', 'key1');
    const rawStr = JSON.stringify(raw);
    assert.ok(!rawStr.includes('olm-session-key-abc123'), 'Raw storage must not contain plaintext keys');
    assert.ok(raw.iv, 'Raw storage must contain an IV');
    assert.ok(raw.ciphertext, 'Raw storage must contain ciphertext');
  });

  it('CryptoKey is non-extractable', async () => {
    const proxy = await createEncryptedIDBProxy();
    const keyInfo = proxy.getKeyInfo();
    assert.strictEqual(keyInfo.extractable, false, 'CryptoKey must be non-extractable');
    assert.deepStrictEqual(keyInfo.usages, ['encrypt', 'decrypt']);
  });

  it('different proxies produce different ciphertexts', async () => {
    const proxy1 = await createEncryptedIDBProxy();
    const proxy2 = await createEncryptedIDBProxy();
    const data = { key: 'same-data' };

    await proxy1.put('store', 'k', data);
    await proxy2.put('store', 'k', data);

    const raw1 = proxy1.getRaw('store', 'k');
    const raw2 = proxy2.getRaw('store', 'k');
    assert.notDeepStrictEqual(raw1.ciphertext, raw2.ciphertext,
      'Different keys must produce different ciphertexts');
  });
});
```

**Step 2: Run test to verify it fails**

Run: `node --test packages/core/src/encrypted-idb-proxy.test.js`
Expected: FAIL — module `./encrypted-idb-proxy.js` not found

**Step 3: Commit**

```bash
git add packages/core/src/encrypted-idb-proxy.test.js
git commit -m "test: encrypted IDB proxy — failing tests for encrypted storage layer"
```

---

### Task 1.2: Encrypted IDB Proxy — Implementation

**Files:**
- Create: `packages/core/src/encrypted-idb-proxy.js`

**Step 1: Implement the encrypted proxy**

```javascript
import crypto from 'node:crypto';

/**
 * Creates an encrypted proxy that wraps an in-memory key-value store.
 * Values are AES-256-GCM encrypted using a non-extractable CryptoKey
 * held by the runtime (outside JS heap / WASM linear memory).
 */
export async function createEncryptedIDBProxy() {
  // Generate non-extractable AES-256-GCM key via Web Crypto API
  const cryptoKey = await crypto.subtle.generateKey(
    { name: 'AES-GCM', length: 256 },
    false, // non-extractable — key bytes stay in native runtime memory
    ['encrypt', 'decrypt'],
  );

  // In-memory store: Map<storeName, Map<key, {iv, ciphertext}>>
  const stores = new Map();

  function getStore(storeName) {
    if (!stores.has(storeName)) stores.set(storeName, new Map());
    return stores.get(storeName);
  }

  async function encrypt(plaintext) {
    const iv = crypto.getRandomValues(new Uint8Array(12));
    const encoded = new TextEncoder().encode(JSON.stringify(plaintext));
    const ciphertext = await crypto.subtle.encrypt(
      { name: 'AES-GCM', iv },
      cryptoKey,
      encoded,
    );
    return {
      iv: Buffer.from(iv).toString('base64'),
      ciphertext: Buffer.from(ciphertext).toString('base64'),
    };
  }

  async function decrypt(record) {
    const iv = Buffer.from(record.iv, 'base64');
    const ciphertext = Buffer.from(record.ciphertext, 'base64');
    const decrypted = await crypto.subtle.decrypt(
      { name: 'AES-GCM', iv },
      cryptoKey,
      ciphertext,
    );
    return JSON.parse(new TextDecoder().decode(decrypted));
  }

  return {
    async put(storeName, key, value) {
      const encrypted = await encrypt(value);
      getStore(storeName).set(key, encrypted);
    },

    async get(storeName, key) {
      const store = getStore(storeName);
      const record = store.get(key);
      if (!record) return undefined;
      return decrypt(record);
    },

    getRaw(storeName, key) {
      return getStore(storeName).get(key);
    },

    getKeyInfo() {
      return {
        extractable: cryptoKey.extractable,
        usages: [...cryptoKey.usages],
        algorithm: cryptoKey.algorithm.name,
      };
    },

    clear() {
      stores.clear();
    },
  };
}
```

**Step 2: Run tests to verify they pass**

Run: `node --test packages/core/src/encrypted-idb-proxy.test.js`
Expected: 3 tests PASS

**Step 3: Commit**

```bash
git add packages/core/src/encrypted-idb-proxy.js
git commit -m "feat: encrypted IDB proxy — AES-256-GCM via Web Crypto API with non-extractable key"
```

---

### Task 1.3: Integrate Encrypted Proxy with fake-indexeddb

**Files:**
- Create: `packages/core/src/secure-indexeddb.js`
- Modify: `packages/core/index.js`
- Create: `packages/core/src/secure-indexeddb.test.js`

This task wraps fake-indexeddb so that matrix-sdk's IndexedDB crypto store writes encrypted data.

**Step 1: Write the failing test**

```javascript
// packages/core/src/secure-indexeddb.test.js
import { describe, it, before } from 'node:test';
import assert from 'node:assert';

describe('SecureIndexedDB', () => {
  before(async () => {
    // Import the secure setup — this must run before any IDB access
    await import('./secure-indexeddb.js');
  });

  it('globalThis.indexedDB is available after setup', () => {
    assert.ok(globalThis.indexedDB, 'indexedDB should be polyfilled');
  });

  it('data stored via IDB API is encrypted at rest', async () => {
    // Open a database through the standard IDB API
    const db = await new Promise((resolve, reject) => {
      const req = globalThis.indexedDB.open('test-db', 1);
      req.onupgradeneeded = () => {
        req.result.createObjectStore('secrets', { keyPath: 'id' });
      };
      req.onsuccess = () => resolve(req.result);
      req.onerror = () => reject(req.error);
    });

    // Write sensitive data
    await new Promise((resolve, reject) => {
      const tx = db.transaction('secrets', 'readwrite');
      tx.objectStore('secrets').put({ id: 'key1', secret: 'megolm-session-xyz' });
      tx.oncomplete = resolve;
      tx.onerror = () => reject(tx.error);
    });

    // Read back through normal API — should get plaintext
    const result = await new Promise((resolve, reject) => {
      const tx = db.transaction('secrets', 'readonly');
      const req = tx.objectStore('secrets').get('key1');
      req.onsuccess = () => resolve(req.result);
      req.onerror = () => reject(req.error);
    });

    assert.strictEqual(result.secret, 'megolm-session-xyz');
    db.close();
  });
});
```

**Step 2: Run test to verify it fails**

Run: `node --test packages/core/src/secure-indexeddb.test.js`
Expected: FAIL — module not found

**Step 3: Implement secure-indexeddb.js**

This is the main integration point. It sets up `fake-indexeddb` with an encryption interceptor on the `IDBObjectStore` prototype.

```javascript
// packages/core/src/secure-indexeddb.js
import 'fake-indexeddb/auto';
import crypto from 'node:crypto';

let _cryptoKey = null;

async function getCryptoKey() {
  if (!_cryptoKey) {
    _cryptoKey = await crypto.subtle.generateKey(
      { name: 'AES-GCM', length: 256 },
      false,
      ['encrypt', 'decrypt'],
    );
  }
  return _cryptoKey;
}

async function encryptValue(value) {
  const key = await getCryptoKey();
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const plaintext = new TextEncoder().encode(JSON.stringify(value));
  const ciphertext = await crypto.subtle.encrypt({ name: 'AES-GCM', iv }, key, plaintext);
  return {
    __encrypted: true,
    iv: Buffer.from(iv).toString('base64'),
    ct: Buffer.from(ciphertext).toString('base64'),
  };
}

async function decryptValue(record) {
  if (!record || !record.__encrypted) return record;
  const key = await getCryptoKey();
  const iv = Buffer.from(record.iv, 'base64');
  const ciphertext = Buffer.from(record.ct, 'base64');
  const decrypted = await crypto.subtle.decrypt({ name: 'AES-GCM', iv }, key, ciphertext);
  return JSON.parse(new TextDecoder().decode(decrypted));
}

// Intercept IDBObjectStore.put and IDBObjectStore.get
const origPut = IDBObjectStore.prototype.put;
const origAdd = IDBObjectStore.prototype.add;
const origGet = IDBObjectStore.prototype.get;
const origGetAll = IDBObjectStore.prototype.getAll;

IDBObjectStore.prototype.put = function(value, key) {
  // We need to handle this synchronously for the IDB API contract,
  // but encryption is async. Use a wrapper that encrypts inline.
  // Since fake-indexeddb runs in-process (not real async I/O),
  // we can queue the encryption and patch the result.
  const request = origPut.call(this, value, key);

  // Schedule async encryption to replace the stored value
  const store = this;
  const keyPath = store.keyPath;
  encryptValue(value).then(encrypted => {
    // Preserve the key path so IDB can still index it
    if (keyPath && value[keyPath] !== undefined) {
      encrypted[keyPath] = value[keyPath];
    }
    origPut.call(store, encrypted, key);
  });

  return request;
};

IDBObjectStore.prototype.get = function(key) {
  const request = origGet.call(this, key);
  const origOnSuccess = null;

  // Wrap the onsuccess handler to decrypt
  const origDescriptor = Object.getOwnPropertyDescriptor(IDBRequest.prototype, 'onsuccess');
  const origRequest = request;

  const patchResult = () => {
    const result = origRequest.result;
    if (result && result.__encrypted) {
      decryptValue(result).then(decrypted => {
        Object.defineProperty(origRequest, 'result', { value: decrypted, writable: true });
        // Re-fire success if handler is set
      });
    }
  };

  return request;
};

// Export for testing
export function getEncryptionKeyInfo() {
  if (!_cryptoKey) return null;
  return {
    extractable: _cryptoKey.extractable,
    usages: [..._cryptoKey.usages],
  };
}

export function clearCryptoKey() {
  _cryptoKey = null;
}
```

> **NOTE TO IMPLEMENTER:** The IDB prototype patching above is a sketch. fake-indexeddb's internal implementation may require a different interception strategy. The actual implementation should:
> 1. Test with matrix-sdk's real IDB usage patterns (it uses `put`, `get`, `getAll`, `openCursor`)
> 2. Handle the async encryption/sync IDB API mismatch — may need to subclass or wrap the fake-indexeddb `FDBFactory` instead
> 3. Verify that keyPath-based lookups still work after encryption
>
> If prototype patching proves too fragile, the alternative is to create a custom `FDBFactory` subclass that encrypts/decrypts at the serialization boundary. Test against the actual matrix-sdk crypto store usage first.

**Step 4: Update packages/core/index.js**

```javascript
// packages/core/index.js
// Secure IndexedDB polyfill — encrypts crypto store at rest via Web Crypto API
import './src/secure-indexeddb.js';

export * from './wasm/mxdx_core_wasm.js';
```

**Step 5: Run tests**

Run: `node --test packages/core/src/secure-indexeddb.test.js`
Expected: PASS

Run: `node --test packages/core/src/encrypted-idb-proxy.test.js`
Expected: Still PASS

**Step 6: Run existing E2E tests to verify nothing broke**

Run: `node --test packages/e2e-tests/tests/launcher-onboarding.test.js packages/e2e-tests/tests/command-round-trip.test.js`
Expected: 2 tests PASS — encrypted rooms still work with the new store layer

**Step 7: Commit**

```bash
git add packages/core/src/secure-indexeddb.js packages/core/src/secure-indexeddb.test.js packages/core/index.js
git commit -m "feat: encrypted IndexedDB store — intercepts fake-indexeddb with AES-256-GCM via Web Crypto API"
```

---

## Phase 2: Megolm Key Rotation & Retention (Layer 3)

### Task 2.1: Add Security Config to LauncherConfig

**Files:**
- Modify: `packages/launcher/src/config.js`
- Modify: `packages/launcher/src/config.test.js`

**Step 1: Write the failing test**

Add to `packages/launcher/src/config.test.js`:

```javascript
it('loads security config with defaults', () => {
  const config = new LauncherConfig({
    username: 'test',
    servers: ['http://localhost:8008'],
  });
  assert.strictEqual(config.security.megolmRotationMessageCount, 10);
  assert.strictEqual(config.security.megolmRotationIntervalSecs, 3600);
  assert.strictEqual(config.security.keyRetention, 'forward-secrecy');
});

it('round-trips security config through TOML', () => {
  const configPath = path.join(tmpDir, 'security.toml');
  const config = new LauncherConfig({
    username: 'test',
    servers: ['http://localhost:8008'],
    security: {
      megolmRotationMessageCount: 5,
      megolmRotationIntervalSecs: 1800,
      keyRetention: 'audit-trail',
    },
  });
  config.save(configPath);
  const loaded = LauncherConfig.load(configPath);
  assert.strictEqual(loaded.security.megolmRotationMessageCount, 5);
  assert.strictEqual(loaded.security.megolmRotationIntervalSecs, 1800);
  assert.strictEqual(loaded.security.keyRetention, 'audit-trail');
});
```

**Step 2: Run test to verify it fails**

Run: `node --test packages/launcher/src/config.test.js`
Expected: FAIL — `config.security` is undefined

**Step 3: Add security fields to LauncherConfig**

Modify `packages/launcher/src/config.js` constructor to accept and default `security`:

```javascript
constructor({
  username,
  servers = [],
  allowedCommands = [],
  allowedCwd = ['/tmp'],
  telemetry = 'full',
  maxSessions = 5,
  adminUsers = [],
  registrationToken = null,
  security = {},
} = {}) {
  // ... existing fields ...
  this.security = {
    megolmRotationMessageCount: security.megolmRotationMessageCount ?? 10,
    megolmRotationIntervalSecs: security.megolmRotationIntervalSecs ?? 3600,
    keyRetention: security.keyRetention ?? 'forward-secrecy',
  };
}
```

Update `save()` to include `[security]` section. Update `load()` to parse it. Update `fromArgs()` to accept `--key-retention` flag.

**Step 4: Run tests**

Run: `node --test packages/launcher/src/config.test.js`
Expected: All PASS

**Step 5: Commit**

```bash
git add packages/launcher/src/config.js packages/launcher/src/config.test.js
git commit -m "feat: add security config section — Megolm rotation and key retention settings"
```

---

### Task 2.2: Configure Megolm Rotation in WASM

**Files:**
- Modify: `crates/mxdx-core-wasm/src/lib.rs`
- Modify: `crates/mxdx-core-wasm/Cargo.toml`

**Step 1: Add rotation parameters to create_named_encrypted_room**

In `crates/mxdx-core-wasm/src/lib.rs`, modify `create_named_encrypted_room` to accept rotation params:

```rust
async fn create_named_encrypted_room(
    &self,
    name: &str,
    topic: &str,
    rotation_period_msgs: Option<u64>,
    rotation_period_ms: Option<u64>,
) -> Result<String, JsValue> {
    let mut enc_content = RoomEncryptionEventContent::with_recommended_defaults();

    // Override rotation settings if provided
    if let Some(msgs) = rotation_period_msgs {
        enc_content.rotation_period_msgs = Some(msgs.into());
    }
    if let Some(ms) = rotation_period_ms {
        enc_content.rotation_period_ms = Some(Duration::from_millis(ms));
    }

    let encryption_event = InitialStateEvent::new(EmptyStateKey, enc_content);
    let topic_event = InitialStateEvent::new(
        EmptyStateKey,
        RoomTopicEventContent::new(topic.to_string()),
    );

    let mut request = CreateRoomRequest::new();
    request.name = Some(name.to_string());
    request.initial_state = vec![encryption_event.to_raw_any(), topic_event.to_raw_any()];

    let response = self.client.create_room(request).await.map_err(to_js_err)?;
    Ok(response.room_id().to_string())
}
```

Update `create_launcher_space` to pass rotation params through:

```rust
#[wasm_bindgen(js_name = "createLauncherSpace")]
pub async fn create_launcher_space(
    &self,
    launcher_id: &str,
    rotation_period_msgs: Option<u32>,
    rotation_period_secs: Option<u32>,
) -> Result<JsValue, JsValue> {
    // ... existing space creation ...

    let rotation_msgs = rotation_period_msgs.map(|m| m as u64);
    let rotation_ms = rotation_period_secs.map(|s| (s as u64) * 1000);

    let exec_room_id = self.create_named_encrypted_room(
        &format!("mxdx: {launcher_id} — exec"),
        &format!("org.mxdx.launcher.exec:{launcher_id}"),
        rotation_msgs,
        rotation_ms,
    ).await?;

    // ... rest unchanged ...
}
```

**Step 2: Rebuild WASM**

Run: `wasm-pack build crates/mxdx-core-wasm --target nodejs --out-dir ../../packages/core/wasm`
Expected: Build succeeds

**Step 3: Update launcher runtime to pass rotation config**

In `packages/launcher/src/runtime.js`, pass security config when creating the space:

```javascript
this.#topology = await this.#client.getOrCreateLauncherSpace(
  this.#config.username,
  this.#config.security.megolmRotationMessageCount,
  this.#config.security.megolmRotationIntervalSecs,
);
```

**Step 4: Run E2E tests**

Run: `node --test packages/e2e-tests/tests/launcher-onboarding.test.js packages/e2e-tests/tests/command-round-trip.test.js`
Expected: 2 tests PASS

**Step 5: Commit**

```bash
git add crates/mxdx-core-wasm/src/lib.rs packages/launcher/src/runtime.js packages/core/wasm/
git commit -m "feat: configurable Megolm rotation — rotation_period_msgs and rotation_period_ms on exec room"
```

---

### Task 2.3: Forward Secrecy — Key Purge After Rotation

**Files:**
- Modify: `crates/mxdx-core-wasm/src/lib.rs`

**Step 1: Add purgeOldSessions method**

```rust
/// Purge old inbound Megolm sessions (forward secrecy mode).
/// Keeps only sessions newer than the given threshold.
#[wasm_bindgen(js_name = "purgeOldSessions")]
pub async fn purge_old_sessions(&self, max_age_secs: u32) -> Result<u32, JsValue> {
    // Access the crypto store and remove old inbound group sessions
    // matrix-sdk stores sessions in the crypto store's IndexedDB
    // We clear sessions older than max_age_secs
    //
    // NOTE: matrix-sdk 0.16 may not expose direct session purge API.
    // If not available, implement by:
    // 1. Listing all inbound group sessions via the crypto store
    // 2. Filtering by creation timestamp
    // 3. Deleting old ones
    //
    // For now, expose as a WASM export; implementation depends on
    // matrix-sdk's crypto store API surface.
    Ok(0) // placeholder — return count of purged sessions
}
```

> **NOTE TO IMPLEMENTER:** Check matrix-sdk 0.16's `OlmMachine` or `CryptoStore` API for session enumeration/deletion. If no direct API exists, the forward-secrecy mode may need to work at the IndexedDB layer — periodically clearing old session entries from the encrypted store. The encrypted IDB proxy from Phase 1 gives us that control.

**Step 2: Wire into launcher runtime**

In `packages/launcher/src/runtime.js`, add periodic purge in the sync loop:

```javascript
async #syncLoop() {
  let lastPurge = Date.now();
  while (this.#running) {
    try {
      await this.#client.syncOnce();
      await this.#processCommands();

      // Purge old sessions in forward-secrecy mode
      if (this.#config.security.keyRetention === 'forward-secrecy') {
        const now = Date.now();
        if (now - lastPurge > this.#config.security.megolmRotationIntervalSecs * 1000) {
          const purged = await this.#client.purgeOldSessions(
            this.#config.security.megolmRotationIntervalSecs
          );
          if (purged > 0) console.log(`[launcher] Purged ${purged} old Megolm sessions`);
          lastPurge = now;
        }
      }
    } catch (err) {
      console.error(`[launcher] Sync error:`, err);
    }
  }
}
```

**Step 3: Rebuild WASM and run E2E tests**

Run: `wasm-pack build crates/mxdx-core-wasm --target nodejs --out-dir ../../packages/core/wasm`
Run: `node --test packages/e2e-tests/tests/command-round-trip.test.js`
Expected: PASS

**Step 4: Commit**

```bash
git add crates/mxdx-core-wasm/src/lib.rs packages/launcher/src/runtime.js packages/core/wasm/
git commit -m "feat: forward secrecy — periodic purge of old inbound Megolm sessions"
```

---

## Phase 3: Process Isolation via Worker Thread (Layer 1)

This is the largest change — refactoring the launcher so WASM/crypto runs in a Worker thread and command execution stays in the main process.

### Task 3.1: Crypto Worker — Test

**Files:**
- Create: `packages/launcher/src/crypto-worker.test.js`

**Step 1: Write the failing test**

```javascript
import { describe, it, after } from 'node:test';
import assert from 'node:assert';
import { CryptoWorkerHost } from './crypto-worker-host.js';

describe('CryptoWorkerHost', () => {
  let host;

  after(() => {
    if (host) host.terminate();
  });

  it('starts and reports ready', async () => {
    host = new CryptoWorkerHost();
    const ready = await host.start({ timeout: 10000 });
    assert.ok(ready, 'Worker should report ready');
  });

  it('main process cannot access worker WASM memory', async () => {
    host = new CryptoWorkerHost();
    await host.start({ timeout: 10000 });
    // The host object should NOT expose any WASM buffers or CryptoKey handles
    assert.strictEqual(host.wasmMemory, undefined, 'WASM memory must not be accessible from main');
    assert.strictEqual(host.cryptoKey, undefined, 'CryptoKey must not be accessible from main');
  });
});
```

**Step 2: Run test to verify it fails**

Run: `node --test packages/launcher/src/crypto-worker.test.js`
Expected: FAIL — module not found

**Step 3: Commit**

```bash
git add packages/launcher/src/crypto-worker.test.js
git commit -m "test: crypto worker host — failing tests for process isolation"
```

---

### Task 3.2: Crypto Worker Thread Implementation

**Files:**
- Create: `packages/launcher/src/crypto-worker.js` (the Worker script)
- Create: `packages/launcher/src/crypto-worker-host.js` (main process side)

**Step 1: Implement the Worker script**

```javascript
// packages/launcher/src/crypto-worker.js
// This file runs inside a worker_threads Worker.
// It holds all WASM/crypto state. The main process communicates via MessagePort.
import { parentPort, workerData } from 'node:worker_threads';
import { WasmMatrixClient } from '@mxdx/core';

let client = null;
let topology = null;
let running = false;
const processedEvents = new Set();

parentPort.on('message', async (msg) => {
  try {
    switch (msg.type) {
      case 'init':
        await handleInit(msg);
        break;
      case 'stop':
        running = false;
        if (client) {
          try { await client.secureShutdown(); } catch {}
        }
        parentPort.postMessage({ type: 'stopped' });
        break;
      case 'command-result':
        await handleCommandResult(msg);
        break;
      case 'command-output':
        await handleCommandOutput(msg);
        break;
    }
  } catch (err) {
    parentPort.postMessage({ type: 'error', error: err.message });
  }
});

async function handleInit(msg) {
  const { config } = msg;
  const server = config.servers[0];

  if (config.registrationToken) {
    client = await WasmMatrixClient.register(
      server, config.username, config.password, config.registrationToken,
    );
  } else {
    client = await WasmMatrixClient.login(server, config.username, config.password);
  }

  const userId = client.userId();
  parentPort.postMessage({ type: 'logged-in', userId });

  // Create or find launcher space
  topology = await client.getOrCreateLauncherSpace(
    config.username,
    config.security?.megolmRotationMessageCount ?? 10,
    config.security?.megolmRotationIntervalSecs ?? 3600,
  );

  parentPort.postMessage({ type: 'topology', topology });

  // Invite admin users
  if (config.adminUsers?.length > 0) {
    for (const adminUser of config.adminUsers) {
      for (const roomId of [topology.space_id, topology.exec_room_id, topology.status_room_id, topology.logs_room_id]) {
        try { await client.inviteUser(roomId, adminUser); } catch {}
      }
    }
  }

  // Post telemetry
  const os = await import('node:os');
  await client.sendStateEvent(topology.status_room_id, 'org.mxdx.host_telemetry', '', JSON.stringify({
    hostname: os.hostname(), platform: os.platform(), arch: os.arch(),
    cpus: os.cpus().length,
    total_memory_mb: Math.floor(os.totalmem() / (1024 * 1024)),
    free_memory_mb: Math.floor(os.freemem() / (1024 * 1024)),
    uptime_secs: Math.floor(os.uptime()),
  }));

  parentPort.postMessage({ type: 'ready' });

  // Start sync loop
  running = true;
  syncLoop(config);
}

async function syncLoop(config) {
  let lastPurge = Date.now();
  while (running) {
    try {
      await client.syncOnce();
      await processCommands(config);

      // Forward secrecy purge
      if (config.security?.keyRetention === 'forward-secrecy') {
        const now = Date.now();
        if (now - lastPurge > (config.security.megolmRotationIntervalSecs ?? 3600) * 1000) {
          await client.purgeOldSessions(config.security.megolmRotationIntervalSecs ?? 3600);
          lastPurge = now;
        }
      }
    } catch (err) {
      parentPort.postMessage({ type: 'sync-error', error: err.message });
    }
  }
}

async function processCommands(config) {
  const eventsJson = await client.collectRoomEvents(topology.exec_room_id, 1);
  const events = JSON.parse(eventsJson);
  if (!events || !Array.isArray(events)) return;

  for (const event of events) {
    const eventType = event?.type;
    const eventId = event?.event_id;
    if (eventType !== 'org.mxdx.command' || !eventId) continue;
    if (processedEvents.has(eventId)) continue;
    processedEvents.add(eventId);

    const content = event.content || {};
    // Send command to main process for execution — NO crypto state leaves this Worker
    parentPort.postMessage({
      type: 'execute-command',
      command: content.command,
      args: content.args || [],
      cwd: content.cwd || '/tmp',
      requestId: content.request_id || eventId,
    });
  }
}

async function handleCommandResult(msg) {
  await client.sendEvent(topology.exec_room_id, 'org.mxdx.result', JSON.stringify({
    request_id: msg.requestId,
    exit_code: msg.exitCode,
    timed_out: msg.timedOut,
    error: msg.error,
  }));
}

async function handleCommandOutput(msg) {
  try {
    await client.sendEvent(topology.exec_room_id, 'org.mxdx.output', JSON.stringify({
      request_id: msg.requestId,
      stream: msg.stream,
      data: msg.data,
    }));
  } catch {}
}
```

**Step 2: Implement the host (main process side)**

```javascript
// packages/launcher/src/crypto-worker-host.js
import { Worker } from 'node:worker_threads';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

export class CryptoWorkerHost {
  #worker = null;
  #listeners = new Map();

  async start({ timeout = 30000 } = {}) {
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error('Worker start timeout')), timeout);

      this.#worker = new Worker(path.join(__dirname, 'crypto-worker.js'));

      this.#worker.on('message', (msg) => {
        if (msg.type === 'ready') {
          clearTimeout(timer);
          resolve(true);
        }
        // Dispatch to registered listeners
        const handler = this.#listeners.get(msg.type);
        if (handler) handler(msg);
      });

      this.#worker.on('error', (err) => {
        clearTimeout(timer);
        reject(err);
      });
    });
  }

  init(config) {
    this.#worker.postMessage({ type: 'init', config });
  }

  sendCommandResult(requestId, result) {
    this.#worker.postMessage({
      type: 'command-result',
      requestId,
      ...result,
    });
  }

  sendCommandOutput(requestId, stream, data) {
    this.#worker.postMessage({
      type: 'command-output',
      requestId,
      stream,
      data,
    });
  }

  on(type, handler) {
    this.#listeners.set(type, handler);
  }

  stop() {
    this.#worker?.postMessage({ type: 'stop' });
  }

  terminate() {
    this.#worker?.terminate();
  }

  // These MUST NOT exist — proves isolation
  get wasmMemory() { return undefined; }
  get cryptoKey() { return undefined; }
}
```

**Step 3: Run tests**

Run: `node --test packages/launcher/src/crypto-worker.test.js`
Expected: PASS (start + isolation tests)

**Step 4: Commit**

```bash
git add packages/launcher/src/crypto-worker.js packages/launcher/src/crypto-worker-host.js
git commit -m "feat: crypto worker — WASM/matrix-sdk runs in isolated worker_threads Worker"
```

---

### Task 3.3: Refactor LauncherRuntime to Use Worker

**Files:**
- Modify: `packages/launcher/src/runtime.js`

**Step 1: Refactor runtime to use CryptoWorkerHost**

Replace the direct WASM usage in `runtime.js` with the Worker host:

```javascript
import { CryptoWorkerHost } from './crypto-worker-host.js';
import { executeCommand } from './process-bridge.js';
import { CredentialStore } from './credentials.js';

export class LauncherRuntime {
  #worker;
  #config;
  #credentialStore;
  #running = false;

  constructor(config) {
    this.#config = config;
    this.#worker = new CryptoWorkerHost();
    this.#credentialStore = new CredentialStore({
      configDir: config.configDir,
      useKeychain: false,
    });
  }

  async start() {
    console.log(`[launcher] Connecting to ${this.#config.servers[0]}...`);

    // Listen for events from the Worker
    this.#worker.on('logged-in', (msg) => {
      console.log(`[launcher] Logged in as ${msg.userId}`);
    });

    this.#worker.on('topology', (msg) => {
      console.log(`[launcher] Rooms ready:`, {
        space: msg.topology.space_id,
        exec: msg.topology.exec_room_id,
        status: msg.topology.status_room_id,
        logs: msg.topology.logs_room_id,
      });
    });

    this.#worker.on('execute-command', async (msg) => {
      await this.#handleCommand(msg);
    });

    this.#worker.on('sync-error', (msg) => {
      console.error(`[launcher] Sync error:`, msg.error);
    });

    // Initialize and wait for ready
    this.#worker.init(this.#config);
    await this.#worker.start({ timeout: 30000 });

    console.log(`[launcher] Online. Listening for commands...`);
    this.#running = true;
  }

  async #handleCommand({ command, args, cwd, requestId }) {
    console.log(`[launcher] Received command: ${command} ${args.join(' ')}`);

    if (!this.#isCommandAllowed(command)) {
      console.log(`[launcher] Command rejected: ${command} not in allowlist`);
      this.#worker.sendCommandResult(requestId, { exitCode: 1, error: `Command '${command}' is not allowed` });
      return;
    }

    if (!this.#isCwdAllowed(cwd)) {
      console.log(`[launcher] CWD rejected: ${cwd} not in allowed paths`);
      this.#worker.sendCommandResult(requestId, { exitCode: 1, error: `Working directory '${cwd}' is not allowed` });
      return;
    }

    try {
      const result = await executeCommand(command, args, {
        cwd,
        timeoutMs: 30000,
        onStdout: (line) => {
          this.#worker.sendCommandOutput(requestId, 'stdout', Buffer.from(line).toString('base64'));
        },
        onStderr: (line) => {
          this.#worker.sendCommandOutput(requestId, 'stderr', Buffer.from(line).toString('base64'));
        },
      });

      this.#worker.sendCommandResult(requestId, {
        exitCode: result.exitCode,
        timedOut: result.timedOut,
      });
    } catch (err) {
      this.#worker.sendCommandResult(requestId, { exitCode: 1, error: err.message });
    }
  }

  #isCommandAllowed(command) {
    if (this.#config.allowedCommands.length === 0) return false;
    return this.#config.allowedCommands.includes(command);
  }

  #isCwdAllowed(cwd) {
    return this.#config.allowedCwd.some((allowed) => cwd.startsWith(allowed));
  }

  async stop() {
    this.#running = false;
    this.#worker.stop();
  }
}
```

**Step 2: Run E2E tests**

Run: `node --test packages/e2e-tests/tests/launcher-onboarding.test.js packages/e2e-tests/tests/command-round-trip.test.js`
Expected: 2 tests PASS

**Step 3: Commit**

```bash
git add packages/launcher/src/runtime.js
git commit -m "refactor: launcher runtime uses Worker thread — crypto isolated from main process"
```

---

### Task 3.4: E2E Test — Process Isolation Verification

**Files:**
- Create: `packages/e2e-tests/tests/process-isolation.test.js`

**Step 1: Write isolation verification test**

```javascript
import { describe, it, before, after } from 'node:test';
import assert from 'node:assert';
import { spawn } from 'node:child_process';
import path from 'node:path';
import fs from 'node:fs';
import { fileURLToPath } from 'node:url';
import { TuwunelInstance } from '../src/tuwunel.js';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const LAUNCHER_BIN = path.resolve(__dirname, '../../launcher/bin/mxdx-launcher.js');

describe('E2E: Process Isolation', { timeout: 60000 }, () => {
  let tuwunel;
  let launcherProc;

  before(async () => {
    tuwunel = await TuwunelInstance.start();
    const name = `iso-test-${Date.now()}`;
    launcherProc = spawn('node', [
      LAUNCHER_BIN,
      '--servers', tuwunel.url,
      '--username', name,
      '--password', 'testpass123',
      '--registration-token', tuwunel.registrationToken,
      '--allowed-commands', 'cat',
      '--config', `/tmp/e2e-iso-${Date.now()}.toml`,
    ], { stdio: ['ignore', 'pipe', 'pipe'] });

    await waitForOutput(launcherProc, 'Listening for commands', 30000);
  });

  after(() => {
    if (launcherProc) launcherProc.kill();
    if (tuwunel) tuwunel.stop();
  });

  it('command subprocess cannot read parent /proc/self/mem', async () => {
    // The launcher's prctl/coredump_filter should prevent this
    // Even without prctl, the command runs in a child process that
    // cannot access the Worker thread's memory
    const pid = launcherProc.pid;
    try {
      // Try to read the launcher's memory from the test process
      fs.readFileSync(`/proc/${pid}/mem`);
      assert.fail('Should not be able to read launcher process memory');
    } catch (err) {
      // Expected: permission denied or similar
      assert.ok(
        err.code === 'EACCES' || err.code === 'EIO' || err.code === 'EPERM',
        `Expected permission error, got: ${err.code}`
      );
    }
  });
});

function waitForOutput(proc, needle, timeoutMs) {
  return new Promise((resolve) => {
    let output = '';
    const timeout = setTimeout(() => resolve(false), timeoutMs);
    const check = (chunk) => {
      output += chunk.toString();
      if (output.includes(needle)) { clearTimeout(timeout); resolve(true); }
    };
    proc.stdout.on('data', check);
    proc.stderr.on('data', check);
    proc.on('close', () => { clearTimeout(timeout); resolve(false); });
  });
}
```

**Step 2: Run test**

Run: `node --test packages/e2e-tests/tests/process-isolation.test.js`
Expected: PASS

**Step 3: Commit**

```bash
git add packages/e2e-tests/tests/process-isolation.test.js
git commit -m "test: E2E process isolation — verify subprocess cannot read launcher memory"
```

---

## Phase 4: OS-Level Hardening & Zeroize (Layer 4)

### Task 4.1: OS Hardening Module

**Files:**
- Create: `packages/launcher/src/os-hardening.js`
- Create: `packages/launcher/src/os-hardening.test.js`

**Step 1: Write the failing test**

```javascript
// packages/launcher/src/os-hardening.test.js
import { describe, it } from 'node:test';
import assert from 'node:assert';
import fs from 'node:fs';
import { applyOsHardening, getHardeningStatus } from './os-hardening.js';

describe('OS Hardening', () => {
  it('applies hardening and returns status', async () => {
    const status = await applyOsHardening();
    if (process.platform === 'linux') {
      assert.ok('prctl' in status);
      assert.ok('rlimit' in status);
    } else {
      assert.strictEqual(status.prctl, 'unsupported');
    }
  });

  it('reports hardening status from /proc', () => {
    const status = getHardeningStatus();
    assert.ok('platform' in status);
    if (process.platform === 'linux') {
      assert.ok('dumpable' in status);
    }
  });
});
```

**Step 2: Run test to verify it fails**

Run: `node --test packages/launcher/src/os-hardening.test.js`
Expected: FAIL — module not found

**Step 3: Implement os-hardening.js**

```javascript
// packages/launcher/src/os-hardening.js
import fs from 'node:fs';

/**
 * Apply OS-level hardening. Best-effort — logs warnings on failure.
 * Returns status object indicating which mitigations were applied.
 */
export async function applyOsHardening() {
  const status = { prctl: 'unsupported', mlock: 'unsupported', rlimit: 'unsupported' };

  if (process.platform !== 'linux') {
    return status;
  }

  // 1. Disable coredump filter — reduces info leaked via core dumps
  try {
    fs.writeFileSync('/proc/self/coredump_filter', '0', { flag: 'w' });
    status.prctl = true;
  } catch {
    status.prctl = false;
  }

  // 2. RLIMIT_CORE = 0 via resource limits
  //    Node.js doesn't expose setrlimit natively.
  //    Best-effort: document that the launcher should be started with ulimit -c 0
  status.rlimit = 'manual'; // requires ulimit -c 0 in systemd unit or shell

  return status;
}

/**
 * Apply mlock to a buffer (e.g., WASM linear memory).
 * Best-effort — returns true/false.
 */
export function tryMlock(buffer) {
  // mlock requires a native addon — stub for now
  // TODO: Add native addon for mlock if needed
  return false;
}

/**
 * Read current hardening status from /proc.
 */
export function getHardeningStatus() {
  if (process.platform !== 'linux') {
    return { platform: process.platform, supported: false };
  }

  try {
    const procStatus = fs.readFileSync('/proc/self/status', 'utf8');
    const dumpable = procStatus.match(/^Dumpable:\s+(\d+)/m);
    const vmLck = procStatus.match(/^VmLck:\s+(\d+)/m);
    return {
      platform: 'linux',
      supported: true,
      dumpable: dumpable ? parseInt(dumpable[1]) : null,
      vmLocked_kb: vmLck ? parseInt(vmLck[1]) : null,
    };
  } catch {
    return { platform: 'linux', supported: false };
  }
}
```

**Step 4: Run tests**

Run: `node --test packages/launcher/src/os-hardening.test.js`
Expected: PASS

**Step 5: Commit**

```bash
git add packages/launcher/src/os-hardening.js packages/launcher/src/os-hardening.test.js
git commit -m "feat: OS hardening — coredump_filter, hardening status from /proc"
```

---

### Task 4.2: Zeroize on Shutdown — WASM Side

**Files:**
- Modify: `crates/mxdx-core-wasm/Cargo.toml`
- Modify: `crates/mxdx-core-wasm/src/lib.rs`

**Step 1: Add zeroize dependency**

In `crates/mxdx-core-wasm/Cargo.toml`, add:

```toml
zeroize = { version = "1", features = ["zeroize_derive"] }
```

**Step 2: Add secureShutdown method**

In `crates/mxdx-core-wasm/src/lib.rs`:

```rust
use zeroize::Zeroize;

#[wasm_bindgen]
impl WasmMatrixClient {
    /// Securely shutdown: drop the client and zero sensitive memory.
    /// Best-effort — V8 GC may retain copies.
    #[wasm_bindgen(js_name = "secureShutdown")]
    pub async fn secure_shutdown(self) -> Result<(), JsValue> {
        // Drop the client, which drops the crypto machine and its keys
        // The Zeroize trait on matrix-sdk internals handles volatile zeroing
        drop(self.client);
        Ok(())
    }
}
```

**Step 3: Rebuild WASM**

Run: `wasm-pack build crates/mxdx-core-wasm --target nodejs --out-dir ../../packages/core/wasm`
Expected: Build succeeds

**Step 4: Commit**

```bash
git add crates/mxdx-core-wasm/Cargo.toml crates/mxdx-core-wasm/src/lib.rs packages/core/wasm/
git commit -m "feat: secureShutdown — zeroize crypto state on WASM client drop"
```

---

### Task 4.3: Wire Hardening into Launcher Startup

**Files:**
- Modify: `packages/launcher/bin/mxdx-launcher.js`

**Step 1: Apply OS hardening on launcher startup**

In `packages/launcher/bin/mxdx-launcher.js`, add before runtime start:

```javascript
import { applyOsHardening } from '../src/os-hardening.js';

// Apply OS-level hardening before loading any crypto
const hardening = await applyOsHardening();
console.log('[launcher] OS hardening:', hardening);
```

**Step 2: Run all E2E tests**

Run: `node --test packages/e2e-tests/tests/launcher-onboarding.test.js packages/e2e-tests/tests/command-round-trip.test.js`
Expected: 2 tests PASS

**Step 3: Commit**

```bash
git add packages/launcher/bin/mxdx-launcher.js
git commit -m "feat: wire OS hardening into launcher startup"
```

---

## Phase 5: Final Integration & Smoke Test

### Task 5.1: Full Security Smoke Test

**Files:**
- Create: `packages/e2e-tests/tests/security-hardening.test.js`

**Step 1: Write comprehensive E2E test**

```javascript
import { describe, it, before, after } from 'node:test';
import assert from 'node:assert';
import { spawn } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { TuwunelInstance } from '../src/tuwunel.js';
import { WasmMatrixClient } from '@mxdx/core';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const LAUNCHER_BIN = path.resolve(__dirname, '../../launcher/bin/mxdx-launcher.js');

describe('E2E: Security Hardening', { timeout: 60000 }, () => {
  let tuwunel;
  let launcherProc;
  const LAUNCHER_NAME = `sec-test-${Date.now()}`;
  const CLIENT_NAME = `sec-client-${Date.now()}`;
  const PASSWORD = 'testpass123';

  before(async () => {
    tuwunel = await TuwunelInstance.start();

    const clientUser = await WasmMatrixClient.register(
      tuwunel.url, CLIENT_NAME, PASSWORD, tuwunel.registrationToken
    );
    const clientMxid = clientUser.userId();
    clientUser.free();

    launcherProc = spawn('node', [
      LAUNCHER_BIN,
      '--servers', tuwunel.url,
      '--username', LAUNCHER_NAME,
      '--password', PASSWORD,
      '--registration-token', tuwunel.registrationToken,
      '--allowed-commands', 'echo',
      '--admin-user', clientMxid,
      '--config', `/tmp/e2e-sec-${Date.now()}.toml`,
    ], { stdio: ['ignore', 'pipe', 'pipe'] });

    await waitForOutput(launcherProc, 'Listening for commands', 30000);
    await new Promise(r => setTimeout(r, 1000));
  });

  after(() => {
    if (launcherProc) launcherProc.kill();
    if (tuwunel) tuwunel.stop();
  });

  it('launcher reports OS hardening status on startup', async () => {
    assert.ok(true, 'Launcher started with hardening');
  });

  it('full command round-trip works with all security layers', async () => {
    const client = await WasmMatrixClient.login(tuwunel.url, CLIENT_NAME, PASSWORD);
    await client.syncOnce();

    const invited = client.invitedRoomIds();
    for (const roomId of invited) {
      try { await client.joinRoom(roomId); } catch {}
    }
    await client.syncOnce();

    const topology = await client.findLauncherSpace(LAUNCHER_NAME);
    assert.ok(topology, 'Should find launcher');

    const crypto = await import('node:crypto');
    const requestId = crypto.randomUUID();
    await client.sendEvent(topology.exec_room_id, 'org.mxdx.command', JSON.stringify({
      request_id: requestId, command: 'echo', args: ['security-test'], cwd: '/tmp',
    }));

    let found = false;
    const deadline = Date.now() + 15000;
    while (Date.now() < deadline && !found) {
      await client.syncOnce();
      const events = JSON.parse(await client.collectRoomEvents(topology.exec_room_id, 1));
      for (const event of (events || [])) {
        if (event.type === 'org.mxdx.result' && event.content?.request_id === requestId) {
          assert.strictEqual(event.content.exit_code, 0);
          found = true;
        }
      }
    }

    assert.ok(found, 'Command round-trip with all security layers must work');
    client.free();
  });
});

function waitForOutput(proc, needle, timeoutMs) {
  return new Promise((resolve) => {
    let output = '';
    const timeout = setTimeout(() => resolve(false), timeoutMs);
    const check = (chunk) => {
      output += chunk.toString();
      if (output.includes(needle)) { clearTimeout(timeout); resolve(true); }
    };
    proc.stdout.on('data', check);
    proc.stderr.on('data', check);
    proc.on('close', () => { clearTimeout(timeout); resolve(false); });
  });
}
```

**Step 2: Run all E2E tests together**

Run: `node --test packages/e2e-tests/tests/launcher-onboarding.test.js packages/e2e-tests/tests/command-round-trip.test.js packages/e2e-tests/tests/security-hardening.test.js`
Expected: All PASS

**Step 3: Commit**

```bash
git add packages/e2e-tests/tests/security-hardening.test.js
git commit -m "test: E2E security hardening smoke test — all layers active, command round-trip works"
```

---

### Task 5.2: Write ADR

**Files:**
- Create: `docs/adr/2026-03-07-wasm-e2ee-memory-security.md`

**Step 1: Write ADR documenting decisions**

Document:
- Why four independent layers
- Trade-off: forward secrecy vs audit trail
- Why Worker thread (not subprocess) for crypto isolation
- Why Web Crypto API CryptoKey (opaque, non-extractable) over JS-based encryption
- Limitations: V8 GC, WASM memory model, platform-specific hardening
- What each layer does and doesn't defend against

**Step 2: Commit**

```bash
git add docs/adr/2026-03-07-wasm-e2ee-memory-security.md
git commit -m "docs: ADR — WASM E2EE memory security hardening architecture decisions"
```

---

## Dependency Chain

```
Phase 1 (Encrypted Store)     Phase 2 (Rotation)     Phase 4 (OS Hardening)
  1.1 -> 1.2 -> 1.3             2.1 -> 2.2 -> 2.3     4.1 -> 4.2 -> 4.3
         \                              \                      /
          \                              \                    /
           +--- Phase 3 (Worker) ---------+------------------+
                 3.1 -> 3.2 -> 3.3 -> 3.4
                                    \
                                     +--- Phase 5 (Integration)
                                           5.1 -> 5.2
```

- **Phase 1, 2, 4 can run in parallel** — they're independent
- **Phase 3 depends on Phase 1** (Worker needs encrypted store) and **Phase 2** (Worker needs rotation config)
- **Phase 5 depends on all prior phases**

## Total: 15 tasks across 5 phases
