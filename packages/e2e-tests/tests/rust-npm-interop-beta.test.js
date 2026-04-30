/**
 * Rust ↔ npm Interop Beta E2E: 8-combination test matrix.
 *
 * {rust|npm client} × {rust worker|npm launcher} × {same-hs|federated}
 *
 * Parameterized via combinations.forEach to prevent N×M drift.
 * Per ADR 2026-04-29 Pillar 2, req 7/10: all 8 combinations must run;
 * 6 are skipped pending Phase 2 wiring (T-2.3, T-2.4, T-2.5, T-2.6).
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

/** All 8 {client_runtime × worker_runtime × hs_topology} combinations. */
const COMBINATIONS = [
  { id: 't1a', client_runtime: 'rust', worker_runtime: 'rust', hs_topology: 'same-hs' },
  { id: 't1b', client_runtime: 'rust', worker_runtime: 'rust', hs_topology: 'federated' },
  { id: 't2a', client_runtime: 'npm', worker_runtime: 'rust', hs_topology: 'same-hs' },
  { id: 't2b', client_runtime: 'npm', worker_runtime: 'rust', hs_topology: 'federated' },
  { id: 't3a', client_runtime: 'rust', worker_runtime: 'npm', hs_topology: 'same-hs' },
  { id: 't3b', client_runtime: 'rust', worker_runtime: 'npm', hs_topology: 'federated' },
  { id: 't4a', client_runtime: 'npm', worker_runtime: 'npm', hs_topology: 'same-hs' },
  { id: 't4b', client_runtime: 'npm', worker_runtime: 'npm', hs_topology: 'federated' },
];

// Per-combination env-var overrides for debugging
function isEnvSkipped({ client_runtime, worker_runtime }) {
  const key = `SKIP_${client_runtime.toUpperCase()}_${worker_runtime.toUpperCase()}`;
  return process.env[key] === '1' ? `${key}=1` : null;
}

// npm subprocess wiring is pending T-2.4 (Phase 2)
function npmNotWired({ worker_runtime, client_runtime }) {
  if (worker_runtime === 'npm' || client_runtime === 'npm') {
    return 'npm launcher/client subprocess not yet wired (pending T-2.4)';
  }
  return null;
}

function skipFor(combo) {
  return isEnvSkipped(combo) ?? npmNotWired(combo) ?? (skipReason || undefined);
}

describe('Rust ↔ npm Interop Beta', {
  skip: skipReason,
  timeout: 300_000,
}, () => {
  let creds;

  before(() => {
    creds = loadBetaCredentials();
  });

  COMBINATIONS.forEach(({ id, client_runtime, worker_runtime, hs_topology }) => {
    const label = `${id}: ${client_runtime} client → ${worker_runtime} worker (${hs_topology})`;
    const skipMsg = skipFor({ client_runtime, worker_runtime });

    it(label, { skip: skipMsg }, async () => {
      // -- Rust worker, Rust client (t1a / t1b) --
      if (worker_runtime === 'rust' && client_runtime === 'rust') {
        const workerServer = hs_topology === 'federated' ? creds.server2.url : creds.server.url;
        const clientServer = creds.server.url;

        // --p2p flag removed pending T-2.3 (fix clap-parse bug on mxdx-worker)
        const worker = spawnRustBinary('mxdx-worker', [
          'start', '--homeserver', workerServer,
          '--username', creds.account1.username,
          '--password', creds.account1.password,
        ]);

        try {
          await worker.waitForOutput('worker ready', 30_000);

          const client = spawnRustBinary('mxdx-client', [
            '--homeserver', clientServer,
            '--username', creds.account2.username,
            '--password', creds.account2.password,
            'exec', 'echo', `interop-${id}`,
          ]);

          try {
            const { code, stdout } = await client.waitForExit(
              hs_topology === 'federated' ? 90_000 : 60_000,
            );
            assert.equal(code, 0);
            assert.ok(stdout.includes(`interop-${id}`));
          } finally {
            client.kill();
          }
        } finally {
          worker.kill();
        }
        return;
      }

      // npm combinations are skipped via skipFor() above; this body is unreachable
      // until T-2.4/T-2.5/T-2.6 wire subprocess spawning in Phase 2.
      assert.fail(`Combination ${id} reached implementation body without npm wiring`);
    });
  });
});
