import { spawn } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import net from 'node:net';

const REGISTRATION_TOKEN = 'mxdx-test-token';
const TUWUNEL_PATHS = ['/usr/sbin/tuwunel', '/usr/local/bin/tuwunel'];

function findTuwunel() {
  for (const p of TUWUNEL_PATHS) {
    if (fs.existsSync(p)) return p;
  }
  throw new Error('tuwunel binary not found');
}

function pickFreePort() {
  return new Promise((resolve, reject) => {
    const srv = net.createServer();
    srv.listen(0, '127.0.0.1', () => {
      const port = srv.address().port;
      srv.close(() => resolve(port));
    });
    srv.on('error', reject);
  });
}

export class TuwunelInstance {
  #process;
  #dataDir;

  /**
   * Check whether the tuwunel binary is available on this system.
   * @returns {boolean}
   */
  static isAvailable() {
    return TUWUNEL_PATHS.some((p) => fs.existsSync(p));
  }

  constructor({ port, serverName, process: proc, dataDir }) {
    this.port = port;
    this.serverName = serverName;
    this.registrationToken = REGISTRATION_TOKEN;
    this.#process = proc;
    this.#dataDir = dataDir;
  }

  get url() {
    return `http://127.0.0.1:${this.port}`;
  }

  static async start() {
    const port = await pickFreePort();
    const dataDir = fs.mkdtempSync(path.join(os.tmpdir(), 'mxdx-tuwunel-'));
    const dbPath = path.join(dataDir, 'db');
    fs.mkdirSync(dbPath, { recursive: true });

    const serverName = `e2e-${port}.localhost`;
    const configPath = path.join(dataDir, 'tuwunel.toml');

    fs.writeFileSync(configPath, `[global]
server_name = "${serverName}"
database_path = "${dbPath}"
address = ["127.0.0.1"]
port = ${port}
allow_registration = true
registration_token = "${REGISTRATION_TOKEN}"
log = "error"
new_user_displayname_suffix = ""
`);

    const bin = findTuwunel();
    const proc = spawn(bin, ['-c', configPath], {
      stdio: ['ignore', 'ignore', 'ignore'],
    });

    const instance = new TuwunelInstance({
      port,
      serverName,
      process: proc,
      dataDir,
    });

    await instance.#waitForHealth();
    return instance;
  }

  async #waitForHealth() {
    const url = `${this.url}/_matrix/client/versions`;
    const deadline = Date.now() + 30000;

    while (Date.now() < deadline) {
      try {
        const resp = await fetch(url);
        if (resp.ok) return;
      } catch {
        // not ready yet
      }
      await new Promise((r) => setTimeout(r, 100));
    }
    throw new Error(`Tuwunel on port ${this.port} did not become healthy`);
  }

  stop() {
    this.#process.kill();
    try {
      fs.rmSync(this.#dataDir, { recursive: true });
    } catch {
      // best effort
    }
  }
}
