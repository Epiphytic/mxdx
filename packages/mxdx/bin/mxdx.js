#!/usr/bin/env node

import { execFileSync } from 'node:child_process';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);

const SUBCOMMANDS = {
  launcher:       '@mxdx/launcher/bin/mxdx-launcher.js',
  client:         '@mxdx/client/bin/mxdx-client.js',
  'web-console':  '@mxdx/web-console/bin/mxdx-web-console.js',
};

const HELP = `
mxdx — Matrix-native fleet management

Usage:
  mxdx <command> [options]
  mx <command> [options]

Commands:
  launcher      Start or manage a launcher agent on this host
  client        CLI for fleet management (exec, shell, telemetry)
  web-console   Start the browser-based management console

Options:
  --help        Show this help message
  --version     Show version

Quickstart:
  https://github.com/Epiphytic/mxdx/blob/main/docs/quickstart.md

Examples:
  mxdx launcher start --servers http://localhost:8008
  mxdx client exec my-launcher echo hello
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

const childArgs = args.slice(1);
try {
  execFileSync(process.execPath, [binPath, ...childArgs], { stdio: 'inherit' });
} catch (err) {
  process.exit(err.status || 1);
}
