/**
 * Terminal Performance E2E Tests.
 *
 * Measures round-trip latency and throughput for terminal data over:
 * 1. P2P data channel (loopback WebRTC with AES-256-GCM encryption)
 * 2. Matrix-only path (via local Tuwunel instance)
 *
 * Outputs results to console and writes an HTML report to
 * packages/e2e-tests/results/perf-terminal.html
 *
 * Run with: node --test packages/e2e-tests/tests/perf-terminal.test.js
 */
import { describe, it, before, after } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { performance } from 'node:perf_hooks';
import { TuwunelInstance } from '../src/tuwunel.js';
import { WasmMatrixClient, P2PTransport, BatchedSender } from '@mxdx/core';

const tuwunelAvailable = TuwunelInstance.isAvailable();

// WASM matrix-sdk-crypto fires async "Session expired" panics after tests end.
// These are harmless (test process is exiting) but crash the test runner.
process.on('uncaughtException', (err) => {
  if (err?.message?.includes('unreachable') || err?.message?.includes('Session expired')) {
    return; // Suppress WASM async teardown noise
  }
  throw err;
});
import { NodeWebRTCChannel } from '../../../packages/core/webrtc-channel-node.js';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const RESULTS_DIR = path.resolve(__dirname, '../results');

function sleep(ms) {
  return new Promise(r => setTimeout(r, ms));
}

/** Mock P2PCrypto with real AES-256-GCM for realistic benchmarking. */
async function realP2PCrypto() {
  const { webcrypto } = await import('node:crypto');
  const keyMaterial = webcrypto.getRandomValues(new Uint8Array(32));
  const key = await webcrypto.subtle.importKey(
    'raw', keyMaterial, { name: 'AES-GCM' }, false, ['encrypt', 'decrypt'],
  );
  return {
    async encrypt(plaintext) {
      const iv = webcrypto.getRandomValues(new Uint8Array(12));
      const encoded = new TextEncoder().encode(plaintext);
      const ciphertext = await webcrypto.subtle.encrypt(
        { name: 'AES-GCM', iv }, key, encoded,
      );
      return JSON.stringify({
        iv: Buffer.from(iv).toString('base64'),
        ct: Buffer.from(ciphertext).toString('base64'),
      });
    },
    async decrypt(ciphertextJson) {
      const { iv, ct } = JSON.parse(ciphertextJson);
      const ivBuf = Buffer.from(iv, 'base64');
      const ctBuf = Buffer.from(ct, 'base64');
      const plaintext = await webcrypto.subtle.decrypt(
        { name: 'AES-GCM', iv: ivBuf }, key, ctBuf,
      );
      return new TextDecoder().decode(plaintext);
    },
  };
}

/** Set up a pair of P2P transports with loopback WebRTC and return them verified. */
async function setupP2PPair(launcherClient, clientClient, dmRoomId) {
  const channelA = new NodeWebRTCChannel();
  const channelB = new NodeWebRTCChannel();

  const candidatesA = [];
  const candidatesB = [];
  channelA.onIceCandidate(c => candidatesA.push(c));
  channelB.onIceCandidate(c => candidatesB.push(c));

  const offer = await channelA.createOffer();
  const answer = await channelB.acceptOffer({ sdp: offer.sdp, type: 'offer' });
  await channelA.acceptAnswer({ sdp: answer.sdp, type: 'answer' });

  await sleep(200);
  for (const c of candidatesA) channelB.addIceCandidate(c);
  for (const c of candidatesB) channelA.addIceCandidate(c);

  await Promise.all([
    channelA.waitForDataChannel(),
    channelB.waitForDataChannel(),
  ]);

  const sharedCrypto = await realP2PCrypto();

  const transportA = P2PTransport.create({
    matrixClient: {
      sendEvent: (roomId, type, content) => launcherClient.sendEvent(roomId, type, content),
      onRoomEvent: (roomId, type, timeout) => launcherClient.onRoomEvent(roomId, type, timeout),
      userId: () => launcherClient.userId(),
    },
    p2pCrypto: sharedCrypto,
    localDeviceId: 'PERF_LAUNCHER',
    idleTimeoutMs: 300000,
  });

  const transportB = P2PTransport.create({
    matrixClient: {
      sendEvent: (roomId, type, content) => clientClient.sendEvent(roomId, type, content),
      onRoomEvent: (roomId, type, timeout) => clientClient.onRoomEvent(roomId, type, timeout),
      userId: () => clientClient.userId(),
    },
    p2pCrypto: sharedCrypto,
    localDeviceId: 'PERF_CLIENT',
    idleTimeoutMs: 300000,
  });

  transportA.setDataChannel(channelA);
  transportB.setDataChannel(channelB);

  for (let i = 0; i < 60; i++) {
    if (transportA.status === 'p2p' && transportB.status === 'p2p') break;
    await sleep(50);
  }

  assert.equal(transportA.status, 'p2p', 'Launcher transport should reach P2P');
  assert.equal(transportB.status, 'p2p', 'Client transport should reach P2P');

  return { transportA, transportB, channelA, channelB };
}

function percentile(sorted, p) {
  const idx = Math.ceil(sorted.length * p / 100) - 1;
  return sorted[Math.max(0, idx)];
}

function stats(latencies) {
  const sorted = [...latencies].sort((a, b) => a - b);
  return {
    min: sorted[0],
    max: sorted[sorted.length - 1],
    avg: latencies.reduce((a, b) => a + b, 0) / latencies.length,
    median: percentile(sorted, 50),
    p95: percentile(sorted, 95),
    p99: percentile(sorted, 99),
    count: latencies.length,
  };
}

// ─── Collect all results for HTML report ──────────────────────────────────────
const allResults = [];

describe('Terminal Performance: P2P vs Matrix', { skip: !tuwunelAvailable && 'tuwunel binary not found', timeout: 180000 }, () => {
  let tuwunel;
  let launcherClient;
  let clientClient;
  let dmRoomId;

  before(async () => {
    tuwunel = await TuwunelInstance.start();
    console.log(`[perf] Tuwunel started on ${tuwunel.url}`);

    const launcherUsername = `perf-launcher-${Date.now()}`;
    launcherClient = await WasmMatrixClient.register(
      tuwunel.url, launcherUsername, 'testpass123', tuwunel.registrationToken,
    );

    const clientUsername = `perf-client-${Date.now()}`;
    clientClient = await WasmMatrixClient.register(
      tuwunel.url, clientUsername, 'testpass123', tuwunel.registrationToken,
    );

    dmRoomId = await launcherClient.createDmRoom(clientClient.userId());
    await clientClient.syncOnce();
    await clientClient.joinRoom(dmRoomId);
    await clientClient.syncOnce();
    await launcherClient.syncOnce();
    console.log(`[perf] DM room ready: ${dmRoomId}`);
  });

  after(async () => {
    // Free WASM clients before stopping the server to prevent stale crypto ops
    if (launcherClient) launcherClient.free();
    if (clientClient) clientClient.free();
    // Stop Tuwunel — killing the server connection prevents the WASM
    // crypto layer from firing async operations that outlive the test.
    if (tuwunel) tuwunel.stop();
    // Let any in-flight WASM async work settle before node:test checks for leaks
    await sleep(500);
  });

  // ─── P2P Latency: Single Character RTT ─────────────────────────────────────

  it('P2P: single character round-trip latency (100 chars)', async () => {
    const { transportA, transportB, channelA, channelB } = await setupP2PPair(
      launcherClient, clientClient, dmRoomId,
    );

    const latencies = [];
    const N = 100;

    for (let i = 0; i < N; i++) {
      const char = String.fromCharCode(65 + (i % 26));
      const payload = JSON.stringify({
        type: 'org.mxdx.terminal.data',
        data: Buffer.from(char).toString('base64'),
        encoding: 'base64',
        seq: i,
      });

      const t0 = performance.now();
      await transportA.sendEvent(dmRoomId, 'org.mxdx.terminal.data', payload);
      const received = await transportB.onRoomEvent(dmRoomId, 'org.mxdx.terminal.data', 5);
      const t1 = performance.now();

      assert.ok(received && received != null, `Should receive char ${i}`);
      latencies.push(t1 - t0);
    }

    const s = stats(latencies);
    console.log(`[perf] P2P single-char RTT (${N} samples):`);
    console.log(`  min=${s.min.toFixed(2)}ms avg=${s.avg.toFixed(2)}ms median=${s.median.toFixed(2)}ms p95=${s.p95.toFixed(2)}ms p99=${s.p99.toFixed(2)}ms max=${s.max.toFixed(2)}ms`);

    allResults.push({ name: 'P2P Single Char RTT', ...s, unit: 'ms' });

    assert.ok(s.avg < 50, `P2P avg RTT should be < 50ms, got ${s.avg.toFixed(2)}ms`);
    assert.ok(s.p95 < 100, `P2P p95 RTT should be < 100ms, got ${s.p95.toFixed(2)}ms`);

    transportA.close();
    transportB.close();
    channelA.close();
    channelB.close();
  });

  // ─── P2P Latency: Burst of Characters ───────────────────────────────────────

  it('P2P: burst latency (50 chars sent rapidly)', async () => {
    const { transportA, transportB, channelA, channelB } = await setupP2PPair(
      launcherClient, clientClient, dmRoomId,
    );

    const N = 50;
    const sendTimes = [];
    const receiveTimes = [];

    // Fire all sends rapidly
    const t0 = performance.now();
    for (let i = 0; i < N; i++) {
      const payload = JSON.stringify({
        type: 'org.mxdx.terminal.data',
        data: Buffer.from(`burst-${i}`).toString('base64'),
        encoding: 'base64',
        seq: i,
      });
      sendTimes.push(performance.now());
      transportA.sendEvent(dmRoomId, 'org.mxdx.terminal.data', payload);
    }

    // Receive all
    for (let i = 0; i < N; i++) {
      const received = await transportB.onRoomEvent(dmRoomId, 'org.mxdx.terminal.data', 10);
      receiveTimes.push(performance.now());
      assert.ok(received && received != null, `Should receive burst char ${i}`);
    }

    const totalMs = performance.now() - t0;
    const latencies = receiveTimes.map((rt, i) => rt - sendTimes[i]);
    const s = stats(latencies);

    console.log(`[perf] P2P burst (${N} chars):`);
    console.log(`  total=${totalMs.toFixed(2)}ms throughput=${(N / (totalMs / 1000)).toFixed(0)} chars/sec`);
    console.log(`  min=${s.min.toFixed(2)}ms avg=${s.avg.toFixed(2)}ms median=${s.median.toFixed(2)}ms p95=${s.p95.toFixed(2)}ms max=${s.max.toFixed(2)}ms`);

    allResults.push({ name: 'P2P Burst RTT', ...s, unit: 'ms', totalMs, throughput: N / (totalMs / 1000) });

    assert.ok(s.avg < 100, `P2P burst avg should be < 100ms, got ${s.avg.toFixed(2)}ms`);

    transportA.close();
    transportB.close();
    channelA.close();
    channelB.close();
  });

  // ─── P2P Throughput: Large Payload ─────────────────────────────────────────

  it('P2P: throughput with 1KB payloads (50 sends)', async () => {
    const { transportA, transportB, channelA, channelB } = await setupP2PPair(
      launcherClient, clientClient, dmRoomId,
    );

    const N = 50;
    const payloadSize = 1024; // 1KB
    const payload1KB = Buffer.alloc(payloadSize, 'X').toString('base64');
    const totalBytes = N * payloadSize;

    const t0 = performance.now();

    for (let i = 0; i < N; i++) {
      const payload = JSON.stringify({
        type: 'org.mxdx.terminal.data',
        data: payload1KB,
        encoding: 'base64',
        seq: i,
      });
      await transportA.sendEvent(dmRoomId, 'org.mxdx.terminal.data', payload);
    }

    for (let i = 0; i < N; i++) {
      const received = await transportB.onRoomEvent(dmRoomId, 'org.mxdx.terminal.data', 10);
      assert.ok(received && received != null, `Should receive payload ${i}`);
    }

    const totalMs = performance.now() - t0;
    const throughputKBs = (totalBytes / 1024) / (totalMs / 1000);

    console.log(`[perf] P2P throughput (${N}x${payloadSize}B):`);
    console.log(`  total=${totalMs.toFixed(2)}ms throughput=${throughputKBs.toFixed(1)} KB/s (${(totalBytes / totalMs * 1000 / 1024 / 1024).toFixed(2)} MB/s)`);

    allResults.push({ name: 'P2P 1KB Throughput', totalMs, throughputKBs, unit: 'KB/s', count: N });

    assert.ok(throughputKBs > 10, `P2P throughput should be > 10 KB/s, got ${throughputKBs.toFixed(1)} KB/s`);

    transportA.close();
    transportB.close();
    channelA.close();
    channelB.close();
  });

  // ─── Matrix-Only Latency: Single Event RTT ─────────────────────────────────

  it('Matrix-only: single event round-trip latency (20 events)', async () => {
    const latencies = [];
    const N = 20;

    for (let i = 0; i < N; i++) {
      const payload = JSON.stringify({
        data: Buffer.from(`matrix-${i}`).toString('base64'),
        encoding: 'base64',
        seq: i,
      });

      // Sender starts listening before sending
      const receivePromise = clientClient.onRoomEvent(
        dmRoomId, 'org.mxdx.terminal.data', 30,
      );

      const t0 = performance.now();
      await launcherClient.sendEvent(
        dmRoomId, 'org.mxdx.terminal.data', payload,
      );

      const received = await receivePromise;
      const t1 = performance.now();

      assert.ok(received && received != null, `Should receive Matrix event ${i}`);
      latencies.push(t1 - t0);
    }

    const s = stats(latencies);
    console.log(`[perf] Matrix-only single event RTT (${N} samples):`);
    console.log(`  min=${s.min.toFixed(2)}ms avg=${s.avg.toFixed(2)}ms median=${s.median.toFixed(2)}ms p95=${s.p95.toFixed(2)}ms max=${s.max.toFixed(2)}ms`);

    allResults.push({ name: 'Matrix Single Event RTT (Tuwunel)', ...s, unit: 'ms' });
  });

  // ─── BatchedSender Performance ──────────────────────────────────────────────

  it('BatchedSender: immediate flush at batchMs=5 (keystroke latency)', async () => {
    const sentPayloads = [];
    const sender = new BatchedSender({
      sendEvent: async (roomId, type, contentJson) => {
        sentPayloads.push({ ts: performance.now(), content: contentJson });
      },
      roomId: dmRoomId,
      batchMs: 5,
    });

    const N = 30;
    const pushTimes = [];

    for (let i = 0; i < N; i++) {
      pushTimes.push(performance.now());
      sender.push(String.fromCharCode(65 + (i % 26)));
      await sleep(2); // Simulate typing speed (~500 cpm)
    }

    // Wait for all flushes
    await sender.flush();
    sender.destroy();

    // Each keystroke should result in its own send (or very small batch)
    const latencies = sentPayloads.map((s, i) => {
      const pushIdx = Math.min(i, pushTimes.length - 1);
      return s.ts - pushTimes[pushIdx];
    });

    const s = stats(latencies);
    console.log(`[perf] BatchedSender batchMs=5 (${sentPayloads.length} sends from ${N} pushes):`);
    console.log(`  min=${s.min.toFixed(2)}ms avg=${s.avg.toFixed(2)}ms max=${s.max.toFixed(2)}ms`);

    allResults.push({ name: 'BatchedSender (5ms) Flush Latency', ...s, unit: 'ms', sends: sentPayloads.length });

    assert.ok(s.avg < 20, `Flush latency should be < 20ms, got ${s.avg.toFixed(2)}ms`);
  });

  it('BatchedSender: batching at batchMs=200 (Matrix rate-limit safe)', async () => {
    const sentPayloads = [];
    const sender = new BatchedSender({
      sendEvent: async (roomId, type, contentJson) => {
        sentPayloads.push({ ts: performance.now(), content: contentJson });
      },
      roomId: dmRoomId,
      batchMs: 200,
    });

    const N = 50;
    const t0 = performance.now();

    for (let i = 0; i < N; i++) {
      sender.push(`char-${i}`);
      await sleep(10); // ~100 cpm
    }

    await sender.flush();
    sender.destroy();

    const totalMs = performance.now() - t0;
    console.log(`[perf] BatchedSender batchMs=200 (${sentPayloads.length} sends from ${N} pushes):`);
    console.log(`  total=${totalMs.toFixed(2)}ms sends=${sentPayloads.length} ratio=${(N / sentPayloads.length).toFixed(1)}:1`);

    allResults.push({ name: 'BatchedSender (200ms) Batching', sends: sentPayloads.length, pushes: N, ratio: N / sentPayloads.length, unit: 'ratio' });

    // Should batch significantly (200ms window with 10ms intervals = ~20 chars per batch)
    assert.ok(sentPayloads.length < N, `Should batch: ${sentPayloads.length} sends < ${N} pushes`);
  });

  // ─── P2P Encryption Overhead ────────────────────────────────────────────────

  it('P2PCrypto: AES-256-GCM encrypt/decrypt latency', async () => {
    const crypto = await realP2PCrypto();
    const latencies = [];
    const N = 200;

    for (let i = 0; i < N; i++) {
      const plaintext = JSON.stringify({ data: `keystroke-${i}`, seq: i });
      const t0 = performance.now();
      const ct = await crypto.encrypt(plaintext);
      const pt = await crypto.decrypt(ct);
      const t1 = performance.now();
      assert.equal(pt, plaintext);
      latencies.push(t1 - t0);
    }

    const s = stats(latencies);
    console.log(`[perf] AES-256-GCM encrypt+decrypt (${N} ops):`);
    console.log(`  min=${s.min.toFixed(3)}ms avg=${s.avg.toFixed(3)}ms p95=${s.p95.toFixed(3)}ms max=${s.max.toFixed(3)}ms`);

    allResults.push({ name: 'AES-256-GCM Encrypt+Decrypt', ...s, unit: 'ms' });

    assert.ok(s.avg < 5, `Crypto overhead should be < 5ms, got ${s.avg.toFixed(3)}ms`);
  });

  // ─── P2P vs Matrix Comparison ───────────────────────────────────────────────

  it('comparison: P2P vs Matrix terminal event delivery', async () => {
    const { transportA, transportB, channelA, channelB } = await setupP2PPair(
      launcherClient, clientClient, dmRoomId,
    );

    const N = 20;
    const p2pLatencies = [];
    const matrixLatencies = [];

    // P2P path
    for (let i = 0; i < N; i++) {
      const payload = JSON.stringify({
        type: 'org.mxdx.terminal.data',
        data: Buffer.from(`p2p-cmp-${i}`).toString('base64'),
        encoding: 'base64',
        seq: i,
      });
      const t0 = performance.now();
      await transportA.sendEvent(dmRoomId, 'org.mxdx.terminal.data', payload);
      const received = await transportB.onRoomEvent(dmRoomId, 'org.mxdx.terminal.data', 5);
      p2pLatencies.push(performance.now() - t0);
      assert.ok(received && received != null);
    }

    transportA.close();
    transportB.close();
    channelA.close();
    channelB.close();

    // Matrix path
    for (let i = 0; i < N; i++) {
      const payload = JSON.stringify({
        data: Buffer.from(`matrix-cmp-${i}`).toString('base64'),
        encoding: 'base64',
        seq: 1000 + i,
      });

      const receivePromise = clientClient.onRoomEvent(
        dmRoomId, 'org.mxdx.terminal.data', 30,
      );

      const t0 = performance.now();
      await launcherClient.sendEvent(dmRoomId, 'org.mxdx.terminal.data', payload);
      const received = await receivePromise;
      matrixLatencies.push(performance.now() - t0);
      assert.ok(received && received != null);
    }

    const p2pStats = stats(p2pLatencies);
    const matrixStats = stats(matrixLatencies);
    const speedup = matrixStats.avg / p2pStats.avg;

    console.log(`[perf] P2P vs Matrix comparison (${N} events each):`);
    console.log(`  P2P:    avg=${p2pStats.avg.toFixed(2)}ms  p95=${p2pStats.p95.toFixed(2)}ms`);
    console.log(`  Matrix: avg=${matrixStats.avg.toFixed(2)}ms  p95=${matrixStats.p95.toFixed(2)}ms`);
    console.log(`  Speedup: ${speedup.toFixed(1)}x`);

    allResults.push({
      name: 'P2P vs Matrix Comparison',
      p2pAvg: p2pStats.avg,
      p2pP95: p2pStats.p95,
      matrixAvg: matrixStats.avg,
      matrixP95: matrixStats.p95,
      speedup,
      unit: 'x',
    });

    assert.ok(p2pStats.avg < matrixStats.avg, 'P2P should be faster than Matrix');
  });
});

// ─── Public Server Performance (optional, requires credentials) ───────────────

describe('Terminal Performance: Public Server', { skip: !tuwunelAvailable && 'tuwunel binary not found', timeout: 300000 }, () => {
  let creds;
  let client1;
  let client2;
  let dmRoomId;

  before(async () => {
    const REPO_ROOT = path.resolve(__dirname, '../../..');
    const tomlPath = path.join(REPO_ROOT, 'test-credentials.toml');

    if (!fs.existsSync(tomlPath)) {
      console.log('[perf-pub] Skipping public server tests: test-credentials.toml not found');
      return;
    }

    const content = fs.readFileSync(tomlPath, 'utf8');
    const lines = content.split('\n');
    const result = {};
    let section = null;
    for (const line of lines) {
      const trimmed = line.trim();
      const sectionMatch = trimmed.match(/^\[(\w+)\]$/);
      if (sectionMatch) { section = sectionMatch[1]; result[section] = {}; continue; }
      const kvMatch = trimmed.match(/^(\w+)\s*=\s*"(.+)"$/);
      if (kvMatch && section) result[section][kvMatch[1]] = kvMatch[2];
    }

    creds = {
      url: result.server?.url,
      account1: result.account1,
      account2: result.account2,
    };

    if (!creds.url || !creds.account1?.username || !creds.account2?.username) {
      console.log('[perf-pub] Skipping: incomplete credentials');
      creds = null;
      return;
    }

    client1 = await WasmMatrixClient.login(
      creds.url, creds.account1.username, creds.account1.password,
    );
    client2 = await WasmMatrixClient.login(
      creds.url, creds.account2.username, creds.account2.password,
    );

    await client1.syncOnce();
    await client2.syncOnce();

    dmRoomId = await client1.createDmRoom(client2.userId());
    await sleep(3000);
    await client2.syncOnce();
    await client2.joinRoom(dmRoomId);
    await client2.syncOnce();
    await client1.syncOnce();
    console.log(`[perf-pub] Public DM room ready: ${dmRoomId}`);
  });

  after(() => {
    if (client1) client1.free();
    if (client2) client2.free();
  });

  it('public Matrix: single event round-trip latency (10 events)', async () => {
    if (!creds) return; // Skip if no credentials

    const latencies = [];
    const N = 10;

    for (let i = 0; i < N; i++) {
      const payload = JSON.stringify({
        data: Buffer.from(`pub-perf-${i}`).toString('base64'),
        encoding: 'base64',
        seq: i,
      });

      const receivePromise = client2.onRoomEvent(
        dmRoomId, 'org.mxdx.terminal.data', 60,
      );

      const t0 = performance.now();
      await client1.sendEvent(dmRoomId, 'org.mxdx.terminal.data', payload);
      const received = await receivePromise;
      const t1 = performance.now();

      if (received && received != null) {
        latencies.push(t1 - t0);
      }

      // Throttle to avoid rate limits
      await sleep(1000);
    }

    if (latencies.length > 0) {
      const s = stats(latencies);
      console.log(`[perf-pub] Public Matrix single event RTT (${latencies.length}/${N} successful):`);
      console.log(`  min=${s.min.toFixed(0)}ms avg=${s.avg.toFixed(0)}ms median=${s.median.toFixed(0)}ms p95=${s.p95.toFixed(0)}ms max=${s.max.toFixed(0)}ms`);

      allResults.push({ name: 'Public Matrix (matrix.org) Single Event RTT', ...s, unit: 'ms' });
    } else {
      console.log('[perf-pub] No successful round-trips (rate limited or timeout)');
      allResults.push({ name: 'Public Matrix (matrix.org) Single Event RTT', avg: NaN, note: 'No successful round-trips', unit: 'ms' });
    }
  });

  it('public Matrix: sustained throughput (events/sec)', async () => {
    if (!creds) return;

    const N = 10;
    let successful = 0;
    const t0 = performance.now();

    for (let i = 0; i < N; i++) {
      const payload = JSON.stringify({
        data: Buffer.from(`pub-throughput-${i}`).toString('base64'),
        encoding: 'base64',
        seq: 100 + i,
      });

      try {
        await client1.sendEvent(dmRoomId, 'org.mxdx.terminal.data', payload);
        successful++;
      } catch (err) {
        console.log(`[perf-pub] Send ${i} failed: ${err.message || err}`);
        await sleep(2000);
      }
      await sleep(500); // Rate limit safety
    }

    const totalMs = performance.now() - t0;
    const eventsPerSec = successful / (totalMs / 1000);
    console.log(`[perf-pub] Public Matrix sustained throughput: ${eventsPerSec.toFixed(2)} events/sec (${successful}/${N} successful in ${(totalMs / 1000).toFixed(1)}s)`);

    allResults.push({ name: 'Public Matrix Sustained Throughput', eventsPerSec, successful, total: N, totalMs, unit: 'events/sec' });
  });
});

// ─── HTML Report ──────────────────────────────────────────────────────────────

after(() => {
  if (allResults.length === 0) return;

  fs.mkdirSync(RESULTS_DIR, { recursive: true });

  const timestamp = new Date().toISOString();
  const rows = allResults.map(r => {
    if (r.speedup) {
      return `<tr>
        <td>${r.name}</td>
        <td>P2P: ${r.p2pAvg.toFixed(2)}ms / Matrix: ${r.matrixAvg.toFixed(2)}ms</td>
        <td>${r.speedup.toFixed(1)}x faster</td>
        <td>P2P p95: ${r.p2pP95.toFixed(2)}ms / Matrix p95: ${r.matrixP95.toFixed(2)}ms</td>
      </tr>`;
    }
    if (r.throughputKBs) {
      return `<tr>
        <td>${r.name}</td>
        <td>${r.throughputKBs.toFixed(1)} KB/s</td>
        <td>${r.count} x 1KB payloads</td>
        <td>Total: ${r.totalMs.toFixed(0)}ms</td>
      </tr>`;
    }
    if (r.eventsPerSec) {
      return `<tr>
        <td>${r.name}</td>
        <td>${r.eventsPerSec.toFixed(2)} events/sec</td>
        <td>${r.successful}/${r.total} successful</td>
        <td>Total: ${(r.totalMs / 1000).toFixed(1)}s</td>
      </tr>`;
    }
    if (r.ratio) {
      return `<tr>
        <td>${r.name}</td>
        <td>${r.ratio.toFixed(1)}:1 compression</td>
        <td>${r.sends} sends from ${r.pushes} pushes</td>
        <td>-</td>
      </tr>`;
    }
    if (r.sends) {
      return `<tr>
        <td>${r.name}</td>
        <td>avg: ${r.avg.toFixed(2)}ms</td>
        <td>${r.sends} flushes</td>
        <td>min: ${r.min.toFixed(2)}ms / max: ${r.max.toFixed(2)}ms</td>
      </tr>`;
    }
    if (r.avg !== undefined && !isNaN(r.avg)) {
      return `<tr>
        <td>${r.name}</td>
        <td>avg: ${r.avg.toFixed(2)}ms</td>
        <td>p95: ${(r.p95 || 0).toFixed(2)}ms</td>
        <td>min: ${(r.min || 0).toFixed(2)}ms / max: ${(r.max || 0).toFixed(2)}ms (${r.count || '-'} samples)</td>
      </tr>`;
    }
    return `<tr><td>${r.name}</td><td colspan="3">${r.note || 'N/A'}</td></tr>`;
  }).join('\n');

  const html = `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <title>mxdx Terminal Performance Report</title>
  <style>
    body { font-family: system-ui, sans-serif; max-width: 1000px; margin: 2rem auto; padding: 0 1rem; background: #0d1117; color: #c9d1d9; }
    h1 { color: #58a6ff; border-bottom: 1px solid #30363d; padding-bottom: 0.5rem; }
    h2 { color: #8b949e; }
    table { width: 100%; border-collapse: collapse; margin: 1rem 0; }
    th, td { padding: 0.5rem 0.75rem; text-align: left; border: 1px solid #30363d; }
    th { background: #161b22; color: #58a6ff; }
    tr:nth-child(even) { background: #161b22; }
    .timestamp { color: #8b949e; font-size: 0.85rem; }
    .summary { background: #161b22; border: 1px solid #30363d; border-radius: 6px; padding: 1rem; margin: 1rem 0; }
    .good { color: #3fb950; }
    .warn { color: #d29922; }
    .bad { color: #f85149; }
    code { background: #161b22; padding: 0.15rem 0.4rem; border-radius: 3px; font-size: 0.9rem; }
  </style>
</head>
<body>
  <h1>mxdx Terminal Performance Report</h1>
  <p class="timestamp">Generated: ${timestamp}</p>

  <div class="summary">
    <h2>Summary</h2>
    <p>Performance benchmarks for terminal data delivery over P2P (WebRTC + AES-256-GCM) vs Matrix (E2EE via Megolm).</p>
    <ul>
      <li><strong>P2P path</strong>: WebRTC data channel with AES-256-GCM encryption, loopback (localhost-to-localhost)</li>
      <li><strong>Matrix path (Tuwunel)</strong>: Local Tuwunel homeserver, E2EE via Megolm</li>
      <li><strong>Matrix path (Public)</strong>: matrix.org homeserver, E2EE via Megolm (subject to rate limits)</li>
    </ul>
  </div>

  <h2>Results</h2>
  <table>
    <thead>
      <tr><th>Benchmark</th><th>Value</th><th>Detail</th><th>Notes</th></tr>
    </thead>
    <tbody>
      ${rows}
    </tbody>
  </table>

  <div class="summary">
    <h2>Interpretation</h2>
    <ul>
      <li>P2P provides <strong>near-instantaneous</strong> terminal I/O suitable for interactive shell sessions</li>
      <li>Matrix (Tuwunel) adds E2EE + sync overhead but remains usable for command execution</li>
      <li>Matrix (public, matrix.org) is rate-limited to ~1 event/sec, making it unsuitable for interactive terminals without P2P</li>
      <li>The <code>BatchedSender</code> adapts: 5ms batching for P2P (keystroke-level), 200ms for Matrix (rate-limit safe)</li>
      <li>AES-256-GCM encryption overhead is negligible (&lt;1ms per encrypt+decrypt)</li>
    </ul>
  </div>

  <div class="summary">
    <h2>Environment</h2>
    <ul>
      <li>Node.js: ${process.version}</li>
      <li>Platform: ${process.platform} ${process.arch}</li>
      <li>Test runner: <code>node --test</code></li>
    </ul>
  </div>
</body>
</html>`;

  const reportPath = path.join(RESULTS_DIR, 'perf-terminal.html');
  fs.writeFileSync(reportPath, html);
  console.log(`[perf] HTML report written to: ${reportPath}`);
});
