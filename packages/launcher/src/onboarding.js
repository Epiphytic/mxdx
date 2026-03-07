import { createInterface } from 'node:readline/promises';
import os from 'node:os';
import { LauncherConfig } from './config.js';

const SERVERS = [
  { name: 'matrix.org', value: 'https://matrix.org' },
  { name: 'mxdx.dev', value: 'https://mxdx.dev' },
];

/**
 * Run interactive onboarding flow. Returns a LauncherConfig.
 */
export async function runOnboarding() {
  const rl = createInterface({ input: process.stdin, output: process.stdout });

  try {
    console.log('\nWelcome to mxdx launcher setup.\n');

    // Server selection
    console.log('Select a Matrix server:');
    SERVERS.forEach((s, i) => console.log(`  ${i + 1}. ${s.name}`));
    console.log(`  ${SERVERS.length + 1}. Other (enter URL)`);

    const serverChoice = await rl.question('\nChoice: ');
    let serverUrl;
    const idx = parseInt(serverChoice, 10) - 1;
    if (idx >= 0 && idx < SERVERS.length) {
      serverUrl = SERVERS[idx].value;
    } else {
      serverUrl = await rl.question('Server URL: ');
    }

    // Username
    const defaultUsername = os.hostname();
    const username = await rl.question(`Username [${defaultUsername}]: `) || defaultUsername;

    // Password
    const password = await rl.question('Password: ');
    if (!password) {
      console.error('Password is required.');
      process.exit(1);
    }

    // Email (optional, for account recovery)
    const email = await rl.question('Email (for account recovery, optional): ');

    // Allowed commands
    const allowedStr = await rl.question('Allowed commands (comma-separated, e.g. echo,cat): ');

    const config = new LauncherConfig({
      username,
      servers: [serverUrl],
      allowedCommands: allowedStr ? allowedStr.split(',').map(s => s.trim()) : [],
    });

    // Store password separately (not in config file)
    config._password = password;
    config._email = email;

    return config;
  } finally {
    rl.close();
  }
}
