/**
 * Attach to a running session by UUID.
 * @param {Object} client - Connected WASM Matrix client
 * @param {string} roomId - Target exec room ID
 * @param {string} uuid - Session UUID
 * @param {Object} [options]
 * @param {boolean} [options.interactive=false] - Enable interactive input
 * @returns {Promise<void>}
 */
export async function attachSession(client, roomId, uuid, { interactive = false } = {}) {
    // TODO: Connect to session thread, optionally negotiate WebRTC
    console.log(`Attaching to session ${uuid} not yet connected to Matrix`);
}
