// Скрытые области веб-клиента T3 в окне HoH (plans/hoh40-t3-base.md, «Скрыто выключателем»).
// Файлы T3 в дереве остаются без правок; область выключается одним из трёх способов:
//   host  — хост HoH (ui/src/hoh-host, u3) не сообщает её в конфиге/возможностях сервера, T3 сам её не показывает;
//   route — страница не собирается: файл маршрута подменяется при сборке (src/t3-entry/vite.ts) маршрутом
//           с тем же id, который сразу уводит на `redirect`; код страницы и её пакеты в сборку не попадают;
//   nav   — пункт настроек убирается из списка разделов (SETTINGS_NAV_GROUPS) при сборке, см. vite.ts.
// Этот файл — договор с хостом: `HIDDEN_AREAS` передаётся в `installHohHost` (src/t3-entry/main.ts).

export type HideMechanism = 'host' | 'route' | 'nav';

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
  '_chat.usage': '/',
  'connect': '/',
  'connect-agent': '/',
  'approve-agent': '/',
  'pair': '/',
  'guest': '/',
  '_chat.settings.diffs': '/settings/general',
  '_chat.settings.keybindings': '/settings/general',
  '_chat.settings.voice': '/settings/general',
  '_chat.settings.snap-shot': '/settings/general',
  '_chat.settings.threads': '/settings/general',
  '_chat.settings.browser': '/settings/general',
  '_chat.settings.devices': '/settings/general',
  '_chat.settings.scheduled-tasks': '/settings/general',
  '_chat.settings.projects': '/settings/general',
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
  '_chat.settings.archived': '/settings/general',
};

/** Разделы настроек, которые остаются (план: общие, вид, чат, провайдеры, доступ). */
export const VISIBLE_SETTINGS: readonly string[] = [
  '/settings/general',
  '/settings/appearance',
  '/settings/chat',
  '/settings/providers',
  '/settings/access',
  // Сведения о программе и лицензии T3 (MIT; страница лицензий открывается из неё).
  '/settings/about',
];

export const HIDDEN_AREAS: readonly HiddenArea[] = [
  { id: 'vcs', title: 'git/vcs, source control', how: ['host', 'route', 'nav'] },
  { id: 'pull-requests', title: 'pull requests', how: ['host', 'route'] },
  { id: 'issues', title: 'issues и трекеры', how: ['host', 'route'] },
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
  { id: 'usage', title: 'использование/телеметрия/диагностика/ресурсы', how: ['host', 'route', 'nav'] },
  { id: 'updates', title: 'обновления приложения', how: ['host'] },
  { id: 'providers-install', title: 'ACP-реестр, установка и вход провайдеров T3; в выборе модели только Claude', how: ['host'] },
  { id: 'secrets', title: 'секреты/1Password', how: ['host', 'route', 'nav'] },
  { id: 'github-star', title: 'GitHub-звезда/конфетти', how: ['host'] },
  { id: 'voice', title: 'голосовой ввод', how: ['host', 'route', 'nav'] },
  { id: 'settings', title: 'разделы настроек кроме общих, вида, чата, провайдеров, доступа', how: ['route', 'nav'] },
];
