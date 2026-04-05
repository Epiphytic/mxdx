/**
 * Unified session event type constants.
 */
const EVENT_TYPES = {
    CANCEL: 'org.mxdx.session.cancel',
    SIGNAL: 'org.mxdx.session.signal',
};

/**
 * Cancel a running session or send a signal to it.
 * Posts a cancel or signal event to the exec room via E2EE.
 * @param {Object} client - Connected WASM Matrix client
 * @param {string} roomId - Target exec room ID
 * @param {string} uuid - Session UUID
 * @param {Object} [options]
 * @param {string} [options.signal] - Signal to send (e.g. 'SIGTERM', 'SIGKILL')
 * @returns {Promise<void>}
 */
export async function cancelSession(client, roomId, uuid, { signal } = {}) {
    if (signal) {
        const content = {
            session_uuid: uuid,
            signal,
        };
        await client.sendEvent(roomId, EVENT_TYPES.SIGNAL, JSON.stringify(content));
        console.log(`Sent ${signal} to session ${uuid}`);
    } else {
        const content = {
            session_uuid: uuid,
            reason: 'user requested',
            grace_seconds: 5,
        };
        await client.sendEvent(roomId, EVENT_TYPES.CANCEL, JSON.stringify(content));
        console.log(`Cancelled session ${uuid}`);
    }
}
