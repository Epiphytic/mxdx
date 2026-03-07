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

export class CredentialStore {
  #configDir;
  #useKeychain;
  #key;

  constructor({ configDir, useKeychain = true } = {}) {
    this.#configDir = configDir || path.join(os.homedir(), '.config', 'mxdx');
    this.#useKeychain = useKeychain;
    this.#key = deriveKey();
  }

  async save(credentials) {
    if (this.#useKeychain) {
      try {
        const keytar = await import('keytar');
        await keytar.default.setPassword('mxdx', 'credentials', JSON.stringify(credentials));
        return;
      } catch {
        // keytar not available, fall through to file
      }
    }

    fs.mkdirSync(this.#configDir, { recursive: true, mode: 0o700 });
    const filePath = path.join(this.#configDir, 'credentials.enc');
    const encrypted = encrypt(JSON.stringify(credentials), this.#key);
    fs.writeFileSync(filePath, encrypted, { mode: 0o600 });
  }

  async load() {
    if (this.#useKeychain) {
      try {
        const keytar = await import('keytar');
        const stored = await keytar.default.getPassword('mxdx', 'credentials');
        if (stored) return JSON.parse(stored);
      } catch {
        // keytar not available, fall through to file
      }
    }

    const filePath = path.join(this.#configDir, 'credentials.enc');
    if (!fs.existsSync(filePath)) return null;
    const encrypted = fs.readFileSync(filePath, 'utf8');
    return JSON.parse(decrypt(encrypted, this.#key));
  }
}
