/**
 * Multi-launcher per account tests.
 * Verifies that multiple launchers sharing the same Matrix account
 * create separate spaces and session rooms via hostname disambiguation.
 */
import { describe, it, before, after } from 'node:test';
import assert from 'node:assert';
import { TuwunelInstance } from '../src/tuwunel.js';
import { WasmMatrixClient } from '@mxdx/core';

const tuwunelAvailable = TuwunelInstance.isAvailable();

describe('Multi-launcher per account', { skip: !tuwunelAvailable && 'tuwunel binary not found', timeout: 120000 }, () => {
  let tuwunel;
  let sharedClient; // one Matrix account, used by both "launchers"
  let clientUser;   // the client connecting to launchers

  before(async () => {
    tuwunel = await TuwunelInstance.start();
    console.log(`[multi-launcher] Tuwunel on ${tuwunel.url}`);

    // Register shared launcher account
    const launcherUsername = `launcher-${Date.now()}`;
    sharedClient = await WasmMatrixClient.register(
      tuwunel.url, launcherUsername, 'testpass123', tuwunel.registrationToken,
    );
    console.log(`[multi-launcher] Shared launcher: ${sharedClient.userId()}`);

    // Register client
    const clientUsername = `client-${Date.now()}`;
    clientUser = await WasmMatrixClient.register(
      tuwunel.url, clientUsername, 'testpass123', tuwunel.registrationToken,
    );
    console.log(`[multi-launcher] Client: ${clientUser.userId()}`);
  });

  after(() => {
    if (sharedClient) sharedClient.free();
    if (clientUser) clientUser.free();
    if (tuwunel) tuwunel.stop();
  });

  it('two launchers with different hostnames create separate spaces', async () => {
    // Create spaces for two different "launchers" on the same account
    const topologyA = await sharedClient.getOrCreateLauncherSpace('host-alpha');
    const topologyB = await sharedClient.getOrCreateLauncherSpace('host-beta');

    // Verify they got different rooms
    assert.notStrictEqual(topologyA.space_id, topologyB.space_id, 'Spaces should differ');
    assert.notStrictEqual(topologyA.exec_room_id, topologyB.exec_room_id, 'Exec rooms should differ');
    assert.notStrictEqual(topologyA.logs_room_id, topologyB.logs_room_id, 'Logs rooms should differ');

    console.log(`[multi-launcher] host-alpha: space=${topologyA.space_id}, exec=${topologyA.exec_room_id}`);
    console.log(`[multi-launcher] host-beta: space=${topologyB.space_id}, exec=${topologyB.exec_room_id}`);

    // Sync to pick up both spaces before listing
    await sharedClient.syncOnce();

    // Verify listLauncherSpaces returns both
    const launchersJson = await sharedClient.listLauncherSpaces();
    const launchers = JSON.parse(launchersJson);
    const launcherIds = launchers.map(l => l.launcher_id);

    assert.ok(launcherIds.includes('host-alpha'), 'Should list host-alpha');
    assert.ok(launcherIds.includes('host-beta'), 'Should list host-beta');
    console.log(`[multi-launcher] Listed launchers: ${launcherIds.join(', ')}`);
  });

  it('session rooms are keyed by hostname, not matrix user ID', async () => {
    // Create session rooms as each "launcher" would
    const topicA = `org.mxdx.launcher.sessions:host-alpha:${clientUser.userId()}`;
    const topicB = `org.mxdx.launcher.sessions:host-beta:${clientUser.userId()}`;

    const roomA = await sharedClient.createRoom(JSON.stringify({
      invite: [clientUser.userId()],
      topic: topicA,
      preset: 'trusted_private_chat',
    }));

    const roomB = await sharedClient.createRoom(JSON.stringify({
      invite: [clientUser.userId()],
      topic: topicB,
      preset: 'trusted_private_chat',
    }));

    assert.ok(roomA, 'Room A should be created');
    assert.ok(roomB, 'Room B should be created');
    assert.notStrictEqual(roomA, roomB, 'Session rooms should be different');

    console.log(`[multi-launcher] host-alpha session room: ${roomA}`);
    console.log(`[multi-launcher] host-beta session room: ${roomB}`);

    // Client joins both rooms
    await clientUser.syncOnce();
    await clientUser.joinRoom(roomA);
    await clientUser.joinRoom(roomB);
    await clientUser.syncOnce();

    // Send a message in each room — verify they don't cross
    await sharedClient.sendEvent(roomA, 'org.mxdx.terminal.data', JSON.stringify({
      data: btoa('alpha-data'), encoding: 'base64', seq: 0, session_id: 'sess-a',
    }));

    await sharedClient.sendEvent(roomB, 'org.mxdx.terminal.data', JSON.stringify({
      data: btoa('beta-data'), encoding: 'base64', seq: 0, session_id: 'sess-b',
    }));

    // Client reads from room A
    await clientUser.syncOnce();
    const eventsA = JSON.parse(await clientUser.collectRoomEvents(roomA, 5));
    const termA = eventsA.filter(e => e.type === 'org.mxdx.terminal.data');
    assert.ok(termA.length >= 1, 'Room A should have terminal data');
    assert.strictEqual(termA[0].content.session_id, 'sess-a', 'Room A data should have session_id sess-a');

    // Client reads from room B
    const eventsB = JSON.parse(await clientUser.collectRoomEvents(roomB, 5));
    const termB = eventsB.filter(e => e.type === 'org.mxdx.terminal.data');
    assert.ok(termB.length >= 1, 'Room B should have terminal data');
    assert.strictEqual(termB[0].content.session_id, 'sess-b', 'Room B data should have session_id sess-b');

    console.log('[multi-launcher] Session rooms properly isolated by hostname');
  });

  it('telemetry includes timestamp and heartbeat_interval_ms', async () => {
    // Post telemetry to an exec room (simulating launcher)
    const topology = await sharedClient.getOrCreateLauncherSpace('host-telemetry-test');

    const telemetry = {
      timestamp: new Date().toISOString(),
      heartbeat_interval_ms: 60000,
      hostname: 'host-telemetry-test',
      platform: 'linux',
      arch: 'x64',
    };

    await sharedClient.sendStateEvent(
      topology.exec_room_id,
      'org.mxdx.host_telemetry',
      '',
      JSON.stringify(telemetry),
    );

    // Read it back
    await sharedClient.syncOnce();
    const eventsJson = await sharedClient.readRoomEvents(topology.exec_room_id);
    const events = JSON.parse(eventsJson);
    const telemetryEvent = events.find(e => e.type === 'org.mxdx.host_telemetry');

    assert.ok(telemetryEvent, 'Telemetry event should exist');
    assert.ok(telemetryEvent.content.timestamp, 'Should have timestamp');
    assert.strictEqual(telemetryEvent.content.heartbeat_interval_ms, 60000, 'Should have heartbeat_interval_ms');
    console.log('[multi-launcher] Telemetry fields verified');
  });
});
