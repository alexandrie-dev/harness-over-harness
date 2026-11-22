// Снимки входа окна на веб-клиенте T3 (HOH-40): экраны пути основателя при 1440×900 и 800×550, headless Chrome,
// без сервера и сети. Страница — собранная dist (vite build && vite build --mode t3), отдаётся из файлов через
// page.route на выдуманном адресе (роутеру T3 нужен путь «/», а не file://). Движок — подделка tests/chat/fake-engine.mjs
// за хостом HoH (ui/src/hoh-host): вход берёт её из window.__HOH_T3_TEST_ENGINE__ только вне Tauri (src/t3-entry/main.ts).
//   node scripts/snapshot-t3.mjs [папка-снимков]      (по умолчанию test-results/t3-entry)
// Экраны: первый запуск, пустой разговор, разговор с ответом и шагом, запрос разрешения, меню модели, меню режима, настройки.
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
const SIZES = [{ width: 1440, height: 900 }, { width: 800, height: 550 }];
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

const CHAT = { id: 'chat-1', title: 'Files in the folder', user: 'What files are in this folder?', command: 'ls',
  output: 'notes.txt\nreport.txt\nscript.py\n[Command finished with exit code 0]',
  answer: 'The folder has three files:\n\n- `notes.txt`\n- `report.txt`\n- `script.py`' };

async function open(browser, engine, size, path = '/') {
  const context = await browser.newContext({ viewport: size, deviceScaleFactor: 1, locale: 'en-US', reducedMotion: 'reduce', colorScheme: 'light' });
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
  await page.addInitScript(() => {
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
  });
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

const screens = [
  {
    name: 'first-run', engine: () => engineWith([]),
    // Хост u3 (fix1): движок без разговоров → T3 сам открывает /welcome; хост до fix1 — сразу поле ввода.
    run: async page => { await page.getByText('Welcome to T3 Code').or(page.getByRole('textbox')).first().waitFor(); },
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
    name: 'settings', engine: () => engineWith([CHAT]), path: '/settings/general',
    run: async page => { await page.getByRole('navigation').first().waitFor(); },
  },
];

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
  for (const screen of screens) {
    for (const size of SIZES) {
      const engine = screen.engine();
      const { context, page, problems, off } = await open(browser, engine, size, screen.path);
      const file = `${screen.name}-${size.width}x${size.height}.png`;
      let status = 'passed'; let error;
      try { await screen.run(page, engine); }
      catch (caught) { status = 'failed'; error = String(caught?.message ?? caught).split('\n')[0]; }
      await settle(page);
      await page.screenshot({ path: join(OUT, file), animations: 'disabled' });
      const text = (await page.evaluate(() => document.body.innerText)).slice(0, 400);
      report.push({ screen: screen.name, size: `${size.width}x${size.height}`, file, status, error, problems, text, metrics: await page.evaluate(metrics) });
      console.log(`${status === 'passed' ? 'PASS' : 'FAIL'} ${file}${error ? ' — ' + error : ''}${problems.length ? ` (${problems.length} problems)` : ''}`);
      off(); await context.close();
    }
  }
} finally {
  await browser?.close();
  await rm(home, { recursive: true, force: true });
}
await writeFile(join(OUT, 'report.json'), JSON.stringify(report, null, 2) + '\n');
const failed = report.filter(entry => entry.status !== 'passed' || entry.problems.some(p => p.startsWith('remote') || p.startsWith('pageerror')));
console.log(`${report.length - failed.length}/${report.length} screens clean; report ${join(OUT, 'report.json')}`);
process.exit(failed.length ? 1 : 0);
