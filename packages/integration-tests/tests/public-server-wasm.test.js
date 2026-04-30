/**
 * Integration tests: WASM Client against a public homeserver.
 *
 * These tests call WasmMatrixClient directly without spawning binary subprocesses.
 * They are integration tests per CLAUDE.md policy, not E2E tests.
 *
 * Extracted from packages/e2e-tests/tests/public-server.test.js.
 * The subprocess-spawning "Launcher + Client Round-Trip" tests remain in e2e-tests.
 *
 * ## Setup
 * Create `test-credentials.toml` in the repo root. See public-server.test.js header.
 */

import { describe, it, before } from 'node:test';
import assert from 'node:assert';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { WasmMatrixClient } from '@mxdx/core';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(__dirname, '../../..');

const FIXED_LAUNCHER_ID = 'pub-e2e-stable';

function loadCredentials() {
  const tomlPath = path.join(REPO_ROOT, 'test-credentials.toml');
  if (!fs.existsSync(tomlPath)) {
    throw new Error(
      'test-credentials.toml not found in repo root. See packages/e2e-tests/tests/public-server.test.js for setup.'
    );
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
  if (!result.server?.url) throw new Error('server.url missing in test-credentials.toml');
  if (!result.account1?.username) throw new Error('account1.username missing');
  if (!result.account1?.password) throw new Error('account1.password missing');
  if (!result.account2?.username) throw new Error('account2.username missing');
  if (!result.account2?.password) throw new Error('account2.password missing');
  return {
    url: result.server.url,
    account1: { username: result.account1.username, password: result.account1.password },
    account2: { username: result.account2.username, password: result.account2.password },
  };
}

function sleep(ms) {
  return new Promise((r) => setTimeout(r, ms));
}

describe('Public Server: WASM Client', { timeout: 120000 }, () => {
  let creds;

  before(() => {
    creds = loadCredentials();
  });

  it('login account1 via WasmMatrixClient', async () => {
    const client = await WasmMatrixClient.login(
      creds.url, creds.account1.username, creds.account1.password,
    );
    assert.ok(client.isLoggedIn(), 'Should be logged in');
    assert.ok(client.userId(), 'Should have a user ID');
    console.log(`[pub] Account1 logged in as ${client.userId()}`);
    client.free();
  });

  it('login account2 via WasmMatrixClient', async () => {
    const client = await WasmMatrixClient.login(
      creds.url, creds.account2.username, creds.account2.password,
    );
    assert.ok(client.isLoggedIn(), 'Should be logged in');
    assert.ok(client.userId(), 'Should have a user ID');
    console.log(`[pub] Account2 logged in as ${client.userId()}`);
    client.free();
  });

  it('verify cross-signing state between accounts', async () => {
    const client1 = await WasmMatrixClient.login(
      creds.url, creds.account1.username, creds.account1.password,
    );
    const client2 = await WasmMatrixClient.login(
      creds.url, creds.account2.username, creds.account2.password,
    );

    const userId1 = client1.userId();
    const userId2 = client2.userId();
    console.log(`[pub] Checking cross-signing ${userId1} <-> ${userId2}`);

    await client1.bootstrapCrossSigningIfNeeded(creds.account1.password);
    await client2.bootstrapCrossSigningIfNeeded(creds.account2.password);
    await client1.verifyOwnIdentity();
    await client2.verifyOwnIdentity();

    await client1.syncOnce();
    await client2.syncOnce();

    const verified1 = await client1.isUserVerified(userId2);
    const verified2 = await client2.isUserVerified(userId1);
    console.log(`[pub] Account1 sees Account2 verified: ${verified1}`);
    console.log(`[pub] Account2 sees Account1 verified: ${verified2}`);
    assert.ok(verified1, 'Account1 should see Account2 as verified (cross-sign via Element first)');
    assert.ok(verified2, 'Account2 should see Account1 as verified (cross-sign via Element first)');

    client1.free();
    client2.free();
  });

  it('find-or-create launcher space (room reuse)', async () => {
    const client = await WasmMatrixClient.login(
      creds.url, creds.account1.username, creds.account1.password,
    );

    const topology = await client.getOrCreateLauncherSpace(FIXED_LAUNCHER_ID);
    assert.ok(topology, 'Should create/find topology');
    assert.ok(topology.space_id, 'Should have space_id');
    assert.ok(topology.exec_room_id, 'Should have exec_room_id');
    assert.ok(topology.logs_room_id, 'Should have logs_room_id');
    assert.strictEqual(topology.status_room_id, undefined, 'Should NOT have status_room_id');
    console.log(`[pub] Launcher space: ${topology.space_id}`);

    client.free();
  });

  it('getOrCreateLauncherSpace is idempotent', async () => {
    const client = await WasmMatrixClient.login(
      creds.url, creds.account1.username, creds.account1.password,
    );

    const first = await client.getOrCreateLauncherSpace(FIXED_LAUNCHER_ID);
    await sleep(2000);
    const second = await client.getOrCreateLauncherSpace(FIXED_LAUNCHER_ID);

    assert.strictEqual(second.space_id, first.space_id, 'space_id should be identical');
    assert.strictEqual(second.exec_room_id, first.exec_room_id, 'exec_room_id should be identical');
    assert.strictEqual(second.logs_room_id, first.logs_room_id, 'logs_room_id should be identical');
    console.log('[pub] Idempotency verified');

    client.free();
  });

  it('send and receive encrypted custom events', async () => {
    const client = await WasmMatrixClient.login(
      creds.url, creds.account1.username, creds.account1.password,
    );

    const topology = await client.getOrCreateLauncherSpace(FIXED_LAUNCHER_ID);

    const crypto = await import('node:crypto');
    const requestId = crypto.randomUUID();
    await client.sendEvent(
      topology.exec_room_id,
      'org.mxdx.command',
      JSON.stringify({
        request_id: requestId,
        command: 'echo',
        args: ['public-server-test'],
        cwd: '/tmp',
      }),
    );
    console.log(`[pub] Sent command event: ${requestId}`);

    let found = false;
    const deadline = Date.now() + 15000;
    while (Date.now() < deadline && !found) {
      await client.syncOnce();
      const eventsJson = await client.collectRoomEvents(topology.exec_room_id, 1);
      const events = JSON.parse(eventsJson);
      if (events && Array.isArray(events)) {
        for (const event of events) {
          if (event.content?.request_id === requestId) {
            found = true;
            console.log(`[pub] Found event: ${JSON.stringify(event.content)}`);
          }
        }
      }
    }

    assert.ok(found, 'Should find the custom event we sent');
    client.free();
  });

  it('send telemetry state event to exec room', async () => {
    const client = await WasmMatrixClient.login(
      creds.url, creds.account1.username, creds.account1.password,
    );

    const topology = await client.getOrCreateLauncherSpace(FIXED_LAUNCHER_ID);

    await client.sendStateEvent(
      topology.exec_room_id,
      'org.mxdx.host_telemetry',
      '',
      JSON.stringify({
        hostname: 'test-host',
        platform: 'linux',
        arch: 'x64',
      }),
    );
    console.log('[pub] Sent state event to exec room');

    await client.syncOnce();
    const eventsJson = await client.collectRoomEvents(topology.exec_room_id, 3);
    const events = JSON.parse(eventsJson);
    const found = events?.some(e => e.type === 'org.mxdx.host_telemetry');
    assert.ok(found, 'Should find telemetry state event');

    client.free();
  });

  it('listLauncherSpaces discovers existing spaces', async () => {
    const client = await WasmMatrixClient.login(
      creds.url, creds.account1.username, creds.account1.password,
    );

    const launchersJson = await client.listLauncherSpaces();
    const launchers = JSON.parse(launchersJson);
    assert.ok(Array.isArray(launchers), 'Should return array');

    const found = launchers.find(l => l.launcher_id === FIXED_LAUNCHER_ID);
    assert.ok(found, `Should find ${FIXED_LAUNCHER_ID} in launcher list`);
    assert.ok(found.space_id, 'Should have space_id');
    assert.ok(found.exec_room_id, 'Should have exec_room_id');
    assert.ok(found.logs_room_id, 'Should have logs_room_id');
    console.log(`[pub] Found ${launchers.length} launcher(s)`);

    client.free();
  });
});
