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
import { FakeEngine } from '/mnt/toshiba/alexandrie/hoh/wt/u4/ui/tests/chat/fake-engine.mjs';

const UI = '/mnt/toshiba/alexandrie/hoh/wt/u4/ui';
const DIST = process.env.HOH_T3_DIST ? resolve(process.env.HOH_T3_DIST) : join(UI, 'dist');
const OUT = "/dev/shm/hoh/u4/x";
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
await mkdir(OUT, { recursive: true });
const browser = await chromium.launch({ executablePath: '/usr/bin/google-chrome', headless: true, args: ['--no-sandbox'] });
const { page } = await open(browser, engineWith([CHAT]), SIZES[0]);
await page.getByRole('textbox').first().waitFor(); await page.waitForTimeout(800);
console.log(await page.evaluate(() => [...document.querySelectorAll('a[href="/"]')].map(a => a.outerHTML.slice(0, 300))));
await browser.close();
