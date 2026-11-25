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
const screenshot = (page, name) => page.screenshot({ path: join(QA, `${name}.png`), animations: 'disabled' });
let browser, session;
try {
  browser = await chromium.launch({ executablePath: '/usr/bin/google-chrome', headless: true,
    env: { ...process.env, HOME: chromeHome, XDG_CONFIG_HOME: join(chromeHome, 'config'), XDG_CACHE_HOME: join(chromeHome, 'cache'), PULSE_SERVER: 'unix:/nonexistent', PIPEWIRE_REMOTE: '/nonexistent', ALSA_CONFIG_PATH: '/dev/null' },
    args: ['--no-sandbox', '--disable-gpu', '--disable-background-networking', '--disable-component-update', '--no-first-run'] });
  const engine = completedEngine(); const chat = engine.sessions.get(CHAT.id);
  chat.events.at(-1).data.usage = { inputTokens: 100, cacheCreationInputTokens: 20, cacheReadInputTokens: 30, outputTokens: 40, maxTokens: 200000 };
  engine.append(chat, 'user/message', { id: 'chat-u2', source: { kind: 'user' }, content: [{ type: 'text', text: 'Second turn' }] });
  engine.append(chat, 'turn/start', { turn: 2 });
  engine.append(chat, 'assistant/message', { message: { id: 'chat-a2', content: [{ type: 'text', text: 'Second answer' }] } });
  engine.append(chat, 'turn/end', { turn: 2, reason: { kind: 'completed' }, usage: { inputTokens: 200, cacheCreationInputTokens: 25, cacheReadInputTokens: 50, outputTokens: 60, maxTokens: 200000 } });
  engine.selected = { executor: 'claude', model: 'opus' }; engine.bind(CHAT.id); engine.settingSources = 'user'; let trusted = false;
  engine.answer = (target, messageId, text) => {
    const send = (type, data) => engine.emit('session.event', { sessionId: target.id, event: engine.append(target, type, data) });
    engine.emit('session.status', { sessionId: target.id, status: 'running' });
    send('user/message', { id: messageId, source: { kind: 'user' }, content: [{ type: 'text', text }] });
    send('turn/start', {});
    send('assistant/message', { message: { id: `a-${messageId}`, content: [{ type: 'text', text: `Answer: ${text}` }] } });
    send('turn/end', { reason: { kind: 'completed' } });
    engine.emit('session.status', { sessionId: target.id, status: 'idle' });
  };
  const originalRequest = engine.request.bind(engine);
  engine.request = async (method, params) => {
    if (method === 'hoh/executor/trust/get') return { cwd: params.cwd, executor: 'claude', trusted, changed: false };
    if (method === 'hoh/executor/trust/set') { trusted = true; return {}; }
    if (method === 'session/prompt') engine.settingSources = 'user,project,local';
    return originalRequest(method, params);
  };
  session = await open(browser, engine, 2000, 1137, '/', conversationStorage()); let { page } = session;
  await page.getByText(/Good (night|morning|afternoon|evening)/).first().waitFor();
  assert.equal(await page.locator('[data-thread-details-card]:visible').count(), 0);
  await screenshot(page, '01-home-no-panel');
  await page.goto(ORIGIN + `/hoh-local/${CHAT.id}`);
  await page.getByText('Second answer').waitFor();
  assert.equal(await page.locator('[data-thread-details-card]:visible').count(), 0);
  await page.getByRole('button', { name: 'Toggle chat details panel' }).click();
  const panel = page.locator('[data-thread-details-card]:visible'); await panel.waitFor();
  await panel.getByText('Tokens', { exact: true }).waitFor();
  await panel.getByText('275', { exact: true }).waitFor();
  await panel.getByText('425', { exact: true }).waitFor();
  await panel.getByText('60', { exact: true }).waitFor();
  await panel.getByText('100', { exact: true }).waitFor();
  await panel.getByText('2', { exact: true }).waitFor();
  await panel.getByText('opus', { exact: true }).waitFor();
  const panelText = await panel.innerText();
  assert.doesNotMatch(panelText, /[\u0400-\u04ff]/);
  assert.doesNotMatch(panelText, /Subagents|Skills|Background jobs|MCP servers/);
  assert.match(panelText, /Goal/); assert.match(panelText, /Supervised/);
  await screenshot(page, '02-chat-two-turns');
  await page.getByRole('button', { name: /Trust project settings in this folder/ }).click();
  await page.getByText(/will be enabled from the next message/).waitFor();
  await screenshot(page, '03-trust-pending');
  const editor = page.locator('[contenteditable=true]').first(); await editor.fill('Use project settings'); await editor.press('Enter');
  await page.getByText('Use project settings').first().waitFor();
  const inputRow = panel.locator('div.grid').filter({ hasText: 'Input' }).last();
  await inputRow.getByText('—', { exact: true }).waitFor();
  await inputRow.getByText('425', { exact: true }).waitFor();
  await page.evaluate(() => window.dispatchEvent(new Event('focus')));
  await page.getByText(/Project settings in this folder.*\): active/).waitFor();
  await screenshot(page, '04-trust-active');
  engine.bindings.get(CHAT.id).running = null;
  await page.reload(); await page.getByText(/Project settings in this folder.*\): active/).waitFor();
  await screenshot(page, '05-trust-restart');
  trusted = false;
  await page.evaluate(() => window.dispatchEvent(new Event('focus')));
  await page.getByRole('button', { name: /Trust project settings in this folder/ }).waitFor();
  await page.getByRole('button', { name: /Trust project settings in this folder/ }).click();
  await page.getByText(/will be enabled from the next message/).waitFor();
  assert.equal(await page.locator('[data-thread-details-card]:visible').count(), 1, 'open panel persists across restart');
  await page.getByRole('button', { name: 'Toggle chat details panel' }).click();
  assert.equal(await page.locator('[data-thread-details-card]:visible').count(), 0);
  await page.reload(); await page.getByText('Second answer').waitFor();
  assert.equal(await page.locator('[data-thread-details-card]:visible').count(), 0, 'closed panel persists across restart');
  await page.goto(ORIGIN + '/settings/appearance');
  const search = page.getByLabel('Search settings').first();
  await search.fill('Choose what to show');
  await page.getByText('Choose what to show', { exact: true }).first().waitFor();
  await screenshot(page, '06-settings-search');
  await page.getByText('Choose what to show', { exact: true }).first().click();
  const control = page.getByRole('button', { name: /Chat details:.*on/ }); await control.waitFor(); await control.click();
  await page.getByRole('menuitemcheckbox', { name: /Context window/ }).click();
  await page.reload(); await control.waitFor(); await control.click();
  assert.equal(await page.getByRole('menuitemcheckbox', { name: /Context window/ }).getAttribute('aria-checked'), 'false');
  await screenshot(page, '07-settings-restart');
  assert.deepEqual(session.problems, []);
  console.log('C7f browser: two turns, panel, settings, trust and restart passed');
} catch (error) { if (session?.page) await screenshot(session.page, 'failure').catch(() => {}); throw error; }
finally { session?.off(); await session?.context.close(); await browser?.close(); await rm(chromeHome, { recursive: true, force: true }); }
