import { spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';

/**
 * PtyBridge — manages a PTY session backed by tmux for persistence.
 *
 * Spawns: tmux new-session -d -s <id> -x <cols> -y <rows> <command>
 * Attaches a pipe process that reads/writes to the tmux session.
 */
export class PtyBridge {
  #sessionId;
  #proc = null;
  #alive = false;
  #dataCallbacks = [];

  /**
   * @param {string} command - Shell command to run (e.g. "bash", "/bin/sh")
   * @param {Object} options
   * @param {number} [options.cols=80]
   * @param {number} [options.rows=24]
   * @param {string} [options.cwd='/tmp']
   * @param {Record<string,string>} [options.env={}]
   */
  constructor(command, { cols = 80, rows = 24, cwd = '/tmp', env = {} } = {}) {
    this.#sessionId = `mxdx-${randomUUID().slice(0, 8)}`;

    // Create tmux session in detached mode
    const tmuxCreate = spawn('tmux', [
      'new-session', '-d',
      '-s', this.#sessionId,
      '-x', String(cols),
      '-y', String(rows),
      command,
    ], {
      cwd,
      env: { ...process.env, ...env },
      stdio: 'ignore',
    });

    tmuxCreate.on('close', (code) => {
      if (code !== 0) return;
      this.#alive = true;
      this.#attachPipe();
    });

    tmuxCreate.on('error', () => {
      this.#alive = false;
    });
  }

  #attachPipe() {
    // Use script + tmux attach to get PTY output as a byte stream
    this.#proc = spawn('tmux', [
      'attach-session', '-t', this.#sessionId,
    ], {
      stdio: ['pipe', 'pipe', 'pipe'],
      env: { ...process.env, TERM: 'xterm-256color' },
    });

    this.#proc.stdout.on('data', (chunk) => {
      for (const cb of this.#dataCallbacks) {
        cb(new Uint8Array(chunk));
      }
    });

    this.#proc.stderr.on('data', (chunk) => {
      for (const cb of this.#dataCallbacks) {
        cb(new Uint8Array(chunk));
      }
    });

    this.#proc.on('close', () => {
      this.#alive = false;
    });

    this.#proc.on('error', () => {
      this.#alive = false;
    });
  }

  write(data) {
    if (!this.#alive) return;

    if (this.#proc && this.#proc.stdin.writable) {
      this.#proc.stdin.write(data instanceof Uint8Array ? Buffer.from(data) : data);
    } else {
      // Fallback: send keys via tmux send-keys
      const text = data instanceof Uint8Array ? Buffer.from(data).toString() : data;
      spawn('tmux', ['send-keys', '-t', this.#sessionId, '-l', text], {
        stdio: 'ignore',
      });
    }
  }

  onData(callback) {
    this.#dataCallbacks.push(callback);
    return () => {
      const idx = this.#dataCallbacks.indexOf(callback);
      if (idx >= 0) this.#dataCallbacks.splice(idx, 1);
    };
  }

  resize(cols, rows) {
    if (!this.#alive) return;
    spawn('tmux', [
      'resize-window', '-t', this.#sessionId,
      '-x', String(cols), '-y', String(rows),
    ], { stdio: 'ignore' });
  }

  kill() {
    this.#alive = false;
    if (this.#proc) {
      this.#proc.kill();
      this.#proc = null;
    }
    spawn('tmux', ['kill-session', '-t', this.#sessionId], { stdio: 'ignore' });
  }

  get alive() {
    return this.#alive;
  }

  get sessionId() {
    return this.#sessionId;
  }
}
