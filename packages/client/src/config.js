import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import * as TOML from 'smol-toml';

export class ClientConfig {
  constructor({ username, server, password, registrationToken, batchMs = 200, p2pEnabled = true, p2pBatchMs = 10, p2pIdleTimeoutS = 300 } = {}) {
    this.username = username || os.hostname();
    this.server = server;
    this.password = password;
    this.registrationToken = registrationToken;
    this.batchMs = batchMs;
    this.p2pEnabled = p2pEnabled;
    this.p2pBatchMs = p2pBatchMs;
    this.p2pIdleTimeoutS = p2pIdleTimeoutS;
  }

  static fromArgs(args) {
    return new ClientConfig({
      username: args.username,
      server: args.server,
      password: args.password,
      registrationToken: args.registrationToken,
      batchMs: args.batchMs ? parseInt(args.batchMs, 10) : 200,
      p2pEnabled: args.p2pEnabled !== undefined ? args.p2pEnabled !== 'false' : true,
      p2pBatchMs: args.p2pBatchMs ? parseInt(args.p2pBatchMs, 10) : 10,
      p2pIdleTimeoutS: args.p2pIdleTimeoutS ? parseInt(args.p2pIdleTimeoutS, 10) : 300,
    });
  }

  save(filePath) {
    const dir = path.dirname(filePath);
    fs.mkdirSync(dir, { recursive: true, mode: 0o700 });
    const toml = TOML.stringify({
      client: {
        username: this.username,
        server: this.server,
        batch_ms: this.batchMs,
        p2p_enabled: this.p2pEnabled,
        p2p_batch_ms: this.p2pBatchMs,
        p2p_idle_timeout_s: this.p2pIdleTimeoutS,
      },
    });
    fs.writeFileSync(filePath, toml, { mode: 0o600 });
  }

  static load(filePath) {
    if (!fs.existsSync(filePath)) return null;
    const content = fs.readFileSync(filePath, 'utf8');
    const parsed = TOML.parse(content);
    const c = parsed.client || {};
    return new ClientConfig({
      username: c.username,
      server: c.server,
      batchMs: c.batch_ms || 200,
      p2pEnabled: c.p2p_enabled !== undefined ? c.p2p_enabled : true,
      p2pBatchMs: c.p2p_batch_ms || 10,
      p2pIdleTimeoutS: c.p2p_idle_timeout_s || 300,
    });
  }

  static defaultPath() {
    return path.join(os.homedir(), '.config', 'mxdx', 'client.toml');
  }
}
