// Сборка входа T3 (HOH-40): `vite build --mode t3` собирает веб-клиент T3 (src/t3/base, u2) с хостом HoH
// (src/hoh-host, u3) в ту же папку dist, что и прежняя оболочка, но своими псевдонимами и в dist/t3-assets.
// Страница T3 становится dist/index.html (вход окна по умолчанию); прежняя оболочка остаётся как dist/legacy.html
// (её копию пишет `legacyCopy` в основной сборке; окно открывает её при HOH_WINDOW_SHELL=legacy).
// Файлы T3 не правятся: скрытые страницы и пункты настроек выключаются здесь, при сборке (features.ts).
import { existsSync, readFileSync } from 'node:fs';
import { basename, resolve } from 'node:path';
import type { Plugin, UserConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';
import { HOH_THEME_COLORS } from './hoh-theme';
import { HIDDEN_COMPONENTS, HIDDEN_PALETTE_ITEMS, HIDDEN_ROUTES, VISIBLE_PROVIDER_DRIVERS, VISIBLE_SETTINGS } from './features';
import { HOH_HTTP_URL } from './host';

const UI = resolve(import.meta.dirname, '../..');
export const T3_BASE = resolve(UI, 'src/t3/base');
const T3_WEB = resolve(T3_BASE, 'web');
const HOST = resolve(UI, 'src/hoh-host');
const HOST_STUB = resolve(import.meta.dirname, 'host-stub.ts');
const PROJECT_GROUPING_SETTING = resolve(UI, 'src/t3-entry/hoh-screens/ProjectGroupingSetting.tsx');
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

const PROVIDER_ANCHORS = [
  ['const visibleProviderSettings = PROVIDER_SETTINGS.filter(', 'const visibleProviderSettings = PROVIDER_SETTINGS.filter((entry) => HOH_PROVIDER_DRIVERS.includes(entry.provider)).filter('],
  ['const canAddProvider =\n', 'const canAddProvider = false &&\n'],
  // Настройки, которых движок не исполняет (путь к claude, свои аргументы, «Usage providers», проверка состояния, сжатие): не рисуются.
  ['<UsageProviderSettings\n', '<HohNone\n'],
  ['      <SettingsSection title={gt("Advanced")}>', '      {false ? <SettingsSection title={gt("Advanced")}>'],
  ['        <CompactIdleThreadsRow />\n      </SettingsSection>\n', '        <CompactIdleThreadsRow />\n      </SettingsSection> : null}\n'],
] as const;
const CARD_ANCHORS = [
  // Карточка поставщика: остаются название и состояние; «Runtime», «Environment» и «Models» движок не исполняет.
  ['!driverOption || deriveProviderSettingsFields(driverOption, undefined, m).length > 0 ? (', 'false ? ('],
  ['      <SettingsSection\n        title={gt("Environment")}', '      {false ? <SettingsSection\n        title={gt("Environment")}'],
  ['      </SettingsSection>\n      {environmentId !== undefined && liveProvider?.driver === "acpRegistry" ? (', '      </SettingsSection> : null}\n      {environmentId !== undefined && liveProvider?.driver === "acpRegistry" ? ('],
  ['{driverOption !== undefined ? (\n        <SettingsSection title={gt("Models")}>', '{false ? (\n        <SettingsSection title={gt("Models")}>'],
] as const;
function anchorPatch(file: string, code: string, edits: ReadonlyArray<readonly [string, string]>, header = ''): string {
  let out = code;
  for (const [from, to] of edits) {
    if (!out.includes(from)) throw new Error(`${file}: нет «${from.trim()}» — T3 изменился, проверьте features.ts`);
    out = out.replace(from, to);
  }
  return header + out;
}
/** Providers (ProviderSettingsPanel.tsx): только поставщики VISIBLE_PROVIDER_DRIVERS, без «Add provider» и «+», без настроек, которых движок не исполняет. */
export function providerSettingsPanel(code: string): string {
  return anchorPatch('ProviderSettingsPanel.tsx', code, PROVIDER_ANCHORS,
    `const HOH_PROVIDER_DRIVERS: readonly string[] = ${JSON.stringify(VISIBLE_PROVIDER_DRIVERS)}; // HoH (vite.ts, features.ts)\nconst HohNone = (_props: Record<string, unknown>) => null;\n`);
}
export function providerInstanceCard(code: string): string {
  return anchorPatch('ProviderInstanceCard.tsx', code, CARD_ANCHORS);
}

/** Имена тем в Appearance → Themes: без названий продуктов T3 («T3 Code» → «Default», «T3 Chat» → «Chat»). */
export function themeCards(code: string): string {
  return anchorPatch('ThemePreviewCircles.tsx', code, [['label: "T3 Code",', 'label: "Default",']]);
}
/** Карточка «HoH» первой среди тем в Appearance → Themes. */
export function themeSettings(code: string): string {
  return anchorPatch('ThemeSettings.tsx', code, [
    ['const MAINTAINER_THEMES: ReadonlyArray<ThemeDefinition> = [\n', 'const MAINTAINER_THEMES: ReadonlyArray<ThemeDefinition> = [\n  HOH_THEME,\n'],
    ['    <div className="space-y-3">\n', '    <div className="space-y-3">\n      <HohProjectGroupingSetting />\n'],
  ], 'import { HOH_THEME } from "@t3tools/shared/themePalettes";\nimport { ProjectGroupingSetting as HohProjectGroupingSetting } from ' + JSON.stringify(PROJECT_GROUPING_SETTING) + ';\n');
}
export function themePalettes(code: string): string {
  const theme = `export const HOH_THEME: ThemeDefinition = { id: "hoh", label: "HoH", appearance: "dark", colors: ${JSON.stringify(HOH_THEME_COLORS)} };\n`;
  return anchorPatch('themePalettes.ts', code, [
    ['label: "T3 Chat",', 'label: "Chat",'],
    ['export const BUILT_IN_THEME_IDS = ["t3-chat",', 'export const BUILT_IN_THEME_IDS = ["hoh", "t3-chat",'],
    ['export const BUILT_IN_THEMES: ReadonlyArray<ThemeDefinition> = [\n', theme + 'export const BUILT_IN_THEMES: ReadonlyArray<ThemeDefinition> = [\n  HOH_THEME,\n'],
  ]);
}

/**
 * Главная (HOH-41 В2б) — пустой разговор T3 целиком: `/` у T3 открывает черновик, в нём их поле ввода (чип модели с уровнями,
 * Permissions, выбор проекта, отправка). Над полем подставлено содержимое главной (hoh-screens/HomeBody.tsx):
 * ChatView.tsx — колонка вместо центрирования в «пустом» состоянии и тело главной над заголовком черновика.
 */
const HOME_BODY = resolve(UI, 'src/t3-entry/hoh-screens/HomeBody.tsx');
const SIDEBAR_TOP = resolve(UI, 'src/t3-entry/hoh-screens/SidebarTop.tsx');
export function chatViewHome(code: string): string {
  return anchorPatch('ChatView.tsx', code, [
    ['"pointer-events-none absolute inset-0 z-20 flex items-center [@media(max-height:34rem)]:items-end"', '"hoh-draft-hero pointer-events-none absolute inset-0 z-20 flex flex-col justify-end"'],
    ['              <div ref={draftHeroTransitionGroupRef} className="chat-composer-lane w-full">', '              {isDraftHeroState ? <HohHomeBody /> : null}\n              <div ref={draftHeroTransitionGroupRef} className="chat-composer-lane w-full">'],
    ['    if (input.runtimeMode !== serverThread.runtimeMode) {', '    if (true) { // HoH: persist the composer choice even when the running projection is stale.'],
    ['                      <ComposerSurface.Shell contextRow={showComposerContextStrip}>', `                      <ComposerSurface.Shell contextRow={showComposerContextStrip}>
                        {isWorking && activeThread?.runtimeMode ? <div className="px-3 pt-1 text-xs text-muted-foreground" role="status">Active permissions: {({"approval-required":"Confirm", "auto-accept-edits":"Accept edits", "auto":"Auto", "full-access":"Full"} as Record<string, string>)[activeThread.runtimeMode] ?? activeThread.runtimeMode}{runtimeMode !== activeThread.runtimeMode ? <> · Next: {({"approval-required":"Confirm", "auto-accept-edits":"Accept edits", "auto":"Auto", "full-access":"Full"} as Record<string, string>)[runtimeMode] ?? runtimeMode}</> : null}</div> : null}`],
  ], `import { HomeBody as HohHomeBody } from ${JSON.stringify(HOME_BODY)};\n`);
}
/** Боковая панель (HOH-41, состав раунда 12): верх — SidebarTop (New chat, Team с «+», Recent chats) вместо шапки T3 с поиском и значками;
 *  все разговоры — строки в один ряд без строки проекта («slim»), без полки Working. Полки Snoozed/Settled и кнопки Settle/Snooze скрыты (features.ts). */
export function sidebarList(code: string): string {
  return anchorPatch('Sidebar.tsx', code, [
    ['import { SidebarHeaderIconButton, SidebarThreadHeader } from "./sidebar/SidebarThreadHeader";', 'import { SidebarHeaderIconButton } from "./sidebar/SidebarThreadHeader";\nimport { SidebarTop as SidebarThreadHeader } from ' + JSON.stringify(SIDEBAR_TOP) + ';'],
    ['useClientSettings((s) => s.sidebarWorkingShelfEnabled) && folderGroups === null', '(useClientSettings((s) => s.sidebarWorkingShelfEnabled), false)'],
    ['section === "active" || section === "pinned" || section === "working";', 'false;'],
    // Пустой черновик (разговор без первого сообщения: хост даёт ему latestUserMessageAt = null) в «Recent chats» не показывается.
    ['  const threads = useQuietThreadShells();', '  const allThreads = useQuietThreadShells();\n  const threads = useHohMemo(() => allThreads.filter((thread) => thread.latestUserMessageAt != null), [allThreads]);'],
  ], 'import { useMemo as useHohMemo } from "react";\n');
}
/** «Settings» внизу панели — со значком и подписью (как в раунде 12), а не только значком. */
export function sidebarChrome(code: string): string {
  return anchorPatch('SidebarChrome.tsx', code, [
    ['<SidebarMenuButton aria-label={label} onClick={onClick} size="icon">\n              {icon}\n            </SidebarMenuButton>', '<SidebarMenuButton aria-label={label} onClick={onClick}>\n              {icon}\n              <span>{label}</span>\n            </SidebarMenuButton>'],
  ]);
}
/** Главная без проектов — «What should we work on?» T3 (эмблема, «Add project», «Start without a project»), заголовок — приветствие с датой. */
export function noProjectsHero(code: string): string {
  return anchorPatch('NoProjectsHero.tsx', code, [
    ['<T>What should we work on?</T>', '<HohGreeting />'],
  ], `import { Greeting as HohGreeting } from ${JSON.stringify(HOME_BODY)};\n`);
}

/** Модуль вместо модуля компонента T3: всё как у него, кроме названных компонентов — они ничего не рисуют. */
export function hiddenComponentModule(original: string, names: readonly string[]): string {
  return `export * from ${JSON.stringify(original)};\n`
    + names.map((name) => name === 'default'
      ? 'export default function HohHiddenComponent() { return null; }\n'
      : `export function ${name}() { return null; }\n`).join('');
}

/** Модуль вместо логики палитры команд T3: всё как у неё, только группы, которые палитра показывает
 * (filterCommandPaletteGroups), отдаются без пунктов HIDDEN_PALETTE_ITEMS и без опустевших групп. */
export function paletteFilterModule(original: string, patterns: readonly string[]): string {
  return `export * from ${JSON.stringify(original)};\n`
    + `import { filterCommandPaletteGroups as t3FilterCommandPaletteGroups } from ${JSON.stringify(original)};\n`
    + `const hidden = [${patterns.map((pattern) => `new RegExp(${JSON.stringify(pattern)})`).join(', ')}];\n`
    + `export function filterCommandPaletteGroups(...args) {\n`
    + `  return t3FilterCommandPaletteGroups(...args)\n`
    + `    .map((group) => ({ ...group, items: group.items.filter((item) => !hidden.some((pattern) => pattern.test(item.value))) }))\n`
    + `    .filter((group) => group.items.length > 0);\n}\n`;
}

const PALETTE_LOGIC = 'components/CommandPalette.logic.ts';

/** Имена файлов скрытых компонентов: по ним импорт сверяется, прежде чем его разрешать полностью. */
const HIDDEN_COMPONENT_NAMES = new Set(Object.keys(HIDDEN_COMPONENTS).map((file) => basename(file, '.tsx')));

const ROUTE_PREFIX = '\0hoh-hidden-route:';
const COMPONENT_PREFIX = '\0hoh-hidden-component:';
const PALETTE_MODULE = '\0hoh-palette-filter';
const VIRTUAL_HOST = 'virtual:hoh-host';
const VIRTUAL_BOOTSTRAP = 'virtual:t3-bootstrap';

function hohT3(): Plugin {
  return {
    name: 'hoh-t3-entry',
    enforce: 'pre',
    async resolveId(source, importer) {
      // Хост HoH (u3): ui/src/hoh-host/{index,tauri}.ts; пока его нет — заглушка с теми же именами.
      if (source === VIRTUAL_HOST) return existsSync(resolve(HOST, 'index.ts')) ? HOST_MODULE : HOST_STUB;
      // Загрузчик T3 как есть: apps/web/src/bootstrap.ts (импортирует их main.tsx и показывает ошибку запуска).
      if (source === VIRTUAL_BOOTSTRAP) return resolve(T3_WEB, 'bootstrap.ts');
      if (!importer?.startsWith(T3_BASE)) return null;
      const route = /(?:^|\/)routes\/([^/]+?)(?:\.tsx)?$/.exec(source)?.[1];
      if (route && route in HIDDEN_ROUTES) return ROUTE_PREFIX + route;
      // Логика палитры: импорт из кода T3 ведёт на обёртку (из самой обёртки — на настоящий файл).
      if (basename(source, '.ts') === 'CommandPalette.logic') {
        const resolved = await this.resolve(source, importer, { skipSelf: true });
        return resolved?.id === resolve(T3_WEB, PALETTE_LOGIC) ? PALETTE_MODULE : null;
      }
      // Скрытый компонент: импорт из кода T3 ведёт на модуль-замену (из самой замены — на настоящий файл).
      if (!HIDDEN_COMPONENT_NAMES.has(basename(source, '.tsx'))) return null;
      const resolved = await this.resolve(source, importer, { skipSelf: true });
      const file = resolved?.id.startsWith(T3_WEB + '/') ? resolved.id.slice(T3_WEB.length + 1) : null;
      return file && file in HIDDEN_COMPONENTS ? COMPONENT_PREFIX + file : null;
    },
    load(id) {
      if (id === PALETTE_MODULE) return paletteFilterModule(resolve(T3_WEB, PALETTE_LOGIC), HIDDEN_PALETTE_ITEMS);
      if (id.startsWith(COMPONENT_PREFIX)) {
        const file = id.slice(COMPONENT_PREFIX.length);
        return hiddenComponentModule(resolve(T3_WEB, file), HIDDEN_COMPONENTS[file]!);
      }
      if (id === HOST_MODULE) {
        return `export { installHohHost } from ${JSON.stringify(resolve(HOST, 'index.ts'))};\n`
          + `export { tauriEngine } from ${JSON.stringify(resolve(HOST, 'tauri.ts'))};\n`;
      }
      if (!id.startsWith(ROUTE_PREFIX)) return null;
      const file = id.slice(ROUTE_PREFIX.length);
      return hiddenRouteModule(file, HIDDEN_ROUTES[file]!);
    },
    transform(code, id) {
      if (id === resolve(T3_WEB, 'components/settings/settingsSearch.ts')) return { code: settingsNavFilter(code), map: null };
      if (id === resolve(T3_WEB, 'components/settings/ProviderSettingsPanel.tsx')) return { code: providerSettingsPanel(code), map: null };
      if (id === resolve(T3_WEB, 'components/settings/ThemePreviewCircles.tsx')) return { code: themeCards(code), map: null };
      if (id === resolve(T3_WEB, 'components/settings/ThemeSettings.tsx')) return { code: themeSettings(code), map: null };
      if (id === resolve(T3_BASE, 'shared/themePalettes.ts')) return { code: themePalettes(code), map: null };
      if (id === resolve(T3_WEB, 'components/ChatView.tsx')) return { code: chatViewHome(code), map: null };
      if (id === resolve(T3_WEB, 'components/Sidebar.tsx')) return { code: sidebarList(code), map: null };
      if (id === resolve(T3_WEB, 'components/sidebar/SidebarChrome.tsx')) return { code: sidebarChrome(code), map: null };
      if (id === resolve(T3_WEB, 'components/NoProjectsHero.tsx')) return { code: noProjectsHero(code), map: null };
      if (id === resolve(T3_WEB, 'components/settings/ProviderInstanceCard.tsx')) return { code: providerInstanceCard(code), map: null };
      return null;
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

/** Скрытые компоненты сверяются с файлами T3: файл есть, названный экспорт в нём есть, иначе сборка останавливается.
 * Так же — логика палитры: filterCommandPaletteGroups в ней есть. */
export function checkHiddenComponents(web: string): void {
  if (!/export function filterCommandPaletteGroups\b/.test(readFileSync(resolve(web, PALETTE_LOGIC), 'utf8'))) {
    throw new Error(`features.ts: ${PALETTE_LOGIC} не экспортирует filterCommandPaletteGroups`);
  }
  for (const [file, names] of Object.entries(HIDDEN_COMPONENTS)) {
    const path = resolve(web, file);
    if (!existsSync(path)) throw new Error(`features.ts: нет компонента T3 ${file}`);
    const code = readFileSync(path, 'utf8');
    for (const name of names) {
      const exported = name === 'default'
        ? /export default /.test(code)
        : new RegExp(`export (?:const|function) ${name}\\b`).test(code);
      if (!exported) throw new Error(`features.ts: ${file} не экспортирует ${name}`);
    }
  }
}

export function t3Config(): UserConfig {
  if (!existsSync(resolve(T3_WEB, 'bootstrap.ts'))) {
    throw new Error('нет основы T3 (src/t3/base/web/bootstrap.ts): сначала перенос u2 (scripts/t3-sync.py)');
  }
  checkHiddenRoutes(T3_WEB);
  checkHiddenComponents(T3_WEB);
  return {
    // От корня приложения (tauri://localhost/), а не от страницы: роутер T3 меняет путь (/settings/general),
    // и после перезагрузки на таком пути относительные адреса ресурсов не нашлись бы.
    base: '/',
    root: UI,
    // boot-theme.js — их встроенный сценарий темы до запуска, вынесенный в файл (CSP окна: script-src 'self').
    publicDir: resolve(import.meta.dirname, 'public'),
    // React Compiler, как в их vite.config.ts (`react({ compiler: true })`): код T3 рассчитывает на его мемоизацию
    // (синхронизация состояния во время отрисовки, напр. useWorktreeSetup в ChatView), без него — бесконечная отрисовка.
    plugins: [hohT3(), react({ compiler: true }), tailwindcss(), t3Page()],
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
