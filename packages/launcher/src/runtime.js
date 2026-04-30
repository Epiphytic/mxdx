import crypto from 'node:crypto';
import path from 'node:path';
import os from 'node:os';
import {
  connectWithSession,
  saveIndexedDB,
  BatchedSender,
  P2PTransport,
  generateSessionKey,
  createP2PCrypto,
  buildTelemetryPayload,
  SessionTransportManager,
  WasmSessionManager,
} from '@mxdx/core';
// Rust equivalent: crates/mxdx-core-wasm/src/lib.rs::WasmBatchedSender + compress_terminal_data
import { executeCommand } from './process-bridge.js';
// Rust equivalent: crates/mxdx-worker/src/bin/mxdx_exec.rs::main (subprocess execution via mxdx-exec binary)
import { PtyBridge } from './pty-bridge.js';
// Rust equivalent: crates/mxdx-worker/src/bin/mxdx_exec.rs::main (tmux + Unix-socket exit-code channel)
import { SessionMux } from './session-mux.js';
// Rust equivalent: crates/mxdx-core-wasm/src/lib.rs::SessionTransportManager (state tracking)
import { attemptP2PConnection } from './p2p-bridge.js';
// Rust equivalent: crates/mxdx-worker/src/p2p/ (OS-bound: node-datachannel native addon)

const DEFAULT_SESSION_DIR = path.join(os.homedir(), '.mxdx');

/**
 * Structured logger with JSON and text output modes.
 * Rust equivalent: crates/mxdx-worker/src/logging.rs
 */
class Logger {
  #format;
  constructor(format = 'json') { this.#format = format; }
  info(msg, data) { this.#log('info', msg, data); }
  warn(msg, data) { this.#log('warn', msg, data); }
  error(msg, data) { this.#log('error', msg, data); }
  debug(msg, data) { this.#log('debug', msg, data); }
  #log(level, msg, data) {
    if (this.#format === 'json') {
      (level === 'error' ? process.stderr : process.stdout).write(JSON.stringify({ level, msg, ts: new Date().toISOString(), ...data }) + '\n');
    } else {
      const extra = data ? ' ' + JSON.stringify(data) : '';
      (level === 'error' ? process.stderr : process.stdout).write(`[${level}] [${new Date().toISOString()}] ${msg}${extra}\n`);
    }
  }
}

/**
 * The launcher runtime: thin OS-bound shell delegating to WasmSessionManager.
 * All session state, routing, and authorization live in Rust (WasmSessionManager).
 * Rust equivalent: crates/mxdx-core-wasm/src/lib.rs::WasmSessionManager
 */
export class LauncherRuntime {
  #client; #config; #topology; #stateRoomId; #running = false;
  #backoffMs = 0; #lastStoreSave = 0; #log;
  #sessionMgr = null; // Rust equivalent: crates/mxdx-core-wasm/src/lib.rs::WasmSessionManager
  #ptys = new Map(); // sessionId -> PtyBridge (OS-bound, not in WASM)
  #roomMuxes = new Map(); // dmRoomId -> SessionMux
  #roomTransports = new Map(); // roomId -> { transport, p2pCrypto }
  #transportManager = null; // Rust equivalent: crates/mxdx-core-wasm/src/lib.rs::SessionTransportManager
  #telemetryTimer = null;

  constructor(config) { this.#config = config; this.#log = new Logger(config.logFormat || 'json'); }

  get #sessionDir() { return this.#config.sessionDir || DEFAULT_SESSION_DIR; }
  get #socketDir() { return this.#config.tmuxSocketDir || path.join(this.#sessionDir, 'tmux'); }
  get topology() { return this.#topology; }
  get client() { return this.#client; }

  async start() {
    const servers = this.#config.servers; const username = this.#config.username;
    const log = (msg) => this.#log.info(msg);
    if (servers.length > 1) {
      const { MultiHsClient } = await import('@mxdx/core');
      this.#client = await MultiHsClient.connect(servers.map(server => { const creds = this.#config.serverCredentials?.[server]; return { username: creds?.username || username, server, password: creds?.password || this.#config.password, registrationToken: this.#config.registrationToken, configDir: this.#config.configDir, useKeychain: true, log }; }), { preferredServer: this.#config.preferredServer, log });
      this.#client.onPreferredChange((n, o) => { this.#log.info('Preferred server changed', { from: o.server, to: n.server }); this.#postTelemetry().catch(err => this.#log.warn('telemetry post failed after failover', { error: err.message })); });
    } else {
      const { client, freshLogin } = await connectWithSession({ username, server: servers[0], password: this.#config.password, registrationToken: this.#config.registrationToken, configDir: this.#config.configDir, useKeychain: true, log });
      this.#client = client;
      if (freshLogin && this.#config.password && this.#config.configPath) { this.#config.password = undefined; this.#config._password = undefined; try { this.#config.save(this.#config.configPath); log('Password removed from config file (now in keyring)'); } catch { /* Non-fatal */ } }
    }
    log(`Setting up rooms for ${username}...`);
    this.#topology = await this.#client.getOrCreateLauncherSpace(username);
    log(`Rooms ready: space=${this.#topology.space_id} exec=${this.#topology.exec_room_id}`);
    this.#stateRoomId = await this.#client.getOrCreateStateRoom(os.hostname(), os.userInfo().username, username);
    log(`State room ready: ${this.#stateRoomId}`);
    if (this.#config.adminUsers?.length > 0) {
      log(`Inviting admin users: ${this.#config.adminUsers.join(', ')}`);
      for (const adminUser of this.#config.adminUsers) for (const roomId of [this.#topology.space_id, this.#topology.exec_room_id, this.#topology.logs_room_id]) try { await this.#client.inviteUser(roomId, adminUser); } catch { /* May already be invited */ }
    }
    // Rust equivalent: crates/mxdx-core-wasm/src/lib.rs::WasmSessionManager
    this.#sessionMgr = new WasmSessionManager(JSON.stringify({ allowed_commands: this.#config.allowedCommands, allowed_cwd: this.#config.allowedCwd, max_sessions: this.#config.maxSessions || 10, username: this.#config.username, use_tmux: this.#config.useTmux || 'auto', batch_ms: this.#config.batchMs || 200 }), this.#topology.exec_room_id, this.#stateRoomId, this.#client.userId(), this.#client.deviceId());
    this.#transportManager = new SessionTransportManager(15_000);
    await this.#recoverSessions();
    await this.#postTelemetry();
    this.#telemetryTimer = setInterval(() => this.#postTelemetry().catch(err => this.#log.warn('telemetry post failed', { error: err.message })), this.#config.telemetryIntervalS * 1000);
    log('Online. Listening for commands...');
    this.#running = true;
    await this.#syncLoop();
  }

  async stop() {
    this.#running = false;
    try { await Promise.race([this.#postOfflineStatus(), new Promise((_, r) => setTimeout(() => r(new Error('timeout')), 1000))]); } catch { /* Don't block shutdown */ }
    if (this.#telemetryTimer) { clearInterval(this.#telemetryTimer); this.#telemetryTimer = null; }
    for (const [id, pty] of this.#ptys) { if (this.#sessionMgr?.sessionTmuxName(id)) { this.#log.info('Detaching persistent session', { sessionId: id }); pty.detach(); } }
    for (const [, entry] of this.#roomTransports) entry.transport.close();
    this.#roomTransports.clear();
    if (this.#sessionMgr) {
      const deviceId = this.#client.deviceId();
      for (const s of JSON.parse(this.#sessionMgr.listSessions())) {
        if (!s.persistent || !s.tmux_name) continue;
        try { await this.#client.writeSession(this.#stateRoomId, deviceId, s.session_id, JSON.stringify({ uuid: s.session_id, tmuxName: s.tmux_name, dmRoomId: this.#sessionMgr.sessionDmRoomId(s.session_id), sender: this.#sessionMgr.sessionSender(s.session_id), persistent: true, createdAt: s.created_at, state: 'detached' })); } catch (err) { this.#log.warn('Failed to persist session on shutdown', { session_id: s.session_id, error: err.message }); }
      }
    }
  }

  async #recoverSessions() {
    try { const rooms = JSON.parse(await this.#client.readRooms(this.#stateRoomId)); for (const entry of rooms) if (entry.content?.room_id && entry.content?.role === 'dm' && entry.content?.room_key) this.#sessionMgr.registerSessionRoom(entry.content.room_key, entry.content.room_id); }
    catch (err) { this.#log.warn('Failed to load rooms from state room', { error: err.message }); }
    const deviceId = this.#client.deviceId(); const liveTmux = PtyBridge.list(this.#socketDir); let recovered = 0;
    try {
      for (const entry of JSON.parse(await this.#client.readSessions(this.#stateRoomId))) {
        const content = entry.content || {}; const stateKey = entry.state_key || ''; const sessionId = content.uuid || stateKey.split('/')[1];
        if (!sessionId || !content.tmuxName || (stateKey && !stateKey.startsWith(`${deviceId}/`))) continue;
        if (liveTmux.includes(content.tmuxName)) { this.#sessionMgr.recoverSession(sessionId, content.tmuxName, content.dmRoomId || '', content.sender || '', true, content.createdAt || ''); recovered++; this.#log.info('Recovered tmux session', { session_id: sessionId, tmux: content.tmuxName }); }
        else { this.#log.info('Stale session removed (tmux gone)', { session_id: sessionId }); this.#client.removeSession(this.#stateRoomId, deviceId, sessionId).catch((err) => this.#log.warn('Failed to remove stale session', { session_id: sessionId, error: err.message })); }
      }
    } catch (err) { this.#log.warn('Failed to load sessions from state room', { error: err.message }); }
    if (recovered > 0) this.#log.info(`Recovered ${recovered} tmux session(s)`);
  }

  async #syncLoop() {
    while (this.#running) {
      try {
        await Promise.race([this.#client.syncOnce(), new Promise((_, r) => setTimeout(() => r(new Error('syncOnce timed out')), 30000))]);
        await Promise.race([this.#processCommands(), new Promise((_, r) => setTimeout(() => r(new Error('processCommands timed out')), 30000))]);
        this.#backoffMs = 0;
        if (Date.now() - this.#lastStoreSave > 300000) { try { await saveIndexedDB(this.#config.configDir); this.#lastStoreSave = Date.now(); } catch { /* Non-fatal */ } }
      } catch (err) { this.#backoffMs = Math.min(Math.max(1000, this.#backoffMs * 2 || 1000), 30000); this.#log.error('Sync error', { error: err.message, backoff_ms: this.#backoffMs }); await new Promise((r) => setTimeout(r, this.#backoffMs)); }
    }
  }

  async #processCommands() {
    const eventsJson = await this.#client.collectRoomEvents(this.#topology.exec_room_id, 1);
    // Rust equivalent: crates/mxdx-core-wasm/src/lib.rs::WasmSessionManager::process_commands
    for (const action of JSON.parse(this.#sessionMgr.processCommands(eventsJson))) await this.#executeAction(action);
  }

  async #executeAction(action) {
    switch (action.kind) {
      case 'send_event': await this.#client.sendEvent(action.room_id, action.event_type, JSON.stringify(action.content)).catch(() => {}); break;
      case 'send_state_event': await this.#client.sendStateEvent(action.room_id, action.event_type, action.state_key, JSON.stringify(action.content)).catch(() => {}); break;
      case 'write_session': await this.#client.writeSession(action.state_room_id, action.device_id, action.session_id, JSON.stringify(action.content)).catch(() => {}); break;
      case 'remove_session': await this.#client.removeSession(action.state_room_id, action.device_id, action.session_id).catch(() => {}); break;
      case 'kill_pty': { const pty = this.#ptys.get(action.session_id); if (pty?.alive) pty.kill(action.signal); break; }
      case 'spawn_pty': await this.#spawnPty(action); break;
      case 'exec_command': this.#runExecCommand(action); break;
    }
  }

  async #spawnPty(action) {
    const { session_id, request_id, command, args, cols, rows, cwd, env, batch_ms } = action;
    const cmd = command || process.env.SHELL || '/bin/bash';
    const sender = this.#sessionMgr.sessionSender(session_id);
    let dmRoomId = action.dm_room_id || this.#sessionMgr.getSessionRoomId(sender);
    if (!dmRoomId) {
      const roomKey = this.#sessionMgr.sessionRoomKey(sender);
      dmRoomId = await this.#getOrCreateDmRoom(sender, roomKey).catch(() => null);
    }
    if (!dmRoomId) { await this.#client.sendEvent(this.#topology.exec_room_id, 'org.mxdx.terminal.session', JSON.stringify({ request_id, status: 'error', room_id: null })).catch(() => {}); this.#sessionMgr.decrementActiveSessions(); return; }
    const sessionId = session_id || crypto.randomUUID().slice(0, 8);
    const pty = new PtyBridge(cmd, { cols, rows, cwd, env, useTmux: this.#config.useTmux || 'auto', socketDir: this.#socketDir });
    this.#ptys.set(sessionId, pty);
    const startedAt = Math.floor(Date.now() / 1000);
    for (const a of JSON.parse(this.#sessionMgr.onSessionStarted(sessionId, request_id, dmRoomId, pty.tmuxName || '', pty.persistent, batch_ms, sender || '', startedAt, cmd, JSON.stringify(args)))) await this.#executeAction(a);
    await this.#client.sendEvent(this.#topology.exec_room_id, 'org.mxdx.terminal.session', JSON.stringify({ request_id, status: 'started', room_id: dmRoomId, session_id: sessionId, persistent: pty.persistent, batch_ms })).catch(() => {});
    await this.#client.syncOnce();
    const transport = await this.#setupSessionTransport(dmRoomId, '', batch_ms);
    const batchSender = new BatchedSender({ sendEvent: (rId, t, c) => transport.sendEvent(rId, t, c), roomId: dmRoomId, batchMs: batch_ms, sessionId, onError: (err, seq) => this.#log.warn('terminal.data send failed', { seq, error: String(err) }) });
    pty.onData((data) => batchSender.push(data));
    if (!this.#roomMuxes.has(dmRoomId)) this.#roomMuxes.set(dmRoomId, new SessionMux(transport, dmRoomId, this.#client.userId(), this.#log));
    const mux = this.#roomMuxes.get(dmRoomId);
    mux.addSession(sessionId, pty); mux.registerSender(sessionId, batchSender);
    if (transport.status === 'p2p') batchSender.batchMs = 5;
    (async () => { while (pty.alive) await new Promise((r) => setTimeout(r, 1000)); })().finally(() => {
      mux.removeSession(sessionId); batchSender.destroy(); this.#releaseRoomTransport(dmRoomId);
      if (mux.sessionCount === 0) this.#roomMuxes.delete(dmRoomId);
      this.#ptys.delete(sessionId); this.#sessionMgr.markSessionDead(sessionId);
      if (!pty.persistent) { this.#sessionMgr.removeSession(sessionId); JSON.parse(this.#sessionMgr.onPtyExit(sessionId, 0)).forEach(a => this.#executeAction(a).catch(() => {})); }
      else pty.detach();
      this.#client.sendEvent(this.#topology.exec_room_id, 'org.mxdx.terminal.session', JSON.stringify({ request_id, status: 'ended', room_id: dmRoomId })).catch(() => {});
      this.#sessionMgr.decrementActiveSessions();
    });
  }

  #runExecCommand(action) {
    const { uuid, command, args, cwd, timeout_ms, exec_room_id } = action;
    const tail = []; const MAX_TAIL = 10;
    const sendOutput = (stream, line) => { tail.push(line); if (tail.length > MAX_TAIL) tail.shift(); return this.#client.sendEvent(exec_room_id, 'org.mxdx.session.output', JSON.stringify({ session_uuid: uuid, worker_id: this.#client.userId(), stream, data: Buffer.from(line).toString('base64'), encoding: 'base64', seq: 0, timestamp: Math.floor(Date.now() / 1000) })).catch(() => {}); };
    executeCommand(command, args, { cwd, timeoutMs: timeout_ms, onStdout: (line) => sendOutput('stdout', line), onStderr: (line) => sendOutput('stderr', line) })
      .then(async (result) => { for (const a of JSON.parse(this.#sessionMgr.onCommandComplete(uuid, exec_room_id, result.exitCode, 0, result.timedOut || false, JSON.stringify(tail), ''))) await this.#executeAction(a); })
      .catch(async (err) => { for (const a of JSON.parse(this.#sessionMgr.onCommandComplete(uuid, exec_room_id, 1, 0, false, '[]', err.message))) await this.#executeAction(a); });
  }

  async #getOrCreateDmRoom(clientUserId, roomKey) {
    const existing = this.#sessionMgr.getSessionRoomId(clientUserId);
    if (existing) { try { await this.#client.joinRoom(existing); return existing; } catch { this.#log.warn('Stale session room, creating new one', { room_id: existing }); } }
    const roomId = await this.#client.createRoom(JSON.stringify({ invite: [clientUserId], topic: `org.mxdx.launcher.sessions:${this.#config.username}:${clientUserId}`, preset: 'trusted_private_chat' }));
    this.#sessionMgr.registerSessionRoom(roomKey, roomId);
    this.#client.writeRoom(this.#stateRoomId, roomId, JSON.stringify({ room_id: roomId, room_key: roomKey, role: 'dm', joined_at: new Date().toISOString() })).catch((err) => this.#log.warn('Failed to persist session room', { room_id: roomId, error: err.message }));
    return roomId;
  }

  async #postTelemetry() {
    const nodeOs = await import('node:os');
    const level = this.#config.telemetry || 'full';
    const tmuxInfo = PtyBridge.tmuxInfo();
    const sessionPersistence = (this.#config.useTmux === 'never') ? false : (this.#config.useTmux === 'always') ? true : tmuxInfo.available;
    let preferredServer = '', preferredIdentity = '', accountsJson = '', serverHealthJson = '';
    if (this.#client.serverCount > 1) {
      preferredServer = this.#client.preferred.server; preferredIdentity = this.#client.preferred.userId; accountsJson = JSON.stringify(this.#client.allUserIds());
      const h = {}; for (const [s, health] of this.#client.serverHealth()) h[s] = { status: health.status, latency_ms: Math.round(health.latencyMs) }; serverHealthJson = JSON.stringify(h);
    }
    // Rust equivalent: crates/mxdx-core-wasm/src/lib.rs::build_telemetry_payload
    await this.#client.sendStateEvent(this.#topology.exec_room_id, 'org.mxdx.host_telemetry', '', buildTelemetryPayload(level, nodeOs.hostname(), nodeOs.platform(), nodeOs.arch(), nodeOs.cpus().length, Math.floor(nodeOs.totalmem() / (1024 * 1024)), Math.floor(nodeOs.freemem() / (1024 * 1024)), Math.floor(nodeOs.uptime()), tmuxInfo.available, tmuxInfo.version || '', sessionPersistence, this.#config.p2pEnabled !== false, this.#config.p2pAdvertiseIps ? JSON.stringify(this.#getInternalIps()) : '', preferredServer, preferredIdentity, accountsJson, serverHealthJson, 'online', this.#config.telemetryIntervalS * 1000));
  }

  async #postOfflineStatus() {
    const nodeOs = await import('node:os');
    // Rust equivalent: crates/mxdx-core-wasm/src/lib.rs::build_telemetry_payload
    await this.#client.sendStateEvent(this.#topology.exec_room_id, 'org.mxdx.host_telemetry', '', buildTelemetryPayload('summary', nodeOs.hostname(), nodeOs.platform(), nodeOs.arch(), 0, 0, 0, 0, false, '', false, false, '', '', '', '', '', 'offline', this.#config.telemetryIntervalS * 1000));
  }

  // Rust equivalent: crates/mxdx-core-wasm/src/lib.rs::SessionTransportManager
  async #setupSessionTransport(dmRoomId, remotePeer, batchMs) {
    if (this.#config.p2pEnabled === false) return { sendEvent: (rId, t, c) => this.#client.sendEvent(rId, t, c), onRoomEvent: (rId, t, timeout) => this.#client.onRoomEvent(rId, t, timeout), close: () => {} };
    const existing = this.#roomTransports.get(dmRoomId);
    if (existing) { this.#transportManager.addTransport(dmRoomId, batchMs); if (existing.transport.status !== 'p2p') { this.#transportManager.resetRateLimit(dmRoomId); this.#doAttemptP2P(dmRoomId).catch((err) => this.#log.warn('P2P reconnect on session join failed', { error: err.message, room_id: dmRoomId })); } return existing.transport; }
    this.#transportManager.addTransport(dmRoomId, batchMs);
    const sessionKey = await generateSessionKey(); const p2pCrypto = await createP2PCrypto(sessionKey);
    const transport = P2PTransport.create({ matrixClient: { sendEvent: (rId, t, c) => this.#client.sendEvent(rId, t, c), onRoomEvent: (rId, t, timeout) => this.#client.onRoomEvent(rId, t, timeout), userId: () => this.#client.userId() }, p2pCrypto, localDeviceId: this.#client.deviceId(), idleTimeoutMs: (this.#config.p2pIdleTimeoutS || 300) * 1000, onStatusChange: (status) => { this.#log.info('P2P transport status changed', { status, room_id: dmRoomId }); const ms = status === 'p2p' ? 5 : 200; this.#transportManager.setBatchMs(dmRoomId, ms); this.#roomMuxes.get(dmRoomId)?.setBatchMs(ms); }, onReconnectNeeded: () => { this.#doAttemptP2P(dmRoomId).catch((err) => this.#log.warn('P2P reconnect failed', { error: err.message, room_id: dmRoomId })); }, onHangup: (reason) => { this.#log.info('P2P hangup', { reason, room_id: dmRoomId }); } });
    this.#roomTransports.set(dmRoomId, { transport, p2pCrypto });
    this.#doAttemptP2P(dmRoomId).catch((err) => this.#log.warn('Initial P2P connection failed, continuing on Matrix', { error: err.message, room_id: dmRoomId }));
    return transport;
  }

  // Rust equivalent: crates/mxdx-core-wasm/src/lib.rs::SessionTransportManager::release_transport
  #releaseRoomTransport(roomId) {
    if (this.#transportManager.releaseTransport(roomId)) { const entry = this.#roomTransports.get(roomId); if (entry) { entry.transport.close(); this.#roomTransports.delete(roomId); } }
  }

  #doAttemptP2P(dmRoomId) {
    const entry = this.#roomTransports.get(dmRoomId);
    if (!entry) { this.#log.debug('P2P: no entry for room', { room_id: dmRoomId }); return Promise.resolve(); }
    // Rust equivalent: packages/launcher/src/p2p-bridge.js::attemptP2PConnection (OS-bound: node-datachannel)
    return attemptP2PConnection({ transport: entry.transport, transportMgr: this.#transportManager, dmRoomId, signalingRoomId: this.#topology.exec_room_id, matrixClient: this.#client, config: this.#config, log: this.#log });
  }

  #getInternalIps() {
    const nets = os.networkInterfaces(); const ips = [];
    for (const name of Object.keys(nets)) for (const net of nets[name]) if (net.family === 'IPv4' && !net.internal) ips.push(net.address);
    return ips;
  }
}
