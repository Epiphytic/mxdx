// Persistent IndexedDB polyfill for Node.js — must load before WASM.
// In browser environments this module is not used (browser has real IndexedDB).
export { saveIndexedDB, restoreIndexedDB } from './persistent-indexeddb.js';

export * from './wasm/nodejs/mxdx_core_wasm.js';

// Coerce WASM onRoomEvent 'null' string → JS null at the JS/WASM boundary.
// The WASM layer serializes None as the string 'null'; callers expect JS null.
import { WasmMatrixClient } from './wasm/nodejs/mxdx_core_wasm.js';
const _origOnRoomEvent = WasmMatrixClient.prototype.onRoomEvent;
WasmMatrixClient.prototype.onRoomEvent = async function (...args) {
  const result = await _origOnRoomEvent.apply(this, args);
  return result === 'null' ? null : result;
};

export { CredentialStore } from './credentials.js';
export { connectWithSession } from './session.js';
export { TerminalSocket } from './terminal-socket.js';
export { BatchedSender } from './batched-sender.js';
export * from './terminal-types.js';
export { parseOlderThan, cleanupDevices, cleanupRooms, cleanupEvents, logoutAll } from './cleanup.js';
export { fetchTurnCredentials, turnToIceServers } from './turn-credentials.js';
export { NodeWebRTCChannel } from './webrtc-channel-node.js';
export { P2PSignaling } from './p2p-signaling.js';
export { P2PTransport } from './p2p-transport.js';
export { P2PCrypto, generateSessionKey, createP2PCrypto } from './p2p-crypto.js';
export { MultiHsClient } from './multi-hs-client.js';
export {
  createSessionTask,
  parseSessionResult,
  parseActiveSession,
  parseCompletedSession,
  parseWorkerInfo,
  getSessionEventTypes,
} from './src/session-client.js';
