import { loadSession } from './session-store.js';

/**
 * Set up the login form handlers.
 * @param {object} callbacks
 * @param {function} callbacks.onLogin - Called with (client, sessionJson) on success
 * @param {function} callbacks.getWasmClient - Returns the WasmMatrixClient class (after init)
 */
export function setupAuth({ onLogin, getWasmClient }) {
  const form = document.getElementById('login-form');
  const errorEl = document.getElementById('login-error');
  const statusEl = document.getElementById('login-status');
  const loginBtn = document.getElementById('login-btn');

  function showError(msg) {
    statusEl.hidden = true;
    errorEl.textContent = msg;
    errorEl.hidden = false;
    console.error('[auth]', msg);
  }

  function showStatus(msg) {
    errorEl.hidden = true;
    statusEl.textContent = msg;
    statusEl.hidden = false;
  }

  form.addEventListener('submit', async (e) => {
    e.preventDefault();
    errorEl.hidden = true;
    statusEl.hidden = false;
    loginBtn.disabled = true;

    const server = document.getElementById('server').value.trim();
    const username = document.getElementById('username').value.trim();
    const password = document.getElementById('password').value;

    if (!server || !username || !password) {
      showError('All fields are required.');
      loginBtn.disabled = false;
      return;
    }

    try {
      const WasmMatrixClient = getWasmClient();
      if (!WasmMatrixClient) {
        showError('WASM module not loaded yet. Please wait and try again.');
        loginBtn.disabled = false;
        return;
      }

      // Try restoring existing session first — preserves device_id and Megolm keys
      let client;
      let sessionJson;
      const savedSession = await loadSession();
      if (savedSession) {
        try {
          const parsed = JSON.parse(savedSession);
          showStatus(`Restoring session for ${parsed.user_id} (device: ${parsed.device_id})...`);
          console.log(`[auth] Restoring saved session — device: ${parsed.device_id}, store: ${parsed.store_name || 'legacy'}`);
          client = await WasmMatrixClient.restoreSession(savedSession);
          sessionJson = savedSession;
          console.log(`[auth] Session restored — reusing device ${parsed.device_id}`);
          showStatus(`Session restored (device: ${parsed.device_id})`);
        } catch (restoreErr) {
          console.warn('[auth] Session restore failed, falling back to fresh login:', restoreErr);
          showStatus('Saved session expired, logging in fresh...');
          client = null;
        }
      }

      if (!client) {
        showStatus(`Resolving homeserver for ${server}...`);
        console.log(`[auth] No saved session — fresh login to ${server} as ${username}`);
        console.warn('[auth] Creating new device ID — this should only happen on first login');

        showStatus(`Authenticating with ${server}...`);
        client = await WasmMatrixClient.login(server, username, password);

        const deviceId = client.deviceId();
        const userId = client.userId();
        console.warn(`[auth] New device created: ${deviceId} for ${userId}`);
        showStatus(`Logged in as ${userId} (new device: ${deviceId})`);

        showStatus('Setting up encryption (cross-signing)...');
        console.log('[auth] Bootstrapping cross-signing keys...');
        try {
          await Promise.race([
            (async () => {
              await client.bootstrapCrossSigningIfNeeded(password);
              console.log('[auth] Cross-signing keys bootstrapped');
              showStatus('Verifying device identity...');
              await client.verifyOwnIdentity();
              console.log('[auth] Own identity verified');
            })(),
            new Promise((_, reject) => setTimeout(() => reject(new Error('timeout')), 10000)),
          ]);
          showStatus('Encryption ready');
        } catch (csErr) {
          console.warn('[auth] Cross-signing skipped (non-fatal):', csErr);
          showStatus('Encryption setup skipped (non-fatal)');
        }

        sessionJson = client.exportSession();
        console.log('[auth] Session exported for persistence');
      }

      showStatus('Saving session to browser...');

      statusEl.hidden = true;
      form.reset();
      document.getElementById('server').value = 'matrix.org';
      onLogin(client, sessionJson);
    } catch (err) {
      console.error('[auth] Login error:', err);
      let msg;
      if (err instanceof Error) {
        msg = err.message;
      } else if (typeof err === 'string') {
        msg = err;
      } else if (err && typeof err.message === 'string') {
        msg = err.message;
      } else {
        msg = String(err) || 'Unknown error';
      }
      showError(`Login failed: ${msg}`);
    } finally {
      loginBtn.disabled = false;
    }
  });
}
