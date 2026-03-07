import { WasmMatrixClient } from '@mxdx/core';

/**
 * Discover online launchers by scanning joined rooms for launcher space topics.
 * Returns an array of { name, spaceId, execRoomId, statusRoomId, logsRoomId }.
 */
export async function discoverLaunchers(client) {
  await client.syncOnce();

  // The WASM client doesn't expose room listing directly,
  // so we use findLauncherSpace with known launcher names.
  // For full discovery, we'll need to add a room-listing export.
  // For now, return empty — the E2E test will use findLauncherSpace directly.
  return [];
}

/**
 * Connect to a Matrix server and discover a specific launcher.
 * @param {WasmMatrixClient} client - Connected client
 * @param {string} launcherName - The launcher ID to find
 * @returns {Object|null} Topology or null
 */
export async function findLauncher(client, launcherName) {
  const topology = await client.findLauncherSpace(launcherName);
  if (!topology) return null;
  return {
    name: launcherName,
    ...topology,
  };
}
