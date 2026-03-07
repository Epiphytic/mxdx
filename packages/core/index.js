// Polyfill IndexedDB for Node.js — matrix-sdk's crypto store requires it for E2EE key exchange
import 'fake-indexeddb/auto';

export * from './wasm/mxdx_core_wasm.js';
