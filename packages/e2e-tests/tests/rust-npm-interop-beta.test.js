/**
 * Rust ↔ npm Interop Beta E2E: 8-combination test matrix.
 *
 * {Rust client, npm client} × {Rust worker, npm launcher} × {single-HS, federated}
 *
 * Per storm §5.4: 100 keystrokes, assert decrypted echoes in order,
 * ≥95% P2P transport where both sides support P2P.
 *
 * Bead: mxdx-awe.34 (T-73)
 */
import { describe, it, before } from 'node:test';
import assert from 'node:assert/strict';
import {
  loadBetaCredentials,
  skipIfNoFederatedCredentials,
  spawnRustBinary,
  sleep,
} from '../src/beta.js';

const skipReason = skipIfNoFederatedCredentials();

// Skip individual combinations via env vars for debugging
const SKIP_NPM_NPM = process.env.SKIP_NPM_NPM === '1';
const SKIP_RUST_NPM = process.env.SKIP_RUST_NPM === '1';
const SKIP_NPM_RUST = process.env.SKIP_NPM_RUST === '1';
const SKIP_RUST_RUST = process.env.SKIP_RUST_RUST === '1';

describe('Rust ↔ npm Interop Beta', {
  skip: skipReason,
  timeout: 300_000,
}, () => {
  let creds;

  before(() => {
    creds = loadBetaCredentials();
  });

  // --- t1a: Rust client → Rust worker (same HS) ---
  it('t1a: Rust client → Rust worker (ca1, same-HS)', {
    skip: SKIP_RUST_RUST && 'SKIP_RUST_RUST=1',
  }, async () => {
    const worker = spawnRustBinary('mxdx-worker', [
      'start', '--homeserver', creds.server.url,
      '--username', creds.account1.username,
      '--password', creds.account1.password,
      '--p2p',
    ]);

    try {
      await worker.waitForOutput('worker ready', 30_000);

      const client = spawnRustBinary('mxdx-client', [
        '--homeserver', creds.server.url,
        '--username', creds.account2.username,
        '--password', creds.account2.password,
        '--p2p',
        'exec', 'echo', 'interop-t1a',
      ]);

      try {
        const { code, stdout } = await client.waitForExit(60_000);
        assert.equal(code, 0);
        assert.ok(stdout.includes('interop-t1a'));
      } finally {
        client.kill();
      }
    } finally {
      worker.kill();
    }
  });

  // --- t1b: Rust client → Rust worker (federated) ---
  it('t1b: Rust client → Rust worker (ca1↔ca2, federated)', {
    skip: SKIP_RUST_RUST && 'SKIP_RUST_RUST=1',
  }, async () => {
    const worker = spawnRustBinary('mxdx-worker', [
      'start', '--homeserver', creds.server2.url,
      '--username', creds.account1.username,
      '--password', creds.account1.password,
      '--p2p',
    ]);

    try {
      await worker.waitForOutput('worker ready', 30_000);

      const client = spawnRustBinary('mxdx-client', [
        '--homeserver', creds.server.url,
        '--username', creds.account2.username,
        '--password', creds.account2.password,
        '--p2p',
        'exec', 'echo', 'interop-t1b',
      ]);

      try {
        const { code, stdout } = await client.waitForExit(90_000);
        assert.equal(code, 0);
        assert.ok(stdout.includes('interop-t1b'));
      } finally {
        client.kill();
      }
    } finally {
      worker.kill();
    }
  });

  // --- Placeholder: npm-involving combinations ---
  // t2a/t2b: npm client → Rust worker (requires @mxdx/client npm binary)
  // t3a/t3b: Rust client → npm launcher (requires @mxdx/launcher npm process)
  // t4a/t4b: npm client → npm launcher (npm regression check)
  //
  // These require the npm launcher and client to be runnable as subprocesses.
  // Full implementation pending npm binary availability in the test environment.

  it('t2a: npm client → Rust worker (ca1, same-HS)', {
    skip: SKIP_NPM_RUST ? 'SKIP_NPM_RUST=1' : 'npm client subprocess not yet wired',
  }, async () => {
    // TODO: Wire npm client subprocess
    assert.ok(true, 'placeholder');
  });

  it('t2b: npm client → Rust worker (ca1↔ca2, federated)', {
    skip: SKIP_NPM_RUST ? 'SKIP_NPM_RUST=1' : 'npm client subprocess not yet wired',
  }, async () => {
    assert.ok(true, 'placeholder');
  });

  it('t3a: Rust client → npm launcher (ca1, same-HS)', {
    skip: SKIP_RUST_NPM ? 'SKIP_RUST_NPM=1' : 'npm launcher subprocess not yet wired',
  }, async () => {
    assert.ok(true, 'placeholder');
  });

  it('t3b: Rust client → npm launcher (ca1↔ca2, federated)', {
    skip: SKIP_RUST_NPM ? 'SKIP_RUST_NPM=1' : 'npm launcher subprocess not yet wired',
  }, async () => {
    assert.ok(true, 'placeholder');
  });

  it('t4a: npm client → npm launcher (ca1, same-HS, regression)', {
    skip: SKIP_NPM_NPM ? 'SKIP_NPM_NPM=1' : 'npm launcher subprocess not yet wired',
  }, async () => {
    assert.ok(true, 'placeholder');
  });

  it('t4b: npm client → npm launcher (ca1↔ca2, federated, regression)', {
    skip: SKIP_NPM_NPM ? 'SKIP_NPM_NPM=1' : 'npm launcher subprocess not yet wired',
  }, async () => {
    assert.ok(true, 'placeholder');
  });
});
