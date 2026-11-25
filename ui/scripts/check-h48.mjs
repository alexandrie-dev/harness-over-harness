// HOH-45 A: measured T3 alignment matrix. The browser uses the built dist directly (no server/network).
// QA_DIR optionally receives every requested screenshot plus TSV/JSON evidence; the build runs the same assertions without artifacts.
import assert from 'node:assert/strict';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, extname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright-core';
import { FakeEngine } from '../tests/chat/fake-engine.mjs';

const UI = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const DIST = join(UI, 'dist');
const QA = process.env.QA_DIR ? resolve(process.env.QA_DIR) : null;
const ORIGIN = 'http://hoh-align.test';
const TOL = 1;
const SIZES = [[2000, 1137], [1920, 1080], [1440, 900], [800, 550]].filter(([width, height]) => !process.env.ALIGN_SIZE || process.env.ALIGN_SIZE === `${width}x${height}`);
const SIDEBARS = ['expanded', 'collapsed'].filter(sidebar => !process.env.ALIGN_SIDEBAR || process.env.ALIGN_SIDEBAR === sidebar);
const TYPES = { '.js': 'text/javascript', '.css': 'text/css', '.html': 'text/html', '.png': 'image/png', '.svg': 'image/svg+xml', '.woff2': 'font/woff2', '.woff': 'font/woff', '.json': 'application/json', '.wasm': 'application/wasm' };
const CLAUDE = { installed: true, version: '2.1.0', auth: 'authenticated', models: [{ id: 'opus', displayName: 'Claude Opus', isDefault: true }] };
const CHAT = { id: 'chat-1', title: 'Align the release window' };
const rows = [];
const observations = [];
const hostRequests = [];
const failures = [];
const round = value => Math.round(value * 10) / 10;
const fail = (label, message) => failures.push(`${label}: ${message}`);

function box(rect) {
  return rect && { left: rect.left, top: rect.top, right: rect.right, bottom: rect.bottom, width: rect.width, height: rect.height };
}

function unionRects(...rects) {
  const present = rects.filter(Boolean);
  if (!present.length) return null;
  const left = Math.min(...present.map(rect => rect.left)); const top = Math.min(...present.map(rect => rect.top));
  const right = Math.max(...present.map(rect => rect.right)); const bottom = Math.max(...present.map(rect => rect.bottom));
  return { left, top, right, bottom, width: right - left, height: bottom - top };
}

function completedEngine() {
  const engine = new FakeEngine();
  const today = new Date(); today.setHours(12, 0, 0, 0); engine.clock = today.getTime();
  engine.access = 'confirm'; engine.claude = CLAUDE;
  const session = engine.open(CHAT.id, CHAT.title);
  engine.append(session, 'user/message', { id: 'chat-u', source: { kind: 'user' }, content: [{ type: 'text', text: 'Check every alignment edge.' }] });
  engine.append(session, 'turn/start', { turn: 1 });
  engine.append(session, 'assistant/message', { message: { id: 'chat-a', content: [{ type: 'text', text: 'The measured conversation lane is ready.' }] } });
  engine.append(session, 'turn/end', { turn: 1, reason: { kind: 'completed' } });
  return engine;
}

function busyEngine() {
  const engine = new FakeEngine();
  const today = new Date(); today.setHours(12, 0, 0, 0); engine.clock = today.getTime();
  engine.access = 'confirm'; engine.claude = CLAUDE;
  const start = (id, title, done = false) => {
    const session = engine.open(id, title);
    engine.append(session, 'user/message', { id: `${id}-u`, source: { kind: 'user' }, content: [{ type: 'text', text: title }] });
    engine.append(session, 'turn/start', { turn: 1 });
    engine.append(session, 'tool/call', { callId: `${id}-c`, turn: 1, name: 'Bash', arguments: { command: `check ${id}` } });
    if (done) {
      engine.append(session, 'assistant/message', { message: { id: `${id}-a`, content: [{ type: 'text', text: 'Done' }] } });
      engine.append(session, 'turn/end', { turn: 1, reason: { kind: 'completed' } });
    }
  };
  start('alpha-1', 'Prepare release notes'); start('alpha-2', 'Review API boundary'); start('alpha-done', 'Update onboarding', true);
  start('beta-1', 'Verify plugin loading'); start('scratch-1', 'Plan the next milestone');
  start('gamma-done', 'Archive completed migration', true);
  return engine;
}

function storage(grouped) {
  const at = Date.now() - 60_000;
  return {
    't3code:theme': 'hoh', 't3code:theme-appearance-mode': 'dark', chat_thread_sidebar_open: 'true',
    'hoh-home.group-agents-by-project.v1': String(grouped),
    'hoh-host.projects.v1': JSON.stringify({
      version: 1, defaultFolder: '/work/no-project',
      projects: [
        { id: 'project-alpha', title: 'Alexandrie', workspaceRoot: '/work/alexandrie', createdAt: at, updatedAt: at, pinnedAt: null, archivedAt: null },
        { id: 'project-beta', title: 'Harness over Harness', workspaceRoot: '/work/hoh', createdAt: at, updatedAt: at, pinnedAt: null, archivedAt: null },
        { id: 'project-gamma', title: 'Archive', workspaceRoot: '/work/archive', createdAt: at, updatedAt: at, pinnedAt: null, archivedAt: null },
      ],
      threads: { 'alpha-1': 'project-alpha', 'alpha-2': 'project-alpha', 'alpha-done': 'project-alpha', 'beta-1': 'project-beta', 'scratch-1': 'hoh-project', 'gamma-done': 'project-gamma' },
      homeDispositions: { 'alpha-done': { turnOrdinal: 1, state: 'done' }, 'gamma-done': { turnOrdinal: 1, state: 'waiting', stopped: true } },
    }),
  };
}

function conversationStorage() {
  const stored = storage(false);
  const projects = JSON.parse(stored['hoh-host.projects.v1']);
  projects.threads[CHAT.id] = 'project-alpha';
  // The engine default can be the same folder the person explicitly added as a project.
  projects.defaultFolder = '/work/alexandrie';
  stored['hoh-host.projects.v1'] = JSON.stringify(projects);
  return stored;
}

async function open(browser, engine, width, height, path = '/', stored = {}) {
  const context = await browser.newContext({ viewport: { width, height }, deviceScaleFactor: 1, locale: 'en-US', reducedMotion: 'reduce', colorScheme: 'dark' });
  const page = await context.newPage();
  page.setDefaultTimeout(30_000);
  const problems = [];
  page.on('pageerror', error => problems.push(`pageerror: ${error.message}`));
  page.on('console', message => { if (message.type() === 'error') problems.push(`console: ${message.text().slice(0, 240)}`); });
  await page.route('**/*', async route => {
    const url = new URL(route.request().url());
    if (url.origin !== ORIGIN) { problems.push(`remote: ${url.href}`); return route.abort(); }
    const file = extname(url.pathname) ? url.pathname : '/index.html';
    try { return await route.fulfill({ body: await readFile(join(DIST, file)), contentType: TYPES[extname(file)] ?? 'application/octet-stream' }); }
    catch { return route.fulfill({ status: 404, body: '' }); }
  });
  await page.exposeBinding('__hohT3Request', (_, method, params) => engine.request(method, params).catch(error => ({ __hohError: String(error?.message ?? error) })));
  if (engine.readImageAsset) await page.exposeBinding('__hohT3ReadImageAsset', (_, input) => engine.readImageAsset(input));
  await page.addInitScript(({ stored, canReadImage }) => {
    for (const [key, value] of Object.entries(stored)) if (localStorage.getItem(key) === null) localStorage.setItem(key, value);
    window.__HOH_T3_TEST_REQUESTS__ = [];
    const listeners = new Set();
    window.__hohT3Notify = notification => { for (const listener of listeners) listener(notification); };
    window.__HOH_T3_TEST_ENGINE__ = {
      ...(canReadImage ? { readImageAsset: input => window.__hohT3ReadImageAsset(input) } : {}),
      request: async (method, params) => { const result = await window.__hohT3Request(method, params); if (result?.__hohError) throw new Error(result.__hohError); return result; },
      subscribe: async listener => { listeners.add(listener); return () => listeners.delete(listener); },
      subscribeStatus: async listener => { setTimeout(() => listener({ state: 'ready' }), 0); return () => {}; },
      subscribeQuestions: async () => () => {}, answerQuestion: async () => {}, decideApproval: async () => {},
    };
  }, { stored, canReadImage: Boolean(engine.readImageAsset) });
  const off = engine.subscribe(notification => { page.evaluate(value => window.__hohT3Notify?.(value), notification).catch(() => {}); });
  await page.goto(ORIGIN + path);
  return { context, page, problems, off };
}

if (!QA) throw new Error("QA_DIR is required");
await mkdir(QA, { recursive: true });
const chromeHome = await mkdtemp(join(tmpdir(), 'hoh-h48-'));
const screenshot = async (page, name) => {
  const visibleWords = await page.evaluate(() => {
    const errors = [];
    const word = /\bthreads?\b/i;
    for (const node of document.querySelectorAll('body *')) {
      const style = getComputedStyle(node);
      if (style.display === 'none' || style.visibility === 'hidden' || !node.getClientRects().length) continue;
      for (const child of node.childNodes) if (child.nodeType === Node.TEXT_NODE && word.test(child.textContent ?? '')) errors.push(child.textContent.trim());
      for (const name of ['aria-label', 'aria-description', 'title', 'placeholder']) {
        const text = node.getAttribute(name);
        if (text && word.test(text)) errors.push(`${name}: ${text}`);
      }
    }
    return errors;
  });
  await page.screenshot({ path: join(QA, `${name}.png`), animations: 'disabled' });
  assert.deepEqual(visibleWords, [], `visible thread terminology in ${name}`);
};
let browser, session;
try {
  browser = await chromium.launch({ executablePath: '/usr/bin/google-chrome', headless: true,
    env: { ...process.env, HOME: chromeHome, XDG_CONFIG_HOME: join(chromeHome, 'config'), XDG_CACHE_HOME: join(chromeHome, 'cache'), PULSE_SERVER: 'unix:/nonexistent', PIPEWIRE_REMOTE: '/nonexistent', ALSA_CONFIG_PATH: '/dev/null' },
    args: ['--no-sandbox', '--disable-gpu', '--disable-background-networking', '--disable-component-update', '--no-first-run'] });
  const engine = completedEngine();
  engine.bind(CHAT.id); engine.settingSources = 'user';
  let trusted = false;
  const originalRequest = engine.request.bind(engine);
  engine.request = async (method, params) => {
    if (method === 'hoh/executor/trust/get') return { cwd: params.cwd, executor: 'claude', trusted, changed: false };
    if (method === 'hoh/executor/trust/set') { trusted = true; return {}; }
    if (method === 'session/prompt') engine.settingSources = 'user,project,local';
    return originalRequest(method, params);
  };
  session = await open(browser, engine, 2000, 1137, `/hoh-local/${CHAT.id}`, conversationStorage());
  let { page } = session;
  await page.getByText('The measured conversation lane is ready.').waitFor();
  await page.waitForTimeout(1000);
  const folder = page.getByTestId('sidebar-project-folder').filter({ hasText: 'Alexandrie' });
  await folder.getByRole('button').first().click({ button: 'right' });
  await page.getByRole('menuitem', { name: 'Rename project', exact: true }).waitFor();
  await screenshot(page, '01-project-menu');
  await page.getByRole('menuitem', { name: 'Rename project', exact: true }).click();
  await page.getByRole('textbox', { name: 'Project name' }).fill('Renamed project');
  await screenshot(page, '02-project-rename-edit');
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  await page.getByTestId('sidebar-project-folder').filter({ hasText: 'Renamed project' }).waitFor();
  await screenshot(page, '03-project-renamed');
  await page.reload();
  await page.getByTestId('sidebar-project-folder').filter({ hasText: 'Renamed project' }).waitFor();
  await screenshot(page, '04-project-restart');
  const row = page.locator('[data-thread-item]').filter({ hasText: CHAT.title });
  await row.click({ button: 'right' });
  await page.getByRole('menuitem', { name: 'Rename chat', exact: true }).click();
  const rename = page.getByRole('textbox', { name: 'Chat title' });
  await rename.fill('Renamed from sidebar');
  await screenshot(page, '05-chat-rename-sidebar');
  await rename.press('Enter');
  await page.getByRole('navigation', { name: 'Chat breadcrumb' }).getByText('Renamed from sidebar').waitFor();
  await page.getByRole('navigation', { name: 'Chat breadcrumb' }).getByText('Renamed from sidebar').dblclick();
  const headerInput = page.getByRole('textbox', { name: 'Chat title' });
  await headerInput.fill('Renamed from header');
  await screenshot(page, '06-chat-rename-header');
  await headerInput.press('Enter');
  await page.getByRole('navigation', { name: 'Chat breadcrumb' }).getByText('Renamed from header').waitFor();
  await page.reload();
  await page.getByRole('navigation', { name: 'Chat breadcrumb' }).getByText('Renamed from header').waitFor();
  await screenshot(page, '07-chat-restart');
  await page.getByRole('button', { name: /Trust project settings in this folder/ }).click();
  await page.getByText(/will be enabled from the next message/).waitFor();
  await screenshot(page, '08-trust-pending');
  const editor = page.locator('[contenteditable=true]').first();
  await editor.fill('Test project settings on the next turn');
  await editor.press('Enter');
  await page.getByText('Test project settings on the next turn').first().waitFor();
  assert(engine.requests.some(request => request.method === 'session/prompt'));
  await page.evaluate(() => window.dispatchEvent(new Event('focus')));
  await page.getByText(/Project settings in this folder.*\): active(?:$| —)/).waitFor();
  await screenshot(page, '09-trust-active');
  await page.reload();
  await page.getByText(/Project settings in this folder.*\): active(?:$| —)/).waitFor();
  await screenshot(page, '10-trust-restart');
  await page.goto(ORIGIN + '/settings/appearance');
  const control = page.getByRole('button', { name: /Chat details:.*on/ });
  await control.waitFor(); await control.click();
  await screenshot(page, '11-settings-visibility');
  const item = page.getByRole('menuitemcheckbox', { name: /Context window/ });
  await item.click(); await page.reload();
  await control.waitFor(); await control.click();
  assert.equal(await page.getByRole('menuitemcheckbox', { name: /Context window/ }).getAttribute('aria-checked'), 'false');
  await screenshot(page, '12-settings-restart');
  await page.goto(ORIGIN + `/hoh-local/${CHAT.id}`);
  await page.getByText('The measured conversation lane is ready.').waitFor();
  const details = page.locator('[data-thread-details-card]');
  await details.waitFor();
  assert.equal(await details.getByText('Контекст', { exact: true }).count(), 0, 'disabled context must disappear from the panel');
  await page.reload();
  await details.waitFor();
  assert.equal(await details.getByText('Контекст', { exact: true }).count(), 0, 'disabled context must remain hidden after host restart');
  assert.deepEqual(session.problems, [], 'browser console or requests failed');
  session.off(); await session.context.close(); session = null;
  const noScratch = new FakeEngine(); noScratch.access = 'confirm'; noScratch.claude = CLAUDE;
  session = await open(browser, noScratch, 2000, 1137, '/', {});
  page = session.page;
  await page.getByRole('button', { name: 'Start', exact: true }).click();
  await page.getByRole('button', { name: 'Start without a project', exact: true }).click();
  await page.getByText(/Good (night|morning|afternoon|evening)/).first().waitFor();
  assert((await page.evaluate(() => window.__HOH_T3_TEST_REQUESTS__)).some(request => request.method === 'projects.ensureScratch'));
  assert.equal(await page.getByTestId('sidebar-project-folder').filter({ hasText: 'Chats' }).count(), 0);
  await screenshot(page, '13-no-scratch-chats');
  assert.deepEqual(session.problems, [], 'browser console or requests failed');
  session.off(); await session.context.close(); session = null;
  const scratch = completedEngine();
  const saved = conversationStorage(); const savedProjects = JSON.parse(saved['hoh-host.projects.v1']);
  savedProjects.threads[CHAT.id] = 'hoh-project'; saved['hoh-host.projects.v1'] = JSON.stringify(savedProjects);
  session = await open(browser, scratch, 2000, 1137, `/hoh-local/${CHAT.id}`, saved);
  page = session.page; await page.getByText('The measured conversation lane is ready.').waitFor();
  await page.getByTestId('sidebar-project-folder').filter({ hasText: 'Chats' }).waitFor();
  await screenshot(page, '14-scratch-chat');
  assert.deepEqual(session.problems, [], 'browser console or requests failed');
  console.log('HOH-48/49 browser scenarios passed');
} catch (error) {
  if (session?.page) await screenshot(session.page, 'failure').catch(() => {});
  throw error;
} finally {
  session?.off(); await session?.context.close(); await browser?.close(); await rm(chromeHome, { recursive: true, force: true });
}
