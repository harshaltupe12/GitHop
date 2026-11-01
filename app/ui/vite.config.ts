/// <reference types="vitest/config" />
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// Tauri expects a fixed dev port and relative asset paths in the bundle.
export default defineConfig({
  plugins: [react()],
  base: './',
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: '127.0.0.1',
    watch: { ignored: ['**/src-tauri/**'] },
  },
  envPrefix: ['VITE_', 'TAURI_ENV_'],
  build: {
    outDir: 'dist',
    target: 'es2021',
    sourcemap: false,
    // Fonts must ship as files (CSP font-src 'self'), never inlined from a CDN.
    assetsInlineLimit: 0,
  },
  test: {
    environment: 'jsdom',
    globals: false,
    setupFiles: ['./src/test/setup.ts'],
    css: false,
    include: ['src/**/*.test.{ts,tsx}'],
  },
});
