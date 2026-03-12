#!/usr/bin/env node
/**
 * Phase 0, Task 0.4: Prove WasmMatrixClient can register+login against Tuwunel.
 * This is a one-shot test script, not a permanent test fixture.
 *
 * Usage: Start a Tuwunel instance first, then:
 *   TUWUNEL_URL=http://127.0.0.1:PORT node packages/core/test-wasm-login.mjs
 */

import { WasmMatrixClient } from './wasm/nodejs/mxdx_core_wasm.js';

const url = process.env.TUWUNEL_URL;
if (!url) {
  console.error('Set TUWUNEL_URL=http://127.0.0.1:PORT');
  process.exit(1);
}

const username = `wasm-test-${Date.now()}`;
const password = 'testpass123';
const token = 'mxdx-test-token';

console.log(`Registering ${username} on ${url}...`);
const client = await WasmMatrixClient.register(url, username, password, token);

console.log('Logged in:', client.isLoggedIn());
console.log('User ID:', client.userId());

if (!client.isLoggedIn()) {
  console.error('FAIL: not logged in');
  process.exit(1);
}
if (!client.userId().startsWith(`@${username}:`)) {
  console.error('FAIL: unexpected user ID');
  process.exit(1);
}

console.log('Syncing...');
await client.syncOnce();
console.log('PASS: WASM MatrixClient register+login+sync works');
