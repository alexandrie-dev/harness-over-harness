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
  page.setDefaultTimeout(20_000);
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
  await page.addInitScript(({ stored }) => {
    for (const [key, value] of Object.entries(stored)) localStorage.setItem(key, value);
    const listeners = new Set();
    window.__hohT3Notify = notification => { for (const listener of listeners) listener(notification); };
    window.__HOH_T3_TEST_ENGINE__ = {
      request: async (method, params) => { const result = await window.__hohT3Request(method, params); if (result?.__hohError) throw new Error(result.__hohError); return result; },
      subscribe: async listener => { listeners.add(listener); return () => listeners.delete(listener); },
      subscribeStatus: async listener => { setTimeout(() => listener({ state: 'ready' }), 0); return () => {}; },
      subscribeQuestions: async () => () => {}, answerQuestion: async () => {}, decideApproval: async () => {},
    };
  }, { stored });
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

const scenarios = [
  {
    name: 'home-empty', engine: () => { const e = new FakeEngine(); e.access = 'confirm'; e.claude = CLAUDE; return e; }, stored: () => ({}),
    prepare: async (page, _engine, sidebar) => {
      await page.getByRole('button', { name: 'Start', exact: true }).click();
      await page.getByRole('button', { name: 'Start without a project', exact: true }).click();
      await page.getByText(/Good (night|morning|afternoon|evening)/).first().waitFor(); await setSidebar(page, sidebar);
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
    name: 'home-flat', engine: busyEngine, stored: () => storage(false),
    prepare: async (page, engine, sidebar) => {
      await readyBusy(page, engine, false); await setSidebar(page, sidebar);
      await page.getByText('Stopped', { exact: true }).waitFor();
    }, blocks: ['home', 'cards'],
  },
  {
    name: 'home-projects', engine: busyEngine, stored: () => storage(true),
    prepare: async (page, engine, sidebar) => { await readyBusy(page, engine, true); await setSidebar(page, sidebar); }, blocks: ['home', 'projects'],
  },
  {
    name: 'home-project-expanded', engine: busyEngine, stored: () => storage(true),
    prepare: async (page, engine, sidebar) => { await readyBusy(page, engine, true); await setSidebar(page, sidebar); const folder = page.locator('.hoh-project').filter({ hasText: 'Alexandrie' }).first(); await folder.focus(); await page.keyboard.press('Enter'); await page.locator('.hoh-project-popover').waitFor(); }, blocks: ['home', 'projects', 'disclosure'],
  },
  {
    name: 'conversation', engine: completedEngine, path: () => `/hoh-local/${CHAT.id}`, stored: conversationStorage,
    prepare: async (page, _engine, sidebar) => {
      await page.getByText('The measured conversation lane is ready.').waitFor(); await setSidebar(page, sidebar);
      const breadcrumb = page.getByRole('navigation', { name: 'Thread breadcrumb' });
      if (await breadcrumb.getByText('Alexandrie', { exact: true }).count() !== 1 || await breadcrumb.getByText('No project', { exact: true }).count() !== 0)
        throw new Error(`project conversation breadcrumb is wrong: ${JSON.stringify(await breadcrumb.innerText())}`);
    }, blocks: ['conversation'],
  },
  {
    name: 'quick-view', engine: completedEngine, stored: () => ({}),
    prepare: async (page, _engine, sidebar) => { await page.locator(`.hoh-card`).first().waitFor(); await setSidebar(page, sidebar); await page.getByRole('button', { name: `Open ${CHAT.title}` }).click(); await page.getByRole('dialog').last().waitFor(); await page.getByRole('dialog').last().getByRole('textbox').waitFor(); }, blocks: ['quick'],
  },
  {
    name: 'settings', engine: completedEngine, path: () => '/settings/general', stored: () => ({}),
    prepare: async (page, _engine, sidebar) => { await page.locator('[data-settings-page-scroll]').waitFor(); await setSidebar(page, sidebar); }, blocks: ['settings'],
  },
  {
    name: 'plugins', engine: busyEngine, stored: () => storage(true),
    prepare: async (page, engine, sidebar) => {
      await readyBusy(page, engine, true); await page.locator('[data-hoh-page-item="plugins"]').click(); await page.locator('.hoh-plugins').waitFor();
      await page.locator('[data-plugin-empty]').waitFor(); await setSidebar(page, sidebar);
    }, blocks: ['plugins'],
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
      content: firstVisible('[data-slot="sidebar-inset"]'), notch: firstVisible('.hoh-notch'),
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
  if (!r.content || !r.notch) { fail(label, 'content inset or notch not found'); return; }
  for (const [name, rect] of [['content-inset', r.content], ['notch', r.notch], ['home-scrollport', r.homeViewport], ['composer', r.composer], ['quick-dialog', r.dialog], ['settings-scrollport', r.settingsScroll], ['plugin-scrollport', r.pluginPage]])
    withinViewport(label, name, rect, r.viewport);
  add(label, scenario.name, size, sidebar, 'notch', r.notch, r.chatLane ?? r.content);
  if (scenario.blocks.includes('home')) {
    add(label, scenario.name, size, sidebar, 'home-column', r.home, r.content);
    add(label, scenario.name, size, sidebar, 'greeting', r.greeting, r.home);
    add(label, scenario.name, size, sidebar, 'composer', r.composer, r.content);
    if (r.headline) add(label, scenario.name, size, sidebar, 'headline', r.headline, r.composer);
    if (r.composer && r.home && r.composer.width > r.home.width + TOL) fail(label, `composer ${round(r.composer.width)}px is wider than home ${round(r.home.width)}px`);
    if (r.greeting && r.home && r.greeting.top > r.home.top + 40) fail(label, `greeting starts ${round(r.greeting.top - r.home.top)}px below the column top`);
    if (scenario.name === 'home-empty') {
      add(label, scenario.name, size, sidebar, 'notch-composer', r.notch, r.chatLane, { equalMargins: false, tolerance: 2 });
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
    add(label, scenario.name, size, sidebar, 'notch-composer', r.notch, r.chatLane, { equalMargins: false, tolerance: 2 });
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
    const initialSidebar = scenario.name === 'plugins' ? 'expanded' : sidebar;
    const initialStorage = { ...scenario.stored(), chat_thread_sidebar_open: initialSidebar === 'expanded' ? 'true' : 'false' };
    const { context, page, problems, off } = await open(browser, engine, width, height, scenario.path?.() ?? '/', initialStorage);
    try {
      await scenario.prepare(page, engine, sidebar); await settle(page);
      const rects = await rectangles(page); measure(label, scenario, size, sidebar, rects);
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
    } catch (error) { const message = String(error?.message ?? error).split('\n')[0]; fail(label, message); console.error(`align: setup ${label}: ${message}`); }
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
  if (screen.startsWith('home-')) compareToConversation(observation, 'home column', rects.home, rects.content);
  if (screen === 'plugins') compareToConversation(observation, 'plugins column', rects.plugins, rects.content);
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
  await writeFile(join(QA, 'alignments.tsv'), table.map(line => line.join('\t')).join('\n') + '\n');
  await writeFile(join(QA, 'alignments.json'), JSON.stringify({ tolerance: TOL, measurements: rows, failures }, null, 2) + '\n');
}
if (failures.length) { for (const failure of failures) console.error(`align: ${failure}`); process.exit(1); }
