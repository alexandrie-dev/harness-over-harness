// Replay Claude stream notifications captured from the real engine before the final answer.
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { dirname, extname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright-core';
import { FakeEngine } from '../tests/chat/fake-engine.mjs';
const UI = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const DIST = process.env.C11_DIST ?? join(UI, 'dist');
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
const engine = new FakeEngine(); engine.access='confirm'; engine.claude=CLAUDE; engine.selected={executor:'claude',model:'opus'}; engine.bind(CHAT.id);
const session=engine.open(CHAT.id,CHAT.title);
engine.append(session,'turn/start',{turn:1}); engine.append(session,'user/message',{id:'old',source:{kind:'user'},content:[{type:'text',text:'previous'}]}); engine.append(session,'turn/end',{turn:1,reason:{kind:'completed'}});
const original=engine.request.bind(engine); let release; const gate=new Promise(resolve=>{release=resolve});
engine.request=async (method,params)=>{ if(method==='session/prompt') await gate; return original(method,params); };
const {page,context,off,problems}=await open(browser,engine,1200,900,`/hoh-local/${CHAT.id}`,conversationStorage());
try {
  await page.locator('[contenteditable=true]').first().waitFor(); await page.waitForTimeout(1500);
  await page.evaluate(() => { window.__samples=[]; let active=true; window.__stopSampling=()=>active=false; const sample=()=>{if(!active)return; const body=document.body.innerText; window.__samples.push({thinking:body.includes('Thinking'),starting:body.includes('Starting')}); requestAnimationFrame(sample)}; requestAnimationFrame(sample) });
  const editor=page.locator('[contenteditable=true]').first(); await editor.fill('Next test message'); await editor.press('Enter');
  await page.waitForTimeout(700);
  const before=await page.evaluate(()=>window.__samples);
  console.log('before stream:', [...new Set(before.map(sample => JSON.stringify(sample)))].join(' → '));
  let thinking=false; for(const sample of before) { if(sample.thinking) thinking=true; assert(!(thinking&&sample.starting),'optimistic Thinking regressed to Starting'); }
  const template=JSON.parse(await readFile(join(UI,'tests/hoh-host/fixtures/u2a-stream.json'),'utf8')).find(f=>f.method==='session.assistant-stream');
  const make=frame=>({...template,params:{...template.params,frame:{...frame,turn:2}}});
  await page.evaluate(f=>window.__hohT3Notify(f),make({type:'start',attemptId:'c11-reason',revision:1,step:1}));
  await page.evaluate(f=>window.__hohT3Notify(f),make({type:'chunk',attemptId:'c11-reason',revision:1,index:0,step:1,chunk:{type:'reasoning-delta',text:''}}));
  await page.waitForTimeout(500); const samples=await page.evaluate(()=>{window.__stopSampling();return window.__samples});
  thinking=false; for(const sample of samples){ if(sample.thinking) thinking=true; assert(!(thinking&&sample.starting),'Thinking regressed after early stream frame'); }
  assert(thinking,'live reasoning became visible'); assert.deepEqual(problems,[]);
  console.log(`optimistic send: ${samples.length} frames, no Thinking → Starting`);
} finally { release(); off(); await context.close(); await browser.close(); }
