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
    await client.syncOnce();
    console.log('Use: mxdx-client exec <launcher-name> <command>');
    console.log('Launcher names match the --username used at launcher startup.');
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

    // Read telemetry state event
    const events = JSON.parse(await client.collectRoomEvents(topology.exec_room_id, 3));
    if (events && events.length > 0) {
      for (const event of events) {
        if (event.type === 'org.mxdx.host_telemetry') {
          const t = event.content;
          console.log(`  Hostname:  ${t.hostname}`);
          console.log(`  Platform:  ${t.platform} (${t.arch})`);
          console.log(`  CPUs:      ${t.cpus}`);
          console.log(`  Memory:    ${t.free_memory_mb}MB free / ${t.total_memory_mb}MB total`);
          console.log(`  Uptime:    ${Math.floor(t.uptime_secs / 3600)}h`);
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
