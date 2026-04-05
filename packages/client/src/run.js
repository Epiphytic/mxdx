import { createSessionTask } from '@mxdx/core/src/session-client.js';

/**
 * Unified session event type constants.
 */
const EVENT_TYPES = {
    TASK: 'org.mxdx.session.task',
    START: 'org.mxdx.session.start',
    OUTPUT: 'org.mxdx.session.output',
    RESULT: 'org.mxdx.session.result',
};

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
 * @returns {Promise<number>} Exit code (0 on success, non-zero on failure)
 */
export async function run(client, roomId, opts) {
    const task = createSessionTask({
        bin: opts.command,
        args: opts.args,
        interactive: opts.interactive || false,
        noRoomOutput: opts.noRoomOutput || false,
        timeoutSeconds: opts.timeout || null,
        heartbeatInterval: opts.heartbeatInterval || 30,
        senderId: client.userId(),
    });

    // Submit task to exec room via E2EE
    await client.sendEvent(roomId, EVENT_TYPES.TASK, JSON.stringify(task));

    if (opts.detach) {
        console.log(task.uuid);
        return 0;
    }

    console.log(`Session ${task.uuid}: ${opts.command} ${(opts.args || []).join(' ')}`);

    // Track which output sequences we have already printed
    const printedSeqs = new Set();
    const SYNC_TIMEOUT_MS = 60_000;
    const POLL_INTERVAL_MS = 500;
    const startTime = Date.now();

    while (true) {
        // Guard against indefinite polling
        if (Date.now() - startTime > SYNC_TIMEOUT_MS) {
            console.error(`Timed out waiting for session ${task.uuid} to complete`);
            return 1;
        }

        await client.syncOnce();

        const eventsJson = await client.readRoomEvents(roomId);
        const events = JSON.parse(eventsJson);

        if (!Array.isArray(events)) {
            await delay(POLL_INTERVAL_MS);
            continue;
        }

        // Process output events for our task
        for (const event of events) {
            const content = event.content || event;
            if (event.type === EVENT_TYPES.OUTPUT && content.session_uuid === task.uuid) {
                const seq = content.seq;
                if (seq != null && printedSeqs.has(seq)) continue;
                if (seq != null) printedSeqs.add(seq);

                const data = decodeOutputData(content.data, content.encoding);
                const stream = content.stream === 'stderr' ? process.stderr : process.stdout;
                stream.write(data);
            }
        }

        // Check for result event
        for (const event of events) {
            const content = event.content || event;
            if (event.type === EVENT_TYPES.RESULT && content.session_uuid === task.uuid) {
                const exitCode = content.exit_code ?? 1;
                if (content.status === 'success') {
                    return exitCode;
                }
                if (content.error) {
                    console.error(`Session error: ${content.error}`);
                }
                return exitCode;
            }
        }

        await delay(POLL_INTERVAL_MS);
    }
}

/**
 * Decode output data from event content.
 * Handles base64 and plain text encodings.
 * @param {string} data - Encoded data
 * @param {string} [encoding] - Encoding type ('base64' or undefined for plain)
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
