import { defineConfig } from 'vite';

export default defineConfig({
  root: '.',
  build: {
    outDir: 'dist',
    target: 'esnext',
    rollupOptions: {
      input: 'index.html',
      // p2p-verify.js and batched-sender.js import node:crypto and node:zlib
      // for the npm launcher path. Web-console uses WASM crypto instead —
      // these imports are dead code in the browser bundle but Rollup fails
      // without explicit externalization.
      external: ['node:crypto', 'node:zlib'],
    },
  },
  optimizeDeps: {
    exclude: ['../../core/wasm/web/mxdx_core_wasm.js'],
  },
  server: {
    port: 5173,
    hmr: false, // Disable HMR — it kills active P2P/WebRTC sessions
  },
});
