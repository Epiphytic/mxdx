#!/usr/bin/env node
import { program } from 'commander';
import { saveIndexedDB, connectWithSession, parseOlderThan, cleanupDevices, cleanupRooms, cleanupEvents, logoutAll } from '@mxdx/core';
import { LauncherConfig } from '../src/config.js';
import { LauncherRuntime } from '../src/runtime.js';
import { runOnboarding } from '../src/onboarding.js';

program
  .name('mxdx-launcher')
  .description('mxdx launcher — Matrix-native fleet management agent')
  .option('--username <name>', 'Username (default: hostname)')
  .option('--servers <url,...>', 'Comma-separated server URLs')
  .option('--registration-token <tok>', 'Auto-register with this token')
  .option('--admin-user <mxid,...>', 'Admin users to invite at PL100')
  .option('--allowed-commands <cmd,..>', 'Command allowlist')
  .option('--allowed-cwd <path,...>', 'Allowed working directories')
  .option('--config <path>', 'Config file path')
  .option('--telemetry <full|summary>', 'Telemetry detail level', 'full')
  .option('--max-sessions <n>', 'Max concurrent sessions', '5')
  .option('--password <pass>', 'Password (first run only — stored in keyring)')
  .option('--log-format <json|text>', 'Log output format', 'json')
  .option('--use-tmux <mode>', 'tmux mode: auto|always|never', 'auto')
  .option('--batch-ms <ms>', 'Terminal output batch window in ms', '200')
  .option('--p2p-enabled <bool>', 'Enable P2P transport (default: true)')
  .option('--p2p-batch-ms <ms>', 'P2P batch window in ms (default: 10)')
  .option('--p2p-idle-timeout-s <seconds>', 'P2P idle timeout in seconds (default: 300)')
  .option('--p2p-advertise-ips <bool>', 'Include internal IPs in telemetry (default: false)')
  .option('--p2p-turn-only <bool>', 'Force P2P through TURN relay only — no direct connections (default: false)');

async function resolveConfig(opts) {
  const configPath = opts.config || LauncherConfig.defaultPath();
  let config;

  config = LauncherConfig.load(configPath);

  if (!config && opts.servers) {
    config = LauncherConfig.fromArgs(opts);
    config.save(configPath);
    console.log(`[launcher] Config saved to ${configPath}`);
  }

  if (!config) {
    config = await runOnboarding();
    config.save(configPath);
    console.log(`[launcher] Config saved to ${configPath}`);
  }

  config.password = opts.password || config._password;
  config.configPath = configPath;

  if (opts.registrationToken) {
    config.registrationToken = opts.registrationToken;
  }

  config.logFormat = opts.logFormat || 'json';

  if (opts.useTmux) {
    config.useTmux = opts.useTmux;
  }

  if (opts.batchMs) {
    config.batchMs = parseInt(opts.batchMs, 10);
  }

  if (opts.p2pEnabled !== undefined) config.p2pEnabled = opts.p2pEnabled !== 'false';
  if (opts.p2pBatchMs) config.p2pBatchMs = parseInt(opts.p2pBatchMs, 10);
  if (opts.p2pIdleTimeoutS) config.p2pIdleTimeoutS = parseInt(opts.p2pIdleTimeoutS, 10);
  if (opts.p2pAdvertiseIps !== undefined) config.p2pAdvertiseIps = opts.p2pAdvertiseIps === 'true';
  if (opts.p2pTurnOnly !== undefined) config.p2pTurnOnly = opts.p2pTurnOnly === 'true';

  return config;
}

// Default command: start the launcher agent
program
  .command('start', { isDefault: true })
  .description('Start the launcher agent (default)')
  .action(async () => {
    const opts = program.opts();
    const config = await resolveConfig(opts);
    const runtime = new LauncherRuntime(config);

    async function shutdown() {
      await runtime.stop();
      try { await saveIndexedDB(config.configDir); } catch { /* best effort */ }
    }
    process.on('SIGINT', async () => {
      console.log('\n[launcher] Shutting down...');
      await shutdown();
      process.exit(0);
    });
    process.on('SIGTERM', async () => {
      await shutdown();
      process.exit(0);
    });

    await runtime.start();
  });

// Reload command — re-exec with fresh process to pick up new WASM/libraries
program
  .command('reload')
  .description('Restart the launcher with fresh modules (picks up new WASM/libraries)')
  .action(async () => {
    const launcherBin = new URL(import.meta.url).pathname;
    const passthrough = process.argv.slice(2).filter(a => a !== 'reload');

    console.log('[launcher] Reloading — spawning fresh process...');

    const { spawn } = await import('node:child_process');
    const child = spawn(process.execPath, [launcherBin, 'start', ...passthrough], {
      stdio: 'inherit',
    });

    process.on('SIGINT', () => child.kill('SIGINT'));
    process.on('SIGTERM', () => child.kill('SIGTERM'));
    child.on('exit', (code) => process.exit(code ?? 0));
  });

// Cleanup command
program
  .command('cleanup <targets>')
  .description('Clean up stale Matrix state (devices, events, rooms)')
  .option('--force-cleanup', 'Skip confirmation prompts')
  .option('--older-than <duration>', 'Only clean items older than duration (e.g. 1h, 1d, 2w, 3m)')
  .option('--delete-all-sessions', 'Log out ALL sessions and delete ALL devices (nuclear — requires re-login)')
  .action(async (targets, opts) => {
    const parentOpts = program.opts();
    const log = (msg) => console.error(`[cleanup] ${msg}`);

    // Resolve config to get server/username
    const config = await resolveConfig(parentOpts);
    const server = config.servers?.[0] || config.server;
    const username = config.username || (await import('os')).hostname();

    const { client, password } = await connectWithSession({
      username,
      server,
      password: config.password,
      registrationToken: config.registrationToken,
      configDir: config.configDir,
      useKeychain: true,
      log,
    });

    const session = JSON.parse(client.exportSession());
    const accessToken = session.access_token;
    const homeserverUrl = session.homeserver_url;
    const userId = client.userId();
    const currentDeviceId = client.deviceId();

    // Handle --delete-all-sessions (nuclear device cleanup)
    if (opts.deleteAllSessions) {
      if (!opts.forceCleanup) {
        const confirmed = await confirmPrompt('This will log out ALL sessions and delete ALL devices. You will need to re-login. Proceed?');
        if (!confirmed) {
          log('Aborted.');
          process.exit(0);
        }
      }
      await logoutAll({ accessToken, homeserverUrl, onProgress: log });
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

    const totalItems = Object.values(results).reduce((sum, r) => {
      if (r.preview) return sum + (Array.isArray(r.preview) ? r.preview.length : 0);
      return sum;
    }, 0);

    if (totalItems === 0) {
      log('Nothing to clean up.');
      process.exit(0);
    }

    if (!opts.forceCleanup) {
      const confirmed = await confirmPrompt('\nProceed with cleanup? This cannot be undone.');
      if (!confirmed) {
        log('Aborted.');
        process.exit(0);
      }
    }

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
