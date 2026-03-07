import { connectWithSession } from '@mxdx/core';
import { executeCommand } from './process-bridge.js';

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
  #log;

  constructor(config) {
    this.#config = config;
    this.#maxSessions = config.maxSessions || 10;
    this.#log = new Logger(config.logFormat || 'json');
  }

  async start() {
    const server = this.#config.servers[0];
    const username = this.#config.username;
    const log = (msg) => this.#log.info(msg);

    // ── 1. Connect with session persistence + cross-signing ─────
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

    // Post initial telemetry
    await this.#postTelemetry();

    log('Online. Listening for commands...');
    this.#running = true;
    await this.#syncLoop();
  }

  async stop() {
    this.#running = false;
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
        await this.#client.syncOnce();
        await this.#processCommands();
        this.#backoffMs = 0;
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
      const command = content.command;
      const args = content.args || [];
      const cwd = content.cwd || '/tmp';
      const requestId = content.request_id || eventId;

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

    await this.#client.sendStateEvent(
      this.#topology.exec_room_id,
      'org.mxdx.host_telemetry',
      '',
      JSON.stringify(telemetry),
    );
  }
}
