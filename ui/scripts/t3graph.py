"""Граф импортов веб-клиента T3 Code (MIT, только чтение) для HOH-40: что переносится, что ставится, что заглушается.

Общий модуль для t3-deps.py (список пакетов) и t3-sync.py (перенос). Сам ничего не пишет.
Корни переноса: apps/web/src целиком (кроме тестов) и нужные файлы packages/{contracts,shared,client-runtime}/src.
Видимая часть — замыкание импортов от apps/web/src/main.tsx, в которое не входят файлы скрытых частей (HIDDEN).
Пакеты видимой части ставятся; пакеты, которые импортируют только скрытые части, и пакеты T3, которые не берутся
(провайдеры, source-control, секреты, трекеры), заменяются заглушками через псевдонимы.
"""
import json, os, re, subprocess

T3_DEFAULT = '/mnt/toshiba/alexandrie/third-party/t3code'
ENTRY = 'apps/web/src/main.tsx'
# Папка T3 -> папка в ui/src/t3/base (имена как у прежнего переноса: web, shared, client-runtime; плюс contracts).
ROOTS = {'web': 'apps/web/src', 'contracts': 'packages/contracts/src', 'shared': 'packages/shared/src', 'client-runtime': 'packages/client-runtime/src'}
TAKEN_PKGS = {'@t3tools/contracts': 'packages/contracts', '@t3tools/shared': 'packages/shared', '@t3tools/client-runtime': 'packages/client-runtime'}
CODE = ('.ts', '.tsx', '.mts', '.js', '.mjs')
# Тесты и замеры T3 не переносятся.
TEST = re.compile(r'\.test\.|\.bench\.|/testing/')

# Скрытые части (plans/hoh40-t3-base.md, «Скрыто выключателем»). Файлы остаются в дереве; их пакеты не ставятся.
HIDDEN = [
    ('git и source control, запросы на слияние, задачи и трекеры, review, worktree, клон проекта', [
        r'apps/web/src/components/(GitActionsControl|BranchToolbar|BranchPicker|PullRequestThreadDialog|ProjectCloneDialog|CloneProject|Worktree|SourceControl|ReviewGuide|Review)[^/]*$',
        r'apps/web/src/components/(pullRequest|issue)/', r'apps/web/src/routes/_chat\.(pull-requests|issues)\.tsx$',
        r'apps/web/src/(sourceControlPresentation|worktreeCleanup|reviewCommentContext)\.', r'apps/web/src/lib/open(PullRequest|Issue)Link\.',
        r'apps/web/src/hooks/use(PullRequest|Issue)[^/]*$', ]),
    ('терминал', [r'apps/web/src/terminal/', r'apps/web/src/components/terminal/', r'apps/web/src/components/[^/]*Terminal[^/]*$', r'apps/web/src/terminal[A-Z][^/]*$']),
    ('превью и встроенный браузер', [r'apps/web/src/browser/', r'apps/web/src/components/preview/', r'apps/web/src/(preview[A-Z]|browser[A-Z]|portDiscovery)[^/]*$']),
    ('облачный вход Clerk, устройства и пары, peer links, guest share, tailscale/ssh', [
        r'apps/web/src/cloud/', r'apps/web/src/components/(clerk|cloud|device|guest|auth)/', r'apps/web/src/(guestPage|hostedPairing)\.',
        r'apps/web/src/components/(Hosted|ConnectDeviceApproval)[^/]*$', r'apps/web/src/components/chat/Guest[^/]*$', r'apps/web/src/components/desktop/SshPasswordPrompt[^/]*$',
        r'apps/web/src/routes/(connect|connect-agent|approve-agent|pair|guest)\.tsx$',
        r'packages/client-runtime/src/device/', ]),
    ('запланированные задачи и отложенные разговоры (snooze)', [r'apps/web/src/components/[^/]*Scheduled[^/]*$', r'apps/web/src/components/settings/[^/]*Scheduled[^/]*$',
        r'apps/web/src/components/(CustomSnoozeDialog|Sidebar\.snooze)[^/]*$', r'apps/web/src/components/settings/SnoozePresets[^/]*$']),
    ('MCP Apps', [r'apps/web/src/components/mcp/', r'apps/web/src/components/chat/McpApp[^/]*$']),
    ('панель изменений (diff)', [r'apps/web/src/components/(DiffPanel|DiffWorkerPoolProvider)[^/]*$', r'apps/web/src/components/diffs/', r'apps/web/src/(diffPanelStore|diffFileActions)\.']),
    ('офисные и двоичные форматы (docx/xlsx/pptx, sqlite, tiff/heic)', [
        r'apps/web/src/components/files/office/', r'apps/web/src/components/files/(officeArchive|extendedDocumentPreview|binaryPreview\.worker|imageDecode\.worker)\.',
        r'apps/web/src/components/files/(BinaryViewer|BinaryFilePreview|binaryPreviewProtocol|readBinaryPreview)[^/]*$', ]),
    ('вложения (загрузка) и снимки экрана (snap-shot)', [r'apps/web/src/lib/imageCompression\.', r'apps/web/src/components/files/AttachmentFilePreview[^/]*$',
        r'apps/web/src/components/chat/(ComposerAttachmentStrip|SnapShot)[^/]*$', r'apps/web/src/components/desktop/SnapShot[^/]*$']),
    ('использование, телеметрия, диагностика, ресурсы', [r'apps/web/src/components/usage/', r'apps/web/src/routes/_chat\.usage\.tsx$', r'apps/web/src/observability/']),
    ('импорт и экспорт тем VS Code / Open VSX', [r'apps/web/src/(openVsxThemes|vscodeThemeImport)\.']),
    ('секреты и 1Password', [r'apps/web/src/components/settings/Secret[^/]*$']),
    ('GitHub-звезда и конфетти', [r'apps/web/src/components/GitHubStar[^/]*$']),
    ('голосовой ввод', [r'apps/web/src/speech/', r'apps/web/src/components/settings/Microphone[^/]*$']),
    ('разделы настроек, кроме общих, вида, чата, провайдеров и доступа', [
        r'apps/web/src/routes/_chat\.settings\.(about|archived|beta|browser|connections|devices|diagnostics|diffs|integrations|keybindings|labs|maintenance|open-source-licenses|projects|scheduled-tasks|secrets|snap-shot|source-control|storage|threads|voice|worktrees)\.tsx$', ]),
]
# Пакеты T3, которые не берутся (план: файлы данных T3 — провайдеры, source-control, секреты, трекеры, ssh, tailscale).
NOT_TAKEN_WS = {
    'provider': 'провайдеры T3 (установка, вход, ACP-реестр)', 'source-control': 'git и source control',
    'secret-provider': 'секреты и 1Password', 'issue-tracker': 'задачи и трекеры', 'tailscale': 'tailscale/ssh', 'ssh': 'tailscale/ssh',
    'effect-acp': 'провайдеры T3 (установка, вход, ACP-реестр)', 'effect-codex-app-server': 'провайдеры T3 (установка, вход, ACP-реестр)',
}
# Пакеты скрытых частей, которые импортируют и видимые файлы: не ставятся, заглушка должна давать «части нет»
# (вход Clerk: «вход не выполнен»). Список видимых файлов-импортёров пишется в T3-DEPS.md.
FORCE_STUB = {'@clerk/react': 'облачный вход Clerk, устройства и пары, peer links, guest share, tailscale/ssh',
              '@clerk/electron': 'облачный вход Clerk, устройства и пары, peer links, guest share, tailscale/ssh',
              '@clerk/clerk-js': 'облачный вход Clerk, устройства и пары, peer links, guest share, tailscale/ssh',
              'vite-plus': 'сборка T3 (Vite+): окно собирает обычным vite, типы `vite-plus/client` дают `vite/client`'}
# Импорты типов, которые дают пакеты @types/*.
TYPES_ONLY = {'hast': '@types/hast', 'mdast': '@types/mdast', 'unist': '@types/unist'}

# Неявные зависимости: глобальные объявления, которые TypeScript применяет ко всей программе, а не через импорт.
# (шаблон в тексте файла, файл T3 с объявлением)
ROUTER = re.compile(r'from\s+[\'"]@tanstack/react-router[\'"]')
IMPLICIT = [
    (re.compile(r'\b(desktopBridge|t3dev)\b'), 'apps/web/src/vite-env.d.ts'),                       # declare global { interface Window }
    (re.compile(r'from\s+[\'"]mdast[\'"]'), 'packages/shared/src/markdownMath.ts'),              # /// <reference types="mdast-util-to-hast" />
    (re.compile(r'from\s+[\'"]effect/Schema[\'"]'), 'packages/contracts/src/settings.ts'),       # declare module "effect/Schema" (Annotations)
    (re.compile(r'<webview\b|querySelector\(\s*[\'"]webview'), 'apps/web/src/browser/HostedBrowserWebview.tsx'),  # HTMLElementTagNameMap
    # Маршрутизатор: типы маршрутов (routeTree.gen.ts), Register (router.ts) и HistoryState — на всю программу.
    (ROUTER, 'apps/web/src/routeTree.gen.ts'), (ROUTER, 'apps/web/src/router.ts'),
    (ROUTER, 'apps/web/src/components/settings/settingsLayout.tsx'), (ROUTER, 'apps/web/src/lib/assistantCitationNavigation.ts'),
]
IMPORT = re.compile(r'''(?:^|[\s;}])(import|export)\s+(type\s+)?(?:[\w*{}\s,$]+?\s+from\s+)?['"]([^'"\n]+)['"]|import\(\s*['"]([^'"\n]+)['"]\s*\)|new\s+URL\(\s*['"](\.[^'"\n]+)['"]\s*,\s*import\.meta\.url''', re.M)
REFERENCE = re.compile(r'^///\s*<reference\s+types="([^"]+)"', re.M)
LINE_COMMENT = re.compile(r'^\s*//.*$', re.M)
BLOCK_COMMENT = re.compile(r'/\*.*?\*/', re.S)


def importers(lock_path):
    """{importer: {пакет: версия с суффиксом peer}} из последнего раздела importers: pnpm-lock.yaml."""
    lines = open(lock_path).read().split('\n')
    start = [n for n, l in enumerate(lines) if l == 'importers:'][-1]
    out, cur, name = {}, None, None
    for l in lines[start + 1:]:
        if l and not l.startswith(' '): break
        m = re.match(r'^  (\S[^:]*):$', l)
        if m: cur = out.setdefault(m.group(1), {}); continue
        m = re.match(r"^      '?([^']+?)'?:$", l)
        if m: name = m.group(1); continue
        m = re.match(r'^        version: (\S+)', l)
        if m and cur is not None and name: cur[name] = m.group(1)
    return out


IMPORTER_ORDER = ['apps/web', 'packages/shared', 'packages/client-runtime', 'packages/contracts']


def t3_versions(t3=T3_DEFAULT):
    """{пакет: версия из lock T3} для прямых зависимостей apps/web и перенесённых packages (первый по IMPORTER_ORDER)."""
    imp, out = importers(os.path.join(t3, 'pnpm-lock.yaml')), {}
    for i in reversed(IMPORTER_ORDER):
        out.update({k: v.split('(')[0] for k, v in imp.get(i, {}).items()})
    return out


def hidden_part(path):
    for name, patterns in HIDDEN:
        if any(re.search(p, path) for p in patterns): return name
    return None


def root_pkg(spec):
    parts = spec.split('/')
    return '/'.join(parts[:2]) if spec.startswith('@') else parts[0]


class Graph:
    def __init__(self, t3=T3_DEFAULT):
        self.t3 = t3
        listed = subprocess.run(['git', '-C', t3, 'ls-files', *ROOTS.values()], capture_output=True, text=True, check=True).stdout.split('\n')
        self.all = sorted(f for f in listed if f and not TEST.search(f))
        self.set = set(self.all)
        self.exports = {}
        for name, pdir in TAKEN_PKGS.items():
            for key, value in json.load(open(os.path.join(t3, pdir, 'package.json')))['exports'].items():
                target = value if isinstance(value, str) else (value.get('types') or value.get('import') or value.get('default'))
                self.exports[name + key[1:]] = os.path.normpath(os.path.join(pdir, target))
        self.edges = {f: self._edges(f) for f in self.all if f.endswith(CODE)}

    def _file(self, base):
        cands = [base]
        if base.endswith(('.js', '.mjs')): cands += [re.sub(r'\.m?js$', e, base) for e in ('.ts', '.tsx', '.mts')]
        cands += [base + e for e in ('.ts', '.tsx', '.js', '.mjs', '.json')] + [base + '/index' + e for e in ('.ts', '.tsx')]
        return next((c for c in cands if c in self.set), None)

    def resolve(self, importer, spec):
        """('file', путь T3) | ('npm', пакет) | ('ws', пакет T3, который не берётся) | ('missing', spec)."""
        bare = spec.split('?')[0]
        if bare.startswith('.'):
            found = self._file(os.path.normpath(os.path.join(os.path.dirname(importer), bare)))
        elif bare.startswith('~/'):
            found = self._file('apps/web/src/' + bare[2:])
        elif bare.startswith('@t3tools/'):
            pkg = root_pkg(bare)
            if pkg not in TAKEN_PKGS: return ('ws', pkg)
            found = self.exports.get(bare)
        else:
            if bare.startswith('node:'): return ('npm', bare)
            name = root_pkg(bare)
            return ('npm', TYPES_ONLY.get(name, name))
        return ('file', found) if found else ('missing', spec)

    def _edges(self, path):
        text = open(os.path.join(self.t3, path), encoding='utf-8', errors='replace').read()
        out = [(spec, 'type', self.resolve(path, spec)) for spec in REFERENCE.findall(text)]
        out += [(target, 'ambient', ('file', target)) for pattern, target in IMPLICIT
                if target != path and target in self.set and pattern.search(text)]
        text = LINE_COMMENT.sub('', BLOCK_COMMENT.sub('', text))
        for m in IMPORT.finditer(text):
            spec = m.group(3) or m.group(4) or m.group(5)
            kind = 'type' if m.group(2) else 'dynamic' if m.group(4) else 'static'
            out.append((spec, kind, self.resolve(path, spec)))
        return out

    def closure(self, starts, cut=lambda path: False):
        """Файлы, достижимые от starts; в файлы, для которых cut() истинно, обход не заходит (они попадают в reached_cut)."""
        seen, reached_cut, stack = set(), set(), list(starts)
        while stack:
            f = stack.pop()
            if f in seen or f in reached_cut: continue
            if f not in starts and cut(f): reached_cut.add(f); continue
            seen.add(f)
            for _, _, target in self.edges.get(f, ()):
                if target[0] == 'file': stack.append(target[1])
        return seen, reached_cut

    def packages(self, files):
        """{пакет: {файл: {виды импорта}}} для npm-пакетов, которые импортируют files."""
        used = {}
        for f in files:
            for _, kind, target in self.edges.get(f, ()):
                if target[0] == 'npm': used.setdefault(target[1], {}).setdefault(f, set()).add(kind)
        return used

    def workspace(self, files):
        used = {}
        for f in files:
            for spec, _, target in self.edges.get(f, ()):
                if target[0] == 'ws': used.setdefault(target[1], {}).setdefault(f, set()).add(spec)
        return used

    def missing(self):
        return sorted((f, spec) for f, es in self.edges.items() for spec, _, t in es if t[0] == 'missing')


def ws_reason(pkg):
    name = pkg.split('/', 1)[1]
    return next((why for prefix, why in NOT_TAKEN_WS.items() if name == prefix or name.startswith(prefix + '-')), 'пакет T3, который не берётся')


def analyse(t3=T3_DEFAULT):
    """Видимое замыкание, файлы к переносу, пакеты к установке и к заглушке."""
    g = Graph(t3)
    visible, cut_reached = g.closure([ENTRY], cut=lambda f: hidden_part(f) is not None)
    web = [f for f in g.all if f.startswith('apps/web/src/')]
    # Переносится весь apps/web/src и всё из packages/*, до чего дотягиваются его файлы (скрытые тоже: они в дереве).
    transfer, _ = g.closure([f for f in web if f.endswith(CODE)])
    transfer = sorted(set(transfer) | set(web))
    install = {p: fs for p, fs in g.packages(visible).items() if p not in FORCE_STUB}
    everything = g.packages(transfer)
    stubbed = {p: fs for p, fs in everything.items() if p not in install and not p.startswith('node:')}
    ws = g.workspace(transfer)
    return {'graph': g, 'visible': visible, 'cut_reached': cut_reached, 'transfer': transfer, 'install': install, 'stubbed': stubbed, 'ws': ws}
