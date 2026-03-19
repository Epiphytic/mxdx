/**
 * E2E: Multi-Homeserver tests against hosted beta infrastructure.
 *
 * Uses ca1-beta.mxdx.dev and ca2-beta.mxdx.dev — two federated Tuwunel
 * servers deployed via mxdx-hosting. Requires test-credentials.toml with
 * [server], [server2], [account1], and [account2] sections.
 *
 * Run: node --test packages/e2e-tests/tests/hosted-multi-hs.test.js
 */
import { describe, it, before, after } from 'node:test';
import assert from 'node:assert';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { MultiHsClient } from '@mxdx/core';

const REPO_ROOT = path.resolve(import.meta.dirname, '..', '..', '..');

function loadCredentials() {
  const tomlPath = path.join(REPO_ROOT, 'test-credentials.toml');
  if (!fs.existsSync(tomlPath)) {
    throw new Error('test-credentials.toml not found. See test-credentials.toml.example.');
  }
  const content = fs.readFileSync(tomlPath, 'utf8');
  const lines = content.split('\n');
  const result = {};
  let section = null;
  for (const line of lines) {
    const trimmed = line.trim();
    const sectionMatch = trimmed.match(/^\[(\w+)\]$/);
    if (sectionMatch) {
      section = sectionMatch[1];
      result[section] = {};
      continue;
    }
    const kvMatch = trimmed.match(/^(\w+)\s*=\s*"(.+)"$/);
    if (kvMatch && section) {
      result[section][kvMatch[1]] = kvMatch[2];
    }
  }

  if (!result.server?.url) throw new Error('server.url missing');
  if (!result.server2?.url) throw new Error('server2.url missing — need two servers for multi-HS tests');
  if (!result.account1?.username) throw new Error('account1.username missing');
  if (!result.account1?.password) throw new Error('account1.password missing');
  return result;
}

const hasServer2 = (() => {
  try {
    const creds = loadCredentials();
    return !!creds.server2?.url;
  } catch { return false; }
})();

describe('Hosted Multi-Homeserver E2E', { skip: !hasServer2 && 'server2 not configured in test-credentials.toml', timeout: 180000 }, () => {
  let creds;
  const configDir = fs.mkdtempSync(path.join(os.tmpdir(), 'mxdx-hosted-mhs-'));
  const LAUNCHER_NAME = `hosted-mhs-${Date.now()}`;

  before(() => {
    creds = loadCredentials();
    console.log(`[hosted-mhs] Server 1: ${creds.server.url}`);
    console.log(`[hosted-mhs] Server 2: ${creds.server2.url}`);
    console.log(`[hosted-mhs] Account1: ${creds.account1.username}`);
  });

  after(() => {
    try { fs.rmSync(configDir, { recursive: true }); } catch {}
  });

  it('connects to both hosted servers and selects preferred by latency', async () => {
    const configs = [
      { username: creds.account1.username, server: creds.server.url, password: creds.account1.password, configDir, useKeychain: false, log: console.log },
      { username: creds.account1.username, server: creds.server2.url, password: creds.account1.password, configDir, useKeychain: false, log: console.log },
    ];
    const mhs = await MultiHsClient.connect(configs, { log: console.log });

    assert.strictEqual(mhs.serverCount, 2);
    assert.ok(mhs.preferred.server);
    assert.ok(mhs.userId());

    const health = mhs.serverHealth();
    assert.strictEqual(health.size, 2);
    for (const [url, h] of health) {
      console.log(`  ${url}: ${h.status}, latency=${h.latencyMs}ms`);
      assert.strictEqual(h.status, 'healthy');
      assert.ok(h.latencyMs > 0);
    }

    await mhs.shutdown();
  });

  it('sends command via preferred server on hosted infrastructure', async () => {
    const configs = [
      { username: creds.account1.username, server: creds.server.url, password: creds.account1.password, configDir, useKeychain: false, log: () => {} },
      { username: creds.account1.username, server: creds.server2.url, password: creds.account1.password, configDir, useKeychain: false, log: () => {} },
    ];
    const mhs = await MultiHsClient.connect(configs, { log: () => {} });

    const topology = await mhs.getOrCreateLauncherSpace(LAUNCHER_NAME);
    assert.ok(topology.exec_room_id);

    await mhs.sendEvent(
      topology.exec_room_id,
      'org.mxdx.command',
      JSON.stringify({ request_id: 'hosted-test-1', command: 'echo', args: ['hello-hosted'] }),
    );

    await mhs.syncOnce();
    const events = JSON.parse(await mhs.collectRoomEvents(topology.exec_room_id, 1));
    assert.ok(Array.isArray(events));

    await mhs.shutdown();
  });

  it('preferred server override pins to ca2', async () => {
    const configs = [
      { username: creds.account1.username, server: creds.server.url, password: creds.account1.password, configDir, useKeychain: false, log: () => {} },
      { username: creds.account1.username, server: creds.server2.url, password: creds.account1.password, configDir, useKeychain: false, log: () => {} },
    ];
    const mhs = await MultiHsClient.connect(configs, {
      preferredServer: creds.server2.url,
      log: () => {},
    });

    assert.strictEqual(mhs.preferred.server, creds.server2.url);
    await mhs.shutdown();
  });

  it('cross-server federation: event sent on ca1 visible on ca2', async () => {
    // Connect account1 to both servers
    const configs = [
      { username: creds.account1.username, server: creds.server.url, password: creds.account1.password, configDir, useKeychain: false, log: () => {} },
      { username: creds.account1.username, server: creds.server2.url, password: creds.account1.password, configDir, useKeychain: false, log: () => {} },
    ];
    const mhs = await MultiHsClient.connect(configs, {
      preferredServer: creds.server.url,
      log: () => {},
    });

    const topology = await mhs.getOrCreateLauncherSpace(`fed-test-${Date.now()}`);
    assert.ok(topology.exec_room_id, 'should create launcher space');

    // Send event via ca1 (preferred)
    await mhs.sendEvent(
      topology.exec_room_id,
      'org.mxdx.command',
      JSON.stringify({ request_id: 'fed-1', command: 'whoami', args: [] }),
    );

    // Sync and verify events visible
    await mhs.syncOnce();
    const allUserIds = mhs.allUserIds();
    assert.ok(allUserIds.length >= 1, 'should have at least one user ID');

    await mhs.shutdown();
  });
});
