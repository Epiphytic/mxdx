import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { connectWithSession, TerminalDataEvent, saveIndexedDB, BatchedSender, fetchTurnCredentials, turnToIceServers, NodeWebRTCChannel, P2PSignaling, P2PTransport, generateSessionKey, createP2PCrypto } from '@mxdx/core';
import { executeCommand } from './process-bridge.js';
import { PtyBridge } from './pty-bridge.js';
import { inflateSync } from 'node:zlib';

const DEFAULT_SESSION_DIR = path.join(os.homedir(), '.mxdx');
const SESSIONS_FILE = 'sessions.json';

const MAX_DECOMPRESSED_SIZE = 1024 * 1024; // 1MB zlib bomb protection

/**
 * Structured logger with JSON and text output modes.
 */
class Logger {
  #format;

  constructor(format = 'json') {
    this.#format = format;
  }

  info(msg, data) { this.#log('info', msg, data); }
  warn(msg, data) { this.#log('warn', msg, data); }
  error(msg, data) { this.#log('error', msg, data); }
  debug(msg, data) { this.#log('debug', msg, data); }

  #log(level, msg, data) {
    if (this.#format === 'json') {
      const entry = { level, msg, ts: new Date().toISOString(), ...data };
      const stream = level === 'error' ? process.stderr : process.stdout;
      stream.write(JSON.stringify(entry) + '\n');
    } else {
      const ts = new Date().toISOString();
      const prefix = `[${level}] [${ts}]`;
      const extra = data ? ' ' + JSON.stringify(data) : '';
      const stream = level === 'error' ? process.stderr : process.stdout;
      stream.write(`${prefix} ${msg}${extra}\n`);
    }
  }
}

/**
 * The launcher runtime: connects to Matrix, creates rooms, listens for commands.
 */
export class LauncherRuntime {
  #client;
  #config;
  #topology;
  #running = false;
  #processedEvents = new Set();
  #activeSessions = 0;
  #maxSessions;
  #backoffMs = 0;
  #lastStoreSave = 0;
  #log;
  #sessionRegistry = new Map(); // sessionId -> { tmuxName, dmRoomId, sender, persistent, pty, createdAt }

  constructor(config) {
    this.#config = config;
    this.#maxSessions = config.maxSessions || 10;
    this.#log = new Logger(config.logFormat || 'json');
  }

  get #sessionDir() {
    return this.#config.sessionDir || DEFAULT_SESSION_DIR;
  }

  get #socketDir() {
    return this.#config.tmuxSocketDir || path.join(this.#sessionDir, 'tmux');
  }

  get #sessionsFilePath() {
    return path.join(this.#sessionDir, SESSIONS_FILE);
  }

  #saveSessionsFile() {
    const data = {};
    for (const [id, entry] of this.#sessionRegistry) {
      if (entry.persistent && entry.tmuxName) {
        data[id] = {
          tmuxName: entry.tmuxName,
          dmRoomId: entry.dmRoomId,
          sender: entry.sender,
          persistent: true,
          createdAt: entry.createdAt,
        };
      }
    }
    try {
      fs.mkdirSync(this.#sessionDir, { recursive: true, mode: 0o700 });
      fs.writeFileSync(this.#sessionsFilePath, JSON.stringify(data, null, 2), { mode: 0o600 });
    } catch (err) {
      this.#log.warn('Failed to save sessions file', { error: err.message });
    }
  }

  #loadSessionsFile() {
    try {
      if (!fs.existsSync(this.#sessionsFilePath)) return {};
      return JSON.parse(fs.readFileSync(this.#sessionsFilePath, 'utf8'));
    } catch {
      return {};
    }
  }

  #recoverSessions() {
    const saved = this.#loadSessionsFile();
    const liveTmux = PtyBridge.list(this.#socketDir);
    let recovered = 0;

    for (const [sessionId, entry] of Object.entries(saved)) {
      if (liveTmux.includes(entry.tmuxName)) {
        this.#sessionRegistry.set(sessionId, {
          tmuxName: entry.tmuxName,
          dmRoomId: entry.dmRoomId,
          sender: entry.sender,
          persistent: true,
          pty: null, // no attached PtyBridge yet — will attach on reconnect
          createdAt: entry.createdAt,
        });
        recovered++;
        this.#log.info('Recovered tmux session', { session_id: sessionId, tmux: entry.tmuxName });
      } else {
        this.#log.info('Stale session removed (tmux gone)', { session_id: sessionId, tmux: entry.tmuxName });
      }
    }

    // Clean up sessions file to remove stale entries
    if (recovered > 0 || Object.keys(saved).length > 0) {
      this.#saveSessionsFile();
    }

    return recovered;
  }

  async start() {
    const server = this.#config.servers[0];
    const username = this.#config.username;
    const log = (msg) => this.#log.info(msg);

    // ── 1. Connect (crypto store is persistent via IndexedDB snapshots) ──
    const { client, freshLogin, password } = await connectWithSession({
      username,
      server,
      password: this.#config.password,
      registrationToken: this.#config.registrationToken,
      configDir: this.#config.configDir,
      useKeychain: true,
      log,
    });
    this.#client = client;

    // ── 2. Remove password from config file after keyring storage ─
    if (freshLogin && this.#config.password && this.#config.configPath) {
      this.#config.password = undefined;
      this.#config._password = undefined;
      try {
        this.#config.save(this.#config.configPath);
        log('Password removed from config file (now in keyring)');
      } catch {
        // Non-fatal: config may be read-only
      }
    }

    // ── 3. Set up rooms ─────────────────────────────────────────
    log(`Setting up rooms for ${username}...`);
    this.#topology = await this.#client.getOrCreateLauncherSpace(username);
    log(`Rooms ready: space=${this.#topology.space_id} exec=${this.#topology.exec_room_id}`);

    // Invite admin users to all rooms
    if (this.#config.adminUsers && this.#config.adminUsers.length > 0) {
      log(`Inviting admin users: ${this.#config.adminUsers.join(', ')}`);
      for (const adminUser of this.#config.adminUsers) {
        for (const roomId of [
          this.#topology.space_id,
          this.#topology.exec_room_id,
          this.#topology.logs_room_id,
        ]) {
          try {
            await this.#client.inviteUser(roomId, adminUser);
          } catch {
            // May already be invited/joined
          }
        }
      }
    }

    // Recover tmux sessions from previous launcher instance
    const recovered = this.#recoverSessions();
    if (recovered > 0) {
      log(`Recovered ${recovered} tmux session(s) from previous instance`);
    }

    // Post initial telemetry
    await this.#postTelemetry();

    log('Online. Listening for commands...');
    this.#running = true;
    await this.#syncLoop();
  }

  async stop() {
    this.#running = false;

    // Detach persistent sessions so tmux sessions survive launcher restart
    for (const [id, entry] of this.#sessionRegistry) {
      if (entry.persistent && entry.pty) {
        this.#log.info('Detaching persistent session for restart survival', { sessionId: id, tmuxName: entry.tmuxName });
        entry.pty.detach();
      }
    }

    // Save session metadata for recovery on next start
    this.#saveSessionsFile();
  }

  get topology() {
    return this.#topology;
  }

  get client() {
    return this.#client;
  }

  async #syncLoop() {
    while (this.#running) {
      try {
        await Promise.race([
          this.#client.syncOnce(),
          new Promise((_, reject) =>
            setTimeout(() => reject(new Error('syncOnce timed out after 30s')), 30000),
          ),
        ]);
        await Promise.race([
          this.#processCommands(),
          new Promise((_, reject) =>
            setTimeout(() => reject(new Error('processCommands timed out after 30s')), 30000),
          ),
        ]);
        this.#backoffMs = 0;

        // Save crypto store every 5 minutes to persist new Megolm keys
        if (Date.now() - this.#lastStoreSave > 300000) {
          try {
            await saveIndexedDB(this.#config.configDir);
            this.#lastStoreSave = Date.now();
          } catch {
            // Non-fatal
          }
        }
      } catch (err) {
        this.#backoffMs = Math.min(Math.max(1000, this.#backoffMs * 2 || 1000), 30000);
        this.#log.error('Sync error', { error: err.message, backoff_ms: this.#backoffMs });
        await new Promise((r) => setTimeout(r, this.#backoffMs));
      }
    }
  }

  async #processCommands() {
    const eventsJson = await this.#client.collectRoomEvents(
      this.#topology.exec_room_id,
      1,
    );
    const events = JSON.parse(eventsJson);

    if (!events || !Array.isArray(events)) return;

    for (const event of events) {
      const eventType = event?.type;
      const eventId = event?.event_id;

      if (eventType !== 'org.mxdx.command' || !eventId) continue;
      if (this.#processedEvents.has(eventId)) continue;
      this.#processedEvents.add(eventId);

      const content = event.content || {};
      const action = content.action;
      const command = content.command;
      const args = content.args || [];
      const cwd = content.cwd || '/tmp';
      const requestId = content.request_id || eventId;
      const sender = event.sender;

      // Route session management actions
      if (action === 'list_sessions') {
        this.#log.info('Session list requested', { request_id: requestId, sender });
        await this.#handleListSessions(requestId);
        continue;
      }

      if (action === 'reconnect') {
        this.#log.info('Session reconnect requested', { request_id: requestId, sender });
        await this.#handleReconnect(content, requestId, sender);
        continue;
      }

      // Route interactive sessions
      if (action === 'interactive') {
        this.#log.info('Interactive session requested', { request_id: requestId, sender });
        await this.#handleInteractiveSession(content, requestId, sender);
        continue;
      }

      this.#log.info(`Received command: ${command} ${args.join(' ')}`, { request_id: requestId });

      // Validate command against allowlist
      if (!this.#isCommandAllowed(command)) {
        this.#log.warn(`Command rejected: ${command} not in allowlist`, { request_id: requestId });
        await this.#sendResult(requestId, {
          exit_code: 1,
          error: `Command '${command}' is not allowed`,
        });
        continue;
      }

      // Validate cwd
      if (!this.#isCwdAllowed(cwd)) {
        this.#log.warn(`CWD rejected: ${cwd}`, { request_id: requestId });
        await this.#sendResult(requestId, {
          exit_code: 1,
          error: `Working directory '${cwd}' is not allowed`,
        });
        continue;
      }

      // Check session limit
      if (this.#activeSessions >= this.#maxSessions) {
        this.#log.warn('Session limit reached', { active: this.#activeSessions, max: this.#maxSessions, request_id: requestId });
        await this.#sendResult(requestId, {
          exit_code: 1,
          error: `Session limit reached (${this.#maxSessions} max)`,
        });
        continue;
      }

      // Execute command
      this.#activeSessions++;
      try {
        const result = await executeCommand(command, args, {
          cwd,
          timeoutMs: 30000,
          onStdout: async (line) => {
            await this.#sendOutput(requestId, 'stdout', line);
          },
          onStderr: async (line) => {
            await this.#sendOutput(requestId, 'stderr', line);
          },
        });

        await this.#sendResult(requestId, {
          exit_code: result.exitCode,
          timed_out: result.timedOut,
        });
      } catch (err) {
        await this.#sendResult(requestId, {
          exit_code: 1,
          error: err.message,
        });
      } finally {
        this.#activeSessions--;
      }
    }
  }

  async #handleInteractiveSession(content, requestId, sender) {
    const defaultShell = process.env.SHELL || '/bin/bash';
    const command = content.command || defaultShell;
    const cols = content.cols || 80;
    const rows = content.rows || 24;
    const cwd = content.cwd || '/tmp';
    const env = content.env || {};

    // Negotiate batch window: use the longest of client and launcher preferences
    const clientBatchMs = content.batch_ms || 200;
    const launcherBatchMs = this.#config.batchMs || 200;
    const negotiatedBatchMs = Math.max(clientBatchMs, launcherBatchMs);
    this.#log.info('Batch window negotiated', {
      client: clientBatchMs, launcher: launcherBatchMs, negotiated: negotiatedBatchMs,
    });

    // Validate explicit command against allowlist; default shell is always permitted
    if (content.command && !this.#isCommandAllowed(command)) {
      this.#log.warn(`Interactive command rejected: ${command}`, { request_id: requestId });
      await this.#sendSessionResponse(requestId, 'rejected', null);
      return;
    }

    // Validate cwd
    if (!this.#isCwdAllowed(cwd)) {
      this.#log.warn(`Interactive CWD rejected: ${cwd}`, { request_id: requestId });
      await this.#sendSessionResponse(requestId, 'rejected', null);
      return;
    }

    // Check session limit
    if (this.#activeSessions >= this.#maxSessions) {
      this.#log.warn('Session limit reached for interactive', { request_id: requestId });
      await this.#sendSessionResponse(requestId, 'rejected', null);
      return;
    }

    if (!sender) {
      this.#log.warn('Interactive session missing sender', { request_id: requestId });
      await this.#sendSessionResponse(requestId, 'rejected', null);
      return;
    }

    this.#activeSessions++;

    try {
      // Reuse existing DM room for same sender if available, else create new
      let dmRoomId;
      const existingSession = [...this.#sessionRegistry.values()]
        .find(s => s.sender === sender && s.dmRoomId);
      if (existingSession) {
        dmRoomId = existingSession.dmRoomId;
        this.#log.info('Reusing existing DM room for interactive session', {
          request_id: requestId,
          room_id: dmRoomId,
          sender,
        });
      } else {
        dmRoomId = await this.#client.createDmRoom(sender);
        this.#log.info('Created new DM room for interactive session', {
          request_id: requestId,
          room_id: dmRoomId,
          sender,
        });
      }

      // Spawn PTY bridge with tmux support
      const sessionId = crypto.randomUUID().slice(0, 8);
      const pty = new PtyBridge(command, {
        cols, rows, cwd, env,
        useTmux: this.#config.useTmux || 'auto',
        socketDir: this.#socketDir,
      });

      this.#sessionRegistry.set(sessionId, {
        tmuxName: pty.tmuxName,
        dmRoomId,
        sender,
        persistent: pty.persistent,
        pty,
        createdAt: new Date().toISOString(),
      });
      this.#saveSessionsFile();

      // Respond with DM room ID and negotiated batch window
      await this.#sendSessionResponse(requestId, 'started', dmRoomId, {
        session_id: sessionId,
        persistent: pty.persistent,
        batch_ms: negotiatedBatchMs,
      });

      // Wait for the client to join the DM
      await new Promise((r) => setTimeout(r, 2000));
      await this.#client.syncOnce();

      // Wait for PTY to initialize
      await new Promise((r) => setTimeout(r, 500));

      // Set up transport — P2PTransport if enabled, raw Matrix client otherwise
      const transport = await this.#setupSessionTransport(dmRoomId, sender, negotiatedBatchMs);

      // Forward PTY output -> DM room as batched terminal.data events
      const batchSender = new BatchedSender({
        sendEvent: (roomId, type, content) => transport.sendEvent(roomId, type, content),
        roomId: dmRoomId,
        batchMs: negotiatedBatchMs,
        onError: (err, seq) => this.#log.warn('terminal.data send failed', { seq, error: String(err) }),
      });
      pty.onData((data) => batchSender.push(data));

      // Poll for incoming terminal data and resize events from the client
      const pollForInput = async () => {
        while (pty.alive) {
          try {
            // Check for terminal data
            const dataEventJson = await transport.onRoomEvent(
              dmRoomId,
              'org.mxdx.terminal.data',
              1,
            );
            if (dataEventJson && dataEventJson !== 'null') {
              const dataEvent = JSON.parse(dataEventJson);
              const eventContent = dataEvent.content || dataEvent;
              const eventSender = dataEvent.sender;

              // Only process events from the client (not our own output)
              if (eventSender && eventSender !== this.#client.userId()) {
                this.#processTerminalInput(eventContent, pty);
              }
            }

            // Check for resize events
            const resizeJson = await transport.onRoomEvent(
              dmRoomId,
              'org.mxdx.terminal.resize',
              1,
            );
            if (resizeJson && resizeJson !== 'null') {
              const resizeEvent = JSON.parse(resizeJson);
              const resizeContent = resizeEvent.content || resizeEvent;
              const resizeCols = resizeContent.cols;
              const resizeRows = resizeContent.rows;
              if (resizeCols && resizeRows) {
                pty.resize(resizeCols, resizeRows);
              }
            }
          } catch {
            // Sync error, retry
            await new Promise((r) => setTimeout(r, 1000));
          }
        }
      };

      // Run input polling (don't await — let it run in background)
      pollForInput().finally(() => {
        if (pty.persistent) {
          pty.detach();
          this.#log.info('Interactive session bridge detached (tmux alive)', {
            request_id: requestId,
            session_id: sessionId,
          });
        } else {
          this.#sessionRegistry.delete(sessionId);
          this.#saveSessionsFile();
          this.#log.info('Interactive session ended', { request_id: requestId, session_id: sessionId });
        }
        this.#sendSessionResponse(requestId, 'ended', dmRoomId).catch(() => {});
        this.#activeSessions--;
      });

      // Don't decrement activeSessions here — the finally block above handles it
      return;
    } catch (err) {
      this.#log.error('Interactive session failed', { request_id: requestId, error: err.message || String(err), stack: err.stack });
      await this.#sendSessionResponse(requestId, 'error', null);
      this.#activeSessions--;
    }
  }

  async #handleListSessions(requestId) {
    const sessions = [];
    for (const [sessionId, entry] of this.#sessionRegistry) {
      // Check if tmux session is still alive
      const alive = entry.pty?.alive ?? (entry.tmuxName ? PtyBridge.list(this.#socketDir).includes(entry.tmuxName) : false);
      sessions.push({
        session_id: sessionId,
        room_id: entry.dmRoomId,
        persistent: entry.persistent,
        tmux_name: entry.tmuxName || null,
        alive,
        created_at: entry.createdAt,
      });
    }
    await this.#client.sendEvent(
      this.#topology.exec_room_id,
      'org.mxdx.terminal.sessions',
      JSON.stringify({ request_id: requestId, sessions }),
    );
  }

  async #handleReconnect(content, requestId, sender) {
    const sessionId = content.session_id;
    const cols = content.cols || 80;
    const rows = content.rows || 24;

    const entry = this.#sessionRegistry.get(sessionId);
    if (!entry || !entry.persistent) {
      await this.#sendSessionResponse(requestId, 'expired', null);
      return;
    }

    if (entry.sender !== sender) {
      await this.#sendSessionResponse(requestId, 'rejected', null);
      return;
    }

    try {
      const pty = new PtyBridge('bash', {
        cols, rows,
        sessionName: entry.tmuxName,
        useTmux: 'always',
        socketDir: this.#socketDir,
      });

      entry.pty = pty;
      this.#activeSessions++;

      await this.#sendSessionResponse(requestId, 'reconnected', entry.dmRoomId, {
        session_id: sessionId,
        persistent: true,
      });

      await new Promise((r) => setTimeout(r, 2000));
      await this.#client.syncOnce();

      // Set up transport — P2PTransport if enabled, raw Matrix client otherwise
      const reconnectTransport = await this.#setupSessionTransport(entry.dmRoomId, sender, this.#config.batchMs || 200);

      // Forward PTY output -> DM room as batched terminal.data events
      const reconnectSender = new BatchedSender({
        sendEvent: (roomId, type, content) => reconnectTransport.sendEvent(roomId, type, content),
        roomId: entry.dmRoomId,
        batchMs: this.#config.batchMs || 200,
        onError: (err, seq) => this.#log.warn('terminal.data send failed', { seq, error: String(err) }),
      });
      pty.onData((data) => reconnectSender.push(data));

      // Poll for input from client
      const pollForInput = async () => {
        while (pty.alive) {
          try {
            const dataEventJson = await reconnectTransport.onRoomEvent(
              entry.dmRoomId, 'org.mxdx.terminal.data', 1,
            );
            if (dataEventJson && dataEventJson !== 'null') {
              const dataEvent = JSON.parse(dataEventJson);
              const eventContent = dataEvent.content || dataEvent;
              const eventSender = dataEvent.sender;
              if (eventSender && eventSender !== this.#client.userId()) {
                this.#processTerminalInput(eventContent, pty);
              }
            }
            const resizeJson = await reconnectTransport.onRoomEvent(
              entry.dmRoomId, 'org.mxdx.terminal.resize', 1,
            );
            if (resizeJson && resizeJson !== 'null') {
              const resizeEvent = JSON.parse(resizeJson);
              const resizeContent = resizeEvent.content || resizeEvent;
              if (resizeContent.cols && resizeContent.rows) {
                pty.resize(resizeContent.cols, resizeContent.rows);
              }
            }
          } catch {
            await new Promise((r) => setTimeout(r, 1000));
          }
        }
      };

      pollForInput().finally(() => {
        if (pty.persistent) {
          pty.detach();
          this.#log.info('Reconnected session bridge detached', { session_id: sessionId });
        } else {
          this.#sessionRegistry.delete(sessionId);
          this.#saveSessionsFile();
        }
        this.#sendSessionResponse(requestId, 'ended', entry.dmRoomId).catch(() => {});
        this.#activeSessions--;
      });
    } catch (err) {
      this.#log.error('Reconnect failed', { session_id: sessionId, error: err.message });
      await this.#sendSessionResponse(requestId, 'expired', null);
    }
  }

  #processTerminalInput(content, pty) {
    const parsed = TerminalDataEvent.safeParse(content);
    if (!parsed.success) return;

    const { data, encoding } = parsed.data;
    const raw = Buffer.from(data, 'base64');

    if (encoding === 'zlib+base64') {
      try {
        const decompressed = inflateSync(raw, { maxOutputLength: MAX_DECOMPRESSED_SIZE });
        pty.write(new Uint8Array(decompressed));
      } catch {
        // Decompression failed or exceeded size limit (zlib bomb protection)
      }
    } else {
      pty.write(new Uint8Array(raw));
    }
  }

  async #sendSessionResponse(requestId, status, roomId, extra = {}) {
    await Promise.race([
      this.#client.sendEvent(
        this.#topology.exec_room_id,
        'org.mxdx.terminal.session',
        JSON.stringify({
          request_id: requestId,
          status,
          room_id: roomId,
          ...extra,
        }),
      ),
      new Promise((_, reject) =>
        setTimeout(() => reject(new Error('sendSessionResponse timed out after 30s')), 30000),
      ),
    ]);
  }

  #isCommandAllowed(command) {
    if (this.#config.allowedCommands.length === 0) return false;
    return this.#config.allowedCommands.includes(command);
  }

  #isCwdAllowed(cwd) {
    return this.#config.allowedCwd.some((allowed) => cwd.startsWith(allowed));
  }

  async #sendOutput(requestId, stream, line) {
    try {
      await this.#client.sendEvent(
        this.#topology.exec_room_id,
        'org.mxdx.output',
        JSON.stringify({
          request_id: requestId,
          stream,
          data: Buffer.from(line).toString('base64'),
        }),
      );
    } catch {
      // Best effort — don't stop execution on send failure
    }
  }

  async #sendResult(requestId, result) {
    await this.#client.sendEvent(
      this.#topology.exec_room_id,
      'org.mxdx.result',
      JSON.stringify({
        request_id: requestId,
        ...result,
      }),
    );
  }

  async #postTelemetry() {
    const os = await import('node:os');
    const level = this.#config.telemetry || 'full';

    const telemetry = {
      hostname: os.hostname(),
      platform: os.platform(),
      arch: os.arch(),
    };

    if (level === 'full') {
      telemetry.cpus = os.cpus().length;
      telemetry.total_memory_mb = Math.floor(os.totalmem() / (1024 * 1024));
      telemetry.free_memory_mb = Math.floor(os.freemem() / (1024 * 1024));
      telemetry.uptime_secs = Math.floor(os.uptime());
    }

    const tmuxInfo = PtyBridge.tmuxInfo();
    telemetry.tmux_available = tmuxInfo.available;
    if (tmuxInfo.version) telemetry.tmux_version = tmuxInfo.version;
    telemetry.session_persistence =
      (this.#config.useTmux === 'never') ? false :
      (this.#config.useTmux === 'always') ? true :
      tmuxInfo.available;

    // P2P capability advertisement
    telemetry.p2p = {
      enabled: this.#config.p2pEnabled !== false,
    };
    // Internal IPs only when explicitly enabled — state events persist indefinitely
    if (this.#config.p2pAdvertiseIps) {
      telemetry.p2p.internal_ips = this.#getInternalIps();
    }

    await this.#client.sendStateEvent(
      this.#topology.exec_room_id,
      'org.mxdx.host_telemetry',
      '',
      JSON.stringify(telemetry),
    );
  }

  /**
   * Set up transport for a terminal session.
   * Returns P2PTransport (with Matrix fallback) when P2P is enabled,
   * or a thin Matrix client wrapper when P2P is disabled.
   */
  async #setupSessionTransport(dmRoomId, remotePeer, batchMs) {
    // When P2P is disabled, return raw Matrix client interface
    if (this.#config.p2pEnabled === false) {
      return {
        sendEvent: (roomId, type, content) => this.#client.sendEvent(roomId, type, content),
        onRoomEvent: (roomId, type, timeout) => this.#client.onRoomEvent(roomId, type, timeout),
        close: () => {},
      };
    }

    const idleTimeoutMs = (this.#config.p2pIdleTimeoutS || 300) * 1000;
    const p2pBatchMs = this.#config.p2pBatchMs || 10;

    // Generate session key for P2P encryption (sent via E2EE Matrix signaling)
    const sessionKey = await generateSessionKey();
    const p2pCrypto = await createP2PCrypto(sessionKey);

    // Create P2PTransport with Matrix fallback
    const transport = P2PTransport.create({
      matrixClient: {
        sendEvent: (roomId, type, content) => this.#client.sendEvent(roomId, type, content),
        onRoomEvent: (roomId, type, timeout) => this.#client.onRoomEvent(roomId, type, timeout),
        userId: () => this.#client.userId(),
      },
      p2pCrypto,
      localDeviceId: this.#client.deviceId(),
      idleTimeoutMs,
      onStatusChange: (status) => {
        this.#log.info('P2P transport status changed', { status, room_id: dmRoomId });
      },
      onReconnectNeeded: () => {
        // Attempt to re-establish P2P in background
        this.#attemptP2PConnection(transport, dmRoomId, remotePeer).catch((err) => {
          this.#log.warn('P2P reconnect failed', { error: err.message, room_id: dmRoomId });
        });
      },
      onHangup: (reason) => {
        this.#log.info('P2P hangup', { reason, room_id: dmRoomId });
      },
    });

    // Attempt P2P connection in background — session works on Matrix immediately
    this.#attemptP2PConnection(transport, dmRoomId, remotePeer).catch((err) => {
      this.#log.warn('Initial P2P connection failed, continuing on Matrix', {
        error: err.message, room_id: dmRoomId,
      });
    });

    return transport;
  }

  /**
   * Attempt to establish a P2P WebRTC connection for a session.
   * Non-blocking — the terminal session works on Matrix while this runs.
   */
  async #attemptP2PConnection(transport, dmRoomId, remotePeer) {
    // Fetch TURN credentials from homeserver
    const session = JSON.parse(this.#client.exportSession());
    const server = session.homeserver_url;
    const accessToken = session.access_token;
    let iceServers = [];

    const turnCreds = await fetchTurnCredentials(server, accessToken);
    if (turnCreds) {
      iceServers = turnToIceServers(turnCreds);
    }

    // Create WebRTC channel and signaling
    const channel = new NodeWebRTCChannel({ iceServers });
    const signaling = new P2PSignaling(
      {
        sendEvent: (roomId, type, content) => this.#client.sendEvent(roomId, type, content),
        onRoomEvent: (roomId, cb) => this.#client.onRoomEvent(roomId, cb),
      },
      dmRoomId,
      this.#client.userId(),
    );

    const callId = P2PSignaling.generateCallId();
    const partyId = P2PSignaling.generatePartyId();

    // Collect ICE candidates for batched sending
    const candidates = [];
    let candidateTimer = null;
    channel.onIceCandidate((candidate) => {
      candidates.push(candidate);
      if (candidateTimer) clearTimeout(candidateTimer);
      candidateTimer = setTimeout(async () => {
        const batch = candidates.splice(0);
        if (batch.length > 0) {
          await signaling.sendCandidates({ callId, partyId, candidates: batch }).catch(() => {});
        }
      }, 100);
    });

    // Create offer and send invite
    const offer = await channel.createOffer();
    await signaling.sendInvite({ callId, partyId, sdp: offer.sdp, lifetime: 30000 });

    // Listen for answer (poll with timeout)
    const answerJson = await this.#client.onRoomEvent(dmRoomId, 'm.call.answer', 30);
    if (!answerJson || answerJson === 'null') {
      channel.close();
      throw new Error('No P2P answer received within timeout');
    }

    const answerEvent = JSON.parse(answerJson);
    const answerContent = answerEvent.content || answerEvent;
    if (answerContent.call_id !== callId) {
      channel.close();
      throw new Error('Answer call_id mismatch');
    }

    await channel.acceptAnswer({ sdp: answerContent.answer.sdp, type: answerContent.answer.type });

    // Poll for remote ICE candidates in background
    const pollCandidates = async () => {
      for (let i = 0; i < 30; i++) { // Poll for up to 30 seconds
        const candJson = await this.#client.onRoomEvent(dmRoomId, 'm.call.candidates', 1);
        if (!candJson || candJson === 'null') continue;
        try {
          const candEvent = JSON.parse(candJson);
          const candContent = candEvent.content || candEvent;
          if (candContent.call_id !== callId) continue;
          for (const c of (candContent.candidates || [])) {
            channel.addIceCandidate(c);
          }
        } catch { /* malformed candidate event */ }
      }
    };
    pollCandidates().catch(() => {});

    // Wait for data channel to open
    await channel.waitForDataChannel();

    // Attach channel to transport — triggers peer verification flow
    transport.setDataChannel(channel);
    this.#log.info('P2P data channel established', { room_id: dmRoomId, call_id: callId });
  }

  #getInternalIps() {
    const nets = os.networkInterfaces();
    const ips = [];
    for (const name of Object.keys(nets)) {
      for (const net of nets[name]) {
        if (net.family === 'IPv4' && !net.internal) ips.push(net.address);
      }
    }
    return ips;
  }
}
