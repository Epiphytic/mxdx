import { defineConfig } from 'vite';

export default defineConfig({
  root: '.',
  build: {
    outDir: 'dist',
    target: 'esnext',
    rollupOptions: {
      input: 'index.html',
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
