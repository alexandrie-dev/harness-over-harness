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


if (!QA) throw new Error('QA_DIR is required');
await mkdir(QA, { recursive: true });
const chromeHome = await mkdtemp(join(tmpdir(), 'hoh-del-'));
const screenshot = (page, name) => page.screenshot({ path: join(QA, `${name}.png`), animations: 'disabled' });
let browser, session;
try {
  browser = await chromium.launch({ executablePath: '/usr/bin/google-chrome', headless: true,
    env: { ...process.env, HOME: chromeHome, XDG_CONFIG_HOME: join(chromeHome, 'config'), XDG_CACHE_HOME: join(chromeHome, 'cache'), PULSE_SERVER: 'unix:/nonexistent', PIPEWIRE_REMOTE: '/nonexistent', ALSA_CONFIG_PATH: '/dev/null' },
    args: ['--no-sandbox', '--disable-gpu', '--disable-background-networking', '--disable-component-update', '--no-first-run'] });
  const engine = completedEngine(); const trash = new Map();
  const request = engine.request.bind(engine);
  engine.request = async (method, params) => {
    if (method === 'session/delete') { if (engine.sessions.has(params.sessionId)) trash.set(params.sessionId, engine.sessions.get(params.sessionId)); engine.sessions.delete(params.sessionId); return {}; }
    return request(method, params);
  };
  session = await open(browser, engine, 2000, 1137, `/hoh-local/${CHAT.id}`, conversationStorage());
  const page = session.page;
  await page.getByText('The measured conversation lane is ready.').waitFor();
  const contextMenu = async () => { await page.locator('[data-thread-item]').first().click({ button: 'right' }); await page.getByRole('menu').last().waitFor(); };
  await contextMenu();
  const menu = page.getByRole('menu').last();
  await menu.getByText('Delete', { exact: true }).waitFor();
  assert.equal(await menu.getByText(/Delete.*Coming Soon/).count(), 0);
  await screenshot(page, '01-chat-menu');
  await menu.getByText('Delete', { exact: true }).click();
  let dialog = page.getByRole('alertdialog'); await dialog.waitFor();
  await dialog.getByText(/moves the chat to the trash/).waitFor();
  await screenshot(page, '02-delete-confirm');
  await dialog.getByRole('button', { name: 'Cancel' }).click();
  assert.equal(engine.sessions.has(CHAT.id), true);
  await contextMenu(); await page.getByRole('menu').last().getByText('Delete', { exact: true }).click();
  dialog = page.getByRole('alertdialog'); await dialog.waitFor();
  await dialog.getByRole('button', { name: 'Confirm' }).click();
  await page.waitForFunction(id => !window.__HOH_HOME__?.entries().some(entry => entry.id === id), CHAT.id);
  await page.waitForFunction(id => !location.pathname.endsWith('/' + id), CHAT.id);
  assert.equal(engine.sessions.has(CHAT.id), false); assert.equal(trash.has(CHAT.id), true);
  assert.equal(engine.requests.some(item => item.method === 'session/cancel'), false);
  await screenshot(page, '03-after-delete');
  await page.reload();
  await page.waitForFunction(id => !window.__HOH_HOME__?.entries().some(entry => entry.id === id), CHAT.id);
  assert.equal(await page.locator('[data-thread-item]').filter({ hasText: CHAT.title }).count(), 0);
  await screenshot(page, '04-after-restart');
  console.log('DEL browser: confirmation, cancellation, deletion, navigation and restart passed');
} catch (error) { if (session?.page) await screenshot(session.page, 'failure').catch(() => {}); throw error; }
finally { session?.off(); await session?.context.close(); await browser?.close(); await rm(chromeHome, { recursive: true, force: true }); }
