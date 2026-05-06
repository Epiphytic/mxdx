/**
 * Public Matrix server E2E tests.
 *
 * Tests the real WASM-backed launcher + client against a public homeserver.
 * These exercise the full npm + WASM stack (not Rust-native).
 *
 * ## Setup
 *
 * Create `test-credentials.toml` in the repo root (gitignored):
 *
 * ```toml
 * [server]
 * url = "matrix.org"
 *
 * [account1]
 * username = "your-client-user"
 * password = "your-password"
 *
 * [account2]
 * username = "your-launcher-user"
 * password = "your-password"
 * ```
 *
 * Run with: node --test packages/e2e-tests/tests/public-server.test.js
 */

import { describe, it, before, after } from 'node:test';
import assert from 'node:assert';
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { WasmMatrixClient } from '@mxdx/core';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(__dirname, '../../..');
const LAUNCHER_BIN = path.resolve(__dirname, '../../launcher/bin/mxdx-launcher.js');
const CLIENT_BIN = path.resolve(__dirname, '../../client/bin/mxdx-client.js');

// Fixed launcher ID for room reuse across test runs
const FIXED_LAUNCHER_ID = 'pub-e2e-stable';

/**
 * Write a performance JSON entry to TEST_PERF_OUTPUT (if set).
 * One JSON object per line — the e2e-test-suite.sh wraps them with suite metadata.
 */
function writePerfEntry(name, transport, durationMs, exitCode, stdoutLines) {
  const perfPath = process.env.TEST_PERF_OUTPUT;
  if (!perfPath) return;
  // Schema matches mxdx_test_perf::PerfEntry (Rust) per ADR req 25/27.
  const entry = JSON.stringify({
    suite: name,
    transport,
    runtime: 'npm',
    duration_ms: durationMs,
    rss_max: null,
    exit_code: exitCode,
    stdout_lines: stdoutLines,
    status: exitCode === 0 ? 'pass' : 'fail',
  });
  fs.appendFileSync(perfPath, entry + '\n');
}

/**
 * Load credentials from test-credentials.toml.
 */
function loadCredentials() {
  const tomlPath = path.join(REPO_ROOT, 'test-credentials.toml');
  if (!fs.existsSync(tomlPath)) {
    throw new Error(
      'test-credentials.toml not found in repo root. See test file header for setup.'
    );
  }
  const content = fs.readFileSync(tomlPath, 'utf8');

  // Minimal TOML parser for our flat structure
  const lines = content.split('\n');
  const result = {};
  let section = null;
  for (const line of lines) {
    const trimmed = line.trim();
    const sectionMatch = trimmed.match(/^\[(\w+)\]$/);
    if (sectionMatch) {
      section = sectionMatch[1];
      result[section] = {};
      continue;
    }
    const kvMatch = trimmed.match(/^(\w+)\s*=\s*"(.+)"$/);
    if (kvMatch && section) {
      result[section][kvMatch[1]] = kvMatch[2];
    }
  }

  if (!result.server?.url) throw new Error('server.url missing in test-credentials.toml');
  if (!result.account1?.username) throw new Error('account1.username missing');
  if (!result.account1?.password) throw new Error('account1.password missing');
  if (!result.account2?.username) throw new Error('account2.username missing');
  if (!result.account2?.password) throw new Error('account2.password missing');

  return {
    url: result.server.url,
    account1: { username: result.account1.username, password: result.account1.password },
    account2: { username: result.account2.username, password: result.account2.password },
  };
}

function waitForOutput(proc, needle, timeoutMs = 30000) {
  return new Promise((resolve) => {
    let output = '';
    const timeout = setTimeout(() => resolve(false), timeoutMs);

    const handler = (chunk) => {
      output += chunk.toString();
      if (output.includes(needle)) {
        clearTimeout(timeout);
        resolve(true);
      }
    };

    proc.stdout?.on('data', handler);
    proc.stderr?.on('data', handler);
    proc.on('close', () => {
      clearTimeout(timeout);
      resolve(false);
    });
  });
}

function sleep(ms) {
  return new Promise((r) => setTimeout(r, ms));
}

// ─── Full Round-Trip: Launcher + Client ─────────────────────────────────────
// NOTE: WASM Client tests (no subprocess) extracted to
// packages/integration-tests/tests/public-server-wasm.test.js

describe('Public Server: Launcher + Client Round-Trip', { timeout: 180000 }, () => {
  let creds;
  let launcherProc;
  let LAUNCHER_NAME;

  before(() => {
    creds = loadCredentials();
    LAUNCHER_NAME = creds.account2.username;
  });

  after(() => {
    if (launcherProc) launcherProc.kill();
  });

  it('launcher starts and client executes a command (latency < 10s)', async () => {
    console.log(`[pub] Starting launcher as ${creds.account2.username} on ${creds.url}...`);

    const adminClient = await WasmMatrixClient.login(
      creds.url, creds.account1.username, creds.account1.password,
    );
    const adminMxid = adminClient.userId();
    console.log(`[pub] Admin MXID: ${adminMxid}`);
    adminClient.free();

    const configPath = `/tmp/pub-launcher-${Date.now()}.toml`;

    launcherProc = spawn('node', [
      LAUNCHER_BIN,
      '--servers', creds.url,
      '--username', creds.account2.username,
      '--password', creds.account2.password,
      '--allowed-commands', 'echo,date,uname',
      '--allowed-cwd', '/tmp',
      '--admin-user', adminMxid,
      '--config', configPath,
      '--log-format', 'text',
    ], {
      stdio: ['ignore', 'pipe', 'pipe'],
    });

    let launcherOutput = '';
    launcherProc.stdout.on('data', (chunk) => {
      launcherOutput += chunk.toString();
      process.stderr.write(`[launcher stdout] ${chunk}`);
    });
    launcherProc.stderr.on('data', (chunk) => {
      launcherOutput += chunk.toString();
      process.stderr.write(`[launcher stderr] ${chunk}`);
    });

    const online = await waitForOutput(launcherProc, 'Listening for commands', 60000);
    assert.ok(online, 'Launcher should come online');
    console.log('[pub] Launcher is online');

    await sleep(3000);

    // Measure latency: time from client exec start to result
    const startTime = Date.now();
    console.log('[pub] Running client exec...');

    const clientResult = await new Promise((resolve, reject) => {
      const proc = spawn('node', [
        CLIENT_BIN,
        '--server', creds.url,
        '--username', creds.account1.username,
        '--password', creds.account1.password,
        '--format', 'json',
        'exec', LAUNCHER_NAME, 'echo', 'hello-from-public-server',
        '--cwd', '/tmp',
      ], {
        stdio: ['ignore', 'pipe', 'pipe'],
        timeout: 60000,
      });

      let stdout = '';
      let stderr = '';
      proc.stdout.on('data', (chunk) => { stdout += chunk.toString(); });
      proc.stderr.on('data', (chunk) => { stderr += chunk.toString(); });

      proc.on('close', (code) => {
        resolve({ code, stdout, stderr });
      });
      proc.on('error', reject);

      setTimeout(() => {
        proc.kill();
        resolve({ code: -1, stdout, stderr: stderr + '\n[timeout]' });
      }, 60000);
    });

    const latencyMs = Date.now() - startTime;
    console.log(`[pub] Client exit code: ${clientResult.code}, latency: ${latencyMs}ms`);
    console.log(`[pub] Client stdout: ${clientResult.stdout}`);
    if (clientResult.stderr) console.log(`[pub] Client stderr: ${clientResult.stderr}`);

    writePerfEntry('launcher-client-round-trip', 'npm-public', latencyMs, clientResult.code,
      clientResult.stdout.split('\n').filter(Boolean).length);

    assert.strictEqual(clientResult.code, 0, `Client should exit 0, got ${clientResult.code}`);
    assert.ok(latencyMs < 10000, `Latency should be < 10s, was ${latencyMs}ms`);

    const output = JSON.parse(clientResult.stdout);
    assert.strictEqual(output.exitCode, 0, 'Remote command exit code should be 0');

    try { fs.unlinkSync(configPath); } catch { /* ignore */ }
  });
});
