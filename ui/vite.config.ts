import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';
import { resolve } from 'node:path';
import { localDiagnostics } from './scripts/local-diagnostics';

export default defineConfig(({ mode }) => ({
  base: './',
  plugins: [localDiagnostics(), react(), tailwindcss()],
  clearScreen: false,
  build: {
    target: 'es2022',
    sourcemap: false,
    assetsInlineLimit: 0,
    rolldownOptions: {
      input: {
        main: resolve(import.meta.dirname, 'index.html'),
        ...(mode === 'probe' ? { probe: resolve(import.meta.dirname, 'probe.html') } : {}),
        approval: resolve(import.meta.dirname, 'approval.html'),
        firstrun: resolve(import.meta.dirname, 'firstrun.html'),
      },
    },
  },
}));
