import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { TerminalSocket } from './terminal-socket.js';
import { BrowserWebRTCChannel } from './webrtc-channel.js';
import { P2PSignaling } from '../../core/p2p-signaling.js';
import { P2PTransport } from '../../core/p2p-transport.js';
import { generateSessionKey, createP2PCrypto } from '../../core/p2p-crypto.js';
import { fetchTurnCredentials, turnToIceServers } from '../../core/turn-credentials.js';

let activeSocket = null;
let activeTerminal = null;
const roomTransports = new Map(); // roomId -> { transport, p2pCrypto, refCount, lastP2PAttempt }

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

/** Read P2P settings from localStorage with sane clamping. */
function getP2PSettings() {
  const enabled = localStorage.getItem('mxdx-p2p-enabled');
  const batchMs = parseInt(localStorage.getItem('mxdx-p2p-batch-ms') || '10', 10);
  const idleTimeoutS = parseInt(localStorage.getItem('mxdx-p2p-idle-timeout-s') || '300', 10);
  return {
    enabled: enabled !== 'false',
    batchMs: Math.max(1, Math.min(1000, isNaN(batchMs) ? 10 : batchMs)),
    idleTimeoutS: Math.max(30, Math.min(3600, isNaN(idleTimeoutS) ? 300 : idleTimeoutS)),
  };
}

/** Update the #terminal-status element with P2P status info. */
function updateP2PStatus(status, detail) {
  const el = document.getElementById('terminal-status');
  if (!el) return;

  // Remove all status classes
  el.className = '';

  switch (status) {
    case 'p2p':
      el.textContent = 'P2P';
      el.classList.add('status-p2p');
      el.hidden = false;
      break;
    case 'connecting':
      el.textContent = 'P2P connecting...';
      el.classList.add('status-connecting');
      el.hidden = false;
      break;
    case 'matrix':
      el.textContent = detail || 'Matrix';
      el.classList.add('status-matrix');
      el.hidden = false;
      break;
    case 'matrix-lost':
      el.textContent = 'Matrix (P2P lost)';
      el.classList.add('status-matrix-lost');
      el.hidden = false;
      // Fade to dim after 5s
      setTimeout(() => {
        el.textContent = 'Matrix';
        el.classList.remove('status-matrix-lost');
        el.classList.add('status-matrix');
      }, 5000);
      break;
    case 'turn-limit':
      el.textContent = 'P2P unavailable (TURN limit)';
      el.classList.add('status-turn-limit');
      el.hidden = false;
      break;
    case 'turn-unreachable':
      el.textContent = 'P2P unavailable';
      el.classList.add('status-turn-unreachable');
      el.hidden = false;
      break;
    case 'rate-limited':
      el.textContent = detail || 'Rate-limited';
      el.classList.add('status-rate-limited');
      el.hidden = false;
      break;
    default:
      el.hidden = true;
      break;
  }
}

/**
 * Get or create a shared P2P transport for a room.
 * Returns a P2PTransport (with Matrix fallback) or a thin Matrix wrapper.
 * Multiple sessions in the same room share one transport via refcounting.
 */
async function getOrCreateRoomTransport(client, roomId) {
  const existing = roomTransports.get(roomId);
  if (existing) {
    existing.refCount++;
    return existing.transport;
  }

  const settings = getP2PSettings();
  if (!settings.enabled) {
    const transport = {
      sendEvent: (rid, type, content) => client.sendEvent(rid, type, content),
      onRoomEvent: (rid, type, timeout) => client.onRoomEvent(rid, type, timeout),
      close: () => {},
    };
    // Don't cache disabled transport — no refcount needed
    return transport;
  }

  updateP2PStatus('connecting');

  const sessionKey = await generateSessionKey();
  const p2pCrypto = await createP2PCrypto(sessionKey);

  const transport = P2PTransport.create({
    matrixClient: {
      sendEvent: (rid, type, content) => client.sendEvent(rid, type, content),
      onRoomEvent: (rid, type, timeout) => client.onRoomEvent(rid, type, timeout),
      userId: () => client.userId(),
    },
    p2pCrypto,
    localDeviceId: client.deviceId(),
    idleTimeoutMs: settings.idleTimeoutS * 1000,
    onStatusChange: (status) => {
      if (status === 'p2p') {
        updateP2PStatus('p2p');
      } else {
        updateP2PStatus('matrix-lost');
      }
    },
    onReconnectNeeded: () => {
      const entry = roomTransports.get(roomId);
      if (!entry) return;
      const now = Date.now();
      if (now - entry.lastP2PAttempt < 60000) return;
      entry.lastP2PAttempt = now;
      attemptBrowserP2P(client, transport, roomId, sessionKey).catch(() => {
        updateP2PStatus('matrix');
      });
    },
    onHangup: (reason) => {
      if (reason === 'idle_timeout') {
        updateP2PStatus('matrix', 'Matrix');
      }
    },
  });

  const entry = { transport, p2pCrypto, refCount: 1, lastP2PAttempt: Date.now() };
  roomTransports.set(roomId, entry);

  // Attempt P2P (non-blocking)
  attemptBrowserP2P(client, transport, roomId, sessionKey).catch(() => {
    updateP2PStatus('matrix');
  });

  return transport;
}

/**
 * Release a reference to a room's shared P2P transport.
 * Closes the transport when the last session in the room ends.
 */
function releaseRoomTransport(roomId) {
  const entry = roomTransports.get(roomId);
  if (!entry) return;
  entry.refCount--;
  if (entry.refCount <= 0) {
    entry.transport.close();
    roomTransports.delete(roomId);
  }
}

/**
 * Attempt to establish browser P2P WebRTC connection.
 */
async function attemptBrowserP2P(client, transport, dmRoomId, sessionKey) {
  const session = JSON.parse(client.exportSession());
  const homeserverUrl = session.homeserver_url;
  const accessToken = session.access_token;
  let iceServers = [];

  const turnCreds = await fetchTurnCredentials(homeserverUrl, accessToken);
  if (turnCreds) {
    iceServers = turnToIceServers(turnCreds);
  }

  const channel = new BrowserWebRTCChannel({ iceServers });

  // Detect TURN-specific errors (browser only)
  channel.onIceCandidateError((err) => {
    if (err.errorCode === 486 || err.errorCode === 508) {
      updateP2PStatus('turn-limit');
    } else if (err.errorCode === 701) {
      updateP2PStatus('turn-unreachable');
    }
  });

  const signaling = new P2PSignaling(
    {
      sendEvent: (roomId, type, content) => client.sendEvent(roomId, type, content),
      onRoomEvent: (roomId, cb) => client.onRoomEvent(roomId, cb),
    },
    dmRoomId,
    client.userId(),
  );

  const callId = P2PSignaling.generateCallId();
  const partyId = P2PSignaling.generatePartyId();

  // Batch ICE candidates
  const candidates = [];
  let candidateTimer = null;
  channel.onIceCandidate((candidate) => {
    candidates.push(candidate);
    if (candidateTimer) clearTimeout(candidateTimer);
    candidateTimer = setTimeout(async () => {
      const batch = candidates.splice(0);
      if (batch.length > 0) {
        await signaling.sendCandidates({ callId, partyId, candidates: batch }).catch(() => {});
      }
    }, 100);
  });

  const offer = await channel.createOffer();
  try {
    await signaling.sendInvite({ callId, partyId, sdp: offer.sdp, lifetime: 30000 });
  } catch (err) {
    channel.close();
    if (String(err).includes('429') || String(err).includes('M_LIMIT_EXCEEDED')) {
      updateP2PStatus('rate-limited', 'Matrix (rate-limited)');
      throw new Error('P2P signaling rate-limited');
    }
    throw err;
  }

  // Wait for answer
  const answerJson = await client.onRoomEvent(dmRoomId, 'm.call.answer', 30);
  if (!answerJson || answerJson === 'null') {
    channel.close();
    throw new Error('No P2P answer received');
  }

  const answerEvent = JSON.parse(answerJson);
  const answerContent = answerEvent.content || answerEvent;
  if (answerContent.call_id !== callId) {
    channel.close();
    throw new Error('Answer call_id mismatch');
  }

  await channel.acceptAnswer({ sdp: answerContent.answer.sdp, type: answerContent.answer.type });

  // Poll for remote ICE candidates in background
  const pollCandidates = async () => {
    for (let i = 0; i < 30; i++) {
      const candJson = await client.onRoomEvent(dmRoomId, 'm.call.candidates', 1);
      if (!candJson || candJson === 'null') continue;
      try {
        const candEvent = JSON.parse(candJson);
        const candContent = candEvent.content || candEvent;
        if (candContent.call_id !== callId) continue;
        for (const c of (candContent.candidates || [])) {
          channel.addIceCandidate(c);
        }
      } catch { /* malformed candidate event */ }
    }
  };
  pollCandidates().catch(() => {});

  await channel.waitForDataChannel();

  transport.setDataChannel(channel);
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

    // Accept DM invitation (may already be joined from a previous session)
    await client.syncOnce();
    try { await client.joinRoom(dmRoomId); } catch { /* already joined */ }
    await client.syncOnce();

    term.writeln('Connected.\r\n');
    term.clear();

    // Create TerminalSocket on DM room (use negotiated batch window)
    const negotiatedBatchMs = sessionContent.batch_ms || 200;
    const sessionId = sessionContent.session_id;

    // Set up P2P transport (or Matrix-only wrapper) — non-blocking
    const p2pTransport = await getOrCreateRoomTransport(client, dmRoomId);
    const socket = new TerminalSocket(p2pTransport, dmRoomId, { pollIntervalMs: 100, batchMs: negotiatedBatchMs, sessionId });
    activeSocket = socket;

    // Wire: buffering status indicator
    const statusEl = document.getElementById('terminal-status');
    socket.onbuffering = (buffering) => {
      if (statusEl && !statusEl.classList.contains('status-p2p')) {
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
      releaseRoomTransport(dmRoomId);
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

    // Set up P2P transport (or Matrix-only wrapper) — non-blocking
    const p2pTransport = await getOrCreateRoomTransport(client, dmRoomId);
    const socket = new TerminalSocket(p2pTransport, dmRoomId, { pollIntervalMs: 100, batchMs: negotiatedBatchMs, sessionId: session.session_id });
    activeSocket = socket;

    // Wire: buffering status indicator
    const statusEl = document.getElementById('terminal-status');
    socket.onbuffering = (buffering) => {
      if (statusEl && !statusEl.classList.contains('status-p2p')) {
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
      releaseRoomTransport(dmRoomId);
      term.writeln('\r\n\r\n[Session ended]');
      activeSocket = null;
    };

  } catch (err) {
    term.writeln(`\r\nError: ${err}`);
  }
}
