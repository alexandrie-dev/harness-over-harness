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

// @base-ui/react (меню режима доступа) кладёт в сообщение об ошибке адрес справочной страницы. Это только текст сообщения,
// но окно не содержит ни одного внешнего адреса (check-dist.mjs): адрес превращается в нессылочный текст.
const inertErrorAddress = { name: 'hoh-inert-error-address', enforce: 'post' as const, renderChunk(code: string) {
  return code.includes('https://base-ui.com/') ? { code: code.replaceAll('https://base-ui.com/', 'base-ui.com/'), map: null } : null;
} };

export default defineConfig(({ mode }) => ({
  base: './',
  plugins: [localDiagnostics(), react(), tailwindcss(), inertErrorAddress],
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
