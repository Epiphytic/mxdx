import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import * as TOML from 'smol-toml';

// Fields this runtime owns — used by save() to merge without clobbering unrelated keys.
const LAUNCHER_OWNED_KEYS = [
  'username', 'servers', 'allowed_commands', 'allowed_cwd', 'telemetry',
  'max_sessions', 'admin_users', 'use_tmux', 'batch_ms', 'p2p_enabled',
  'p2p_batch_ms', 'p2p_idle_timeout_s', 'p2p_advertise_ips', 'p2p_turn_only',
  'telemetry_interval_s', 'preferred_server', 'server_credentials',
];

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
    p2pTurnOnly = false,
    telemetryIntervalS = 60,
    preferredServer = null,
    serverCredentials = {},
    password = null,
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
    this.p2pTurnOnly = p2pTurnOnly;
    this.telemetryIntervalS = Math.max(10, telemetryIntervalS);
    this.preferredServer = preferredServer;
    this.serverCredentials = serverCredentials;
    this.password = password;
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
      p2pTurnOnly: args.p2pTurnOnly === 'true' || args.p2pTurnOnly === true,
      telemetryIntervalS: args.telemetryIntervalS ? parseInt(args.telemetryIntervalS, 10) : 60,
      preferredServer: args.preferredServer || null,
      password: args.password || null,
    });
  }

  save(filePath) {
    const dir = path.dirname(filePath);
    fs.mkdirSync(dir, { recursive: true, mode: 0o700 });

    // Read existing file and merge: only overwrite launcher-owned keys, preserve others.
    let existing = {};
    if (fs.existsSync(filePath)) {
      try {
        const raw = fs.readFileSync(filePath, 'utf8');
        const parsed = TOML.parse(raw);
        // If the existing file still has a legacy section, flatten it in-memory before merging.
        if (parsed.launcher && typeof parsed.launcher === 'object') {
          existing = { ...parsed.launcher };
        } else {
          existing = { ...parsed };
        }
      } catch (_) {
        // Unreadable existing file — start fresh
      }
    }

    const ownedFields = {
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
      p2p_turn_only: this.p2pTurnOnly,
      telemetry_interval_s: this.telemetryIntervalS,
      ...(this.preferredServer ? { preferred_server: this.preferredServer } : {}),
      ...(Object.keys(this.serverCredentials).length ? { server_credentials: this.serverCredentials } : {}),
    };

    // Merge: preserve unknown keys from existing flat file, overwrite owned keys.
    const merged = { ...existing };
    // Remove any legacy section key if present
    delete merged.launcher;
    Object.assign(merged, ownedFields);

    const toml = TOML.stringify(merged);
    fs.writeFileSync(filePath, toml, { mode: 0o600 });
  }

  static load(filePath) {
    if (!fs.existsSync(filePath)) return null;
    const content = fs.readFileSync(filePath, 'utf8');
    const parsed = TOML.parse(content);

    // ADR 2026-04-29 req 6a: detect legacy [launcher] section and migrate.
    // The Rust binary will also perform this migration, but if npm runs first we
    // handle it here so security-critical fields are never silently zero-fielded.
    let flat;
    if (parsed.launcher && typeof parsed.launcher === 'object') {
      process.stderr.write(
        `mxdx: WARNING: ${filePath} uses legacy [launcher] section wrapper. ` +
        `Migrating to flat-key layout. Original saved to ${filePath}.legacy.bak\n`
      );
      try {
        fs.writeFileSync(`${filePath}.legacy.bak`, content, { mode: 0o600 });
      } catch (_) {}
      flat = parsed.launcher;
      // Rewrite file in flat format so subsequent reads (including Rust) get the migrated version.
      try {
        fs.writeFileSync(filePath, TOML.stringify(flat), { mode: 0o600 });
      } catch (_) {}
    } else {
      flat = parsed;
    }

    return new LauncherConfig({
      username: flat.username,
      servers: flat.servers || [],
      allowedCommands: flat.allowed_commands || [],
      allowedCwd: flat.allowed_cwd || ['/tmp'],
      telemetry: flat.telemetry || 'full',
      maxSessions: flat.max_sessions || 5,
      adminUsers: flat.admin_users || [],
      useTmux: flat.use_tmux || 'auto',
      batchMs: flat.batch_ms || 200,
      p2pEnabled: flat.p2p_enabled !== undefined ? flat.p2p_enabled : true,
      p2pBatchMs: flat.p2p_batch_ms || 10,
      p2pIdleTimeoutS: flat.p2p_idle_timeout_s || 300,
      p2pAdvertiseIps: flat.p2p_advertise_ips === true,
      p2pTurnOnly: flat.p2p_turn_only === true,
      telemetryIntervalS: flat.telemetry_interval_s || 60,
      preferredServer: flat.preferred_server || null,
      serverCredentials: flat.server_credentials || {},
    });
  }

  static defaultPath() {
    return path.join(os.homedir(), '.mxdx', 'worker.toml');
  }

  /**
   * @deprecated Use defaultPath() instead. Retained for migration.
   */
  static legacyDefaultPath() {
    return path.join(os.homedir(), '.config', 'mxdx', 'launcher.toml');
  }
}
