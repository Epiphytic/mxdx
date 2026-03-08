import { WasmMatrixClient } from './wasm/mxdx_core_wasm.js';
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
  const restored = await restoreIndexedDB(configDir);
  if (restored) {
    log('Crypto store restored from disk');
  }

  // ── 2. Try restoring an existing session ────────────────────────
  const savedSession = await credentialStore.loadSession(username, server);
  if (savedSession) {
    try {
      log(`Restoring session for ${username}@${server}...`);
      client = await WasmMatrixClient.restoreSession(
        JSON.stringify(savedSession),
      );
      log(`Session restored as ${client.userId()} (device: ${client.deviceId()})`);
    } catch (err) {
      log(`Session restore failed (${err}), will login fresh`);
      client = null;
    }
  }

  // ── 3. Fresh login if no session restored ───────────────────────
  if (!client) {
    freshLogin = true;

    // Password chain: provided arg → keyring → interactive prompt
    if (!password) {
      password = await credentialStore.loadPassword(username, server);
    }

    if (!password) {
      password = await promptPassword();
    }

    if (!password) {
      throw new Error(
        'Password required. Use --password, store in keyring, or run interactively.',
      );
    }

    log(`Connecting to ${server}...`);

    if (registrationToken) {
      client = await WasmMatrixClient.register(
        server, username, password, registrationToken,
      );
    } else {
      client = await WasmMatrixClient.login(server, username, password);
    }

    log(`Logged in as ${client.userId()} (device: ${client.deviceId()})`);

    // ── 4. Bootstrap cross-signing ──────────────────────────────
    try {
      log('Bootstrapping cross-signing...');
      await client.bootstrapCrossSigningIfNeeded(password);
      await client.verifyOwnIdentity();
      log('Cross-signing ready');
    } catch (err) {
      log(`Cross-signing bootstrap failed (non-fatal): ${err}`);
    }

    // ── 5. Store credentials in keyring ─────────────────────────
    await credentialStore.savePassword(username, server, password);

    const sessionData = client.exportSession();
    await credentialStore.saveSession(
      username, server, JSON.parse(sessionData),
    );
    log('Credentials stored in keyring');
  }

  // ── 6. Save IndexedDB crypto store to disk ──────────────────────
  try {
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
