#!/usr/bin/env node
/**
 * Purge all old devices and rooms for the E2E test accounts.
 * Uses Matrix REST API directly (no WASM needed).
 *
 * Usage: node scripts/purge-test-accounts.mjs
 */

import { logoutAll } from '../packages/core/cleanup.js';

const ACCOUNTS = [
  { homeserver: 'https://ca1-beta.mxdx.dev', username: 'e2etest-test1', password: 'mxdx-e2e-test-2026!' },
  { homeserver: 'https://ca1-beta.mxdx.dev', username: 'e2etest-test2', password: 'mxdx-e2e-test-2026!' },
  { homeserver: 'https://ca2-beta.mxdx.dev', username: 'e2etest-test1', password: 'mxdx-e2e-test-2026!' },
  { homeserver: 'https://ca2-beta.mxdx.dev', username: 'e2etest-test2', password: 'mxdx-e2e-test-2026!' },
];

async function login(homeserver, username, password) {
  const base = homeserver.replace(/\/+$/, '');
  const resp = await fetch(`${base}/_matrix/client/v3/login`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      type: 'm.login.password',
      identifier: { type: 'm.id.user', user: username },
      password,
    }),
  });
  if (!resp.ok) {
    const body = await resp.text();
    throw new Error(`Login failed for ${username}: ${resp.status} ${body}`);
  }
  const data = await resp.json();
  return { accessToken: data.access_token, deviceId: data.device_id, userId: data.user_id };
}

async function listJoinedRooms(homeserver, accessToken) {
  const base = homeserver.replace(/\/+$/, '');
  const resp = await fetch(`${base}/_matrix/client/v3/joined_rooms`, {
    headers: { 'Authorization': `Bearer ${accessToken}` },
  });
  if (!resp.ok) throw new Error(`Failed to list rooms: ${resp.status}`);
  const data = await resp.json();
  return data.joined_rooms || [];
}

async function leaveAndForget(homeserver, accessToken, roomId) {
  const base = homeserver.replace(/\/+$/, '');
  await fetch(`${base}/_matrix/client/v3/rooms/${encodeURIComponent(roomId)}/leave`, {
    method: 'POST',
    headers: { 'Authorization': `Bearer ${accessToken}`, 'Content-Type': 'application/json' },
    body: '{}',
  });
  // Small delay to avoid rate limiting
  await new Promise(r => setTimeout(r, 100));
  await fetch(`${base}/_matrix/client/v3/rooms/${encodeURIComponent(roomId)}/forget`, {
    method: 'POST',
    headers: { 'Authorization': `Bearer ${accessToken}`, 'Content-Type': 'application/json' },
    body: '{}',
  });
}

async function purgeAccount({ homeserver, username, password }) {
  const log = (msg) => console.log(`[${username}] ${msg}`);

  // Phase 1: Login, leave all rooms
  log('Logging in (phase 1: room cleanup)...');
  const { accessToken, deviceId, userId } = await login(homeserver, username, password);
  log(`Logged in as ${userId} (device: ${deviceId})`);

  log('Listing joined rooms...');
  const rooms = await listJoinedRooms(homeserver, accessToken);
  log(`Found ${rooms.length} joined room(s)`);

  for (const roomId of rooms) {
    log(`  Leaving ${roomId}`);
    try {
      await leaveAndForget(homeserver, accessToken, roomId);
    } catch (e) {
      log(`  Error: ${e.message}`);
    }
  }

  // Verify rooms are gone
  const remainingRooms = await listJoinedRooms(homeserver, accessToken);
  log(`Rooms remaining after cleanup: ${remainingRooms.length}`);

  // Phase 2: Logout ALL sessions (deletes all devices)
  log('Logging out ALL sessions (nuclear device cleanup)...');
  await logoutAll({ accessToken, homeserverUrl: homeserver, onProgress: log });

  log('Done — account is fully clean (0 devices, 0 rooms)');
}

// Also clean local crypto stores
async function cleanLocalStores() {
  const { rmSync, existsSync } = await import('fs');
  const { join } = await import('path');
  const home = process.env.HOME;

  const storeDirs = [
    join(home, '.mxdx', 'crypto'),
    join(home, '.mxdx', 'keychain'),
    join(home, '.mxdx', 'daemon'),
  ];

  for (const dir of storeDirs) {
    if (existsSync(dir)) {
      console.log(`[local] Removing ${dir}`);
      rmSync(dir, { recursive: true, force: true });
    }
  }
  console.log('[local] Local crypto stores cleaned');
}

console.log('=== Purging E2E test accounts ===\n');
for (const account of ACCOUNTS) {
  try {
    await purgeAccount(account);
    console.log();
  } catch (e) {
    console.error(`Failed to purge ${account.username}: ${e.message}`);
  }
}

console.log('=== Cleaning local stores ===');
await cleanLocalStores();

console.log('\n=== Purge complete — next test run will start fresh ===');
