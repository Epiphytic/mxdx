import { spawn } from 'node:child_process';
import { createInterface } from 'node:readline';

/**
 * Execute a command and capture output.
 * @param {string} command - The command to run
 * @param {string[]} args - Command arguments
 * @param {Object} [options]
 * @param {function} [options.onStdout] - Callback for each stdout line
 * @param {function} [options.onStderr] - Callback for each stderr line
 * @param {string} [options.cwd] - Working directory
 * @param {number} [options.timeoutMs] - Timeout in milliseconds
 * @returns {Promise<{exitCode: number, stdout: string, stderr: string, timedOut: boolean}>}
 */
export function executeCommand(command, args = [], options = {}) {
  return new Promise((resolve) => {
    const { onStdout, onStderr, cwd, timeoutMs } = options;
    const stdoutChunks = [];
    const stderrChunks = [];
    let timedOut = false;

    const proc = spawn(command, args, {
      cwd,
      stdio: ['ignore', 'pipe', 'pipe'],
    });

    let timeout;
    if (timeoutMs) {
      timeout = setTimeout(() => {
        timedOut = true;
        proc.kill('SIGKILL');
      }, timeoutMs);
    }

    if (onStdout) {
      const rl = createInterface({ input: proc.stdout });
      rl.on('line', (line) => {
        stdoutChunks.push(line);
        onStdout(line);
      });
    } else {
      proc.stdout.on('data', (chunk) => stdoutChunks.push(chunk.toString()));
    }

    if (onStderr) {
      const rl = createInterface({ input: proc.stderr });
      rl.on('line', (line) => {
        stderrChunks.push(line);
        onStderr(line);
      });
    } else {
      proc.stderr.on('data', (chunk) => stderrChunks.push(chunk.toString()));
    }

    proc.on('close', (code, signal) => {
      if (timeout) clearTimeout(timeout);
      resolve({
        exitCode: timedOut ? 137 : (code ?? 1),
        stdout: stdoutChunks.join(onStdout ? '\n' : ''),
        stderr: stderrChunks.join(onStderr ? '\n' : ''),
        timedOut,
      });
    });

    proc.on('error', (err) => {
      if (timeout) clearTimeout(timeout);
      resolve({
        exitCode: 1,
        stdout: '',
        stderr: err.message,
        timedOut: false,
      });
    });
  });
}
