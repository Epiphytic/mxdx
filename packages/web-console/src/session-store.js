/**
 * Browser session store using IndexedDB.
 * Stores the Matrix session (access_token, device_id, etc.) in IndexedDB
 * alongside the crypto store, rather than in localStorage where it's
 * visible in DevTools.
 *
 * This is the browser equivalent of the Node.js CredentialStore which
 * uses the OS keychain via keytar.
 */

const DB_NAME = 'mxdx-session';
const STORE_NAME = 'session';
const SESSION_KEY = 'current';

function openDB() {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open(DB_NAME, 1);
    request.onupgradeneeded = () => {
      request.result.createObjectStore(STORE_NAME);
    };
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}

/**
 * Save session JSON to IndexedDB.
 * @param {string} sessionJson - Serialized session from exportSession()
 */
export async function saveSession(sessionJson) {
  const db = await openDB();
  return new Promise((resolve, reject) => {
    const tx = db.transaction(STORE_NAME, 'readwrite');
    tx.objectStore(STORE_NAME).put(sessionJson, SESSION_KEY);
    tx.oncomplete = () => { db.close(); resolve(); };
    tx.onerror = () => { db.close(); reject(tx.error); };
  });
}

/**
 * Load session JSON from IndexedDB.
 * @returns {Promise<string|null>} Session JSON or null
 */
export async function loadSession() {
  const db = await openDB();
  return new Promise((resolve, reject) => {
    const tx = db.transaction(STORE_NAME, 'readonly');
    const request = tx.objectStore(STORE_NAME).get(SESSION_KEY);
    request.onsuccess = () => { db.close(); resolve(request.result ?? null); };
    request.onerror = () => { db.close(); reject(request.error); };
  });
}

/**
 * Remove session from IndexedDB.
 */
export async function clearSession() {
  const db = await openDB();
  return new Promise((resolve, reject) => {
    const tx = db.transaction(STORE_NAME, 'readwrite');
    tx.objectStore(STORE_NAME).delete(SESSION_KEY);
    tx.oncomplete = () => { db.close(); resolve(); };
    tx.onerror = () => { db.close(); reject(tx.error); };
  });
}
