/**
 * Accept all pending room invitations, then discover a specific launcher.
 * @param {WasmMatrixClient} client - Connected client
 * @param {string} launcherName - The launcher ID to find
 * @returns {Object|null} Topology or null
 */
export async function findLauncher(client, launcherName) {
  // Sync to pick up invitations
  await client.syncOnce();

  // Accept all pending invitations
  const invited = client.invitedRoomIds();
  for (const roomId of invited) {
    try {
      await client.joinRoom(roomId);
    } catch {
      // May fail if invite was revoked
    }
  }

  // Sync again to get room state after joining
  if (invited.length > 0) {
    await client.syncOnce();
  }

  // findLauncherSpace returns either JS null or a JSON string. The WASM
  // boundary forces JSON-string returns for serde_json::Value-shaped types
  // (serde_wasm_bindgen::to_value silently drops nested values). See
  // crates/mxdx-core-wasm/src/lib.rs::LauncherTopology.
  const topologyRaw = await client.findLauncherSpace(launcherName);
  if (!topologyRaw) return null;
  const topology = typeof topologyRaw === 'string' ? JSON.parse(topologyRaw) : topologyRaw;
  return {
    name: launcherName,
    ...topology,
  };
}
