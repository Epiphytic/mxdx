/**
 * Public Matrix server E2E tests.
 *
 * Tests the real WASM-backed launcher + client against a public homeserver.
 * These exercise the full npm + WASM stack (not Rust-native).
 *
 * ## Setup
 *
 * Create `test-credentials.toml` in the repo root (gitignored):
 *
 * ```toml
 * [server]
 * url = "matrix.org"
 *
 * [account1]
 * username = "your-client-user"
 * password = "your-password"
 *
 * [account2]
 * username = "your-launcher-user"
 * password = "your-password"
 * ```
 *
 * Run with: node --test packages/e2e-tests/tests/public-server.test.js
 */

import { describe, it, before, after } from 'node:test';
import assert from 'node:assert';
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { WasmMatrixClient } from '@mxdx/core';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(__dirname, '../../..');
const LAUNCHER_BIN = path.resolve(__dirname, '../../launcher/bin/mxdx-launcher.js');
const CLIENT_BIN = path.resolve(__dirname, '../../client/bin/mxdx-client.js');

// Fixed launcher ID for room reuse across test runs
const FIXED_LAUNCHER_ID = 'pub-e2e-stable';

/**
 * Load credentials from test-credentials.toml.
 */
function loadCredentials() {
  const tomlPath = path.join(REPO_ROOT, 'test-credentials.toml');
  if (!fs.existsSync(tomlPath)) {
    throw new Error(
      'test-credentials.toml not found in repo root. See test file header for setup.'
    );
  }
  const content = fs.readFileSync(tomlPath, 'utf8');

  // Minimal TOML parser for our flat structure
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

function waitForOutput(proc, needle, timeoutMs = 30000) {
  return new Promise((resolve) => {
    let output = '';
    const timeout = setTimeout(() => resolve(false), timeoutMs);

    const handler = (chunk) => {
      output += chunk.toString();
      if (output.includes(needle)) {
        clearTimeout(timeout);
        resolve(true);
      }
    };

    proc.stdout?.on('data', handler);
    proc.stderr?.on('data', handler);
    proc.on('close', () => {
      clearTimeout(timeout);
      resolve(false);
    });
  });
}

function sleep(ms) {
  return new Promise((r) => setTimeout(r, ms));
}

// ─── WASM Client Tests ──────────────────────────────────────────────────────

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
    // Throttle between room operations
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

// ─── Full Round-Trip: Launcher + Client ─────────────────────────────────────

describe('Public Server: Launcher + Client Round-Trip', { timeout: 180000 }, () => {
  let creds;
  let launcherProc;
  let LAUNCHER_NAME;

  before(() => {
    creds = loadCredentials();
    LAUNCHER_NAME = creds.account2.username;
  });

  after(() => {
    if (launcherProc) launcherProc.kill();
  });

  it('launcher starts and client executes a command (latency < 10s)', async () => {
    console.log(`[pub] Starting launcher as ${creds.account2.username} on ${creds.url}...`);

    const adminClient = await WasmMatrixClient.login(
      creds.url, creds.account1.username, creds.account1.password,
    );
    const adminMxid = adminClient.userId();
    console.log(`[pub] Admin MXID: ${adminMxid}`);
    adminClient.free();

    const configPath = `/tmp/pub-launcher-${Date.now()}.toml`;

    launcherProc = spawn('node', [
      LAUNCHER_BIN,
      '--servers', creds.url,
      '--username', creds.account2.username,
      '--password', creds.account2.password,
      '--allowed-commands', 'echo,date,uname',
      '--allowed-cwd', '/tmp',
      '--admin-user', adminMxid,
      '--config', configPath,
      '--log-format', 'text',
    ], {
      stdio: ['ignore', 'pipe', 'pipe'],
    });

    let launcherOutput = '';
    launcherProc.stdout.on('data', (chunk) => {
      launcherOutput += chunk.toString();
      process.stderr.write(`[launcher stdout] ${chunk}`);
    });
    launcherProc.stderr.on('data', (chunk) => {
      launcherOutput += chunk.toString();
      process.stderr.write(`[launcher stderr] ${chunk}`);
    });

    const online = await waitForOutput(launcherProc, 'Listening for commands', 60000);
    assert.ok(online, 'Launcher should come online');
    console.log('[pub] Launcher is online');

    await sleep(3000);

    // Measure latency: time from client exec start to result
    const startTime = Date.now();
    console.log('[pub] Running client exec...');

    const clientResult = await new Promise((resolve, reject) => {
      const proc = spawn('node', [
        CLIENT_BIN,
        '--server', creds.url,
        '--username', creds.account1.username,
        '--password', creds.account1.password,
        '--format', 'json',
        'exec', LAUNCHER_NAME, 'echo', 'hello-from-public-server',
        '--cwd', '/tmp',
      ], {
        stdio: ['ignore', 'pipe', 'pipe'],
        timeout: 60000,
      });

      let stdout = '';
      let stderr = '';
      proc.stdout.on('data', (chunk) => { stdout += chunk.toString(); });
      proc.stderr.on('data', (chunk) => { stderr += chunk.toString(); });

      proc.on('close', (code) => {
        resolve({ code, stdout, stderr });
      });
      proc.on('error', reject);

      setTimeout(() => {
        proc.kill();
        resolve({ code: -1, stdout, stderr: stderr + '\n[timeout]' });
      }, 60000);
    });

    const latencyMs = Date.now() - startTime;
    console.log(`[pub] Client exit code: ${clientResult.code}, latency: ${latencyMs}ms`);
    console.log(`[pub] Client stdout: ${clientResult.stdout}`);
    if (clientResult.stderr) console.log(`[pub] Client stderr: ${clientResult.stderr}`);

    assert.strictEqual(clientResult.code, 0, `Client should exit 0, got ${clientResult.code}`);
    assert.ok(latencyMs < 10000, `Latency should be < 10s, was ${latencyMs}ms`);

    const output = JSON.parse(clientResult.stdout);
    assert.strictEqual(output.exitCode, 0, 'Remote command exit code should be 0');

    try { fs.unlinkSync(configPath); } catch { /* ignore */ }
  });
});
