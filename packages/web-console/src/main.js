import init, { WasmMatrixClient } from '../wasm/mxdx_core_wasm.js';
import { setupAuth } from './auth.js';
import { setupDashboard, stopDashboardRefresh } from './dashboard.js';
import { setupTerminalView } from './terminal-view.js';
import { saveSession, loadSession, clearSession } from './session-store.js';

// Persist state across Vite HMR reloads
const hmrState = import.meta.hot?.data ?? {};
let client = hmrState.client ?? null;
let wasmReady = hmrState.wasmReady ?? false;

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
      client = await WasmMatrixClient.restoreSession(savedSession);
      showDashboard();
      return;
    } catch (err) {
      console.warn('[boot] Session restore failed:', err);
      await clearSession();
    }
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
  document.getElementById('header').hidden = false;

  setupDashboard(client, {
    onOpenTerminal: (launcher) => showTerminal(launcher),
  });
}

function showTerminal(launcher) {
  document.getElementById('login').hidden = true;
  document.getElementById('dashboard').hidden = true;
  document.getElementById('terminal').hidden = false;

  document.getElementById('terminal-title').textContent = launcher.launcher_id;

  setupTerminalView(client, launcher, {
    onClose: () => showDashboard(),
  });
}

async function handleLogout() {
  stopDashboardRefresh();
  await clearSession();
  client = null;
  showLogin();
}

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
