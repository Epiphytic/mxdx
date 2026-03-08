/**
 * Persistent IndexedDB for Node.js.
 *
 * Snapshots all fake-indexeddb databases (schema + data) to an encrypted
 * file on disk, enabling matrix-sdk crypto key persistence across process
 * restarts. Browser environments have real persistent IndexedDB and don't
 * need this module.
 *
 * Encryption: AES-256-GCM with a machine-derived key (hostname + uid).
 */
import 'fake-indexeddb/auto';
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import os from 'node:os';

const DEFAULT_DIR = path.join(os.homedir(), '.config', 'mxdx');
const SNAPSHOT_FILE = 'indexeddb-snapshot.enc';

// ── Encryption helpers (same scheme as credentials.js) ───────────

function deriveKey() {
  const material = `${os.hostname()}:${os.userInfo().uid}:mxdx-indexeddb-store`;
  return crypto.createHash('sha256').update(material).digest();
}

function encrypt(plaintext, key) {
  const iv = crypto.randomBytes(16);
  const cipher = crypto.createCipheriv('aes-256-gcm', key, iv);
  const encrypted = Buffer.concat([cipher.update(plaintext, 'utf8'), cipher.final()]);
  const tag = cipher.getAuthTag();
  return Buffer.concat([iv, tag, encrypted]).toString('base64');
}

function decrypt(ciphertext, key) {
  const buf = Buffer.from(ciphertext, 'base64');
  const iv = buf.subarray(0, 16);
  const tag = buf.subarray(16, 32);
  const encrypted = buf.subarray(32);
  const decipher = crypto.createDecipheriv('aes-256-gcm', key, iv);
  decipher.setAuthTag(tag);
  return decipher.update(encrypted, null, 'utf8') + decipher.final('utf8');
}

// ── Structured clone serialization ───────────────────────────────
// IndexedDB values may contain ArrayBuffer, TypedArrays, Date, etc.
// JSON.stringify can't handle these, so we tag them for round-tripping.

function serialize(value) {
  if (value === null || value === undefined) return value;
  if (value instanceof ArrayBuffer) {
    return { __t: 'AB', d: Buffer.from(value).toString('base64') };
  }
  if (ArrayBuffer.isView(value)) {
    return {
      __t: 'TV',
      c: value.constructor.name,
      d: Buffer.from(value.buffer, value.byteOffset, value.byteLength).toString('base64'),
    };
  }
  if (value instanceof Date) {
    return { __t: 'D', d: value.toISOString() };
  }
  if (value instanceof Map) {
    return { __t: 'M', d: [...value.entries()].map(([k, v]) => [serialize(k), serialize(v)]) };
  }
  if (value instanceof Set) {
    return { __t: 'S', d: [...value].map(serialize) };
  }
  if (Array.isArray(value)) {
    return value.map(serialize);
  }
  if (typeof value === 'object') {
    const result = {};
    for (const [k, v] of Object.entries(value)) {
      result[k] = serialize(v);
    }
    return result;
  }
  return value;
}

function deserialize(value) {
  if (value === null || value === undefined) return value;
  if (typeof value === 'object' && value.__t) {
    switch (value.__t) {
      case 'AB':
        return Buffer.from(value.d, 'base64').buffer;
      case 'TV': {
        const buf = Buffer.from(value.d, 'base64');
        const ab = new ArrayBuffer(buf.byteLength);
        new Uint8Array(ab).set(buf);
        const ctor = globalThis[value.c] || Uint8Array;
        return new ctor(ab);
      }
      case 'D':
        return new Date(value.d);
      case 'M':
        return new Map(value.d.map(([k, v]) => [deserialize(k), deserialize(v)]));
      case 'S':
        return new Set(value.d.map(deserialize));
    }
  }
  if (Array.isArray(value)) return value.map(deserialize);
  if (typeof value === 'object') {
    const result = {};
    for (const [k, v] of Object.entries(value)) {
      result[k] = deserialize(v);
    }
    return result;
  }
  return value;
}

// ── IDB helpers ──────────────────────────────────────────────────

function idbOpen(name, version) {
  return new Promise((resolve, reject) => {
    const req = indexedDB.open(name, version);
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
}

function idbTxComplete(tx) {
  return new Promise((resolve, reject) => {
    tx.oncomplete = resolve;
    tx.onerror = () => reject(tx.error);
  });
}

function idbCursorAll(store) {
  return new Promise((resolve, reject) => {
    const records = [];
    const req = store.openCursor();
    req.onsuccess = () => {
      const cursor = req.result;
      if (cursor) {
        records.push({ key: serialize(cursor.key), value: serialize(cursor.value) });
        cursor.continue();
      } else {
        resolve(records);
      }
    };
    req.onerror = () => reject(req.error);
  });
}

// ── Snapshot: dump all databases ─────────────────────────────────

async function dumpDatabases() {
  const databases = await indexedDB.databases();
  const snapshot = {};

  for (const { name, version } of databases) {
    if (!name) continue;
    const db = await idbOpen(name, version);
    const dbData = { version, stores: {} };

    for (const storeName of db.objectStoreNames) {
      const tx = db.transaction(storeName, 'readonly');
      const store = tx.objectStore(storeName);

      const storeData = {
        keyPath: store.keyPath,
        autoIncrement: store.autoIncrement,
        indices: [],
        records: [],
      };

      for (const indexName of store.indexNames) {
        const index = store.index(indexName);
        storeData.indices.push({
          name: indexName,
          keyPath: index.keyPath,
          unique: index.unique,
          multiEntry: index.multiEntry,
        });
      }

      storeData.records = await idbCursorAll(store);
      dbData.stores[storeName] = storeData;
    }

    db.close();
    snapshot[name] = dbData;
  }

  return snapshot;
}

// ── Snapshot: restore databases ──────────────────────────────────

async function restoreDatabases(snapshot) {
  for (const [dbName, dbData] of Object.entries(snapshot)) {
    // Open with version to trigger onupgradeneeded and create schema
    const db = await new Promise((resolve, reject) => {
      const req = indexedDB.open(dbName, dbData.version);
      req.onupgradeneeded = () => {
        const db = req.result;
        for (const [storeName, storeData] of Object.entries(dbData.stores)) {
          const opts = {};
          if (storeData.keyPath !== null && storeData.keyPath !== undefined) {
            opts.keyPath = storeData.keyPath;
          }
          if (storeData.autoIncrement) opts.autoIncrement = true;
          const store = db.createObjectStore(storeName, opts);
          for (const idx of storeData.indices) {
            store.createIndex(idx.name, idx.keyPath, {
              unique: idx.unique,
              multiEntry: idx.multiEntry,
            });
          }
        }
      };
      req.onsuccess = () => resolve(req.result);
      req.onerror = () => reject(req.error);
    });

    // Populate records
    for (const [storeName, storeData] of Object.entries(dbData.stores)) {
      if (storeData.records.length === 0) continue;
      const tx = db.transaction(storeName, 'readwrite');
      const store = tx.objectStore(storeName);
      for (const record of storeData.records) {
        const value = deserialize(record.value);
        if (storeData.keyPath !== null && storeData.keyPath !== undefined) {
          store.put(value);
        } else {
          store.put(value, deserialize(record.key));
        }
      }
      await idbTxComplete(tx);
    }

    db.close();
  }
}

// ── Public API ───────────────────────────────────────────────────

/**
 * Save all IndexedDB databases to an encrypted file on disk.
 * Call after sync operations to persist crypto keys.
 * @param {string} [configDir] - Config directory (default: ~/.config/mxdx)
 */
export async function saveIndexedDB(configDir = DEFAULT_DIR) {
  const snapshot = await dumpDatabases();
  if (Object.keys(snapshot).length === 0) return;

  const json = JSON.stringify(snapshot);
  const key = deriveKey();
  const encrypted = encrypt(json, key);

  fs.mkdirSync(configDir, { recursive: true, mode: 0o700 });
  const filePath = path.join(configDir, SNAPSHOT_FILE);
  fs.writeFileSync(filePath, encrypted, { mode: 0o600 });
}

/**
 * Restore IndexedDB databases from an encrypted file on disk.
 * Call before WASM client login/restore to rehydrate crypto keys.
 * No-op if snapshot file doesn't exist.
 * @param {string} [configDir] - Config directory (default: ~/.config/mxdx)
 * @returns {Promise<boolean>} true if snapshot was restored
 */
export async function restoreIndexedDB(configDir = DEFAULT_DIR) {
  const filePath = path.join(configDir, SNAPSHOT_FILE);
  if (!fs.existsSync(filePath)) return false;

  try {
    const encrypted = fs.readFileSync(filePath, 'utf8');
    const key = deriveKey();
    const json = decrypt(encrypted, key);
    const snapshot = JSON.parse(json);
    await restoreDatabases(snapshot);
    return true;
  } catch {
    // Snapshot corrupted or key changed — start fresh
    return false;
  }
}
