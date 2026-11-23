// Built-window regression for HOH-45: T3 renders a Markdown image in an optimistic
// first message before the new conversation's subscribe/create RPCs reach the host.
// The lookup must stay pending until the authoritative cwd exists, then become an
// inline image without leaving and reopening the conversation.
import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, extname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright-core';
import { FakeEngine } from '../tests/chat/fake-engine.mjs';

const UI = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const DIST = join(UI, 'dist');
const ORIGIN = 'http://hoh-window.test';
const TYPES = {
  '.css': 'text/css', '.html': 'text/html', '.js': 'text/javascript', '.json': 'application/json',
  '.png': 'image/png', '.svg': 'image/svg+xml', '.wasm': 'application/wasm', '.woff2': 'font/woff2',
};
const PNG = 'iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAEElEQVR4nGP4z8AARAwQCgAf7gP9i18U1AAAAABJRU5ErkJggg==';
const IMAGE_TEXT = '![HoH first-turn image](shots/test-image.png)';

const engine = new FakeEngine();
engine.access = 'confirm';
engine.claude = {
  installed: true, version: '2.1.0', auth: 'authenticated',
  models: [{ id: 'opus', displayName: 'Claude Opus', isDefault: true }],
};
const originalRequest = engine.request.bind(engine);
let draftId;
let folderLookups = 0;
engine.request = async (method, params) => {
  if (method === 'session/working-directory/get' && !engine.sessions.has(params.sessionId)) {
    draftId = params.sessionId;
    folderLookups++;
    // Keep thread.create inside its real cwd lookup long enough to observe the
    // optimistic image query waiting, rather than returning a cacheable refusal.
    await new Promise(resolve => setTimeout(resolve, 350));
  }
  return originalRequest(method, params);
};
let nativeReads = 0;
engine.readImageAsset = async ({ sessionId, path }) => {
  nativeReads++;
  assert.equal(sessionId, draftId);
  assert.equal(path, '/workspace/chat/shots/test-image.png', 'the provisional workspace path was not rebased');
  return {
    dataBase64: PNG, mimeType: 'image/png', sizeBytes: Buffer.from(PNG, 'base64').length,
    sourcePath: '/workspace/chat/shots/test-image.png', fileRevision: 'first-turn-browser',
  };
};

const home = await mkdtemp(join(tmpdir(), 'hoh-first-turn-image-'));
let browser;
let context;
const problems = [];
try {
  browser = await chromium.launch({
    executablePath: process.env.HOH_CHROMIUM ?? '/usr/bin/google-chrome', headless: true,
    env: { ...process.env, HOME: home, XDG_CONFIG_HOME: join(home, 'config'), XDG_CACHE_HOME: join(home, 'cache') },
    args: ['--no-sandbox', '--disable-gpu', '--disable-background-networking', '--disable-component-update', '--no-first-run'],
  });
  context = await browser.newContext({ viewport: { width: 1440, height: 900 }, locale: 'en-US', reducedMotion: 'reduce', colorScheme: 'dark' });
  const page = await context.newPage();
  page.setDefaultTimeout(20_000);
  page.on('pageerror', error => problems.push(`pageerror: ${error.message}`));
  page.on('console', message => { if (message.type() === 'error') problems.push(`console: ${message.text().slice(0, 300)}`); });
  await page.route('**/*', async route => {
    const url = new URL(route.request().url());
    if (url.origin !== ORIGIN) { problems.push(`remote: ${url.href}`); return route.abort(); }
    const file = extname(url.pathname) ? url.pathname : '/index.html';
    try { return await route.fulfill({ body: await readFile(join(DIST, file)), contentType: TYPES[extname(file)] ?? 'application/octet-stream' }); }
    catch { return route.fulfill({ status: 404, body: '' }); }
  });
  await page.exposeBinding('__hohT3Request', (_, method, params) =>
    engine.request(method, params).catch(error => ({ __hohError: String(error?.message ?? error) })));
  await page.exposeBinding('__hohT3ReadImageAsset', (_, input) => engine.readImageAsset(input));
  await page.addInitScript(() => {
    const listeners = new Set();
    window.__HOH_T3_TEST_REQUESTS__ = [];
    window.__hohT3Notify = notification => { for (const listener of listeners) listener(notification); };
    window.__HOH_T3_TEST_ENGINE__ = {
      request: async (method, params) => {
        const result = await window.__hohT3Request(method, params);
        if (result && result.__hohError) throw new Error(result.__hohError);
        return result;
      },
      readImageAsset: input => window.__hohT3ReadImageAsset(input),
      subscribe: async listener => { listeners.add(listener); return () => listeners.delete(listener); },
      subscribeStatus: async listener => { setTimeout(() => listener({ state: 'ready' }), 0); return () => {}; },
      subscribeQuestions: async () => () => {}, answerQuestion: async () => {}, decideApproval: async () => {},
    };
  });
  const off = engine.subscribe(notification => { page.evaluate(value => window.__hohT3Notify?.(value), notification).catch(() => {}); });
  try {
    await page.goto(ORIGIN + '/');
    await page.getByRole('button', { name: 'Start', exact: true }).click();
    await page.getByRole('button', { name: 'Start without a project', exact: true }).click();
    const editor = page.getByRole('textbox').last();
    await editor.fill(IMAGE_TEXT);
    await editor.press('Enter');

    try {
      await page.waitForFunction(() => {
        const trace = window.__HOH_T3_TEST_REQUESTS__ ?? [];
        return trace.some(entry => entry.method === 'assets.createUrl')
          && trace.some(entry => entry.method === 'orchestration.launchThread')
          && trace.some(entry => entry.method === 'orchestration.subscribeThread');
      });
    } catch (error) {
      const diagnostic = await page.evaluate(() => ({
        trace: window.__HOH_T3_TEST_REQUESTS__,
        body: document.body.innerText.slice(0, 2_000),
        url: location.href,
      }));
      throw new Error(`${error.message}\n${JSON.stringify(diagnostic)}`);
    }
    const trace = await page.evaluate(() => window.__HOH_T3_TEST_REQUESTS__);
    const assetIndex = trace.findIndex(entry => entry.method === 'assets.createUrl'
      && entry.payload?.resource?._tag === 'media-file'
      && /\/shots\/test-image\.png$/.test(entry.payload.resource.path));
    const tracedThreadId = trace[assetIndex]?.payload?.resource?.threadId;
    assert.equal(typeof tracedThreadId, 'string', `first-turn media request is missing: ${JSON.stringify(trace.slice(-20))}`);
    const createIndex = trace.findIndex(entry => entry.method === 'orchestration.launchThread'
      && entry.payload?.threadId === tracedThreadId);
    const subscribeIndex = trace.findIndex(entry => entry.method === 'orchestration.subscribeThread'
      && entry.payload?.threadId === tracedThreadId);
    assert(assetIndex >= 0 && assetIndex < createIndex, `assets.createUrl did not precede launchThread: ${JSON.stringify(trace.slice(-20))}`);
    assert(assetIndex < subscribeIndex, `assets.createUrl did not precede subscribeThread: ${JSON.stringify(trace.slice(-20))}`);

    const images = page.locator('img[alt="HoH first-turn image"]');
    await images.first().waitFor({ state: 'visible' });
    await page.waitForFunction(() => document.querySelectorAll('img[alt="HoH first-turn image"]').length >= 2);
    assert.equal(await page.getByText(/Image unavailable/).count(), 0, 'a cacheable first-frame refusal remained in the timeline');
    assert.equal(folderLookups, 1);
    assert.equal(nativeReads, 1, 'the shared user/assistant resource should use one native read');
    assert.equal(problems.length, 0, problems.join('\n'));
  } finally {
    off();
  }
} finally {
  await context?.close();
  await browser?.close();
  await rm(home, { recursive: true, force: true });
}

console.log('first-turn image: assets.createUrl preceded subscribe/launch and rendered inline without reopening');
