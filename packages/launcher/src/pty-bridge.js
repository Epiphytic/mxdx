import { spawn } from 'node:child_process';

/**
 * PtyBridge — manages an interactive shell session using `script` for
 * PTY allocation. This avoids the need for node-pty native bindings.
 *
 * Uses: script -q /dev/null -c <command>
 * This allocates a real PTY for the child process.
 */
export class PtyBridge {
  #proc = null;
  #alive = false;
  #dataCallbacks = [];
  #cols;
  #rows;

  /**
   * @param {string} command - Shell command to run (e.g. "bash", "/bin/sh")
   * @param {Object} options
   * @param {number} [options.cols=80]
   * @param {number} [options.rows=24]
   * @param {string} [options.cwd='/tmp']
   * @param {Record<string,string>} [options.env={}]
   */
  constructor(command, { cols = 80, rows = 24, cwd = '/tmp', env = {} } = {}) {
    this.#cols = cols;
    this.#rows = rows;

    // Use script(1) to allocate a real PTY for the shell
    this.#proc = spawn('script', ['-q', '/dev/null', '-c', command], {
      cwd,
      env: {
        ...process.env,
        ...env,
        TERM: 'xterm-256color',
        COLUMNS: String(cols),
        LINES: String(rows),
      },
      stdio: ['pipe', 'pipe', 'pipe'],
    });

    this.#alive = true;

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
    if (!this.#alive || !this.#proc?.pid) return;
    this.#cols = cols;
    this.#rows = rows;
    // Send SIGWINCH to the script process with new size via stty
    try {
      spawn('kill', ['-WINCH', String(this.#proc.pid)], { stdio: 'ignore' });
    } catch {
      // Best effort
    }
  }

  kill() {
    this.#alive = false;
    if (this.#proc) {
      this.#proc.kill();
      this.#proc = null;
    }
  }

  get alive() {
    return this.#alive;
  }
}
