/**
 * Unified Session Client — wraps WASM session operations for JS consumption.
 * Handles task creation, output parsing, and session state management.
 */

import { createRequire } from 'node:module';
const require = createRequire(import.meta.url);

/**
 * Create a new session task.
 * @param {Object} opts
 * @param {string} opts.bin - Command to execute
 * @param {string[]} opts.args - Command arguments
 * @param {boolean} [opts.interactive=false] - Interactive session
 * @param {boolean} [opts.noRoomOutput=false] - Suppress room output
 * @param {number|null} [opts.timeoutSeconds=null] - Timeout
 * @param {number} [opts.heartbeatInterval=30] - Heartbeat interval
 * @param {string} opts.senderId - Matrix user ID
 * @returns {Object} Parsed SessionTask object
 */
export function createSessionTask(opts) {
    // Import is deferred to allow WASM to load first
    const wasm = require('../wasm/nodejs/mxdx_core_wasm');
    const json = wasm.create_session_task(
        opts.bin,
        opts.args || [],
        opts.interactive || false,
        opts.noRoomOutput || false,
        opts.timeoutSeconds ?? null,
        opts.heartbeatInterval || 30,
        opts.senderId,
    );
    return JSON.parse(json);
}

/**
 * Parse a session result event.
 * @param {Object|string} data - Raw event content (JSON string or object)
 * @returns {Object} Parsed SessionResult
 */
export function parseSessionResult(data) {
    const wasm = require('../wasm/nodejs/mxdx_core_wasm');
    const json = typeof data === 'string' ? data : JSON.stringify(data);
    return JSON.parse(wasm.parse_session_result(json));
}

/**
 * Parse an active session state event.
 * @param {Object|string} data
 * @returns {Object} Parsed ActiveSessionState
 */
export function parseActiveSession(data) {
    const wasm = require('../wasm/nodejs/mxdx_core_wasm');
    const json = typeof data === 'string' ? data : JSON.stringify(data);
    return JSON.parse(wasm.parse_active_session(json));
}

/**
 * Parse a completed session state event.
 * @param {Object|string} data
 * @returns {Object} Parsed CompletedSessionState
 */
export function parseCompletedSession(data) {
    const wasm = require('../wasm/nodejs/mxdx_core_wasm');
    const json = typeof data === 'string' ? data : JSON.stringify(data);
    return JSON.parse(wasm.parse_completed_session(json));
}

/**
 * Parse a worker info state event.
 * @param {Object|string} data
 * @returns {Object} Parsed WorkerInfo
 */
export function parseWorkerInfo(data) {
    const wasm = require('../wasm/nodejs/mxdx_core_wasm');
    const json = typeof data === 'string' ? data : JSON.stringify(data);
    return JSON.parse(wasm.parse_worker_info(json));
}

/**
 * Get all session event type constants.
 * @returns {Object} Map of event type names to their string values
 */
export function getSessionEventTypes() {
    const wasm = require('../wasm/nodejs/mxdx_core_wasm');
    return JSON.parse(wasm.session_event_types());
}
