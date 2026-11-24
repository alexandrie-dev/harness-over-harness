// Headless replay of redacted C6 engine events; run after both Vite builds.
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
const CHAT = { id:'chat-1', title:'C6 timing replay' };
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


const records=JSON.parse(await readFile(join(UI,'tests/hoh-host/fixtures/c6-turns.json'),'utf8'));
const engine=new FakeEngine();engine.access='confirm';engine.claude=CLAUDE;engine.bind(CHAT.id);
const session=engine.open(CHAT.id,CHAT.title);const originalId=records.find(e=>e.seq===5).data.id;
// C6 lost its send timestamp; reconstruct it from the observed 38s solely for this replay.
const inferredSend = records.find(e => e.seq === 17).time - 38_000;
const saved=conversationStorage();saved['hoh-host.thread-metadata.v1']=JSON.stringify({[CHAT.id]:{promptTimes:{[originalId]:inferredSend}}});
const browser=await chromium.launch({executablePath:'/usr/bin/google-chrome',headless:true,env:{...process.env,DBUS_SESSION_BUS_ADDRESS:'',PULSE_SERVER:'unix:/nonexistent',PIPEWIRE_REMOTE:'/nonexistent',ALSA_CONFIG_PATH:'/dev/null'},args:['--no-sandbox','--disable-gpu','--disable-background-networking']});
async function labels(page){const body=await page.locator('body').innerText();return body.split('\n').filter(v=>/^(You stopped after|Worked for)/.test(v));}
async function emit(from,to){for(const event of records.filter(e=>e.seq>=from&&e.seq<=to)){session.events.push(event);session.updatedAt=event.time;engine.emit('session.event',{sessionId:CHAT.id,event});await new Promise(r=>setTimeout(r,8));} await new Promise(r=>setTimeout(r,500));}
const first=await open(browser,engine,2000,1137,`/hoh-local/${CHAT.id}`,saved);
try {
 await first.page.waitForTimeout(2000);await emit(2,17);
 const short=await labels(first.page);console.log('short',JSON.stringify(short));assert(short.includes('Worked for 38s'));
 assert.equal(await first.page.getByText('Worked for 38s', { exact: true }).evaluate(node => node.closest('button')), null);
 await emit(18,54);const prior=await labels(first.page);console.log('before next',JSON.stringify(prior));assert(prior.includes('You stopped after 23s'));assert(prior.includes('You stopped after 28s'));
 await emit(56,93);const after=await labels(first.page);console.log('after next',JSON.stringify(after));for(const label of prior)assert(after.includes(label),`${label} changed`);assert(after.includes('Worked for 1m 22s'));
 assert.deepEqual(first.problems, []);await emit(94,183);const longer=await labels(first.page);assert(longer.includes('Worked for 1m 11s'));for(const label of after)assert(longer.includes(label),`${label} changed`);
 await emit(184,205);const last=await labels(first.page);for(const label of longer)assert(last.includes(label),`${label} changed after another turn`);
 first.off();await first.context.close();const restarted=await open(browser,engine,2000,1137,`/hoh-local/${CHAT.id}`,saved);
 try { await restarted.page.waitForTimeout(2000);const replay=await labels(restarted.page);console.log('after restart',JSON.stringify(replay));for(const label of last)assert(replay.includes(label),`${label} missing after restart`);assert.deepEqual(restarted.problems, []); }
 finally {restarted.off();await restarted.context.close();}
 // The original C6 storage retained only the engine's start. It cannot recover 38s.
 const originalStorage = { ...saved, 'hoh-host.thread-metadata.v1': JSON.stringify({ [CHAT.id]: { promptTimes: { [originalId]: records.find(e => e.seq === 2).time } } }) };
 const original = await open(browser, engine, 2000, 1137, `/hoh-local/${CHAT.id}`, originalStorage);
 try {
   await original.page.waitForTimeout(2000);
   const replay = await labels(original.page);
   console.log('original C6 storage', JSON.stringify(replay));
   assert(replay.includes('Worked for 3.5s'));
   assert.deepEqual(original.problems, []);
 } finally { original.off(); await original.context.close(); }
} finally {await browser.close();}
