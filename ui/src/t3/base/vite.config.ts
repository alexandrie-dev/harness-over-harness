// Сборка основы HOH-40 как отдельного приложения: вход — их web/bootstrap.ts (→ main.tsx → AppRoot), как в
// apps/web/index.html T3. Нужна для проверки «основа собирается» и снимков пустой оболочки; в окно основу подключает
// вход u4 (src/t3-entry/vite.ts), со своими настройками и хостом u3. Запуск из ui/:
//   node_modules/.bin/vite build -c src/t3/base/vite.config.ts   (папку сборки задаёт T3_BASE_OUT)
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';

const BASE = import.meta.dirname;
const UI = resolve(BASE, '../../..');

/** Псевдонимы основы: их пишет scripts/t3-sync.py в aliases.json (пути от ui/). */
const aliases = Object.entries(JSON.parse(readFileSync(resolve(BASE, 'aliases.json'), 'utf8')) as Record<string, string>)
  .map(([name, target]) => name === '~'
    ? { find: /^~\//, replacement: resolve(UI, target) + '/' }
    : { find: new RegExp('^' + name.replace(/[.*+?^${}()|[\]\\/]/g, '\\$&') + '$'), replacement: resolve(UI, target) });

// Переменные сборки T3 (их vite.config.ts): облачный вход, ретрансляция, телеметрия, секреты — пусто, части выключены.
const EMPTY_ENV = [
  'VITE_WS_URL', 'VITE_HTTP_URL', 'VITE_T3CODE_RELAY_URL', 'VITE_CLERK_PUBLISHABLE_KEY', 'VITE_CLERK_JWT_TEMPLATE',
  'VITE_CLERK_CLI_OAUTH_CLIENT_ID', 'VITE_RELAY_OTLP_TRACES_URL', 'VITE_RELAY_OTLP_TRACES_DATASET', 'VITE_RELAY_OTLP_TRACES_TOKEN',
  'VITE_HOSTED_APP_URL', 'VITE_HOSTED_APP_CHANNEL', 'VITE_RELEASE_CHANNEL', 'VITE_DEV_SERVER_URL',
  'T3CODE_DESKTOP_UPDATE_REPOSITORY', 'T3CODE_FEATURE_SECRETS',
];

export default defineConfig({
  root: BASE,
  base: './',
  // React Compiler, как у T3 (`react({ compiler: true })`): код T3 рассчитывает на его мемоизацию.
  plugins: [react({ compiler: true }), tailwindcss()],
  clearScreen: false,
  publicDir: false,
  resolve: { alias: aliases },
  define: {
    ...Object.fromEntries(EMPTY_ENV.map((name) => [`import.meta.env.${name}`, '""'])),
    'import.meta.env.APP_VERSION': JSON.stringify(JSON.parse(readFileSync(resolve(UI, 'package.json'), 'utf8')).version),
  },
  assetsInclude: ['**/*.wasm'],
  worker: { format: 'es' },
  build: {
    target: 'es2022',
    sourcemap: false,
    assetsInlineLimit: 0,
    outDir: process.env.T3_BASE_OUT ?? '/mnt/toshiba/alexandrie/hoh/target-u2/t3-base-dist',
    emptyOutDir: true,
    // T3_BASE_QA=1 — ещё и qa.html: T3 на хосте u3 с пустым движком (снимки пустой оболочки, scripts/t3-snapshot.mjs).
    rolldownOptions: { input: { index: resolve(BASE, 'index.html'), ...(process.env.T3_BASE_QA ? { qa: resolve(BASE, 'qa.html') } : {}) } },
  },
});
