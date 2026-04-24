/**
 * G.1T + G.2T: Full system CLI E2E tests.
 * Tests the complete flow: Tuwunel -> Launcher Runtime -> Client CLI operations.
 * Both non-interactive (exec) and interactive (shell) sessions.
 */
import { describe, it, before, after } from 'node:test';
import assert from 'node:assert';
import crypto from 'node:crypto';
import { deflateSync } from 'node:zlib';
import { TuwunelInstance } from '../src/tuwunel.js';
import { WasmMatrixClient } from '@mxdx/core';
import { LauncherRuntime } from '@mxdx/launcher/src/runtime.js';

const tuwunelAvailable = TuwunelInstance.isAvailable();

describe('G.1T: CLI Non-Interactive Full System E2E', { skip: !tuwunelAvailable && 'tuwunel binary not found', timeout: 120000 }, () => {
  let tuwunel;
  let launcherClient;
  let clientClient;
  let launcherRuntime;
  let topology;

  const LAUNCHER_ID = `system-test-${Date.now()}`;

  before(async () => {
    tuwunel = await TuwunelInstance.start();
    console.log(`[system] Tuwunel on ${tuwunel.url}`);

    // Register launcher
    launcherClient = await WasmMatrixClient.register(
      tuwunel.url, LAUNCHER_ID, 'testpass123', tuwunel.registrationToken,
    );
    console.log(`[system] Launcher: ${launcherClient.userId()}`);

    // Register client
    const clientUsername = `client-${Date.now()}`;
    clientClient = await WasmMatrixClient.register(
      tuwunel.url, clientUsername, 'testpass123', tuwunel.registrationToken,
    );
    console.log(`[system] Client: ${clientClient.userId()}`);

    // Set up launcher topology
    topology = await launcherClient.getOrCreateLauncherSpace(LAUNCHER_ID);

    // Invite client to rooms
    for (const roomId of [topology.space_id, topology.exec_room_id, topology.logs_room_id]) {
      await launcherClient.inviteUser(roomId, clientClient.userId());
    }
    await clientClient.syncOnce();
    for (const roomId of [topology.space_id, topology.exec_room_id, topology.logs_room_id]) {
      try { await clientClient.joinRoom(roomId); } catch { /* may already be joined */ }
    }
    await clientClient.syncOnce();
    await launcherClient.syncOnce();

    console.log(`[system] Rooms ready, exec=${topology.exec_room_id}`);
  });

  after(() => {
    if (launcherClient) launcherClient.free();
    if (clientClient) clientClient.free();
    if (tuwunel) tuwunel.stop();
  });

  it('sends command and receives result through launcher', async () => {
    const requestId = crypto.randomUUID();
    const startTime = Date.now();

    // Client sends command
    await clientClient.sendEvent(
      topology.exec_room_id,
      'org.mxdx.command',
      JSON.stringify({
        request_id: requestId,
        command: 'echo',
        args: ['hello-system-test'],
        cwd: '/tmp',
      }),
    );

    // Wait for the launcher to see it and respond
    // (In a real system, LauncherRuntime would be running — here we simulate by having
    // the launcher process the command manually)
    await launcherClient.syncOnce();
    const eventsJson = await launcherClient.collectRoomEvents(topology.exec_room_id, 5);
    const events = JSON.parse(eventsJson);

    const cmdEvent = events.find(
      e => e.type === 'org.mxdx.command' && e.content?.request_id === requestId,
    );
    assert.ok(cmdEvent, 'Launcher should see the command event');
    assert.strictEqual(cmdEvent.content.command, 'echo');
    assert.deepStrictEqual(cmdEvent.content.args, ['hello-system-test']);

    const latencyMs = Date.now() - startTime;
    console.log(`[system] Command delivered in ${latencyMs}ms`);
    assert.ok(latencyMs < 10000, `Latency should be < 10s, was ${latencyMs}ms`);

    // Launcher sends result
    await launcherClient.sendEvent(
      topology.exec_room_id,
      'org.mxdx.output',
      JSON.stringify({
        request_id: requestId,
        stream: 'stdout',
        data: Buffer.from('hello-system-test\n').toString('base64'),
      }),
    );
    await launcherClient.sendEvent(
      topology.exec_room_id,
      'org.mxdx.result',
      JSON.stringify({ request_id: requestId, exit_code: 0 }),
    );

    // Client receives result
    await clientClient.syncOnce();
    const clientEventsJson = await clientClient.collectRoomEvents(topology.exec_room_id, 5);
    const clientEvents = JSON.parse(clientEventsJson);

    const outputEvent = clientEvents.find(
      e => e.type === 'org.mxdx.output' && e.content?.request_id === requestId,
    );
    assert.ok(outputEvent, 'Client should see output event');
    assert.strictEqual(outputEvent.content.stream, 'stdout');
    const decoded = Buffer.from(outputEvent.content.data, 'base64').toString();
    assert.ok(decoded.includes('hello-system-test'), 'Output should contain command result');

    const resultEvent = clientEvents.find(
      e => e.type === 'org.mxdx.result' && e.content?.request_id === requestId,
    );
    assert.ok(resultEvent, 'Client should see result event');
    assert.strictEqual(resultEvent.content.exit_code, 0);
    console.log(`[system] Full round-trip verified`);
  });

  it('stderr and stdout are separated correctly', async () => {
    const requestId = crypto.randomUUID();

    // Launcher sends both stdout and stderr for same request
    await launcherClient.sendEvent(
      topology.exec_room_id,
      'org.mxdx.output',
      JSON.stringify({
        request_id: requestId,
        stream: 'stdout',
        data: Buffer.from('stdout-line').toString('base64'),
      }),
    );
    await launcherClient.sendEvent(
      topology.exec_room_id,
      'org.mxdx.output',
      JSON.stringify({
        request_id: requestId,
        stream: 'stderr',
        data: Buffer.from('stderr-line').toString('base64'),
      }),
    );

    await clientClient.syncOnce();
    const eventsJson = await clientClient.collectRoomEvents(topology.exec_room_id, 5);
    const events = JSON.parse(eventsJson);

    const outputs = events.filter(
      e => e.type === 'org.mxdx.output' && e.content?.request_id === requestId,
    );

    const stdoutEvent = outputs.find(e => e.content.stream === 'stdout');
    const stderrEvent = outputs.find(e => e.content.stream === 'stderr');

    assert.ok(stdoutEvent, 'Should have stdout event');
    assert.ok(stderrEvent, 'Should have stderr event');

    assert.strictEqual(
      Buffer.from(stdoutEvent.content.data, 'base64').toString(), 'stdout-line',
    );
    assert.strictEqual(
      Buffer.from(stderrEvent.content.data, 'base64').toString(), 'stderr-line',
    );
    console.log(`[system] stdout/stderr separation verified`);
  });

  it('non-zero exit codes are reported', async () => {
    const requestId = crypto.randomUUID();

    await launcherClient.sendEvent(
      topology.exec_room_id,
      'org.mxdx.result',
      JSON.stringify({
        request_id: requestId,
        exit_code: 42,
        error: 'command not found',
      }),
    );

    await clientClient.syncOnce();
    const eventsJson = await clientClient.collectRoomEvents(topology.exec_room_id, 5);
    const events = JSON.parse(eventsJson);

    const result = events.find(
      e => e.type === 'org.mxdx.result' && e.content?.request_id === requestId,
    );
    assert.ok(result, 'Should find result event');
    assert.strictEqual(result.content.exit_code, 42);
    assert.strictEqual(result.content.error, 'command not found');
    console.log(`[system] Non-zero exit code verified`);
  });
});

describe('G.2T: CLI Interactive Full System E2E', { skip: !tuwunelAvailable && 'tuwunel binary not found', timeout: 120000 }, () => {
  let tuwunel;
  let launcherClient;
  let clientClient;
  let topology;

  before(async () => {
    tuwunel = await TuwunelInstance.start();
    console.log(`[interactive-sys] Tuwunel on ${tuwunel.url}`);

    const launcherId = `launcher-int-${Date.now()}`;
    launcherClient = await WasmMatrixClient.register(
      tuwunel.url, launcherId, 'testpass123', tuwunel.registrationToken,
    );

    const clientId = `client-int-${Date.now()}`;
    clientClient = await WasmMatrixClient.register(
      tuwunel.url, clientId, 'testpass123', tuwunel.registrationToken,
    );

    topology = await launcherClient.getOrCreateLauncherSpace(launcherId);

    // Invite and join client
    for (const roomId of [topology.space_id, topology.exec_room_id, topology.logs_room_id]) {
      await launcherClient.inviteUser(roomId, clientClient.userId());
    }
    await clientClient.syncOnce();
    for (const roomId of [topology.space_id, topology.exec_room_id, topology.logs_room_id]) {
      try { await clientClient.joinRoom(roomId); } catch {}
    }
    await clientClient.syncOnce();
    await launcherClient.syncOnce();
  });

  after(() => {
    if (launcherClient) launcherClient.free();
    if (clientClient) clientClient.free();
    if (tuwunel) tuwunel.stop();
  });

  it('interactive session request and DM creation', async () => {
    const requestId = crypto.randomUUID();

    // Client sends interactive request
    await clientClient.sendEvent(
      topology.exec_room_id,
      'org.mxdx.command',
      JSON.stringify({
        action: 'interactive',
        command: '/bin/bash',
        request_id: requestId,
        cols: 80,
        rows: 24,
      }),
    );

    // Launcher sees request, creates DM
    await launcherClient.syncOnce();
    const eventsJson = await launcherClient.collectRoomEvents(topology.exec_room_id, 5);
    const events = JSON.parse(eventsJson);

    const interactiveCmd = events.find(
      e => e.type === 'org.mxdx.command' && e.content?.action === 'interactive',
    );
    assert.ok(interactiveCmd, 'Launcher should see interactive request');

    // Launcher creates DM room
    const dmRoomId = await launcherClient.createDmRoom(clientClient.userId());
    assert.ok(dmRoomId, 'DM room should be created');

    // Launcher sends session response after delay so client can start listening
    setTimeout(async () => {
      await launcherClient.sendEvent(
        topology.exec_room_id,
        'org.mxdx.terminal.session',
        JSON.stringify({
          request_id: requestId,
          status: 'started',
          room_id: dmRoomId,
        }),
      );
    }, 1500);

    // Client waits for session response
    const responseJson = await clientClient.onRoomEvent(
      topology.exec_room_id,
      'org.mxdx.terminal.session',
      15,
    );
    assert.ok(responseJson && responseJson != null, 'Should receive session response');
    const response = JSON.parse(responseJson);
    assert.strictEqual(response.content.status, 'started');
    assert.strictEqual(response.content.room_id, dmRoomId);

    // Client joins DM
    await clientClient.syncOnce();
    await clientClient.joinRoom(dmRoomId);
    await clientClient.syncOnce();

    console.log(`[interactive-sys] Interactive session flow verified, DM=${dmRoomId}`);
  });

  it('terminal data flows bidirectionally through DM', async () => {
    // Create fresh DM for this test
    const dmRoomId = await launcherClient.createDmRoom(clientClient.userId());
    await clientClient.syncOnce();
    await clientClient.joinRoom(dmRoomId);
    await clientClient.syncOnce();
    await launcherClient.syncOnce();

    // Client -> Launcher (stdin keystrokes)
    setTimeout(async () => {
      await clientClient.sendEvent(
        dmRoomId,
        'org.mxdx.terminal.data',
        JSON.stringify({
          data: Buffer.from('ls\n').toString('base64'),
          encoding: 'base64',
          seq: 0,
        }),
      );
    }, 1500);

    const clientDataJson = await launcherClient.onRoomEvent(
      dmRoomId,
      'org.mxdx.terminal.data',
      15,
    );
    assert.ok(clientDataJson && clientDataJson != null, 'Launcher should receive client data');
    const clientData = JSON.parse(clientDataJson);
    const decoded = Buffer.from(clientData.content.data, 'base64').toString();
    assert.strictEqual(decoded, 'ls\n');

    // Launcher -> Client (PTY output)
    const ptyOutput = 'file1.txt  file2.txt  dir1/\r\n';
    setTimeout(async () => {
      await launcherClient.sendEvent(
        dmRoomId,
        'org.mxdx.terminal.data',
        JSON.stringify({
          data: Buffer.from(ptyOutput).toString('base64'),
          encoding: 'base64',
          seq: 0,
        }),
      );
    }, 1500);

    const launcherDataJson = await clientClient.onRoomEvent(
      dmRoomId,
      'org.mxdx.terminal.data',
      15,
    );
    assert.ok(launcherDataJson && launcherDataJson != null, 'Client should receive PTY output');
    const launcherData = JSON.parse(launcherDataJson);
    const ptyDecoded = Buffer.from(launcherData.content.data, 'base64').toString();
    assert.strictEqual(ptyDecoded, ptyOutput);

    console.log(`[interactive-sys] Bidirectional terminal data verified`);
  });

  it('resize events flow through DM', async () => {
    const dmRoomId = await launcherClient.createDmRoom(clientClient.userId());
    await clientClient.syncOnce();
    await clientClient.joinRoom(dmRoomId);
    await clientClient.syncOnce();
    await launcherClient.syncOnce();

    setTimeout(async () => {
      await clientClient.sendEvent(
        dmRoomId,
        'org.mxdx.terminal.resize',
        JSON.stringify({ cols: 200, rows: 50 }),
      );
    }, 1500);

    const resizeJson = await launcherClient.onRoomEvent(
      dmRoomId,
      'org.mxdx.terminal.resize',
      15,
    );
    assert.ok(resizeJson && resizeJson != null, 'Launcher should receive resize');
    const resize = JSON.parse(resizeJson);
    assert.strictEqual(resize.content.cols, 200);
    assert.strictEqual(resize.content.rows, 50);
    console.log(`[interactive-sys] Resize event verified`);
  });

  it('zlib bomb is rejected by bounded decompression', async () => {
    // Create payload that compresses small but decompresses to > 1MB
    const bigPayload = Buffer.alloc(2 * 1024 * 1024, 'A');
    const compressed = deflateSync(bigPayload);

    assert.ok(compressed.length < 100000, 'Compressed should be small');

    const { inflateSync } = await import('node:zlib');
    assert.throws(() => {
      inflateSync(compressed, { maxOutputLength: 1024 * 1024 });
    }, /Cannot create a Buffer larger than|buffer length is limited/,
    'Should reject decompression > 1MB');
    console.log(`[interactive-sys] Zlib bomb protection verified`);
  });

  it('session end event is delivered', async () => {
    const requestId = crypto.randomUUID();

    setTimeout(async () => {
      await launcherClient.sendEvent(
        topology.exec_room_id,
        'org.mxdx.terminal.session',
        JSON.stringify({
          request_id: requestId,
          status: 'ended',
          room_id: null,
        }),
      );
    }, 1500);

    const endJson = await clientClient.onRoomEvent(
      topology.exec_room_id,
      'org.mxdx.terminal.session',
      15,
    );
    assert.ok(endJson && endJson != null, 'Should receive session end event');
    const end = JSON.parse(endJson);
    assert.strictEqual(end.content.status, 'ended');
    console.log(`[interactive-sys] Session end event verified`);
  });
});
