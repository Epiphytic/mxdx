/**
 * Integration tests: WASM Room Topology against local Tuwunel.
 *
 * Calls WasmMatrixClient directly without spawning binary subprocesses.
 * These are integration tests per CLAUDE.md policy, not E2E tests.
 *
 * Extracted from packages/e2e-tests/tests/launcher-onboarding.test.js.
 * The subprocess-spawning 'E2E: Launcher Onboarding' block remains in e2e-tests.
 */

import { describe, it, before, after } from 'node:test';
import assert from 'node:assert';
import crypto from 'node:crypto';
import { TuwunelInstance } from '../src/tuwunel.js';
import { WasmMatrixClient } from '@mxdx/core';

const tuwunelAvailable = TuwunelInstance.isAvailable();

describe('WASM: Room Topology', { skip: !tuwunelAvailable && 'tuwunel binary not found', timeout: 120000 }, () => {
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

    await client.syncOnce();
    const eventsJson = await client.collectRoomEvents(topology.exec_room_id, 3);
    const events = JSON.parse(eventsJson);
    const found = events?.some(e => e.type === 'org.mxdx.host_telemetry');
    assert.ok(found, 'Should find telemetry state event in exec room');

    console.log('[topology] Telemetry state event verified in exec room');
  });
});
