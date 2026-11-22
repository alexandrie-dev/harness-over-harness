// Снимки основы HOH-40 (src/t3/base) без сервера и сети: страница открывается как http://hoh.local/, файлы сборки
// отдаёт перехват запросов Chromium (page.route), всё остальное отклоняется. «Пустой хост»: два ответа HTTP до сокета —
// как у хоста u3 (ui/src/hoh-host/http.ts: сессия и описание среды), остальное /api — 503; сокет T3 принимается и
// молчит (протокол данных не отвечает). Пишет снимки и журнал ошибок страницы.
// С третьим аргументом qa.html — страница сборки T3_BASE_QA=1: T3 на настоящем хосте u3 с пустым движком (сокет и
// HTTP отвечает хост в странице; перехват отдаёт только файлы).
//   node scripts/t3-snapshot.mjs <папка сборки> <папка снимков> [index.html|qa.html]
import { chromium } from 'playwright-core';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { extname, join, normalize, resolve } from 'node:path';

const [dist, out] = process.argv.slice(2, 4).map((p) => resolve(p));
const PAGE = process.argv[4] ?? 'index.html';
if (!dist || !out) throw new Error('usage: t3-snapshot.mjs <dist> <out>');
const ORIGIN = 'http://hoh.local';
const TYPES = { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.json': 'application/json', '.wasm': 'application/wasm',
  '.woff2': 'font/woff2', '.woff': 'font/woff', '.png': 'image/png', '.svg': 'image/svg+xml', '.mp3': 'audio/mpeg', '.ttf': 'font/ttf' };
const SIZES = [[1440, 900], [800, 550]];
// Ответы среды HoH до сокета (форма — ui/src/hoh-host/http.ts u3).
const API = {
  '/api/auth/session': { authenticated: true, scopes: ['orchestration:read', 'orchestration:operate', 'settings:write', 'providers:manage', 'filesystem:read', 'filesystem:write'],
    auth: { policy: 'desktop-managed-local', bootstrapMethods: ['desktop-bootstrap'], sessionMethods: ['browser-session-cookie'], sessionCookieName: 'hoh_session' },
    sessionMethod: 'browser-session-cookie' },
  '/.well-known/t3/environment': { environmentId: 'hoh-local', label: 'HoH', platform: { os: 'linux', arch: 'x64' }, serverVersion: '0.0.0-hoh',
    orchestrationProtocolVersion: 2, capabilities: { repositoryIdentity: false, connectionProbe: true, threadForceStop: true, inlineMessageContext: true } },
};

await mkdir(out, { recursive: true });
await mkdir('/dev/shm/hoh/u2', { recursive: true });
const home = await mkdtemp('/dev/shm/hoh/u2/chrome-');
const log = [];
let browser;
try {
  browser = await chromium.launch({
    executablePath: process.env.HOH_CHROMIUM ?? '/usr/bin/google-chrome',
    headless: true,
    env: { ...process.env, HOME: home, XDG_CONFIG_HOME: join(home, 'config'), XDG_CACHE_HOME: join(home, 'cache') },
    args: ['--no-sandbox', '--disable-gpu', '--disable-background-networking', '--disable-component-update', '--disable-domain-reliability',
      '--no-first-run', '--disable-default-apps', '--force-color-profile=srgb'],
  });
  for (const [width, height] of SIZES) {
    const context = await browser.newContext({ viewport: { width, height }, deviceScaleFactor: 1, reducedMotion: 'reduce', locale: 'en-US' });
    const page = await context.newPage();
    const tag = `${width}x${height}`;
    page.on('pageerror', (e) => log.push(`[${tag}] pageerror: ${e.message}`));
    page.on('console', (m) => { if (m.type() === 'error' || m.type() === 'warning') log.push(`[${tag}] console.${m.type()}: ${m.text()}`); });
    if (PAGE === 'index.html') await context.routeWebSocket(/.*/, (ws) => { log.push(`[${tag}] websocket: ${ws.url()} (принят, без ответа)`); });
    await context.route('**/*', async (route) => {
      const url = new URL(route.request().url());
      if (url.origin !== ORIGIN) { log.push(`[${tag}] отклонён: ${url.href}`); return route.abort(); }
      if (url.pathname in API) return route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(API[url.pathname]) });
      if (url.pathname.startsWith('/api/')) { log.push(`[${tag}] 503: ${url.pathname}`); return route.fulfill({ status: 503, body: '' }); }
      const path = normalize(decodeURIComponent(url.pathname)).replace(/^\/+/, '');
      const file = join(dist, path && !path.endsWith('/') && extname(path) ? path : PAGE);
      if (!file.startsWith(dist)) return route.abort();
      try {
        return await route.fulfill({ status: 200, body: await readFile(file), contentType: TYPES[extname(file)] ?? 'application/octet-stream' });
      } catch {
        log.push(`[${tag}] нет файла: ${url.pathname}`);
        return route.fulfill({ status: 404, body: '' });
      }
    });
    await page.goto(ORIGIN + '/');
    await page.evaluate(() => document.fonts.ready);
    await page.waitForTimeout(4000);
    await page.screenshot({ path: join(out, `${PAGE.replace('.html', '')}-${tag}.png`), animations: 'disabled' });
    log.push(`[${tag}] адрес: ${page.url()}; заголовок: ${await page.title()}`);
    log.push(`[${tag}] текст: ${(await page.locator('body').innerText()).replace(/\s+/g, ' ').slice(0, 400)}`);
    await context.close();
  }
} finally {
  await browser?.close();
  await rm(home, { recursive: true, force: true });
  await writeFile(join(out, `page-log-${PAGE.replace('.html', '')}.txt`), log.join('\n') + '\n');
}
console.log(log.join('\n'));
