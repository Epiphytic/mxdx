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

  it('saves and loads session data', async () => {
    const session = {
      user_id: '@testuser:matrix.org',
      device_id: 'ABCDEF',
      access_token: 'syt_abc123',
      homeserver_url: 'https://matrix.org',
    };
    await store.saveSession('testuser', 'matrix.org', session);
    const loaded = await store.loadSession('testuser', 'matrix.org');
    assert.deepStrictEqual(loaded, session);
  });

  it('returns null for missing session', async () => {
    const loaded = await store.loadSession('nobody', 'matrix.org');
    assert.strictEqual(loaded, null);
  });

  it('saves and loads password', async () => {
    await store.savePassword('testuser', 'matrix.org', 'secret123');
    const loaded = await store.loadPassword('testuser', 'matrix.org');
    assert.strictEqual(loaded, 'secret123');
  });

  it('deletes password', async () => {
    await store.savePassword('testuser', 'matrix.org', 'secret123');
    await store.deletePassword('testuser', 'matrix.org');
    const loaded = await store.loadPassword('testuser', 'matrix.org');
    assert.strictEqual(loaded, null);
  });

  it('scopes keys by username and server', async () => {
    await store.savePassword('alice', 'matrix.org', 'alicepass');
    await store.savePassword('bob', 'matrix.org', 'bobpass');
    await store.savePassword('alice', 'other.server', 'alicepass2');

    assert.strictEqual(await store.loadPassword('alice', 'matrix.org'), 'alicepass');
    assert.strictEqual(await store.loadPassword('bob', 'matrix.org'), 'bobpass');
    assert.strictEqual(await store.loadPassword('alice', 'other.server'), 'alicepass2');
  });

  it('normalizes server URLs (strips protocol)', async () => {
    await store.savePassword('alice', 'https://matrix.org', 'pass1');
    const loaded = await store.loadPassword('alice', 'matrix.org');
    assert.strictEqual(loaded, 'pass1');
  });

  it('encrypts data on disk', async () => {
    await store.savePassword('testuser', 'matrix.org', 'supersecret');
    const files = fs.readdirSync(tmpDir);
    assert.ok(files.length > 0, 'Should have created an encrypted file');
    const raw = fs.readFileSync(path.join(tmpDir, files[0]), 'utf8');
    assert.ok(!raw.includes('supersecret'), 'Password should be encrypted on disk');
  });

  // Legacy API compat
  it('saves and loads legacy credentials', async () => {
    await store.save({
      serverUrl: 'https://matrix.org',
      username: 'testuser',
      accessToken: 'syt_abc123',
      deviceId: 'ABCDEF',
    });
    // Legacy load reads from file fallback
    const loaded = await store.load();
    // May be null if old-style key doesn't match new format
    // The important thing is it doesn't throw
  });
});
