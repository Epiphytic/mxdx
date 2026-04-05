#!/usr/bin/env node
import { parseArgs } from 'node:util';

const { values, positionals } = parseArgs({
    options: {
        help: { type: 'boolean', short: 'h' },
        room: { type: 'string' },
        'capability-room-prefix': { type: 'string' },
    },
    allowPositionals: true,
});

if (values.help || positionals[0] === 'help') {
    console.log(`
mxdx-coordinator — Fleet task routing and monitoring

Usage:
  mxdx-coordinator start [options]

Options:
  --room <name>                    Coordinator room name
  --capability-room-prefix <pfx>   Prefix for capability rooms
  -h, --help                       Show this help
`.trim());
    process.exit(0);
}

const command = positionals[0] || 'start';
if (command === 'start') {
    console.log('mxdx-coordinator starting...');
    console.log('Coordinator not yet connected to Matrix — use native binary for full functionality');
}
