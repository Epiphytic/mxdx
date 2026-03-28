/**
 * Cancel a running session or send a signal to it.
 * @param {Object} client - Connected WASM Matrix client
 * @param {string} roomId - Target exec room ID
 * @param {string} uuid - Session UUID
 * @param {Object} [options]
 * @param {string} [options.signal] - Signal to send (e.g. 'SIGTERM', 'SIGKILL')
 * @returns {Promise<void>}
 */
export async function cancelSession(client, roomId, uuid, { signal } = {}) {
    // TODO: Post cancel/signal event to room
    console.log(`Cancel session ${uuid} not yet connected to Matrix`);
}
