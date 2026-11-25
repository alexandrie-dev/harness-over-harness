import assert from 'node:assert/strict';
import { after, before, test } from 'node:test';
import { rm } from 'node:fs/promises';
import { buildHost } from './build.mjs';
import { FakeEngine } from './fake-engine.mjs';
import { WireClient, tick } from './wire-client.mjs';
let built, hostModule, wireModule, socketModule;
before(async () => { built = await buildHost(); [hostModule, wireModule, socketModule] = await Promise.all(['host', 'wire', 'socket'].map(name => built.load(name))); });
after(async () => { await rm(built.dir, { recursive: true, force: true }); });
function fixture() {
  const store = new Map();
  const engine = new FakeEngine({ store });
  engine.seed('chat', 'Original title', [['user/message', { id: 'u1', source: { kind: 'user' }, content: [{ type: 'text', text: 'Hello' }] }]]);
  const items = new Map();
  const storage = { getItem: key => items.get(key) ?? null, setItem: (key, value) => items.set(key, value) };
  const open = async () => {
    const e = new FakeEngine({ store });
    const host = new hostModule.HohHost({ engine: e, storage, throttleMs: 0 });
    const client = new WireClient(socketModule.HohWebSocket, new wireModule.WireServer(host.handlers()));
    await host.start(); return { host, client, engine: e };
  };
  return { open, store, storage };
}
const command = (client, type, extra = {}) => client.call('orchestration.dispatchCommand', { type, commandId: 'test', threadId: 'chat', ...extra });
const projection = async client => (await client.call('orchestration.getThreadProjection', { threadId: 'chat' })).thread;
const close = ({ host, client }) => { client.close(); host.stop(); };
test('rename survives restart and engine title reconciliation', async () => {
  const f = fixture(); const first = await f.open();
  await command(first.client, 'thread.metadata.update', { title: 'Renamed conversation' });
  close(first); const second = await f.open();
  assert.equal((await projection(second.client)).title, 'Renamed conversation');
  assert.equal(second.engine.requests.find(r => r.method === 'session/list')?.method, 'session/list'); close(second);
});
test('pin and unpin survive separate host restarts', async () => {
  const f = fixture(); const first = await f.open();
  await command(first.client, 'thread.pin', { orderKey: 'a1' }); const pinnedAt = (await projection(first.client)).pinnedAt;
  assert.ok(pinnedAt); close(first); const second = await f.open();
  assert.deepEqual([(await projection(second.client)).pinnedAt, (await projection(second.client)).pinOrderKey], [pinnedAt, 'a1']);
  await command(second.client, 'thread.pin.reorder', { orderKey: 'b2' }); close(second); const third = await f.open();
  assert.deepEqual([(await projection(third.client)).pinnedAt, (await projection(third.client)).pinOrderKey], [pinnedAt, 'b2']);
  await command(third.client, 'thread.unpin'); close(third); const fourth = await f.open();
  assert.equal((await projection(fourth.client)).pinnedAt, null); close(fourth);
});
test('archive and unarchive survive separate host restarts', async () => {
  const f = fixture(); const first = await f.open();
  await command(first.client, 'thread.archive'); const archivedAt = (await projection(first.client)).archivedAt;
  assert.ok(archivedAt); close(first); const second = await f.open();
  assert.equal((await projection(second.client)).archivedAt, archivedAt);
  const stream = await second.client.stream('orchestration.subscribeShell', {});
  const snapshot = await stream.until(value => value.kind === 'snapshot');
  assert.equal(snapshot.snapshot.threads.some(thread => thread.id === 'chat'), false);
  assert.equal(snapshot.snapshot.archivedThreads.find(thread => thread.id === 'chat')?.archivedAt, archivedAt);
  const archive = await second.client.call('orchestration.getArchivedShellSnapshot', {});
  assert.equal(archive.threads.find(thread => thread.id === 'chat')?.archivedAt, archivedAt);
  await command(second.client, 'thread.unarchive'); close(second); const third = await f.open();
  assert.equal((await projection(third.client)).archivedAt, null); close(third);
});
test('delete removes the chat from the shell and home, including after restart', async () => {
  const f = fixture(); const first = await f.open();
  const shell = await first.client.stream('orchestration.subscribeShell', {});
  await shell.until(value => value.kind === 'snapshot');
  await command(first.client, 'thread.metadata.update', { title: 'A title worth keeping' });
  assert.equal(Object.values(first.host.home.projectCounts()).reduce((a, b) => a + b, 0), 1);
  await command(first.client, 'thread.delete');
  assert.equal(JSON.parse(f.storage.getItem('hoh-host.thread-metadata.v1')).chat.title, 'A title worth keeping');
  const removed = await shell.until(value => value.kind === 'thread.removed');
  assert.equal(removed.threadId, 'chat');
  assert.equal(first.engine.requests.some(r => r.method === 'session/delete' && r.params.sessionId === 'chat'), true);
  assert.equal(first.host.home.entries().some(entry => entry.id === 'chat'), false);
  assert.equal(Object.values(first.host.home.projectCounts()).reduce((a, b) => a + b, 0), 0);
  assert.equal(f.store.has('chat'), false);
  close(first); const second = await f.open();
  assert.equal(second.host.thread('chat'), undefined);
  assert.equal(second.host.home.entries().some(entry => entry.id === 'chat'), false);
  assert.equal(Object.values(second.host.home.projectCounts()).reduce((a, b) => a + b, 0), 0);
  close(second);
});
test('failed engine delete keeps the chat visible', async () => {
  const f = fixture(); const first = await f.open();
  const request = first.engine.request.bind(first.engine);
  first.engine.request = (method, params) => method === 'session/delete' ? Promise.reject(new Error('Chat is running')) : request(method, params);
  const result = await first.client.request('orchestration.dispatchCommand', { type: 'thread.delete', commandId: 'test', threadId: 'chat' });
  assert.equal(result._tag, 'Failure'); assert.ok(first.host.thread('chat')); assert.equal(f.store.has('chat'), true); close(first);
});
test('a storage failure does not report a successful pin or mutate its state', async () => {
  const f = fixture(); const original = f.storage.setItem;
  f.storage.setItem = (key, value) => { if (key === 'hoh-host.thread-metadata.v1') throw new Error('full'); original(key, value); };
  const first = await f.open();
  const result = await first.client.request('orchestration.dispatchCommand', { type: 'thread.pin', commandId: 'test', threadId: 'chat' });
  assert.equal(result._tag, 'Failure'); assert.equal((await projection(first.client)).pinnedAt, null); close(first);
});

test('metadata is restored when an event arrives before a delayed session list', async () => {
  const f = fixture(); const first = await f.open();
  await command(first.client, 'thread.metadata.update', { title: 'Early title' });
  await command(first.client, 'thread.pin', { orderKey: 'early' }); close(first);
  const cache = JSON.parse(f.storage.getItem(hostModule.STORAGE_KEY));
  cache.sessions = []; f.storage.setItem(hostModule.STORAGE_KEY, JSON.stringify(cache));
  const engine = new FakeEngine({ store: f.store });
  const original = engine.request.bind(engine);
  let release; const gate = new Promise(resolve => { release = resolve; });
  engine.request = async (method, params) => { if (method === 'session/list') await gate; return original(method, params); };
  const host = new hostModule.HohHost({ engine, storage: f.storage, throttleMs: 0, loadRetryMs: 10 });
  const started = host.start();
  for (let i = 0; i < 100 && engine.listeners.size === 0; i++) await tick(5);
  assert.ok(engine.listeners.size, 'engine subscribed before the list resolves');
  engine.emitEvent('chat', 'assistant/message', { message: { id: 'early-answer', content: [{ type: 'text', text: 'Ready' }] } });
  assert.equal(host.thread('chat')?.title, 'Early title');
  assert.ok(host.thread('chat')?.pinnedAt);
  assert.equal(host.thread('chat')?.pinOrderKey, 'early');
  release(); await started; host.stop();
});

test('generated titles survive reconciliation with blank engine names; manual names take precedence', async () => {
  const store = new Map();
  const engine = new FakeEngine({ store });
  for (const id of ['generated', 'manual']) {
    engine.seed(id, '', [['user/message', { id: `${id}-u`, source: { kind: 'user' }, content: [{ type: 'text', text: `First ${id}` }] }]]);
    Object.assign(store.get(id), { executor: 'claude', model: 'claude-sonnet', runtimeMode: 'full' });
  }
  const data = new Map();
  const storage = { getItem: key => data.get(key) ?? null, setItem: (key, value) => data.set(key, value) };
  const saved = { version: 1, titles: { generated: 'Generated title', manual: 'Old generated title' }, sessions: ['generated', 'manual'].map(id => ({ id, title: 'New thread', projectId: 'hoh-project', executor: 'claude', model: 'claude-sonnet', mode: 'full', createdAt: 1700000000000, updatedAt: 1700000001000, archivedAt: null })) };
  storage.setItem(hostModule.STORAGE_KEY, JSON.stringify(saved));
  storage.setItem('hoh-host.thread-metadata.v1', JSON.stringify({ manual: { title: 'Manual title' } }));
  const host = new hostModule.HohHost({ engine, storage, throttleMs: 0 });
  await host.start(); await tick(25); host.flush();
  assert.equal(host.thread('generated')?.title, 'Generated title');
  assert.equal(host.thread('manual')?.title, 'Manual title');
  host.stop();
  const restarted = new hostModule.HohHost({ engine: new FakeEngine({ store }), storage, throttleMs: 0 });
  await restarted.start(); await tick(25); restarted.flush();
  assert.equal(restarted.thread('generated')?.title, 'Generated title');
  assert.equal(restarted.thread('manual')?.title, 'Manual title');
  restarted.stop();
});

test('rename, pin and archive do not wait for a cold conversation history', async () => {
  const f = fixture();
  const engine = new FakeEngine({ store: f.store });
  const request = engine.request.bind(engine);
  engine.request = (method, params) => method === 'session/events' ? new Promise(() => {}) : request(method, params);
  const host = new hostModule.HohHost({ engine, storage: f.storage, throttleMs: 0 });
  const client = new WireClient(socketModule.HohWebSocket, new wireModule.WireServer(host.handlers()));
  await host.start();
  const timed = operation => Promise.race([operation, new Promise((_, reject) => setTimeout(() => reject(new Error('list action waited for history')), 1000))]);
  await timed(command(client, 'thread.metadata.update', { title: 'Named while loading' }));
  await timed(command(client, 'thread.pin', { orderKey: 'first' }));
  await timed(command(client, 'thread.archive'));
  assert.equal(host.thread('chat')?.title, 'Named while loading');
  assert.equal(host.thread('chat')?.pinOrderKey, 'first');
  assert.ok(host.thread('chat')?.archivedAt);
  client.close(); host.stop();
  const reopened = await f.open();
  const restored = await projection(reopened.client);
  assert.deepEqual([restored.title, restored.pinOrderKey, !!restored.archivedAt], ['Named while loading', 'first', true]);
  close(reopened);
});

test('del2 events and approval remain visible while a deletion is refused', async () => {
  const f = fixture(); const first = await f.open();
  await projection(first.client);
  const original = first.engine.request.bind(first.engine);
  let release; const gate = new Promise(resolve => { release = resolve; });
  first.engine.request = async (method, params) => {
    if (method === 'session/delete') { await gate; throw new Error('Chat is running'); }
    return original(method, params);
  };
  const deleting = first.client.request('orchestration.dispatchCommand', { type: 'thread.delete', commandId: 'delete-gate', threadId: 'chat' });
  for (let i = 0; i < 100 && !first.host.deletingThreads.has('chat'); i++) await tick(2);
  assert(first.host.deletingThreads.has('chat'));
  first.engine.emitStatus('chat', 'running');
  first.engine.emitEvent('chat', 'assistant/message', { message: { id: 'during-delete', content: [{ type: 'text', text: 'answer during delete' }] } });
  first.engine.askApproval('chat', 'approval-during-delete', 'Allow this?');
  release(); assert.equal((await deleting)._tag, 'Failure');
  const data = await first.client.call('orchestration.getThreadProjection', { threadId: 'chat' });
  assert(data.turnItems.some(item => item.type === 'assistant_message' && item.text?.includes('answer during delete')));
  assert(data.runtimeRequests.some(item => item.id === 'approval-during-delete'));
  assert.equal(first.host.thread('chat').running, true);
  close(first);
});

test('del2 a delayed message cannot send after its chat is deleted', async () => {
  const f = fixture(); const first = await f.open();
  await projection(first.client);
  const original = first.engine.request.bind(first.engine);
  let release, entered; const gate = new Promise(resolve => { release = resolve; });
  const seen = new Promise(resolve => { entered = resolve; });
  first.engine.request = async (method, params) => {
    if (method === 'hoh/executor/select') { entered(); await gate; }
    return original(method, params);
  };
  const sending = first.client.request('orchestration.dispatchCommand', { type: 'message.dispatch', commandId: 'send-gate', threadId: 'chat', messageId: 'late', text: 'late', attachments: [], modelSelection: { instanceId: 'claudeAgent', model: 'a-different-model' }, runtimeMode: 'approval-required' });
  await seen;
  await command(first.client, 'thread.delete'); release();
  assert.equal((await sending)._tag, 'Failure');
  assert.equal(first.engine.requests.some(request => request.method === 'session/prompt' && request.params.sessionId === 'chat'), false);
  close(first);
});
