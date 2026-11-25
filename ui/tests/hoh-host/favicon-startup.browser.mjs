// Run after the T3 build: a stalled, optional favicon database must not block
// the shell or saved chat cards. Uses the existing fake engine, never an agent.
import assert from 'node:assert/strict';
import { readFile, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, extname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright-core';
import { FakeEngine } from '../chat/fake-engine.mjs';

const ui = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const dist = process.env.HOH_FAVICON_TEST_DIST || join(ui, 'dist');
const origin = 'http://hoh-window.test';
const types = { '.js': 'text/javascript', '.css': 'text/css', '.html': 'text/html', '.png': 'image/png', '.svg': 'image/svg+xml', '.woff2': 'font/woff2', '.json': 'application/json', '.wasm': 'application/wasm' };
const engine = new FakeEngine();
engine.claude = { installed: true, version: '2.1.0', auth: 'authenticated', models: [{ id: 'opus', displayName: 'Claude Opus', isDefault: true }] };
const chat = engine.open('favicon-test', 'Saved chat while icon cache is blocked');
engine.append(chat, 'turn/start', { turn: 1 });
engine.append(chat, 'user/message', { id: 'favicon-test-u', source: { kind: 'user' }, content: [{ type: 'text', text: 'Saved chat while icon cache is blocked' }] });
engine.append(chat, 'assistant/message', { message: { id: 'favicon-test-a', content: [{ type: 'text', text: 'History is available.' }] } });
engine.append(chat, 'turn/end', { turn: 1, reason: { kind: 'completed' } });
const home = await mkdtemp(join(tmpdir(), 'hoh-favicon-'));
let browser;
try {
  browser = await chromium.launch({
    executablePath: process.env.HOH_CHROMIUM ?? '/usr/bin/google-chrome', headless: true,
    env: { ...process.env, HOME: home, XDG_CONFIG_HOME: join(home, 'config'), XDG_CACHE_HOME: join(home, 'cache') },
    args: ['--no-sandbox', '--disable-gpu', '--disable-background-networking', '--disable-component-update', '--no-first-run'],
  });
  const context = await browser.newContext({ viewport: { width: 1440, height: 900 }, locale: 'en-US', reducedMotion: 'reduce' });
  const page = await context.newPage();
  await page.route('**/*', async (route) => {
    const url = new URL(route.request().url());
    if (url.origin !== origin) return route.abort();
    const file = extname(url.pathname) ? url.pathname : '/index.html';
    try { return await route.fulfill({ body: await readFile(join(dist, file)), contentType: types[extname(file)] ?? 'application/octet-stream' }); }
    catch { return route.fulfill({ status: 404, body: '' }); }
  });
  await page.exposeBinding('__hohT3Request', (_, method, params) => engine.request(method, params).catch((error) => ({ __hohError: String(error?.message ?? error) })));
  await page.addInitScript(() => {
    const open = IDBFactory.prototype.open;
    IDBFactory.prototype.open = function(name, ...args) {
      if (name === 't3code:project-favicons') {
        window.__hohFaviconBlocked = true;
        return new EventTarget(); // No success/error: simulate a stalled IndexedDB open.
      }
      return open.call(this, name, ...args);
    };
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
  try {
    await page.goto(origin + '/');
    await page.waitForFunction(() => window.__hohFaviconBlocked === true, null, { timeout: 20000 });
    const card = page.locator('.hoh-card').filter({ hasText: 'Saved chat while icon cache is blocked' });
    if (process.env.HOH_EXPECT_FAVICON_STALL === '1') {
      await assert.rejects(card.waitFor({ state: 'attached', timeout: 5000 }));
      console.log('control: stalled favicon database blocked the saved chat card');
    } else {
      await card.waitFor({ state: 'attached', timeout: 20000 });
      console.log('passed: saved chat card rendered while favicon IndexedDB remained blocked');
    }
  } finally {
    off();
    await context.close();
  }
} finally {
  if (browser) await browser.close();
  await rm(home, { recursive: true, force: true });
}
