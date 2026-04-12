#!/usr/bin/env node
/**
 * One-shot fix: purge stale federated rooms across ca1 and ca2.
 * Run this to clean up rooms where REST leave didn't propagate via federation.
 *
 * Usage: node scripts/fix-stale-federation.mjs
 */

import { federatedLeave } from '../packages/core/cleanup.js';

const SERVERS = {
  'ca1-beta.mxdx.dev': 'https://ca1-beta.mxdx.dev',
  'ca2-beta.mxdx.dev': 'https://ca2-beta.mxdx.dev',
};

const USERNAMES = ['e2etest-test1', 'e2etest-test2', 'e2etest-coordinator'];
const PASSWORD = 'mxdx-e2e-test-2026!';

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
  if (!resp.ok) throw new Error(`Login failed: ${resp.status} ${await resp.text()}`);
  return resp.json();
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

async function getInvitedRooms(homeserver, accessToken) {
  const base = homeserver.replace(/\/+$/, '');
  const resp = await fetch(`${base}/_matrix/client/v3/sync?timeout=0&filter={"room":{"timeline":{"limit":0}}}`, {
    headers: { 'Authorization': `Bearer ${accessToken}` },
  });
  if (!resp.ok) return [];
  const data = await resp.json();
  return Object.keys(data.rooms?.invite || {});
}

console.log('=== Stale Federation Room Fix ===\n');
console.log('Servers:', Object.keys(SERVERS).join(', '));
console.log('Accounts:', USERNAMES.join(', '));
console.log('');

let totalRooms = 0;
let totalFixed = 0;

for (const username of USERNAMES) {
  for (const [serverName, serverUrl] of Object.entries(SERVERS)) {
    const log = (msg) => console.log(`[${username}@${serverName}] ${msg}`);

    try {
      const { access_token: accessToken } = await login(serverUrl, username, PASSWORD);

      // Get both joined and invited rooms
      const joinedRooms = await listJoinedRooms(serverUrl, accessToken);
      const invitedRooms = await getInvitedRooms(serverUrl, accessToken);
      const allRooms = [...new Set([...joinedRooms, ...invitedRooms])];

      if (allRooms.length === 0) {
        log('Clean — no rooms');
        // Logout
        await fetch(`${serverUrl}/_matrix/client/v3/logout`, {
          method: 'POST',
          headers: { 'Authorization': `Bearer ${accessToken}`, 'Content-Type': 'application/json' },
          body: '{}',
        });
        continue;
      }

      log(`Found ${joinedRooms.length} joined + ${invitedRooms.length} invited room(s)`);
      totalRooms += allRooms.length;

      // Build remote server map
      const remoteServers = {};
      for (const [name, url] of Object.entries(SERVERS)) {
        if (name !== serverName) {
          remoteServers[name] = { url, username, password: PASSWORD };
        }
      }

      for (const roomId of allRooms) {
        const result = await federatedLeave({
          roomId,
          localHomeserver: serverUrl,
          localAccessToken: accessToken,
          remoteServers,
          verifyTimeoutMs: 5000,
          onProgress: log,
        });

        if (result.local || result.remote) totalFixed++;
      }

      // Verify final state
      const remaining = await listJoinedRooms(serverUrl, accessToken);
      if (remaining.length > 0) {
        log(`⚠ ${remaining.length} room(s) still remaining after cleanup:`);
        for (const r of remaining) log(`  ${r}`);
      } else {
        log('✓ All rooms cleared');
      }

      // Logout
      await fetch(`${serverUrl}/_matrix/client/v3/logout`, {
        method: 'POST',
        headers: { 'Authorization': `Bearer ${accessToken}`, 'Content-Type': 'application/json' },
        body: '{}',
      });
    } catch (err) {
      log(`Error: ${err.message}`);
    }
  }
}

console.log(`\n=== Done: ${totalFixed}/${totalRooms} rooms cleaned across federation ===`);
