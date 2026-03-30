/**
 * Attach to a running session by UUID.
 * Finds the SessionStart event for this UUID in the exec room to discover
 * the DM room_id for interactive terminal I/O.
 *
 * @param {Object} client - Connected WASM Matrix client
 * @param {string} roomId - Target exec room ID
 * @param {string} uuid - Session UUID
 * @param {Object} [options]
 * @param {boolean} [options.interactive=false] - Enable interactive input
 * @returns {Promise<void>}
 */
export async function attachSession(client, roomId, uuid, { interactive = false } = {}) {
    // Sync to get the latest events
    await client.syncOnce();

    // Read room events to find the SessionStart for this UUID
    const eventsJson = await client.readRoomEvents(roomId);
    const events = JSON.parse(eventsJson);

    const startEvent = events.find(
        (e) =>
            e.type === 'org.mxdx.session.start' &&
            e.content?.session_uuid === uuid
    );

    if (!startEvent) {
        console.error(`No SessionStart event found for session ${uuid}`);
        process.exit(1);
    }

    const dmRoomId = startEvent.content?.dm_room_id;

    if (dmRoomId && interactive) {
        console.log(`Attaching to session ${uuid} in DM room ${dmRoomId}`);
        // TODO: Terminal raw mode, stdin/stdout piping via WASM methods
        // - process.stdin.setRawMode(true)
        // - process.stdin on 'data' -> client.sendTerminalInput(dmRoomId, uuid, data)
        // - Poll for SESSION_OUTPUT events in DM room -> process.stdout.write(data)
        // - Handle SIGWINCH -> client.sendTerminalResize(dmRoomId, uuid, cols, rows)
        // - Detect Ctrl-] (0x1d) to detach
        console.log('Interactive terminal attach not yet fully implemented');
        console.log('Press Ctrl-] to detach');
    } else if (dmRoomId) {
        console.log(`Session ${uuid} has DM room ${dmRoomId} (use -i for interactive mode)`);
        console.log('Falling back to thread tail mode...');
        // TODO: Tail the exec room thread for output events
    } else {
        console.log(`Session ${uuid} is not interactive (no DM room)`);
        console.log('Falling back to thread tail mode...');
        // TODO: Tail the exec room thread for output events
    }
}
