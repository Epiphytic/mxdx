import init, { WasmMatrixClient } from '../wasm/mxdx_core_wasm.js';
import { setupAuth } from './auth.js';
import { setupDashboard } from './dashboard.js';
import { setupTerminalView } from './terminal-view.js';

const SESSION_KEY = 'mxdx-session';

let client = null;
let wasmReady = false;

async function boot() {
  // Initialize WASM
  await init();
  wasmReady = true;

  // Try restoring saved session
  const savedSession = localStorage.getItem(SESSION_KEY);
  if (savedSession) {
    try {
      client = await WasmMatrixClient.restoreSession(savedSession);
      showDashboard();
      return;
    } catch {
      localStorage.removeItem(SESSION_KEY);
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

function handleLogout() {
  localStorage.removeItem(SESSION_KEY);
  if (client) {
    client.free();
    client = null;
  }
  showLogin();
}

// Wire up auth
setupAuth({
  getWasmClient: () => wasmReady ? WasmMatrixClient : null,
  onLogin: (newClient, sessionJson) => {
    client = newClient;
    localStorage.setItem(SESSION_KEY, sessionJson);
    showDashboard();
  },
});

// Wire up nav
document.getElementById('nav-dashboard').addEventListener('click', () => {
  if (client) showDashboard();
});
document.getElementById('nav-logout').addEventListener('click', handleLogout);

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
