#!/usr/bin/env node
import { program } from 'commander';
import { connectWithSession } from '@mxdx/core';
import { ClientConfig } from '../src/config.js';
import { findLauncher } from '../src/discovery.js';
import { execCommand } from '../src/exec.js';

program
  .name('mxdx-client')
  .description('mxdx client — interactive fleet management')
  .option('--server <url>', 'Matrix server URL')
  .option('--username <name>', 'Username')
  .option('--password <pass>', 'Password (first run only — stored in keyring)')
  .option('--registration-token <tok>', 'Registration token')
  .option('--format <text|json>', 'Output format', 'text')
  .option('--config <path>', 'Config file path');

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

program.parse();

// If no command given, show help
if (!process.argv.slice(2).length) {
  program.help();
}

async function connect(opts) {
  const configPath = opts.config || ClientConfig.defaultPath();
  let config = ClientConfig.load(configPath);

  const server = opts.server || config?.server;
  const username = opts.username || config?.username;
  const password = opts.password || config?.password;

  if (!server || !username) {
    console.error('Required: --server, --username (password will be prompted if not in keyring)');
    process.exit(1);
  }

  const log = (msg) => console.error(`[client] ${msg}`);

  const result = await connectWithSession({
    username,
    server,
    password,
    registrationToken: opts.registrationToken,
    useKeychain: true,
    log,
  });

  // Save config for future use (without password)
  if (!config) {
    config = new ClientConfig({ username, server });
    config.save(configPath);
  }

  return result;
}
