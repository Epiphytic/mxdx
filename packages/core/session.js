import { WasmMatrixClient } from './wasm/nodejs/mxdx_core_wasm.js';
import { CredentialStore } from './credentials.js';
import { saveIndexedDB, restoreIndexedDB } from './persistent-indexeddb.js';

/**
 * Connect to Matrix with full session lifecycle:
 *   1. Restore IndexedDB crypto store from disk (Node.js only)
 *   2. Try restoring saved session from keyring
 *   3. Fresh login if no session (password from args → keyring → TTY prompt)
 *   4. Bootstrap cross-signing + verify own identity
 *   5. Store session + password in keyring
 *   6. Save IndexedDB crypto store to disk
 *
 * @param {Object} options
 * @param {string} options.username - Matrix username (localpart)
 * @param {string} options.server - Homeserver URL or hostname
 * @param {string} [options.password] - Password (optional if stored in keyring)
 * @param {string} [options.registrationToken] - Auto-register with this token
 * @param {string} [options.configDir] - Config directory for file-based fallback
 * @param {boolean} [options.useKeychain=true] - Use OS keychain via keytar
 * @param {Function} [options.log] - Log function (default: console.log)
 * @returns {Promise<{client: WasmMatrixClient, credentialStore: CredentialStore}>}
 */
export async function connectWithSession({
  username,
  server,
  password,
  registrationToken,
  configDir,
  useKeychain = true,
  log = console.log,
} = {}) {
  const credentialStore = new CredentialStore({ configDir, useKeychain });
  let client = null;
  let freshLogin = false;

  // ── 1. Restore IndexedDB crypto store from disk ─────────────────
  log('Loading crypto store from disk...');
  const restored = await restoreIndexedDB(configDir);
  if (restored) {
    log('Crypto store restored from disk');
  } else {
    log('No crypto store found on disk (first run or cleared)');
  }

  // ── 2. Try restoring an existing session ────────────────────────
  const savedSession = await credentialStore.loadSession(username, server);
  if (savedSession) {
    try {
      log(`Restoring session for ${username}@${server} (device: ${savedSession.device_id})...`);
      client = await WasmMatrixClient.restoreSession(
        JSON.stringify(savedSession),
      );
      log(`Session restored — reusing device ${client.deviceId()} for ${client.userId()}`);
    } catch (err) {
      log(`Session restore failed (${err}), will login fresh`);
      client = null;
    }
  } else {
    log(`No saved session found for ${username}@${server}`);
  }

  // ── 3. Fresh login if no session restored ───────────────────────
  if (!client) {
    freshLogin = true;

    // Password chain: provided arg → keyring → interactive prompt
    if (!password) {
      log('Loading password from keyring...');
      password = await credentialStore.loadPassword(username, server);
      if (password) log('Password loaded from keyring');
    }

    if (!password) {
      password = await promptPassword();
    }

    if (!password) {
      throw new Error(
        'Password required. Use --password, store in keyring, or run interactively.',
      );
    }

    log(`WARNING: Creating new device — this should only happen on first login or after credential reset`);
    log(`Authenticating with ${server} as ${username}...`);

    if (registrationToken) {
      log('Registering new account with token...');
      client = await WasmMatrixClient.register(
        server, username, password, registrationToken,
      );
    } else {
      client = await WasmMatrixClient.login(server, username, password);
    }

    log(`Logged in as ${client.userId()} (new device: ${client.deviceId()})`);

    // ── 4. Bootstrap cross-signing ──────────────────────────────
    try {
      log('Bootstrapping cross-signing keys...');
      await client.bootstrapCrossSigningIfNeeded(password);
      log('Cross-signing keys bootstrapped');
      log('Verifying own device identity...');
      await client.verifyOwnIdentity();
      log('Device identity verified — encryption ready');
    } catch (err) {
      log(`Cross-signing bootstrap skipped (non-fatal): ${err}`);
    }

    // ── 5. Store credentials in keyring ─────────────────────────
    log('Saving credentials to keyring...');
    await credentialStore.savePassword(username, server, password);

    const sessionData = client.exportSession();
    await credentialStore.saveSession(
      username, server, JSON.parse(sessionData),
    );
    log(`Credentials stored — device ${client.deviceId()} persisted for future sessions`);
  }

  // ── 6. Save IndexedDB crypto store to disk ──────────────────────
  try {
    log('Saving crypto store to disk...');
    await saveIndexedDB(configDir);
    log('Crypto store saved to disk');
  } catch (err) {
    log(`Crypto store save failed (non-fatal): ${err}`);
  }

  return { client, credentialStore, freshLogin, password };
}

async function promptPassword() {
  if (!process.stdin.isTTY) return null;

  const { createInterface } = await import('node:readline/promises');
  const rl = createInterface({ input: process.stdin, output: process.stderr });
  try {
    const password = await rl.question('Password: ');
    return password || null;
  } finally {
    rl.close();
  }
}
