/**
 * Rust P2P Beta E2E: Federated tests (ca1-beta <-> ca2-beta).
 *
 * Worker on ca2-beta, client on ca1-beta. Verifies P2P establishes
 * across federated homeservers and Megolm decryption works on both sides.
 *
 * Bead: mxdx-awe.33 (T-72)
 */
import { describe, it, before, after } from 'node:test';
import assert from 'node:assert/strict';
import {
  loadBetaCredentials,
  skipIfNoFederatedCredentials,
  spawnRustBinary,
  sleep,
} from '../src/beta.js';

const skipReason = skipIfNoFederatedCredentials();

describe('Rust P2P Beta: Federated (ca1 <-> ca2)', {
  skip: skipReason,
  timeout: 180_000,
}, () => {
  let creds;

  before(() => {
    creds = loadBetaCredentials();
  });

  it('P2P establishes across federated homeservers', async () => {
    // Worker on ca2-beta (server2)
    const worker = spawnRustBinary('mxdx-worker', [
      '--server', creds.server2.url,
      '--username', creds.account1.username,
      '--password', creds.account1.password,
      '--p2p',
    ], {
      env: { RUST_LOG: 'info,mxdx_p2p=debug' },
    });

    try {
      await worker.waitForOutput('worker ready', 30_000);

      // Client on ca1-beta (server)
      const client = spawnRustBinary('mxdx-client', [
        '--server', creds.server.url,
        '--username', creds.account2.username,
        '--password', creds.account2.password,
        '--p2p',
        'exec', 'echo', 'federated-p2p-test',
      ], {
        env: { RUST_LOG: 'info,mxdx_p2p=debug' },
      });

      try {
        const { code, stdout, stderr } = await client.waitForExit(90_000);
        assert.equal(code, 0,
          `Federated P2P test failed with code ${code}.\n` +
          `stdout: ${stdout.slice(-500)}\nstderr: ${stderr.slice(-500)}`);
        assert.ok(
          stdout.includes('federated-p2p-test'),
          `Expected 'federated-p2p-test' in output.\nstdout: ${stdout}`,
        );
      } finally {
        client.kill();
      }
    } finally {
      worker.kill();
    }
  });

  it('Megolm decryption works on both sides of federation', async () => {
    // Worker on ca2-beta
    const worker = spawnRustBinary('mxdx-worker', [
      '--server', creds.server2.url,
      '--username', creds.account1.username,
      '--password', creds.account1.password,
      '--p2p',
    ], {
      env: { RUST_LOG: 'info,mxdx_p2p=debug' },
    });

    try {
      await worker.waitForOutput('worker ready', 30_000);

      // Run a multi-output command to verify bidirectional E2EE
      const client = spawnRustBinary('mxdx-client', [
        '--server', creds.server.url,
        '--username', creds.account2.username,
        '--password', creds.account2.password,
        '--p2p',
        'exec', 'sh', '-c', 'echo line1 && echo line2 && echo line3',
      ], {
        env: { RUST_LOG: 'info,mxdx_p2p=debug' },
      });

      try {
        const { code, stdout, stderr } = await client.waitForExit(90_000);
        assert.equal(code, 0,
          `Federated Megolm test failed.\nstderr: ${stderr.slice(-500)}`);

        // All three lines should be decrypted and delivered in order
        const lines = stdout.trim().split('\n').map(l => l.trim());
        assert.ok(lines.includes('line1'), 'Missing line1');
        assert.ok(lines.includes('line2'), 'Missing line2');
        assert.ok(lines.includes('line3'), 'Missing line3');

        // Verify ordering
        const idx1 = lines.indexOf('line1');
        const idx2 = lines.indexOf('line2');
        const idx3 = lines.indexOf('line3');
        assert.ok(idx1 < idx2 && idx2 < idx3, 'Lines should be in order');
      } finally {
        client.kill();
      }
    } finally {
      worker.kill();
    }
  });
});
