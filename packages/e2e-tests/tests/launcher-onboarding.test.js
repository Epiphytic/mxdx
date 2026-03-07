import { describe, it, before, after } from 'node:test';
import assert from 'node:assert';
import crypto from 'node:crypto';
import { spawn } from 'node:child_process';
import path from 'node:path';
import fs from 'node:fs';
import { fileURLToPath } from 'node:url';
import { TuwunelInstance } from '../src/tuwunel.js';
import { WasmMatrixClient } from '@mxdx/core';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const LAUNCHER_BIN = path.resolve(__dirname, '../../launcher/bin/mxdx-launcher.js');

describe('E2E: Launcher Onboarding', () => {
  let tuwunel;

  before(async () => {
    tuwunel = await TuwunelInstance.start();
    console.log(`[e2e] Tuwunel started on ${tuwunel.url}`);
  });

  after(() => {
    if (tuwunel) tuwunel.stop();
  });

  it('registers, creates rooms, and goes online', async () => {
    const configPath = `/tmp/e2e-onboard-${Date.now()}.toml`;
    const launcherName = `onboard-test-${Date.now()}`;

    const proc = spawn('node', [
      LAUNCHER_BIN,
      '--servers', tuwunel.url,
      '--username', launcherName,
      '--password', 'testpass123',
      '--registration-token', tuwunel.registrationToken,
      '--allowed-commands', 'echo',
      '--config', configPath,
    ], {
      stdio: ['ignore', 'pipe', 'pipe'],
    });

    // Collect all output
    let output = '';
    proc.stdout.on('data', (chunk) => { output += chunk.toString(); });
    proc.stderr.on('data', (chunk) => { output += chunk.toString(); });

    // Wait for "Listening for commands" or timeout
    const online = await new Promise((resolve) => {
      const timeout = setTimeout(() => {
        proc.kill();
        resolve(false);
      }, 30000);

      const check = () => {
        if (output.includes('Listening for commands')) {
          clearTimeout(timeout);
          proc.kill();
          resolve(true);
        }
      };

      proc.stdout.on('data', check);
      proc.stderr.on('data', check);
      proc.on('close', () => {
        clearTimeout(timeout);
        resolve(false);
      });
    });

    console.log('[e2e] Launcher output:', output);

    // Verify
    assert.ok(online, 'Launcher should come online');
    assert.ok(output.includes('Logged in as'), 'Should log in');
    assert.ok(output.includes('Rooms ready'), 'Should create rooms');
    assert.ok(fs.existsSync(configPath), 'Config file should be created');

    // Cleanup
    fs.rmSync(configPath, { force: true });
  });
});

// ─── WASM: Room Topology ────────────────────────────────────────────────────

describe('WASM: Room Topology', { timeout: 120000 }, () => {
  let tuwunel;
  let client;

  before(async () => {
    tuwunel = await TuwunelInstance.start();
    console.log(`[topology] Tuwunel started on ${tuwunel.url}`);

    const username = `topology-test-${Date.now()}`;
    client = await WasmMatrixClient.register(
      tuwunel.url, username, 'testpass123', tuwunel.registrationToken,
    );
    console.log(`[topology] Registered as ${client.userId()}`);
  });

  after(() => {
    if (client) client.free();
    if (tuwunel) tuwunel.stop();
  });

  it('getOrCreateLauncherSpace() returns { space_id, exec_room_id, logs_room_id } with no status_room_id', async () => {
    const launcherId = `topo-shape-${Date.now()}`;
    const topology = await client.getOrCreateLauncherSpace(launcherId);

    assert.ok(topology, 'Topology object should be returned');
    assert.ok(topology.space_id, 'Should have space_id');
    assert.ok(topology.exec_room_id, 'Should have exec_room_id');
    assert.ok(topology.logs_room_id, 'Should have logs_room_id');
    assert.strictEqual(
      topology.status_room_id, undefined,
      'Should NOT have status_room_id — topology is 2-room (exec + logs)',
    );

    console.log(`[topology] Topology: space=${topology.space_id}, exec=${topology.exec_room_id}, logs=${topology.logs_room_id}`);
  });

  it('calling getOrCreateLauncherSpace() twice returns the same topology (idempotent)', async () => {
    const launcherId = `topo-idempotent-${Date.now()}`;

    const first = await client.getOrCreateLauncherSpace(launcherId);
    const second = await client.getOrCreateLauncherSpace(launcherId);

    assert.strictEqual(second.space_id, first.space_id, 'space_id should be identical');
    assert.strictEqual(second.exec_room_id, first.exec_room_id, 'exec_room_id should be identical');
    assert.strictEqual(second.logs_room_id, first.logs_room_id, 'logs_room_id should be identical');

    console.log('[topology] Idempotency verified');
  });

  it('exec room is E2EE (encrypted event round-trip)', async () => {
    const launcherId = `topo-exec-e2ee-${Date.now()}`;
    const topology = await client.getOrCreateLauncherSpace(launcherId);

    const requestId = crypto.randomUUID();
    await client.sendEvent(
      topology.exec_room_id,
      'org.mxdx.command',
      JSON.stringify({
        request_id: requestId,
        command: 'echo',
        args: ['e2ee-exec-test'],
        cwd: '/tmp',
      }),
    );

    // Sync and collect events to verify the encrypted round-trip
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
            console.log(`[topology] Exec room E2EE round-trip OK: ${JSON.stringify(event.content)}`);
          }
        }
      }
    }

    assert.ok(found, 'Should decrypt and read back the event sent to the exec room');
  });

  it('logs room is E2EE (encrypted event round-trip)', async () => {
    const launcherId = `topo-logs-e2ee-${Date.now()}`;
    const topology = await client.getOrCreateLauncherSpace(launcherId);

    const logId = crypto.randomUUID();
    await client.sendEvent(
      topology.logs_room_id,
      'org.mxdx.log_entry',
      JSON.stringify({
        log_id: logId,
        level: 'info',
        message: 'e2ee-logs-test',
      }),
    );

    // Sync and collect events to verify the encrypted round-trip
    let found = false;
    const deadline = Date.now() + 15000;
    while (Date.now() < deadline && !found) {
      await client.syncOnce();
      const eventsJson = await client.collectRoomEvents(topology.logs_room_id, 1);
      const events = JSON.parse(eventsJson);
      if (events && Array.isArray(events)) {
        for (const event of events) {
          if (event.content?.log_id === logId) {
            found = true;
            console.log(`[topology] Logs room E2EE round-trip OK: ${JSON.stringify(event.content)}`);
          }
        }
      }
    }

    assert.ok(found, 'Should decrypt and read back the event sent to the logs room');
  });

  it('telemetry state event is sendable to exec room', async () => {
    const launcherId = `topo-telemetry-${Date.now()}`;
    const topology = await client.getOrCreateLauncherSpace(launcherId);

    // Send telemetry state event to the exec room (not status room)
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
    console.log('[topology] Sent telemetry state event to exec room');

    // Verify by syncing and collecting events
    await client.syncOnce();
    const eventsJson = await client.collectRoomEvents(topology.exec_room_id, 3);
    const events = JSON.parse(eventsJson);
    const found = events?.some(e => e.type === 'org.mxdx.host_telemetry');
    assert.ok(found, 'Should find telemetry state event in exec room');

    console.log('[topology] Telemetry state event verified in exec room');
  });
});
