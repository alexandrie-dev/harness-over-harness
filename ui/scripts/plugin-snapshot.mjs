// HOH-42 plugin design-round matrix: A/B/C list and full detail at three requested viewports.
import { chromium } from 'playwright-core';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, extname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { FakeEngine } from '../tests/chat/fake-engine.mjs';

const UI = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const DIST = process.env.HOH_T3_DIST ? resolve(process.env.HOH_T3_DIST) : join(UI, 'dist');
const OUT = resolve(process.argv[2] ?? join(UI, 'test-results/plugins'));
const ORIGIN = 'http://hoh-window.test';
const SIZES = [{ width: 1440, height: 900 }, { width: 1920, height: 1080 }, { width: 800, height: 550 }];
const VARIANTS = ['A', 'B', 'C'];
const TYPES = { '.js': 'text/javascript', '.mjs': 'text/javascript', '.css': 'text/css', '.html': 'text/html', '.png': 'image/png',
  '.svg': 'image/svg+xml', '.woff2': 'font/woff2', '.woff': 'font/woff', '.ttf': 'font/ttf', '.json': 'application/json', '.wasm': 'application/wasm' };

function sampleEngine() {
  const engine = new FakeEngine();
  engine.access = 'confirm';
  engine.claude = { installed: true, version: '2.1.0', auth: 'authenticated', models: [{ id: 'opus', displayName: 'Claude Opus', isDefault: true }] };
  const session = engine.open('plugin-round', 'Plugin design round');
  engine.append(session, 'user/message', { id: 'plugin-u', source: { kind: 'user' }, content: [{ type: 'text', text: 'Design the plugins page.' }] });
  engine.append(session, 'turn/start', { turn: 1 });
  engine.append(session, 'assistant/message', { message: { id: 'plugin-a', content: [{ type: 'text', text: 'Ready for the design round.' }] } });
  engine.append(session, 'turn/end', { turn: 1, reason: { kind: 'completed' } });
  return engine;
}

async function open(browser, size) {
  const engine = sampleEngine();
  const context = await browser.newContext({ viewport: size, deviceScaleFactor: 1, locale: 'en-US', reducedMotion: 'reduce', colorScheme: 'dark' });
  const page = await context.newPage();
  page.setDefaultTimeout(20000);
  const problems = [];
  page.on('pageerror', (error) => problems.push(`pageerror: ${error.message}`));
  page.on('console', (message) => { if (message.type() === 'error') problems.push(`console: ${message.text().slice(0, 300)}`); });
  await page.route('**/*', async (route) => {
    const url = new URL(route.request().url());
    if (url.origin !== ORIGIN) { problems.push(`remote: ${url.href}`); return route.abort(); }
    const file = extname(url.pathname) ? url.pathname : '/index.html';
    try { return await route.fulfill({ body: await readFile(join(DIST, file)), contentType: TYPES[extname(file)] ?? 'application/octet-stream' }); }
    catch { problems.push(`missing: ${url.pathname}`); return route.fulfill({ status: 404, body: '' }); }
  });
  await page.exposeBinding('__hohT3Request', (_, method, params) => engine.request(method, params).catch((error) => ({ __hohError: String(error?.message ?? error) })));
  await page.addInitScript(() => {
    const listeners = new Set();
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
  await page.locator('[data-hoh-page-item="plugins"]').click();
  await page.locator('.hoh-plugins').waitFor();
  return { context, page, problems, off };
}

async function settle(page) {
  await page.evaluate(() => document.fonts.ready);
  await page.waitForTimeout(300);
  await page.mouse.move(0, 0);
}

async function choose(page, variant) {
  await page.getByRole('button', { name: `Variant ${variant}` }).click();
  await page.locator(`.hoh-plugins[data-plugin-variant="${variant}"]`).waitFor();
}

async function openDetail(page, variant) {
  if (variant === 'A') {
    await page.locator('.hoh-plugin-card[data-plugin-id="bluehour-signal-relay"]').click();
  } else if (variant === 'B') {
    const row = page.locator('.hoh-plugin-row[data-plugin-id="bluehour-signal-relay"]');
    await row.locator('.hoh-plugin-row__trigger').click();
    await row.getByRole('button', { name: 'Open plugin page' }).click();
  } else {
    await page.locator('.hoh-plugin-master__item').filter({ hasText: 'Signal Relay' }).click();
    await page.getByRole('button', { name: 'Open full page' }).click();
  }
  await page.locator('[data-plugin-detail="bluehour-signal-relay"]:not([data-embedded])').waitFor();
}

await mkdir(OUT, { recursive: true });
const browserHome = await mkdtemp(join(process.env.TMPDIR ?? tmpdir(), 'hoh-plugin-chromium-'));
const report = [];
let browser;
try {
  browser = await chromium.launch({
    executablePath: process.env.HOH_CHROMIUM ?? '/usr/bin/google-chrome', headless: true,
    env: { ...process.env, HOME: browserHome, XDG_CONFIG_HOME: join(browserHome, 'config'), XDG_CACHE_HOME: join(browserHome, 'cache') },
    args: ['--no-sandbox', '--disable-gpu', '--disable-audio-output', '--mute-audio', '--disable-background-networking', '--disable-component-update', '--no-first-run', '--disable-default-apps', '--force-color-profile=srgb'],
  });
  for (const variant of VARIANTS) {
    for (const size of SIZES) {
      const { context, page, problems, off } = await open(browser, size);
      for (const view of ['list', 'detail']) {
        const file = `plugins-${variant.toLowerCase()}-${view}-${size.width}x${size.height}.png`;
        let status = 'passed'; let error;
        try {
          if (view === 'list') await choose(page, variant);
          else await openDetail(page, variant);
          await settle(page);
          if (view === 'detail') {
            const startsAtTop = await page.evaluate(() => {
              const pageLayer = document.querySelector('.hoh-page')?.getBoundingClientRect();
              const top = document.querySelector('.hoh-plugins__top')?.getBoundingClientRect();
              return Boolean(pageLayer && top && top.top >= pageLayer.top - 1);
            });
            if (!startsAtTop) throw new Error('detail opened with the page scrolled past its header');
          }
          const overflow = await page.locator('.hoh-plugins').evaluate((node) => node.scrollWidth > node.clientWidth + 1);
          if (overflow) throw new Error('horizontal overflow');
          await page.screenshot({ path: join(OUT, file), animations: 'disabled' });
        } catch (caught) {
          status = 'failed'; error = String(caught?.message ?? caught).split('\n')[0];
          await page.screenshot({ path: join(OUT, file), animations: 'disabled' }).catch(() => {});
        }
        report.push({ variant, view, size: `${size.width}x${size.height}`, file, status, error, problems: [...problems] });
        console.log(`${status === 'passed' ? 'PASS' : 'FAIL'} ${file}${error ? ` — ${error}` : ''}${problems.length ? ` (${problems.length} problems)` : ''}`);
      }
      off(); await context.close();
    }
  }
} finally {
  await browser?.close();
  await rm(browserHome, { recursive: true, force: true });
}

await writeFile(join(OUT, 'report.json'), JSON.stringify(report, null, 2) + '\n');
const failed = report.filter((entry) => entry.status !== 'passed' || entry.problems.some((problem) => /^(remote|pageerror|console):/.test(problem)));
console.log(`${report.length - failed.length}/${report.length} plugin screens clean; report ${join(OUT, 'report.json')}`);
process.exit(failed.length ? 1 : 0);
