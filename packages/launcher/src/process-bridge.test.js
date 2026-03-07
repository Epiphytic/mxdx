import { describe, it } from 'node:test';
import assert from 'node:assert';
import { executeCommand } from './process-bridge.js';

describe('ProcessBridge', () => {
  it('captures stdout from a simple command', async () => {
    const result = await executeCommand('echo', ['hello world']);
    assert.strictEqual(result.exitCode, 0);
    assert.ok(result.stdout.includes('hello world'));
  });

  it('captures stderr', async () => {
    const result = await executeCommand('sh', ['-c', 'echo error >&2']);
    assert.strictEqual(result.exitCode, 0);
    assert.ok(result.stderr.includes('error'));
  });

  it('returns non-zero exit code', async () => {
    const result = await executeCommand('sh', ['-c', 'exit 42']);
    assert.strictEqual(result.exitCode, 42);
  });

  it('streams output lines via callback', async () => {
    const lines = [];
    await executeCommand('seq', ['1', '5'], {
      onStdout: (line) => lines.push(line),
    });
    assert.strictEqual(lines.length, 5);
    assert.strictEqual(lines[0], '1');
    assert.strictEqual(lines[4], '5');
  });

  it('enforces timeout', async () => {
    const result = await executeCommand('sleep', ['10'], { timeoutMs: 500 });
    assert.ok(result.timedOut);
    assert.notStrictEqual(result.exitCode, 0);
  });
});
