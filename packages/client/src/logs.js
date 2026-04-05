/**
 * Unified session event type constants.
 */
const EVENT_TYPES = {
    OUTPUT: 'org.mxdx.session.output',
    RESULT: 'org.mxdx.session.result',
};

/**
 * View logs for a specific session by UUID.
 * Fetches output events from the exec room, sorted by sequence number.
 * @param {Object} client - Connected WASM Matrix client
 * @param {string} roomId - Target exec room ID
 * @param {string} uuid - Session UUID
 * @param {Object} [options]
 * @param {boolean} [options.follow=false] - Follow/tail mode (keep syncing for new output)
 * @returns {Promise<void>}
 */
export async function viewLogs(client, roomId, uuid, { follow = false } = {}) {
    const printedSeqs = new Set();
    let resultSeen = false;

    const processEvents = (events) => {
        if (!Array.isArray(events)) return;

        // Collect and sort output events by seq
        const outputs = [];
        for (const event of events) {
            const content = event.content || event;
            if (event.type === EVENT_TYPES.OUTPUT && content.session_uuid === uuid) {
                const seq = content.seq;
                if (seq != null && printedSeqs.has(seq)) continue;
                outputs.push({ seq: seq ?? 0, content });
            }
            if (event.type === EVENT_TYPES.RESULT && content.session_uuid === uuid) {
                resultSeen = true;
            }
        }

        outputs.sort((a, b) => a.seq - b.seq);

        for (const { seq, content } of outputs) {
            if (seq != null) printedSeqs.add(seq);
            const data = decodeOutputData(content.data, content.encoding);
            const stream = content.stream === 'stderr' ? process.stderr : process.stdout;
            stream.write(data);
        }
    };

    // Initial fetch
    await client.syncOnce();
    const eventsJson = await client.readRoomEvents(roomId);
    processEvents(JSON.parse(eventsJson));

    if (!follow) {
        if (printedSeqs.size === 0) {
            console.error(`No output found for session ${uuid}`);
        }
        return;
    }

    // Follow mode: keep syncing until result event arrives
    const POLL_INTERVAL_MS = 500;
    const FOLLOW_TIMEOUT_MS = 3600_000; // 1 hour max follow
    const startTime = Date.now();

    while (!resultSeen) {
        if (Date.now() - startTime > FOLLOW_TIMEOUT_MS) {
            console.error('Follow mode timed out after 1 hour');
            break;
        }

        await delay(POLL_INTERVAL_MS);
        await client.syncOnce();
        const freshJson = await client.readRoomEvents(roomId);
        processEvents(JSON.parse(freshJson));
    }
}

/**
 * Decode output data from event content.
 * @param {string} data - Encoded data
 * @param {string} [encoding] - Encoding type
 * @returns {string} Decoded string
 */
function decodeOutputData(data, encoding) {
    if (!data) return '';
    if (encoding === 'base64' || encoding === 'zlib+base64') {
        return Buffer.from(data, 'base64').toString('utf-8');
    }
    return data;
}

/**
 * @param {number} ms
 * @returns {Promise<void>}
 */
function delay(ms) {
    return new Promise((resolve) => setTimeout(resolve, ms));
}
