import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { TerminalSocket } from './terminal-socket.js';

let activeSocket = null;
let activeTerminal = null;

function base64Decode(str) {
  const binary = atob(str);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes;
}

async function decompress(data) {
  const ds = new DecompressionStream('deflate');
  const writer = ds.writable.getWriter();
  const reader = ds.readable.getReader();
  writer.write(data);
  writer.close();
  const chunks = [];
  let totalLength = 0;
  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    chunks.push(value);
    totalLength += value.length;
    if (totalLength > 1024 * 1024) throw new Error('Decompressed data exceeds max');
  }
  const result = new Uint8Array(totalLength);
  let offset = 0;
  for (const chunk of chunks) {
    result.set(chunk, offset);
    offset += chunk.length;
  }
  return result;
}

/**
 * Set up an interactive terminal session in the browser.
 * @param {object} client - WasmMatrixClient
 * @param {object} launcher - Launcher info with exec_room_id
 * @param {object} callbacks
 * @param {function} callbacks.onClose - Called when session ends
 * @param {function} [callbacks.onSessionStarted] - Called with session info after session starts
 */
export async function setupTerminalView(client, launcher, { onClose, onSessionStarted }) {
  const container = document.getElementById('terminal-container');
  container.replaceChildren();

  // Clean up previous session
  if (activeSocket) {
    activeSocket.close();
    activeSocket = null;
  }
  if (activeTerminal) {
    activeTerminal.dispose();
    activeTerminal = null;
  }

  // Create xterm.js terminal
  const term = new Terminal({
    cursorBlink: true,
    fontSize: 14,
    fontFamily: '"SFMono-Regular", Consolas, "Liberation Mono", Menlo, monospace',
    theme: {
      background: '#0d1117',
      foreground: '#c9d1d9',
      cursor: '#58a6ff',
    },
  });

  const fitAddon = new FitAddon();
  term.loadAddon(fitAddon);
  term.open(container);
  fitAddon.fit();
  activeTerminal = term;

  term.writeln('Requesting interactive session...');

  try {
    // Sync first to pick up any device key changes (e.g. launcher restart)
    await client.syncOnce();

    // Send interactive session request
    const requestId = crypto.randomUUID();
    const cols = term.cols;
    const rows = term.rows;
    const batchMs = parseInt(localStorage.getItem('mxdx-batch-ms') || '200', 10);

    try {
      await client.sendEvent(
        launcher.exec_room_id,
        'org.mxdx.command',
        JSON.stringify({
          action: 'interactive',
          request_id: requestId,
          cols,
          rows,
          batch_ms: batchMs,
        }),
      );
    } catch (sendErr) {
      term.writeln(`\r\nFailed to send session request: ${sendErr}`);
      return;
    }

    term.writeln('Waiting for session...');

    // Wait for session response (30s timeout — E2EE key sharing with many
    // devices can take 5-10s each direction)
    const responseJson = await client.onRoomEvent(
      launcher.exec_room_id,
      'org.mxdx.terminal.session',
      30,
    );

    if (!responseJson || responseJson === 'null') {
      term.writeln('\r\nTimeout: launcher did not respond within 30 seconds.');
      term.writeln('Check that the launcher is running and can decrypt messages.');
      return;
    }

    const response = JSON.parse(responseJson);
    const sessionContent = response.content || response;

    if (sessionContent.status !== 'started' || !sessionContent.room_id) {
      term.writeln(`\r\nSession rejected: ${sessionContent.status || 'unknown'}`);
      return;
    }

    const dmRoomId = sessionContent.room_id;

    if (onSessionStarted) {
      onSessionStarted({
        session_id: sessionContent.session_id,
        room_id: dmRoomId,
        persistent: sessionContent.persistent ?? false,
      });
    }

    term.writeln(`Session started. Joining room...`);

    // Accept DM invitation
    await client.syncOnce();
    await client.joinRoom(dmRoomId);
    await client.syncOnce();

    term.writeln('Connected.\r\n');
    term.clear();

    // Create TerminalSocket on DM room (use negotiated batch window)
    const negotiatedBatchMs = sessionContent.batch_ms || 200;
    const socket = new TerminalSocket(client, dmRoomId, { pollIntervalMs: 100, batchMs: negotiatedBatchMs });
    activeSocket = socket;

    // Wire: buffering status indicator
    const statusEl = document.getElementById('terminal-status');
    socket.onbuffering = (buffering) => {
      if (statusEl) {
        statusEl.textContent = buffering ? 'Buffering...' : '';
        statusEl.hidden = !buffering;
      }
    };

    // Wire: terminal input -> socket
    term.onData(async (data) => {
      try {
        await socket.send(data);
      } catch {
        // Socket may be closed
      }
    });

    // Wire: socket output -> terminal
    socket.onmessage = (event) => {
      term.write(new Uint8Array(event.data));
    };

    // Wire: terminal resize -> socket
    term.onResize(({ cols, rows }) => {
      if (socket.connected) {
        socket.resize(cols, rows).catch(() => {});
      }
    });

    // Wire: window resize -> fit terminal
    const onWindowResize = () => fitAddon.fit();
    window.addEventListener('resize', onWindowResize);

    // Handle socket close
    socket.onclose = () => {
      window.removeEventListener('resize', onWindowResize);
      term.writeln('\r\n\r\n[Session ended]');
      activeSocket = null;
    };

  } catch (err) {
    term.writeln(`\r\nError: ${err}`);
  }
}

export async function reconnectTerminalView(client, launcher, session, { onClose }) {
  const container = document.getElementById('terminal-container');
  container.replaceChildren();

  if (activeSocket) { activeSocket.close(); activeSocket = null; }
  if (activeTerminal) { activeTerminal.dispose(); activeTerminal = null; }

  const term = new Terminal({
    cursorBlink: true,
    fontSize: 14,
    fontFamily: '"SFMono-Regular", Consolas, "Liberation Mono", Menlo, monospace',
    theme: { background: '#0d1117', foreground: '#c9d1d9', cursor: '#58a6ff' },
  });

  const fitAddon = new FitAddon();
  term.loadAddon(fitAddon);
  term.open(container);
  fitAddon.fit();
  activeTerminal = term;

  term.writeln('Reconnecting to session...');

  try {
    await client.syncOnce();

    const requestId = crypto.randomUUID();
    await client.sendEvent(
      launcher.exec_room_id,
      'org.mxdx.command',
      JSON.stringify({
        action: 'reconnect',
        session_id: session.session_id,
        request_id: requestId,
        cols: term.cols,
        rows: term.rows,
      }),
    );

    term.writeln('Waiting for launcher...');

    const responseJson = await client.onRoomEvent(
      launcher.exec_room_id, 'org.mxdx.terminal.session', 30,
    );

    if (!responseJson || responseJson === 'null') {
      term.writeln('\r\nTimeout: launcher did not respond.');
      return;
    }

    const response = JSON.parse(responseJson);
    const sessionContent = response.content || response;

    if (sessionContent.status === 'expired') {
      term.writeln('\r\nSession expired — tmux session no longer exists.');
      return;
    }

    if (sessionContent.status !== 'reconnected' || !sessionContent.room_id) {
      term.writeln(`\r\nReconnect failed: ${sessionContent.status || 'unknown'}`);
      return;
    }

    const dmRoomId = sessionContent.room_id;
    term.writeln('Replaying history...');

    // Replay recent terminal.data events from room history
    try {
      await client.syncOnce();
      const historyJson = await client.collectRoomEvents(dmRoomId, 50);
      const historyEvents = JSON.parse(historyJson);
      const terminalEvents = (historyEvents || [])
        .filter(e => e.type === 'org.mxdx.terminal.data' && e.sender !== client.userId())
        .sort((a, b) => (a.content?.seq ?? 0) - (b.content?.seq ?? 0));

      term.clear();
      for (const event of terminalEvents) {
        const content = event.content;
        if (!content?.data || !content?.encoding) continue;
        const raw = base64Decode(content.data);
        if (content.encoding === 'zlib+base64') {
          try {
            const decompressed = await decompress(raw);
            term.write(decompressed);
          } catch { /* skip corrupt event */ }
        } else {
          term.write(raw);
        }
      }
    } catch (err) {
      term.writeln(`\r\n(History replay failed: ${err})`);
    }

    // Go live (use negotiated batch window from reconnect response)
    const negotiatedBatchMs = sessionContent.batch_ms || 200;
    const socket = new TerminalSocket(client, dmRoomId, { pollIntervalMs: 100, batchMs: negotiatedBatchMs });
    activeSocket = socket;

    // Wire: buffering status indicator
    const statusEl = document.getElementById('terminal-status');
    socket.onbuffering = (buffering) => {
      if (statusEl) {
        statusEl.textContent = buffering ? 'Buffering...' : '';
        statusEl.hidden = !buffering;
      }
    };

    term.onData(async (data) => {
      try { await socket.send(data); } catch { /* closed */ }
    });

    socket.onmessage = (event) => {
      term.write(new Uint8Array(event.data));
    };

    term.onResize(({ cols, rows }) => {
      if (socket.connected) socket.resize(cols, rows).catch(() => {});
    });

    const onWindowResize = () => fitAddon.fit();
    window.addEventListener('resize', onWindowResize);

    socket.onclose = () => {
      window.removeEventListener('resize', onWindowResize);
      term.writeln('\r\n\r\n[Session ended]');
      activeSocket = null;
    };

  } catch (err) {
    term.writeln(`\r\nError: ${err}`);
  }
}
