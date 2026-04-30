// Rust equivalent: crates/mxdx-worker/src/bin/mxdx_exec.rs::main (PTY + tmux + Unix-socket exit-code channel, OS-bound)
import { spawn, execFileSync } from 'node:child_process';
import crypto from 'node:crypto';
import path from 'node:path';
import os from 'node:os';
import fs from 'node:fs';

// Default tmux socket directory — isolates mxdx sessions from user's regular tmux
const DEFAULT_SOCKET_DIR = path.join(os.homedir(), '.mxdx', 'tmux');

function ensureDir(dir) {
  fs.mkdirSync(dir, { recursive: true, mode: 0o700 });
}

function detectTmux() {
  try {
    const version = execFileSync('tmux', ['-V'], { encoding: 'utf8', timeout: 5000 }).trim();
    const match = version.match(/tmux\s+([\d.]+)/);
    return { available: true, version: match ? match[1] : version };
  } catch {
    return { available: false, version: null };
  }
}

/**
 * PtyBridge — manages an interactive shell session using `script` for
 * PTY allocation. Supports tmux for session persistence with graceful
 * fallback to bare script(1) when tmux is unavailable.
 */
export class PtyBridge {
  #proc = null;
  #alive = false;
  #dataCallbacks = [];
  #cols;
  #rows;
  #tmuxName = null;
  #persistent = false;
  #socketDir = null;

  static tmuxInfo() {
    return detectTmux();
  }

  static list(socketDir = DEFAULT_SOCKET_DIR) {
    try {
      const socketPath = path.join(socketDir, 'mxdx');
      const output = execFileSync('tmux', ['-S', socketPath, 'list-sessions', '-F', '#{session_name}'], {
        encoding: 'utf8',
        timeout: 5000,
      }).trim();
      return output
        .split('\n')
        .filter(name => name.startsWith('mxdx-'));
    } catch {
      return [];
    }
  }

  static get defaultSocketDir() {
    return DEFAULT_SOCKET_DIR;
  }

  /**
   * @param {string} command - Shell command to run (e.g. "bash", "/bin/sh")
   * @param {Object} options
   * @param {number} [options.cols=80]
   * @param {number} [options.rows=24]
   * @param {string} [options.cwd='/tmp']
   * @param {Record<string,string>} [options.env={}]
   * @param {string|null} [options.sessionName=null] - tmux session name (for reconnect)
   * @param {string} [options.useTmux='auto'] - 'auto'|'always'|'never'
   */
  constructor(command, { cols = 80, rows = 24, cwd = '/tmp', env = {}, sessionName = null, useTmux = 'auto', socketDir = DEFAULT_SOCKET_DIR } = {}) {
    this.#cols = cols;
    this.#rows = rows;

    const tmux = detectTmux();
    const wantTmux = useTmux === 'always' || (useTmux === 'auto' && tmux.available);

    if (useTmux === 'always' && !tmux.available) {
      throw new Error('tmux required (use_tmux=always) but not found on PATH');
    }

    this.#persistent = wantTmux;

    const shellEnv = {
      ...process.env,
      ...env,
      TERM: 'xterm-256color',
      COLUMNS: String(cols),
      LINES: String(rows),
    };

    if (wantTmux) {
      this.#socketDir = socketDir;
      ensureDir(socketDir);
      const socketPath = path.join(socketDir, 'mxdx');
      this.#tmuxName = sessionName || `mxdx-${crypto.randomUUID().slice(0, 8)}`;

      const existing = PtyBridge.list(socketDir).includes(this.#tmuxName);

      if (!existing) {
        // Create detached tmux session using dedicated socket
        execFileSync('tmux', [
          '-S', socketPath,
          'new-session', '-d', '-s', this.#tmuxName,
          '-x', String(cols), '-y', String(rows),
          command,
        ], { env: shellEnv, cwd, timeout: 5000 });
      } else {
        execFileSync('tmux', [
          '-S', socketPath,
          'resize-window', '-t', this.#tmuxName,
          '-x', String(cols), '-y', String(rows),
        ], { timeout: 5000 });
      }

      // Attach via script for piped stdio
      this.#proc = spawn('script', ['-q', '/dev/null', '-c', `tmux -S ${socketPath} attach -t ${this.#tmuxName}`], {
        cwd,
        env: shellEnv,
        stdio: ['pipe', 'pipe', 'pipe'],
      });
    } else {
      this.#tmuxName = null;
      this.#proc = spawn('script', ['-q', '/dev/null', '-c', command], {
        cwd,
        env: shellEnv,
        stdio: ['pipe', 'pipe', 'pipe'],
      });
    }

    this.#alive = true;

    this.#proc.stdout.on('data', (chunk) => {
      for (const cb of this.#dataCallbacks) cb(new Uint8Array(chunk));
    });

    this.#proc.stderr.on('data', (chunk) => {
      for (const cb of this.#dataCallbacks) cb(new Uint8Array(chunk));
    });

    this.#proc.on('close', () => { this.#alive = false; });
    this.#proc.on('error', () => { this.#alive = false; });
  }

  get persistent() { return this.#persistent; }
  get tmuxName() { return this.#tmuxName; }

  write(data) {
    if (!this.#alive || !this.#proc?.stdin?.writable) return;
    this.#proc.stdin.write(data instanceof Uint8Array ? Buffer.from(data) : data);
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
    this.#cols = cols;
    this.#rows = rows;

    if (this.#tmuxName && this.#socketDir) {
      try {
        const socketPath = path.join(this.#socketDir, 'mxdx');
        execFileSync('tmux', ['-S', socketPath, 'resize-window', '-t', this.#tmuxName, '-x', String(cols), '-y', String(rows)], { timeout: 5000 });
      } catch { /* best effort */ }
    } else if (this.#proc?.pid) {
      try {
        spawn('kill', ['-WINCH', String(this.#proc.pid)], { stdio: 'ignore' });
      } catch { /* best effort */ }
    }
  }

  detach() {
    if (this.#proc) {
      if (this.#tmuxName && this.#socketDir) {
        // Detach the tmux client cleanly so the session survives
        try {
          const socketPath = path.join(this.#socketDir, 'mxdx');
          execFileSync('tmux', ['-S', socketPath, 'detach-client', '-s', this.#tmuxName], { timeout: 5000 });
        } catch { /* best effort */ }
      }
      this.#proc.kill();
      this.#proc = null;
    }
    if (!this.#tmuxName) {
      this.#alive = false;
    }
  }

  kill() {
    this.#alive = false;
    if (this.#proc) {
      this.#proc.kill();
      this.#proc = null;
    }
    if (this.#tmuxName && this.#socketDir) {
      try {
        const socketPath = path.join(this.#socketDir, 'mxdx');
        execFileSync('tmux', ['-S', socketPath, 'kill-session', '-t', this.#tmuxName], { timeout: 5000 });
      } catch { /* session may already be dead */ }
      this.#tmuxName = null;
    }
  }

  get alive() {
    return this.#alive;
  }
}
