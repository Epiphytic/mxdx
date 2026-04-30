/**
 * Shared helpers for beta-server E2E tests.
 *
 * Centralizes credential loading, skip logic, timing tolerance, and
 * room provisioning that was previously duplicated across beta test files.
 *
 * Usage:
 *   import { loadBetaCredentials, skipIfNoBetaCredentials, ... } from '../src/beta.js';
 */

import fs from 'node:fs';
import path from 'node:path';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(__dirname, '..', '..', '..');
const CREDENTIALS_PATH = path.join(REPO_ROOT, 'test-credentials.toml');

/**
 * Parse the test-credentials.toml file. Returns an object with sections
 * as keys (server, server2, account1, account2, coordinator).
 *
 * Throws if the file doesn't exist or critical fields are missing.
 */
export function loadBetaCredentials() {
  if (!fs.existsSync(CREDENTIALS_PATH)) {
    throw new Error(
      'test-credentials.toml not found at ' + CREDENTIALS_PATH +
      '. See test-credentials.toml.example.',
    );
  }
  const content = fs.readFileSync(CREDENTIALS_PATH, 'utf8');
  const lines = content.split('\n');
  const result = {};
  let section = null;
  for (const line of lines) {
    const trimmed = line.trim();
    if (trimmed.startsWith('#') || trimmed.length === 0) continue;
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
  return result;
}

/**
 * Check if beta credentials are available without throwing.
 * Returns the credentials or null.
 */
export function tryLoadBetaCredentials() {
  try {
    return loadBetaCredentials();
  } catch {
    return null;
  }
}

/**
 * Skip the current test suite if beta credentials are not available.
 * Use as the `skip` option in `describe()`:
 *
 *   describe('Suite', { skip: skipIfNoBetaCredentials() }, () => { ... });
 *
 * Returns a skip reason string if credentials are missing, or false if OK.
 */
export function skipIfNoBetaCredentials() {
  if (!fs.existsSync(CREDENTIALS_PATH)) {
    return 'test-credentials.toml not found — skipping beta tests';
  }
  try {
    loadBetaCredentials();
    return false;
  } catch (e) {
    return `beta credentials incomplete: ${e.message}`;
  }
}

/**
 * Check if both beta servers (ca1 + ca2) are configured for federated tests.
 * Returns a skip reason or false.
 */
export function skipIfNoFederatedCredentials() {
  const skip = skipIfNoBetaCredentials();
  if (skip) return skip;
  const creds = loadBetaCredentials();
  if (!creds.server2?.url) {
    return 'server2 not configured in test-credentials.toml — skipping federated tests';
  }
  return false;
}

/**
 * Assert that an observed timing value is within tolerance of the expected.
 * Uses a wider tolerance than exact equality to account for network jitter
 * and CI variance.
 *
 * @param {number} observedMs - The measured duration in milliseconds
 * @param {number} expectedMs - The expected duration
 * @param {number} toleranceMs - Acceptable deviation (default 200ms)
 * @param {string} label - Description for error messages
 */
export function assertTimingTolerant(observedMs, expectedMs, toleranceMs = 200, label = 'timing') {
  const diff = Math.abs(observedMs - expectedMs);
  if (diff > toleranceMs) {
    throw new Error(
      `${label}: observed ${observedMs}ms vs expected ${expectedMs}ms ` +
      `(diff ${diff}ms exceeds tolerance ${toleranceMs}ms)`,
    );
  }
}

/**
 * Spawn a Rust binary (mxdx-worker or mxdx-client) as a subprocess.
 * Returns { proc, stdout, stderr, waitForExit, kill }.
 *
 * @param {string} binary - 'mxdx-worker' or 'mxdx-client'
 * @param {string[]} args - Command-line arguments
 * @param {object} opts - { env, cwd, timeout }
 */
export function spawnRustBinary(binary, args = [], opts = {}) {
  const binPath = path.join(REPO_ROOT, 'target', 'release', binary);
  if (!fs.existsSync(binPath)) {
    throw new Error(`${binary} not found at ${binPath}. Run: cargo build --release -p ${binary}`);
  }

  const env = { ...process.env, ...opts.env };
  const proc = spawn(binPath, args, {
    env,
    cwd: opts.cwd || REPO_ROOT,
    stdio: ['pipe', 'pipe', 'pipe'],
  });

  let stdoutBuf = '';
  let stderrBuf = '';
  proc.stdout.on('data', (d) => { stdoutBuf += d.toString(); });
  proc.stderr.on('data', (d) => { stderrBuf += d.toString(); });

  const waitForExit = (timeoutMs = 30000) =>
    new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        proc.kill('SIGTERM');
        reject(new Error(`${binary} did not exit within ${timeoutMs}ms`));
      }, timeoutMs);
      proc.on('exit', (code) => {
        clearTimeout(timer);
        resolve({ code, stdout: stdoutBuf, stderr: stderrBuf });
      });
    });

  const waitForOutput = (pattern, timeoutMs = 15000) =>
    new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        reject(new Error(
          `${binary}: pattern "${pattern}" not found in output within ${timeoutMs}ms.\n` +
          `stdout: ${stdoutBuf.slice(-500)}\nstderr: ${stderrBuf.slice(-500)}`,
        ));
      }, timeoutMs);
      const check = () => {
        if (stdoutBuf.includes(pattern) || stderrBuf.includes(pattern)) {
          clearTimeout(timer);
          resolve();
        } else {
          setTimeout(check, 50);
        }
      };
      check();
    });

  return {
    proc,
    get stdout() { return stdoutBuf; },
    get stderr() { return stderrBuf; },
    waitForExit,
    waitForOutput,
    kill: (signal = 'SIGTERM') => proc.kill(signal),
  };
}

/**
 * Spawn an npm binary (mxdx-launcher or mxdx-client) as a Node.js subprocess.
 * Returns the same interface as spawnRustBinary: { proc, stdout, stderr, waitForExit, waitForOutput, kill }.
 *
 * @param {'launcher'|'client'} binary - Which npm binary to spawn
 * @param {string[]} args - Command-line arguments
 * @param {object} opts - { env, cwd, timeout }
 */
export function spawnNpmBinary(binary, args = [], opts = {}) {
  const binMap = {
    launcher: path.join(REPO_ROOT, 'packages', 'launcher', 'bin', 'mxdx-launcher.js'),
    client: path.join(REPO_ROOT, 'packages', 'client', 'bin', 'mxdx-client.js'),
  };
  const binPath = binMap[binary];
  if (!binPath) {
    throw new Error(`Unknown npm binary '${binary}'. Valid values: ${Object.keys(binMap).join(', ')}`);
  }
  if (!fs.existsSync(binPath)) {
    throw new Error(`${binary} not found at ${binPath}.`);
  }

  const env = { ...process.env, ...opts.env };
  const proc = spawn(process.execPath, [binPath, ...args], {
    env,
    cwd: opts.cwd || REPO_ROOT,
    stdio: ['pipe', 'pipe', 'pipe'],
  });

  let stdoutBuf = '';
  let stderrBuf = '';
  proc.stdout.on('data', (d) => { stdoutBuf += d.toString(); });
  proc.stderr.on('data', (d) => { stderrBuf += d.toString(); });

  const waitForExit = (timeoutMs = 30000) =>
    new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        proc.kill('SIGTERM');
        reject(new Error(`npm ${binary} did not exit within ${timeoutMs}ms`));
      }, timeoutMs);
      proc.on('exit', (code) => {
        clearTimeout(timer);
        resolve({ code, stdout: stdoutBuf, stderr: stderrBuf });
      });
    });

  const waitForOutput = (pattern, timeoutMs = 15000) =>
    new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        reject(new Error(
          `npm ${binary}: pattern "${pattern}" not found in output within ${timeoutMs}ms.\n` +
          `stdout: ${stdoutBuf.slice(-500)}\nstderr: ${stderrBuf.slice(-500)}`,
        ));
      }, timeoutMs);
      const check = () => {
        if (stdoutBuf.includes(pattern) || stderrBuf.includes(pattern)) {
          clearTimeout(timer);
          resolve();
        } else {
          setTimeout(check, 50);
        }
      };
      check();
    });

  return {
    proc,
    get stdout() { return stdoutBuf; },
    get stderr() { return stderrBuf; },
    waitForExit,
    waitForOutput,
    kill: (signal = 'SIGTERM') => proc.kill(signal),
  };
}

/**
 * Sleep for the given number of milliseconds.
 */
export function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/**
 * Measure the execution time of an async function.
 * Returns { result, durationMs }.
 */
export async function measure(fn) {
  const start = performance.now();
  const result = await fn();
  const durationMs = performance.now() - start;
  return { result, durationMs };
}
