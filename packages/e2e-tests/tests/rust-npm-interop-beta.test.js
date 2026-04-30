/**
 * Rust ↔ npm Interop Beta E2E: 8-combination test matrix.
 *
 * {rust|npm client} × {rust worker|npm launcher} × {same-hs|federated}
 *
 * Parameterized via combinations.forEach to prevent N×M drift.
 * Per ADR 2026-04-29 Pillar 2, req 7/10: all 8 combinations must run.
 *
 * MSC4362 traceability: every Matrix event in these tests (exec commands,
 * session output) MUST be encrypted on the wire under the
 * experimental-encrypted-state-events extension. Both Rust (matrix-sdk
 * with msrv 0.16 + experimental-encrypted-state-events feature) and npm
 * (WasmMatrixClient via mxdx-core-wasm with MSC4362 enabled) enforce this.
 * See: docs/adr/2026-04-29-rust-npm-binary-parity.md Pillar 2 + CLAUDE.md.
 */
import { describe, it, before } from 'node:test';
import assert from 'node:assert/strict';
import {
  loadBetaCredentials,
  skipIfNoFederatedCredentials,
  spawnRustBinary,
  spawnNpmBinary,
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
  // advisory: not yet blocking (P2P quarantine — see wire-format-parity-gate-policy.md)
  // Security doc: docs/reviews/security/2026-04-29-p2p-cross-runtime-dtls-verification.md
  { id: 't4a', client_runtime: 'npm', worker_runtime: 'npm', hs_topology: 'same-hs', advisory: true },
  { id: 't4b', client_runtime: 'npm', worker_runtime: 'npm', hs_topology: 'federated', advisory: true },
];

// Per-combination env-var overrides for debugging
function isEnvSkipped({ client_runtime, worker_runtime }) {
  const key = `SKIP_${client_runtime.toUpperCase()}_${worker_runtime.toUpperCase()}`;
  return process.env[key] === '1' ? `${key}=1` : null;
}

function skipFor(combo) {
  return isEnvSkipped(combo) ?? (skipReason || undefined);
}

describe('Rust ↔ npm Interop Beta', {
  skip: skipReason,
  timeout: 300_000,
}, () => {
  let creds;

  before(() => {
    creds = loadBetaCredentials();
  });

  COMBINATIONS.forEach(({ id, client_runtime, worker_runtime, hs_topology, advisory }) => {
    const label = `${id}: ${client_runtime} client → ${worker_runtime} worker (${hs_topology})${advisory ? ' [advisory]' : ''}`;
    const skipMsg = skipFor({ client_runtime, worker_runtime });

    it(label, { skip: skipMsg }, async () => {
      const workerServer = hs_topology === 'federated' ? creds.server2.url : creds.server.url;
      const clientServer = creds.server.url;
      const timeout = hs_topology === 'federated' ? 90_000 : 60_000;

      // -- Rust worker, Rust client (t1a / t1b) --
      if (worker_runtime === 'rust' && client_runtime === 'rust') {
        const worker = spawnRustBinary('mxdx-worker', [
          'start', '--homeserver', workerServer,
          '--username', creds.account1.username,
          '--password', creds.account1.password,
          '--allowed-command', 'echo',
          '--allowed-command', 'date',
          '--allowed-command', 'uname',
          '--allowed-cwd', '/tmp',
        ]);

        try {
          await worker.waitForOutput('MXDX_WORKER_READY', 30_000);

          const client = spawnRustBinary('mxdx-client', [
            '--homeserver', clientServer,
            '--username', creds.account2.username,
            '--password', creds.account2.password,
            'exec', '--cwd', '/tmp', 'echo', `interop-${id}`,
          ]);

          try {
            const { code, stdout } = await client.waitForExit(timeout);
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

      // -- npm client → Rust worker (t2a / t2b) --
      if (worker_runtime === 'rust' && client_runtime === 'npm') {
        const worker = spawnRustBinary('mxdx-worker', [
          'start', '--homeserver', workerServer,
          '--username', creds.account1.username,
          '--password', creds.account1.password,
          '--allowed-command', 'echo',
          '--allowed-command', 'date',
          '--allowed-command', 'uname',
          '--allowed-cwd', '/tmp',
        ]);

        try {
          await worker.waitForOutput('MXDX_WORKER_READY', 30_000);

          const client = spawnNpmBinary('client', [
            '--server', clientServer,
            '--username', creds.account2.username,
            '--password', creds.account2.password,
            'exec', creds.account1.username, 'echo', `interop-${id}`,
          ]);

          try {
            const { code, stdout } = await client.waitForExit(timeout);
            assert.equal(code, 0, `npm client should exit 0, got ${code}\nstdout: ${stdout}\nstderr: ${client.stderr}`);
            assert.ok(stdout.includes(`interop-${id}`), `stdout should contain interop-${id}, got: ${stdout}`);
          } finally {
            client.kill();
          }
        } finally {
          worker.kill();
        }
        return;
      }

      // -- Rust client → npm launcher (t3a / t3b) --
      if (worker_runtime === 'npm' && client_runtime === 'rust') {
        const launcher = spawnNpmBinary('launcher', [
          '--servers', workerServer,
          '--username', creds.account1.username,
          '--password', creds.account1.password,
          '--allowed-commands', 'echo,date,uname',
          '--allowed-cwd', '/tmp',
        ]);

        try {
          await launcher.waitForOutput('Listening for commands', 30_000);

          const client = spawnRustBinary('mxdx-client', [
            '--homeserver', clientServer,
            '--username', creds.account2.username,
            '--password', creds.account2.password,
            'exec', '--cwd', '/tmp', 'echo', `interop-${id}`,
          ]);

          try {
            const { code, stdout } = await client.waitForExit(timeout);
            assert.equal(code, 0, `Rust client should exit 0, got ${code}\nstdout: ${stdout}\nstderr: ${client.stderr}`);
            assert.ok(stdout.includes(`interop-${id}`), `stdout should contain interop-${id}, got: ${stdout}`);
          } finally {
            client.kill();
          }
        } finally {
          launcher.kill();
        }
        return;
      }

      // -- npm client → npm launcher (t4a / t4b) --
      // advisory: not yet blocking — P2P combinations pending security verification
      // Security doc: docs/reviews/security/2026-04-29-p2p-cross-runtime-dtls-verification.md
      if (worker_runtime === 'npm' && client_runtime === 'npm') {
        const launcher = spawnNpmBinary('launcher', [
          '--servers', workerServer,
          '--username', creds.account1.username,
          '--password', creds.account1.password,
          '--allowed-commands', 'echo,date,uname',
          '--allowed-cwd', '/tmp',
        ]);

        try {
          await launcher.waitForOutput('Listening for commands', 30_000);

          const client = spawnNpmBinary('client', [
            '--server', clientServer,
            '--username', creds.account2.username,
            '--password', creds.account2.password,
            'exec', creds.account1.username, 'echo', `interop-${id}`,
          ]);

          try {
            const { code, stdout } = await client.waitForExit(timeout);
            assert.equal(code, 0, `npm client should exit 0, got ${code}\nstdout: ${stdout}\nstderr: ${client.stderr}`);
            assert.ok(stdout.includes(`interop-${id}`), `stdout should contain interop-${id}, got: ${stdout}`);
          } finally {
            client.kill();
          }
        } finally {
          launcher.kill();
        }
        return;
      }

      assert.fail(`Combination ${id} has unhandled runtime combination: client=${client_runtime} worker=${worker_runtime}`);
    });
  });
});
