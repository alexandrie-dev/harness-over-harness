// Снимки входа окна на веб-клиенте T3 (HOH-40): экраны пути основателя при 1440×900 и 800×550, headless Chrome,
// без сервера и сети. Страница — собранная dist (vite build && vite build --mode t3), отдаётся из файлов через
// page.route на выдуманном адресе (роутеру T3 нужен путь «/», а не file://). Движок — подделка tests/chat/fake-engine.mjs
// за хостом HoH (ui/src/hoh-host): вход берёт её из window.__HOH_T3_TEST_ENGINE__ только вне Tauri (src/t3-entry/main.ts).
//   node scripts/snapshot-t3.mjs [папка-снимков]      (по умолчанию test-results/t3-entry)
// Экраны: первый запуск, пустой разговор, разговор с ответом и шагом, запрос разрешения, меню модели, меню режима,
// режим в начатом разговоре, «Add project», настройки.
import { chromium } from 'playwright-core';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, extname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { FakeEngine } from '../tests/chat/fake-engine.mjs';

const UI = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const DIST = process.env.HOH_T3_DIST ? resolve(process.env.HOH_T3_DIST) : join(UI, 'dist');
const OUT = resolve(process.argv[2] ?? join(UI, 'test-results/t3-entry'));
const ORIGIN = 'http://hoh-window.test';
const SIZES = [{ width: 1440, height: 900 }, { width: 1920, height: 1080 }, { width: 800, height: 550 }];
const GROUPING_SIZES = [{ width: 1440, height: 900 }, { width: 1920, height: 1080 }, { width: 800, height: 550 }];
const TYPES = { '.js': 'text/javascript', '.mjs': 'text/javascript', '.css': 'text/css', '.html': 'text/html', '.png': 'image/png',
  '.svg': 'image/svg+xml', '.woff2': 'font/woff2', '.woff': 'font/woff', '.ttf': 'font/ttf', '.json': 'application/json', '.wasm': 'application/wasm' };
const CLAUDE = { installed: true, version: '2.1.0', auth: 'authenticated',
  models: [{ id: 'opus', displayName: 'Claude Opus', isDefault: true }, { id: 'sonnet', displayName: 'Claude Sonnet' }] };

/** Движок с Claude; `chats` — разговоры с ответом и шагом инструмента (как их пишет движок HoH). */
function engineWith(chats) {
  const engine = new FakeEngine();
  engine.access = 'confirm';
  engine.claude = CLAUDE;
  for (const { id, title, user, command, output, answer } of chats) {
    const session = engine.open(id, title);
    engine.append(session, 'user/message', { id: `${id}-u`, source: { kind: 'user' }, content: [{ type: 'text', text: user }] });
    engine.append(session, 'turn/start', { turn: 1 });
    engine.append(session, 'tool/call', { callId: 'c1', turn: 1, name: 'Bash', arguments: { command } });
    engine.append(session, 'tool/result', { turn: 1, message: { toolCallId: 'c1', content: [{ type: 'text', text: output }] } });
    engine.append(session, 'assistant/message', { message: { id: `${id}-a`, content: [{ type: 'text', text: answer } ] } });
    engine.append(session, 'turn/end', { turn: 1, reason: { kind: 'completed' } });
  }
  return engine;
}

function groupingEngine() {
  const engine = new FakeEngine();
  engine.access = 'confirm';
  engine.claude = CLAUDE;
  const turn = (id, title, done = false) => {
    const session = engine.open(id, title);
    engine.append(session, 'user/message', { id: `${id}-u`, source: { kind: 'user' }, content: [{ type: 'text', text: `Work on ${title}` }] });
    engine.append(session, 'turn/start', { turn: 1 });
    engine.append(session, 'tool/call', { callId: `${id}-call`, turn: 1, name: 'Bash', arguments: { command: `check ${id}` } });
    if (done) {
      engine.append(session, 'tool/result', { turn: 1, message: { toolCallId: `${id}-call`, content: [{ type: 'text', text: 'done' }] } });
      engine.append(session, 'assistant/message', { message: { id: `${id}-a`, content: [{ type: 'text', text: `${title} is ready` }] } });
      engine.append(session, 'turn/end', { turn: 1, reason: { kind: 'completed' } });
    }
  };
  turn('alpha-live', 'Prepare release notes');
  turn('alpha-review', 'Review the API boundary');
  turn('alpha-done', 'Update onboarding copy', true);
  turn('beta-wait', 'Clean generated artifacts');
  turn('scratch-live', 'Plan the next milestone');
  return engine;
}

function groupingStorage(grouped = true) {
  const at = Date.now() - 60_000;
  return {
    't3code:theme': 'hoh',
    't3code:theme-appearance-mode': 'dark',
    'hoh-home.group-agents-by-project.v1': String(grouped),
    'hoh-host.projects.v1': JSON.stringify({
      version: 1,
      defaultFolder: '/work/no-project',
      projects: [
        { id: 'project-alpha', title: 'Alexandrie', workspaceRoot: '/work/alexandrie', createdAt: at, updatedAt: at, pinnedAt: null, archivedAt: null },
        { id: 'project-beta', title: 'Harness over Harness', workspaceRoot: '/work/hoh', createdAt: at, updatedAt: at, pinnedAt: null, archivedAt: null },
      ],
      threads: { 'alpha-live': 'project-alpha', 'alpha-review': 'project-alpha', 'alpha-done': 'project-alpha', 'beta-wait': 'project-beta', 'scratch-live': 'hoh-project' },
      homeDispositions: { 'alpha-done': { turnOrdinal: 1, state: 'done' } },
    }),
  };
}

const CHAT = { id: 'chat-1', title: 'Files in the folder', user: 'What files are in this folder?', command: 'ls',
  output: 'notes.txt\nreport.txt\nscript.py\n[Command finished with exit code 0]',
  answer: 'The folder has three files:\n\n- `notes.txt`\n- `report.txt`\n- `script.py`' };

async function open(browser, engine, size, path = '/', options = {}) {
  const context = await browser.newContext({ viewport: size, deviceScaleFactor: 1, locale: 'en-US', reducedMotion: options.reducedMotion ?? 'reduce', colorScheme: options.colorScheme ?? 'dark' });
  const page = await context.newPage();
  page.setDefaultTimeout(15000);
  const problems = [];
  page.on('pageerror', error => problems.push('pageerror: ' + error.message));
  page.on('console', message => { if (message.type() === 'error') problems.push('console: ' + message.text().slice(0, 300)); });
  await page.route('**/*', async route => {
    const url = new URL(route.request().url());
    if (url.origin !== ORIGIN) { problems.push('remote: ' + url.href); return route.abort(); }
    // Пути роутера T3 (без расширения) отдают страницу, как Tauri отдаёт index.html.
    const file = extname(url.pathname) ? url.pathname : '/index.html';
    try { return await route.fulfill({ body: await readFile(join(DIST, file)), contentType: TYPES[extname(file)] ?? 'application/octet-stream' }); }
    catch { problems.push('missing: ' + url.pathname); return route.fulfill({ status: 404, body: '' }); }
  });
  await page.exposeBinding('__hohT3Request', (_, method, params) =>
    engine.request(method, params).catch(error => ({ __hohError: String(error?.message ?? error) })));
  await page.addInitScript(({ storage }) => {
    for (const [key, value] of Object.entries(storage)) window.localStorage.setItem(key, value);
    const listeners = new Set();
    window.__hohT3Notify = notification => { for (const listener of listeners) listener(notification); };
    window.__hohT3Decisions = [];
    window.__HOH_T3_TEST_ENGINE__ = {
      request: async (method, params) => {
        const result = await window.__hohT3Request(method, params);
        if (result && result.__hohError) throw new Error(result.__hohError);
        return result;
      },
      subscribe: async listener => { listeners.add(listener); return () => listeners.delete(listener); },
      subscribeStatus: async listener => { setTimeout(() => listener({ state: 'ready' }), 0); return () => {}; },
      subscribeQuestions: async () => () => {},
      answerQuestion: async () => {},
      decideApproval: async (approvalId, decision) => { window.__hohT3Decisions.push({ approvalId, decision }); },
    };
  }, { storage: options.storage ?? {} });
  const off = engine.subscribe(notification => { page.evaluate(value => window.__hohT3Notify?.(value), notification).catch(() => {}); });
  await page.goto(ORIGIN + path);
  return { context, page, problems, off };
}

// Размеры ключевых элементов (те же признаки, что в образце T3: qa/t3-reference-20261122-1905/styles.js).
function metrics() {
  const box = (el) => {
    if (!el) return null;
    const r = el.getBoundingClientRect(); const c = getComputedStyle(el);
    return { rect: [Math.round(r.x), Math.round(r.y), Math.round(r.width), Math.round(r.height)], centerY: Math.round(r.y + r.height / 2),
      fontSize: c.fontSize, color: c.color, background: c.backgroundColor };
  };
  const q = (s) => document.querySelector(s);
  const byText = (sel, t) => [...document.querySelectorAll(sel)].find((x) => x.innerText && x.innerText.trim() === t);
  const root = getComputedStyle(document.documentElement);
  return {
    root: { fontSize: root.fontSize, className: document.documentElement.className, topbar: root.getPropertyValue('--workspace-topbar-height').trim() },
    sidebar: box(q('[data-slot=sidebar-inner]') || q('[data-sidebar=sidebar]')),
    sidebarToggle: box(q('[data-sidebar=trigger]') || q('button[aria-label="Toggle Sidebar"]')),
    newThread: box(q('button[aria-label="New thread"]')),
    search: box(byText('button', 'Search') || q('[aria-label="Search threads"]')),
    headerTitle: box(byText('span,div,p,h1,h2', 'New thread')),
    heading: box(q('h1')),
    composer: box(q('[role=textbox],textarea,[contenteditable=true]')),
    send: box(q('button[aria-label*="Send"],button[aria-label*="send"]')),
    permissions: box(q('button[aria-label^="Permissions"]') || byText('button', 'Supervised')),
  };
}

async function settle(page) {
  await page.evaluate(() => document.fonts.ready);
  await page.waitForTimeout(600);
  await page.mouse.move(0, 0);
}

async function openQuickView(page) {
  const trigger = page.getByRole('button', { name: `Open ${CHAT.title}` }).first();
  await trigger.waitFor();
  const homePath = new URL(page.url()).pathname;
  await trigger.click();
  const dialog = page.getByRole('dialog').last();
  await dialog.waitFor();
  await dialog.getByRole('heading', { name: CHAT.title }).waitFor();
  await dialog.getByText('The folder has three files').first().waitFor();
  await dialog.getByRole('textbox').waitFor();
  return { dialog, trigger, homePath };
}

function deferredReplyEngine() {
  const engine = engineWith([CHAT]);
  engine.answer = (session, messageId, text) => {
    const send = (type, data) => engine.emit('session.event', { sessionId: session.id, event: engine.append(session, type, data) });
    engine.emit('session.status', { sessionId: session.id, status: 'running' });
    send('user/message', { id: messageId, source: { kind: 'user' }, content: [{ type: 'text', text }] });
    send('turn/start', { turn: 2 });
  };
  return engine;
}

async function readyGroupedHome(page, engine) {
  await page.locator('.hoh-project').filter({ hasText: 'Harness over Harness' }).first().waitFor();
  for (const sessionId of ['alpha-live', 'alpha-review', 'scratch-live']) engine.emit('session.status', { sessionId, status: 'running' });
  engine.emit('approval.pending', { sessionId: 'beta-wait', approvalId: 'grouping-approval', risk: 'high', humanText: 'Delete generated target files?', expiresAt: Date.now() + 300_000 });
  await page.waitForFunction(() => document.querySelectorAll('.hoh-project[data-waiting]').length === 1);
  await page.evaluate(() => document.fonts.ready);
}

const screens = [
  {
    name: 'first-run', engine: () => engineWith([]),
    // Хост u3 (fix1): движок без разговоров → T3 сам открывает /welcome; хост до fix1 — сразу поле ввода.
    run: async page => { await page.getByText(/^Welcome to /).or(page.getByRole('textbox')).first().waitFor(); },
  },
  {
    name: 'empty-conversation', engine: () => engineWith([CHAT]),
    run: async page => { await page.getByRole('textbox').first().waitFor(); },
  },
  {
    name: 'conversation', engine: () => engineWith([CHAT]),
    run: async page => {
      await page.getByText(CHAT.title).first().click();
      await page.getByText('The folder has three files').first().waitFor();
    },
  },
  {
    name: 'quick-view', engine: () => engineWith([CHAT]),
    run: async (page, _engine, size) => {
      let { dialog, trigger, homePath } = await openQuickView(page);
      const box = await dialog.boundingBox();
      if (!box) throw new Error('quick-view dialog has no geometry');
      if (size.width >= 1440 && (box.width < 719 || box.width > 881)) throw new Error(`desktop quick-view width is ${box.width}px`);
      if (size.width === 800 && (box.width < 760 || box.height < 500)) throw new Error(`compact quick-view is only ${box.width}×${box.height}px`);
      const composer = await dialog.locator('[data-chat-composer-overlay="true"]').boundingBox();
      if (!composer || composer.y < box.y + box.height / 2 || composer.y + composer.height > box.y + box.height + 1)
        throw new Error(`composer is not pinned inside quick view: ${JSON.stringify({ box, composer })}`);

      await page.keyboard.press('Escape');
      await dialog.waitFor({ state: 'hidden' });
      const focusedLabel = await page.evaluate(() => document.activeElement?.getAttribute('aria-label'));
      if (focusedLabel !== `Open ${CHAT.title}`) throw new Error(`Escape returned focus to ${focusedLabel ?? 'nothing'}`);
      await trigger.press('Enter');
      dialog = page.getByRole('dialog').last();
      await dialog.waitFor();

      if (size.width === 1440) {
        await dialog.getByRole('button', { name: `Open ${CHAT.title} in full conversation` }).click();
        await page.waitForTimeout(500);
        if (!new URL(page.url()).pathname.endsWith(`/hoh-local/${CHAT.id}`)) throw new Error(`full transition opened ${page.url()}`);
        await page.goBack();
        await page.waitForFunction((path) => location.pathname === path, homePath);
        ({ dialog, trigger, homePath } = await openQuickView(page));
      }
    },
  },
  {
    name: 'quick-view-after-reply', engine: deferredReplyEngine,
    run: async page => {
      const { dialog, homePath } = await openQuickView(page);
      const editor = dialog.getByRole('textbox');
      await editor.fill('Continue from quick view');
      await editor.press('Enter');
      await dialog.getByText('Continue from quick view').waitFor();
      await page.locator(`.hoh-card[data-state="working"]`).first().waitFor();
      if (new URL(page.url()).pathname !== homePath) throw new Error(`quick reply navigated to ${page.url()}`);
    },
  },
  {
    name: 'quick-view-approval', engine: () => engineWith([CHAT]),
    run: async (page, engine) => {
      const { dialog } = await openQuickView(page);
      engine.emit('approval.pending', { sessionId: CHAT.id, approvalId: 'quick-approval-1', risk: 'high',
        humanText: 'Run the quick-view verification command?', expiresAt: Date.now() + 300_000 });
      await dialog.getByRole('button', { name: 'Approve' }).first().waitFor();
    },
  },
  {
    name: 'quick-view-approval-action', engine: () => engineWith([CHAT]),
    run: async (page, engine) => {
      const { dialog } = await openQuickView(page);
      engine.emit('approval.pending', { sessionId: CHAT.id, approvalId: 'quick-approval-1', risk: 'high',
        humanText: 'Run the quick-view verification command?', expiresAt: Date.now() + 300_000 });
      const approve = dialog.getByRole('button', { name: 'Approve' }).first();
      await approve.waitFor();
      await approve.click();
      await page.waitForFunction(() => window.__hohT3Decisions.some(entry => entry.approvalId === 'quick-approval-1'));
      await approve.waitFor({ state: 'hidden' });
    },
  },
  {
    name: 'permission-request', engine: () => engineWith([CHAT]),
    run: async (page, engine) => {
      await page.getByText(CHAT.title).first().click();
      await page.getByText('The folder has three files').first().waitFor();
      engine.emit('approval.pending', { sessionId: CHAT.id, approvalId: 'approval-1', risk: 'high',
        humanText: 'Run `rm -rf ./build`?', expiresAt: Date.now() + 300_000 });
      await page.getByRole('button', { name: 'Approve' }).first().waitFor();
    },
  },
  {
    name: 'model-menu', engine: () => engineWith([CHAT]),
    run: async page => {
      await page.getByRole('textbox').first().waitFor();
      await page.getByRole('button', { name: /Claude/ }).last().click();
      await page.getByRole('menu').or(page.getByRole('listbox')).or(page.getByRole('dialog')).first().waitFor();
    },
  },
  {
    name: 'mode-menu', engine: () => engineWith([CHAT]),
    run: async page => {
      await page.getByRole('textbox').first().waitFor();
      await page.getByRole('button', { name: /access|permission|разрешени|доступ/i }).last().click();
      await page.getByRole('menu').or(page.getByRole('listbox')).or(page.getByRole('dialog')).first().waitFor();
    },
  },
  {
    // Режим в начатом разговоре: строки с чипом под полем ввода у T3 нет (она остаётся только у git-проектов,
    // BranchToolbar.logic.ts shouldShowComposerContextStrip) — режим в меню модели, «Permissions: …» внизу.
    name: 'conversation-mode', engine: () => engineWith([CHAT]),
    run: async page => {
      await page.getByText(CHAT.title).first().click();
      await page.getByText('The folder has three files').first().waitFor();
      await page.getByRole('button', { name: /Claude|Opus/ }).last().click();
      await page.locator('button[aria-label^="Permissions"]').first().click();
      await page.getByText('Permissions', { exact: true }).first().waitFor();
    },
  },
  {
    // «Add project»: источник — только папка на диске; «Git URL» (клон) скрыт (features.ts, HIDDEN_PALETTE_ITEMS).
    name: 'add-project', engine: () => engineWith([]),
    run: async page => {
      await page.getByRole('button', { name: 'Start', exact: true }).click();
      await page.getByRole('button', { name: 'Add project' }).first().click();
      await page.getByText('Local folder').first().waitFor();
      if (await page.getByText('Git URL').count()) throw new Error('«Git URL» виден в «Add project»');
    },
  },
  {
    name: 'settings', engine: () => engineWith([CHAT]), path: '/settings/general',
    run: async page => { await page.getByRole('navigation').first().waitFor(); },
  },
];

const selectedScreens = process.env.HOH_T3_SCREEN
  ? screens.filter(screen => screen.name === process.env.HOH_T3_SCREEN)
  : screens;
if (selectedScreens.length === 0) throw new Error(`Unknown HOH_T3_SCREEN: ${process.env.HOH_T3_SCREEN}`);

await mkdir(OUT, { recursive: true });
const home = await mkdtemp(join(process.env.TMPDIR ?? tmpdir(), 'hoh-t3-chromium-'));
const report = [];
let browser;
try {
  browser = await chromium.launch({
    executablePath: process.env.HOH_CHROMIUM ?? '/usr/bin/google-chrome', headless: true,
    env: { ...process.env, HOME: home, XDG_CONFIG_HOME: join(home, 'config'), XDG_CACHE_HOME: join(home, 'cache') },
    args: ['--no-sandbox', '--disable-gpu', '--disable-background-networking', '--disable-component-update', '--no-first-run', '--disable-default-apps', '--force-color-profile=srgb'],
  });
  for (const screen of process.env.HOH_GROUPING_ONLY === '1' ? [] : selectedScreens) {
    for (const size of SIZES) {
      const engine = screen.engine();
      const { context, page, problems, off } = await open(browser, engine, size, screen.path);
      const file = `${screen.name}-${size.width}x${size.height}.png`;
      let status = 'passed'; let error;
      try { await screen.run(page, engine, size); }
      catch (caught) { status = 'failed'; error = String(caught?.message ?? caught).split('\n')[0]; }
      await settle(page);
      await page.screenshot({ path: join(OUT, file), animations: 'disabled' });
      const text = (await page.evaluate(() => document.body.innerText)).slice(0, 400);
      report.push({ screen: screen.name, size: `${size.width}x${size.height}`, file, status, error, problems, text, metrics: await page.evaluate(metrics) });
      console.log(`${status === 'passed' ? 'PASS' : 'FAIL'} ${file}${error ? ' — ' + error : ''}${problems.length ? ` (${problems.length} problems)` : ''}`);
      off(); await context.close();
    }
  }
  if (process.env.HOH_GROUPING_SHOTS === '1') {
    const capture = async ({ name, size, grouped = true, reducedMotion = 'no-preference', path = '/', run, delay = 0 }) => {
      const engine = groupingEngine();
      const { context, page, problems, off } = await open(browser, engine, size, path, {
        colorScheme: 'dark', reducedMotion, storage: groupingStorage(grouped),
      });
      const file = `${name}-${size.width}x${size.height}.png`;
      let status = 'passed'; let error; let bounds; let projectState;
      try {
        if (path === '/' && grouped) {
          await readyGroupedHome(page, engine);
          projectState = await page.evaluate(() => {
            const folders = [...document.querySelectorAll('.hoh-project')];
            const waiting = document.querySelector('.hoh-project[data-waiting]');
            const working = document.querySelector('.hoh-project[data-working]');
            const resolveColor = value => {
              const probe = document.createElement('i');
              probe.style.color = value; document.body.append(probe);
              const color = getComputedStyle(probe).color; probe.remove(); return color;
            };
            return {
              colors: folders.map(folder => folder.getAttribute('data-color')),
              waitingCount: document.querySelectorAll('.hoh-project[data-waiting]').length,
              workingCount: document.querySelectorAll('.hoh-project[data-working]').length,
              waitingDot: waiting ? getComputedStyle(waiting.querySelector('.hoh-project__chip i')).backgroundColor : null,
              workingDot: working ? getComputedStyle(working.querySelector('.hoh-project__chip i')).backgroundColor : null,
              destructive: resolveColor('var(--destructive)'), primary: resolveColor('var(--primary)'),
              waitingBorder: waiting ? getComputedStyle(waiting).borderColor : null,
              workingBorder: working ? getComputedStyle(working).borderColor : null,
            };
          });
          if (new Set(projectState.colors).size !== projectState.colors.length) throw new Error(`project palette collision: ${JSON.stringify(projectState.colors)}`);
          if (projectState.waitingCount !== 1 || projectState.workingCount !== 2) throw new Error(`wrong project states: ${JSON.stringify(projectState)}`);
          if (projectState.waitingDot !== projectState.destructive || projectState.workingDot !== projectState.primary) throw new Error(`wrong project dots: ${JSON.stringify(projectState)}`);
          if (projectState.waitingBorder === projectState.workingBorder) throw new Error(`project state borders match: ${projectState.waitingBorder}`);
        }
        else if (path === '/') await page.locator('.hoh-card').first().waitFor();
        else await page.getByRole('switch', { name: 'Group agents by project' }).waitFor();
        if (run) await run(page);
        if (delay > 0) await page.waitForTimeout(delay);
        bounds = await page.evaluate(() => {
          const panel = document.querySelector('.hoh-project-popover');
          if (!panel) return null;
          const rect = panel.getBoundingClientRect();
          const grid = document.querySelector('.hoh-projects')?.getBoundingClientRect();
          const home = document.querySelector('.hoh-home')?.getBoundingClientRect();
          const composer = document.querySelector('[data-chat-composer-stack]')?.getBoundingClientRect();
          const cards = [...panel.querySelectorAll('.hoh-project-popover__agent')];
          const visible = home ? { left: Math.max(rect.left, home.left), top: Math.max(rect.top, home.top), right: Math.min(rect.right, home.right), bottom: Math.min(rect.bottom, home.bottom) } : rect;
          const overlapsComposer = composer ? visible.left < composer.right && visible.right > composer.left && visible.top < composer.bottom && visible.bottom > composer.top : false;
          const last = cards.at(-1);
          const allCardsVisible = cards.every(card => {
            const cardRect = card.getBoundingClientRect();
            return cardRect.left >= visible.left - 1 && cardRect.right <= visible.right + 1 && cardRect.top >= visible.top - 1 && cardRect.bottom <= visible.bottom + 1;
          });
          return {
            left: rect.left, top: rect.top, right: rect.right, bottom: rect.bottom, viewport: [innerWidth, innerHeight],
            position: getComputedStyle(panel).position,
            insideColumn: Boolean(grid && rect.left >= grid.left - 1 && rect.right <= grid.right + 1),
            overlapsComposer, composerVisible: Boolean(composer && composer.top >= -1 && composer.bottom <= innerHeight + 1), cardCount: cards.length, allCardsVisible,
            lastCardReachable: Boolean(last && last.offsetTop + last.offsetHeight <= panel.scrollHeight + 1),
            panelScrollable: panel.scrollHeight > panel.clientHeight,
          };
        });
        if (bounds && (!bounds.insideColumn || bounds.position === 'fixed' || bounds.position === 'absolute')) throw new Error(`folder disclosure leaves content flow: ${JSON.stringify(bounds)}`);
        if (bounds?.overlapsComposer) throw new Error(`folder disclosure overlaps composer: ${JSON.stringify(bounds)}`);
        if (bounds && !bounds.composerVisible) throw new Error(`folder disclosure moved composer outside viewport: ${JSON.stringify(bounds)}`);
        if (bounds && (bounds.cardCount !== 3 || !bounds.lastCardReachable)) throw new Error(`not all project cards are reachable: ${JSON.stringify(bounds)}`);
        if (name === 'projects-hover' && bounds && !bounds.allCardsVisible) throw new Error(`not all project cards are visible after expansion: ${JSON.stringify(bounds)}`);
      } catch (caught) { status = 'failed'; error = String(caught?.message ?? caught).split('\n')[0]; }
      await page.screenshot({ path: join(OUT, file) });
      report.push({ screen: name, size: `${size.width}x${size.height}`, file, status, error, problems, bounds, projectState, text: (await page.evaluate(() => document.body.innerText)).slice(0, 400), metrics: await page.evaluate(metrics) });
      console.log(`${status === 'passed' ? 'PASS' : 'FAIL'} ${file}${error ? ' — ' + error : ''}${problems.length ? ` (${problems.length} problems)` : ''}`);
      off(); await context.close();
    };

    await capture({ name: 'grouping-settings', size: GROUPING_SIZES[0], path: '/settings/appearance' });
    await capture({ name: 'home-flat', size: GROUPING_SIZES[0], grouped: false });
    for (const size of GROUPING_SIZES) await capture({ name: 'projects-closed', size });
    const alphaFolder = page => page.locator('.hoh-project').filter({ hasText: 'Alexandrie' }).first();
    const openAlpha = async page => { await alphaFolder(page).hover(); await page.locator('.hoh-project-popover').waitFor(); };
    for (const size of GROUPING_SIZES) await capture({ name: 'projects-hover', size, run: openAlpha, delay: 320 });
    for (const size of GROUPING_SIZES) for (const delay of [0, 80, 160, 240]) {
      await capture({ name: `projects-animation-${delay}ms`, size, run: openAlpha, delay });
    }
    await capture({ name: 'projects-reduced-motion', size: GROUPING_SIZES[0], reducedMotion: 'reduce', run: page => alphaFolder(page).hover() });
    await capture({ name: 'projects-keyboard-pinned', size: GROUPING_SIZES[0], run: async page => { const folder = alphaFolder(page); await folder.focus(); await page.keyboard.press('Enter'); }, delay: 260 });
  }
} finally {
  await browser?.close();
  await rm(home, { recursive: true, force: true });
}
await writeFile(join(OUT, 'report.json'), JSON.stringify(report, null, 2) + '\n');
const failed = report.filter(entry => entry.status !== 'passed' || entry.problems.some(p => p.startsWith('remote') || p.startsWith('pageerror')));
console.log(`${report.length - failed.length}/${report.length} screens clean; report ${join(OUT, 'report.json')}`);
process.exit(failed.length ? 1 : 0);
