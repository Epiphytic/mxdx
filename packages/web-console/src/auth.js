import { WasmMatrixClient } from '../wasm/mxdx_core_wasm.js';

/**
 * Set up the login form handlers.
 * @param {object} callbacks
 * @param {function} callbacks.onLogin - Called with (client, sessionJson) on success
 */
export function setupAuth({ onLogin }) {
  const form = document.getElementById('login-form');
  const errorEl = document.getElementById('login-error');
  const statusEl = document.getElementById('login-status');
  const loginBtn = document.getElementById('login-btn');

  form.addEventListener('submit', async (e) => {
    e.preventDefault();
    errorEl.hidden = true;
    statusEl.hidden = false;
    loginBtn.disabled = true;

    const server = document.getElementById('server').value.trim();
    const username = document.getElementById('username').value.trim();
    const password = document.getElementById('password').value;

    try {
      statusEl.textContent = 'Connecting...';
      const client = await WasmMatrixClient.login(server, username, password);

      statusEl.textContent = 'Bootstrapping cross-signing...';
      try {
        await client.bootstrapCrossSigningIfNeeded(password);
        await client.verifyOwnIdentity();
      } catch {
        // Non-fatal: cross-signing may not be available
      }

      statusEl.textContent = 'Saving session...';
      const sessionJson = client.exportSession();

      statusEl.hidden = true;
      onLogin(client, sessionJson);
    } catch (err) {
      statusEl.hidden = true;
      errorEl.textContent = `Login failed: ${err}`;
      errorEl.hidden = false;
    } finally {
      loginBtn.disabled = false;
    }
  });
}
