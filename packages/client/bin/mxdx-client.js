#!/usr/bin/env node
import { program } from 'commander';
import { connectWithSession, parseOlderThan, cleanupDevices, cleanupRooms, cleanupEvents, logoutAll } from '@mxdx/core';
import { ClientConfig } from '../src/config.js';
import { findLauncher } from '../src/discovery.js';
import { execCommand } from '../src/exec.js';
import { startInteractiveSession } from '../src/interactive.js';

program
  .name('mxdx-client')
  .description('mxdx client — interactive fleet management')
  .option('--server <url>', 'Matrix server URL')
  .option('--username <name>', 'Username')
  .option('--password <pass>', 'Password (first run only — stored in keyring)')
  .option('--registration-token <tok>', 'Registration token')
  .option('--format <text|json>', 'Output format', 'text')
  .option('--config <path>', 'Config file path')
  .option('--batch-ms <ms>', 'Terminal output batch window in ms', '200')
  .option('--p2p-enabled <bool>', 'Enable P2P transport (default: true)')
  .option('--p2p-batch-ms <ms>', 'P2P batch window in ms (default: 10)')
  .option('--p2p-idle-timeout-s <seconds>', 'P2P idle timeout in seconds (default: 300)');

program
  .command('exec <launcher> [cmd...]')
  .description('Execute a command on a launcher')
  .option('--cwd <path>', 'Working directory', '/tmp')
  .action(async (launcher, cmd, opts) => {
    const parentOpts = program.opts();
    const { client } = await connect(parentOpts);

    const topology = await findLauncher(client, launcher);
    if (!topology) {
      console.error(`Launcher '${launcher}' not found`);
      process.exit(1);
    }

    const command = cmd[0];
    const args = cmd.slice(1);

    const result = await execCommand(client, topology, command, args, {
      cwd: opts.cwd,
      format: parentOpts.format,
    });

    if (parentOpts.format === 'json') {
      console.log(JSON.stringify(result, null, 2));
    } else if (result.error) {
      console.error(`Error: ${result.error}`);
    }

    process.exit(result.exitCode);
  });

program
  .command('shell <launcher> [command]')
  .description('Start an interactive terminal session on a launcher')
  .option('--cols <n>', 'Terminal columns (default: current terminal width)')
  .option('--rows <n>', 'Terminal rows (default: current terminal height)')
  .action(async (launcher, command, opts) => {
    const parentOpts = program.opts();
    const { client } = await connect(parentOpts);

    const topology = await findLauncher(client, launcher);
    if (!topology) {
      console.error(`Launcher '${launcher}' not found`);
      process.exit(1);
    }

    const log = (msg) => console.error(`[shell] ${msg}`);

    try {
      await startInteractiveSession(client, topology, {
        command: command || '/bin/bash',
        cols: opts.cols ? parseInt(opts.cols, 10) : undefined,
        rows: opts.rows ? parseInt(opts.rows, 10) : undefined,
        batchMs: parseInt(parentOpts.batchMs, 10),
        log,
      });
    } catch (err) {
      console.error(`Shell session failed: ${err.message}`);
      process.exit(1);
    }
  });

program
  .command('verify <user_id>')
  .description('Cross-sign verify another user by their Matrix ID')
  .action(async (userId) => {
    const opts = program.opts();
    const { client } = await connect(opts);
    await client.syncOnce();

    try {
      await client.verifyUser(userId);
      console.log(`Verified ${userId}`);

      const isVerified = await client.isUserVerified(userId);
      console.log(`Verification status: ${isVerified ? 'verified' : 'not verified'}`);
    } catch (err) {
      console.error(`Verification failed: ${err}`);
      process.exit(1);
    }
  });

program
  .command('launchers')
  .description('List discovered launchers')
  .action(async () => {
    const opts = program.opts();
    const { client } = await connect(opts);

    const launchersJson = await client.listLauncherSpaces();
    const launchers = JSON.parse(launchersJson);

    if (opts.format === 'json') {
      console.log(JSON.stringify(launchers, null, 2));
      return;
    }

    if (launchers.length === 0) {
      console.log('No launchers discovered.');
      return;
    }

    for (const l of launchers) {
      console.log(`  ${l.launcher_id}`);
      console.log(`    Space: ${l.space_id}`);
      console.log(`    Exec:  ${l.exec_room_id}`);
      console.log(`    Logs:  ${l.logs_room_id}`);
    }
  });

program
  .command('telemetry <launcher>')
  .description('Show host telemetry for a launcher')
  .action(async (launcher) => {
    const opts = program.opts();
    const { client } = await connect(opts);

    const topology = await findLauncher(client, launcher);
    if (!topology) {
      console.error(`Launcher '${launcher}' not found`);
      process.exit(1);
    }

    // Read telemetry state event from exec room
    const events = JSON.parse(await client.collectRoomEvents(topology.exec_room_id, 3));
    if (events && events.length > 0) {
      for (const event of events) {
        if (event.type === 'org.mxdx.host_telemetry') {
          const t = event.content;
          if (opts.format === 'json') {
            console.log(JSON.stringify(t, null, 2));
          } else {
            console.log(`  Hostname:  ${t.hostname}`);
            console.log(`  Platform:  ${t.platform} (${t.arch})`);
            if (t.cpus != null) console.log(`  CPUs:      ${t.cpus}`);
            if (t.total_memory_mb != null) console.log(`  Memory:    ${t.free_memory_mb}MB free / ${t.total_memory_mb}MB total`);
            if (t.uptime_secs != null) console.log(`  Uptime:    ${Math.floor(t.uptime_secs / 3600)}h`);
          }
          return;
        }
      }
    }
    console.log('No telemetry data available');
  });

program
  .command('cleanup <targets>')
  .description('Clean up stale Matrix state (devices, events, rooms)')
  .option('--force-cleanup', 'Skip confirmation prompts')
  .option('--older-than <duration>', 'Only clean items older than duration (e.g. 1h, 1d, 2w, 3m)')
  .option('--delete-all-sessions', 'Log out ALL sessions and delete ALL devices (nuclear — requires re-login)')
  .action(async (targets, opts) => {
    const parentOpts = program.opts();
    const log = (msg) => console.error(`[cleanup] ${msg}`);
    const result = await connect(parentOpts);
    const { client, password } = result;

    // Handle --delete-all-sessions (nuclear device cleanup)
    if (opts.deleteAllSessions) {
      const session = JSON.parse(client.exportSession());
      if (!opts.forceCleanup) {
        const confirmed = await confirmPrompt('This will log out ALL sessions and delete ALL devices. You will need to re-login. Proceed?');
        if (!confirmed) {
          log('Aborted.');
          process.exit(0);
        }
      }
      await logoutAll({
        accessToken: session.access_token,
        homeserverUrl: session.homeserver_url,
        onProgress: log,
      });
      process.exit(0);
    }

    const validTargets = ['devices', 'events', 'rooms'];
    const targetList = targets.split(',').map(t => t.trim()).filter(Boolean);

    for (const t of targetList) {
      if (!validTargets.includes(t)) {
        console.error(`Invalid target: '${t}'. Valid targets: ${validTargets.join(', ')}`);
        process.exit(1);
      }
    }

    const olderThan = parseOlderThan(opts.olderThan);

    const session = JSON.parse(client.exportSession());
    const accessToken = session.access_token;
    const homeserverUrl = session.homeserver_url;
    const userId = client.userId();
    const currentDeviceId = client.deviceId();

    let launchersJson;
    if (targetList.includes('events') || targetList.includes('rooms')) {
      launchersJson = await client.listLauncherSpaces();
    }

    // Preview phase
    const results = {};
    for (const target of targetList) {
      try {
        if (target === 'devices') {
          results.devices = await cleanupDevices({
            accessToken, homeserverUrl, currentDeviceId, userId, password,
            olderThan, onProgress: log,
          });
          log(`\nDevices to delete (${results.devices.preview.length}):`);
          for (const d of results.devices.preview) {
            const ts = d.last_seen_ts ? new Date(d.last_seen_ts).toISOString() : 'unknown';
            log(`  ${d.device_id} — ${d.display_name} (last seen: ${ts})`);
          }
        } else if (target === 'events') {
          results.events = await cleanupEvents({
            accessToken, homeserverUrl, launchersJson, userId,
            olderThan, onProgress: log,
          });
          log(`\nEvents to redact:`);
          for (const r of results.events.preview) {
            log(`  ${r.type} room for ${r.launcher_id}: ${r.event_count} event(s)`);
          }
        } else if (target === 'rooms') {
          results.rooms = await cleanupRooms({
            accessToken, homeserverUrl, launchersJson,
            olderThan, onProgress: log,
          });
          log(`\nRooms to leave+forget (${results.rooms.preview.length}):`);
          for (const r of results.rooms.preview) {
            log(`  ${r.type} — ${r.launcher_id} (${r.room_id})`);
          }
        }
      } catch (err) {
        console.error(`Error previewing ${target}: ${err.message}`);
        process.exit(1);
      }
    }

    // Check if there's anything to do
    const totalItems = Object.values(results).reduce((sum, r) => {
      if (r.preview) return sum + (Array.isArray(r.preview) ? r.preview.length : 0);
      return sum;
    }, 0);

    if (totalItems === 0) {
      log('Nothing to clean up.');
      process.exit(0);
    }

    // Confirmation
    if (!opts.forceCleanup) {
      const confirmed = await confirmPrompt('\nProceed with cleanup? This cannot be undone.');
      if (!confirmed) {
        log('Aborted.');
        process.exit(0);
      }
    }

    // Execute phase
    for (const target of targetList) {
      if (results[target]) {
        try {
          const outcome = await results[target].execute();
          log(`${target}: ${JSON.stringify(outcome)}`);
        } catch (err) {
          console.error(`Error executing ${target} cleanup: ${err.message}`);
        }
      }
    }

    process.exit(0);
  });

program.parse();

// If no command given, show help
if (!process.argv.slice(2).length) {
  program.help();
}

async function confirmPrompt(message) {
  if (!process.stdin.isTTY) return false;
  const { createInterface } = await import('node:readline/promises');
  const rl = createInterface({ input: process.stdin, output: process.stderr });
  try {
    const answer = await rl.question(`${message} (y/N) `);
    return answer.trim().toLowerCase() === 'y';
  } finally {
    rl.close();
  }
}

async function connect(opts) {
  const configPath = opts.config || ClientConfig.defaultPath();
  let config = ClientConfig.load(configPath);

  // Build servers list: CLI --servers flag → config → CLI --server flag
  let servers;
  if (opts.servers) {
    servers = opts.servers.split(',');
  } else if (config?.servers?.length) {
    servers = config.servers;
  } else if (opts.server) {
    servers = [opts.server];
  } else if (config?.server) {
    servers = [config.server];
  } else {
    servers = [];
  }

  const username = opts.username || config?.username;
  let password = opts.password || config?.password;

  if (!servers.length || !username) {
    console.error('Required: --server(s), --username (password will be prompted if not in keyring)');
    process.exit(1);
  }

  const log = (msg) => console.error(`[client] ${msg}`);

  let client;
  if (servers.length > 1) {
    const { MultiHsClient } = await import('@mxdx/core');
    const configs = servers.map(server => {
      const creds = config?.serverCredentials?.[server] || {};
      return {
        username: creds.username || username,
        server,
        password: creds.password || password,
        registrationToken: opts.registrationToken,
        useKeychain: true,
        log,
      };
    });
    client = await MultiHsClient.connect(configs, {
      preferredServer: opts.preferredServer || config?.preferredServer,
      log,
    });
  } else {
    const result = await connectWithSession({
      username,
      server: servers[0],
      password,
      registrationToken: opts.registrationToken,
      useKeychain: true,
      log,
    });
    client = result.client;
    password = result.password;
  }

  // Save config for future use (without password)
  if (!config) {
    config = new ClientConfig({ username, servers });
    config.save(configPath);
  }

  return { client, password };
}
