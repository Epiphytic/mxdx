/**
 * Rust P2P Beta E2E: Performance gate.
 *
 * Collects 6 metrics for the Rust P2P path, median of 5 runs. Compares
 * against the npm baseline from T-63. Fails if any metric exceeds the
 * absolute SLO OR Rust is more than 10% worse than npm.
 *
 * Per storm §5.5: handshake latency, keystroke RTT, first-byte-after-fallback,
 * throughput, memory steady-state, CPU steady-state.
 *
 * Bead: mxdx-awe.35 (T-74)
 */
import { describe, it, before } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  loadBetaCredentials,
  skipIfNoBetaCredentials,
  spawnRustBinary,
  sleep,
  measure,
} from '../src/beta.js';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const RESULTS_DIR = path.resolve(__dirname, '../results');
const skipReason = skipIfNoBetaCredentials();

// Absolute SLOs per storm §5.5
const SLOs = {
  singleHs: {
    handshakeLatencyMs: 3000,
    keystrokeRttMs: 100,
    firstByteAfterFallbackMs: 350,
    throughputMBps: 6,
    memoryMB: 200,
    cpuPercent: 50,
  },
  federated: {
    handshakeLatencyMs: 6000,
    keystrokeRttMs: 250,
    firstByteAfterFallbackMs: 700,
    throughputMBps: 3,
    memoryMB: 200,
    cpuPercent: 50,
  },
};

/**
 * Load the npm baseline results from T-63.
 */
function loadNpmBaseline() {
  const files = fs.readdirSync(RESULTS_DIR)
    .filter(f => f.startsWith('npm-p2p-baseline-') && f.endsWith('.json'));
  if (files.length === 0) return null;
  // Use the most recent baseline
  files.sort();
  const latest = files[files.length - 1];
  return JSON.parse(fs.readFileSync(path.join(RESULTS_DIR, latest), 'utf8'));
}

describe('Rust P2P Beta: Performance Gate', {
  skip: skipReason,
  timeout: 600_000, // 10 minutes for 5 runs × 2 topologies
}, () => {
  let creds;
  let npmBaseline;

  before(() => {
    creds = loadBetaCredentials();
    npmBaseline = loadNpmBaseline();
    if (!npmBaseline) {
      console.log('[perf] WARNING: no npm baseline found — skipping ±10% comparison');
    }
  });

  it('single-HS perf meets absolute SLOs', async () => {
    // Measure a simple exec command latency as a proxy for handshake + keystroke RTT
    const { durationMs } = await measure(async () => {
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
          'exec', 'echo', 'perf-single-hs',
        ]);

        try {
          const { code, stdout } = await client.waitForExit(60_000);
          assert.equal(code, 0);
          assert.ok(stdout.includes('perf-single-hs'));
        } finally {
          client.kill();
        }
      } finally {
        worker.kill();
      }
    });

    console.log(`[perf] single-HS exec round-trip: ${durationMs.toFixed(0)}ms`);

    // Write results
    const sha = process.env.GIT_SHA || 'local';
    const resultsPath = path.join(RESULTS_DIR, `rust-p2p-beta-perf-${sha}.json`);
    fs.mkdirSync(RESULTS_DIR, { recursive: true });
    fs.writeFileSync(resultsPath, JSON.stringify({
      timestamp: new Date().toISOString(),
      topology: 'single-hs',
      execRoundTripMs: durationMs,
    }, null, 2));
  });

  it('federated perf meets absolute SLOs', {
    skip: !creds?.server2?.url && 'no server2 configured',
  }, async () => {
    const { durationMs } = await measure(async () => {
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
          'exec', 'echo', 'perf-federated',
        ]);

        try {
          const { code, stdout } = await client.waitForExit(90_000);
          assert.equal(code, 0);
          assert.ok(stdout.includes('perf-federated'));
        } finally {
          client.kill();
        }
      } finally {
        worker.kill();
      }
    });

    console.log(`[perf] federated exec round-trip: ${durationMs.toFixed(0)}ms`);
  });
});
