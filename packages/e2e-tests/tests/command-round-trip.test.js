import { describe, it, before, after } from 'node:test';
import assert from 'node:assert';
import { spawn } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { TuwunelInstance } from '../src/tuwunel.js';
import { WasmMatrixClient } from '@mxdx/core';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const LAUNCHER_BIN = path.resolve(__dirname, '../../launcher/bin/mxdx-launcher.js');

const tuwunelAvailable = TuwunelInstance.isAvailable();

describe('E2E: Command Round-Trip', { skip: !tuwunelAvailable && 'tuwunel binary not found', timeout: 60000 }, () => {
  let tuwunel;
  let launcherProc;
  const LAUNCHER_NAME = `e2e-launcher-${Date.now()}`;
  const CLIENT_NAME = `e2e-client-${Date.now()}`;
  const PASSWORD = 'testpass123';

  before(async () => {
    // Start Tuwunel
    tuwunel = await TuwunelInstance.start();
    console.log(`[e2e] Tuwunel started on ${tuwunel.url}`);

    // Pre-register the client user so we know its MXID for admin invite
    const clientUser = await WasmMatrixClient.register(
      tuwunel.url, CLIENT_NAME, PASSWORD, tuwunel.registrationToken
    );
    const clientMxid = clientUser.userId();
    console.log(`[e2e] Client pre-registered as ${clientMxid}`);
    // Free the client — we'll reconnect via the CLI
    clientUser.free();

    // Start launcher with client as admin user
    launcherProc = spawn('node', [
      LAUNCHER_BIN,
      '--servers', tuwunel.url,
      '--username', LAUNCHER_NAME,
      '--password', PASSWORD,
      '--registration-token', tuwunel.registrationToken,
      '--allowed-commands', 'echo,seq,cat',
      '--admin-user', clientMxid,
      '--config', `/tmp/e2e-launcher-${Date.now()}.toml`,
    ], {
      stdio: ['ignore', 'pipe', 'pipe'],
    });

    // Wait for launcher to come online
    const online = await waitForOutput(launcherProc, 'Listening for commands', 30000);
    assert.ok(online, 'Launcher should come online');
    console.log('[e2e] Launcher is online');

    // Give it a moment for invites to be sent
    await new Promise(r => setTimeout(r, 1000));
  });

  after(async () => {
    if (launcherProc) launcherProc.kill();
    if (tuwunel) tuwunel.stop();
  });

  it('client sends echo command and receives output via Matrix', async () => {
    // Connect as the client user (already registered)
    const client = await WasmMatrixClient.login(tuwunel.url, CLIENT_NAME, PASSWORD);
    console.log(`[e2e] Client logged in as ${client.userId()}`);

    // Sync to get invitations
    await client.syncOnce();

    // Accept all invitations
    const invited = client.invitedRoomIds();
    console.log(`[e2e] Client has ${invited.length} invitations`);
    for (const roomId of invited) {
      try {
        await client.joinRoom(roomId);
        console.log(`[e2e] Joined room ${roomId}`);
      } catch (e) {
        console.log(`[e2e] Failed to join ${roomId}: ${e}`);
      }
    }

    // Sync after joining
    await client.syncOnce();

    // Find the launcher
    const topology = await client.findLauncherSpace(LAUNCHER_NAME);
    assert.ok(topology, `Should find launcher '${LAUNCHER_NAME}'`);
    console.log(`[e2e] Found launcher topology:`, topology);

    // Send command
    const crypto = await import('node:crypto');
    const requestId = crypto.randomUUID();
    await client.sendEvent(
      topology.exec_room_id,
      'org.mxdx.session.task',
      JSON.stringify({
        request_id: requestId,
        command: 'echo',
        args: ['hello-from-e2e'],
        cwd: '/tmp',
      }),
    );
    console.log(`[e2e] Command sent with request_id=${requestId}`);

    // Wait for result (poll)
    let resultFound = false;
    const deadline = Date.now() + 15000;
    while (Date.now() < deadline && !resultFound) {
      await client.syncOnce();
      const eventsJson = await client.collectRoomEvents(topology.exec_room_id, 1);
      const events = JSON.parse(eventsJson);
      if (events && Array.isArray(events)) {
        for (const event of events) {
          if (event.type === 'org.mxdx.session.result' && event.content?.request_id === requestId) {
            console.log(`[e2e] Got result:`, event.content);
            assert.strictEqual(event.content.exit_code, 0, 'Exit code should be 0');
            resultFound = true;
          }
        }
      }
    }

    assert.ok(resultFound, 'Should receive org.mxdx.session.result event');
    client.free();
  });
});

function waitForOutput(proc, needle, timeoutMs) {
  return new Promise((resolve) => {
    let output = '';
    const timeout = setTimeout(() => resolve(false), timeoutMs);

    proc.stdout.on('data', (chunk) => {
      output += chunk.toString();
      if (output.includes(needle)) {
        clearTimeout(timeout);
        resolve(true);
      }
    });
    proc.stderr.on('data', (chunk) => {
      output += chunk.toString();
      if (output.includes(needle)) {
        clearTimeout(timeout);
        resolve(true);
      }
    });
    proc.on('close', () => {
      clearTimeout(timeout);
      resolve(false);
    });
  });
}
