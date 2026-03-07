import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import * as TOML from 'smol-toml';

export class ClientConfig {
  constructor({ username, server, password, registrationToken } = {}) {
    this.username = username || os.hostname();
    this.server = server;
    this.password = password;
    this.registrationToken = registrationToken;
  }

  static fromArgs(args) {
    return new ClientConfig({
      username: args.username,
      server: args.server,
      password: args.password,
      registrationToken: args.registrationToken,
    });
  }

  save(filePath) {
    const dir = path.dirname(filePath);
    fs.mkdirSync(dir, { recursive: true, mode: 0o700 });
    const toml = TOML.stringify({
      client: {
        username: this.username,
        server: this.server,
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
    });
  }

  static defaultPath() {
    return path.join(os.homedir(), '.config', 'mxdx', 'client.toml');
  }
}
