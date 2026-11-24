// Replay Claude stream notifications captured from the real engine before the final answer.
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { dirname, extname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright-core';
import { FakeEngine } from '../tests/chat/fake-engine.mjs';
const UI = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const DIST = join(UI, 'dist');
const ORIGIN = 'http://hoh-align.test';
const TYPES = { '.js':'text/javascript', '.css':'text/css', '.html':'text/html', '.png':'image/png', '.svg':'image/svg+xml', '.woff2':'font/woff2', '.woff':'font/woff', '.json':'application/json', '.wasm':'application/wasm' };
const CLAUDE = { installed:true, version:'2.1.0', auth:'authenticated', models:[{ id:'opus', displayName:'Claude Opus', isDefault:true }] };
const CHAT = { id:'c7-real', title:'U2a stream' };
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




const browser = await chromium.launch({ executablePath: '/usr/bin/google-chrome', headless: true, args: ['--no-sandbox', '--disable-gpu', '--disable-background-networking'] });
const captured = JSON.parse(await readFile(join(UI, 'tests/hoh-host/fixtures/u2a-stream.json'), 'utf8'));
const raw = JSON.parse(await readFile(join(UI, 'tests/hoh-host/fixtures/u2a-real-stream.json'), 'utf8'));
for (const early of [false, true]) {
  const engine = new FakeEngine(); engine.access = 'confirm'; engine.claude = CLAUDE; engine.bind(CHAT.id);
  const session = engine.open(CHAT.id, CHAT.title);
  const {page,context,off,problems} = await open(browser,engine,1200,900,`/hoh-local/${CHAT.id}`,conversationStorage());
  try {
    await page.waitForTimeout(1000);
    await page.evaluate(() => { window.__c10Frames = []; let active = true; window.__c10StopFrames = () => { active = false; }; const sample = () => { if (!active) return; const text = document.body.innerText; window.__c10Frames.push({ thinking: text.includes('Thinking'), starting: text.includes('Starting'), text: text.includes('part1') }); requestAnimationFrame(sample); }; requestAnimationFrame(sample); });
    const first = captured.findIndex(f => f.method === 'session.assistant-stream');
    const prelude = captured.slice(0, first).filter(f => ['session.event','session.status'].includes(f.method) && f.params?.sessionId === CHAT.id);
    const template = captured[first];
    const make = frame => ({ ...template, params: { ...template.params, frame } });
    const rid = 'early-reasoning'; const tid = 'early-text';
    const start = make({ type: 'start', attemptId: rid, revision: 1, turn: 1, step: 1 });
    const chunk = make({ type: 'chunk', attemptId: rid, revision: 1, index: 0, chunk: { type: 'reasoning-delta', text: '' } });
    let thought = false;
    async function deliver(f) {
      if (f.method === 'session.event') session.events.push(f.params.event);
      await page.evaluate(value => window.__hohT3Notify(value), f); await page.waitForTimeout(100);
      const body = await page.locator('body').innerText();
      if (body.includes('Thinking')) thought = true;
      if (thought) assert(!body.includes('Starting') && (body.includes('Thinking') || body.includes('part1')), `regressed after Thinking: ${f.method} ${f.params?.event?.type ?? f.params?.frame?.type}`);
    }
    if (early) { await deliver(start); await deliver(chunk); }
    for (const f of prelude) await deliver(f);
    if (!early) { await deliver(start); await deliver(chunk); }
    for (const e of raw.filter(e => e.type === 'content_block_delta' && e.delta?.type === 'thinking_delta').slice(1)) {
      const index = (globalThis.__c10ReasonIndex = (globalThis.__c10ReasonIndex ?? 0) + 1);
      await deliver(make({ type:'chunk',attemptId:rid,revision:1,index,chunk:{type:'reasoning-delta',text:e.delta.thinking} }));
    }
    await deliver(make({ type:'start',attemptId:tid,revision:2,turn:1,step:1 }));
    let i = 0; for (const e of raw.filter(e => e.type === 'content_block_delta' && e.delta?.type === 'text_delta')) {
      await deliver(make({ type:'chunk',attemptId:tid,revision:2,index:i++,chunk:{type:'text-delta',text:e.delta.text} }));
    }
    const samples=await page.evaluate(() => { window.__c10StopFrames(); return window.__c10Frames; }); const firstThinking=samples.findIndex(sample=>sample.thinking); assert(firstThinking>=0); for(const sample of samples.slice(firstThinking)) assert(!sample.starting && (sample.thinking || sample.text), 'visible frame regressed or went blank after Thinking');
    assert(thought, 'Thinking was visible'); assert.equal(i,30); assert.deepEqual(problems,[]);
    delete globalThis.__c10ReasonIndex; console.log(`early=${early}: Thinking retained through ${i} text frames`);
  } finally { off(); await context.close(); }
}
await browser.close();
