// The built T3 window with the real HoH web host and a per-session fake engine.
// Exercises the project controls the person sees, including an empty project and a page restart.
import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, extname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright-core';
import { FakeEngine } from '../tests/hoh-host/fake-engine.mjs';

const UI = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const ORIGIN = 'http://hoh-project.test';
const ROOT = '/home/person/Projects/site';
const OTHER_ROOT = '/home/person/Projects/tools';
const EXISTING_ROOT = '/home/person/Projects/older-project';
const TYPES = { '.js': 'text/javascript', '.css': 'text/css', '.html': 'text/html', '.png': 'image/png', '.svg': 'image/svg+xml', '.woff2': 'font/woff2', '.woff': 'font/woff' };
const engine = new FakeEngine();
engine.clock = Date.now();
engine.seed('existing-chat', 'Existing conversation', [
  ['user/message', { id: 'u1', source: { kind: 'user' }, content: [{ type: 'text', text: 'Existing conversation' }] }],
  ['turn/start', { turn: 1 }], ['turn/end', { turn: 1, reason: { kind: 'completed' } }],
]).cwd = EXISTING_ROOT;
const home = await mkdtemp(join(tmpdir(), 'hoh-project-flow-'));
let browser;
try {
  browser = await chromium.launch({ executablePath: process.env.HOH_CHROMIUM ?? '/usr/bin/google-chrome', headless: true,
    env: { ...process.env, HOME: home, XDG_CONFIG_HOME: join(home, 'config'), XDG_CACHE_HOME: join(home, 'cache') },
    args: ['--no-sandbox', '--disable-gpu', '--disable-background-networking', '--disable-component-update', '--no-first-run'] });
  const context = await browser.newContext({ viewport: { width: 1440, height: 900 }, locale: 'en-US', reducedMotion: 'reduce' });
  const page = await context.newPage();
  page.setDefaultTimeout(15_000);
  await page.route('**/*', async route => {
    const url = new URL(route.request().url());
    if (url.origin !== ORIGIN) return route.abort();
    const file = extname(url.pathname) ? url.pathname : '/index.html';
    try { return await route.fulfill({ body: await readFile(join(UI, 'dist', file)), contentType: TYPES[extname(file)] ?? 'application/octet-stream' }); }
    catch { return route.fulfill({ status: 404, body: '' }); }
  });
  await page.exposeBinding('__projectRequest', (_, method, params) => engine.request(method, params));
  await page.exposeBinding('__projectBrowse', (_, params) => engine.browseFolders(params));
  await page.addInitScript(() => {
    const listeners = new Set();
    window.__projectNotify = notification => { for (const listener of listeners) listener(notification); };
    window.__HOH_T3_TEST_ENGINE__ = {
      request: (method, params) => window.__projectRequest(method, params),
      browseFolders: params => window.__projectBrowse(params),
      subscribe: async listener => { listeners.add(listener); return () => listeners.delete(listener); },
      subscribeStatus: async listener => { setTimeout(() => listener({ state: 'ready' }), 0); return () => {}; },
      subscribeQuestions: async () => () => {}, answerQuestion: async () => {}, decideApproval: async () => {},
    };
  });
  const off = await engine.subscribe(notification => { page.evaluate(value => window.__projectNotify?.(value), notification).catch(() => {}); });
  await page.goto(ORIGIN + '/');
  await page.locator('.hoh-card').first().waitFor();
  const projectButton = page.locator('[data-hoh-chat-project]');
  const projectMenu = page.locator('[data-hoh-chat-project-menu]');
  const addProject = async (path, name) => {
    await projectButton.click();
    await projectMenu.getByRole('menuitem', { name: 'Add project', exact: true }).click();
    await page.getByText('Local folder', { exact: true }).click();
    const input = page.getByRole('combobox').last();
    await input.fill(path);
    await input.press('Control+Enter');
    await projectButton.getByText(name, { exact: true }).waitFor();
  };
  await addProject(ROOT, 'site');
  const folder = page.getByTestId('sidebar-project-folder').filter({ hasText: 'site' });
  await folder.waitFor();
  assert.equal(engine.requests.filter(r => r.method === 'session/prompt').length, 0, 'empty project appears before any conversation');
  await addProject(OTHER_ROOT, 'tools');
  const composer = page.getByRole('textbox', { name: /message|ask/i }).last();
  await composer.fill('Work in this project');
  await projectButton.press('Enter');
  await projectMenu.waitFor();
  assert.match(await projectMenu.locator('[data-hoh-chat-cwd]').innerText(), /\/home\/person\/Projects\/tools/);
  await page.keyboard.press('Escape');
  await projectMenu.waitFor({ state: 'hidden' });
  assert(await projectButton.evaluate(node => node === document.activeElement), 'Escape restores focus to the project button');
  await projectButton.press('ArrowDown');
  const site = projectMenu.getByRole('menuitem', { name: /^site\b/ });
  await site.waitFor();
  for (let i = 0; i < 6 && await site.getAttribute('data-highlighted') === null; i++) {
    await page.keyboard.press('ArrowDown');
  }
  assert.notEqual(await site.getAttribute('data-highlighted'), null, 'arrow keys highlight the project');
  await page.keyboard.press('Enter');
  await projectButton.getByText('site', { exact: true }).waitFor();
  assert.equal(await composer.innerText(), 'Work in this project', 'switching keeps the unsent prompt');
  await composer.press('Enter');
  await page.waitForFunction(() => document.body.innerText.includes('Work in this project'));
  const change = engine.requests.find(r => r.method === 'session/working-directory/set' && r.params.path === ROOT);
  assert(change, 'the host sets the selected folder before the turn');
  await page.waitForTimeout(150);
  const prompt = engine.requests.find(r => r.method === 'session/prompt' && r.params.sessionId === change.params.sessionId);
  assert(prompt, 'the same engine session receives the turn');
  assert.equal(engine.store.get(change.params.sessionId).cwd, ROOT);
  assert.deepEqual(engine.starts.find(start => start.sessionId === change.params.sessionId), { sessionId: change.params.sessionId, executor: 'claude', cwd: ROOT });
  assert(engine.requests.indexOf(change) < engine.requests.indexOf(prompt));
  await page.waitForURL(`${ORIGIN}/hoh-local/${change.params.sessionId}`);
  await projectButton.click();
  await projectMenu.getByRole('menuitem', { name: /^tools\b/ }).click();
  await projectButton.getByText('tools', { exact: true }).waitFor();
  assert.match(page.url(), /\/draft\//, 'switching from a started chat opens a separate draft');
  assert.equal(engine.store.get(change.params.sessionId).cwd, ROOT, 'the started chat stays in its folder');
  await page.goto(`${ORIGIN}/hoh-local/existing-chat`);
  await projectButton.getByText('older-project', { exact: true }).waitFor();
  await projectButton.click();
  assert((await projectMenu.locator('[data-hoh-chat-cwd]').innerText()).includes(EXISTING_ROOT), 'the folder comes from the session even without a named project');
  await page.keyboard.press('Escape');
  await page.reload();
  await page.getByTestId('sidebar-project-folder').filter({ hasText: 'site' }).waitFor();
  await page.goto(`${ORIGIN}/hoh-local/${change.params.sessionId}`);
  await page.getByRole('navigation', { name: 'Chat breadcrumb' }).getByText('site', { exact: true }).waitFor();
  off();
  await context.close();
  console.log('project flow: header folder, keyboard navigation, add project, draft selection, started chat and restart passed');
} finally {
  await browser?.close();
  await rm(home, { recursive: true, force: true });
}
