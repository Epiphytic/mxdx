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
    batchMs = 200,
    p2pEnabled = true,
    p2pBatchMs = 10,
    p2pIdleTimeoutS = 300,
    p2pAdvertiseIps = false,
    telemetryIntervalS = 60,
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
    this.batchMs = batchMs;
    this.p2pEnabled = p2pEnabled;
    this.p2pBatchMs = p2pBatchMs;
    this.p2pIdleTimeoutS = p2pIdleTimeoutS;
    this.p2pAdvertiseIps = p2pAdvertiseIps;
    this.telemetryIntervalS = telemetryIntervalS;
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
      batchMs: args.batchMs ? parseInt(args.batchMs, 10) : 200,
      p2pEnabled: args.p2pEnabled !== undefined ? args.p2pEnabled !== 'false' : true,
      p2pBatchMs: args.p2pBatchMs ? parseInt(args.p2pBatchMs, 10) : 10,
      p2pIdleTimeoutS: args.p2pIdleTimeoutS ? parseInt(args.p2pIdleTimeoutS, 10) : 300,
      p2pAdvertiseIps: args.p2pAdvertiseIps === 'true' || args.p2pAdvertiseIps === true,
      telemetryIntervalS: args.telemetryIntervalS ? parseInt(args.telemetryIntervalS, 10) : 60,
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
        batch_ms: this.batchMs,
        p2p_enabled: this.p2pEnabled,
        p2p_batch_ms: this.p2pBatchMs,
        p2p_idle_timeout_s: this.p2pIdleTimeoutS,
        p2p_advertise_ips: this.p2pAdvertiseIps,
        telemetry_interval_s: this.telemetryIntervalS,
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
      batchMs: l.batch_ms || 200,
      p2pEnabled: l.p2p_enabled !== undefined ? l.p2p_enabled : true,
      p2pBatchMs: l.p2p_batch_ms || 10,
      p2pIdleTimeoutS: l.p2p_idle_timeout_s || 300,
      p2pAdvertiseIps: l.p2p_advertise_ips === true,
      telemetryIntervalS: l.telemetry_interval_s || 60,
    });
  }

  static defaultPath() {
    return path.join(os.homedir(), '.config', 'mxdx', 'launcher.toml');
  }
}
