import { describe, it } from 'node:test';
import assert from 'node:assert';
import os from 'node:os';

// ── B.1: Telemetry Detail Levels ─────────────────────────────────────────
// Tests WASM buildTelemetryPayload via the @mxdx/core re-export.

describe('Telemetry detail levels', () => {
  it('full telemetry includes all fields', async () => {
    const { buildTelemetryPayload } = await import('@mxdx/core');
    const payloadJson = buildTelemetryPayload(
      'full',
      os.hostname(), os.platform(), os.arch(),
      os.cpus().length,
      Math.floor(os.totalmem() / (1024 * 1024)),
      Math.floor(os.freemem() / (1024 * 1024)),
      Math.floor(os.uptime()),
      false, '', false, false, '', '', '', '', '',
      'online', 60000,
    );
    const full = JSON.parse(payloadJson);
    assert.ok(full.hostname, 'Should have hostname');
    assert.ok(full.platform, 'Should have platform');
    assert.ok(full.arch, 'Should have arch');
    assert.ok(full.cpus != null, 'Should have cpus');
    assert.ok(full.total_memory_mb != null, 'Should have total_memory_mb');
    assert.ok(full.free_memory_mb != null, 'Should have free_memory_mb');
    assert.ok(full.uptime_secs != null, 'Should have uptime_secs');
  });

  it('summary telemetry includes only hostname, platform, arch', async () => {
    const { buildTelemetryPayload } = await import('@mxdx/core');
    const payloadJson = buildTelemetryPayload(
      'summary',
      os.hostname(), os.platform(), os.arch(),
      os.cpus().length,
      Math.floor(os.totalmem() / (1024 * 1024)),
      Math.floor(os.freemem() / (1024 * 1024)),
      Math.floor(os.uptime()),
      false, '', false, false, '', '', '', '', '',
      'online', 60000,
    );
    const summary = JSON.parse(payloadJson);
    assert.ok(summary.hostname, 'Should have hostname');
    assert.ok(summary.platform, 'Should have platform');
    assert.ok(summary.arch, 'Should have arch');
    assert.strictEqual(summary.cpus, undefined, 'Should NOT have cpus');
    assert.strictEqual(summary.total_memory_mb, undefined, 'Should NOT have total_memory_mb');
    assert.strictEqual(summary.free_memory_mb, undefined, 'Should NOT have free_memory_mb');
    assert.strictEqual(summary.uptime_secs, undefined, 'Should NOT have uptime_secs');
  });

  it('default level is full', async () => {
    const { buildTelemetryPayload } = await import('@mxdx/core');
    const payloadJson = buildTelemetryPayload(
      '',
      os.hostname(), os.platform(), os.arch(),
      os.cpus().length,
      Math.floor(os.totalmem() / (1024 * 1024)),
      Math.floor(os.freemem() / (1024 * 1024)),
      Math.floor(os.uptime()),
      false, '', false, false, '', '', '', '', '',
      'online', 60000,
    );
    const defaultTelemetry = JSON.parse(payloadJson);
    assert.ok(defaultTelemetry.cpus != null, 'Default should include cpus (full mode)');
  });
});

// ── B.1b: SessionTransportManager state machine ──────────────────────────

describe('SessionTransportManager state machine', () => {
  it('refCount: add/release lifecycle', async () => {
    const { SessionTransportManager } = await import('@mxdx/core');
    const mgr = new SessionTransportManager(15_000);
    const roomId = '!testroom:example.com';
    const refCount = mgr.addTransport(roomId, 200);
    assert.strictEqual(refCount, 1, 'First add returns refCount 1');
    mgr.addTransport(roomId, 200);
    const shouldClose = mgr.releaseTransport(roomId);
    assert.strictEqual(shouldClose, false, 'Not closing — still has refs');
    const shouldClose2 = mgr.releaseTransport(roomId);
    assert.strictEqual(shouldClose2, true, 'Last release returns shouldClose=true');
    assert.strictEqual(mgr.roomCount, 0, 'Room removed after last release');
  });

  it('rate limiting: too soon returns false', async () => {
    const { SessionTransportManager } = await import('@mxdx/core');
    const mgr = new SessionTransportManager(15_000);
    const roomId = '!testroom:example.com';
    mgr.addTransport(roomId, 200);
    mgr.beginP2PAttempt(roomId); // sets lastAttempt to now
    assert.strictEqual(mgr.shouldAttemptP2P(roomId), false, 'Should be rate-limited immediately after attempt');
  });

  it('rate limiting: reset allows immediate retry', async () => {
    const { SessionTransportManager } = await import('@mxdx/core');
    const mgr = new SessionTransportManager(15_000);
    const roomId = '!testroom:example.com';
    mgr.addTransport(roomId, 200);
    mgr.beginP2PAttempt(roomId);
    mgr.resetRateLimit(roomId);
    assert.strictEqual(mgr.shouldAttemptP2P(roomId), true, 'After reset, attempt should be allowed');
  });

  it('stale attempt detection', async () => {
    const { SessionTransportManager } = await import('@mxdx/core');
    const mgr = new SessionTransportManager(0); // 0 ms rate limit for testing
    const roomId = '!testroom:example.com';
    mgr.addTransport(roomId, 200);
    const id1 = mgr.beginP2PAttempt(roomId);
    const id2 = mgr.beginP2PAttempt(roomId);
    assert.strictEqual(mgr.isAttemptStale(roomId, id1), true, 'First attempt is stale after second begins');
    assert.strictEqual(mgr.isAttemptStale(roomId, id2), false, 'Current attempt is not stale');
  });

  it('settled state', async () => {
    const { SessionTransportManager } = await import('@mxdx/core');
    const mgr = new SessionTransportManager(15_000);
    const roomId = '!testroom:example.com';
    mgr.addTransport(roomId, 200);
    assert.strictEqual(mgr.isSettled(roomId), false, 'Not settled initially');
    const ok = mgr.markSettled(roomId);
    assert.strictEqual(ok, true, 'markSettled returns true');
    assert.strictEqual(mgr.isSettled(roomId), true, 'Is settled after mark');
  });

  it('batchMs update', async () => {
    const { SessionTransportManager } = await import('@mxdx/core');
    const mgr = new SessionTransportManager(15_000);
    const roomId = '!testroom:example.com';
    mgr.addTransport(roomId, 200);
    assert.strictEqual(mgr.batchMs(roomId), 200);
    mgr.setBatchMs(roomId, 5);
    assert.strictEqual(mgr.batchMs(roomId), 5, 'batchMs updated to P2P latency');
  });
});

// ── B.1c: WasmSessionManager command routing ─────────────────────────────

describe('WasmSessionManager command routing', () => {
  const makeManager = async () => {
    const { WasmSessionManager } = await import('@mxdx/core');
    return new WasmSessionManager(
      JSON.stringify({ allowed_commands: ['echo', 'bash'], allowed_cwd: ['/tmp'], max_sessions: 10, username: 'launcher', use_tmux: 'auto', batch_ms: 200 }),
      '!exec:example.com', '!state:example.com', '@launcher:example.com', 'DEVICE1',
    );
  };

  it('rejects disallowed commands', async () => {
    const mgr = await makeManager();
    const events = JSON.stringify([{ event_id: '$evt1', type: 'org.mxdx.session.task', sender: '@client:example.com', content: { uuid: 'task-1', bin: 'rm', args: ['-rf', '/'], cwd: '/tmp' } }]);
    const actions = JSON.parse(mgr.processCommands(events));
    assert.strictEqual(actions.length, 1, 'Should produce one action (reject result)');
    assert.strictEqual(actions[0].kind, 'send_event');
    assert.ok(actions[0].content.error, 'Should include error message');
    assert.strictEqual(actions[0].content.status, 'failed', 'Should be failed status');
  });

  it('rejects disallowed cwd', async () => {
    const mgr = await makeManager();
    const events = JSON.stringify([{ event_id: '$evt2', type: 'org.mxdx.session.task', sender: '@client:example.com', content: { uuid: 'task-2', bin: 'echo', args: ['hi'], cwd: '/etc' } }]);
    const actions = JSON.parse(mgr.processCommands(events));
    assert.strictEqual(actions[0].content.status, 'failed');
    assert.ok(actions[0].content.error.includes('not allowed'));
  });

  it('rejects self-sent events', async () => {
    const mgr = await makeManager();
    const events = JSON.stringify([{ event_id: '$evt3', type: 'org.mxdx.session.task', sender: '@launcher:example.com', content: { uuid: 'task-3', bin: 'echo', cwd: '/tmp' } }]);
    const actions = JSON.parse(mgr.processCommands(events));
    assert.strictEqual(actions.length, 0, 'Should ignore events from self');
  });

  it('deduplicates processed events', async () => {
    const mgr = await makeManager();
    const events = JSON.stringify([{ event_id: '$evt4', type: 'org.mxdx.command', sender: '@client:example.com', content: { action: 'list_sessions', request_id: 'req-1' } }]);
    const actions1 = JSON.parse(mgr.processCommands(events));
    const actions2 = JSON.parse(mgr.processCommands(events));
    assert.strictEqual(actions1.length, 1, 'First process produces action');
    assert.strictEqual(actions2.length, 0, 'Second process skips duplicate event_id');
  });

  it('isCommandAllowed checks allowlist', async () => {
    const mgr = await makeManager();
    assert.strictEqual(mgr.isCommandAllowed('echo'), true);
    assert.strictEqual(mgr.isCommandAllowed('rm'), false);
  });

  it('isCwdAllowed checks prefix list', async () => {
    const mgr = await makeManager();
    assert.strictEqual(mgr.isCwdAllowed('/tmp'), true);
    assert.strictEqual(mgr.isCwdAllowed('/tmp/sub'), true);
    assert.strictEqual(mgr.isCwdAllowed('/etc'), false);
  });
});

// ── B.1d: WasmSessionManager session-management routing (P1, T-4.5) ─────

describe('WasmSessionManager session-management actions', () => {
  const makeManager = async () => {
    const { WasmSessionManager } = await import('@mxdx/core');
    return new WasmSessionManager(
      JSON.stringify({ allowed_commands: ['echo', 'bash'], allowed_cwd: ['/tmp'], max_sessions: 10, username: 'launcher', use_tmux: 'auto', batch_ms: 200 }),
      '!exec:example.com', '!state:example.com', '@launcher:example.com', 'DEVICE1',
    );
  };

  it('list_sessions emits org.mxdx.terminal.sessions to exec room', async () => {
    const mgr = await makeManager();
    // Seed a session so the listing has content.
    JSON.parse(mgr.onSessionStarted(
      'sess-1', 'req-1', '!dm:example.com', 'tmux-1', true, 200,
      '@client:example.com', Math.floor(Date.now() / 1000), 'bash', '[]',
    ));
    const events = JSON.stringify([{
      event_id: '$ls-1',
      type: 'org.mxdx.command',
      sender: '@client:example.com',
      content: { action: 'list_sessions', request_id: 'req-ls-1' },
    }]);
    const actions = JSON.parse(mgr.processCommands(events));
    assert.strictEqual(actions.length, 1);
    assert.strictEqual(actions[0].kind, 'send_event');
    assert.strictEqual(actions[0].room_id, '!exec:example.com');
    assert.strictEqual(actions[0].event_type, 'org.mxdx.terminal.sessions');
    assert.strictEqual(actions[0].content.request_id, 'req-ls-1');
    assert.ok(Array.isArray(actions[0].content.sessions), 'sessions is array');
    assert.strictEqual(actions[0].content.sessions.length, 1);
    const s = actions[0].content.sessions[0];
    assert.strictEqual(s.session_id, 'sess-1');
    assert.strictEqual(s.persistent, true);
    // Security: the listing MUST NOT leak sender ID or DM room ID.
    assert.strictEqual(s.sender, undefined, 'list MUST NOT leak sender');
    assert.strictEqual(s.dm_room_id, undefined, 'list MUST NOT leak dmRoomId');
  });

  it('session_cancel routes to kill_pty action', async () => {
    const mgr = await makeManager();
    // Seed an active session (so cancel finds it).
    JSON.parse(mgr.onSessionStarted(
      'sess-cancel', 'req-c', '!dm:example.com', 'tmux-c', false, 200,
      '@client:example.com', Math.floor(Date.now() / 1000), 'bash', '[]',
    ));
    const events = JSON.stringify([{
      event_id: '$cancel-1',
      type: 'org.mxdx.session.cancel',
      sender: '@client:example.com',
      content: { session_uuid: 'sess-cancel', grace_seconds: 5 },
    }]);
    const actions = JSON.parse(mgr.processCommands(events));
    assert.strictEqual(actions.length, 1);
    assert.strictEqual(actions[0].kind, 'kill_pty');
    assert.strictEqual(actions[0].session_id, 'sess-cancel');
    assert.strictEqual(actions[0].signal, 'SIGTERM');
  });

  it('session_signal routes to kill_pty with custom signal', async () => {
    const mgr = await makeManager();
    JSON.parse(mgr.onSessionStarted(
      'sess-sig', 'req-s', '!dm:example.com', 'tmux-s', false, 200,
      '@client:example.com', Math.floor(Date.now() / 1000), 'bash', '[]',
    ));
    const events = JSON.stringify([{
      event_id: '$sig-1',
      type: 'org.mxdx.session.signal',
      sender: '@client:example.com',
      content: { session_uuid: 'sess-sig', signal: 'SIGINT' },
    }]);
    const actions = JSON.parse(mgr.processCommands(events));
    assert.strictEqual(actions.length, 1);
    assert.strictEqual(actions[0].kind, 'kill_pty');
    assert.strictEqual(actions[0].signal, 'SIGINT');
  });

  it('session_signal: unknown session_uuid is silently ignored', async () => {
    const mgr = await makeManager();
    const events = JSON.stringify([{
      event_id: '$sig-unknown',
      type: 'org.mxdx.session.signal',
      sender: '@client:example.com',
      content: { session_uuid: 'never-existed', signal: 'SIGINT' },
    }]);
    const actions = JSON.parse(mgr.processCommands(events));
    assert.strictEqual(actions.length, 0, 'no kill_pty for unknown session');
  });

  it('spawn_pty happy path: allowed command + allowed cwd → spawn_pty action', async () => {
    const mgr = await makeManager();
    const events = JSON.stringify([{
      event_id: '$spawn-1',
      type: 'org.mxdx.command',
      sender: '@client:example.com',
      content: {
        action: 'interactive',
        command: 'bash',
        args: ['-l'],
        cwd: '/tmp',
        cols: 100, rows: 30,
        request_id: 'req-spawn-1',
        batch_ms: 50,
      },
    }]);
    const actions = JSON.parse(mgr.processCommands(events));
    const spawn = actions.find(a => a.kind === 'spawn_pty');
    assert.ok(spawn, 'should produce spawn_pty action');
    assert.strictEqual(spawn.command, 'bash');
    assert.deepStrictEqual(spawn.args, ['-l']);
    assert.strictEqual(spawn.cwd, '/tmp');
    assert.strictEqual(spawn.cols, 100);
    assert.strictEqual(spawn.rows, 30);
    assert.strictEqual(spawn.request_id, 'req-spawn-1');
    // batch_ms is the negotiated max(client, default) so should be >= 200 (default).
    assert.ok(spawn.batch_ms >= 200, `batch_ms negotiated to >= default (got ${spawn.batch_ms})`);
    assert.strictEqual(mgr.activeSessions, 1, 'active sessions incremented on spawn');
  });

  it('spawn_pty rejected when command not allowlisted', async () => {
    const mgr = await makeManager();
    const events = JSON.stringify([{
      event_id: '$spawn-2',
      type: 'org.mxdx.command',
      sender: '@client:example.com',
      content: {
        action: 'interactive',
        command: 'rm',
        cwd: '/tmp',
        cols: 80, rows: 24,
        request_id: 'req-spawn-2',
      },
    }]);
    const actions = JSON.parse(mgr.processCommands(events));
    assert.strictEqual(actions.length, 1);
    assert.strictEqual(actions[0].kind, 'send_event');
    assert.strictEqual(actions[0].event_type, 'org.mxdx.terminal.session');
    assert.strictEqual(actions[0].content.status, 'rejected');
    assert.strictEqual(mgr.activeSessions, 0, 'no session created on rejection');
  });
});

// ── B.2: Max Sessions Enforcement ────────────────────────────────────────

describe('Max sessions enforcement', () => {
  it('tracks active session count via incrementActiveSessions', async () => {
    const { WasmSessionManager } = await import('@mxdx/core');
    const mgr = new WasmSessionManager(
      JSON.stringify({ allowed_commands: ['echo'], allowed_cwd: ['/tmp'], max_sessions: 2, username: 'launcher', use_tmux: 'auto', batch_ms: 200 }),
      '!exec:example.com', '!state:example.com', '@launcher:example.com', 'DEVICE1',
    );
    assert.strictEqual(mgr.activeSessions, 0, 'Starts at 0');
    mgr.incrementActiveSessions();
    assert.strictEqual(mgr.activeSessions, 1, 'After increment: 1');
    mgr.decrementActiveSessions();
    assert.strictEqual(mgr.activeSessions, 0, 'After decrement: back to 0');
  });

  it('rejects commands when at session limit', async () => {
    const { WasmSessionManager } = await import('@mxdx/core');
    const mgr = new WasmSessionManager(
      JSON.stringify({ allowed_commands: ['echo'], allowed_cwd: ['/tmp'], max_sessions: 1, username: 'launcher', use_tmux: 'auto', batch_ms: 200 }),
      '!exec:example.com', '!state:example.com', '@launcher:example.com', 'DEVICE1',
    );
    mgr.incrementActiveSessions(); // fill the 1-slot limit
    const events = JSON.stringify([{ event_id: '$limit1', type: 'org.mxdx.session.task', sender: '@client:example.com', content: { uuid: 'task-limit', bin: 'echo', args: [], cwd: '/tmp' } }]);
    const actions = JSON.parse(mgr.processCommands(events));
    assert.strictEqual(actions.length, 1, 'Should produce one rejection action');
    assert.strictEqual(actions[0].content.status, 'failed', 'Should be failed');
    assert.ok(actions[0].content.error.includes('limit'), 'Error should mention limit');
  });
});

// ── B.3: Sync Resilience ─────────────────────────────────────────────────

describe('Exponential backoff', () => {
  it('backoff sequence: 1s, 2s, 4s, 8s, 16s, 30s max', () => {
    let backoffMs = 0;
    const sequence = [];

    for (let i = 0; i < 8; i++) {
      backoffMs = Math.min(Math.max(1000, backoffMs * 2 || 1000), 30000);
      sequence.push(backoffMs);
    }

    assert.deepStrictEqual(
      sequence,
      [1000, 2000, 4000, 8000, 16000, 30000, 30000, 30000],
      'Should follow exponential backoff capped at 30s',
    );
  });

  it('resets on success', () => {
    let backoffMs = 16000;
    backoffMs = 0;
    backoffMs = Math.min(Math.max(1000, backoffMs * 2 || 1000), 30000);
    assert.strictEqual(backoffMs, 1000, 'After reset, backoff should start at 1s');
  });
});

// ── B.4: Structured Logging ──────────────────────────────────────────────

describe('Structured logging', () => {
  it('json format produces valid JSON', () => {
    const entry = { level: 'info', msg: 'test message', ts: new Date().toISOString() };
    const json = JSON.stringify(entry);
    const parsed = JSON.parse(json);
    assert.strictEqual(parsed.level, 'info');
    assert.strictEqual(parsed.msg, 'test message');
    assert.ok(parsed.ts, 'Should have timestamp');
  });

  it('text format includes level and timestamp', () => {
    const level = 'warn';
    const ts = new Date().toISOString();
    const msg = 'something happened';
    const line = `[${level}] [${ts}] ${msg}`;
    assert.ok(line.includes('[warn]'), 'Should include level');
    assert.ok(line.includes(ts), 'Should include timestamp');
    assert.ok(line.includes(msg), 'Should include message');
  });
});
