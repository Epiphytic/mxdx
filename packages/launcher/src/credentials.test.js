import { describe, it, beforeEach, afterEach } from 'node:test';
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { CredentialStore } from './credentials.js';

describe('CredentialStore', () => {
  let tmpDir;
  let store;

  beforeEach(() => {
    tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'mxdx-cred-test-'));
    store = new CredentialStore({ configDir: tmpDir, useKeychain: false });
  });

  afterEach(() => {
    fs.rmSync(tmpDir, { recursive: true });
  });

  it('saves and loads credentials', async () => {
    await store.save({
      serverUrl: 'https://matrix.org',
      username: 'testuser',
      accessToken: 'syt_abc123',
      deviceId: 'ABCDEF',
    });
    const loaded = await store.load();
    assert.strictEqual(loaded.serverUrl, 'https://matrix.org');
    assert.strictEqual(loaded.username, 'testuser');
    assert.strictEqual(loaded.accessToken, 'syt_abc123');
    assert.strictEqual(loaded.deviceId, 'ABCDEF');
  });

  it('returns null when no credentials exist', async () => {
    const loaded = await store.load();
    assert.strictEqual(loaded, null);
  });

  it('encrypts credentials on disk', async () => {
    await store.save({
      serverUrl: 'https://matrix.org',
      username: 'testuser',
      accessToken: 'syt_secret',
      deviceId: 'DEV1',
    });
    const raw = fs.readFileSync(path.join(tmpDir, 'credentials.enc'), 'utf8');
    assert.ok(!raw.includes('syt_secret'), 'Credentials should be encrypted on disk');
  });
});
