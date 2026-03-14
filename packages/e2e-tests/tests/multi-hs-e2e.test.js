import { describe, it, before, after } from 'node:test';
import assert from 'node:assert';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { TuwunelInstance } from '../src/tuwunel.js';
import { MultiHsClient } from '@mxdx/core';

const tuwunelAvailable = TuwunelInstance.isAvailable();

describe('E2E: Multi-Homeserver', { skip: !tuwunelAvailable && 'tuwunel binary not found', timeout: 180000 }, () => {
  let tuwunel1, tuwunel2;
  const LAUNCHER_NAME = `mhs-launcher-${Date.now()}`;
  const PASSWORD = 'testpass123';
  const configDir = fs.mkdtempSync(path.join(os.tmpdir(), 'mxdx-mhs-e2e-'));

  before(async () => {
    [tuwunel1, tuwunel2] = await Promise.all([
      TuwunelInstance.start(),
      TuwunelInstance.start(),
    ]);
    console.log(`[e2e] Tuwunel 1: ${tuwunel1.url}`);
    console.log(`[e2e] Tuwunel 2: ${tuwunel2.url}`);
  });

  after(async () => {
    tuwunel1?.stop();
    tuwunel2?.stop();
    try { fs.rmSync(configDir, { recursive: true }); } catch {}
  });

  it('launcher connects to both servers and selects preferred by latency', async () => {
    // Use registrationToken so connectWithSession handles register+login in one flow
    const configs = [
      { username: LAUNCHER_NAME, server: tuwunel1.url, password: PASSWORD, registrationToken: tuwunel1.registrationToken, configDir, useKeychain: false, log: console.log },
      { username: LAUNCHER_NAME, server: tuwunel2.url, password: PASSWORD, registrationToken: tuwunel2.registrationToken, configDir, useKeychain: false, log: console.log },
    ];
    const mhs = await MultiHsClient.connect(configs, { log: console.log });

    assert.strictEqual(mhs.serverCount, 2);
    assert.ok(mhs.preferred.server);
    assert.ok(mhs.userId());

    const health = mhs.serverHealth();
    assert.strictEqual(health.size, 2);
    for (const [, h] of health) {
      assert.strictEqual(h.status, 'healthy');
      assert.ok(h.latencyMs > 0);
    }

    await mhs.shutdown();
  });

  it('sends command via preferred server', async () => {
    // Reuse existing accounts (no registrationToken needed — sessions cached)
    const configs = [
      { username: LAUNCHER_NAME, server: tuwunel1.url, password: PASSWORD, configDir, useKeychain: false, log: () => {} },
      { username: LAUNCHER_NAME, server: tuwunel2.url, password: PASSWORD, configDir, useKeychain: false, log: () => {} },
    ];
    const mhs = await MultiHsClient.connect(configs, { log: () => {} });

    const topology = await mhs.getOrCreateLauncherSpace(LAUNCHER_NAME);
    assert.ok(topology.exec_room_id);

    await mhs.sendEvent(
      topology.exec_room_id,
      'org.mxdx.command',
      JSON.stringify({ request_id: 'test-1', command: 'echo', args: ['hello'] }),
    );

    await mhs.syncOnce();
    const events = JSON.parse(await mhs.collectRoomEvents(topology.exec_room_id, 1));
    assert.ok(Array.isArray(events));

    await mhs.shutdown();
  });

  it('single server behaves identically to existing behavior', async () => {
    const configs = [
      { username: LAUNCHER_NAME, server: tuwunel1.url, password: PASSWORD, configDir, useKeychain: false, log: () => {} },
    ];
    const mhs = await MultiHsClient.connect(configs, { log: () => {} });

    assert.strictEqual(mhs.serverCount, 1);
    assert.strictEqual(mhs.isSingleServer, true);

    await mhs.syncOnce();
    assert.ok(mhs.userId());
    assert.ok(mhs.deviceId());

    await mhs.shutdown();
  });

  it('preferredServer config override pins to specific server', async () => {
    const configs = [
      { username: LAUNCHER_NAME, server: tuwunel1.url, password: PASSWORD, configDir, useKeychain: false, log: () => {} },
      { username: LAUNCHER_NAME, server: tuwunel2.url, password: PASSWORD, configDir, useKeychain: false, log: () => {} },
    ];
    const mhs = await MultiHsClient.connect(configs, {
      preferredServer: tuwunel2.url,
      log: () => {},
    });

    assert.strictEqual(mhs.preferred.server, tuwunel2.url);
    await mhs.shutdown();
  });
});
