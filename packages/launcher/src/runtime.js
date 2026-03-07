import { WasmMatrixClient } from '@mxdx/core';
import { executeCommand } from './process-bridge.js';
import { CredentialStore } from './credentials.js';

/**
 * The launcher runtime: connects to Matrix, creates rooms, listens for commands.
 */
export class LauncherRuntime {
  #client;
  #config;
  #topology;
  #credentialStore;
  #running = false;
  #processedEvents = new Set();

  constructor(config) {
    this.#config = config;
    this.#credentialStore = new CredentialStore({
      configDir: config.configDir,
      useKeychain: false,
    });
  }

  async start() {
    console.log(`[launcher] Connecting to ${this.#config.servers[0]}...`);

    // Connect to the first server
    const server = this.#config.servers[0];
    if (this.#config.registrationToken) {
      this.#client = await WasmMatrixClient.register(
        server,
        this.#config.username,
        this.#config.password,
        this.#config.registrationToken,
      );
    } else {
      this.#client = await WasmMatrixClient.login(
        server,
        this.#config.username,
        this.#config.password,
      );
    }

    console.log(`[launcher] Logged in as ${this.#client.userId()}`);

    // Save credentials for reconnection
    await this.#credentialStore.save({
      serverUrl: server,
      username: this.#config.username,
      accessToken: 'stored-via-sdk',
      deviceId: 'stored-via-sdk',
    });

    // Get or create launcher space
    console.log(`[launcher] Setting up rooms for ${this.#config.username}...`);
    this.#topology = await this.#client.getOrCreateLauncherSpace(this.#config.username);
    console.log(`[launcher] Rooms ready:`, {
      space: this.#topology.space_id,
      exec: this.#topology.exec_room_id,
      status: this.#topology.status_room_id,
      logs: this.#topology.logs_room_id,
    });

    // Invite admin users to all rooms
    if (this.#config.adminUsers && this.#config.adminUsers.length > 0) {
      console.log(`[launcher] Inviting admin users: ${this.#config.adminUsers.join(', ')}`);
      for (const adminUser of this.#config.adminUsers) {
        for (const roomId of [
          this.#topology.space_id,
          this.#topology.exec_room_id,
          this.#topology.status_room_id,
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

    console.log(`[launcher] Online. Listening for commands...`);
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
      } catch (err) {
        console.error(`[launcher] Sync error:`, err);
        // Continue syncing — transient errors shouldn't stop the launcher
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

      console.log(`[launcher] Received command: ${command} ${args.join(' ')}`);

      // Validate command against allowlist
      if (!this.#isCommandAllowed(command)) {
        console.log(`[launcher] Command rejected: ${command} not in allowlist`);
        await this.#sendResult(requestId, {
          exit_code: 1,
          error: `Command '${command}' is not allowed`,
        });
        continue;
      }

      // Validate cwd
      if (!this.#isCwdAllowed(cwd)) {
        console.log(`[launcher] CWD rejected: ${cwd} not in allowed paths`);
        await this.#sendResult(requestId, {
          exit_code: 1,
          error: `Working directory '${cwd}' is not allowed`,
        });
        continue;
      }

      // Execute command
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
    const telemetry = {
      hostname: os.hostname(),
      platform: os.platform(),
      arch: os.arch(),
      cpus: os.cpus().length,
      total_memory_mb: Math.floor(os.totalmem() / (1024 * 1024)),
      free_memory_mb: Math.floor(os.freemem() / (1024 * 1024)),
      uptime_secs: Math.floor(os.uptime()),
    };

    await this.#client.sendStateEvent(
      this.#topology.status_room_id,
      'org.mxdx.host_telemetry',
      '',
      JSON.stringify(telemetry),
    );
  }
}
