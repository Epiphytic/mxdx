import { spawn, execFileSync } from 'node:child_process';
import crypto from 'node:crypto';

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

  static tmuxInfo() {
    return detectTmux();
  }

  static list() {
    try {
      const output = execFileSync('tmux', ['list-sessions', '-F', '#{session_name}'], {
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
  constructor(command, { cols = 80, rows = 24, cwd = '/tmp', env = {}, sessionName = null, useTmux = 'auto' } = {}) {
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
      this.#tmuxName = sessionName || `mxdx-${crypto.randomUUID().slice(0, 8)}`;

      const existing = PtyBridge.list().includes(this.#tmuxName);

      if (!existing) {
        // Create detached tmux session — execFileSync (no shell)
        execFileSync('tmux', [
          'new-session', '-d', '-s', this.#tmuxName,
          '-x', String(cols), '-y', String(rows),
          command,
        ], { env: shellEnv, cwd, timeout: 5000 });
      } else {
        execFileSync('tmux', [
          'resize-window', '-t', this.#tmuxName,
          '-x', String(cols), '-y', String(rows),
        ], { timeout: 5000 });
      }

      // Attach via script for piped stdio
      this.#proc = spawn('script', ['-q', '/dev/null', '-c', `tmux attach -t ${this.#tmuxName}`], {
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

    if (this.#tmuxName) {
      try {
        execFileSync('tmux', ['resize-window', '-t', this.#tmuxName, '-x', String(cols), '-y', String(rows)], { timeout: 5000 });
      } catch { /* best effort */ }
    } else if (this.#proc?.pid) {
      try {
        spawn('kill', ['-WINCH', String(this.#proc.pid)], { stdio: 'ignore' });
      } catch { /* best effort */ }
    }
  }

  detach() {
    if (this.#proc) {
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
    if (this.#tmuxName) {
      try {
        execFileSync('tmux', ['kill-session', '-t', this.#tmuxName], { timeout: 5000 });
      } catch { /* session may already be dead */ }
      this.#tmuxName = null;
    }
  }

  get alive() {
    return this.#alive;
  }
}
