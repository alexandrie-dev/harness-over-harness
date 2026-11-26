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
    for (const [key, value] of Object.entries(stored)) localStorage.setItem(key, value);
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

async function settle(page) {
  await page.evaluate(() => document.fonts.ready);
  await page.waitForTimeout(250);
}

async function setSidebar(page, wanted) {
  const wrapper = page.locator('[data-slot="sidebar-wrapper"]');
  await wrapper.waitFor();
  const current = await wrapper.getAttribute('data-sidebar-state');
  if (current !== wanted) {
    await page.locator('[data-sidebar="trigger"]').last().click();
    await page.waitForFunction(state => document.querySelector('[data-slot="sidebar-wrapper"]')?.getAttribute('data-sidebar-state') === state, wanted);
  }
  await page.waitForTimeout(100);
}

async function readyBusy(page, engine, grouped) {
  await page.getByText(/Good (night|morning|afternoon|evening)/).first().waitFor();
  for (const id of ['alpha-1', 'alpha-2', 'beta-1', 'scratch-1']) engine.emit('session.status', { sessionId: id, status: 'running' });
  if (grouped) await page.locator('.hoh-project').first().waitFor({ state: 'attached' }); else await page.locator('.hoh-card').first().waitFor({ state: 'attached' });
}

function modeEngine(mode, running) {
  const engine = completedEngine();
  engine.access = mode;
  engine.sessionModes.set(CHAT.id, mode);
  if (running) {
    const session = engine.sessions.get(CHAT.id);
    engine.append(session, 'turn/start', { turn: 2 });
    engine.append(session, 'user/message', { id: 'running-u', source: { kind: 'user' }, content: [{ type: 'text', text: 'Keep working' }] });
    engine.bind(CHAT.id);
  }
  return engine;
}

async function assertModeChip(page, label) {
  const chip = page.getByRole('button', { name: `Permissions: ${label}`, exact: true });
  await chip.waitFor({ state: 'visible' });
  const row = page.locator('[data-chat-composer-stack]');
  if (!(await chip.evaluate((el, root) => !!el.closest(root), '[data-chat-composer-stack]'))) throw new Error('mode chip is outside composer');
  if (await row.getByText(label, { exact: true }).count() !== 1) throw new Error('mode must have a single visible chip in composer');
}

function titleRecoveryStorage() {
  const result = conversationStorage();
  const saved = JSON.parse(result['hoh-host.projects.v1']);
  saved.titles = { [CHAT.id]: 'Generated recovery title' };
  saved.sessions = [{ id: CHAT.id, title: 'Generated recovery title', projectId: 'project-alpha', executor: 'claude', model: 'opus', mode: 'full', createdAt: Date.now() - 60000, updatedAt: Date.now() - 10000, archivedAt: null }];
  saved.threads['another-chat'] = 'project-alpha';
  result['hoh-host.projects.v1'] = JSON.stringify(saved);
  return result;
}
function titleRecoveryEngine() {
  const engine = completedEngine();
  engine.sessions.get(CHAT.id).title = '';
  const another = engine.open('another-chat', 'Another conversation');
  engine.append(another, 'user/message', { id: 'another-u', source: { kind: 'user' }, content: [{ type: 'text', text: 'Another conversation' }] });
  engine.append(another, 'turn/end', { turn: 1, reason: { kind: 'completed' } });
  return engine;
}
function runningRecoveryEngine() {
  const engine = completedEngine();
  engine.access = 'full';
  engine.sessionModes.set(CHAT.id, 'full');
  const session = engine.sessions.get(CHAT.id);
  engine.append(session, 'turn/start', { turn: 2 });
  engine.append(session, 'user/message', { id: 'running-u', source: { kind: 'user' }, content: [{ type: 'text', text: 'Still running' }] });
  engine.bind(CHAT.id);
  return engine;
}

function trustEngine(changed = false) {
  const engine = completedEngine(); engine.bind(CHAT.id);
  const request = engine.request.bind(engine);
  engine.request = async (method, params) => method === 'hoh/executor/trust/get'
    ? { cwd: params.cwd, executor: params.executor, trusted: false, changed }
    : request(method, params);
  return engine;
}

const scenarios = [
  {
    name: 'agent-setup', engine: () => { const e = new FakeEngine(); e.access = 'confirm'; e.claude = { installed: false, version: null, auth: 'unknown', models: [] }; return e; }, stored: () => ({}),
    prepare: async (page) => {
      await page.getByText('Set up an agent', { exact: true }).click();
      await page.getByText('Coming Soon', { exact: true }).first().waitFor();
      if (await page.getByRole('button', { name: /^Install:/ }).count()) throw new Error('Unsupported agent installation button is visible');
      if (await page.getByText('This connection cannot control terminals.').count()) throw new Error('Unsupported terminal error is visible');
    }, blocks: ['onboarding'],
  },
  {
    name: 'agent-signin', engine: () => { const e = new FakeEngine(); e.access = 'confirm'; e.claude = { installed: true, version: '2.1.0', auth: 'unauthenticated', models: [] }; return e; }, stored: () => ({}),
    prepare: async (page) => {
      await page.getByText('Coming Soon', { exact: true }).first().waitFor();
      if (await page.getByRole('button', { name: /^Sign in:/ }).count()) throw new Error('Unsupported agent sign-in button is visible');
      if (await page.getByText('This connection cannot control terminals.').count()) throw new Error('Unsupported terminal error is visible');
    }, blocks: ['onboarding'],
  },
  {
    name: 'home-empty', engine: () => { const e = new FakeEngine(); e.access = 'confirm'; e.claude = CLAUDE; return e; }, stored: () => ({}),
    prepare: async (page, _engine, sidebar) => {
      await page.getByRole('button', { name: 'Start', exact: true }).click();
      await page.getByRole('button', { name: 'Start without a project', exact: true }).waitFor();
      await setSidebar(page, sidebar);
      assert.equal(await page.locator('#hoh-notch').count(), 0, 'the top notch must be absent');
      if (sidebar === 'expanded') {
        const logo = page.getByRole('link', { name: 'HoH home' });
        await logo.locator('img').waitFor({ state: 'visible' });
        assert.equal((await logo.innerText()).trim(), '', 'the sidebar logo has no visible wordmark');
        await page.getByRole('button', { name: /^New chat/ }).first().click();
      } else {
        await page.keyboard.press('Control+Shift+O');
      }
      await page.locator('[data-chat-composer-stack]').waitFor({ state: 'visible' });
      const headline = page.locator('[data-chat-composer-stack] h1').first();
      const before = await headline.innerText();
      const viewport = page.viewportSize();
      await page.setViewportSize({ width: viewport.width + 7, height: viewport.height }); await page.waitForTimeout(50);
      await page.setViewportSize(viewport); await page.waitForTimeout(50);
      const after = await headline.innerText();
      if (before !== after || before !== 'What should we build in HoH?') throw new Error(`empty-home copy changed across resize: ${JSON.stringify({ before, after })}`);
    }, blocks: ['home'],
  },
  {
    name: 'composer-paste', engine: completedEngine, path: () => `/hoh-local/${CHAT.id}`, stored: conversationStorage,
    prepare: async (page, engine, sidebar) => {
      await page.getByText('The measured conversation lane is ready.').waitFor();
      await setSidebar(page, sidebar);
      const editor = page.getByTestId('composer-editor');
      await editor.focus();
      const screenshots = [
        'iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAEElEQVR4nGP4z8AARAwQCgAf7gP9i18U1AAAAABJRU5ErkJggg==',
        'iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAD0lEQVR4nGNg+M8AQhAKABvyA/1tVLjHAAAAAElFTkSuQmCC',
        'iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAD0lEQVR4nGNgYPgPRmAKABf2A/1+6zfzAAAAAElFTkSuQmCC',
      ];
      const paste = (text, file, png) => editor.evaluate((element, { text, file, png }) => {
        const clipboardData = new DataTransfer();
        if (text) clipboardData.setData('text/plain', text);
        if (file === 'text') clipboardData.items.add(new File(['clipboard contents'], 'clipboard.txt', { type: 'text/plain' }));
        if (file === 'image') {
          const bytes = Uint8Array.from(atob(png), character => character.charCodeAt(0));
          clipboardData.items.add(new File([bytes], 'clipboard.png', { type: 'image/png' }));
        }
        element.dispatchEvent(new ClipboardEvent('paste', { clipboardData, bubbles: true, cancelable: true }));
      }, { text, file, png });
      const short = 'Short clipboard text';
      await paste(short);
      assert.equal((await editor.innerText()).trim(), short);
      await editor.press('ControlOrMeta+a');
      const multiline = Array.from({ length: 35 }, (_, index) => `Clipboard line ${index + 1}`).join('\n');
      await paste(multiline, 'text');
      assert.equal((await editor.innerText()).trim().replace(/\n+/g, '\n'), multiline);
      const pendingImages = page.getByRole('button', { name: 'Remove clipboard.png', exact: true });
      for (const [index, png] of screenshots.entries()) {
        await paste('', 'image', png);
        await pendingImages.nth(index).waitFor();
      }
      await page.getByRole('button', { name: 'Submit message' }).click();
      await page.getByText(/Ответ на: Clipboard line 1/).waitFor();
      const prompt = engine.requests.find(({ method }) => method === 'session/prompt')?.params;
      assert.ok(prompt, 'pasted images reached session/prompt');
      const [first, second, third, text] = prompt.contentBlocks;
      assert.equal(prompt.contentBlocks.length, 4);
      assert.deepEqual([first, second, third], screenshots.map(data => ({
        type: 'image', data, mimeType: 'image/png', name: 'clipboard.png',
      })));
      assert.equal(text.type, 'text');
      assert.equal(text.text.replace(/\n+/g, '\n'), multiline);
    }, blocks: ['conversation'],
  },
  {
    name: 'home-flat', engine: busyEngine, stored: () => storage(false),
    prepare: async (page, engine, sidebar) => {
      await readyBusy(page, engine, false); await setSidebar(page, sidebar);
      await page.getByText('Stopped', { exact: true }).waitFor();
    }, blocks: ['home', 'cards'],
  },
  {
    name: 'home-from-project-draft', engine: busyEngine, stored: () => storage(false),
    prepare: async (page, engine, sidebar) => {
      await readyBusy(page, engine, false);
      await setSidebar(page, 'expanded');
      const newChat = page.getByRole('button', { name: /^New chat/ }).first();
      const home = page.getByRole('link', { name: 'HoH home' });
      const emptyChat = page.getByText('Send a message to start the conversation.', { exact: true });
      const card = page.locator('.hoh-card').first();
      for (let attempt = 0; attempt < 2; attempt++) {
        await newChat.click();
        await emptyChat.waitFor({ state: 'visible' });
        await home.click();
        await card.waitFor({ state: 'visible', timeout: 5000 });
      }
      await setSidebar(page, sidebar);
    }, blocks: ['home', 'cards'],
  },
  {
    name: 'home-projects', engine: busyEngine, stored: () => storage(true),
    prepare: async (page, engine, sidebar) => { await readyBusy(page, engine, true); await setSidebar(page, sidebar); }, blocks: ['home', 'projects'],
  },
  {
    name: 'home-project-expanded', engine: busyEngine, stored: () => storage(true),
    prepare: async (page, engine, sidebar) => { await readyBusy(page, engine, true); await setSidebar(page, sidebar); const folder = page.locator('.hoh-project').filter({ hasText: 'Alexandrie' }).first(); await folder.focus(); await page.keyboard.press('Enter'); await page.locator('.hoh-project-popover').waitFor();
      // Home scrolls the disclosure into view smoothly; measure its final position.
      await page.waitForFunction(() => {
        const panel = document.querySelector('.hoh-project-popover');
        const scroller = panel?.closest('.hoh-home');
        return panel && scroller && panel.getBoundingClientRect().bottom <= scroller.getBoundingClientRect().bottom + 1;
      }, null, { timeout: 5000 });
    }, blocks: ['home', 'projects', 'disclosure'],
  },
  {
    name: 'conversation', engine: completedEngine, path: () => `/hoh-local/${CHAT.id}`, stored: conversationStorage,
    prepare: async (page, _engine, sidebar) => {
      await page.getByText('The measured conversation lane is ready.').waitFor(); await setSidebar(page, sidebar);
      const breadcrumb = page.getByRole('navigation', { name: 'Chat breadcrumb' });
      if (await breadcrumb.getByText('Alexandrie', { exact: true }).count() !== 1 || await breadcrumb.getByText('No project', { exact: true }).count() !== 0)
        throw new Error(`project conversation breadcrumb is wrong: ${JSON.stringify(await breadcrumb.innerText())}`);
    }, blocks: ['conversation'],
  },
  {
    name: 'thread-menu', engine: completedEngine, path: () => `/hoh-local/${CHAT.id}`, stored: conversationStorage,
    prepare: async (page, _engine, sidebar) => {
      await page.getByText('The measured conversation lane is ready.').waitFor();
      await setSidebar(page, sidebar);
      if (sidebar === 'collapsed') return;
      await page.locator('[data-thread-item]').first().click({ button: 'right' });
      const menu = page.getByRole('menu').last();
      await menu.waitFor();
      const text = await menu.innerText();
      if (/Mark unread|Regenerate title|Snooze|Settle/.test(text)) throw new Error(`Unsupported thread action: ${text}`);
      if (await menu.getByText('Project settings', { exact: true }).count() !== 1) throw new Error('Project settings missing for a project');
    }, blocks: ['conversation'],
  },
  {
    name: 'thread-actions-restart', engine: completedEngine, path: () => `/hoh-local/${CHAT.id}`, stored: conversationStorage,
    prepare: async (page, _engine, sidebar) => {
      await page.getByText('The measured conversation lane is ready.').waitFor();
      await setSidebar(page, 'expanded');
      await page.locator('[data-thread-item]').first().click({ button: 'right' });
      await page.getByRole('menuitem', { name: 'Pin chat', exact: true }).click();
      await page.waitForFunction(id => JSON.parse(localStorage.getItem('hoh-host.thread-metadata.v1') ?? '{}')[id]?.pinnedAt != null, CHAT.id);
      await page.reload();
      await page.getByText('The measured conversation lane is ready.').waitFor();
      await setSidebar(page, 'expanded');
      await page.waitForTimeout(1000);
      await page.locator('[data-thread-item]').first().click({ button: 'right' });
      try { await page.getByRole('menuitem', { name: 'Unpin chat', exact: true }).waitFor({ timeout: 5000 }); }
      catch { throw new Error(`Unpin unavailable: ${await page.evaluate(() => [...document.querySelectorAll('[role=menu]')].map(x => x.innerText).join('|'))}; metadata: ${await page.evaluate(() => localStorage.getItem('hoh-host.thread-metadata.v1'))}`); }
      await page.getByRole('menuitem', { name: 'Delete', exact: true }).click();
      const confirmation = page.getByRole('alertdialog');
      await confirmation.getByText(/moves the chat to the trash/).waitFor();
      await confirmation.getByRole('button', { name: 'Cancel' }).click();
      const requests = await page.evaluate(() => window.__HOH_T3_TEST_REQUESTS__ ?? []);
      if (requests.some(({ method, payload }) => method === 'orchestration.dispatchCommand' && payload?.type === 'thread.delete')) throw new Error('Cancelled deletion was sent to the host');
      await setSidebar(page, sidebar);
    }, blocks: ['conversation'],
  },
  {
    name: 'thread-archive-restart', engine: completedEngine, path: () => `/hoh-local/${CHAT.id}`, stored: conversationStorage,
    prepare: async (page, _engine, sidebar) => {
      page.on('dialog', dialog => dialog.accept());
      await page.getByText('The measured conversation lane is ready.').waitFor();
      await setSidebar(page, 'expanded');
      await page.locator('[data-thread-item]').first().click({ button: 'right' });
      await page.getByRole('menuitem', { name: 'Archive chat', exact: true }).click();
      await page.waitForFunction(id => JSON.parse(localStorage.getItem('hoh-host.thread-metadata.v1') ?? '{}')[id]?.archivedAt != null, CHAT.id);
      await page.goto(`${ORIGIN}/settings/archived`);
      await page.reload();
      const unarchive = page.getByRole('button', { name: `Unarchive ${CHAT.title}`, exact: true });
      await unarchive.waitFor();
      await page.getByRole('button', { name: `Delete ${CHAT.title} · Coming Soon`, exact: true }).click();
      await page.getByText('Coming Soon', { exact: true }).first().waitFor();
      if (QA && page.viewportSize()?.width === 2000 && sidebar === 'expanded') await page.screenshot({ path: join(QA, 'archived-before-restore-2000x1137.png'), animations: 'disabled' });
      const requests = await page.evaluate(() => window.__HOH_T3_TEST_REQUESTS__ ?? []);
      if (requests.some(({ method, payload }) => method === 'orchestration.dispatchCommand' && payload?.type === 'thread.delete')) throw new Error('Unavailable archive delete was sent');
      await unarchive.click();
      await page.waitForFunction(id => JSON.parse(localStorage.getItem('hoh-host.thread-metadata.v1') ?? '{}')[id]?.archivedAt === null, CHAT.id);
      await page.reload();
      await page.getByText('No archived chats', { exact: true }).waitFor();
      await setSidebar(page, sidebar);
    }, blocks: ['settings'],
  },
  {
    name: 'thread-menu-no-project', engine: completedEngine, path: () => `/hoh-local/${CHAT.id}`, stored: () => ({}),
    prepare: async (page, _engine, sidebar) => {
      await page.getByText('The measured conversation lane is ready.').waitFor();
      await setSidebar(page, sidebar);
      if (sidebar === 'collapsed') return;
      await page.locator('[data-thread-item]').first().click({ button: 'right' });
      const menu = page.getByRole('menu').last();
      await menu.waitFor();
      if (await menu.getByText('Project settings', { exact: true }).count()) throw new Error('Project settings visible without a project');
    }, blocks: ['conversation'],
  },
  {
    name: 'file-link', engine: () => { const e = completedEngine(); const event = e.sessions.get(CHAT.id).events.find(item => item.type === 'assistant/message'); event.data.message.content = [{ type: 'text', text: 'See [README.md](/work/alexandrie/README.md).' }]; return e; }, path: () => `/hoh-local/${CHAT.id}`, stored: conversationStorage,
    prepare: async (page, _engine, sidebar) => {
      await page.getByText('README.md', { exact: true }).first().click();
      await setSidebar(page, sidebar);
      await page.getByText('Coming Soon', { exact: true }).first().waitFor();
      if (await page.getByText(/Not available in HoH|Loading files/i).count()) throw new Error('Unsupported file request is visible');
      const requests = await page.evaluate(() => (window.__HOH_T3_TEST_REQUESTS__ ?? []).map(({ method }) => method));
      if (requests.some(method => ['projects.readFile', 'projects.listEntries', 'filesystem.getMetadata'].includes(method))) throw new Error(`Unsupported file request was sent: ${requests.join(', ')}`);
    }, blocks: ['conversation'],
  },
  {
    name: 'image-file-link', engine: () => {
      const e = completedEngine();
      const event = e.sessions.get(CHAT.id).events.find(item => item.type === 'assistant/message');
      event.data.message.content = [{ type: 'text', text: 'See [diagram.png](/work/alexandrie/diagram.png).' }];
      const png = 'iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAEElEQVR4nGP4z8AARAwQCgAf7gP9i18U1AAAAABJRU5ErkJggg==';
      e.readImageAsset = async ({ path }) => ({ dataBase64: png, mimeType: 'image/png', sizeBytes: Buffer.from(png, 'base64').length, sourcePath: path, fileRevision: 'u1-test', decodeProfile: { canvasPixels: 4, decodedFramePixels: 4, animationPixelsPerSecond: 0, frameCount: 1 } });
      return e;
    }, path: () => `/hoh-local/${CHAT.id}`, stored: conversationStorage,
    prepare: async (page, _engine, sidebar) => {
      await page.locator('img[src^="blob:"]').first().waitFor();
      await setSidebar(page, sidebar);
      await page.locator('img[src^="blob:"]').first().click();
      await page.getByRole('dialog').last().waitFor();
      if (await page.getByText('Coming Soon', { exact: true }).count()) throw new Error('Supported image opened the file placeholder');
    }, blocks: ['conversation'],
  },
  {
    name: 'quick-view', engine: completedEngine, stored: () => ({}),
    prepare: async (page, _engine, sidebar) => { await page.locator(`.hoh-card`).first().waitFor(); await setSidebar(page, sidebar); await page.getByRole('button', { name: `Open ${CHAT.title}` }).click(); await page.getByRole('dialog').last().waitFor(); await page.getByRole('dialog').last().getByRole('textbox').waitFor(); await page.getByRole('dialog').last().getByRole('button', { name: /Permissions: Supervised/ }).waitFor(); }, blocks: ['quick'],
  },
  {
    name: 'quick-view-open', engine: completedEngine, stored: () => ({}),
    prepare: async (page, _engine, sidebar) => {
      await page.locator('.hoh-card').first().waitFor();
      await setSidebar(page, sidebar);
      await page.getByRole('button', { name: `Open ${CHAT.title}` }).click();
      await page.getByRole('dialog').last().getByRole('button', { name: `Open ${CHAT.title} in full conversation` }).click();
      await page.waitForURL(`**/hoh-local/${CHAT.id}`);
      await page.getByText('The measured conversation lane is ready.').waitFor();
      if (await page.locator('.hoh-quick-view').count()) throw new Error('Quick view remained open');
    }, blocks: ['conversation'],
  },
  {
    name: 'sharing-hidden', engine: completedEngine, path: () => '/settings/access', stored: () => ({}),
    prepare: async (page, _engine, sidebar) => {
      await page.waitForURL('**/settings/general');
      await page.locator('[data-settings-page-scroll]').waitFor();
      await setSidebar(page, sidebar);
      if (await page.getByRole('link', { name: /Sharing & access/ }).count()) throw new Error('Sharing remains in settings');
    }, blocks: ['settings'],
  },
  {
    name: 'settings', engine: completedEngine, path: () => '/settings/general', stored: () => ({}),
    prepare: async (page, _engine, sidebar) => { await page.locator('[data-settings-page-scroll]').waitFor(); await setSidebar(page, sidebar); }, blocks: ['settings'],
  },
  {
    name: 'thread-context', engine: () => {
      const engine = completedEngine();
      const session = engine.sessions.get(CHAT.id);
      session.events.at(-1).data.usage = { inputTokens: 1000, cacheCreationInputTokens: 0, cacheReadInputTokens: 0, outputTokens: 20, maxTokens: 200000 };
      return engine;
    }, path: () => `/hoh-local/${CHAT.id}`, stored: conversationStorage,
    prepare: async (page, _engine, sidebar) => {
      await page.getByText('The measured conversation lane is ready.').waitFor();
      await setSidebar(page, sidebar);
      const panel = page.locator('[data-thread-details-card]');
      if (!(await panel.isVisible().catch(() => false))) await page.getByRole('button', { name: 'Toggle chat details panel' }).click();
      await panel.getByText('Context', { exact: true }).waitFor();
      await panel.getByText('Used', { exact: true }).waitFor();
      // At narrow widths the details card is a popover and intentionally overlays the lane.
      // Measure the conversation after closing it, as in the other conversation scenarios.
      await page.keyboard.press('Escape');
      if (await panel.isVisible().catch(() => false)) {
        await page.getByRole('button', { name: 'Toggle chat details panel' }).click();
      }
      await panel.waitFor({ state: 'hidden' });
    }, blocks: ['conversation'],
  },
  {
    name: 'thread-details-menu', engine: completedEngine, path: () => '/settings/appearance', stored: () => ({}),
    prepare: async (page, _engine, sidebar) => {
      await page.locator('[data-settings-page-scroll]').waitFor();
      await setSidebar(page, sidebar);
      await page.getByRole('button', { name: /Chat details: .* of .* on/ }).click();
      const menu = page.getByRole('menu').last();
      await menu.getByText('Context window', { exact: true }).waitFor();
      if (await menu.getByRole('menuitemcheckbox').count() !== 1) throw new Error('Thread details must expose only Context window');
    }, blocks: ['settings'],
  },
  {
    name: 'plugins', engine: busyEngine, stored: () => storage(true),
    prepare: async (page, engine, sidebar) => {
      await readyBusy(page, engine, true); await page.locator('[data-hoh-page-item="plugins"]').click(); await page.locator('.hoh-plugins').waitFor();
      await page.locator('[data-plugin-empty]').waitFor(); await setSidebar(page, sidebar);
    }, blocks: ['plugins'],
  },
  {
    name: 'plugin-creator', engine: () => {
      const engine = completedEngine();
      engine.clock = Date.now();
      const append = engine.append.bind(engine);
      engine.append = (session, type, data) => append(session, type, type === 'assistant/message'
        ? { ...data, message: { ...data.message, content: [{ type: 'text', text: 'Should the clock tool return local time or UTC? I recommend UTC for consistent output.' }] } }
        : data);
      const request = engine.request.bind(engine);
      engine.request = async (method, params) => {
        const result = await request(method, params);
        return method === 'session/working-directory/set' ? { ...result, cwd: engine.cwd } : result;
      };
      return engine;
    }, stored: () => {
      const stored = storage(true);
      const key = 'hoh-host.projects.v1';
      const state = JSON.parse(stored[key]);
      Object.assign(state.projects[1], { title: 'Plugin drafts', workspaceRoot: '/work/plugin-drafts' });
      return { ...stored, [key]: JSON.stringify(state) };
    },
    prepare: async (page, engine, sidebar) => {
      await page.locator('[data-hoh-page-item="plugin-creator"]').click();
      await page.getByRole('heading', { name: 'What should your plugin do?' }).waitFor();
      await setSidebar(page, sidebar);
      const project = page.getByRole('combobox', { name: 'Plugin folder' });
      await project.click();
      await page.getByRole('option', { name: 'Plugin drafts' }).click();
      const harness = page.getByRole('combobox', { name: 'Harness', exact: true });
      await harness.press('ArrowDown');
      const listbox = page.getByRole('listbox');
      await listbox.waitFor();
      await page.getByRole('option', { name: 'Claude' }).waitFor();
      if (QA) await page.screenshot({ path: join(QA, `plugin-creator-picker-${page.viewportSize().width}-${sidebar}.png`), animations: 'disabled' });
      await page.keyboard.press('Escape');
      await listbox.waitFor({ state: 'hidden' });
      assert(await harness.evaluate(element => element === document.activeElement));
      await harness.press('ArrowDown');
      await page.keyboard.press('Home');
      await page.keyboard.press('Enter');
      await harness.getByText('Claude', { exact: true }).waitFor();
      await page.getByRole('button', { name: 'Summarize session activity' }).click();
      const composer = page.getByRole('textbox', { name: 'Plugin request' });
      assert.equal(await composer.inputValue(), 'Summarize session activity');
      await composer.fill('Build a clock plugin');
      if (QA) await page.screenshot({ path: join(QA, `plugin-creator-draft-${page.viewportSize().width}-${sidebar}.png`), animations: 'disabled' });
      await composer.press('Enter');
      const launchError = page.locator('.hoh-plugin-creator [role="alert"]');
      await Promise.race([
        page.waitForURL('**/hoh-local/thread-*'),
        launchError.waitFor().then(async () => { throw new Error(await launchError.innerText()); }),
      ]);
      await page.getByText('Should the clock tool return local time or UTC? I recommend UTC for consistent output.').waitFor();
      await page.getByText('Build a clock plugin', { exact: true }).waitFor();
      assert.equal(engine.cwd, '/work/plugin-drafts');
      assert.deepEqual(engine.promptExecutors, ['claude']);
      const prompt = engine.requests.find(({ method }) => method === 'session/prompt');
      assert.match(prompt.params.contentBlocks[0].text, /^# Author an external HoH plugin/);
      assert.match(prompt.params.contentBlocks[0].text, /ask exactly one design question per turn/);
      assert.match(prompt.params.contentBlocks[0].text, /Build a clock plugin$/);
    }, blocks: ['conversation'],
  },
  {
    name: 'learning', engine: busyEngine, stored: () => storage(true),
    prepare: async (page, engine, sidebar) => {
      await readyBusy(page, engine, true);
      await page.locator('[data-hoh-page-item="learning"]').click();
      await page.locator('[data-hoh-page="learning"]').waitFor();
      await setSidebar(page, sidebar);
    }, blocks: ['learning'],
  },
  ...['folder-trust', 'folder-trust-changed'].map(name => ({
    name, engine: () => trustEngine(name === 'folder-trust-changed'), path: () => `/hoh-local/${CHAT.id}`, stored: conversationStorage,
    prepare: async (page, _engine, sidebar) => {
      await page.getByText('The measured conversation lane is ready.').waitFor();
      await setSidebar(page, sidebar);
      await page.getByRole('button', { name: /Trust project settings in this folder/ }).waitFor();
    }, blocks: ['conversation', 'trust'],
  })),
  {
    name: 'thread-header-menu', engine: completedEngine, path: () => `/hoh-local/${CHAT.id}`, stored: conversationStorage,
    prepare: async (page, _engine, sidebar) => {
      await page.getByText('The measured conversation lane is ready.').waitFor();
      await setSidebar(page, sidebar);
      await page.getByRole('navigation', { name: 'Chat breadcrumb' }).click({ button: 'right' });
      const menu = page.getByRole('menu').last(); await menu.waitFor();
      await menu.getByRole('menuitem', { name: 'Rename chat', exact: true }).waitFor();
      await menu.getByRole('menuitem', { name: 'Delete', exact: true }).waitFor();
    }, blocks: ['conversation', 'thread-header-menu'],
  },
  ...['delete-confirmation', 'rename-editor'].map(name => ({
    name, engine: completedEngine, path: () => `/hoh-local/${CHAT.id}`, stored: conversationStorage,
    prepare: async (page, _engine, sidebar) => {
      await page.getByText('The measured conversation lane is ready.').waitFor();
      await setSidebar(page, sidebar);
      await page.getByRole('navigation', { name: 'Chat breadcrumb' }).click({ button: 'right' });
      const menu = page.getByRole('menu').last(); await menu.waitFor();
      await menu.getByRole('menuitem', { name: name === 'delete-confirmation' ? 'Delete' : 'Rename chat', exact: true }).click();
      if (name === 'delete-confirmation') await page.getByRole('alertdialog').getByText(/moves the chat to the trash/).waitFor();
      else await page.getByRole('textbox', { name: 'Chat title' }).waitFor();
    }, blocks: ['conversation', name],
  })),
  {
    name: 'subagent-approval', engine: () => modeEngine('confirm', true),
    path: () => `/hoh-local/${CHAT.id}`, stored: conversationStorage,
    prepare: async (page, engine, sidebar) => {
      await page.getByText('Keep working').waitFor();
      await setSidebar(page, sidebar);
      const session = engine.sessions.get(CHAT.id);
      const emit = (type, data) => engine.emit('session.event', { sessionId: CHAT.id, event: engine.append(session, type, data) });
      emit('turn/usage', { usage: { inputTokens: 200, cacheCreationInputTokens: 0, cacheReadInputTokens: 0, outputTokens: 2 } });
      emit('tool/call', { turn: 2, callId: 'approve-agent', name: 'Agent', arguments: { description: 'Write a file' } });
      emit('tool/call', { turn: 2, callId: 'approve-write', parentCallId: 'approve-agent', name: 'Write', arguments: { file_path: 'task.txt' } });
      emit('approval/asked', { id: 'approve-subagent', toolName: 'edit_file', callId: 'approve-write' });
      engine.emit('approval.pending', { sessionId: CHAT.id, approvalId: 'approve-subagent', humanText: 'Claude asks to create file task.txt', risk: 'unassessed', expiresAt: Date.now() + 60_000 });
      await page.getByText('Claude asks to create file task.txt', { exact: true }).first().waitFor();
      await page.getByRole('button', { name: 'Approve', exact: true }).waitFor();
      await page.getByRole('button', { name: 'Decline', exact: true }).waitFor();
    }, blocks: ['conversation'],
  },
  {
    name: 'plan-reset', engine: () => modeEngine('full', false),
    path: () => `/hoh-local/${CHAT.id}`, stored: conversationStorage,
    prepare: async (page, _engine, sidebar) => {
      await page.getByText('The measured conversation lane is ready.').waitFor();
      await setSidebar(page, sidebar);
      const selectPlan = async () => {
        await page.getByRole('button', { name: 'Permissions: Full access', exact: true }).click();
        await page.getByRole('menuitem', { name: 'Plan', exact: true }).click();
        await assertModeChip(page, 'Plan');
      };
      await selectPlan();
      await page.getByRole('button', { name: 'Permissions: Plan', exact: true }).click();
      await page.getByRole('menuitem', { name: /Plan/ }).click();
      await assertModeChip(page, 'Full access');
      await selectPlan();
      await page.getByRole('button', { name: 'Permissions: Plan', exact: true }).click();
      await page.getByRole('menuitemradio', { name: 'Full access', exact: true }).click();
      await assertModeChip(page, 'Full access');
      await selectPlan();
      await setSidebar(page, 'expanded');
      await page.locator('[data-sidebar="menu-button"]').filter({ hasText: 'New chat' }).first().click({ modifiers: ['Shift'] });
      await page.waitForURL('**/draft/*');
      await setSidebar(page, sidebar);
      await assertModeChip(page, 'Full access');
    }, blocks: [],
  },
  ...['full', 'confirm'].flatMap(mode => {
    const label = mode === 'full' ? 'Full access' : 'Supervised';
    return [
      { name: `mode-home-${mode}`, engine: () => modeEngine(mode, false), stored: () => storage(false), blocks: ['home'],
        prepare: async (page, _engine, sidebar) => {
          await page.locator('.hoh-card').first().waitFor(); await setSidebar(page, sidebar);
          if (mode === 'confirm') {
            await page.getByRole('button', { name: 'Permissions: Full access', exact: true }).click();
            await page.getByRole('menuitemradio', { name: 'Supervised', exact: true }).click();
          }
          await assertModeChip(page, label);
        } },
      { name: `mode-thread-${mode}`, engine: () => modeEngine(mode, false), path: () => `/hoh-local/${CHAT.id}`, stored: conversationStorage, blocks: ['conversation'],
        prepare: async (page, _engine, sidebar) => { await page.getByText('The measured conversation lane is ready.').waitFor(); await setSidebar(page, sidebar); await assertModeChip(page, label); } },
      { name: `mode-running-${mode}`, engine: () => modeEngine(mode, true), path: () => `/hoh-local/${CHAT.id}`, stored: conversationStorage, blocks: ['conversation'],
        prepare: async (page, _engine, sidebar) => {
          await page.getByText('Keep working').waitFor(); await setSidebar(page, sidebar); await assertModeChip(page, label);
          const chip = page.getByRole('button', { name: `Permissions: ${label}`, exact: true });
          await chip.click();
          const next = mode === 'full' ? 'Supervised' : 'Full access';
          await page.getByRole('menuitemradio', { name: next, exact: true }).click();
          await assertModeChip(page, `Current: ${label} · Next: ${next}`);
        } },
    ];
  }),
  {
    name: 'c1u-title-actions', engine: titleRecoveryEngine, path: () => `/hoh-local/${CHAT.id}`, stored: titleRecoveryStorage,
    prepare: async (page, _engine, sidebar) => {
      await page.getByText('The measured conversation lane is ready.').waitFor();
      await setSidebar(page, 'expanded');
      const row = page.locator('[data-thread-item]').filter({ hasText: 'Generated recovery title' });
      await row.waitFor();
      await page.getByRole('navigation', { name: 'Chat breadcrumb' }).getByText('Generated recovery title', { exact: true }).waitFor();
      await row.click({ button: 'right' });
      await page.getByRole('menuitem', { name: 'Rename chat', exact: true }).click();
      const input = page.getByRole('textbox', { name: 'Chat title' });
      await input.fill('Manual recovery title'); await input.press('Enter');
      const renamed = page.locator('[data-thread-item]').filter({ hasText: 'Manual recovery title' });
      await renamed.waitFor();
      await page.getByRole('navigation', { name: 'Chat breadcrumb' }).getByText('Manual recovery title', { exact: true }).waitFor();
      await renamed.click({ button: 'right' });
      await page.getByRole('menuitem', { name: 'Pin chat', exact: true }).click();
      await page.waitForFunction(id => JSON.parse(localStorage.getItem('hoh-host.thread-metadata.v1') ?? '{}')[id]?.pinnedAt != null, CHAT.id);
      if (!(await page.locator('[data-thread-item]').first().innerText()).includes('Manual recovery title')) throw new Error('Pinned conversation did not move to the top');
      await page.reload();
      await page.getByText('The measured conversation lane is ready.').waitFor(); await setSidebar(page, 'expanded');
      await page.locator('[data-thread-item]').filter({ hasText: 'Manual recovery title' }).waitFor();
      await page.getByRole('navigation', { name: 'Chat breadcrumb' }).getByText('Manual recovery title', { exact: true }).waitFor();
      await page.waitForTimeout(1000);
      if (!(await page.locator('[data-thread-item]').first().innerText()).includes('Manual recovery title')) throw new Error('Pinned order was lost after restart');
      await setSidebar(page, sidebar);
    }, blocks: ['conversation'],
  },
  {
    name: 'c1u-quick-mode', engine: runningRecoveryEngine, stored: () => ({}),
    prepare: async (page, _engine, sidebar) => {
      await page.locator('.hoh-card').first().waitFor(); await setSidebar(page, sidebar);
      await page.getByRole('button', { name: `Open ${CHAT.title}` }).click();
      const dialog = page.getByRole('dialog').last(); await dialog.waitFor();
      await dialog.getByRole('button', { name: /Permissions: Full access/ }).waitFor();
      if (await dialog.locator('[data-composer-access-unknown]').count()) throw new Error('Known running mode is shown as Unknown');
      await page.reload();
      await page.locator('.hoh-card').first().waitFor();
      await page.getByRole('button', { name: `Open ${CHAT.title}` }).click();
      await page.getByRole('dialog').last().getByRole('button', { name: /Permissions: Full access/ }).waitFor();
      await setSidebar(page, sidebar);
    }, blocks: ['quick'],
  },
].filter(scenario => !process.env.ALIGN_SCREEN || process.env.ALIGN_SCREEN === scenario.name);

async function rectangles(page) {
  return page.evaluate(() => {
    const visible = element => {
      if (!element) return null;
      const r = element.getBoundingClientRect();
      return r.width > 0 && r.height > 0 ? { left: r.left, top: r.top, right: r.right, bottom: r.bottom, width: r.width, height: r.height } : null;
    };
    const firstVisible = selector => [...document.querySelectorAll(selector)].map(visible).find(Boolean) ?? null;
    const allVisible = selector => [...document.querySelectorAll(selector)].map(visible).filter(Boolean);
    const contentBoxOf = element => {
      const r = visible(element);
      if (!element || !r) return null;
      const c = getComputedStyle(element); const left = parseFloat(c.paddingLeft) || 0; const right = parseFloat(c.paddingRight) || 0;
      return { ...r, left: r.left + left, right: r.right - right, width: r.width - left - right };
    };
    const contentBox = selector => contentBoxOf(document.querySelector(selector));
    const union = (...rects) => {
      const present = rects.filter(Boolean);
      if (!present.length) return null;
      const left = Math.min(...present.map(rect => rect.left)); const top = Math.min(...present.map(rect => rect.top));
      const right = Math.max(...present.map(rect => rect.right)); const bottom = Math.max(...present.map(rect => rect.bottom));
      return { left, top, right, bottom, width: right - left, height: bottom - top };
    };
    const paint = selector => {
      const element = document.querySelector(selector);
      if (!element) return null;
      const style = getComputedStyle(element);
      return { display: style.display, visibility: style.visibility, opacity: Number(style.opacity) };
    };
    const homeViewport = firstVisible('.hoh-home');
    const composer = firstVisible('[data-chat-composer-stack]');
    const mainLaneElement = [...document.querySelectorAll('.chat-composer-lane')]
      .find(element => !element.closest('.hoh-quick-view') && visible(element));
    const chatLane = contentBoxOf(mainLaneElement);
    return {
      viewport: { left: 0, top: 0, right: innerWidth, bottom: innerHeight, width: innerWidth, height: innerHeight },
      pageOverflow: Math.max(0, document.documentElement.scrollWidth - document.documentElement.clientWidth, document.body.scrollWidth - document.body.clientWidth),
      content: firstVisible('[data-slot="sidebar-inset"]'),
      homeViewport, home: firstVisible('.hoh-home__inner'), greeting: firstVisible('.hoh-home__head'), composer,
      headline: firstVisible('[data-chat-composer-stack] > .absolute.inset-x-0.bottom-full'), homeLabel: firstVisible('.hoh-home__label'), cards: firstVisible('.hoh-home__cards'), projects: firstVisible('.hoh-projects'), disclosure: firstVisible('.hoh-project-popover'),
      cardItems: allVisible('.hoh-home__cards > .hoh-card'), projectItems: allVisible('.hoh-projects > .hoh-project'),
      homeBottomClearance: (() => {
        const scroller = document.querySelector('.hoh-home');
        const targets = [...document.querySelectorAll('.hoh-home__cards > .hoh-card')];
        if (!scroller || !targets.length) return null;
        const previous = scroller.scrollTop; scroller.scrollTop = scroller.scrollHeight;
        const last = targets.map(element => element.getBoundingClientRect()).sort((a, b) => b.bottom - a.bottom)[0];
        const scrollerRect = scroller.getBoundingClientRect(); const composerRect = document.querySelector('[data-chat-composer-stack]')?.getBoundingClientRect();
        const clearance = Math.min(scrollerRect.bottom, composerRect?.top ?? scrollerRect.bottom) - last.bottom;
        scroller.scrollTop = previous; return clearance;
      })(),
      canvas: firstVisible('[data-chat-canvas]'), chatLane, feed: firstVisible('[data-chat-canvas] .chat-content-lane'), detailsCard: firstVisible('[data-thread-details-card]'),
      dialog: firstVisible('[data-slot="dialog-popup"].hoh-quick-view'), quickChat: firstVisible('.hoh-quick-view__chat'), quickComposer: firstVisible('.hoh-quick-view [data-chat-composer-stack]'), quickFeed: firstVisible('.hoh-quick-view [data-chat-canvas] .chat-content-lane'),
      underlayComposerPaint: paint('[data-hoh-home-body] + .chat-composer-lane'), quickComposerPaint: paint('.hoh-quick-view [data-chat-composer-stack]'),
      settingsScroll: firstVisible('[data-settings-page-scroll]'), settings: firstVisible('[data-settings-page-scroll] > div'),
      approval: firstVisible('[data-v2-item-type="approval_request"]'),
      menu: firstVisible('[role=menu]'), confirmation: firstVisible('[role=alertdialog]'), rename: firstVisible('input[aria-label="Chat title"]'),
      learning: contentBox('[data-hoh-page="learning"] .hoh-page__column'),
      trust: (() => { const b = [...document.querySelectorAll('button')].find(e => e.textContent?.includes('Trust project settings in this folder')); return visible(b?.parentElement); })(),
      breadcrumb: firstVisible('[aria-label="Chat breadcrumb"]'),
      pluginPage: firstVisible('.hoh-page'), plugins: contentBox('.hoh-plugins'),
      pluginItems: allVisible('.hoh-plugin-card, .hoh-plugin-detail, .hoh-plugins__top, .hoh-plugins-empty'),
      pluginOverflow: (() => { const element = document.querySelector('.hoh-plugins'); return element ? element.scrollWidth - element.clientWidth : null; })(),
    };
  });
}

function add(label, screen, size, sidebar, name, rect, reference, { equalMargins = true, tolerance = TOL } = {}) {
  if (!rect || !reference) { fail(label, `${name} or its reference not found`); return; }
  const center = rect.left + rect.width / 2; const target = reference.left + reference.width / 2;
  const deviation = center - target; const leftMargin = rect.left - reference.left; const rightMargin = reference.right - rect.right;
  rows.push({ screen, size, sidebar, block: name, axis: 'x', center: round(center), deviation: round(deviation), start: round(leftMargin), end: round(rightMargin), extent: round(rect.width) });
  if (Math.abs(deviation) > tolerance) fail(label, `${name} center differs by ${round(deviation)}px`);
  if (equalMargins && Math.abs(leftMargin - rightMargin) > tolerance) fail(label, `${name} margins differ: ${round(leftMargin)} / ${round(rightMargin)}px`);
  if (rect.left < reference.left - tolerance || rect.right > reference.right + tolerance) fail(label, `${name} leaves its horizontal container`);
  if (rect.left < -tolerance || rect.right > reference.width + reference.left + tolerance) fail(label, `${name} leaves the visible bounds`);
}

function addVertical(label, screen, size, sidebar, name, rect, reference, tolerance) {
  if (!rect || !reference) { fail(label, `${name} or its reference not found`); return; }
  const center = rect.top + rect.height / 2; const target = reference.top + reference.height / 2;
  const deviation = center - target; const topMargin = rect.top - reference.top; const bottomMargin = reference.bottom - rect.bottom;
  rows.push({ screen, size, sidebar, block: name, axis: 'y', center: round(center), deviation: round(deviation), start: round(topMargin), end: round(bottomMargin), extent: round(rect.height) });
  if (Math.abs(deviation) > tolerance) fail(label, `${name} vertical center differs by ${round(deviation)}px`);
  if (rect.top < reference.top - tolerance || rect.bottom > reference.bottom + tolerance) fail(label, `${name} leaves its vertical container`);
  if (rect.top < -tolerance || rect.bottom > reference.bottom + tolerance) fail(label, `${name} leaves the visible bounds`);
}

function withinViewport(label, name, rect, viewport) {
  if (!rect) return;
  if (rect.left < viewport.left - TOL || rect.right > viewport.right + TOL || rect.top < viewport.top - TOL || rect.bottom > viewport.bottom + TOL)
    fail(label, `${name} leaves the viewport: ${JSON.stringify(box(rect))}`);
}

function withinHorizontal(label, name, rect, container) {
  if (!rect || !container) return;
  if (rect.left < container.left - TOL || rect.right > container.right + TOL)
    fail(label, `${name} leaves its lane by ${round(Math.max(container.left - rect.left, rect.right - container.right))}px`);
}

function measure(label, scenario, size, sidebar, r) {
  if (!r.content) { fail(label, 'content inset not found'); return; }
  rows.push({ screen: scenario.name, size, sidebar, block: 'page-overflow', axis: 'bounds', center: 0, deviation: round(r.pageOverflow), start: 0, end: 0, extent: round(r.viewport.width) });
  if (r.pageOverflow > TOL) fail(label, `page has ${round(r.pageOverflow)}px horizontal overflow`);
  for (const [name, rect] of [['content-inset', r.content], ['home-scrollport', r.homeViewport], ['composer', r.composer], ['quick-dialog', r.dialog], ['settings-scrollport', r.settingsScroll], ['plugin-scrollport', r.pluginPage], ['menu', r.menu], ['confirmation', r.confirmation], ['rename editor', r.rename], ['folder trust', r.trust]])
    withinViewport(label, name, rect, r.viewport);
  if (scenario.blocks.includes('home')) {
    add(label, scenario.name, size, sidebar, 'home-column', r.home, r.chatLane ?? r.content);
    add(label, scenario.name, size, sidebar, 'greeting', r.greeting, r.home);
    add(label, scenario.name, size, sidebar, 'composer', r.composer, r.chatLane ?? r.content);
    if (r.headline) add(label, scenario.name, size, sidebar, 'headline', r.headline, r.composer);
    if (r.composer && r.home && r.composer.width > r.home.width + TOL) fail(label, `composer ${round(r.composer.width)}px is wider than home ${round(r.home.width)}px`);
    if (r.greeting && r.home && r.greeting.top > r.home.top + 40) fail(label, `greeting starts ${round(r.greeting.top - r.home.top)}px below the column top`);
    if (scenario.name === 'home-empty') {
      addVertical(label, scenario.name, size, sidebar, 'empty-group', unionRects(r.greeting, r.headline, r.composer), r.viewport, 8);
    }
  }
  if (scenario.blocks.includes('cards')) {
    add(label, scenario.name, size, sidebar, 'cards', r.cards, r.home);
    r.cardItems.forEach((rect, index) => withinHorizontal(label, `card ${index + 1}`, rect, r.home));
    if (r.homeLabel && r.cardItems[0] && Math.abs(r.homeLabel.left - r.cardItems[0].left) > TOL)
      fail(label, `first card starts ${round(r.cardItems[0].left - r.homeLabel.left)}px from the Now/Done heading`);
    if (r.homeBottomClearance === null) fail(label, 'last Home card scroll clearance was not measurable');
    else if (r.homeBottomClearance < 8 - TOL) fail(label, `last Home card cannot scroll clear of the composer (${round(r.homeBottomClearance)}px)`);
  }
  if (scenario.blocks.includes('projects')) {
    add(label, scenario.name, size, sidebar, 'projects', r.projects, r.home);
    r.projectItems.forEach((rect, index) => withinHorizontal(label, `project ${index + 1}`, rect, r.home));
    if (r.homeLabel && r.projectItems[0] && Math.abs(r.homeLabel.left - r.projectItems[0].left) > TOL)
      fail(label, `first project starts ${round(r.projectItems[0].left - r.homeLabel.left)}px from the Projects heading`);
  }
  if (scenario.blocks.includes('disclosure')) {
    add(label, scenario.name, size, sidebar, 'project-disclosure', r.disclosure, r.home);
    if (r.disclosure && r.composer && r.disclosure.bottom > r.composer.top + TOL)
      fail(label, `project disclosure overlaps the composer by ${round(r.disclosure.bottom - r.composer.top)}px`);
  }
  if (scenario.blocks.includes('conversation')) {
    add(label, scenario.name, size, sidebar, 'composer', r.composer, r.chatLane);
    add(label, scenario.name, size, sidebar, 'feed', r.feed, r.chatLane);
    if (r.feed && r.composer && Math.abs(r.feed.width - r.composer.width) > TOL) fail(label, `feed/composer widths differ by ${round(r.feed.width - r.composer.width)}px`);
    if (r.detailsCard && r.feed && r.feed.right > r.detailsCard.left - 8 + TOL)
      fail(label, `thread details overlap the message lane by ${round(r.feed.right - r.detailsCard.left)}px`);
  }
  if (scenario.blocks.includes('quick')) {
    add(label, scenario.name, size, sidebar, 'quick-dialog', r.dialog, r.viewport);
    add(label, scenario.name, size, sidebar, 'quick-composer', r.quickComposer, r.quickChat);
    if (r.quickFeed) add(label, scenario.name, size, sidebar, 'quick-feed', r.quickFeed, r.quickChat);
    if (r.dialog && (r.dialog.top < -TOL || r.dialog.bottom > r.viewport.bottom + TOL)) fail(label, 'quick dialog leaves the viewport vertically');
    const paints = state => state && state.display !== 'none' && state.visibility !== 'hidden' && state.opacity > 0.01;
    if (!r.underlayComposerPaint) fail(label, 'main composer lane not found beneath Quick View');
    else if (paints(r.underlayComposerPaint)) fail(label, `main composer paints beneath Quick View: ${JSON.stringify(r.underlayComposerPaint)}`);
    if (!paints(r.quickComposerPaint)) fail(label, `embedded Quick View composer is hidden: ${JSON.stringify(r.quickComposerPaint)}`);
  }
  if (scenario.blocks.includes('settings')) add(label, scenario.name, size, sidebar, 'settings-content', r.settings, r.settingsScroll);
  if (scenario.name === 'subagent-approval') {
    if (!r.approval || !r.feed) fail(label, 'approval card or feed missing');
    else {
      withinViewport(label, 'approval card', r.approval, r.viewport);
      withinHorizontal(label, 'approval card', r.approval, r.feed);
      const overflow = Math.max(0, r.feed.left - r.approval.left, r.approval.right - r.feed.right);
      rows.push({ screen: scenario.name, size, sidebar, block: 'approval-boundary', axis: 'bounds', center: round(r.approval.left + r.approval.width / 2), deviation: round(overflow), start: round(r.approval.left - r.feed.left), end: round(r.feed.right - r.approval.right), extent: round(r.approval.width) });
    }
  }
  if (scenario.blocks.includes('learning')) {
    add(label, scenario.name, size, sidebar, 'learning-column', r.learning, r.chatLane ?? r.content);
    if (!r.composer) fail(label, 'T3 composer reference missing');
    else if (r.learning && Math.abs(r.learning.width - r.composer.width) > TOL) fail(label, `Learning content width differs from T3 composer by ${round(r.learning.width - r.composer.width)}px`);
  }
  if (scenario.blocks.includes('trust')) {
    if (!r.trust || !r.breadcrumb) fail(label, 'folder trust or breadcrumb missing');
    else {
      const overlap = r.trust.top < r.breadcrumb.bottom - TOL ? Math.max(0, r.trust.right - r.breadcrumb.left) : 0;
      const outside = Math.max(0, r.viewport.left - r.trust.left, r.trust.right - r.viewport.right, r.viewport.top - r.trust.top, r.trust.bottom - r.viewport.bottom);
      rows.push({ screen: scenario.name, size, sidebar, block: 'folder-trust-boundary', axis: 'bounds', center: round(r.trust.left + r.trust.width / 2), deviation: round(Math.max(overlap, outside)), start: round(r.trust.left), end: round(r.viewport.right - r.trust.right), extent: round(r.trust.width) });
      if (overlap > TOL) fail(label, `folder trust overlaps breadcrumb by ${round(overlap)}px`);
    }
  }
  if (scenario.blocks.includes('thread-header-menu')) {
    if (!r.menu) fail(label, 'chat menu missing');
    else {
      const outside = Math.max(0, r.viewport.left - r.menu.left, r.menu.right - r.viewport.right, r.viewport.top - r.menu.top, r.menu.bottom - r.viewport.bottom);
      rows.push({ screen: scenario.name, size, sidebar, block: 'chat-menu-boundary', axis: 'bounds', center: round(r.menu.left + r.menu.width / 2), deviation: round(outside), start: round(r.menu.left), end: round(r.viewport.right - r.menu.right), extent: round(r.menu.width) });
    }
  }
  if (scenario.blocks.includes('delete-confirmation')) {
    add(label, scenario.name, size, sidebar, 'delete-dialog', r.confirmation, r.viewport);
    addVertical(label, scenario.name, size, sidebar, 'delete-dialog', r.confirmation, r.viewport, 2);
  }
  if (scenario.blocks.includes('rename-editor') && !r.rename) fail(label, 'rename editor missing');
  if (scenario.blocks.includes('plugins')) {
    add(label, scenario.name, size, sidebar, 'plugins-content', r.plugins, r.content);
    if (r.pluginOverflow === null) fail(label, 'plugins scroll container not found');
    else if (r.pluginOverflow > TOL) fail(label, `plugins have ${round(r.pluginOverflow)}px horizontal scroll overflow`);
    r.pluginItems.forEach((rect, index) => withinHorizontal(label, `plugin block ${index + 1}`, rect, r.plugins));
  }
  observations.push({ screen: scenario.name, size, sidebar, rects: r });
}

assert(await readFile(join(DIST, 'index.html')).then(() => true, () => false), 'dist/index.html is missing; build T3 first');
if (QA) await mkdir(QA, { recursive: true });
const chromeHome = await mkdtemp(join(tmpdir(), 'hoh-align-'));
let browser;
try {
  browser = await chromium.launch({
    executablePath: process.env.HOH_CHROMIUM ?? '/usr/bin/google-chrome', headless: true,
    env: { ...process.env, HOME: chromeHome, XDG_CONFIG_HOME: join(chromeHome, 'config'), XDG_CACHE_HOME: join(chromeHome, 'cache'), DBUS_SESSION_BUS_ADDRESS: '', PULSE_SERVER: 'unix:/nonexistent', PIPEWIRE_REMOTE: '/nonexistent', ALSA_CONFIG_PATH: '/dev/null' },
    args: ['--no-sandbox', '--disable-gpu', '--disable-background-networking', '--disable-component-update', '--no-first-run', '--disable-default-apps', '--force-color-profile=srgb'],
  });
  for (const scenario of scenarios) for (const [width, height] of SIZES) for (const sidebar of SIDEBARS) {
    const size = `${width}x${height}`; const label = `${scenario.name} ${size} ${sidebar}`; const engine = scenario.engine();
    console.log(`align: measure ${label}`);
    const initialSidebar = ['plugins', 'learning', 'plugin-creator'].includes(scenario.name) ? 'expanded' : sidebar;
    const initialStorage = { ...scenario.stored(), chat_thread_sidebar_open: initialSidebar === 'expanded' ? 'true' : 'false' };
    const { context, page, problems, off } = await open(browser, engine, width, height, scenario.path?.() ?? '/', initialStorage);
    try {
      await scenario.prepare(page, engine, sidebar); await settle(page);
      const rects = await rectangles(page); if (!scenario.blocks.includes('onboarding')) measure(label, scenario, size, sidebar, rects);
      const zeroWaiting = await page.locator('.hoh-project').evaluateAll(nodes => nodes.filter(node => /0 waiting/.test(node.textContent ?? '')).map(node => {
        const segmentLabel = node.querySelector('.hoh-project__segments')?.getAttribute('aria-label') ?? '';
        const workingCount = Number(/(\d+) working/.exec(segmentLabel)?.[1] ?? 0);
        const colorFor = state => {
          const probe = node.cloneNode(false);
          probe.removeAttribute('data-waiting'); probe.removeAttribute('data-working'); probe.removeAttribute('data-expanded');
          if (state) probe.setAttribute(`data-${state}`, 'true');
          probe.style.position = 'fixed'; probe.style.left = '-10000px'; probe.style.visibility = 'hidden';
          document.body.append(probe); const color = getComputedStyle(probe).borderColor; probe.remove(); return color;
        };
        const expected = workingCount > 0 ? 'working' : 'neutral';
        return {
          title: node.querySelector('.hoh-project__title')?.textContent?.trim(), expected,
          waiting: node.hasAttribute('data-waiting'), working: node.hasAttribute('data-working'),
          border: getComputedStyle(node).borderColor, expectedBorder: colorFor(expected === 'working' ? 'working' : null), waitingBorder: colorFor('waiting'),
        };
      }));
      for (const item of zeroWaiting) {
        if (item.waiting || item.border === item.waitingBorder || item.border !== item.expectedBorder || item.working !== (item.expected === 'working'))
          fail(label, `0-waiting project has wrong state/border: ${JSON.stringify(item)}`);
      }
      for (const problem of problems) fail(label, problem);
      const requests = await page.evaluate(() => (window.__HOH_T3_TEST_REQUESTS__ ?? []).map(({ method, payload }) => ({ method, ...(method === 'orchestration.dispatchCommand' && typeof payload?.type === 'string' ? { command: payload.type } : {}) })));
      const unsupportedRequests = requests.filter(({ method }) => ['projects.readFile', 'projects.listEntries', 'filesystem.getMetadata', 'subscribeDeviceState', 'subscribeProjectClones'].includes(method));
      if (unsupportedRequests.length) fail(label, `unsupported requests: ${unsupportedRequests.map(({ method }) => method).join(', ')}`);
      hostRequests.push({ screen: scenario.name, size, sidebar, requests });
      if (QA) {
        await page.screenshot({ path: join(QA, `${scenario.name}-${size}-${sidebar}.png`), animations: 'disabled' });
        if (scenario.name === 'home-flat') {
          await page.locator('.hoh-home').evaluate(node => { node.scrollTop = node.scrollHeight; });
          await page.waitForTimeout(50);
          await page.screenshot({ path: join(QA, `${scenario.name}-${size}-${sidebar}-scrolled.png`), animations: 'disabled' });
        }
      }
      if (scenario.name === 'quick-view') {
        await page.getByRole('button', { name: `Open ${CHAT.title} in full conversation` }).click();
        await page.waitForURL(`**/hoh-local/${CHAT.id}`);
        await page.getByRole('dialog').last().waitFor({ state: 'hidden' });
        await page.getByText('The measured conversation lane is ready.').waitFor();
      }
      if (scenario.name === 'conversation' && sidebar === 'expanded') {
        const folder = page.getByTestId('sidebar-project-folder').filter({ hasText: 'Alexandrie' });
        await folder.getByRole('button').first().click({ button: 'right' });
        await page.getByText('Copy path', { exact: true }).waitFor();
      }
    } catch (error) { const message = String(error?.message ?? error).split('\n')[0]; fail(label, message); console.error(`align: setup ${label}: ${String(error?.message ?? error)}`); }
    finally { off(); await context.close(); }
  }
} finally { await browser?.close(); await rm(chromeHome, { recursive: true, force: true }); }

const centerIn = (rect, reference) => (rect.left + rect.width / 2) - (reference.left + reference.width / 2);
const compareToConversation = (observation, name, rect, reference, { width = true, widthTolerance = TOL } = {}) => {
  const canonical = observations.find(item => item.screen === 'conversation' && item.size === observation.size && item.sidebar === observation.sidebar);
  if (!canonical?.rects.composer || !canonical.rects.chatLane || !rect || !reference) return;
  const label = `${observation.screen} ${observation.size} ${observation.sidebar}`;
  if (width && Math.abs(rect.width - canonical.rects.composer.width) > widthTolerance)
    fail(label, `${name} width ${round(rect.width)}px differs from T3 conversation ${round(canonical.rects.composer.width)}px`);
  const axisDifference = centerIn(rect, reference) - centerIn(canonical.rects.composer, canonical.rects.chatLane);
  if (Math.abs(axisDifference) > TOL) fail(label, `${name} relative axis differs from T3 conversation by ${round(axisDifference)}px`);
};

for (const observation of observations) {
  const { screen, size, sidebar, rects } = observation;
  if (screen.startsWith('home-'))
    compareToConversation(observation, 'home column', rects.home, rects.chatLane ?? rects.content);
  if (screen === 'plugins' || screen.startsWith('plugins-'))
    compareToConversation(observation, 'plugins column', rects.plugins, rects.content);
  if (screen === 'quick-view') {
    const canonical = observations.find(item => item.screen === 'conversation' && item.size === size && item.sidebar === sidebar);
    // The T3 dialog owns a wider local viewport than a constrained expanded thread. Compare the exact 46rem lane
    // whenever the thread can reach it; otherwise both lanes are checked against their own native container axis.
    // The dialog's two one-pixel borders reduce its measured inner lane by 2px.
    compareToConversation(observation, 'quick-view chat column', rects.quickComposer, rects.quickChat, { width: (canonical?.rects.composer?.width ?? 0) >= 735, widthTolerance: 2 });
    if (rects.quickComposer && rects.quickComposer.width > 736 + TOL) fail(`${screen} ${size} ${sidebar}`, `quick-view composer exceeds T3 46rem maximum: ${round(rects.quickComposer.width)}px`);
  }
}

const header = ['screen', 'size', 'sidebar', 'block', 'axis', 'center', 'deviation', 'start margin', 'end margin', 'extent'];
const table = [header, ...rows.map(row => [row.screen, row.size, row.sidebar, row.block, row.axis, row.center, row.deviation, row.start, row.end, row.extent])];
const widths = header.map((_, index) => Math.max(...table.map(line => String(line[index]).length)));
const rendered = table.map(line => line.map((cell, index) => String(cell).padEnd(widths[index])).join(' | ')).join('\n');
console.log(rendered);
console.log(`align: расхождений ${failures.length} из ${rows.length} замеров; экранов ${scenarios.length * SIZES.length * SIDEBARS.length}`);
if (QA) {
  await writeFile(join(QA, 'host-requests.json'), JSON.stringify(hostRequests, null, 2) + '\n');
  await writeFile(join(QA, 'alignments.tsv'), table.map(line => line.join('\t')).join('\n') + '\n');
  await writeFile(join(QA, 'alignments.json'), JSON.stringify({ tolerance: TOL, measurements: rows, failures }, null, 2) + '\n');
}
if (failures.length) { for (const failure of failures) console.error(`align: ${failure}`); process.exit(1); }
