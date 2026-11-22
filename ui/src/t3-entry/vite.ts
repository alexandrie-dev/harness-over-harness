// Сборка входа T3 (HOH-40): `vite build --mode t3` собирает веб-клиент T3 (src/t3/base, u2) с хостом HoH
// (src/hoh-host, u3) в ту же папку dist, что и прежняя оболочка, но своими псевдонимами и в dist/t3-assets.
// Страница T3 становится dist/index.html (вход окна по умолчанию); прежняя оболочка остаётся как dist/legacy.html
// (её копию пишет `legacyCopy` в основной сборке; окно открывает её при HOH_WINDOW_SHELL=legacy).
// Файлы T3 не правятся: скрытые страницы и пункты настроек выключаются здесь, при сборке (features.ts).
import { existsSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import type { Plugin, UserConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';
import { HIDDEN_ROUTES, VISIBLE_SETTINGS } from './features';
import { HOH_HTTP_URL } from './host';

const UI = resolve(import.meta.dirname, '../..');
export const T3_BASE = resolve(UI, 'src/t3/base');
const T3_WEB = resolve(T3_BASE, 'web');
const HOST = resolve(UI, 'src/hoh-host');
const HOST_STUB = resolve(import.meta.dirname, 'host-stub.ts');
const HOST_MODULE = '\0hoh-host';
export const T3_PAGE = 't3.html';
export const T3_ASSETS = 't3-assets';

/** Псевдонимы основы T3: их пишет scripts/t3-sync.py (u2) в src/t3/base/aliases.json, пути — от ui/. */
function t3Aliases() {
  const table: Record<string, string> = JSON.parse(readFileSync(resolve(T3_BASE, 'aliases.json'), 'utf8'));
  return Object.entries(table).map(([name, target]) => name === '~'
    ? { find: /^~\//, replacement: resolve(UI, target) + '/' }
    : { find: new RegExp('^' + name.replace(/[.*+?^${}()|[\]\\/]/g, '\\$&') + '$'), replacement: resolve(UI, target) });
}

/** id маршрута TanStack из имени файла маршрута: `_chat.settings.voice` → `/_chat/settings/voice`. */
export function routeId(file: string): string {
  return '/' + file.split('.').join('/');
}

/** Маршрут вместо скрытой страницы: тот же id, сразу уводит; код страницы в сборку не попадает. */
export function hiddenRouteModule(file: string, to: string): string {
  return `import { createFileRoute, redirect } from "@tanstack/react-router";\n`
    + `export const Route = createFileRoute(${JSON.stringify(routeId(file))})({\n`
    + `  beforeLoad: () => { throw redirect({ to: ${JSON.stringify(to)}, replace: true }); },\n});\n`;
}

const NAV_ANCHOR = 'export const SETTINGS_NAV_GROUPS';
/** Оставляет в списке разделов настроек только VISIBLE_SETTINGS (пустые группы убираются). */
export function settingsNavFilter(code: string): string {
  if (!code.includes(NAV_ANCHOR)) throw new Error('settingsSearch.ts: нет SETTINGS_NAV_GROUPS — T3 изменился, проверьте features.ts');
  return code + `\n;{ // HoH (src/t3-entry/vite.ts): видимые разделы настроек — features.ts\n`
    + `  const visible = new Set(${JSON.stringify(VISIBLE_SETTINGS)});\n`
    + `  const groups = SETTINGS_NAV_GROUPS;\n`
    + `  for (let i = groups.length - 1; i >= 0; i--) {\n`
    + `    groups[i].paths = groups[i].paths.filter((path) => visible.has(path));\n`
    + `    if (groups[i].paths.length === 0) groups.splice(i, 1);\n`
    + `  }\n}\n`;
}

const ROUTE_PREFIX = '\0hoh-hidden-route:';
const VIRTUAL_HOST = 'virtual:hoh-host';
const VIRTUAL_BOOTSTRAP = 'virtual:t3-bootstrap';

function hohT3(): Plugin {
  return {
    name: 'hoh-t3-entry',
    enforce: 'pre',
    resolveId(source, importer) {
      // Хост HoH (u3): ui/src/hoh-host/{index,tauri}.ts; пока его нет — заглушка с теми же именами.
      if (source === VIRTUAL_HOST) return existsSync(resolve(HOST, 'index.ts')) ? HOST_MODULE : HOST_STUB;
      // Загрузчик T3 как есть: apps/web/src/bootstrap.ts (импортирует их main.tsx и показывает ошибку запуска).
      if (source === VIRTUAL_BOOTSTRAP) return resolve(T3_WEB, 'bootstrap.ts');
      if (!importer?.startsWith(T3_WEB)) return null;
      const route = /(?:^|\/)routes\/([^/]+?)(?:\.tsx)?$/.exec(source)?.[1];
      return route && route in HIDDEN_ROUTES ? ROUTE_PREFIX + route : null;
    },
    load(id) {
      if (id === HOST_MODULE) {
        return `export { installHohHost } from ${JSON.stringify(resolve(HOST, 'index.ts'))};\n`
          + `export { tauriEngine } from ${JSON.stringify(resolve(HOST, 'tauri.ts'))};\n`;
      }
      if (!id.startsWith(ROUTE_PREFIX)) return null;
      const file = id.slice(ROUTE_PREFIX.length);
      return hiddenRouteModule(file, HIDDEN_ROUTES[file]!);
    },
    transform(code, id) {
      return id === resolve(T3_WEB, 'components/settings/settingsSearch.ts') ? { code: settingsNavFilter(code), map: null } : null;
    },
  };
}

/** Страница T3 — вход окна: dist/t3.html → dist/index.html (та же папка, относительные адреса не меняются). */
function t3Page(): Plugin {
  return {
    name: 'hoh-t3-page',
    enforce: 'post',
    generateBundle(_options, bundle) {
      const page = bundle[T3_PAGE];
      if (!page || page.type !== 'asset') return this.error(`нет ${T3_PAGE} в сборке`);
      delete bundle[T3_PAGE];
      this.emitFile({ type: 'asset', fileName: 'index.html', source: page.source });
    },
  };
}

/** Основная сборка (прежняя оболочка): копия её страницы как legacy.html — её не перезапишет сборка T3. */
export function legacyCopy(): Plugin {
  return {
    name: 'hoh-legacy-copy',
    enforce: 'post',
    generateBundle(_options, bundle) {
      const page = bundle['index.html'];
      if (page?.type === 'asset') this.emitFile({ type: 'asset', fileName: 'legacy.html', source: page.source });
    },
  };
}

// Переменные сборки T3 (их vite.config.ts): облако, ретрансляция, телеметрия и обновления выключены пустыми значениями.
const T3_ENV = [
  'VITE_WS_URL', 'VITE_T3CODE_RELAY_URL', 'VITE_CLERK_PUBLISHABLE_KEY', 'VITE_CLERK_JWT_TEMPLATE',
  'VITE_CLERK_CLI_OAUTH_CLIENT_ID', 'VITE_RELAY_OTLP_TRACES_URL', 'VITE_RELAY_OTLP_TRACES_DATASET',
  'VITE_RELAY_OTLP_TRACES_TOKEN', 'VITE_HOSTED_APP_URL', 'VITE_HOSTED_APP_CHANNEL', 'VITE_RELEASE_CHANNEL',
  'VITE_DEV_SERVER_URL', 'T3CODE_DESKTOP_UPDATE_REPOSITORY', 'T3CODE_FEATURE_SECRETS',
];
/** Версия программы в окне T3 (их APP_VERSION) — версия окна HoH. */
const APP_VERSION = JSON.parse(readFileSync(resolve(UI, 'package.json'), 'utf8')).version as string;

/** Скрытые маршруты сверяются с файлами T3: тот же id в createFileRoute, иначе сборка останавливается. */
export function checkHiddenRoutes(web: string): void {
  for (const [file, to] of Object.entries(HIDDEN_ROUTES)) {
    const path = resolve(web, 'routes', file + '.tsx');
    if (!existsSync(path)) throw new Error(`features.ts: нет маршрута T3 ${file}.tsx`);
    if (!readFileSync(path, 'utf8').includes(`createFileRoute(${JSON.stringify(routeId(file))})`)) {
      throw new Error(`features.ts: id маршрута ${file} в T3 не ${routeId(file)}`);
    }
    if (Object.keys(HIDDEN_ROUTES).some((hidden) => routeId(hidden).replace('/_chat', '') === to)) {
      throw new Error(`features.ts: ${file} уводит на скрытую страницу ${to}`);
    }
  }
}

export function t3Config(): UserConfig {
  if (!existsSync(resolve(T3_WEB, 'bootstrap.ts'))) {
    throw new Error('нет основы T3 (src/t3/base/web/bootstrap.ts): сначала перенос u2 (scripts/t3-sync.py)');
  }
  checkHiddenRoutes(T3_WEB);
  return {
    base: './',
    root: UI,
    // boot-theme.js — их встроенный сценарий темы до запуска, вынесенный в файл (CSP окна: script-src 'self').
    publicDir: resolve(import.meta.dirname, 'public'),
    plugins: [hohT3(), react(), tailwindcss(), t3Page()],
    clearScreen: false,
    resolve: { alias: t3Aliases() },
    define: {
      ...Object.fromEntries(T3_ENV.map((name) => [`import.meta.env.${name}`, '""'])),
      // T3 ходит на этот адрес (hoh-host/README.md): сокет и fetch отвечает хост в странице, сети нет.
      'import.meta.env.VITE_HTTP_URL': JSON.stringify(HOH_HTTP_URL),
      'import.meta.env.APP_VERSION': JSON.stringify(APP_VERSION),
    },
    assetsInclude: ['**/*.wasm'],
    worker: { format: 'es' },
    build: {
      target: 'es2022',
      sourcemap: false,
      assetsInlineLimit: 0,
      outDir: resolve(UI, 'dist'),
      emptyOutDir: false,
      assetsDir: T3_ASSETS,
      rolldownOptions: { input: { t3: resolve(UI, T3_PAGE) } },
    },
  };
}
