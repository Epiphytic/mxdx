import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import * as TOML from 'smol-toml';

export class LauncherConfig {
  constructor({
    username,
    servers = [],
    allowedCommands = [],
    allowedCwd = ['/tmp'],
    telemetry = 'full',
    maxSessions = 5,
    adminUsers = [],
    registrationToken = null,
    useTmux = 'auto',
  } = {}) {
    this.username = username || os.hostname();
    this.servers = servers;
    this.allowedCommands = allowedCommands;
    this.allowedCwd = allowedCwd;
    this.telemetry = telemetry;
    this.maxSessions = maxSessions;
    this.adminUsers = adminUsers;
    this.registrationToken = registrationToken;
    this.useTmux = useTmux;
  }

  static fromArgs(args) {
    return new LauncherConfig({
      username: args.username,
      servers: args.servers ? args.servers.split(',') : [],
      allowedCommands: args.allowedCommands ? args.allowedCommands.split(',') : [],
      allowedCwd: args.allowedCwd ? args.allowedCwd.split(',') : ['/tmp'],
      telemetry: args.telemetry || 'full',
      maxSessions: args.maxSessions ? parseInt(args.maxSessions, 10) : 5,
      adminUsers: args.adminUser ? args.adminUser.split(',') : [],
      registrationToken: args.registrationToken || null,
      useTmux: args.useTmux || 'auto',
    });
  }

  save(filePath) {
    const dir = path.dirname(filePath);
    fs.mkdirSync(dir, { recursive: true, mode: 0o700 });
    const toml = TOML.stringify({
      launcher: {
        username: this.username,
        servers: this.servers,
        allowed_commands: this.allowedCommands,
        allowed_cwd: this.allowedCwd,
        telemetry: this.telemetry,
        max_sessions: this.maxSessions,
        admin_users: this.adminUsers,
        use_tmux: this.useTmux,
      },
    });
    fs.writeFileSync(filePath, toml, { mode: 0o600 });
  }

  static load(filePath) {
    if (!fs.existsSync(filePath)) return null;
    const content = fs.readFileSync(filePath, 'utf8');
    const parsed = TOML.parse(content);
    const l = parsed.launcher || {};
    return new LauncherConfig({
      username: l.username,
      servers: l.servers || [],
      allowedCommands: l.allowed_commands || [],
      allowedCwd: l.allowed_cwd || ['/tmp'],
      telemetry: l.telemetry || 'full',
      maxSessions: l.max_sessions || 5,
      adminUsers: l.admin_users || [],
      useTmux: l.use_tmux || 'auto',
    });
  }

  static defaultPath() {
    return path.join(os.homedir(), '.config', 'mxdx', 'launcher.toml');
  }
}
