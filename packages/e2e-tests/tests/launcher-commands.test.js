/**
 * E2E tests for launcher command execution.
 * Tests multiple commands, stderr, deduplication, and listLauncherSpaces.
 * Runs against a local Tuwunel instance.
 */
import { describe, it, before, after } from 'node:test';
import assert from 'node:assert';
import crypto from 'node:crypto';
import { TuwunelInstance } from '../src/tuwunel.js';
import { WasmMatrixClient } from '@mxdx/core';

describe('WASM: Extended Command Tests', { timeout: 120000 }, () => {
  let tuwunel;
  let client;

  before(async () => {
    tuwunel = await TuwunelInstance.start();
    console.log(`[cmd] Tuwunel started on ${tuwunel.url}`);

    const username = `cmd-test-${Date.now()}`;
    client = await WasmMatrixClient.register(
      tuwunel.url, username, 'testpass123', tuwunel.registrationToken,
    );
    console.log(`[cmd] Registered as ${client.userId()}`);
  });

  after(() => {
    if (client) client.free();
    if (tuwunel) tuwunel.stop();
  });

  it('sends multiple command events to exec room', async () => {
    const launcherId = `cmd-multi-${Date.now()}`;
    const topology = await client.getOrCreateLauncherSpace(launcherId);

    const ids = [];
    for (let i = 0; i < 3; i++) {
      const requestId = crypto.randomUUID();
      ids.push(requestId);
      await client.sendEvent(
        topology.exec_room_id,
        'org.mxdx.command',
        JSON.stringify({
          request_id: requestId,
          command: i === 0 ? 'echo' : i === 1 ? 'date' : 'uname',
          args: i === 0 ? ['hello'] : [],
          cwd: '/tmp',
        }),
      );
    }

    // Verify all 3 events are readable
    let found = 0;
    const deadline = Date.now() + 15000;
    while (Date.now() < deadline && found < 3) {
      await client.syncOnce();
      const eventsJson = await client.collectRoomEvents(topology.exec_room_id, 1);
      const events = JSON.parse(eventsJson);
      if (events && Array.isArray(events)) {
        for (const event of events) {
          if (ids.includes(event.content?.request_id)) {
            found++;
          }
        }
        if (found >= 3) break;
      }
    }

    assert.ok(found >= 3, `Should find all 3 command events, found ${found}`);
    console.log(`[cmd] Multiple commands verified: ${found}/3`);
  });

  it('stderr events have correct stream field', async () => {
    const launcherId = `cmd-stderr-${Date.now()}`;
    const topology = await client.getOrCreateLauncherSpace(launcherId);

    const requestId = crypto.randomUUID();
    // Send a stderr output event (simulating what the launcher would send)
    await client.sendEvent(
      topology.exec_room_id,
      'org.mxdx.output',
      JSON.stringify({
        request_id: requestId,
        stream: 'stderr',
        data: Buffer.from('error output').toString('base64'),
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
          if (event.content?.request_id === requestId && event.content?.stream === 'stderr') {
            found = true;
            const decoded = Buffer.from(event.content.data, 'base64').toString();
            assert.strictEqual(decoded, 'error output', 'Decoded stderr should match');
            console.log(`[cmd] Stderr event verified: ${decoded}`);
          }
        }
      }
    }

    assert.ok(found, 'Should find stderr output event');
  });

  it('event deduplication: same event not processed twice', async () => {
    const launcherId = `cmd-dedup-${Date.now()}`;
    const topology = await client.getOrCreateLauncherSpace(launcherId);

    const requestId = crypto.randomUUID();
    await client.sendEvent(
      topology.exec_room_id,
      'org.mxdx.command',
      JSON.stringify({
        request_id: requestId,
        command: 'echo',
        args: ['dedup-test'],
        cwd: '/tmp',
      }),
    );

    // Collect events twice — same event should appear with same event_id
    await client.syncOnce();
    const firstJson = await client.collectRoomEvents(topology.exec_room_id, 1);
    const firstEvents = JSON.parse(firstJson);

    await client.syncOnce();
    const secondJson = await client.collectRoomEvents(topology.exec_room_id, 1);
    const secondEvents = JSON.parse(secondJson);

    // Both collections should find the same event with same event_id
    const firstMatch = firstEvents.find(e => e.content?.request_id === requestId);
    const secondMatch = secondEvents.find(e => e.content?.request_id === requestId);

    assert.ok(firstMatch, 'Should find event on first collection');
    assert.ok(secondMatch, 'Should find event on second collection');
    assert.strictEqual(firstMatch.event_id, secondMatch.event_id,
      'Same event should have same event_id (dedup key)');
    console.log(`[cmd] Dedup verified: event_id=${firstMatch.event_id}`);
  });

  it('listLauncherSpaces discovers created spaces', async () => {
    // Create a known space first
    const launcherId = `cmd-list-${Date.now()}`;
    await client.getOrCreateLauncherSpace(launcherId);

    // Sync to pick up newly created space
    await client.syncOnce();

    // List all spaces
    const launchersJson = await client.listLauncherSpaces();
    const launchers = JSON.parse(launchersJson);

    assert.ok(Array.isArray(launchers), 'Should return array');
    const found = launchers.find(l => l.launcher_id === launcherId);
    assert.ok(found, `Should find launcher ${launcherId} in list`);
    assert.ok(found.space_id, 'Should have space_id');
    assert.ok(found.exec_room_id, 'Should have exec_room_id');
    assert.ok(found.logs_room_id, 'Should have logs_room_id');
    console.log(`[cmd] listLauncherSpaces found ${launchers.length} launcher(s)`);
  });
});
