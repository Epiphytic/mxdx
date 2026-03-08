# Terminal Session Persistence Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Users can navigate away from the web console and reconnect to their running terminal sessions, with tmux providing shell persistence and Matrix room history providing scrollback replay.

**Architecture:** tmux is spawned detached, then `script(1)` attaches to it for piped stdio. On disconnect, the script bridge dies but tmux keeps the shell alive. On reconnect, a new script+tmux-attach bridge is spawned. The browser replays recent `org.mxdx.terminal.data` events from the DM room history into xterm.js for scrollback restoration. Graceful fallback to bare `script(1)` when tmux is unavailable.

**Tech Stack:** Node.js, tmux, script(1), xterm.js, Matrix protocol (custom events)

**Security note:** All tmux subprocess calls use `execFileSync`/`spawn` (no shell) to prevent command injection. Session names are generated UUIDs, not user input.

---

### Task 1: LauncherConfig — `useTmux` setting

**Files:**
- Modify: `packages/launcher/src/config.js`
- Modify: `packages/launcher/bin/mxdx-launcher.js`

**Step 1: Add `useTmux` to config constructor and serialization**

In `packages/launcher/src/config.js`, add `useTmux` to the constructor, `fromArgs`, `save`, and `load`:

```javascript
// In constructor parameter destructuring, add:
useTmux = 'auto',

// In constructor body, add:
this.useTmux = useTmux;

// In fromArgs, add to the returned object:
useTmux: args.useTmux || 'auto',

// In save, add to the launcher object:
use_tmux: this.useTmux,

// In load, add to the returned object:
useTmux: l.use_tmux || 'auto',
```

**Step 2: Add CLI option to mxdx-launcher.js**

```javascript
.option('--use-tmux <mode>', 'tmux mode: auto|always|never', 'auto')
```

And in main():
```javascript
if (opts.useTmux) {
  config.useTmux = opts.useTmux;
}
```

**Step 3: Verify config round-trips**

Run: `node -e "const {LauncherConfig}=await import('./packages/launcher/src/config.js'); const c=new LauncherConfig({useTmux:'never'}); c.save('/tmp/test-config.toml'); const c2=LauncherConfig.load('/tmp/test-config.toml'); console.log(c2.useTmux)"`

Expected: `never`

**Step 4: Commit**

```bash
git add packages/launcher/src/config.js packages/launcher/bin/mxdx-launcher.js
git commit -m "feat: add use_tmux config setting to launcher"
```

---

### Task 2: PtyBridge — tmux support with script(1) fallback

**Files:**
- Modify: `packages/launcher/src/pty-bridge.js`

**Step 1: Add tmux detection helper**

Add imports and a module-level helper at the top of pty-bridge.js:

```javascript
import { spawn, execFileSync } from 'node:child_process';
import crypto from 'node:crypto';

function detectTmux() {
  try {
    const version = execFileSync('tmux', ['-V'], { encoding: 'utf8', timeout: 5000 }).trim();
    const match = version.match(/tmux\s+([\d.]+)/);
    return { available: true, version: match ? match[1] : version };
  } catch {
    return { available: false, version: null };
  }
}
```

**Step 2: Add static `tmuxInfo()` and `list()` methods**

```javascript
static tmuxInfo() {
  return detectTmux();
}

static list() {
  try {
    const output = execFileSync('tmux', ['list-sessions', '-F', '#{session_name}'], {
      encoding: 'utf8',
      timeout: 5000,
    }).trim();
    return output
      .split('\n')
      .filter(name => name.startsWith('mxdx-'));
  } catch {
    return [];
  }
}
```

**Step 3: Add new private fields**

```javascript
#tmuxName = null;
#persistent = false;
```

**Step 4: Refactor constructor for tmux mode**

Replace the constructor to support both modes. Uses `execFileSync` (no shell) for tmux commands:

```javascript
constructor(command, { cols = 80, rows = 24, cwd = '/tmp', env = {}, sessionName = null, useTmux = 'auto' } = {}) {
  this.#cols = cols;
  this.#rows = rows;

  const tmux = detectTmux();
  const wantTmux = useTmux === 'always' || (useTmux === 'auto' && tmux.available);

  if (useTmux === 'always' && !tmux.available) {
    throw new Error('tmux required (use_tmux=always) but not found on PATH');
  }

  this.#persistent = wantTmux;

  const shellEnv = {
    ...process.env,
    ...env,
    TERM: 'xterm-256color',
    COLUMNS: String(cols),
    LINES: String(rows),
  };

  if (wantTmux) {
    this.#tmuxName = sessionName || `mxdx-${crypto.randomUUID().slice(0, 8)}`;

    const existing = PtyBridge.list().includes(this.#tmuxName);

    if (!existing) {
      // Create detached tmux session — execFileSync (no shell)
      execFileSync('tmux', [
        'new-session', '-d', '-s', this.#tmuxName,
        '-x', String(cols), '-y', String(rows),
        command,
      ], { env: shellEnv, cwd, timeout: 5000 });
    } else {
      execFileSync('tmux', [
        'resize-window', '-t', this.#tmuxName,
        '-x', String(cols), '-y', String(rows),
      ], { timeout: 5000 });
    }

    // Attach via script for piped stdio
    this.#proc = spawn('script', ['-q', '/dev/null', '-c', `tmux attach -t ${this.#tmuxName}`], {
      cwd,
      env: shellEnv,
      stdio: ['pipe', 'pipe', 'pipe'],
    });
  } else {
    this.#tmuxName = null;
    this.#proc = spawn('script', ['-q', '/dev/null', '-c', command], {
      cwd,
      env: shellEnv,
      stdio: ['pipe', 'pipe', 'pipe'],
    });
  }

  this.#alive = true;

  this.#proc.stdout.on('data', (chunk) => {
    for (const cb of this.#dataCallbacks) cb(new Uint8Array(chunk));
  });

  this.#proc.stderr.on('data', (chunk) => {
    for (const cb of this.#dataCallbacks) cb(new Uint8Array(chunk));
  });

  this.#proc.on('close', () => { this.#alive = false; });
  this.#proc.on('error', () => { this.#alive = false; });
}
```

**Step 5: Add getters**

```javascript
get persistent() { return this.#persistent; }
get tmuxName() { return this.#tmuxName; }
```

**Step 6: Update `resize()` for tmux mode**

```javascript
resize(cols, rows) {
  if (!this.#alive) return;
  this.#cols = cols;
  this.#rows = rows;

  if (this.#tmuxName) {
    try {
      execFileSync('tmux', ['resize-window', '-t', this.#tmuxName, '-x', String(cols), '-y', String(rows)], { timeout: 5000 });
    } catch { /* best effort */ }
  } else if (this.#proc?.pid) {
    try {
      spawn('kill', ['-WINCH', String(this.#proc.pid)], { stdio: 'ignore' });
    } catch { /* best effort */ }
  }
}
```

**Step 7: Add `detach()` method**

```javascript
detach() {
  if (this.#proc) {
    this.#proc.kill();
    this.#proc = null;
  }
  if (!this.#tmuxName) {
    this.#alive = false;
  }
}
```

**Step 8: Update `kill()` to also kill tmux session**

```javascript
kill() {
  this.#alive = false;
  if (this.#proc) {
    this.#proc.kill();
    this.#proc = null;
  }
  if (this.#tmuxName) {
    try {
      execFileSync('tmux', ['kill-session', '-t', this.#tmuxName], { timeout: 5000 });
    } catch { /* session may already be dead */ }
    this.#tmuxName = null;
  }
}
```

**Step 9: Commit**

```bash
git add packages/launcher/src/pty-bridge.js
git commit -m "feat: add tmux persistence to PtyBridge with script(1) fallback"
```

---

### Task 3: Session Registry in Runtime

**Files:**
- Modify: `packages/launcher/src/runtime.js`

**Step 1: Add session registry field**

Add to the class field declarations:

```javascript
#sessionRegistry = new Map(); // sessionId -> { tmuxName, dmRoomId, sender, persistent, pty, createdAt }
```

**Step 2: Update `#handleInteractiveSession` to use tmux and register sessions**

After creating the PtyBridge, pass `useTmux` and register the session:

```javascript
const sessionId = crypto.randomUUID().slice(0, 8);
const pty = new PtyBridge(command, {
  cols, rows, cwd, env,
  useTmux: this.#config.useTmux || 'auto',
});

this.#sessionRegistry.set(sessionId, {
  tmuxName: pty.tmuxName,
  dmRoomId,
  sender,
  persistent: pty.persistent,
  pty,
  createdAt: new Date().toISOString(),
});
```

Update the session response to include `session_id` and `persistent`:

```javascript
await this.#sendSessionResponse(requestId, 'started', dmRoomId, {
  session_id: sessionId,
  persistent: pty.persistent,
});
```

**Step 3: Update session cleanup for persistence**

In the `pollForInput().finally()` block, use `detach()` for persistent sessions:

```javascript
pollForInput().finally(() => {
  if (pty.persistent) {
    pty.detach();
    this.#log.info('Interactive session bridge detached (tmux alive)', {
      request_id: requestId,
      session_id: sessionId,
    });
  } else {
    this.#sessionRegistry.delete(sessionId);
    this.#log.info('Interactive session ended', { request_id: requestId, session_id: sessionId });
  }
  this.#sendSessionResponse(requestId, 'ended', dmRoomId).catch(() => {});
  this.#activeSessions--;
});
```

**Step 4: Update `#sendSessionResponse` to accept extra fields**

```javascript
async #sendSessionResponse(requestId, status, roomId, extra = {}) {
  await Promise.race([
    this.#client.sendEvent(
      this.#topology.exec_room_id,
      'org.mxdx.terminal.session',
      JSON.stringify({
        request_id: requestId,
        status,
        room_id: roomId,
        ...extra,
      }),
    ),
    new Promise((_, reject) =>
      setTimeout(() => reject(new Error('sendSessionResponse timed out after 30s')), 30000),
    ),
  ]);
}
```

**Step 5: Add `crypto` import at top of runtime.js**

```javascript
import crypto from 'node:crypto';
```

**Step 6: Commit**

```bash
git add packages/launcher/src/runtime.js
git commit -m "feat: add session registry with persistence tracking"
```

---

### Task 4: Protocol — `list_sessions` and `reconnect` actions

**Files:**
- Modify: `packages/launcher/src/runtime.js`

**Step 1: Add `list_sessions` and `reconnect` handlers in `#processCommands`**

Add before the existing `interactive` action check:

```javascript
if (action === 'list_sessions') {
  this.#log.info('Session list requested', { request_id: requestId, sender });
  await this.#handleListSessions(requestId);
  continue;
}

if (action === 'reconnect') {
  this.#log.info('Session reconnect requested', { request_id: requestId, sender });
  await this.#handleReconnect(content, requestId, sender);
  continue;
}
```

**Step 2: Implement `#handleListSessions`**

```javascript
async #handleListSessions(requestId) {
  const sessions = [];
  for (const [sessionId, entry] of this.#sessionRegistry) {
    sessions.push({
      session_id: sessionId,
      room_id: entry.dmRoomId,
      persistent: entry.persistent,
      created_at: entry.createdAt,
    });
  }
  await this.#client.sendEvent(
    this.#topology.exec_room_id,
    'org.mxdx.terminal.sessions',
    JSON.stringify({ request_id: requestId, sessions }),
  );
}
```

**Step 3: Implement `#handleReconnect`**

This re-creates a PtyBridge attached to the existing tmux session and wires up the same I/O polling pattern:

```javascript
async #handleReconnect(content, requestId, sender) {
  const sessionId = content.session_id;
  const cols = content.cols || 80;
  const rows = content.rows || 24;

  const entry = this.#sessionRegistry.get(sessionId);
  if (!entry || !entry.persistent) {
    await this.#sendSessionResponse(requestId, 'expired', null);
    return;
  }

  if (entry.sender !== sender) {
    await this.#sendSessionResponse(requestId, 'rejected', null);
    return;
  }

  try {
    const pty = new PtyBridge('bash', {
      cols, rows,
      sessionName: entry.tmuxName,
      useTmux: 'always',
    });

    entry.pty = pty;
    this.#activeSessions++;

    await this.#sendSessionResponse(requestId, 'reconnected', entry.dmRoomId, {
      session_id: sessionId,
      persistent: true,
    });

    await new Promise((r) => setTimeout(r, 2000));
    await this.#client.syncOnce();

    // Forward PTY output -> DM room
    let sendSeq = 0;
    pty.onData(async (data) => {
      const seq = sendSeq++;
      let encoded, encoding;
      if (data.length >= 32) {
        const { deflateSync } = await import('node:zlib');
        const compressed = deflateSync(Buffer.from(data));
        encoded = Buffer.from(compressed).toString('base64');
        encoding = 'zlib+base64';
      } else {
        encoded = Buffer.from(data).toString('base64');
        encoding = 'base64';
      }
      try {
        await this.#client.sendEvent(
          entry.dmRoomId,
          'org.mxdx.terminal.data',
          JSON.stringify({ data: encoded, encoding, seq }),
        );
      } catch (err) {
        this.#log.warn('terminal.data send failed', { seq, error: String(err) });
      }
    });

    // Poll for input from client
    const pollForInput = async () => {
      while (pty.alive) {
        try {
          const dataEventJson = await this.#client.onRoomEvent(
            entry.dmRoomId, 'org.mxdx.terminal.data', 1,
          );
          if (dataEventJson && dataEventJson !== 'null') {
            const dataEvent = JSON.parse(dataEventJson);
            const eventContent = dataEvent.content || dataEvent;
            const eventSender = dataEvent.sender;
            if (eventSender && eventSender !== this.#client.userId()) {
              this.#processTerminalInput(eventContent, pty);
            }
          }
          const resizeJson = await this.#client.onRoomEvent(
            entry.dmRoomId, 'org.mxdx.terminal.resize', 1,
          );
          if (resizeJson && resizeJson !== 'null') {
            const resizeEvent = JSON.parse(resizeJson);
            const resizeContent = resizeEvent.content || resizeEvent;
            if (resizeContent.cols && resizeContent.rows) {
              pty.resize(resizeContent.cols, resizeContent.rows);
            }
          }
        } catch {
          await new Promise((r) => setTimeout(r, 1000));
        }
      }
    };

    pollForInput().finally(() => {
      if (pty.persistent) {
        pty.detach();
        this.#log.info('Reconnected session bridge detached', { session_id: sessionId });
      } else {
        this.#sessionRegistry.delete(sessionId);
      }
      this.#sendSessionResponse(requestId, 'ended', entry.dmRoomId).catch(() => {});
      this.#activeSessions--;
    });
  } catch (err) {
    this.#log.error('Reconnect failed', { session_id: sessionId, error: err.message });
    await this.#sendSessionResponse(requestId, 'expired', null);
  }
}
```

**Step 4: Commit**

```bash
git add packages/launcher/src/runtime.js
git commit -m "feat: add list_sessions and reconnect protocol actions"
```

---

### Task 5: Telemetry Enrichment

**Files:**
- Modify: `packages/launcher/src/runtime.js`

**Step 1: Import PtyBridge and add tmux info to telemetry**

Ensure PtyBridge is imported at the top:

```javascript
import { PtyBridge } from './pty-bridge.js';
```

In `#postTelemetry()`, add after the existing telemetry fields:

```javascript
const tmuxInfo = PtyBridge.tmuxInfo();
telemetry.tmux_available = tmuxInfo.available;
if (tmuxInfo.version) telemetry.tmux_version = tmuxInfo.version;
telemetry.session_persistence =
  (this.#config.useTmux === 'never') ? false :
  (this.#config.useTmux === 'always') ? true :
  tmuxInfo.available;
```

**Step 2: Commit**

```bash
git add packages/launcher/src/runtime.js
git commit -m "feat: add tmux availability to launcher telemetry"
```

---

### Task 6: Browser Dashboard — Active Sessions & Reconnect

**Files:**
- Modify: `packages/web-console/src/dashboard.js`

**Step 1: Update `setupDashboard` and `render` signatures**

```javascript
export function setupDashboard(client, { onOpenTerminal, onReconnect }) {
  stopDashboardRefresh();
  render(client, onOpenTerminal, onReconnect);
  refreshTimer = setInterval(() => {
    render(client, onOpenTerminal, onReconnect);
  }, 10000);
}

async function render(client, onOpenTerminal, onReconnect) {
```

**Step 2: Fetch active sessions for each launcher**

Inside the `for (const launcher of launchers)` loop, after fetching telemetry, add:

```javascript
let sessions = [];
try {
  const listRequestId = crypto.randomUUID();
  await client.sendEvent(launcher.exec_room_id, 'org.mxdx.command', JSON.stringify({
    action: 'list_sessions',
    request_id: listRequestId,
  }));
  await client.syncOnce();
  const sessionsJson = await client.onRoomEvent(
    launcher.exec_room_id, 'org.mxdx.terminal.sessions', 5,
  );
  if (sessionsJson && sessionsJson !== 'null') {
    const sessionsResponse = JSON.parse(sessionsJson);
    const sessionsContent = sessionsResponse.content || sessionsResponse;
    sessions = sessionsContent.sessions || [];
  }
} catch { /* sessions not available */ }
launcherData.push({ ...launcher, telemetry, sessions });
```

**Step 3: Update `renderCard` to show sessions and pass `onReconnect`**

Update function signature and grid call:

```javascript
function renderCard(launcher, client, onOpenTerminal, onReconnect) {
```

```javascript
grid.appendChild(renderCard(launcher, client, onOpenTerminal, onReconnect));
```

After the terminal button, add active sessions section:

```javascript
if (launcher.sessions && launcher.sessions.length > 0) {
  const sessionsDiv = document.createElement('div');
  sessionsDiv.className = 'sessions';

  const sessionsTitle = document.createElement('h4');
  sessionsTitle.textContent = 'Active Sessions';
  sessionsDiv.appendChild(sessionsTitle);

  for (const session of launcher.sessions) {
    const sessionRow = document.createElement('div');
    sessionRow.className = 'session-row';

    const label = document.createElement('span');
    const age = Math.floor((Date.now() - new Date(session.created_at).getTime()) / 60000);
    label.textContent = `${session.session_id} (${age}m ago)${session.persistent ? '' : ' — non-persistent'}`;
    sessionRow.appendChild(label);

    const reconnBtn = document.createElement('button');
    reconnBtn.className = 'btn btn-secondary';
    reconnBtn.textContent = 'Reconnect';
    reconnBtn.addEventListener('click', () => {
      if (refreshTimer) { clearInterval(refreshTimer); refreshTimer = null; }
      onReconnect(launcher, session);
    });
    sessionRow.appendChild(reconnBtn);

    sessionsDiv.appendChild(sessionRow);
  }
  card.appendChild(sessionsDiv);
}
```

**Step 4: Add session persistence to telemetry display**

In the telemetry section, after uptime:

```javascript
if (t.session_persistence != null) {
  appendTelemetryLine(telDiv, 'Session Persistence', t.session_persistence ? 'Yes (tmux)' : 'No');
}
```

**Step 5: Commit**

```bash
git add packages/web-console/src/dashboard.js
git commit -m "feat: show active sessions with reconnect buttons on dashboard"
```

---

### Task 7: Terminal View — Reconnect Flow with History Replay

**Files:**
- Modify: `packages/web-console/src/terminal-view.js`

**Step 1: Add decode/decompress helpers at top of file**

```javascript
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
```

**Step 2: Update `setupTerminalView` to accept `onSessionStarted` callback**

Change the destructuring:

```javascript
export async function setupTerminalView(client, launcher, { onClose, onSessionStarted }) {
```

After parsing the session response and getting dmRoomId, call the callback:

```javascript
const dmRoomId = sessionContent.room_id;

if (onSessionStarted) {
  onSessionStarted({
    session_id: sessionContent.session_id,
    room_id: dmRoomId,
    persistent: sessionContent.persistent ?? false,
  });
}
```

**Step 3: Add `reconnectTerminalView` export**

```javascript
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

    // Go live
    const socket = new TerminalSocket(client, dmRoomId, { pollIntervalMs: 100 });
    activeSocket = socket;

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
```

**Step 4: Commit**

```bash
git add packages/web-console/src/terminal-view.js
git commit -m "feat: add reconnect flow with history replay to terminal view"
```

---

### Task 8: Main.js — Session Memory, Reconnect Routing, beforeunload

**Files:**
- Modify: `packages/web-console/src/main.js`

**Step 1: Update terminal-view import**

```javascript
import { setupTerminalView, reconnectTerminalView } from './terminal-view.js';
```

**Step 2: Add session memory helpers**

```javascript
function saveTerminalSession(sessionId, dmRoomId, launcherExecRoomId, persistent) {
  sessionStorage.setItem('mxdx-terminal-session', JSON.stringify({
    sessionId, dmRoomId, launcherExecRoomId, persistent,
  }));
}

function loadTerminalSession() {
  const raw = sessionStorage.getItem('mxdx-terminal-session');
  if (!raw) return null;
  try { return JSON.parse(raw); } catch { return null; }
}

function clearTerminalSession() {
  sessionStorage.removeItem('mxdx-terminal-session');
}
```

**Step 3: Update `showDashboard` to pass `onReconnect` and clear session**

```javascript
function showDashboard() {
  document.getElementById('login').hidden = true;
  document.getElementById('dashboard').hidden = false;
  document.getElementById('terminal').hidden = true;
  document.getElementById('header').hidden = false;
  clearTerminalSession();

  setupDashboard(client, {
    onOpenTerminal: (launcher) => showTerminal(launcher),
    onReconnect: (launcher, session) => showReconnect(launcher, session),
  });
}
```

**Step 4: Add `showReconnect` function**

```javascript
function showReconnect(launcher, session) {
  document.getElementById('login').hidden = true;
  document.getElementById('dashboard').hidden = true;
  document.getElementById('terminal').hidden = false;

  document.getElementById('terminal-title').textContent = `${launcher.launcher_id} (reconnecting)`;

  saveTerminalSession(session.session_id, session.room_id, launcher.exec_room_id, session.persistent);

  reconnectTerminalView(client, launcher, session, {
    onClose: () => {
      clearTerminalSession();
      showDashboard();
    },
  });
}
```

**Step 5: Update `showTerminal` to save session info**

```javascript
function showTerminal(launcher) {
  document.getElementById('login').hidden = true;
  document.getElementById('dashboard').hidden = true;
  document.getElementById('terminal').hidden = false;

  document.getElementById('terminal-title').textContent = launcher.launcher_id;

  setupTerminalView(client, launcher, {
    onClose: () => {
      clearTerminalSession();
      showDashboard();
    },
    onSessionStarted: (sessionInfo) => {
      saveTerminalSession(
        sessionInfo.session_id,
        sessionInfo.room_id,
        launcher.exec_room_id,
        sessionInfo.persistent,
      );
    },
  });
}
```

**Step 6: Add beforeunload warning for non-persistent sessions**

```javascript
window.addEventListener('beforeunload', (e) => {
  const saved = loadTerminalSession();
  if (saved && !saved.persistent) {
    e.preventDefault();
    e.returnValue = 'Terminal session will be lost — tmux is not available on this launcher.';
  }
});
```

**Step 7: Add auto-reconnect on page reload**

In `boot()`, replace the `showDashboard(); return;` after successful session restore with:

```javascript
const savedTerminal = loadTerminalSession();
if (savedTerminal) {
  showReconnect(
    { exec_room_id: savedTerminal.launcherExecRoomId, launcher_id: 'reconnecting...' },
    { session_id: savedTerminal.sessionId, room_id: savedTerminal.dmRoomId, persistent: savedTerminal.persistent },
  );
} else {
  showDashboard();
}
return;
```

**Step 8: Commit**

```bash
git add packages/web-console/src/main.js packages/web-console/src/terminal-view.js
git commit -m "feat: session memory, auto-reconnect, and beforeunload warning"
```

---

### Task 9: CSS for Sessions UI

**Files:**
- Modify: `packages/web-console/src/style.css`

**Step 1: Add session styles**

```css
.sessions {
  margin-top: 1rem;
  border-top: 1px solid #30363d;
  padding-top: 0.75rem;
}

.sessions h4 {
  margin: 0 0 0.5rem;
  font-size: 0.85rem;
  color: #8b949e;
  font-weight: 500;
}

.session-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 0.35rem 0;
  font-size: 0.85rem;
}

.session-row span {
  color: #c9d1d9;
}

.btn-secondary {
  background: #21262d;
  color: #c9d1d9;
  border: 1px solid #30363d;
  padding: 0.25rem 0.75rem;
  border-radius: 6px;
  cursor: pointer;
  font-size: 0.8rem;
}

.btn-secondary:hover {
  background: #30363d;
  border-color: #8b949e;
}
```

**Step 2: Commit**

```bash
git add packages/web-console/src/style.css
git commit -m "feat: add CSS styles for session list and reconnect buttons"
```

---

### Task 10: Manual End-to-End Test

**Step 1:** Start launcher: `node packages/launcher/bin/mxdx-launcher.js`
- Verify logs show `tmux_available: true`, `session_persistence: true`

**Step 2:** Open `http://127.0.0.1:5173/`, log in, verify "Session Persistence: Yes (tmux)" in telemetry

**Step 3:** Click "Open Terminal", run `echo $$` and `sleep 300 &`

**Step 4:** Click "Back to Dashboard" — no beforeunload warning expected

**Step 5:** Dashboard should show active session with "Reconnect" button — click it

**Step 6:** Verify history replayed, shell alive, `jobs` shows background sleep

**Step 7:** Test `--use-tmux never`: restart launcher, open terminal, navigate away — verify beforeunload warning

**Step 8:** Commit any fixes: `git commit -m "fix: adjustments from manual e2e testing"`

---

### Summary of commits

1. `feat: add use_tmux config setting to launcher`
2. `feat: add tmux persistence to PtyBridge with script(1) fallback`
3. `feat: add session registry with persistence tracking`
4. `feat: add list_sessions and reconnect protocol actions`
5. `feat: add tmux availability to launcher telemetry`
6. `feat: show active sessions with reconnect buttons on dashboard`
7. `feat: add reconnect flow with history replay to terminal view`
8. `feat: session memory, auto-reconnect, and beforeunload warning`
9. `feat: add CSS styles for session list and reconnect buttons`
