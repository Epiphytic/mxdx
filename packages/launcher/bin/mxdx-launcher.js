#!/usr/bin/env node
import { program } from 'commander';
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
  .option('--password <pass>', 'Password (for automated registration)')
  .parse();

const opts = program.opts();

async function main() {
  const configPath = opts.config || LauncherConfig.defaultPath();
  let config;

  // Try loading existing config
  config = LauncherConfig.load(configPath);

  if (!config && opts.servers) {
    // Create from CLI args
    config = LauncherConfig.fromArgs(opts);
    config.save(configPath);
    console.log(`[launcher] Config saved to ${configPath}`);
  }

  if (!config) {
    // Interactive onboarding
    config = await runOnboarding();
    config.save(configPath);
    console.log(`[launcher] Config saved to ${configPath}`);
  }

  // Password from CLI args or onboarding
  config.password = opts.password || config._password;
  if (!config.password) {
    console.error('[launcher] Password required. Use --password or run interactive onboarding.');
    process.exit(1);
  }

  // Registration token from CLI
  if (opts.registrationToken) {
    config.registrationToken = opts.registrationToken;
  }

  const runtime = new LauncherRuntime(config);

  // Graceful shutdown
  process.on('SIGINT', async () => {
    console.log('\n[launcher] Shutting down...');
    await runtime.stop();
    process.exit(0);
  });
  process.on('SIGTERM', async () => {
    await runtime.stop();
    process.exit(0);
  });

  await runtime.start();
}

main().catch((err) => {
  console.error('[launcher] Fatal error:', err);
  process.exit(1);
});
