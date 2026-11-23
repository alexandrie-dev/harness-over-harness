// Проверка сетки окна (HOH-41, Golden Canon Grid): падает, если блок окна стоит не на линиях. Запускается в сборке после check-dist
// (scripts/build.sh). Две части.
// 1) Исходники: grid/tokens.css — 20 дорожек ровно в сумме 64, линии g1..g21 и числа --gc-x/--gc-y совпадают с blocks.ts; в CSS окна
//    положение блока (grid-column/-row/-area) задано только линиями g1..g21 (как check_design.py эталона); все линии карты блоков в 1..21.
// 2) Собранное окно (dist/index.html, headless Chrome, подделка движка): window.hohGridCheck() (src/t3-entry/grid/check.ts, допуск 2 px
//    как у gc-debug.js эталона) на пустой главной и на главной с агентами при 2560×1440, 1920×1080, 1440×900, 1280×720 и 1100×700, а также при 1000×625
//    (сетка отступает: блоки не выходят за край). Любой блок вне линии — ошибка с именем блока, стороной и числом px.
import { chromium } from 'playwright-core';
import { readdir, readFile, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, extname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import assert from 'node:assert/strict';
import { FakeEngine } from '../tests/chat/fake-engine.mjs';

const UI = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const ENTRY = join(UI, 'src/t3-entry');
const DIST = join(UI, 'dist');
const errors = [];
const fail = (message) => errors.push(message);

// ---- 1. Исходники ----
const tokens = await readFile(join(ENTRY, 'grid/tokens.css'), 'utf8');
const blocksSource = await readFile(join(ENTRY, 'grid/blocks.ts'), 'utf8');
const half = /const HALF = \[([^\]]+)\]/.exec(blocksSource)?.[1].split(',').map((item) => { const [a, b = '1'] = item.split('/').map(Number); return a / b; });
if (!half || half.length !== 10) fail('blocks.ts: нет HALF из 10 дорожек');
else {
  const tracks = [...half, ...half.slice().reverse()];
  const total = tracks.reduce((sum, track) => sum + track, 0);
  if (Math.abs(total - 64) > 1e-9) fail(`дорожки в сумме ${total}, нужно 64`);
  const declared = [...(/--gc-tracks:([^;]+);/.exec(tokens)?.[1] ?? '').matchAll(/\[g(\d+)\]\s*(?:minmax\(0,([\d.]+)fr\))?/g)];
  if (declared.length !== 21) fail(`--gc-tracks: ${declared.length} линий вместо 21`);
  declared.slice(0, 20).forEach((match, index) => {
    if (Number(match[1]) !== index + 1) fail(`--gc-tracks: линия ${match[1]} стоит на месте g${index + 1}`);
    if (Math.abs(Number(match[2]) - tracks[index]) > 1e-4) fail(`--gc-tracks: дорожка ${index + 1} = ${match[2]}fr, в blocks.ts ${tracks[index].toFixed(5)}`);
  });
  let sum = 0;
  for (let n = 1; n <= 21; n++) {
    const x = /--gc-x(\d+):\s*([\d.]+)vw/g; const y = new RegExp(`--gc-y${n}:\\s*calc\\(var\\(--gc-vh\\) \\* ([\\d.]+)\\)`);
    const share = sum / 64;
    const xv = [...tokens.matchAll(x)].find((m) => Number(m[1]) === n);
    if (!xv || Math.abs(Number(xv[2]) - share * 100) > 1e-3) fail(`--gc-x${n}: ${xv?.[2]}vw, нужно ${(share * 100).toFixed(5)}vw`);
    const yv = y.exec(tokens);
    if (!yv || Math.abs(Number(yv[1]) - share) > 1e-4) fail(`--gc-y${n}: ${yv?.[1]}, нужно ${share.toFixed(5)}`);
    if (n <= 20) sum += tracks[n - 1];
  }
}
for (const [, a, b] of blocksSource.matchAll(/\[(\d+|null), (\d+|null)\]/g)) for (const n of [a, b]) if (n !== 'null' && (Number(n) < 1 || Number(n) > 21)) fail(`blocks.ts: линия g${n} вне g1..g21`);

async function cssFiles(dir) {
  return (await Promise.all((await readdir(dir, { withFileTypes: true })).map((entry) =>
    entry.isDirectory() ? cssFiles(join(dir, entry.name)) : extname(entry.name) === '.css' ? [join(dir, entry.name)] : []))).flat();
}
for (const file of await cssFiles(ENTRY)) {
  const text = (await readFile(file, 'utf8')).replace(/\/\*[\s\S]*?\*\//g, ' ');
  for (const [, prop, value] of text.matchAll(/(grid-column|grid-row|grid-area)\s*:\s*([^;}]+)/g)) {
    if (/gc-exception/.test(value)) continue;
    const bare = value.replace(/var\(--gc-[^)]*\)/g, ' ').replace(/\s*!important\s*$/i, '');
    for (const token of bare.split(/[\s/,]+/).filter(Boolean)) {
      const line = /^g(\d+)$/i.exec(token);
      if (line ? Number(line[1]) < 1 || Number(line[1]) > 21 : !/^(auto|span|subgrid|\d+)$/.test(token) || (/^\d+$/.test(token) && !/span\s*$/.test(bare.slice(0, bare.indexOf(token)))))
        fail(`${file.slice(UI.length + 1)}: ${prop}: ${value.trim()} — только линии g1..g21`);
    }
  }
}

// ---- 2. Собранное окно ----
const TYPES = { '.js': 'text/javascript', '.css': 'text/css', '.html': 'text/html', '.png': 'image/png', '.svg': 'image/svg+xml', '.woff2': 'font/woff2', '.json': 'application/json', '.wasm': 'application/wasm' };
const ORIGIN = 'http://hoh-window.test';
function engineWith(busy) {
  const engine = new FakeEngine();
  engine.access = 'confirm';
  engine.claude = { installed: true, version: '2.1.0', auth: 'authenticated', models: [{ id: 'opus', displayName: 'Claude Opus', isDefault: true }] };
  if (!busy) return engine;
  for (const [id, title] of [['w1', 'Prepare the weekly report'], ['w2', 'Fix the build'], ['w3', 'Review the release notes']]) {
    const session = engine.open(id, title);
    engine.append(session, 'turn/start', { turn: 1 });
    engine.append(session, 'user/message', { id: `${id}-u`, source: { kind: 'user' }, content: [{ type: 'text', text: title }] });
    engine.append(session, 'tool/call', { callId: `${id}-c`, turn: 1, name: 'Bash', arguments: { command: id === 'w2' ? 'printf "prepare report, verify every source, and save the complete result to the requested destination"' : 'ls' } });
  }
  for (const [id, title] of [['d1', 'Summarize the invoices'], ['d2', 'Check the final totals']]) {
    const done = engine.open(id, title);
    engine.append(done, 'turn/start', { turn: 1 });
    engine.append(done, 'user/message', { id: `${id}-u`, source: { kind: 'user' }, content: [{ type: 'text', text: title }] });
    engine.append(done, 'assistant/message', { message: { id: `${id}-a`, content: [{ type: 'text', text: 'ok' }] } });
    engine.append(done, 'turn/end', { turn: 1, reason: { kind: 'completed' } });
  }
  return engine;
}
async function open(browser, engine, size) {
  const context = await browser.newContext({ viewport: size, deviceScaleFactor: 1, locale: 'en-US', reducedMotion: 'reduce', colorScheme: 'dark' });
  const page = await context.newPage();
  page.setDefaultTimeout(20000);
  await page.route('**/*', async (route) => {
    const url = new URL(route.request().url());
    if (url.origin !== ORIGIN) return route.abort();
    const file = extname(url.pathname) ? url.pathname : '/index.html';
    try { return await route.fulfill({ body: await readFile(join(DIST, file)), contentType: TYPES[extname(file)] ?? 'application/octet-stream' }); }
    catch { return route.fulfill({ status: 404, body: '' }); }
  });
  await page.exposeBinding('__hohT3Request', (_, method, params) => engine.request(method, params).catch((error) => ({ __hohError: String(error?.message ?? error) })));
  await page.addInitScript(() => {
    const listeners = new Set();
    let clipboardText = '';
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: {
      writeText: async (value) => { clipboardText = String(value); },
      readText: async () => clipboardText,
    } });
    window.__hohT3Notify = (notification) => { for (const listener of listeners) listener(notification); };
    window.__HOH_T3_TEST_ENGINE__ = {
      request: async (method, params) => { const result = await window.__hohT3Request(method, params); if (result && result.__hohError) throw new Error(result.__hohError); return result; },
      subscribe: async (listener) => { listeners.add(listener); return () => listeners.delete(listener); },
      subscribeStatus: async (listener) => { setTimeout(() => listener({ state: 'ready' }), 0); return () => {}; },
      subscribeQuestions: async () => () => {}, answerQuestion: async () => {}, decideApproval: async () => {},
    };
  });
  const off = engine.subscribe((notification) => { page.evaluate((value) => window.__hohT3Notify?.(value), notification).catch(() => {}); });
  await page.goto(ORIGIN + '/');
  return { context, page, off };
}

assert(await readFile(join(DIST, 'index.html')).then(() => true, () => false), 'нет dist/index.html: сначала vite build --mode t3');
const home = await mkdtemp(join(tmpdir(), 'hoh-grid-'));
let browser;
try {
  browser = await chromium.launch({
    executablePath: process.env.HOH_CHROMIUM ?? '/usr/bin/google-chrome', headless: true,
    env: { ...process.env, HOME: home, XDG_CONFIG_HOME: join(home, 'config'), XDG_CACHE_HOME: join(home, 'cache') },
    args: ['--no-sandbox', '--disable-gpu', '--disable-background-networking', '--disable-component-update', '--no-first-run'],
  });
  for (const [width, height] of [[2560, 1440], [1920, 1080], [1440, 900], [1280, 720], [1100, 700], [1000, 625], [800, 550]]) {
    for (const busy of [false, true]) {
      const label = `${width}×${height} ${busy ? 'с агентами' : 'пустая главная'}`;
      const engine = engineWith(busy);
      const { context, page, off } = await open(browser, engine, { width, height });
      try {
        if (!busy) {
          await page.getByRole('button', { name: 'Start', exact: true }).click();
          await page.getByRole('button', { name: 'Start without a project', exact: true }).click();
        }
        await page.getByText(/Good (night|morning|afternoon|evening)/).first().waitFor({ state: 'attached' });
        if (busy) {
          for (const sessionId of ['w1', 'w2', 'w3']) engine.emit('session.status', { sessionId, status: 'running' });
          const waitingSession = engine.sessions.get('w2');
          engine.emit('session.event', { sessionId: 'w2', event: engine.append(waitingSession, 'turn/start', { turn: 2 }) });
          engine.emit('session.event', { sessionId: 'w2', event: engine.append(waitingSession, 'tool/call', { callId: 'w2-live-c', turn: 2, name: 'Bash', arguments: { command: 'printf "prepare the weekly report, verify every source twice, compare the totals with the previous period, explain every discrepancy, and save the complete reviewed result to the requested destination"' } }) });
          engine.emit('approval.pending', { approvalId: 'appr-1', sessionId: 'w2', risk: 'unassessed', humanText: 'Claude asks to run a deliberately long command that wraps over several lines: printf "prepare report, verify every source, and save the complete result"', expiresAt: Date.now() + 600_000 });
          await page.locator('.hoh-card').first().waitFor({ state: 'attached' });
          await page.locator('.hoh-ask').first().waitFor({ state: 'attached' });
          await page.evaluate(() => window.__HOH_HOME__.complete('d1'));
          await page.locator('.hoh-card[data-state="done"]').waitFor({ state: 'attached' });
        }
        await page.evaluate(() => document.fonts.ready);
        await page.waitForTimeout(800);
        if (process.env.VITE_HOH_GRID_OVERLAY !== '1' && await page.locator('#hoh-grid-overlay').count() > 0) fail(`${label}: в окне наложение сетки — оно только для снимков (VITE_HOH_GRID_OVERLAY=1)`);
        const overlap = await page.evaluate(() => {
          const body = document.querySelector('[data-hoh-home-body]')?.getBoundingClientRect();
          const stack = document.querySelector('[data-chat-composer-stack]')?.getBoundingClientRect();
          return body && stack && body.bottom > stack.top + 0.5 ? Math.round((body.bottom - stack.top) * 10) / 10 : 0;
        });
        if (overlap > 0) fail(`${label}: тело главной заходит на поле ввода на ${overlap} px`);
        if (!busy) {
          const emptyGap = await page.evaluate(() => {
            const greeting = document.querySelector('.hoh-home__head')?.getBoundingClientRect();
            const prompt = document.querySelector('[data-chat-composer-stack] h1')?.getBoundingClientRect();
            return greeting && prompt ? Math.round((prompt.top - greeting.bottom) * 10) / 10 : null;
          });
          if (emptyGap === null) fail(`${label}: не найдены приветствие и заголовок пустой главной`);
          else if (emptyGap > 64) fail(`${label}: между приветствием и вопросом пустой главной ${emptyGap} px (нужно не больше 64 px)`);
        }
        if (busy && width === 800 && height === 550) {
          const detail = page.locator('.hoh-ask__detail').first();
          const collapsed = await detail.evaluate((button) => {
            const text = button.querySelector('.hoh-ask__text');
            const style = getComputedStyle(text);
            return { expanded: button.getAttribute('aria-expanded'), height: text.getBoundingClientRect().height, line: parseFloat(style.lineHeight), scroll: text.scrollHeight, text: text.textContent };
          });
          if (collapsed.expanded !== 'false') fail(`${label}: запрос изначально раскрыт`);
          if (collapsed.height > collapsed.line * 2 + 1) fail(`${label}: запрос занимает больше двух строк (${collapsed.height}px)`);
          await detail.click();
          const expanded = await detail.evaluate((button) => ({ expanded: button.getAttribute('aria-expanded'), height: button.querySelector('.hoh-ask__text').getBoundingClientRect().height }));
          if (expanded.expanded !== 'true' || expanded.height <= collapsed.height) fail(`${label}: запрос не раскрывается по нажатию (${JSON.stringify({ collapsed, expanded })})`);
          await detail.press('Space');
          if (await detail.getAttribute('aria-expanded') !== 'false') fail(`${label}: запрос не сворачивается клавиатурой`);
          const nowVisible = await page.evaluate(() => {
            const heading = [...document.querySelectorAll('.hoh-home__label')].find((node) => node.textContent?.trim().startsWith('Now'))?.getBoundingClientRect();
            const composer = document.querySelector('[data-chat-composer-stack]')?.getBoundingClientRect();
            return Boolean(heading && composer && heading.top >= 0 && heading.bottom <= composer.top);
          });
          if (!nowVisible) fail(`${label}: шапка карточек «Now» не видна над полем ввода`);
        }
        const found = await page.evaluate(() => (window.hohGridCheck ? window.hohGridCheck() : null));
        if (found === null) fail(`${label}: в окне нет window.hohGridCheck`);
        else for (const miss of found) fail(`${label}: ${miss.block}.${miss.side} ${miss.px} px от ${miss.expected} (ближайшая ${miss.nearest || '—'})`);
        if (busy) {
          const cards = await page.locator('.hoh-card').evaluateAll((nodes) => nodes.map((node) => {
            const box = node.getBoundingClientRect();
            return { x: box.x, y: box.y, right: box.right, width: box.width, state: node.getAttribute('data-state') };
          }));
          const count = state => cards.filter(card => card.state === state).length;
          if (cards.length !== 5 || count('working') !== 2 || count('waiting') !== 2 || count('done') !== 1)
            fail(`${label}: нужны 5 карточек (2 working, 2 waiting, 1 done), получено ${JSON.stringify(cards.map(card => card.state))}`);
          if (cards.some(card => card.x < -0.5 || card.right > width + 0.5)) fail(`${label}: карточка вышла за окно`);
          if (width >= 1100) {
            if (cards.some(card => card.width > 360.5)) fail(`${label}: карточка шире 360 px: ${cards.map(card => card.width.toFixed(1)).join(', ')}`);
            const firstY = Math.min(...cards.map(card => card.y));
            const firstRow = cards.filter(card => Math.abs(card.y - firstY) < 1).length;
            const expected = width === 1440 ? 2 : width === 1920 ? 3 : width === 2560 ? 4 : 2;
            if (firstRow !== expected) fail(`${label}: в первом ряду ${firstRow} карточек, нужно ${expected}`);
          }
        }
        if (busy) { // Learning и живая страница Plugins; «New chat» закрывает обе.
          await page.locator('[data-hoh-page-item="learning"]').click();
          await page.locator('.hoh-page').waitFor({ state: 'visible' });
          const learningText = await page.locator('.hoh-page').innerText();
          if (learningText.replace(/\s+/g, ' ').trim() !== 'Learning Coming soon') fail(`${label}: страница Learning: «${learningText.trim()}»`);
          if (await page.locator('[data-hoh-page-item="learning"][aria-current="page"]').count() !== 1) fail(`${label}: пункт Learning не подсвечен`);
          const pageMiss = await page.evaluate(() => window.hohGridCheck());
          for (const miss of pageMiss) fail(`${label}: страница Learning: ${miss.block}.${miss.side} ${miss.px} px от ${miss.expected}`);

          await page.locator('[data-hoh-page-item="plugins"]').click();
          await page.locator('.hoh-plugins').waitFor({ state: 'visible' });
          if (await page.locator('[data-hoh-page-item="plugins"][aria-current="page"]').count() !== 1) fail(`${label}: пункт Plugins не подсвечен`);
          if (await page.locator('[data-plugin-list] [data-plugin-id]').count() !== 7) fail(`${label}: страница Plugins показывает не 7 образцов`);
          if (!/Sample data\s*— not connected to the engine yet\./.test((await page.locator('.hoh-plugins__sample').innerText()).replace(/\s+/g, ' '))) fail(`${label}: нет заметной пометки об образцовых данных`);
          if (await page.getByRole('heading', { name: 'Built-in plugins' }).count() !== 1) fail(`${label}: нет отдельного раздела встроенных плагинов`);
          const overflow = await page.locator('.hoh-plugins').evaluate((node) => node.scrollWidth > node.clientWidth + 1);
          if (overflow) fail(`${label}: страница Plugins имеет горизонтальное переполнение`);

          if (width === 1440 && height === 900) {
            await page.locator('[data-add-plugin-folder]').click();
            await page.getByRole('heading', { name: 'Add plugin from folder' }).waitFor();
            await page.locator('[data-slot="dialog-footer"]').getByRole('button', { name: 'Close' }).click();

            await page.getByRole('button', { name: 'Orbit Tools' }).click();
            await page.locator('[data-plugin-detail="alexandrie-orbit-tools"]').waitFor();
            await page.getByRole('button', { name: 'Disable' }).click();
            if (await page.locator('[data-plugin-status="approval-pending"]').count() < 1) fail(`${label}: Disable не переводит образец в Approval pending`);
            await page.getByRole('button', { name: 'More plugin actions' }).click();
            await page.getByRole('menuitem', { name: 'Copy plugin ID' }).click();
            await page.getByText('Plugin ID copied').waitFor();
            if (await page.evaluate(() => navigator.clipboard.readText()) !== 'alexandrie-orbit-tools') fail(`${label}: меню не скопировало настоящий plugin ID`);
            await page.getByRole('button', { name: 'Copy SHA-256 fingerprint' }).click();
            await page.getByText('Fingerprint copied').waitFor();
            if (await page.evaluate(() => navigator.clipboard.readText()) !== 'sha256:3d7f6bf47d824da418d43fbba0628dce3f7825688b364a9a292485cab13ed02b') fail(`${label}: кнопка не скопировала полный SHA-256`);
            await page.getByRole('button', { name: 'All plugins' }).click();

            await page.getByRole('button', { name: 'Variant B' }).click();
            const firstRow = page.locator('.hoh-plugin-row__trigger').first();
            await firstRow.focus(); await page.keyboard.press('Enter');
            if (await firstRow.getAttribute('aria-expanded') !== 'true') fail(`${label}: строка варианта B не раскрывается клавиатурой`);
            await page.getByRole('button', { name: 'Open plugin page' }).first().click();
            await page.locator('[data-plugin-detail]').waitFor();
            await page.getByRole('button', { name: 'All plugins' }).click();

            await page.getByRole('button', { name: 'Variant C' }).click();
            if (await page.locator('.hoh-plugin-split').count() !== 1) fail(`${label}: вариант C не показывает две колонки`);
            await page.getByRole('button', { name: 'Open full page' }).click();
            await page.locator('[data-plugin-detail]').waitFor();
            if (await page.evaluate(() => localStorage.getItem('hoh:plugins-design-variant')) !== 'C') fail(`${label}: выбранный вариант C не запомнен`);
            await page.getByRole('button', { name: 'All plugins' }).click();
            await page.reload();
            await page.locator('[data-hoh-page-item="plugins"]').click();
            await page.locator('.hoh-plugins[data-plugin-variant="C"]').waitFor();
          }
          await page.locator('[data-hoh-sidebar-top] .hoh-side__new').first().click();
          if (await page.locator('.hoh-page').count() !== 0) fail(`${label}: «New chat» не закрыл страницу`);
          if (width === 1440 && height === 900) {
            await page.keyboard.press('Control+K');
            const palette = page.locator('[aria-label="Command palette"]').last();
            await palette.waitFor({ state: 'visible' });
            const paletteText = (await palette.innerText()).replace(/\s+/g, ' ');
            if (!paletteText.includes('New chat')) fail(`${label}: в палитре нет видимого «New chat»`);
            if (paletteText.includes('New thread')) fail(`${label}: в палитре ещё видно «New thread»`);
            await page.keyboard.press('Escape');
          }
        }
      } catch (error) {
        fail(`${label}: ${String(error.message).split('\n')[0]}`);
        if (process.env.HOH_GRID_DEBUG) { await page.screenshot({ path: join(process.env.HOH_GRID_DEBUG, `${width}x${height}-${busy}.png`) }); console.error((await page.evaluate(() => document.body.innerText)).slice(0, 400)); }
      }
      off(); await context.close();
    }
  }
} finally { await browser?.close(); await rm(home, { recursive: true, force: true }); }

if (errors.length) { for (const message of errors) console.error('grid: ' + message); process.exit(1); }
console.log('grid: все блоки на линиях g1..g21; карточки 2/3/4 в ряд при 1440/1920/2560 и не шире 360 px (также 1280×720, 1100×700, 1000×625, 800×550; допуск 2 px)');
