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

  const topology = await client.findLauncherSpace(launcherName);
  if (!topology) return null;
  return {
    name: launcherName,
    ...topology,
  };
}
