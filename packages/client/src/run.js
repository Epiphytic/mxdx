import { createSessionTask } from '@mxdx/core/src/session-client.js';

/**
 * Submit a session task to a launcher room.
 * @param {Object} client - Connected WASM Matrix client
 * @param {string} roomId - Target exec room ID
 * @param {Object} opts
 * @param {string} opts.command - Command to execute
 * @param {string[]} [opts.args] - Command arguments
 * @param {boolean} [opts.interactive=false] - Interactive session
 * @param {boolean} [opts.noRoomOutput=false] - Suppress room output
 * @param {number|null} [opts.timeout=null] - Timeout in seconds
 * @param {number} [opts.heartbeatInterval=30] - Heartbeat interval
 * @param {boolean} [opts.detach=false] - Print UUID and return immediately
 * @returns {Promise<void>}
 */
export async function run(client, roomId, opts) {
    const task = createSessionTask({
        bin: opts.command,
        args: opts.args,
        interactive: opts.interactive || false,
        noRoomOutput: opts.noRoomOutput || false,
        timeoutSeconds: opts.timeout || null,
        heartbeatInterval: opts.heartbeatInterval || 30,
        senderId: client.userId,
    });

    if (opts.detach) {
        console.log(task.uuid);
        return;
    }

    console.log(`Session ${task.uuid}: ${opts.command} ${(opts.args || []).join(' ')}`);
    // TODO: Submit task to room and tail thread
}
