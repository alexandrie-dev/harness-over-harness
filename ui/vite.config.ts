import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';
import { resolve } from 'node:path';
import { localDiagnostics } from './scripts/local-diagnostics';
import t3Aliases from './src/t3/aliases.json';
import { legacyCopy, t3Config } from './src/t3-entry/vite';

// Псевдонимы перенесённого кода T3 (ui/src/t3, MIT): их пишет scripts/t3-sync.py вместе с tsconfig.paths.json.
const t3Alias = Object.entries(t3Aliases).map(([name, target]) => name === '~'
  ? { find: /^~\//, replacement: resolve(import.meta.dirname, target) + '/' }
  : { find: new RegExp('^' + name.replace(/[.*+?^${}()|[\]\\/]/g, '\\$&') + '$'), replacement: resolve(import.meta.dirname, target) });

// @base-ui/react (меню режима доступа) кладёт в сообщение об ошибке адрес справочной страницы. Это только текст сообщения,
// но окно не содержит ни одного внешнего адреса (check-dist.mjs): адрес превращается в нессылочный текст.
const inertErrorAddress = { name: 'hoh-inert-error-address', enforce: 'post' as const, renderChunk(code: string) {
  return code.includes('https://base-ui.com/') ? { code: code.replaceAll('https://base-ui.com/', 'base-ui.com/'), map: null } : null;
} };

// `--mode t3` — вход окна на веб-клиенте T3 (HOH-40, src/t3-entry/vite.ts): вторая сборка в ту же dist после этой.
export default defineConfig(({ mode }) => mode === 't3' ? t3Config() : ({
  base: './',
  plugins: [localDiagnostics(), react(), tailwindcss(), inertErrorAddress, legacyCopy()],
  clearScreen: false,
  resolve: { alias: t3Alias },
  build: {
    target: 'es2022',
    sourcemap: false,
    assetsInlineLimit: 0,
    rolldownOptions: {
      input: {
        main: resolve(import.meta.dirname, 'index.html'),
        // Страницы проверок (probe — WebKitGTK, firstrun — tests/screens-a) — только в `--mode probe`; окну они не нужны,
        // в поставку не идут (check-dist.mjs, ревью 222000).
        ...(mode === 'probe' ? { probe: resolve(import.meta.dirname, 'probe.html'), firstrun: resolve(import.meta.dirname, 'firstrun.html') } : {}),
        // Окно подтверждения: своё окно `approval-*` (apps/hoh-desktop, approval_navigation_allowed).
        approval: resolve(import.meta.dirname, 'approval.html'),
      },
    },
  },
}));
