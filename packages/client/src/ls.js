/**
 * Unified session event type constants.
 */
const EVENT_TYPES = {
    TASK: 'org.mxdx.session.task',
    START: 'org.mxdx.session.start',
    RESULT: 'org.mxdx.session.result',
};

/**
 * List sessions on a launcher, reading from room timeline events.
 * Correlates task submissions with results to determine active vs completed.
 * @param {Object} client - Connected WASM Matrix client
 * @param {string} roomId - Target exec room ID
 * @param {Object} [options]
 * @param {boolean} [options.all=false] - Include completed sessions
 * @returns {Promise<void>}
 */
export async function listSessions(client, roomId, { all = false } = {}) {
    await client.syncOnce();

    const eventsJson = await client.readRoomEvents(roomId);
    const events = JSON.parse(eventsJson);

    if (!Array.isArray(events) || events.length === 0) {
        console.log('No sessions found.');
        return;
    }

    // Index tasks and results by UUID
    const tasks = new Map();   // uuid -> task content + metadata
    const starts = new Map();  // uuid -> start content
    const results = new Map(); // uuid -> result content

    for (const event of events) {
        const content = event.content || event;
        const uuid = content.uuid || content.session_uuid;
        if (!uuid) continue;

        if (event.type === EVENT_TYPES.TASK) {
            tasks.set(uuid, {
                uuid,
                command: content.bin || content.command || '?',
                args: content.args || [],
                sender: event.sender || content.sender_id || '',
                submitted_at: content.submitted_at || event.origin_server_ts,
            });
        } else if (event.type === EVENT_TYPES.START) {
            starts.set(uuid, {
                worker_id: content.worker_id || '',
                started_at: content.started_at,
            });
        } else if (event.type === EVENT_TYPES.RESULT) {
            results.set(uuid, {
                status: content.status || 'unknown',
                exit_code: content.exit_code,
                duration_seconds: content.duration_seconds,
            });
        }
    }

    if (tasks.size === 0) {
        console.log('No sessions found.');
        return;
    }

    // Build display rows
    const rows = [];
    for (const [uuid, task] of tasks) {
        const start = starts.get(uuid);
        const result = results.get(uuid);
        const isActive = !result;

        if (!all && !isActive) continue;

        const cmd = [task.command, ...(task.args || [])].join(' ');
        const worker = start?.worker_id || '-';
        const startTime = formatTimestamp(start?.started_at || task.submitted_at);
        const duration = result?.duration_seconds != null
            ? `${result.duration_seconds}s`
            : (start?.started_at ? `${Math.floor(Date.now() / 1000 - start.started_at)}s` : '-');
        const status = result ? `${result.status} (${result.exit_code})` : 'running';

        rows.push({ uuid: uuid.slice(0, 8), cmd: truncate(cmd, 40), worker: truncate(worker, 30), startTime, duration, status });
    }

    if (rows.length === 0) {
        console.log('No active sessions.');
        return;
    }

    // Print table
    const header = { uuid: 'UUID', cmd: 'Command', worker: 'Worker', startTime: 'Started', duration: 'Duration', status: 'Status' };
    const widths = {};
    for (const col of Object.keys(header)) {
        widths[col] = Math.max(header[col].length, ...rows.map((r) => (r[col] || '').length));
    }

    const pad = (val, width) => String(val || '').padEnd(width);
    const line = Object.keys(header).map((col) => pad(header[col], widths[col])).join('  ');
    console.log(line);
    console.log(Object.keys(header).map((col) => '-'.repeat(widths[col])).join('  '));
    for (const row of rows) {
        console.log(Object.keys(header).map((col) => pad(row[col], widths[col])).join('  '));
    }
}

/**
 * Format a unix timestamp or ISO string for display.
 * @param {number|string|undefined} ts
 * @returns {string}
 */
function formatTimestamp(ts) {
    if (!ts) return '-';
    const date = typeof ts === 'number' ? new Date(ts * 1000) : new Date(ts);
    if (isNaN(date.getTime())) return '-';
    return date.toISOString().replace('T', ' ').slice(0, 19);
}

/**
 * Truncate a string to a maximum length.
 * @param {string} str
 * @param {number} maxLen
 * @returns {string}
 */
function truncate(str, maxLen) {
    if (!str) return '';
    return str.length > maxLen ? str.slice(0, maxLen - 1) + '\u2026' : str;
}
