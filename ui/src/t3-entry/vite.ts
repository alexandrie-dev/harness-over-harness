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
import { HOH_THEME_COLORS, MIDNIGHT_PRISM_COLORS } from './hoh-theme';
import { HIDDEN_COMPONENTS, HIDDEN_PALETTE_ITEMS, HIDDEN_ROUTES, VISIBLE_PROVIDER_DRIVERS, VISIBLE_SETTINGS, VISIBLE_SETTINGS_ITEMS } from './features';
import { deferRouterScrollStorage } from './router-scroll';

const UI = resolve(import.meta.dirname, '../..');
export const T3_BASE = resolve(UI, 'src/t3/base');
const T3_WEB = resolve(T3_BASE, 'web');
const HOST = resolve(UI, 'src/hoh-host');
const HOST_STUB = resolve(import.meta.dirname, 'host-stub.ts');
const PROJECT_GROUPING_SETTING = resolve(UI, 'src/t3-entry/hoh-screens/ProjectGroupingSetting.tsx');
const HOST_MODULE = '\0hoh-host';
// This address is answered by the in-page HoH adapter. No socket is opened.
const HOH_HTTP_URL = 'http://hoh.local';
export const T3_PAGE = 't3.html';
export const T3_ASSETS = 't3-assets';

/** Псевдонимы основы T3: их пишет scripts/t3-sync.py (u2) в src/t3/base/aliases.json, пути — от ui/. */
function t3Aliases() {
  const table: Record<string, string> = JSON.parse(readFileSync(resolve(T3_BASE, 'aliases.json'), 'utf8'));
  // The local theme importer uses the window's pinned JSONC parser.
  delete table['jsonc-parser'];
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

/** Settings navigation and search both read the same allowlist. */
export function settingsNavFilter(code: string): string {
  const declaration = 'export const HOH_SETTINGS_PATHS: ReadonlySet<SettingsPath> = new Set<SettingsPath>(';
  const from = code.indexOf(declaration);
  const end = '\n]);';
  const to = code.indexOf(end, from);
  if (from < 0 || to < 0) throw new Error('settingsSearch.ts: missing HoH settings paths');
  const items = `const HOH_SETTINGS_ITEMS: Readonly<Record<string, readonly string[]>> = ${JSON.stringify(VISIBLE_SETTINGS_ITEMS)};\n`;
  return anchorPatch('settingsSearch.ts', code, [
    [code.slice(from, to + end.length), declaration + JSON.stringify(VISIBLE_SETTINGS) + ');'],
    ['      HOH_SETTINGS_PATHS.has(item.to) &&', '      HOH_SETTINGS_PATHS.has(item.to) &&\n      (HOH_SETTINGS_ITEMS[item.to]?.includes(item.id) ?? true) &&'],
    ['msg("name icon emoji image checkout remove delete")', 'msg("name pin archive folder checkout remove delete")'],
  ], items);
}

function settingsRouteTree(code: string): string {
  const declaration = 'const ChatSettingsRouteChildren = {\n';
  const end = '} as ChatSettingsRouteChildren';
  const from = code.indexOf(declaration);
  const to = code.indexOf(end, from);
  if (from < 0 || to < 0) throw new Error('routeTree.gen.ts: missing settings routes');
  const current = code.slice(from, to);
  const settings = [...VISIBLE_SETTINGS];
  if (settings.includes('/settings/about')) settings.push('/settings/open-source-licenses');
  const missing = settings.map(path => {
    const words = path.slice('/settings/'.length).split('-');
    return 'ChatSettings' + words.map(word => word.charAt(0).toUpperCase() + word.slice(1)).join('') + 'Route';
  }).filter(route => !current.includes(`  ${route}:`));
  for (const route of missing) {
    if (!code.includes(`const ${route} =`)) throw new Error(`routeTree.gen.ts: missing ${route}`);
  }
  return anchorPatch('routeTree.gen.ts', code, [[
    current + end,
    current + missing.map(route => `  ${route}: ${route},\n`).join('') + end,
  ]]);
}

const PROVIDER_ANCHORS = [
  ['    <SettingsPageContainer className="@container/providers gap-8">\n      <ProviderSettingsPanelContent', '    <SettingsPageContainer className="@container/providers gap-8">\n      <p className="text-muted-foreground px-1 text-sm">Channel plugins are disabled in the window</p>\n      <ProviderSettingsPanelContent'],
  ['const visibleProviderSettings = PROVIDER_SETTINGS.filter(', 'const visibleProviderSettings = PROVIDER_SETTINGS.filter((entry) => HOH_PROVIDER_DRIVERS.includes(entry.provider)).filter('],
  ['const canAddProvider =\n', 'const canAddProvider = false &&\n'],
  ['    const driver = instance.driver;\n    const list = instancesByDriver.get(driver)', '    const driver = instance.driver;\n    if (!HOH_PROVIDER_DRIVERS.includes(driver)) continue;\n    const list = instancesByDriver.get(driver)'],
  ['    const isDirty = explicitInstance !== undefined;', '    const isDirty = Boolean(explicitInstance?.displayName || explicitInstance?.accentColor);'],
  // Настройки, которых движок не исполняет (путь к claude, свои аргументы, «Usage providers», проверка состояния, сжатие): не рисуются.
  ['<UsageProviderSettings\n', '<HohNone\n'],
  ['      <SettingsSection title={gt("Advanced")}>', '      {false ? <SettingsSection title={gt("Advanced")}>'],
  ['        <CompactIdleThreadsRow />\n      </SettingsSection>\n', '        <CompactIdleThreadsRow />\n      </SettingsSection> : null}\n'],
] as const;
const CARD_ANCHORS = [
  ['    instance.displayName?.trim() || driverOption?.label || String(instance.driver);', '    instance.displayName?.trim() || liveProvider?.displayName || driverOption?.label || String(instance.driver);'],
  ['          <ManagedControl reason={enabled ? undefined : managedDriverReason}>', '          {projectOverride !== undefined ? <ManagedControl reason={enabled ? undefined : managedDriverReason}>'],
  ['          </ManagedControl>\n        </span>', '          </ManagedControl> : null}\n        </span>'],
  // Runtime and environment settings require control of the provider's underlying process.
  ['!driverOption || deriveProviderSettingsFields(driverOption, undefined, m).length > 0 ? (', 'false ? ('],
  ['      <SettingsSection\n        title={gt("Environment")}', '      {false ? <SettingsSection\n        title={gt("Environment")}'],
  ['      </SettingsSection>\n      {environmentId !== undefined && liveProvider?.driver === "acpRegistry" ? (', '      </SettingsSection> : null}\n      {environmentId !== undefined && liveProvider?.driver === "acpRegistry" ? ('],
  ['      {driverOption !== undefined ? (\n        <SettingsSection title={gt("Models")}>', '      <HohProviderSetup provider={liveProvider} />\n      {liveProvider?.status === "ready" ? (\n        <SettingsSection title={gt("Models")}>'],
  ['                Model preferences and custom models are saved on the selected environment and shared\n                across connected devices.', '                Choose which models appear in the model picker and which to mark as favorites.'],
  ['              canManageCustomModels={!readOnly}', '              canManageCustomModels={false}'],
] as const;
function anchorPatch(file: string, code: string, edits: ReadonlyArray<readonly [string, string]>, header = ''): string {
  let out = code;
  for (const [from, to] of edits) {
    if (!out.includes(from)) throw new Error(`${file}: нет «${from.trim()}» — T3 изменился, проверьте features.ts`);
    out = out.replace(from, to);
  }
  return header + out;
}

function commandPalettePicker(code: string): string {
  const picker = resolve(UI, 'src/t3-entry/macos-project-picker.ts');
  return anchorPatch('CommandPalette.tsx', code, [
    ['  const canOpenProjectFromFileManager =\n', '  const pickMacFolder = macProjectPicker(browseEnvironmentId);\n  const canOpenProjectFromFileManager =\n'],
    ['    window.desktopBridge !== undefined;', '    (window.desktopBridge !== undefined || pickMacFolder !== undefined);'],
    [
      '    const command = resolveShortcutCommand(event, keybindings, {',
      `    if (pickMacFolder && canOpenProjectFromFileManager && isPrimaryModifierPressed(event) &&
        !event.altKey && !event.shiftKey && !event.nativeEvent.isComposing &&
        event.key.toLowerCase() === "o") {
      (event as typeof event & { preventBaseUIHandler?: () => void }).preventBaseUIHandler?.();
      event.preventDefault();
      event.stopPropagation();
      if (!event.repeat) void handleOpenProjectFromFileManager();
      return;
    }

    const command = resolveShortcutCommand(event, keybindings, {`,
    ],
    [
      '      pickedPath = await api.dialogs.pickFolder(\n        Object.keys(pickerOptions).length > 0 ? pickerOptions : undefined,\n      );',
      '      pickedPath = pickMacFolder\n        ? await pickMacFolder()\n        : await api.dialogs.pickFolder(\n            Object.keys(pickerOptions).length > 0 ? pickerOptions : undefined,\n          );',
    ],
    ['      disabled={isPickingProjectFolder}\n', '      disabled={isPickingProjectFolder}\n      aria-keyshortcuts={pickMacFolder ? "Meta+O" : undefined}\n'],
    ['      {gt("Open in {app}", { app: fileManagerName })}\n', '      {gt("Open in {app}", { app: fileManagerName })}\n      {pickMacFolder ? <Kbd aria-hidden>⌘O</Kbd> : null}\n'],
  ], `import { macProjectPicker } from ${JSON.stringify(picker)};\nimport { Kbd } from "./ui/kbd";\n`);
}

/** Providers (ProviderSettingsPanel.tsx): только поставщики VISIBLE_PROVIDER_DRIVERS, без «Add provider» и «+», без настроек, которых движок не исполняет. */
export function providerSettingsPanel(code: string): string {
  return anchorPatch('ProviderSettingsPanel.tsx', code, PROVIDER_ANCHORS,
    `const HOH_PROVIDER_DRIVERS: readonly string[] = ${JSON.stringify(VISIBLE_PROVIDER_DRIVERS)}; // HoH (vite.ts, features.ts)\nconst HohNone = (_props: Record<string, unknown>) => null;\n`);
}
export function providerInstanceCard(code: string): string {
  const setup = resolve(UI, 'src/t3-entry/hoh-screens/ProviderSetup.tsx');
  return anchorPatch('ProviderInstanceCard.tsx', code, CARD_ANCHORS,
    `import { ProviderSetup as HohProviderSetup } from ${JSON.stringify(setup)};\n`);
}

function welcomeProviderActions(code: string): string {
  const from = code.indexOf('    // Settings updates a broken CLI');
  const to = code.indexOf('\n  };', from);
  if (from < 0 || to < 0) throw new Error('WelcomePage.tsx: missing provider setup actions');
  return anchorPatch('WelcomePage.tsx', code, [
    [code.slice(from, to), `    return settingsAction(
      state === "install" ? gt("Install") : state === "signIn" ? gt("Sign in") : gt("Details"),
    );`],
    ['      <Collapsible open={showMore} onOpenChange={setMoreOpen}>', '      {available.length > 0 || acpAllowed ? <Collapsible open={showMore} onOpenChange={setMoreOpen}>'],
    ['      </Collapsible>\n      {codexSetupProvider', '      </Collapsible> : null}\n      {codexSetupProvider'],
  ]);
}

function keybindingSettings(code: string): string {
  const fileButton = code.indexOf('aria-label={gt("Open keybindings.json")}');
  const from = code.lastIndexOf('          <Tooltip>\n', fileButton);
  const end = '          </Tooltip>\n';
  const to = code.indexOf(end, fileButton);
  if (fileButton < 0 || from < 0 || to < 0) throw new Error('KeybindingsSettings.tsx: missing keybindings file control');
  return anchorPatch('KeybindingsSettings.tsx', code, [
    [code.slice(from, to + end.length), ''],
    ['headerAction={!isElectron ? <BrowserKeybindingNotice /> : null}', 'headerAction={!isElectron && !isTauri() ? <BrowserKeybindingNotice /> : null}'],
  ], 'import { isTauri } from "@tauri-apps/api/core";\n');
}

/** Имена тем в Appearance → Themes: без названий продуктов T3 («T3 Code» → «Default», «T3 Chat» → «Chat»). */
export function themeCards(code: string): string {
  return anchorPatch('ThemePreviewCircles.tsx', code, [['label: "T3 Code",', 'label: "Default",']]);
}
/** Добавляет темы HoH в Appearance → Themes. */
export function themeSettings(code: string): string {
  return anchorPatch('ThemeSettings.tsx', code, [
    ['const MAINTAINER_THEMES: ReadonlyArray<ThemeDefinition> = [\n', 'const MAINTAINER_THEMES: ReadonlyArray<ThemeDefinition> = [\n  HOH_THEME,\n  MIDNIGHT_PRISM_THEME,\n'],
    ['    <div className="space-y-3">\n', '    <div className="space-y-3">\n      <HohProjectGroupingSetting />\n'],
    ['          {/* HoH (ui/T3-MAP.md): импорт тем VS Code / Open VSX выключен — кнопки «Add theme» нет. */}', `          <Button size="xs" variant="outline" onClick={() => onImportOpenChange(true)}>
            <PlusIcon />
            <T>Import theme</T>
          </Button>`],
  ], 'import { HOH_THEME, MIDNIGHT_PRISM_THEME } from "@t3tools/shared/themePalettes";\nimport { ProjectGroupingSetting as HohProjectGroupingSetting } from ' + JSON.stringify(PROJECT_GROUPING_SETTING) + ';\n');
}
function themeImportDialog(code: string): string {
  const searchAnchor = '      <ThemeSearchSection\n';
  const localImportAnchor = '      {(() => {\n        const dropHandlers';
  const from = code.indexOf(searchAnchor);
  const to = code.indexOf(localImportAnchor, from);
  if (from < 0 || to < 0) throw new Error('ThemeImportDialog.tsx: missing theme import sections');
  return anchorPatch('ThemeImportDialog.tsx', code, [
    ['import { ThemeSearchSection } from "./ThemeSearchSection";\n', ''],
    [code.slice(from, to), ''],
    ['<T>Add a theme</T>', '<T>Import a theme</T>'],
    ['gt("Drop T3 Code or VS Code .json files")', 'gt("Drop a theme JSON file")'],
  ]);
}
export function themePalettes(code: string): string {
  const midnightPrismId = JSON.stringify('hoh.midnight-prism');
  const themes = `export const HOH_THEME: ThemeDefinition = { id: "hoh", label: "HoH", appearance: "dark", colors: ${JSON.stringify(HOH_THEME_COLORS)} };\n`
    + `export const MIDNIGHT_PRISM_THEME: ThemeDefinition = { id: ${midnightPrismId}, label: "Midnight Prism", appearance: "dark", colors: ${JSON.stringify(MIDNIGHT_PRISM_COLORS)} };\n`;
  return anchorPatch('themePalettes.ts', code, [
    ['label: "T3 Chat",', 'label: "Chat",'],
    ['export const BUILT_IN_THEME_IDS = ["t3-chat",', `export const BUILT_IN_THEME_IDS = ["hoh", ${midnightPrismId}, "t3-chat",`],
    ['export const BUILT_IN_THEMES: ReadonlyArray<ThemeDefinition> = [\n', themes + 'export const BUILT_IN_THEMES: ReadonlyArray<ThemeDefinition> = [\n  HOH_THEME,\n  MIDNIGHT_PRISM_THEME,\n'],
  ]);
}

/**
 * Главная (HOH-41 В2б) — пустой разговор T3 целиком: `/` у T3 открывает черновик, в нём их поле ввода (чип модели с уровнями,
 * Permissions, выбор проекта, отправка). Над полем подставлено содержимое главной (hoh-screens/HomeBody.tsx):
 * ChatView.tsx — колонка вместо центрирования в «пустом» состоянии и тело главной над заголовком черновика.
 */
const HOME_BODY = resolve(UI, 'src/t3-entry/hoh-screens/HomeBody.tsx');
const SIDEBAR_TOP = resolve(UI, 'src/t3-entry/hoh-screens/SidebarTop.tsx');
const SIDEBAR_TOGGLE = resolve(UI, 'src/t3-entry/hoh-screens/SidebarToggle.tsx');
const SIDEBAR_PAGES = resolve(UI, 'src/t3-entry/hoh-screens/pages.ts');
const STEEL_BACKDROP = resolve(UI, 'src/t3-entry/hoh-screens/SteelBackdrop.tsx');

// Tauri needs a titlebar hit area before a chat header exists.
const NATIVE_TITLEBAR = [
  '{isElectron ? <WorkspacePageHeader electron /> : null}',
  '{(isElectron || isTauri()) && <WorkspacePageHeader electron={isElectron} className={isElectron ? undefined : "absolute inset-x-0 top-0"} />}',
] as const;
const TAURI_IMPORT = 'import { isTauri } from "@tauri-apps/api/core";\n';
function chatIndexHome(code: string): string {
  const pageStore = `import { useSyncExternalStore as useHohPageStore } from "react";\nimport { pages as hohPages } from ${JSON.stringify(SIDEBAR_PAGES)};\n`;
  return anchorPatch('_chat.index.tsx', code, [
    ['  const router = useRouter();', `  const router = useRouter();
  const hohPage = useHohPageStore(hohPages.subscribe, hohPages.current, () => null);`],
    ['    // Starting a draft after the user has left would replace the page they picked.', `    // A Workspace page can create the first scratch project to prepare its own draft.
    if (hohPages.current()?.startsWith("workspace:")) return;
    // Starting a draft after the user has left would replace the page they picked.`],
    ['  }, [attempt, handleNewThread, mostRecentProject]);', '  }, [attempt, handleNewThread, mostRecentProject, hohPage]);'],
    ['  if (mostRecentProject !== null) {', '  if (mostRecentProject !== null && !hohPage?.startsWith("workspace:")) {'],
    ['    void handleNewThread(projectRef, { replace: true })',
      `    // The landing route can reopen the project draft that New chat just docked.
    // Clear the view marker before navigation so that draft can render Home.
    try { sessionStorage.removeItem("hoh-project-new-chat"); } catch { /* Storage is optional. */ }
    window.dispatchEvent(new CustomEvent("hoh-project-new-chat", { detail: null }));
    void handleNewThread(projectRef, { replace: true })`,
    ], NATIVE_TITLEBAR,
  ], TAURI_IMPORT + pageStore);
}
export function chatViewHome(code: string): string {
  return anchorPatch('ChatView.tsx', code, [
    ['            data-chat-header\n', '            data-chat-header\n            data-tauri-drag-region="deep"\n'],
    ['const isGitRepo = liveIsGitRepo ?? recallCheckoutIsRepo(environmentId, gitStatusCwd) ?? true;',
      'const isGitRepo = liveIsGitRepo ?? recallCheckoutIsRepo(environmentId, gitStatusCwd) ?? false;'],
    ['                                  showGitControls={isGitRepo}', '                                  showGitControls={false}'],
    ['"pointer-events-none absolute inset-0 z-20 flex items-center [@media(max-height:34rem)]:items-end"', '"hoh-draft-hero pointer-events-none absolute inset-0 z-20 flex flex-col justify-end"'],
    ['              <div ref={draftHeroTransitionGroupRef} className="chat-composer-lane w-full">', '              {isDraftHeroState ? <HohHomeBody /> : null}\n              <div ref={draftHeroTransitionGroupRef} className="chat-composer-lane w-full">'],
  ], `import { HomeBody as HohHomeBody } from ${JSON.stringify(HOME_BODY)};\n`);
}

function timelineMessageEdit(code: string): string {
  const editAction = resolve(UI, 'src/t3-entry/hoh-screens/EditUserMessageAction.tsx');
  return anchorPatch('TimelineUserMessageRow.tsx', code, [[
    '        {copyText && (',
    `        {ctx.threadRef && ctx.canOperateThread && userMessage.attribution === null && (
          <HohEditUserMessageAction
            threadId={ctx.threadRef.threadId}
            messageId={row.message.id}
            text={plainText}
            disabled={busy}
            onOpenChat={ctx.onOpenThread}
          />
        )}
        {copyText && (`,
  ], [
    '  const revertTurnCount = row.revertTurnCount;\n  const attachmentReferences',
    '  const activity = use(TimelineRowActivityCtx);\n  const busy = activity.isWorking || activity.isRevertingCheckpoint;\n  const attachmentReferences',
  ]], `import { EditUserMessageAction as HohEditUserMessageAction } from ${JSON.stringify(editAction)};\n`);
}
/** SidebarTop combines the HoH navigation with T3 search, project controls, and chat organization. */
export function sidebarList(code: string): string {
  return anchorPatch('Sidebar.tsx', code, [
    ['import { SidebarHeaderIconButton, SidebarThreadHeader } from "./sidebar/SidebarThreadHeader";', 'import { SidebarHeaderIconButton } from "./sidebar/SidebarThreadHeader";\nimport { SidebarTop as SidebarThreadHeader } from ' + JSON.stringify(SIDEBAR_TOP) + ';'],
    ['  const [settledShelfExpanded, setSettledShelfExpanded] = useLocalStorage(', '  const [storedSettledShelfExpanded, setSettledShelfExpanded] = useLocalStorage('],
    ['  const [snoozedShelfExpanded, setSnoozedShelfExpanded]', '  const settledShelfExpanded = HOH_SHOW_SETTLED_SHELF && storedSettledShelfExpanded;\n  const [snoozedShelfExpanded, setSnoozedShelfExpanded]'],
    [`  const renderedSettledThreads = settledShelfExpanded
    ? visibleSettledThreads
    : routeThreadOnly(visibleSettledThreads, routeRowThreadKey);`, `  const renderedSettledThreads = HOH_SHOW_SETTLED_SHELF
    ? settledShelfExpanded
      ? visibleSettledThreads
      : routeThreadOnly(visibleSettledThreads, routeRowThreadKey)
    : EMPTY_THREADS;`],
    [`    items.push({ kind: "marker", marker: "settled-header" });
    const settledRows = rowsOf(renderedSettledThreads, "settled");
    items.push({ kind: "marker", marker: "settled-placeholder" });
    items.push(...settledRows);`, `    if (HOH_SHOW_SETTLED_SHELF) {
      items.push({ kind: "marker", marker: "settled-header" });
      items.push({ kind: "marker", marker: "settled-placeholder" });
      items.push(...rowsOf(renderedSettledThreads, "settled"));
    }`],
    ['        settledCount ===\n', '        (HOH_SHOW_SETTLED_SHELF ? settledCount : 0) ===\n'],
    ['          (settledCount === 0 || scopedProjectGroups.length > 0) ? (', '          (!HOH_SHOW_SETTLED_SHELF || settledCount === 0 || scopedProjectGroups.length > 0) ? ('],
    ['                <T>No chats yet</T>', '                settledCount > 0 ? <T>No active chats</T> : <T>No chats yet</T>'],
    ['scopedProjectGroups.length > 0 || (group.pinned && !group.archived)', '!group.archived'],
    ['section === "active" || section === "pinned" || section === "working";', 'false;'],
    // Пустой черновик (разговор без первого сообщения: хост даёт ему latestUserMessageAt = null) в «Recent chats» не показывается.
    ['  const threads = useQuietThreadShells();', '  const allThreads = useQuietThreadShells();\n  const threads = useHohMemo(() => allThreads.filter((thread) => thread.latestUserMessageAt != null), [allThreads]);'],
    ['  const handleNewThreadClick = (_event?: ReactMouseEvent) => {', '  const scratch = useHohScratchProject();\n  const scratchId = scratch.scratchEnvironmentId(null);\n  const handleNewThreadClick = (_event?: ReactMouseEvent) => {'],
    ['      if (!started) openCommandPalette({ open: "new-thread-in" });', `      if (!started) {
        if (scratchId === null) openCommandPalette({ open: "new-thread-in" });
        else void scratch.startScratchThread(scratchId);
      }`],
    ['        ref={sidebarContentRef}', '        ref={sidebarContentRef}\n        data-hoh-hide-in-rail=""'],
    ['newThreadDisabled={projects.length === 0}', 'newThreadDisabled={projects.length === 0 && scratchId === null}'],
  ], 'import { useMemo as useHohMemo } from "react";\nimport { useScratchProject as useHohScratchProject } from "~/hooks/useScratchProject";\nconst HOH_SHOW_SETTLED_SHELF = false;\n');
}
function sidebarLayout(code: string): string {
  return anchorPatch('AppSidebarLayout.tsx', code, [
    ['      <SidebarProvider\n', '      <SidebarProvider\n        data-hoh-shell={showSidebar ? "" : undefined}\n'],
    ['    "--sidebar-width":', '    "--sidebar-width-icon": "72px",\n    "--sidebar-width":'],
    ['collapsible="offcanvas"', 'collapsible="icon"'],
    ['  const { toggleSidebar } = useSidebar();', '  const { isMobile, toggleSidebar } = useSidebar();'],
    ['      {showSidebar ? (\n        <Tooltip>', '      {showSidebar && isMobile ? (\n        <Tooltip>'],
    ['        {children}\n        <BackgroundThreadAnnouncer />', '        {showSidebar ? <HohSteelBackdrop /> : null}\n        {children}\n        <BackgroundThreadAnnouncer />'],
  ], `import { SteelBackdrop as HohSteelBackdrop } from ${JSON.stringify(STEEL_BACKDROP)};\n`);
}

export function sidebarChrome(code: string): string {
  return anchorPatch('SidebarChrome.tsx', code, [
    ['  SidebarTrigger,\n', ''],
    ['aria-label={gt("T3 Code home")}', 'aria-label={gt("HoH home")}'],
    ['"relative flex h-[var(--workspace-topbar-height)] shrink-0 flex-row items-center gap-2 pr-3"', '"hoh-sidebar-header relative flex shrink-0 items-center"'],
    ['        isElectron && "drag-region",\n      )}\n    >', '        isElectron && "drag-region",\n      )}\n      data-hoh-sidebar-header=""\n      data-tauri-drag-region="deep"\n    >'],
    [`      <div className="relative top-[calc(var(--workspace-controls-top)-env(safe-area-inset-top))] z-10 ml-[calc(var(--workspace-controls-left)+1px-env(safe-area-inset-left))] flex shrink-0 items-center md:hidden">
        <SidebarTrigger
          variant={backdropVariant ? "media-navigation" : "ghost"}
          className="relative top-auto translate-y-0"
          aria-label={gt("Toggle main sidebar")}
        />
      </div>
`, ''],
    ['className="relative z-10 flex h-8 min-w-0 flex-1 flex-wrap content-start items-center gap-x-2 overflow-hidden py-0.5"', 'className="hoh-sidebar-identity relative z-10 min-w-0 flex-1"'],
    ['<div className="hidden items-center md:flex">', '<div className="hidden items-center md:flex" data-hoh-hide-in-rail="">'],
    ['<div className="ml-1 flex h-7 items-center">', '<div className="mt-1 flex items-center" data-hoh-hide-in-rail="">'],
    ['      </div>\n    </div>\n  );\n});\n\nexport function SidebarBrandWidthProbe', '      </div>\n      <HohSidebarToggle />\n    </div>\n  );\n});\n\nexport function SidebarBrandWidthProbe'],
    ['function SidebarBrand({ onBackdrop }: { onBackdrop: boolean }) {\n  const gt = useGT();', 'function SidebarBrand({ onBackdrop }: { onBackdrop: boolean }) {\n  const gt = useGT();\n  const { isMobile, setOpenMobile } = useSidebar();'],
    ['"relative z-10 ml-[var(--workspace-titlebar-content-left)] hidden h-7 w-fit min-w-0 shrink-0 items-center overflow-hidden rounded-md outline-hidden ring-ring focus-visible:ring-2 focus-visible:ring-inset md:flex"', '"hoh-sidebar-brand relative z-10 flex min-w-0 items-center outline-hidden ring-ring focus-visible:ring-2 focus-visible:ring-inset"'],
    ['      to="/"\n    >\n      <SidebarBrandMark onBackdrop={onBackdrop} />', `      to="/"
      onClick={() => {
        hohSidebarPages.close();
        if (isMobile) setOpenMobile(false);
      }}
    >
      <span className="hoh-sidebar-avatar"><SidebarBrandMark /></span>
      <span className="hoh-sidebar-brand__name" data-hoh-rail-label="">Alexandrie</span>`],
    ['<SidebarBrandMark onBackdrop={false} />', '<SidebarBrandMark />'],
    [`function SidebarBrandMark({ onBackdrop }: { onBackdrop: boolean }) {
  return (
    <span className="inline-flex min-w-0 items-baseline gap-1 text-sm font-medium tracking-tight">
      <T3Wordmark aria-label="T3" className="h-[1cap] w-auto shrink-0" />
      <span
        className={cn(
          "truncate [text-box:trim-both_cap_alphabetic] supports-[text-box:trim-both_cap_alphabetic]:py-1",
          onBackdrop ? "text-white/70" : "text-muted-foreground",
        )}
      >
        {/* HoH (ui/T3-MAP.md): имя продукта — HoH, знак — эмблема HoH (замена T3Wordmark.tsx). */}
        {"HoH"}
      </span>
    </span>
  );
}`, `function SidebarBrandMark() {
  return <T3Wordmark className="block shrink-0" />;
}`],
    ['<SidebarMenuButton aria-label={label} onClick={onClick} size="icon">\n              {icon}\n            </SidebarMenuButton>', '<SidebarMenuButton className="hoh-side__action" aria-label={label} onClick={onClick}>\n              {icon}\n              <span data-hoh-rail-label="">{label}</span>\n            </SidebarMenuButton>'],
    ['<SidebarMenu className="flex-row items-center">', '<SidebarMenu className="hoh-sidebar-utilities flex-row flex-wrap items-center">'],
    ['<SidebarMenuButton onClick={handleBackClick}>', '<SidebarMenuButton className="hoh-side__action" aria-label={gt("Back")} tooltip={gt("Back")} onClick={handleBackClick}>'],
    ['            <span>\n              <T>Back</T>', '            <span data-hoh-rail-label="">\n              <T>Back</T>'],
    [`      <SidebarProviderUpdatePill />
      <SidebarUpdateArchitectureWarning />`, `      <div className="contents" data-hoh-hide-in-rail="">
        <SidebarProviderUpdatePill />
        <SidebarUpdateArchitectureWarning />
      </div>`],
  ], `import { SidebarToggle as HohSidebarToggle } from ${JSON.stringify(SIDEBAR_TOGGLE)};
import { pages as hohSidebarPages } from ${JSON.stringify(SIDEBAR_PAGES)};
`);
}

function composerWhisper(code: string): string {
  const hook = resolve(import.meta.dirname, 'hoh-screens/whisper/useComposerSpeechInput.ts');
  return anchorPatch('ChatComposer.tsx', code, [[
    'import { useEnvironmentSpeechInput } from "../../speech/useEnvironmentSpeechInput";',
    `import { useHohComposerSpeechInput as useEnvironmentSpeechInput } from ${JSON.stringify(hook)};`,
  ], [
    '            {speechInput.preview ? (',
    `            {speechInput.whisperError && (
              <p className="mx-3 mb-2 text-xs text-destructive sm:mx-4">{speechInput.whisperError}</p>
            )}

            {speechInput.preview ? (`,
  ]]);
}

function composerSecret(code: string): string {
  const action = resolve(import.meta.dirname, 'hoh-screens/ChatSecretAction.tsx');
  return anchorPatch('ChatComposer.tsx', code, [[
    '                    {speechInput.available ? (',
    `                    {canOperateThread && !projectSelectionRequired && !noProviderAvailable &&
                      pendingUserInputs.length === 0 && !isEditingQueuedMessage && !dictationTakesFooter ? (
                      <HohChatSecretAction
                        key={attachmentTargetKey}
                        onInsert={(path) => {
                          if (!composerMountedRef.current || attachmentTargetKeyRef.current !== attachmentTargetKey) return false;
                          const prefix = promptRef.current.trim() ? "\\n\\n" : "";
                          return insertComposerTextAtEndStable(prefix +
                            "Use the temporary secret file at " + JSON.stringify(path) +
                            " for this task. Keep its contents out of command output and chat.");
                        }}
                      />
                    ) : null}
                    {speechInput.available ? (`,
  ]], `import { ChatSecretAction as HohChatSecretAction } from ${JSON.stringify(action)};\n`);
}
function chatSettingsWhisper(code: string): string {
  const setting = resolve(import.meta.dirname, 'hoh-screens/whisper/WhisperSetting.tsx');
  return anchorPatch('SettingsPanels.tsx', code, [[
    '    <SettingsPageContainer>\n      <SettingsSection id="composer"',
    '    <SettingsPageContainer>\n      <HohWhisperSetting />\n      <SettingsSection id="composer"',
  ]], `import { WhisperSettingsLink as HohWhisperSetting } from ${JSON.stringify(setting)};\n`);
}

function aboutSettingsUpdate(code: string): string {
  const row = resolve(import.meta.dirname, 'hoh-screens/UpdateVersionRow.tsx');
  return anchorPatch('SettingsPanels.tsx', code, [[
    '{isElectron || HOSTED_APP_CHANNEL ? (\n          <AboutVersionSection />',
    '{isTauri() ? (\n          <UpdateVersionRow id={setting("app-version").id} />\n        ) : isElectron || HOSTED_APP_CHANNEL ? (\n          <AboutVersionSection />',
  ]], TAURI_IMPORT + `import { UpdateVersionRow } from ${JSON.stringify(row)};\n`);
}

function localSettingsRoute(code: string, panel: string, original: string, replacement: string): string {
  return anchorPatch(panel + ' route', code, [[
    `import { ${panel} } from "../components/settings/${original}";`,
    `import { ${panel} } from ${JSON.stringify(resolve(UI, 'src/t3-entry/hoh-screens', replacement))};`,
  ]]);
}

/** HoH accepts pasted text as text. The native engine only accepts raster attachments. */
function composerPaste(code: string): string {
  const pastedImage = resolve(UI, 'src/t3-entry/pastedImage.ts');
  return anchorPatch('ChatComposer.tsx', code, [
    ['    if (pastedContext) return;', `    if (pastedContext) return;
    // WebKitGTK can expose HTML or other formats while hiding the clipboard image itself.
    if (window.__TAURI_INTERNALS__ && event.nativeEvent.isTrusted && activeThreadId &&
        !bypassAutoAttachment && files.length === 0 && !plainText) {
      const draftKey = attachmentTargetKeyRef.current;
      pendingDraftWork.begin(draftKey);
      void readHohPastedImage().then((file) => {
        if (file && composerMountedRef.current && attachmentTargetKeyRef.current === draftKey) {
          return addComposerAttachments([file]);
        }
      }).catch((error: unknown) => {
        if (composerMountedRef.current && attachmentTargetKeyRef.current === draftKey) {
          setThreadError(activeThreadId, typeof error === "string" ? error : "Could not paste the image.");
        }
      }).finally(() => pendingDraftWork.end(draftKey));
      return;
    }`],
    ['  const { addComposerAttachments, removeComposerImage, foldPastedText } =', '  const { addComposerAttachments, removeComposerImage } ='],
    ['    if (!foldPastedText(plainText, bypassAutoAttachment)) {\n      return;\n    }\n\n    event.preventDefault();\n    event.stopPropagation();', '    // Leave text to the editor, including long and multiline clipboard content.'],
    [`      pasteTextAtEnd: (text: string, options) => {
        const shortcutArmed = consumePasteAsTextShortcut(pasteAsTextShortcutUntilRef);
        const bypassAutoAttachment = options?.bypassAutoAttachment === true || shortcutArmed;
        const promptLength = promptRef.current.length;
        if (
          !foldPastedText(text, bypassAutoAttachment, {
            start: promptLength,
            end: promptLength,
          })
        ) {
          return false;
        }
        focusComposer();
        return true;
      },`, '      pasteTextAtEnd: () => false,'],
    ['      foldPastedText,\n', ''],
  ], `import { readPastedImage as readHohPastedImage } from ${JSON.stringify(pastedImage)};\n`);
}
function editorPaste(code: string): string {
  return anchorPatch('ComposerPromptEditorTiptap.tsx', code, [
    ['            clipboardData.files.length > 0 &&\n            (literalText || !readPastedComposerContext(clipboardData)?.records.length)', '            clipboardData.files.length > 0 &&\n            !readPastedPlainText(clipboardData) &&\n            (literalText || !readPastedComposerContext(clipboardData)?.records.length)'],
    ['          if (!pastedText) return false;\n          event.preventDefault();', '          if (!pastedText) return false;'],
    ['          if (isInCodeBlock(view)) {\n            const { from, to } = view.state.selection;', '          if (isInCodeBlock(view)) {\n            event.preventDefault();\n            const { from, to } = view.state.selection;'],
    ['          ) {\n            view.dispatch(\n              markAsClipboardEdit(\n                view.state.tr.insertText(pastedText, pasteFrom, pasteTo),', '          ) {\n            event.preventDefault();\n            view.dispatch(\n              markAsClipboardEdit(\n                view.state.tr.insertText(pastedText, pasteFrom, pasteTo),'],
    ['          if (editorInstance) {\n            const $paste = view.state.selection.$from;', '          if (!editorInstance) return false;\n          {\n            event.preventDefault();\n            const $paste = view.state.selection.$from;'],
  ]);
}

/** Local HTML links open in chat. An assistant can explicitly request an inline card. */
function markdownHtml(code: string): string {
  const preview = resolve(UI, 'src/t3-entry/preview/HtmlDocument.tsx');
  const config = resolve(HOST, 'config.ts');
  return anchorPatch('ChatMarkdown.tsx', code, [
    ['    useChatMarkdownState({ text, inline, ...props });', '    useChatMarkdownState({ text, inline, renderInsights, ...props });'],
    ['  isStreaming = false,\n  skills', '  isStreaming = false,\n  renderInsights = false,\n  skills'],
    ['    mediaSource?: string,\n  ) => {', '    mediaSource?: string,\n    showHtmlPreview = false,\n  ) => {'],
    ['    return (\n      <MarkdownFileLink\n        environmentId={environmentId}', `    const renderChip = (onPreviewHtml?: () => void) => (
      <MarkdownFileLink
        onPreviewHtml={onPreviewHtml}
        environmentId={environmentId}`],
    ['      />\n    );\n  };\n\n  const componentState = {', `      />
    );
    if (!inline && fileLinkMeta.line === undefined && threadRef?.environmentId === HOH_ENVIRONMENT_ID && isTauri() &&
        navigator.platform.startsWith("Linux") && /[.]html?$/i.test(fileLinkMeta.filePath)) {
      return <HohHtmlDocument key={threadRef.threadId + ":" + fileLinkMeta.filePath} sessionId={threadRef.threadId}
        path={fileLinkMeta.filePath} title={fileLinkMeta.basename}
        autoOpen={showHtmlPreview && renderInsights && !isStreaming} file={renderChip} />;
    }
    return renderChip();
  };

  const componentState = {`],
    ['    const chip = fileLinkChip(fileLinkMeta, copyMarkdown, normalizedHref);',
      '    const chip = fileLinkChip(fileLinkMeta, copyMarkdown, normalizedHref, _title === "hoh-preview");'],
  ], 'import { isTauri } from "@tauri-apps/api/core";\n'
    + `import { HtmlDocument as HohHtmlDocument } from ${JSON.stringify(preview)};\n`
    + `import { HOH_ENVIRONMENT_ID } from ${JSON.stringify(config)};\n`);
}

function markdownHtmlFileLink(code: string): string {
  return anchorPatch('MarkdownFileLink.tsx', code, [
    ['  onOpenInPanel:', '  onPreviewHtml?: (() => void) | undefined;\n  onOpenInPanel:'],
    ['  onOpenInPanel,', '  onPreviewHtml,\n  onOpenInPanel,'],
    ['          hasPrimaryAction ? (', '          hasPrimaryAction || onPreviewHtml ? ('],
    ['                if (useBrowserPrimaryAction) {', `                if (onPreviewHtml) {
                  onPreviewHtml();
                  return;
                }
                if (useBrowserPrimaryAction) {`],
    ['    previous.onOpenInPanel === next.onOpenInPanel &&',
      '    previous.onPreviewHtml === next.onPreviewHtml &&\n    previous.onOpenInPanel === next.onOpenInPanel &&'],
  ]);
}

function markdownHtmlMarker(code: string): string {
  return anchorPatch('markdownPipeline.ts', code, [
    ['    a: [...(defaultSchema.attributes?.a ?? []), "dataPullRequestAutolink"],',
      '    a: [...(defaultSchema.attributes?.a ?? []), "dataPullRequestAutolink", ["title", "hoh-preview"]],'],
  ]);
}

/** From a clean profile, New chat invokes the same scratch action as Home. */
export function chatRouteCommands(code: string): string {
  return anchorPatch('chatRouteCommands.ts', code, [
    ['      if (!started) openCommandPalette({ open: "new-thread-in" });', `      if (!started) {
        const environmentId = scratch.scratchEnvironmentId(null);
        if (environmentId === null) openCommandPalette({ open: "new-thread-in" });
        else void scratch.startScratchThread(environmentId);
      }`],
    ['        const groups = readProjectGroups();\n        if (', `        const groups = readProjectGroups();
        if (!groups.some((group) => !group.archived)) {
          startNewLocal(event);
          return;
        }
        if (`],
  ]);
}
/** Главная без проектов — «What should we work on?» T3 (эмблема, «Add project», «Start without a project»), заголовок — приветствие с датой. */
export function noProjectsHero(code: string): string {
  return anchorPatch('NoProjectsHero.tsx', code, [
    NATIVE_TITLEBAR,
    ['<T>What should we work on?</T>', '<HohGreeting />'],
  ], TAURI_IMPORT + `import { Greeting as HohGreeting } from ${JSON.stringify(HOME_BODY)};\n`);
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
  let routerStoragePatched = false;
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
      if (file === 'components/chat/ComposerSpeechButton.tsx' &&
          importer === resolve(T3_WEB, 'components/chat/ChatComposer.tsx')) {
        return resolved?.id;
      }
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
      // Router's unused sessionStorage cache can stall WebKitGTK during startup.
      if (id.replaceAll('\\', '/').includes('/@tanstack/router-core/dist/esm/scroll-restoration.js')) {
        const patched = deferRouterScrollStorage(code);
        routerStoragePatched = true;
        return { code: patched, map: null };
      }
      // The home screen is maintained on another line; keep its chat wording until that change lands.
      if (id === HOME_BODY) return { code: code.replace('Answer in the thread', 'Answer in the chat'), map: null };
      if (id === resolve(T3_WEB, 'components/settings/settingsSearch.ts')) return { code: settingsNavFilter(code), map: null };
      if (id === resolve(T3_WEB, 'routeTree.gen.ts')) {
        return {
          code: anchorPatch('routeTree.gen.ts', settingsRouteTree(code), [
            ['  ChatSettingsRoute: ChatSettingsRouteWithChildren,', '  ChatSettingsRoute: ChatSettingsRouteWithChildren,\n  ChatUsageRoute: ChatUsageRoute,'],
          ]),
          map: null,
        };
      }
      if (id === resolve(T3_WEB, 'components/usage/UsagePage.tsx')) {
        const refreshStart = code.indexOf('  const refreshLimits = (automatic = false) =>');
        const refreshEnd = code.indexOf('  const onUsageKeyDown =', refreshStart);
        if (refreshStart < 0 || refreshEnd < 0) throw new Error('UsagePage.tsx: missing limits refresh');
        return {
          code: anchorPatch('UsagePage.tsx', code, [
            ['import { UsageLimitsSection } from "./UsageLimits";', `import { UsageLimitsSection } from ${JSON.stringify(resolve(UI, 'src/t3-entry/hoh-screens/UsagePage.tsx'))};`],
            [code.slice(refreshStart, refreshEnd), '  const refreshLimits = async () => { setLimitsNow(Date.now()); };\n'],
            ['{metric === "cost"\n                ? formatUsd(merged.costUsd, locale)', '{metric === "cost"\n                ? (merged.costQuality.unpricedShare === 1 ? gt("Unavailable") : formatUsd(merged.costUsd, locale))'],
          ]),
          map: null,
        };
      }
      if (id === resolve(T3_WEB, 'components/CommandPalette.tsx')) return { code: commandPalettePicker(code), map: null };
      if (id === resolve(T3_WEB, 'components/settings/ProviderSettingsPanel.tsx')) return { code: providerSettingsPanel(code), map: null };
      if (id === resolve(T3_WEB, 'components/settings/KeybindingsSettings.tsx')) return { code: keybindingSettings(code), map: null };
      if (id === resolve(T3_WEB, 'routes/_chat.settings.threads.tsx')) return {
        code: localSettingsRoute(code, 'ThreadsSettingsPanel', 'SettingsPanels', 'ThreadSettings.tsx'),
        map: null,
      };
      if (id === resolve(T3_WEB, 'routes/_chat.settings.voice.tsx')) return {
        code: localSettingsRoute(code, 'VoiceSettingsPanel', 'VoiceSettingsPanel', 'VoiceSettings.tsx'),
        map: null,
      };
      if (id === resolve(T3_WEB, 'components/settings/SettingsScopeSentence.tsx')) return {
        code: anchorPatch('SettingsScopeSentence.tsx', code, [[
          '  "/settings/about",',
          '  "/settings/about",\n  "/settings/threads",\n  "/settings/voice",',
        ]]),
        map: null,
      };
      if (id === resolve(T3_WEB, 'keybindings.ts')) return {
        code: anchorPatch('keybindings.ts', code, [[
          '    isWeb: !isElectron,\n    isDesktop: isElectron,',
          '    isWeb: !isElectron && !isTauri(),\n    isDesktop: isElectron || isTauri(),',
        ]], 'import { isTauri } from "@tauri-apps/api/core";\n'),
        map: null,
      };
      if (id === resolve(T3_WEB, 'components/settings/ProviderModelsSection.tsx')) return {
        code: anchorPatch('ProviderModelsSection.tsx', code, [[
          '{driverKind !== "antigravity" && !isAdding ? (',
          '{canManageCustomModels && driverKind !== "antigravity" && !isAdding ? (',
        ]]),
        map: null,
      };
      if (id === resolve(T3_WEB, 'components/settings/ThemePreviewCircles.tsx')) return { code: themeCards(code), map: null };
      if (id === resolve(T3_WEB, 'components/settings/ThemeSettings.tsx')) return { code: themeSettings(code), map: null };
      if (id === resolve(T3_WEB, 'components/settings/ThemeImportDialog.tsx')) return { code: themeImportDialog(code), map: null };
      if (id === resolve(T3_BASE, 'shared/themePalettes.ts')) return { code: themePalettes(code), map: null };
      if (id === resolve(T3_BASE, 'client-runtime/secretRequest.ts')) return {
        code: anchorPatch('secretRequest.ts', code, [[
          "Kept out of chat, in a private file this agent's commands can read for 24 hours",
          'The agent’s commands can read the private file during this turn, for up to 5 minutes.',
        ]]),
        map: null,
      };
      if (id === resolve(T3_WEB, 'components/chat/ChatComposer.tsx')) return { code: composerSecret(composerWhisper(composerPaste(code))), map: null };
      if (id === resolve(T3_WEB, 'components/settings/SettingsPanels.tsx')) return { code: aboutSettingsUpdate(chatSettingsWhisper(code)), map: null };
      if (id === resolve(T3_WEB, 'components/ComposerPromptEditorTiptap.tsx')) return { code: editorPaste(code), map: null };
      if (id === resolve(T3_WEB, 'components/ChatMarkdown.tsx')) return { code: markdownHtml(code), map: null };
      if (id === resolve(T3_WEB, 'components/chat/MarkdownFileLink.tsx')) return { code: markdownHtmlFileLink(code), map: null };
      if (id === resolve(T3_BASE, 'shared/markdownPipeline.ts')) return { code: markdownHtmlMarker(code), map: null };
      if (id === resolve(T3_WEB, 'routes/_chat.index.tsx')) return { code: chatIndexHome(code), map: null };
      if (id === resolve(T3_WEB, 'components/ChatView.tsx')) return { code: chatViewHome(code), map: null };
      if (id === resolve(T3_WEB, 'components/onboarding/WelcomePage.tsx')) return {
        code: anchorPatch('WelcomePage.tsx', welcomeProviderActions(code), [NATIVE_TITLEBAR], TAURI_IMPORT), map: null,
      };
      if (id === resolve(T3_WEB, 'components/WorkspacePageHeader.tsx')) return {
        code: anchorPatch('WorkspacePageHeader.tsx', code, [
          ['    <header\n', '    <header\n      data-tauri-drag-region="deep"\n'],
        ]), map: null,
      };
      if (id === resolve(T3_WEB, 'routes/_chat.settings.tsx')) return {
        code: anchorPatch('_chat.settings.tsx', code, [
          ['          <header className="flex h-12', '          <header data-tauri-drag-region="deep" className="flex h-12'],
        ]), map: null,
      };
      if (id === resolve(T3_WEB, 'components/RightPanelTabs.tsx')) return {
        code: anchorPatch('RightPanelTabs.tsx', code, [
          ['        data-right-panel-tabbar\n', '        data-right-panel-tabbar\n        data-tauri-drag-region={props.mode === "inline" ? "deep" : undefined}\n'],
          ['          data-right-panel-tab-list\n', '          data-right-panel-tab-list\n          data-tauri-drag-region={tabScrollState.hasOverflow ? "false" : undefined}\n'],
        ]), map: null,
      };
      if (id === resolve(T3_WEB, 'components/chat/ThreadDetailsPanel.tsx')) return {
        code: anchorPatch('ThreadDetailsPanel.tsx', code, [[
          '      {() => <div className="group/details-items">',
          `      {() => <div className="group/details-items">
        {props.isServerThread ? (
          <ThreadRelationshipsPanel
            environmentId={props.environmentId} threadId={props.threadId}
            showLineage={false} showSideChats
          />
        ) : null}
        {props.gitCwd && props.isGitRepo && props.onOpenChanges ? (
          <GitActionsControl
            activeThreadRef={{ environmentId: props.environmentId, threadId: props.threadId }}
            gitCwd={props.gitCwd}
            workspaceMutationId={props.workspaceMutationId}
            showActions={false}
            onOpenChanges={props.onOpenChanges}
          />
        ) : null}
        {props.showOpenInPicker && props.gitCwd && props.availableEditors.length > 0 ? (
          <OpenInPicker
            environmentId={props.environmentId}
            keybindings={props.keybindings}
            availableEditors={props.availableEditors}
            openInCwd={props.gitCwd}
            enableShortcut={props.openInShortcutEnabled}
            displayMode="panel"
          />
        ) : null}`,
        ]]), map: null,
      };
      if (id === resolve(T3_WEB, 'components/chat/SideChatPane.tsx')) {
        const button = code.indexOf('                <T>Bring back</T>');
        const from = code.lastIndexOf('        <Tooltip>\n', button);
        const end = '        </Tooltip>\n';
        const to = code.indexOf(end, button);
        if (button < 0 || from < 0 || to < 0) throw new Error('SideChatPane.tsx: missing merge action');
        return { code: code.slice(0, from) + code.slice(to + end.length), map: null };
      }
      if (id === resolve(T3_WEB, 'components/chat/TimelineUserMessageRow.tsx')) return { code: timelineMessageEdit(code), map: null };
      if (id === resolve(T3_WEB, 'components/Sidebar.tsx')) return { code: sidebarList(code), map: null };
      if (id === resolve(T3_WEB, 'components/AppSidebarLayout.tsx')) return { code: sidebarLayout(code), map: null };
      if (id === resolve(T3_WEB, 'components/sidebar/SidebarChrome.tsx')) return { code: sidebarChrome(code), map: null };
      if (id === resolve(T3_WEB, 'components/sidebar/SidebarThreadUndoNotice.tsx')) return {
        code: anchorPatch('SidebarThreadUndoNotice.tsx', code, [
          ['    <SidebarMenuItem\n', '    <SidebarMenuItem data-hoh-hide-in-rail=""\n'],
        ]), map: null,
      };
      if (id === resolve(T3_WEB, 'components/sidebar/SidebarUpdatePill.tsx')) return {
        code: anchorPatch('SidebarUpdatePill.tsx', code, [
          ['<SidebarMenuItem className="ml-auto flex flex-1">', '<SidebarMenuItem className="ml-auto flex flex-1" data-hoh-hide-in-rail="">'],
        ]), map: null,
      };
      if (id === resolve(T3_WEB, 'shortcuts/chatRouteCommands.ts')) return { code: chatRouteCommands(code), map: null };
      if (id === resolve(T3_WEB, 'components/NoProjectsHero.tsx')) return { code: noProjectsHero(code), map: null };
      if (id === resolve(T3_WEB, 'components/settings/ProviderInstanceCard.tsx')) return { code: providerInstanceCard(code), map: null };
      return null;
    },
    generateBundle() {
      if (!routerStoragePatched) this.error('TanStack scroll-restoration.js was not patched; check the router dependency');
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
const APP_VERSION = JSON.parse(readFileSync(resolve(UI, '../apps/hoh-desktop/tauri.conf.json'), 'utf8')).version as string;

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
