import { TerminalSocket } from '@mxdx/core';
import { randomUUID } from 'node:crypto';

/**
 * Start an interactive terminal session with a launcher.
 *
 * Sends an interactive command request, waits for the DM room response,
 * joins the DM, creates a TerminalSocket, and pipes stdin/stdout.
 *
 * @param {object} client - WasmMatrixClient
 * @param {object} topology - Launcher topology with exec_room_id
 * @param {object} options
 * @param {string} [options.command='/bin/bash'] - Shell command
 * @param {number} [options.cols] - Terminal columns (default: terminal width)
 * @param {number} [options.rows] - Terminal rows (default: terminal height)
 * @param {number} [options.batchMs=200] - Preferred batch window in ms
 * @param {function} [options.log] - Logger function
 */
export async function startInteractiveSession(client, topology, options = {}) {
  const {
    command = '/bin/bash',
    cols = process.stdout.columns || 80,
    rows = process.stdout.rows || 24,
    batchMs = 200,
    log = () => {},
  } = options;

  const requestId = randomUUID();

  // Send interactive session request to exec room
  log('Requesting interactive session...');
  await client.sendEvent(
    topology.exec_room_id,
    'org.mxdx.command',
    JSON.stringify({
      action: 'interactive',
      command,
      request_id: requestId,
      cols,
      rows,
      batch_ms: batchMs,
    }),
  );

  // Wait for session response with DM room_id
  log('Waiting for session...');
  const responseJson = await client.onRoomEvent(
    topology.exec_room_id,
    'org.mxdx.terminal.session',
    30,
  );

  if (!responseJson || responseJson === 'null') {
    throw new Error('Timed out waiting for interactive session response');
  }

  const response = JSON.parse(responseJson);
  const sessionContent = response.content || response;

  if (sessionContent.status !== 'started' || !sessionContent.room_id) {
    throw new Error(`Session request ${sessionContent.status || 'failed'}: ${JSON.stringify(sessionContent)}`);
  }

  const dmRoomId = sessionContent.room_id;
  log(`Session started in DM room: ${dmRoomId}`);

  // Accept DM room invitation
  await client.syncOnce();
  await client.joinRoom(dmRoomId);
  await client.syncOnce();
  log('Joined DM room');

  // Create TerminalSocket on the DM room
  const socket = new TerminalSocket(client, dmRoomId, { pollIntervalMs: 100 });

  // Set up raw terminal mode
  if (process.stdin.isTTY) {
    process.stdin.setRawMode(true);
  }
  process.stdin.resume();

  // Pipe stdin -> TerminalSocket
  process.stdin.on('data', async (chunk) => {
    try {
      await socket.send(chunk);
    } catch {
      // Socket may be closed
    }
  });

  // Pipe TerminalSocket -> stdout
  socket.onmessage = (event) => {
    process.stdout.write(Buffer.from(event.data));
  };

  // Handle window resize
  const onResize = () => {
    if (socket.connected) {
      socket.resize(process.stdout.columns, process.stdout.rows).catch(() => {});
    }
  };
  process.stdout.on('resize', onResize);

  // Handle close
  socket.onclose = () => {
    cleanup();
  };

  const cleanup = () => {
    process.stdout.removeListener('resize', onResize);
    if (process.stdin.isTTY) {
      process.stdin.setRawMode(false);
    }
    process.stdin.pause();
    socket.close();
  };

  // Handle Ctrl+C (exit gracefully)
  process.on('SIGINT', () => {
    cleanup();
    process.exit(0);
  });

  // Wait for the session to end
  return new Promise((resolve) => {
    socket.onclose = () => {
      cleanup();
      resolve();
    };

    // Also check for stdin end (pipe closed)
    process.stdin.on('end', () => {
      cleanup();
      resolve();
    });
  });
}
