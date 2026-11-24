// Exploratory headless replay of redacted C7 journal events; run after both Vite builds.
// The journal omits transient stream, status and access-confirmation notifications.
// These are reconstructed from the real engine trace in the C7f-drift reproduction.
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
const CHAT = { id:'chat-1', title:'C7 live timing' };
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
  await page.clock.install({ time: new Date(records[0].time) });
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



const records=JSON.parse(await readFile(process.env.C7_FIXTURE_PATH ?? join(UI,'tests/hoh-host/fixtures/' + (process.env.C7_FIXTURE ?? 'c7-live-short.json')),'utf8'));
const users=records.filter(e=>e.type==='user/message'&&e.data.source?.kind==='user');
const engine=new FakeEngine(); engine.access='confirm';engine.claude=CLAUDE;engine.bind(CHAT.id);
const session=engine.open(CHAT.id,CHAT.title); const original=engine.request.bind(engine);let promptIndex=0;const pendingReplies=new Map();
engine.request=async (method,params)=>{if(method==='session/prompt'){engine.requests.push({method,params});const messageId=users[promptIndex++].data.id;return await new Promise(resolve=>pendingReplies.set(messageId,()=>resolve({messageId})));} return original(method,params);};
const browser=await chromium.launch({executablePath:'/usr/bin/google-chrome',headless:true,env:{...process.env,DBUS_SESSION_BUS_ADDRESS:'',PULSE_SERVER:'unix:/nonexistent',PIPEWIRE_REMOTE:'/nonexistent',ALSA_CONFIG_PATH:'/dev/null'},args:['--no-sandbox','--disable-gpu','--disable-background-networking']});
const first=await open(browser,engine,2000,1137,`/hoh-local/${CHAT.id}`,conversationStorage()); const {page}=first;
async function labels(){return (await page.locator('body').innerText()).split('\n').filter(v=>/^(You stopped after|Worked for)/.test(v));}
let replayTime=records[0].time; async function setClock(time){if(time>replayTime){await page.clock.fastForward(time-replayTime);await page.clock.setFixedTime(new Date(time));replayTime=time;}}
async function emit(event){session.events.push(event);session.updatedAt=event.time;await setClock(event.time);if(event.type==='assistant/message' && !event.data.parentCallId && event.data.message.content.some(c=>c.type==='text')) {const attempt=`stream-${event.seq}`;for(const frame of [{type:'start',attemptId:attempt,revision:1,turn:event.data.turn,step:event.data.step},{type:'chunk',attemptId:attempt,revision:1,index:0,chunk:{type:'text-delta',text:event.data.message.content[0].text}}]) {await page.evaluate(frame=>window.__hohT3Notify({method:'session.assistant-stream',params:{sessionId:'chat-1',frame}}),frame);await page.waitForTimeout(40);}}await page.evaluate(e=>window.__hohT3Notify({method:'session.event',params:{sessionId:'chat-1',event:e}}),event); if(event.type==='user/message' && event.data.source?.kind==='user') pendingReplies.get(event.data.id)?.(); await page.waitForTimeout(40);if(event.type==='assistant/message' && !event.data.parentCallId && event.data.message.content.some(c=>c.type==='text')) {const frame={type:'end',attemptId:`stream-${event.seq}`,revision:1,index:1,outcome:'completed'};await page.evaluate(frame=>window.__hohT3Notify({method:'session.assistant-stream',params:{sessionId:'chat-1',frame}}),frame);await page.waitForTimeout(40);}}
async function send(user){await setClock(user.time);const ed=page.locator('[contenteditable=true]').first();await ed.waitFor();await ed.fill(user.data.content[0].text);await ed.press('Enter');await page.getByText(user.data.content[0].text,{exact:true}).first().waitFor();await page.waitForTimeout(300);}
try {
 await page.waitForTimeout(1800);
 const starts=records.filter(e=>e.type==='turn/start'); let cursor=0; let prior=[];
 for(let i=0;i<starts.length;i++) {
  const user=users[i];
  if (i > 0) {
   await setClock(user.time - 2);
   await page.evaluate(i => window.__hohT3Notify({method:'approval.pending',params:{sessionId:'chat-1',approvalId:`mode-${i}`,humanText:'Return Claude to Supervised mode?',risk:'high',expiresAt:Date.now()+60000}}),i);
   await page.waitForTimeout(100);
   assert.deepEqual((await labels()).slice(0, prior.length), prior, `drift while access confirmation ${i} is pending`);
   await setClock(user.time - 1);
   await page.evaluate(i => window.__hohT3Notify({method:'approval.resolved',params:{approvalId:`mode-${i}`,outcome:'allowed-once'}}),i);
   await page.waitForTimeout(100);
   assert.deepEqual((await labels()).slice(0, prior.length), prior, `drift after access confirmation ${i}`);
  }
  await send(user);
  let now=await labels(); console.log('send',i,JSON.stringify(now));
  assert.deepEqual(now.slice(0, prior.length), prior, `drift after send ${i}`);
  await page.evaluate(()=>window.__hohT3Notify({method:'session.status',params:{sessionId:'chat-1',status:'running'}}));
  const next=starts[i+1]?.seq ?? Infinity;
  while(cursor<records.length && records[cursor].seq<next) {const event=records[cursor++]; await emit(event); now=await labels(); assert.deepEqual(now.slice(0, prior.length), prior, `drift at ${event.seq} ${event.type}`);}
  await page.evaluate(()=>window.__hohT3Notify({method:'session.status',params:{sessionId:'chat-1',status:'idle'}}));await page.waitForTimeout(200);prior=await labels();console.log('end',i,JSON.stringify(prior));
 }
 await page.reload(); await page.waitForTimeout(2000);const restored=await labels(); console.log('reload',JSON.stringify(restored));assert.deepEqual(restored,prior);
 const expected = users.length === 2 ? ['Worked for 4.1s', 'Worked for 3.1s'] : ['Worked for 8.8s', 'You stopped after 39s', 'You stopped after 30s', 'Worked for 3.6s', 'Worked for 1m 6s'];
 assert.deepEqual(restored, expected); assert.deepEqual(first.problems, []);
} finally {first.off();await first.context.close();await browser.close();}
