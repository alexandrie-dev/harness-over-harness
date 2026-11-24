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



const engine=new FakeEngine();engine.access='confirm';engine.claude=CLAUDE;engine.bind(CHAT.id);const session=engine.open(CHAT.id,CHAT.title);
const browser=await chromium.launch({executablePath:'/usr/bin/google-chrome',headless:true,args:['--no-sandbox','--disable-gpu','--disable-background-networking']});
const {page,context,off,problems}=await open(browser,engine,1200,900,`/hoh-local/${CHAT.id}`,conversationStorage());
const captured = JSON.parse(await readFile(join(UI, 'tests/hoh-host/fixtures/u2a-stream.json'), 'utf8'));
const raw = JSON.parse(await readFile(join(UI, 'tests/hoh-host/fixtures/u2a-real-stream.json'), 'utf8'));
const firstStream = captured.findIndex(f => f.method === 'session.assistant-stream');
const lastStream = captured.findLastIndex(f => f.method === 'session.assistant-stream');
const template = captured[firstStream];
const frames = captured.slice(0, firstStream);
const attempts = new Map();
for (const event of raw) {
  if (event.type !== 'content_block_delta') continue;
  const kind = event.delta.type === 'thinking_delta' ? 'reasoning' : event.delta.type === 'text_delta' ? 'text' : null;
  if (!kind) continue;
  let attempt = attempts.get(kind);
  if (!attempt) {
    attempt = { id: `${CHAT.id}:1:${kind}:fixture:${event.index}`, revision: attempts.size + 1, index: 0 };
    attempts.set(kind, attempt);
    frames.push({ ...template, params: { ...template.params, frame: { type: 'start', attemptId: attempt.id, revision: attempt.revision, turn: 1, step: 1 } } });
  }
  frames.push({ ...template, params: { ...template.params, frame: { type: 'chunk', attemptId: attempt.id, revision: attempt.revision, index: attempt.index++, time: Date.now(), chunk: { type: `${kind}-delta`, text: kind === 'reasoning' ? event.delta.thinking : event.delta.text } } } });
}
for (const attempt of attempts.values()) frames.push({ ...template, params: { ...template.params, frame: { type: 'end', attemptId: attempt.id, revision: attempt.revision, index: attempt.index, outcome: 'committed' } } });
frames.push(...captured.slice(lastStream + 1));
let gotText = false; let textFrames = 0;
try {
  await page.waitForTimeout(1200);
  for (const f of frames) {
    if (!['session.event', 'session.status', 'session.assistant-stream'].includes(f.method) || f.params?.sessionId !== CHAT.id) continue;
    if (f.method === 'session.event') session.events.push(f.params.event);
    await page.evaluate(x => window.__hohT3Notify(x), f);
    await page.waitForTimeout(80);
    const body = await page.locator('body').innerText();
    const fr = f.params.frame;
    if (fr?.chunk?.type === 'reasoning-delta' && !gotText) { assert(body.includes('Thinking'), 'empty live reasoning must show Thinking'); assert(!body.includes('Starting'), 'Starting must disappear on reasoning'); }
    if (fr?.chunk?.type === 'text-delta') { gotText = true; textFrames++; assert(body.includes('part1'), 'partial answer visible before final message'); }
    if (gotText && f.method === 'session.assistant-stream') assert(!body.includes('Starting'), 'Starting must stay hidden after the first text frame');
  }
  assert.equal(textFrames, 30); assert.deepEqual(problems, []);
  console.log(`C9: ${textFrames} text frames visible without Starting`);
} finally { off(); await context.close(); await browser.close(); }
