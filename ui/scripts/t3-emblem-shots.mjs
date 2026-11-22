// Снимки эмблемы в окне на T3 (HOH-41): dist/index.html (vite build --mode t3) в Chromium без сети. Страница открывается как
// http://hoh.local/, файлы dist отдаёт перехват запросов, движок — поддельный из tests/hoh-host/fake-engine.mjs через
// привязку страницы (хост HoH — настоящий, ui/src/hoh-host). Экраны: заставка, Welcome, главный экран, пустой разговор
// в папке, шапка боковой панели (вырезка ×4); размеры 1440×900 и 800×550; масштаб окна 100 % и 150 % / 120 % (CSS zoom
// браузера задаёт deviceScaleFactor и окно в CSS px, как Ctrl+«+»).
//   node scripts/t3-emblem-shots.mjs <папка снимков> [dist]
import { chromium } from 'playwright-core';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { extname, join, normalize, resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { FakeEngine } from '../tests/hoh-host/fake-engine.mjs';

const UI = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const out = resolve(process.argv[2] ?? 'test-results/emblem');
const dist = resolve(process.argv[3] ?? join(UI, 'dist'));
const ORIGIN = 'http://hoh.local';
const TYPES = { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.json': 'application/json', '.wasm': 'application/wasm', '.woff2': 'font/woff2', '.woff': 'font/woff', '.png': 'image/png', '.svg': 'image/svg+xml', '.mp3': 'audio/mpeg' };
const SIZES = [[1440, 900], [800, 550]];
const SCALES = [{ tag: '100', dpr: 1, zoom: 1 }, { tag: '150', dpr: 1.5, zoom: 1 }, { tag: 'plus2', dpr: 1, zoom: 1.2 }, { tag: 'plus4', dpr: 1, zoom: 1.44 }, { tag: 'plus6', dpr: 1, zoom: 1.73 }];
const PROJECT = { version: 1, projects: [{ id: 'p-site', title: 'site', workspaceRoot: '/home/person/Projects/site', createdAt: 1700000000000, updatedAt: 1700000000000 }], threads: {} };

await mkdir(out, { recursive: true });
await mkdir('/dev/shm/hoh/u2', { recursive: true });
const home = await mkdtemp('/dev/shm/hoh/u2/chrome-');
const log = [];
let browser;

async function open(width, height, scale, { seed, hold }) {
  const context = await browser.newContext({ viewport: { width: Math.round(width / scale.zoom), height: Math.round(height / scale.zoom) }, deviceScaleFactor: scale.dpr * scale.zoom, reducedMotion: 'reduce', locale: 'en-US' });
  const page = await context.newPage();
  const engine = new FakeEngine();
  const tag = `${width}x${height}-${scale.tag}`;
  page.on('pageerror', (e) => log.push(`[${tag}] pageerror: ${e.message}`));
  await page.exposeBinding('__e', (_, method, params) => engine.request(method, params));
  await page.addInitScript((seedValue) => {
    if (seedValue) localStorage.setItem('hoh-host.projects.v1', JSON.stringify(seedValue));
    window.__HOH_T3_TEST_ENGINE__ = { request: (m, p) => window.__e(m, p), subscribe: async () => () => {}, subscribeStatus: async () => () => {}, subscribeQuestions: async () => () => {}, answerQuestion: async () => {} };
  }, seed ? PROJECT : null);
  // EMB_FORCE=1 — показать эмблему пустого разговора в любой высоте (подбор порога высоты).
  if (process.env.EMB_FORCE) await context.addInitScript(() => { document.addEventListener('DOMContentLoaded', () => { const st = document.createElement('style'); st.textContent = '@media (max-height: 99rem) { div.mx-auto.max-w-5xl.flex-col.items-center:has(> h1, > div[data-inline])::before, [data-slot="empty-header"]:has(> [data-slot="empty-title"])::before { display: block !important; } }'; document.head.append(st); }); });
  await context.route('**/*', async (route) => {
    const url = new URL(route.request().url());
    if (url.origin !== ORIGIN) { log.push(`[${tag}] отклонён: ${url.href}`); return route.abort(); }
    if (hold && url.pathname.startsWith('/t3-assets/') && url.pathname.endsWith('.js')) return route.fulfill({ status: 200, contentType: 'text/javascript', body: '' });   // сценарии не работают: остаётся статическая заставка из t3.html
    if (url.pathname.startsWith('/api/') || url.pathname.startsWith('/.well-known')) return route.fallback();
    const path = normalize(decodeURIComponent(url.pathname)).replace(/^\/+/, '');
    const file = join(dist, path && extname(path) ? path : 'index.html');
    try { return await route.fulfill({ status: 200, body: await readFile(file), contentType: TYPES[extname(file)] ?? 'application/octet-stream' }); }
    catch { log.push(`[${tag}] нет файла: ${url.pathname}`); return route.fulfill({ status: 404, body: '' }); }
  });
  return { context, page, tag };
}

try {
  browser = await chromium.launch({
    executablePath: process.env.HOH_CHROMIUM ?? '/usr/bin/google-chrome', headless: true,
    env: { ...process.env, HOME: home, XDG_CONFIG_HOME: join(home, 'config'), XDG_CACHE_HOME: join(home, 'cache') },
    args: ['--no-sandbox', '--disable-gpu', '--disable-background-networking', '--disable-component-update', '--no-first-run', '--force-color-profile=srgb'],
  });
  const measures = [];
  for (const [width, height] of SIZES) for (const scale of SCALES) {
    const name = `${width}x${height}-${scale.tag}`;
    // 1. заставка (запрос сессии не отвечает)
    let s = await open(width, height, scale, { hold: true });
    await s.page.goto(ORIGIN + '/'); await s.page.waitForSelector('#boot-shell-logo'); await s.page.evaluate(() => document.getElementById('boot-shell-logo').decode());
    const logo = await s.page.locator('#boot-shell-logo').boundingBox();
    const vw = await s.page.evaluate(() => [innerWidth, innerHeight]);
    measures.push({ name, screen: 'splash', logo, viewport: vw, centerOffset: [logo.x + logo.width / 2 - vw[0] / 2, logo.y + logo.height / 2 - vw[1] / 2] });
    await s.page.screenshot({ path: join(out, `01-splash-${name}.png`) });
    // смена размера окна: центр заставки по окну в любом кадре
    const jumps = [];
    for (const [w, h] of [[width + 400, height + 250], [Math.round(width / scale.zoom / 2), Math.round(height / scale.zoom / 2)], [Math.round(width / scale.zoom), Math.round(height / scale.zoom)]]) {
      await s.page.setViewportSize({ width: w, height: h }); await s.page.waitForTimeout(50);
      const [b, v] = await s.page.evaluate(() => { const r = document.getElementById('boot-shell-logo')?.getBoundingClientRect(); return [r ? { x: r.x, y: r.y, width: r.width, height: r.height } : { x: NaN, y: NaN, width: 0, height: 0 }, [innerWidth, innerHeight]]; });
      jumps.push({ viewport: v, centerOffset: [b.x + b.width / 2 - v[0] / 2, b.y + b.height / 2 - v[1] / 2] });
      if (w === width + 400) await s.page.screenshot({ path: join(out, `01b-splash-resized-${name}.png`) });
    }
    measures.push({ name, screen: 'splash-resize', jumps });
    await s.context.close();
    // 2. Welcome (пустой хост), 3. главный (проект есть, разговора нет), 4. пустой разговор в папке
    s = await open(width, height, scale, { seed: false });
    await s.page.goto(ORIGIN + '/'); await s.page.waitForTimeout(3500);
    await s.page.screenshot({ path: join(out, `02-welcome-${name}.png`) });
    const h1 = await s.page.locator('h1').first().boundingBox().catch(() => null);
    const em = await s.page.evaluate(() => { const h = document.querySelector('div.max-w-3xl > h1.text-2xl'); if (!h) return null; const r = h.getBoundingClientRect(); return { h1: [r.x, r.y] }; });
    measures.push({ name, screen: 'welcome', h1, em }); await s.context.close();
    s = await open(width, height, scale, { seed: true });
    await s.page.goto(ORIGIN + '/'); await s.page.waitForTimeout(3500);
    await s.page.screenshot({ path: join(out, `03-main-${name}.png`) });
    const empty = await s.page.evaluate(() => {
      const c = document.querySelector('div.mx-auto.max-w-5xl.flex-col.items-center, [data-slot="empty-header"]'); if (!c) return null;
      const r = c.getBoundingClientRect(); const cs = getComputedStyle(c, '::before');
      const header = document.querySelector('header, [data-slot="topbar"]')?.getBoundingClientRect();
      return { box: [r.x, r.y, r.width, r.height], before: [cs.display, cs.width, cs.height], headerBottom: header?.bottom ?? null, title: document.querySelector('h1,[data-slot="empty-title"]')?.textContent?.slice(0, 60) };
    });
    measures.push({ name, screen: 'main', empty });
    const fitState = await s.page.evaluate(() => { const c = document.querySelector('[data-hoh-emblem]'); const t = document.querySelector('h1,[data-slot="empty-title"]'); return { emblemShown: !!c, titleTop: t?.getBoundingClientRect().top ?? null, titleVisible: !!t && t.getBoundingClientRect().top >= 40 }; });
    measures.push({ name, screen: 'fit', ...fitState });
    // шапка боковой панели: вырезка ×4
    const brand = s.page.locator('a[href="/"]').first();
    const box = await brand.boundingBox();
    if (box) {
      const clip = { x: Math.max(0, box.x - 8), y: Math.max(0, box.y - 8), width: 120, height: Math.min(56, box.height + 16) };
      await s.page.screenshot({ path: join(out, `05-sidebar-header-${name}.png`), clip });
    }
    await s.context.close();
  }
  await writeFile(join(out, 'measures.json'), JSON.stringify(measures, null, 1));
} finally {
  await browser?.close();
  await rm(home, { recursive: true, force: true });
  await writeFile(join(out, 'page-log.txt'), log.join('\n') + '\n');
}
console.log(log.join('\n'));
