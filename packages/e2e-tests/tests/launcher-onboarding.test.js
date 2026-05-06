import { describe, it, before, after } from 'node:test';
import assert from 'node:assert';
import { spawn } from 'node:child_process';
import path from 'node:path';
import fs from 'node:fs';
import { fileURLToPath } from 'node:url';
import { TuwunelInstance } from '../src/tuwunel.js';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const LAUNCHER_BIN = path.resolve(__dirname, '../../launcher/bin/mxdx-launcher.js');

const tuwunelAvailable = TuwunelInstance.isAvailable();

describe('E2E: Launcher Onboarding', { skip: !tuwunelAvailable && 'tuwunel binary not found' }, () => {
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

// NOTE: WASM: Room Topology block (WasmMatrixClient-direct) extracted to
// packages/integration-tests/tests/launcher-onboarding-wasm.test.js
