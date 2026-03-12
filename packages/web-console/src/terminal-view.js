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
const roomTransports = new Map(); // roomId -> { transport, p2pCrypto, refCount, lastP2PAttempt, attemptInFlight }

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
  const turnOnly = localStorage.getItem('mxdx-p2p-turn-only') === 'true';
  return {
    enabled: enabled !== 'false',
    batchMs: Math.max(1, Math.min(1000, isNaN(batchMs) ? 10 : batchMs)),
    idleTimeoutS: Math.max(30, Math.min(3600, isNaN(idleTimeoutS) ? 300 : idleTimeoutS)),
    turnOnly,
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
 * @param {string} execRoomId - Exec room for P2P signaling (has established E2EE)
 */
async function getOrCreateRoomTransport(client, roomId, execRoomId) {
  const existing = roomTransports.get(roomId);
  if (existing) {
    existing.refCount++;
    // Re-attempt P2P if not currently active AND no attempt already in flight
    if (existing.transport.status !== 'p2p' && !existing.attemptInFlight) {
      const now = Date.now();
      if (now - existing.lastP2PAttempt >= 60000) {
        existing.lastP2PAttempt = now;
        existing.attemptInFlight = true;
        attemptBrowserP2P(client, existing.transport, roomId, null, execRoomId)
          .catch(() => { updateP2PStatus('matrix'); })
          .finally(() => { existing.attemptInFlight = false; });
      }
    }
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
        // Switch to low-latency batching for P2P
        if (activeSocket) activeSocket.batchMs = 5;
      } else {
        updateP2PStatus('matrix-lost');
        // Revert to rate-limit-safe batching for Matrix
        if (activeSocket) activeSocket.batchMs = 200;
      }
    },
    onReconnectNeeded: () => {
      const entry = roomTransports.get(roomId);
      if (!entry || entry.attemptInFlight) return;
      const now = Date.now();
      if (now - entry.lastP2PAttempt < 60000) return;
      entry.lastP2PAttempt = now;
      entry.attemptInFlight = true;
      attemptBrowserP2P(client, transport, roomId, sessionKey, execRoomId)
        .catch(() => { updateP2PStatus('matrix'); })
        .finally(() => { entry.attemptInFlight = false; });
    },
    onHangup: (reason) => {
      if (reason === 'idle_timeout') {
        updateP2PStatus('matrix', 'Matrix');
      }
    },
  });

  const entry = { transport, p2pCrypto, refCount: 1, lastP2PAttempt: Date.now(), attemptInFlight: true };
  roomTransports.set(roomId, entry);

  // Attempt P2P (non-blocking)
  attemptBrowserP2P(client, transport, roomId, sessionKey, execRoomId)
    .catch(() => { updateP2PStatus('matrix'); })
    .finally(() => { entry.attemptInFlight = false; });

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
 * Strategy: wait 5s for incoming launcher offer first (launcher offers immediately).
 * If no offer received in 5s, client offers as fallback.
 * First verified channel wins.
 */
async function attemptBrowserP2P(client, transport, dmRoomId, sessionKey, execRoomId) {
  // Skip if P2P is already active
  if (transport.status === 'p2p') {
    console.log('[p2p] Already connected, skipping attempt');
    return;
  }

  const session = JSON.parse(client.exportSession());
  const homeserverUrl = session.homeserver_url;
  const accessToken = session.access_token;
  let iceServers = [];

  const turnCreds = await fetchTurnCredentials(homeserverUrl, accessToken);
  console.log('[p2p] TURN credentials:', turnCreds ? `${turnCreds.uris?.length} URIs` : 'none');
  if (turnCreds) {
    iceServers = turnToIceServers(turnCreds);
  }
  console.log('[p2p] ICE servers:', JSON.stringify(iceServers.map(s => s.urls)));

  const turnOnly = localStorage.getItem('mxdx-p2p-turn-only') === 'true';
  // P2P signaling goes through exec room (established E2EE) not DM room
  // (newly-created DM rooms have unreliable Megolm key exchange).
  const signalingRoomId = execRoomId || dmRoomId;
  console.log('[p2p] turnOnly:', turnOnly, 'dmRoom:', dmRoomId, 'signalingRoom:', signalingRoomId);

  let settled = false;
  const settle = (channel, callId, role, p2pCrypto) => {
    if (settled) {
      channel.close();
      return false;
    }
    settled = true;
    if (p2pCrypto) transport.setP2PCrypto(p2pCrypto);
    transport.setDataChannel(channel);
    console.log(`[p2p] Data channel established (${role}, call=${callId})`);
    return true;
  };

  // Helper: wire ICE candidate batching
  const wireIceBatching = (channel, callId, partyId) => {
    const candidates = [];
    let candidateTimer = null;
    channel.onIceCandidate((candidate) => {
      candidates.push(candidate);
      if (candidateTimer) clearTimeout(candidateTimer);
      candidateTimer = setTimeout(async () => {
        const batch = candidates.splice(0);
        if (batch.length > 0) {
          const signaling = new P2PSignaling(
            {
              sendEvent: (roomId, type, content) => client.sendEvent(roomId, type, content),
              onRoomEvent: (roomId, cb) => client.onRoomEvent(roomId, cb),
            },
            signalingRoomId,
            client.userId(),
          );
          await signaling.sendCandidates({ callId, partyId, candidates: batch }).catch(() => {});
        }
      }, 100);
    });

    // Detect TURN-specific errors (browser only)
    // Only update status if P2P hasn't already connected (late errors are noise)
    channel.onIceCandidateError((err) => {
      if (settled || transport.status === 'p2p') return;
      if (err.errorCode === 486 || err.errorCode === 508) {
        updateP2PStatus('turn-limit');
      } else if (err.errorCode === 701) {
        updateP2PStatus('turn-unreachable');
      }
    });
  };

  // Helper: poll for ICE candidates (skip own by party_id, add to channel as they arrive)
  // Note: uses call_id + party_id to filter, NOT sender — browser and launcher may
  // share the same Matrix user ID (same-user, different device).
  // Phase 1: scan existing room history (candidates may arrive before polling starts).
  // Phase 2: poll for new candidates via onRoomEvent.
  const pollCandidates = async (channel, callId, ownPartyId) => {
    // Phase 1: Scan existing room history for candidates that arrived before polling started.
    // This fixes a race where onRoomEvent marks existing events as "seen" and never returns them.
    try {
      const existingJson = await client.findRoomEvents(signalingRoomId, 'm.call.candidates', 20);
      const existing = JSON.parse(existingJson);
      for (const evt of existing) {
        const content = evt.content || evt;
        if (content.call_id !== callId) continue;
        if (content.party_id === ownPartyId) continue;
        const cands = content.candidates || [];
        console.log('[p2p] Found', cands.length, 'existing candidates for call', callId);
        for (const c of cands) {
          channel.addIceCandidate(c);
        }
      }
    } catch (err) {
      console.log('[p2p] findRoomEvents scan failed:', err.message);
    }

    // Phase 2: Poll for new candidates arriving after polling started
    for (let i = 0; i < 30; i++) {
      if (settled && i > 5) return; // After settle, poll a few more for stragglers
      const candJson = await client.onRoomEvent(signalingRoomId, 'm.call.candidates', 1);
      if (!candJson || candJson === 'null') continue;
      try {
        const candEvent = JSON.parse(candJson);
        const candContent = candEvent.content || candEvent;
        if (candContent.call_id !== callId) continue;
        if (candContent.party_id === ownPartyId) continue; // Skip own candidates
        const cands = candContent.candidates || [];
        console.log('[p2p] Got', cands.length, 'new candidates for call', callId);
        for (const c of cands) {
          channel.addIceCandidate(c);
        }
      } catch { /* malformed */ }
    }
  };

  // Pre-generate offerer's call_id so the answerer can skip our own invite
  // (can't use sender check since browser and launcher may share the same Matrix user ID)
  const offererCallId = P2PSignaling.generateCallId();

  // --- Answerer path: wait for launcher's offer ---
  const answererPath = async () => {
    console.log('[p2p] Answerer: polling for launcher invite...');
    let inviteEvent = null;

    // Poll for incoming m.call.invite (launcher sends after 5s delay)
    // onRoomEvent marks existing events as "seen" so we only get NEW invites
    const answererDeadline = Date.now() + 20_000; // 20s to receive launcher invite
    while (Date.now() < answererDeadline && !settled) {
      const json = await client.onRoomEvent(signalingRoomId, 'm.call.invite', 3);
      if (!json || json === 'null') continue;
      try {
        const evt = JSON.parse(json);
        const evtContent = evt.content || evt;
        if (evtContent.call_id === offererCallId) {
          console.log('[p2p] Answerer: skipping own invite');
          continue;
        }
        inviteEvent = evt;
        break;
      } catch { continue; }
    }

    if (settled || !inviteEvent) {
      throw new Error('No incoming offer from launcher');
    }

    const inviteContent = inviteEvent.content || inviteEvent;
    console.log('[p2p] Answerer: got invite from', inviteEvent.sender, 'call_id:', inviteContent.call_id);
    const callId = inviteContent.call_id;
    if (!callId || !inviteContent.offer?.sdp) {
      throw new Error('Invalid invite');
    }

    // Extract shared session key from offerer's invite
    let answererP2PCrypto = null;
    if (inviteContent.session_key) {
      answererP2PCrypto = await createP2PCrypto(inviteContent.session_key);
      console.log('[p2p] Answerer: using shared session key from invite');
    }

    const channel = new BrowserWebRTCChannel({ iceServers, turnOnly });
    channel.onStateChange((state) => console.log('[p2p] Answerer ICE state:', state));
    const partyId = P2PSignaling.generatePartyId();
    wireIceBatching(channel, callId, partyId);

    const answer = await channel.acceptOffer({ sdp: inviteContent.offer.sdp, type: 'offer' });
    const signaling = new P2PSignaling(
      {
        sendEvent: (roomId, type, content) => client.sendEvent(roomId, type, content),
        onRoomEvent: (roomId, cb) => client.onRoomEvent(roomId, cb),
      },
      signalingRoomId,
      client.userId(),
    );
    await signaling.sendAnswer({ callId, partyId, sdp: answer.sdp });

    console.log('[p2p] Answerer: answer sent, waiting for data channel...');
    pollCandidates(channel, callId, partyId).catch(() => {});
    await Promise.race([
      channel.waitForDataChannel(),
      new Promise((_, reject) => setTimeout(() => reject(new Error('Data channel open timeout (30s)')), 30_000)),
    ]);
    console.log('[p2p] Answerer: data channel open!');
    settle(channel, callId, 'answerer', answererP2PCrypto);
  };

  // --- Offerer path: client creates offer after 5s delay ---
  const offererPath = async () => {
    console.log('[p2p] Offerer: waiting 8s before offering...');
    // Wait 8s to give launcher's offer a chance to arrive and be answered first
    await new Promise((r) => setTimeout(r, 8000));
    if (settled) { console.log('[p2p] Offerer: answerer already settled, skipping'); return; }

    const channel = new BrowserWebRTCChannel({ iceServers, turnOnly });
    channel.onStateChange((state) => console.log('[p2p] Offerer ICE state:', state));
    const callId = offererCallId;
    const partyId = P2PSignaling.generatePartyId();
    wireIceBatching(channel, callId, partyId);

    // Generate session key for P2P encryption — shared via E2EE Matrix invite
    const offerSessionKey = await generateSessionKey();
    const offerP2PCrypto = await createP2PCrypto(offerSessionKey);

    const offer = await channel.createOffer();
    const signaling = new P2PSignaling(
      {
        sendEvent: (roomId, type, content) => client.sendEvent(roomId, type, content),
        onRoomEvent: (roomId, cb) => client.onRoomEvent(roomId, cb),
      },
      signalingRoomId,
      client.userId(),
    );

    try {
      await signaling.sendInvite({ callId, partyId, sdp: offer.sdp, lifetime: 30000, sessionKey: offerSessionKey });
    } catch (err) {
      channel.close();
      if (String(err).includes('429') || String(err).includes('M_LIMIT_EXCEEDED')) {
        updateP2PStatus('rate-limited', 'Matrix (rate-limited)');
        throw new Error('P2P signaling rate-limited');
      }
      throw err;
    }

    console.log('[p2p] Offerer: invite sent, waiting for answer...');
    // Note: do NOT filter by sender — browser and launcher may share the same
    // Matrix user ID (same-user, different device). The call_id check is sufficient.
    let answerContent = null;
    const offerDeadline = Date.now() + 30_000;
    while (Date.now() < offerDeadline && !settled) {
      const answerJson = await client.onRoomEvent(signalingRoomId, 'm.call.answer', 5);
      if (!answerJson || answerJson === 'null') continue;
      try {
        const answerEvent = JSON.parse(answerJson);
        const content = answerEvent.content || answerEvent;
        if (content.call_id !== callId) continue;
        answerContent = content;
        break;
      } catch { continue; }
    }
    if (!answerContent || settled) {
      channel.close();
      throw new Error('No P2P answer received');
    }

    console.log('[p2p] Offerer: answer received, accepting');
    await channel.acceptAnswer({ sdp: answerContent.answer.sdp, type: answerContent.answer.type });
    pollCandidates(channel, callId, partyId).catch(() => {});
    await Promise.race([
      channel.waitForDataChannel(),
      new Promise((_, reject) => setTimeout(() => reject(new Error('Data channel open timeout (30s)')), 30_000)),
    ]);
    settle(channel, callId, 'offerer', offerP2PCrypto);
  };

  // Race both paths — answerer (launcher's offer) vs offerer (client fallback)
  console.log('[p2p] Starting race: answerer + offerer');
  await Promise.any([
    answererPath().catch((err) => { console.log('[p2p] Answerer failed:', err.message); throw err; }),
    offererPath().catch((err) => { console.log('[p2p] Offerer failed:', err.message); throw err; }),
  ]).catch((err) => {
    const msg = err instanceof AggregateError
      ? err.errors.map(e => e.message).join('; ')
      : err.message;
    console.log('[p2p] Both paths failed:', msg);
    throw new Error(msg);
  });
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
    const p2pTransport = await getOrCreateRoomTransport(client, dmRoomId, launcher.exec_room_id);
    // Start with P2P-optimized batching (5ms) — falls back gracefully if on Matrix
    const effectiveBatchMs = (p2pTransport.status === 'p2p') ? 5 : negotiatedBatchMs;
    const socket = new TerminalSocket(p2pTransport, dmRoomId, { pollIntervalMs: 100, batchMs: effectiveBatchMs, sessionId });
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
    if (typeof dmRoomId !== 'undefined') releaseRoomTransport(dmRoomId);
    term.writeln(`\r\nError: ${err}`);
  }
}

export async function reconnectTerminalView(client, launcher, session, { onClose, onReconnectFailed }) {
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
      if (onReconnectFailed) onReconnectFailed();
      return;
    }

    const response = JSON.parse(responseJson);
    const sessionContent = response.content || response;

    if (sessionContent.status === 'expired') {
      term.writeln('\r\nSession expired — tmux session no longer exists.');
      if (onReconnectFailed) onReconnectFailed();
      return;
    }

    if (sessionContent.status !== 'reconnected' || !sessionContent.room_id) {
      term.writeln(`\r\nReconnect failed: ${sessionContent.status || 'unknown'}`);
      if (onReconnectFailed) onReconnectFailed();
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
    const p2pTransport = await getOrCreateRoomTransport(client, dmRoomId, launcher.exec_room_id);
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
    if (typeof dmRoomId !== 'undefined') releaseRoomTransport(dmRoomId);
    term.writeln(`\r\nError: ${err}`);
  }
}
