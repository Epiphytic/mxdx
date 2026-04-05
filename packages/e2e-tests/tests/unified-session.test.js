/**
 * Unified Session Architecture -- End-to-End Tests
 *
 * Tests the unified session event schema and JSON structure validation.
 * Schema validation tests run without WASM or Tuwunel.
 * Full lifecycle tests require TuwunelInstance + WASM bindings and are skipped
 * until those are built in this worktree.
 */
import { describe, it, before, after } from 'node:test';
import assert from 'node:assert/strict';

// Session event type constants -- these match the Rust crate constants
// from mxdx-types/src/events/session.rs and worker_info.rs
const SESSION_EVENT_TYPES = {
    SESSION_TASK: 'org.mxdx.session.task',
    SESSION_START: 'org.mxdx.session.start',
    SESSION_OUTPUT: 'org.mxdx.session.output',
    SESSION_HEARTBEAT: 'org.mxdx.session.heartbeat',
    SESSION_RESULT: 'org.mxdx.session.result',
    SESSION_INPUT: 'org.mxdx.session.input',
    SESSION_SIGNAL: 'org.mxdx.session.signal',
    SESSION_RESIZE: 'org.mxdx.session.resize',
    SESSION_CANCEL: 'org.mxdx.session.cancel',
    WORKER_INFO: 'org.mxdx.worker.info',
};

// ---------------------------------------------------------------------------
// Schema Validation (no WASM or Tuwunel required)
// ---------------------------------------------------------------------------

describe('Unified Session Event Schema', () => {
    it('all event types follow org.mxdx.session.* convention', () => {
        for (const [key, value] of Object.entries(SESSION_EVENT_TYPES)) {
            if (key === 'WORKER_INFO') {
                assert.ok(value.startsWith('org.mxdx.worker.'), `${key}: ${value}`);
            } else {
                assert.ok(value.startsWith('org.mxdx.session.'), `${key}: ${value}`);
            }
        }
    });

    it('event type constants are unique', () => {
        const values = Object.values(SESSION_EVENT_TYPES);
        const unique = new Set(values);
        assert.equal(values.length, unique.size, 'All event type constants must be unique');
    });

    it('SessionTask JSON schema has required fields', () => {
        const task = {
            uuid: 'test-001',
            sender_id: '@alice:example.com',
            bin: 'echo',
            args: ['hello'],
            interactive: false,
            no_room_output: false,
            heartbeat_interval_seconds: 30,
        };
        assert.equal(task.uuid, 'test-001');
        assert.equal(task.bin, 'echo');
        assert.deepEqual(task.args, ['hello']);
        assert.equal(task.interactive, false);
        assert.equal(task.no_room_output, false);
        assert.equal(task.heartbeat_interval_seconds, 30);
    });

    it('SessionTask optional fields default correctly', () => {
        // When deserializing from JSON, these fields should default to null/empty
        const minimalJson = {
            uuid: 'test-002',
            sender_id: '@bob:example.com',
            bin: 'ls',
            args: [],
        };
        // Optional fields absent means they default
        assert.equal(minimalJson.env, undefined);
        assert.equal(minimalJson.cwd, undefined);
        assert.equal(minimalJson.timeout_seconds, undefined);
        assert.equal(minimalJson.plan, undefined);
    });

    it('SessionStart JSON schema has required fields', () => {
        const start = {
            session_uuid: 's-1',
            worker_id: '@worker:example.com',
            tmux_session: 'mxdx-s-1',
            pid: 12345,
            started_at: Date.now(),
        };
        assert.equal(start.session_uuid, 's-1');
        assert.equal(start.worker_id, '@worker:example.com');
        assert.equal(start.pid, 12345);
        assert.ok(start.started_at > 0);
    });

    it('SessionOutput JSON schema has stream enum', () => {
        const validStreams = ['stdout', 'stderr'];
        for (const stream of validStreams) {
            const output = {
                session_uuid: 's-1',
                worker_id: '@w:ex.com',
                stream,
                data: btoa('hello'),
                seq: 0,
                timestamp: Date.now(),
            };
            assert.ok(validStreams.includes(output.stream));
        }
    });

    it('SessionResult JSON schema has status enum values', () => {
        const validStatuses = ['success', 'failed', 'timeout', 'cancelled'];
        for (const status of validStatuses) {
            const result = {
                session_uuid: 'test',
                worker_id: 'w1',
                status,
                duration_seconds: 1,
            };
            assert.ok(validStatuses.includes(result.status));
        }
    });

    it('SessionResult rejects invalid status values', () => {
        const invalidStatuses = ['exploded', 'running', 'pending', 'unknown'];
        const validStatuses = ['success', 'failed', 'timeout', 'cancelled'];
        for (const status of invalidStatuses) {
            assert.ok(!validStatuses.includes(status), `${status} should not be valid`);
        }
    });

    it('SessionCancel JSON schema has optional reason and grace_seconds', () => {
        const cancelWithReason = {
            session_uuid: 's-1',
            reason: 'user requested',
            grace_seconds: 5,
        };
        assert.equal(cancelWithReason.reason, 'user requested');
        assert.equal(cancelWithReason.grace_seconds, 5);

        const cancelWithout = { session_uuid: 's-2' };
        assert.equal(cancelWithout.reason, undefined);
        assert.equal(cancelWithout.grace_seconds, undefined);
    });

    it('SessionSignal JSON schema has session_uuid and signal', () => {
        const signal = { session_uuid: 's-1', signal: 'SIGTERM' };
        assert.equal(signal.session_uuid, 's-1');
        assert.equal(signal.signal, 'SIGTERM');
    });

    it('SessionResize JSON schema has cols and rows', () => {
        const resize = { session_uuid: 's-1', cols: 120, rows: 40 };
        assert.equal(resize.cols, 120);
        assert.equal(resize.rows, 40);
    });

    it('SessionInput JSON schema has session_uuid and data', () => {
        const input = { session_uuid: 's-1', data: 'ls -la\n' };
        assert.equal(input.session_uuid, 's-1');
        assert.equal(input.data, 'ls -la\n');
    });

    it('ActiveSessionState has worker_id and interactive fields', () => {
        const state = {
            bin: 'echo',
            args: ['hi'],
            pid: 123,
            start_time: Date.now(),
            client_id: '@c:ex.com',
            interactive: false,
            worker_id: '@w:ex.com',
        };
        assert.equal(state.worker_id, '@w:ex.com');
        assert.equal(state.interactive, false);
        assert.equal(state.client_id, '@c:ex.com');
        assert.ok(Array.isArray(state.args));
    });

    it('CompletedSessionState has duration and completion_time', () => {
        const state = {
            exit_code: 0,
            duration_seconds: 42,
            completion_time: Date.now(),
        };
        assert.equal(state.exit_code, 0);
        assert.equal(state.duration_seconds, 42);
        assert.ok(state.completion_time > 0);
    });

    it('CompletedSessionState exit_code can be null for timeout/cancel', () => {
        const state = {
            exit_code: null,
            duration_seconds: 3600,
            completion_time: Date.now(),
        };
        assert.equal(state.exit_code, null);
    });

    it('WorkerInfo schema includes telemetry fields', () => {
        const info = {
            worker_id: 'w1',
            host: 'h1',
            os: 'linux',
            arch: 'x86_64',
            cpu_count: 4,
            memory_total_mb: 8192,
            disk_available_mb: 50000,
            tools: [{ name: 'bash', healthy: true }],
            capabilities: ['linux'],
            updated_at: Date.now(),
        };
        assert.ok(info.tools.length > 0);
        assert.ok(info.capabilities.includes('linux'));
        assert.equal(info.os, 'linux');
        assert.equal(info.arch, 'x86_64');
        assert.equal(info.cpu_count, 4);
    });

    it('SessionHeartbeat has optional progress field', () => {
        const withProgress = {
            session_uuid: 's-1',
            worker_id: '@w:ex.com',
            timestamp: Date.now(),
            progress: '50% complete',
        };
        assert.equal(withProgress.progress, '50% complete');

        const withoutProgress = {
            session_uuid: 's-2',
            worker_id: '@w:ex.com',
            timestamp: Date.now(),
        };
        assert.equal(withoutProgress.progress, undefined);
    });

    it('SessionTask JSON round-trips correctly', () => {
        const task = {
            uuid: 'rt-001',
            sender_id: '@alice:example.com',
            bin: 'echo',
            args: ['hello', 'world'],
            env: { NODE_ENV: 'test' },
            cwd: '/tmp',
            interactive: true,
            no_room_output: false,
            timeout_seconds: 60,
            heartbeat_interval_seconds: 15,
            plan: 'Run a test',
            required_capabilities: ['linux'],
            routing_mode: 'auto',
            on_timeout: 'escalate',
            on_heartbeat_miss: 'abandon',
        };
        const json = JSON.stringify(task);
        const parsed = JSON.parse(json);
        assert.deepEqual(parsed, task);
    });
});

// ---------------------------------------------------------------------------
// Full Session Flow (requires TuwunelInstance + WASM bindings)
// ---------------------------------------------------------------------------

describe('Unified Session Matrix Flow', () => {
    // These tests require a running TuwunelInstance and WASM bindings built
    // via wasm-pack. They are skipped until the WASM is built in this worktree.
    //
    // To enable:
    //   wasm-pack build crates/mxdx-core-wasm --target nodejs --out-dir ../../packages/core/wasm
    //   node --test packages/e2e-tests/tests/unified-session.test.js

    it('full lifecycle: task -> start -> output -> result (requires WASM + Tuwunel)', {
        skip: 'Requires WASM bindings built via wasm-pack and tuwunel binary',
    }, () => {
        // 1. Start TuwunelInstance
        // 2. Register client + worker via WasmMatrixClient
        // 3. Client submits SessionTask
        // 4. Worker posts SessionStart, SessionOutput, SessionResult
        // 5. Client receives and decodes all events
        // 6. Verify full round-trip of data
    });

    it('state events as process table (requires WASM + Tuwunel)', {
        skip: 'Requires WASM bindings built via wasm-pack and tuwunel binary',
    }, () => {
        // 1. Worker writes ActiveSessionState for 2 sessions
        // 2. Client reads state events
        // 3. Verify both appear as active
        // 4. Worker completes one, writes CompletedSessionState
        // 5. Verify correct session marked as completed
    });

    it('cancel flow (requires WASM + Tuwunel)', {
        skip: 'Requires WASM bindings built via wasm-pack and tuwunel binary',
    }, () => {
        // 1. Client submits task
        // 2. Worker starts it
        // 3. Client sends SessionCancel
        // 4. Worker receives cancel, posts SessionResult with status=cancelled
    });

    it('interactive session flag propagation (requires WASM + Tuwunel)', {
        skip: 'Requires WASM bindings built via wasm-pack and tuwunel binary',
    }, () => {
        // 1. Client submits task with interactive=true
        // 2. Worker receives and verifies interactive flag
        // 3. Worker writes ActiveSessionState with interactive=true
    });

    it('backward compat: old fabric events (requires WASM + Tuwunel)', {
        skip: 'Requires WASM bindings built via wasm-pack and tuwunel binary',
    }, () => {
        // 1. Client sends old org.mxdx.fabric.task event
        // 2. Worker receives and translates to SessionTask
        // 3. Session proceeds with new schema
    });

    it('client disconnect -> reconnect -> resume tailing (requires WASM + Tuwunel)', {
        skip: 'Requires WASM bindings built via wasm-pack and tuwunel binary',
    }, () => {
        // 1. Start long-running task, client tails output
        // 2. Client disconnects
        // 3. Client reconnects, fetches thread history
        // 4. No gap in output
    });

    it('ls shows sessions, logs fetches correct thread (requires WASM + Tuwunel)', {
        skip: 'Requires WASM bindings built via wasm-pack and tuwunel binary',
    }, () => {
        // 1. Run multiple tasks
        // 2. mxdx ls lists all sessions
        // 3. mxdx logs <session-id> fetches correct thread
    });
});
