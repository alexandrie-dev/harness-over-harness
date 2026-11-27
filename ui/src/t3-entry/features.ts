// Скрытые области веб-клиента T3 в окне HoH (plans/hoh40-t3-base.md, «Скрыто выключателем»).
// Файлы T3 в дереве остаются без правок; область выключается одним из трёх способов:
//   host  — хост HoH (ui/src/hoh-host, u3) не сообщает её в конфиге/возможностях сервера, T3 сам её не показывает;
//   route — страница не собирается: файл маршрута подменяется при сборке (src/t3-entry/vite.ts) маршрутом
//           с тем же id, который сразу уводит на `redirect`; код страницы и её пакеты в сборку не попадают;
//   nav   — пункт настроек убирается из списка разделов (SETTINGS_NAV_GROUPS) при сборке, см. vite.ts;
//   component — модуль компонента T3 подменяется при сборке: всё, что он экспортирует, остаётся, а названные
//           компоненты ничего не рисуют (HIDDEN_COMPONENTS); файл T3 не меняется;
//   css   — элемент, который T3 рисует без условия внутри общего компонента, скрывается правилом по его
//           подписи/атрибуту (HIDDEN_SELECTORS, стиль входа src/t3-entry/hidden.css собирается из этого списка);
//   palette — пункт палитры команд убирается из её групп при сборке (HIDDEN_PALETTE_ITEMS, см. vite.ts).
// Перечень `HIDDEN_AREAS` — договор с хостом (u3): области с `host` хост не сообщает в конфиге/возможностях сервера.

export type HideMechanism = 'host' | 'route' | 'nav' | 'component' | 'css' | 'palette';

export interface HiddenArea {
  readonly id: string;
  /** Как в плане. */
  readonly title: string;
  readonly how: readonly HideMechanism[];
}

/** Маршруты T3, которые не собираются: имя файла в apps/web/src/routes (без .tsx) → куда уводить. */
export const HIDDEN_ROUTES: Readonly<Record<string, string>> = {
  '_chat.issues': '/',
  '_chat.pull-requests': '/',
  'connect': '/',
  'connect-agent': '/',
  'approve-agent': '/',
  'pair': '/',
  'guest': '/',
  '_chat.settings.access': '/settings/general',
  '_chat.settings.snap-shot': '/settings/general',
  '_chat.settings.browser': '/settings/general',
  '_chat.settings.devices': '/settings/general',
  '_chat.settings.scheduled-tasks': '/settings/general',
  '_chat.settings.source-control': '/settings/general',
  '_chat.settings.worktrees': '/settings/general',
  '_chat.settings.connections': '/settings/general',
  '_chat.settings.secrets': '/settings/general',
  '_chat.settings.maintenance': '/settings/general',
  '_chat.settings.diagnostics': '/settings/general',
  '_chat.settings.labs': '/settings/general',
  '_chat.settings.beta': '/settings/general',
  '_chat.settings.integrations': '/settings/general',
  '_chat.settings.storage': '/settings/general',
};

/** Разделы настроек, которые доступны в HoH. */
export const VISIBLE_SETTINGS: readonly string[] = [
  '/settings/general',
  '/settings/appearance',
  '/settings/chat',
  '/settings/diffs',
  '/settings/keybindings',
  '/settings/voice',
  '/settings/providers',
  '/settings/threads',
  '/settings/projects',
  '/settings/archived',
  // Сведения о программе и лицензии T3 (MIT; страница лицензий открывается из неё).
  '/settings/about',
];

/** Settings pages with a subset of T3 rows. Search only links to rows rendered in HoH. */
export const VISIBLE_SETTINGS_ITEMS: Readonly<Record<string, readonly string[]>> = {
  '/settings/voice': ['voice-microphone', 'dictation-shortcut-mode'],
  '/settings/threads': ['default-permissions', 'snooze-options'],
  '/settings/projects': ['project-overview'],
};

/** Компоненты T3, которые не рисуются: файл в apps/web/src → имена экспортов (`default` — экспорт по умолчанию). */
export const HIDDEN_COMPONENTS: Readonly<Record<string, readonly string[]>> = {
  // Выбор значка среды не поддерживается движком.
  'components/settings/EnvironmentIconPicker.tsx': ['EnvironmentIconMenu'],
  // «Add project script» и кнопки скриптов проекта.
  'components/ProjectScriptsControl.tsx': ['default'],
  // Микрофон в поле ввода (голосовой ввод) и полоска записи.
  'components/chat/ComposerSpeechButton.tsx': ['ComposerSpeechButton', 'ComposerSpeechRecordingPill'],
  // T3 рисует несохранённый composer draft отдельной карточкой; Recent chats начинается только с первого сообщения.
  'components/sidebar/SidebarDraftBlock.tsx': ['SidebarDraftBlock'],
};

/** Providers показывает встроенные харнессы HoH. */
export const VISIBLE_PROVIDER_DRIVERS: readonly string[] = ['claudeAgent', 'codex', 'pi'];

/** Элементы, которые T3 рисует без условия. */
export const HIDDEN_SELECTORS: readonly string[] = [
  '[data-sidebar="menu-item"]:has(> [aria-label="Usage"])',
  '[data-sidebar="menu-item"]:has(> [aria-label="Pull requests"])',
  '[data-sidebar="menu-item"]:has(> [aria-label="Issues"])',
  // Полоски TimelineMinimap у левого края ленты выглядят как случайный знак «=» и дублируют прокрутку разговора.
  '[data-testid="timeline-minimap"]',
];

/** Пункты палитры команд T3 (components/CommandPalette.tsx), которые не показываются: образец значения пункта. Все
 * показанные группы палитры (корень, поиск, подменю «Add project») проходят через filterCommandPaletteGroups
 * (components/CommandPalette.logic.ts); при сборке её вывод отдаётся без этих пунктов (vite.ts), файл T3 не меняется. */
export const HIDDEN_PALETTE_ITEMS: readonly string[] = [
  // «Git URL» в «Add project» (клон по адресу): git у HoH нет.
  '^action:add-project:.+:url$',
];

export const HIDDEN_AREAS: readonly HiddenArea[] = [
  { id: 'vcs', title: 'git/vcs, source control, «Initialize Git», «Git URL» в «Add project»', how: ['host', 'route', 'nav', 'component', 'palette'] },
  { id: 'pull-requests', title: 'pull requests', how: ['host', 'route', 'css'] },
  { id: 'issues', title: 'issues и трекеры', how: ['host', 'route', 'css'] },
  { id: 'review', title: 'review', how: ['host'] },
  { id: 'worktrees', title: 'worktree, клон проекта', how: ['host', 'route', 'nav'] },
  { id: 'terminal', title: 'терминал', how: ['host'] },
  { id: 'preview', title: 'превью и встроенный браузер', how: ['host', 'route', 'nav'] },
  { id: 'cloud', title: 'облачный вход Clerk, устройства и пары, peer links, guest share, tailscale/ssh', how: ['host', 'route', 'nav'] },
  { id: 'scheduled', title: 'запланированные задачи', how: ['host', 'route', 'nav'] },
  { id: 'mcp-apps', title: 'MCP Apps', how: ['host'] },
  { id: 'import', title: 'импорт сессий агентов', how: ['host'] },
  { id: 'attachments', title: 'вложения (загрузка)', how: ['host'] },
  { id: 'diff', title: 'панель изменений (diff)', how: ['host', 'route', 'nav'] },
  { id: 'office', title: 'офисные форматы', how: ['host'] },
  { id: 'diagnostics', title: 'телеметрия, диагностика процессов и ресурсы', how: ['host', 'route', 'nav', 'css'] },
  { id: 'updates', title: 'обновления приложения через хост T3', how: ['host'] },
  { id: 'providers-install', title: 'ACP-реестр и управляемая установка поставщиков моделей T3', how: ['host'] },
  { id: 'secrets', title: 'настройки внешних хранилищ секретов, включая 1Password', how: ['host', 'route', 'nav'] },
  { id: 'github-star', title: 'GitHub-звезда/конфетти', how: ['host'] },
  { id: 'voice', title: 'голосовой ввод', how: ['host', 'route', 'nav', 'component'] },
  { id: 'project-scripts', title: 'скрипты проекта («Add project script»)', how: ['component'] },
  { id: 'other-providers', title: 'Сторонние поставщики, «Add provider», Runtime, Environment, Usage providers и Advanced', how: ['component'] },
  { id: 'theme-names', title: 'имена тем «T3 Code» и «T3 Chat» в Appearance → Themes → «Default» и «Chat»', how: ['component'] },
  { id: 'settings', title: 'разделы настроек вне VISIBLE_SETTINGS', how: ['route', 'nav'] },
];
