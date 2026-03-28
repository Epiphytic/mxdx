/**
 * List sessions on a launcher, reading from room state events.
 * @param {Object} client - Connected WASM Matrix client
 * @param {string} roomId - Target exec room ID
 * @param {Object} [options]
 * @param {boolean} [options.all=false] - Include completed sessions
 * @returns {Promise<void>}
 */
export async function listSessions(client, roomId, { all = false } = {}) {
    // TODO: Read state events and format as table
    console.log('Session listing not yet connected to Matrix');
}
