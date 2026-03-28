#!/usr/bin/env node

import { execFileSync } from 'node:child_process';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);

const SUBCOMMANDS = {
  // Core packages
  launcher:       '@mxdx/launcher/bin/mxdx-launcher.js',
  client:         '@mxdx/client/bin/mxdx-client.js',
  'web-console':  '@mxdx/web-console/bin/mxdx-web-console.js',
  // Unified session commands — delegate to client
  run:            '@mxdx/client/bin/mxdx-client.js',
  exec:           '@mxdx/client/bin/mxdx-client.js',
  attach:         '@mxdx/client/bin/mxdx-client.js',
  ls:             '@mxdx/client/bin/mxdx-client.js',
  logs:           '@mxdx/client/bin/mxdx-client.js',
  cancel:         '@mxdx/client/bin/mxdx-client.js',
  // Worker mode
  worker:         '@mxdx/launcher/bin/mxdx-launcher.js',
  // Coordinator
  coordinator:    '@mxdx/coordinator/bin/mxdx-coordinator.js',
};

// Commands that delegate to client with the command name as a subcommand
const CLIENT_SUBCOMMANDS = new Set(['run', 'exec', 'attach', 'ls', 'logs', 'cancel']);

const HELP = `
mxdx — Matrix-native fleet management

Usage:
  mxdx <command> [options]
  mx <command> [options]

Commands:
  launcher      Start or manage a launcher/worker agent on this host
  worker        Alias for launcher (unified session mode)
  client        CLI for fleet management (exec, shell, telemetry)
  web-console   Start the browser-based management console
  coordinator   Start the fleet task coordinator

Session Commands:
  run           Submit a session task to a launcher
  exec          Alias for run (backward compatible)
  ls            List active/completed sessions
  logs          View session output logs
  attach        Attach to a running session
  cancel        Cancel a running session

Options:
  --help        Show this help message
  --version     Show version

Quickstart:
  https://github.com/Epiphytic/mxdx/blob/main/docs/quickstart.md

Examples:
  mxdx launcher start --servers http://localhost:8008
  mxdx run my-launcher echo hello
  mxdx ls my-launcher
  mxdx logs my-launcher <uuid>
  mxdx web-console --port 3000
  mx launcher start
`.trim();

const args = process.argv.slice(2);
const command = args[0];

if (!command || command === '--help' || command === '-h') {
  console.log(HELP);
  process.exit(0);
}

if (command === '--version' || command === '-v') {
  const pkg = require('../package.json');
  console.log(`mxdx v${pkg.version}`);
  process.exit(0);
}

const target = SUBCOMMANDS[command];
if (!target) {
  console.error(`Unknown command: ${command}`);
  console.error(`Run "mxdx --help" for available commands.`);
  process.exit(1);
}

let binPath;
try {
  binPath = require.resolve(target);
} catch {
  const pkgName = target.split('/bin/')[0];
  console.error(`Package not found: ${pkgName}`);
  console.error(`Install it with: npm install ${pkgName}`);
  process.exit(1);
}

const childArgs = CLIENT_SUBCOMMANDS.has(command)
    ? [command, ...args.slice(1)]  // Pass command name as first arg to client
    : args.slice(1);               // Original behavior for other commands
try {
  execFileSync(process.execPath, [binPath, ...childArgs], { stdio: 'inherit' });
} catch (err) {
  process.exit(err.status || 1);
}
