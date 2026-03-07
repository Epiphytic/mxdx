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
    exclude: ['./wasm/mxdx_core_wasm.js'],
  },
  server: {
    port: 5173,
  },
});
