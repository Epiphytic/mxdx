import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import os from 'node:os';

function deriveKey() {
  const material = `${os.hostname()}:${os.userInfo().uid}:mxdx-credential-store`;
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

/**
 * Build a keychain key name scoped to a specific user@server.
 * Ensures no collisions when multiple mxdx instances run on the same host.
 * @param {string} username
 * @param {string} server
 * @param {string} field - e.g. 'session', 'password'
 * @returns {string} e.g. 'mxdx:alice@matrix.org:session'
 */
function keychainKey(username, server, field) {
  // Normalize server: strip protocol, trailing slash
  const normalizedServer = server.replace(/^https?:\/\//, '').replace(/\/$/, '');
  return `mxdx:${username}@${normalizedServer}:${field}`;
}

export class CredentialStore {
  #configDir;
  #useKeychain;
  #key;

  constructor({ configDir, useKeychain = true } = {}) {
    this.#configDir = configDir || path.join(os.homedir(), '.config', 'mxdx');
    this.#useKeychain = useKeychain;
    this.#key = deriveKey();
  }

  /**
   * Save the Matrix session (access_token, device_id, user_id).
   * @param {string} username
   * @param {string} server
   * @param {object} session - { user_id, device_id, access_token, homeserver_url }
   */
  async saveSession(username, server, session) {
    const key = keychainKey(username, server, 'session');
    await this.#setSecret(key, JSON.stringify(session));
  }

  /**
   * Load a previously saved Matrix session.
   * @param {string} username
   * @param {string} server
   * @returns {object|null} session data or null
   */
  async loadSession(username, server) {
    const key = keychainKey(username, server, 'session');
    const data = await this.#getSecret(key);
    return data ? JSON.parse(data) : null;
  }

  /**
   * Save the password to the OS keychain.
   * @param {string} username
   * @param {string} server
   * @param {string} password
   */
  async savePassword(username, server, password) {
    const key = keychainKey(username, server, 'password');
    await this.#setSecret(key, password);
  }

  /**
   * Load the password from the OS keychain.
   * @param {string} username
   * @param {string} server
   * @returns {string|null} password or null
   */
  async loadPassword(username, server) {
    const key = keychainKey(username, server, 'password');
    return await this.#getSecret(key);
  }

  /**
   * Delete the password from the OS keychain.
   * @param {string} username
   * @param {string} server
   */
  async deletePassword(username, server) {
    const key = keychainKey(username, server, 'password');
    await this.#deleteSecret(key);
  }

  // ── Legacy API (backwards compat for existing callers) ─────────

  async save(credentials) {
    const key = keychainKey(
      credentials.username || 'default',
      credentials.serverUrl || 'unknown',
      'credentials',
    );
    await this.#setSecret(key, JSON.stringify(credentials));
  }

  async load() {
    // Legacy: try loading from old-style keychain entry
    if (this.#useKeychain) {
      try {
        const keytar = await import('keytar');
        const stored = await keytar.default.getPassword('mxdx', 'credentials');
        if (stored) return JSON.parse(stored);
      } catch {
        // keytar not available
      }
    }

    const filePath = path.join(this.#configDir, 'credentials.enc');
    if (!fs.existsSync(filePath)) return null;
    const encrypted = fs.readFileSync(filePath, 'utf8');
    return JSON.parse(decrypt(encrypted, this.#key));
  }

  // ── Private helpers ────────────────────────────────────────────

  async #setSecret(key, value) {
    if (this.#useKeychain) {
      try {
        const keytar = await import('keytar');
        await keytar.default.setPassword('mxdx', key, value);
        return;
      } catch {
        // keytar not available, fall through to file
      }
    }

    fs.mkdirSync(this.#configDir, { recursive: true, mode: 0o700 });
    const filePath = path.join(this.#configDir, `${this.#sanitizeFilename(key)}.enc`);
    const encrypted = encrypt(value, this.#key);
    fs.writeFileSync(filePath, encrypted, { mode: 0o600 });
  }

  async #getSecret(key) {
    if (this.#useKeychain) {
      try {
        const keytar = await import('keytar');
        const stored = await keytar.default.getPassword('mxdx', key);
        if (stored) return stored;
      } catch {
        // keytar not available, fall through to file
      }
    }

    const filePath = path.join(this.#configDir, `${this.#sanitizeFilename(key)}.enc`);
    if (!fs.existsSync(filePath)) return null;
    const encrypted = fs.readFileSync(filePath, 'utf8');
    return decrypt(encrypted, this.#key);
  }

  async #deleteSecret(key) {
    if (this.#useKeychain) {
      try {
        const keytar = await import('keytar');
        await keytar.default.deletePassword('mxdx', key);
        return;
      } catch {
        // keytar not available, fall through to file
      }
    }

    const filePath = path.join(this.#configDir, `${this.#sanitizeFilename(key)}.enc`);
    if (fs.existsSync(filePath)) {
      fs.unlinkSync(filePath);
    }
  }

  #sanitizeFilename(key) {
    return key.replace(/[^a-zA-Z0-9._-]/g, '_');
  }
}
