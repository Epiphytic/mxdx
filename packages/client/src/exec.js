import crypto from 'node:crypto';
import { run } from './run.js';

/**
 * @deprecated Use `run()` from './run.js' instead. This function is retained
 * for backward compatibility during the unified session migration.
 *
 * Send a command to a launcher and collect the result.
 * @param {WasmMatrixClient} client - Connected WASM client
 * @param {Object} topology - Launcher topology with exec_room_id
 * @param {string} command - Command to execute
 * @param {string[]} args - Command arguments
 * @param {Object} [options]
 * @param {string} [options.cwd] - Working directory
 * @param {number} [options.timeoutSecs] - Timeout in seconds
 * @param {string} [options.format] - Output format: 'text' or 'json'
 * @returns {Promise<{exitCode: number, stdout: string[], stderr: string[], timedOut: boolean}>}
 */
export async function execCommand(client, topology, command, args = [], options = {}) {
  const { cwd = '/tmp', timeoutSecs = 30, format = 'text' } = options;
  const requestId = crypto.randomUUID();

  // Send command event
  await client.sendEvent(
    topology.exec_room_id,
    'org.mxdx.command',
    JSON.stringify({
      request_id: requestId,
      command,
      args,
      cwd,
    }),
  );

  // Poll for result
  const deadline = Date.now() + (timeoutSecs * 1000);
  const stdoutLines = [];
  const stderrLines = [];
  let result = null;

  while (Date.now() < deadline) {
    await client.syncOnce();

    const events = JSON.parse(await client.collectRoomEvents(topology.exec_room_id, 1));
    if (!events || !Array.isArray(events)) continue;

    for (const event of events) {
      const content = event?.content;
      if (!content || content.request_id !== requestId) continue;

      if (event.type === 'org.mxdx.output') {
        const line = Buffer.from(content.data || '', 'base64').toString('utf8');
        if (content.stream === 'stdout') {
          stdoutLines.push(line);
          if (format === 'text') process.stdout.write(line + '\n');
        } else if (content.stream === 'stderr') {
          stderrLines.push(line);
          if (format === 'text') process.stderr.write(line + '\n');
        }
      }

      if (event.type === 'org.mxdx.result') {
        result = {
          exitCode: content.exit_code ?? 1,
          timedOut: content.timed_out || false,
          error: content.error || null,
        };
      }
    }

    if (result) break;
  }

  if (!result) {
    result = { exitCode: 1, timedOut: true, error: 'Timed out waiting for result' };
  }

  return {
    ...result,
    stdout: stdoutLines,
    stderr: stderrLines,
  };
}
