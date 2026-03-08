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

      showStatus(`Connecting to ${server}...`);
      const client = await WasmMatrixClient.login(server, username, password);

      showStatus('Setting up encryption...');
      try {
        await Promise.race([
          (async () => {
            await client.bootstrapCrossSigningIfNeeded(password);
            await client.verifyOwnIdentity();
          })(),
          new Promise((_, reject) => setTimeout(() => reject(new Error('timeout')), 10000)),
        ]);
      } catch (csErr) {
        console.warn('[auth] Cross-signing skipped (non-fatal):', csErr);
      }

      showStatus('Saving session...');
      const sessionJson = client.exportSession();

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
