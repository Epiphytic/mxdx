import init, { WasmMatrixClient } from '../../core/wasm/web/mxdx_core_wasm.js';

// Coerce WASM onRoomEvent 'null' string → JS null at the JS/WASM boundary.
// The WASM layer serializes None as the string 'null'; callers expect JS null.
const _origOnRoomEvent = WasmMatrixClient.prototype.onRoomEvent;
WasmMatrixClient.prototype.onRoomEvent = async function (...args) {
  const result = await _origOnRoomEvent.apply(this, args);
  return result === 'null' ? null : result;
};

import { setupAuth } from './auth.js';
import { setupDashboard, stopDashboardRefresh } from './dashboard.js';
import { setupTerminalView, reconnectTerminalView } from './terminal-view.js';
import { saveSession, loadSession, clearSession } from './session-store.js';
import { setupSettings } from './settings.js';

// Persist state across Vite HMR reloads
const hmrState = import.meta.hot?.data ?? {};
let client = hmrState.client ?? null;
let wasmReady = hmrState.wasmReady ?? false;

// Session memory helpers (sessionStorage — survives page reload, not tab close)
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

async function boot() {
  // Skip boot if we already have a live client (HMR reload)
  if (client && wasmReady) {
    showDashboard();
    return;
  }

  // Initialize WASM
  await init();
  wasmReady = true;

  // Migrate any session from localStorage to IndexedDB (one-time)
  const legacySession = localStorage.getItem('mxdx-session');
  if (legacySession) {
    await saveSession(legacySession);
    localStorage.removeItem('mxdx-session');
  }

  // Try restoring saved session from IndexedDB
  const savedSession = await loadSession();
  if (savedSession) {
    try {
      const parsed = JSON.parse(savedSession);
      console.log(`[boot] Found saved session — restoring device ${parsed.device_id} for ${parsed.user_id}`);
      client = await WasmMatrixClient.restoreSession(savedSession);
      console.log(`[boot] Session restored — reusing device ${parsed.device_id}`);

      // Auto-reconnect if we had an active terminal session
      const savedTerminal = loadTerminalSession();
      if (savedTerminal) {
        console.log(`[boot] Reconnecting to terminal session ${savedTerminal.sessionId}`);
        showReconnect(
          { exec_room_id: savedTerminal.launcherExecRoomId, launcher_id: 'reconnecting...' },
          { session_id: savedTerminal.sessionId, room_id: savedTerminal.dmRoomId, persistent: savedTerminal.persistent },
        );
      } else {
        showDashboard();
      }
      return;
    } catch (err) {
      console.warn('[boot] Session restore failed:', err);
      console.log('[boot] Clearing invalid session — next login will create a new device');
      await clearSession();
    }
  } else {
    console.log('[boot] No saved session found — showing login form');
  }

  showLogin();
}

function showLogin() {
  document.getElementById('login').hidden = false;
  document.getElementById('dashboard').hidden = true;
  document.getElementById('terminal').hidden = true;
  document.getElementById('header').hidden = true;
}

function showDashboard() {
  document.getElementById('login').hidden = true;
  document.getElementById('dashboard').hidden = false;
  document.getElementById('terminal').hidden = true;
  document.getElementById('settings').hidden = true;
  document.getElementById('header').hidden = false;
  clearTerminalSession();

  // Update nav active state
  document.getElementById('nav-dashboard').classList.add('active');
  document.getElementById('nav-settings').classList.remove('active');

  setupDashboard(client, {
    onOpenTerminal: (launcher) => showTerminal(launcher),
    onReconnect: (launcher, session) => showReconnect(launcher, session),
  });
}

function showSettings() {
  document.getElementById('login').hidden = true;
  document.getElementById('dashboard').hidden = true;
  document.getElementById('terminal').hidden = true;
  document.getElementById('settings').hidden = false;
  document.getElementById('header').hidden = false;
  clearTerminalSession();
  stopDashboardRefresh();

  // Update nav active state
  document.getElementById('nav-dashboard').classList.remove('active');
  document.getElementById('nav-settings').classList.add('active');

  setupSettings(client);
}

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
    onReconnectFailed: () => {
      clearTerminalSession();
      setTimeout(() => showDashboard(), 2000);
    },
  });
}

async function handleLogout() {
  stopDashboardRefresh();
  clearTerminalSession();
  // Keep session in IndexedDB — preserves device_id and Megolm keys
  // so restoreSession() can reuse them on next login
  client = null;
  showLogin();
}

// Warn before closing non-persistent terminal sessions
window.addEventListener('beforeunload', (e) => {
  const saved = loadTerminalSession();
  if (saved && !saved.persistent) {
    e.preventDefault();
    e.returnValue = 'Terminal session will be lost — tmux is not available on this launcher.';
  }
});

// Wire up auth
setupAuth({
  getWasmClient: () => wasmReady ? WasmMatrixClient : null,
  onLogin: async (newClient, sessionJson) => {
    client = newClient;
    await saveSession(sessionJson);
    showDashboard();
  },
});

// Wire up nav
document.getElementById('nav-dashboard').addEventListener('click', () => {
  if (client) showDashboard();
});
document.getElementById('nav-settings').addEventListener('click', () => {
  if (client) showSettings();
});
document.getElementById('nav-logout').addEventListener('click', () => handleLogout());

// Wire up terminal back button
document.getElementById('terminal-back').addEventListener('click', () => {
  showDashboard();
});

boot().catch((err) => {
  console.error('Boot failed:', err);
  const errorEl = document.getElementById('login-error');
  if (errorEl) {
    errorEl.textContent = `Failed to initialize: ${err instanceof Error ? err.message : String(err)}`;
    errorEl.hidden = false;
  }
  document.getElementById('login').hidden = false;
});

// Vite HMR: preserve client state and clean up intervals
if (import.meta.hot) {
  import.meta.hot.dispose((data) => {
    data.client = client;
    data.wasmReady = wasmReady;
    stopDashboardRefresh();
  });
}
