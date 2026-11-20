import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';
import { resolve } from 'node:path';
import { localDiagnostics } from './scripts/local-diagnostics';
import t3Aliases from './src/t3/aliases.json';

// Псевдонимы перенесённого кода T3 (ui/src/t3, MIT): их пишет scripts/t3-sync.py вместе с tsconfig.paths.json.
const t3Alias = Object.entries(t3Aliases).map(([name, target]) => name === '~'
  ? { find: /^~\//, replacement: resolve(import.meta.dirname, target) + '/' }
  : { find: new RegExp('^' + name.replace(/[.*+?^${}()|[\]\\/]/g, '\\$&') + '$'), replacement: resolve(import.meta.dirname, target) });

export default defineConfig(({ mode }) => ({
  base: './',
  plugins: [localDiagnostics(), react(), tailwindcss()],
  clearScreen: false,
  resolve: { alias: t3Alias },
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
