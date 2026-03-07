// Polyfill IndexedDB for Node.js — matrix-sdk's crypto store requires it for E2EE key exchange
import 'fake-indexeddb/auto';

export * from './wasm/mxdx_core_wasm.js';
export { CredentialStore } from './credentials.js';
export { connectWithSession } from './session.js';
export { TerminalSocket } from './terminal-socket.js';
export * from './terminal-types.js';
