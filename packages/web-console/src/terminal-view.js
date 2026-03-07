import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { TerminalSocket } from './terminal-socket.js';

let activeSocket = null;
let activeTerminal = null;

/**
 * Set up an interactive terminal session in the browser.
 * @param {object} client - WasmMatrixClient
 * @param {object} launcher - Launcher info with exec_room_id
 * @param {object} callbacks
 * @param {function} callbacks.onClose - Called when session ends
 */
export async function setupTerminalView(client, launcher, { onClose }) {
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
    // Send interactive session request
    const requestId = crypto.randomUUID();
    const cols = term.cols;
    const rows = term.rows;

    await client.sendEvent(
      launcher.exec_room_id,
      'org.mxdx.command',
      JSON.stringify({
        action: 'interactive',
        command: '/bin/bash',
        request_id: requestId,
        cols,
        rows,
      }),
    );

    term.writeln('Waiting for session...');

    // Wait for session response
    const responseJson = await client.onRoomEvent(
      launcher.exec_room_id,
      'org.mxdx.terminal.session',
      30,
    );

    if (!responseJson || responseJson === 'null') {
      term.writeln('\r\nTimeout waiting for session response.');
      return;
    }

    const response = JSON.parse(responseJson);
    const sessionContent = response.content || response;

    if (sessionContent.status !== 'started' || !sessionContent.room_id) {
      term.writeln(`\r\nSession rejected: ${sessionContent.status || 'unknown'}`);
      return;
    }

    const dmRoomId = sessionContent.room_id;
    term.writeln(`Session started. Joining room...`);

    // Accept DM invitation
    await client.syncOnce();
    await client.joinRoom(dmRoomId);
    await client.syncOnce();

    term.writeln('Connected.\r\n');
    term.clear();

    // Create TerminalSocket on DM room
    const socket = new TerminalSocket(client, dmRoomId, { pollIntervalMs: 100 });
    activeSocket = socket;

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
