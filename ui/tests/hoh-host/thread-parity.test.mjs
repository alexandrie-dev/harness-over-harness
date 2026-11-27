import assert from 'node:assert/strict';
import { after, before, test } from 'node:test';
import { rm } from 'node:fs/promises';
import { buildHost } from './build.mjs';
import { FakeEngine } from './fake-engine.mjs';
import { WireClient, tick } from './wire-client.mjs';

let build, HohHost, HohWebSocket, WireServer;
before(async () => {
  build = await buildHost();
  ({ HohHost } = await build.load('host'));
  ({ HohWebSocket } = await build.load('socket'));
  ({ WireServer } = await build.load('wire'));
});
after(async () => { if (build) await rm(build.dir, { recursive: true, force: true }); });

const iso = milliseconds => new Date(milliseconds).toISOString();
const user = (id, text) => ['user/message', { id, source: { kind: 'user' }, content: [{ type: 'text', text }] }];
const assistant = (id, text) => ['assistant/message', { message: { id, content: [{ type: 'text', text }] } }];
const turn = (index, prompt, answer) => [
  ['turn/start', { turn: index }],
  user('question-' + index, prompt),
  assistant('reply-' + index, answer),
  ['turn/end', { turn: index, reason: { kind: 'completed' } }],
];

function storage() {
  const data = new Map();
  return { getItem: key => data.get(key) ?? null, setItem: (key, value) => data.set(key, value) };
}

function connect(engine, options = {}) {
  const host = new HohHost({ engine, storage: storage(), throttleMs: 0, ...options });
  const client = new WireClient(HohWebSocket, new WireServer(host.handlers()));
  return { host, client, close: () => { client.close(); host.stop(); } };
}

const command = (client, type, extra = {}) => client.call('orchestration.dispatchCommand', {
  type, commandId: type, threadId: 'chat', ...extra,
});
const project = client => client.call('orchestration.getThreadProjection', { threadId: 'chat' });

test('Find uses rendered Markdown and can select and page to an older result', async () => {
  const engine = new FakeEngine();
  engine.seed('chat', 'Older result', [
    ...turn(0, 'Introduction', 'A **copper** plate and a [copper plate](https://example.com/hidden-address).'),
    ...Array.from({ length: 90 }, (_, i) => turn(i + 1, 'Continue', 'Reply continues.')).flat(),
    ...turn(91, 'Another copper plate', 'Finished.'),
  ]);
  const { client, host, close } = connect(engine);
  try {
    const config = await client.call('server.getConfig');
    assert.equal(config.threadFind, true);
    assert.equal(config.threadFindProgressive, true);
    const initial = await host.readTimeline('chat');
    assert.equal(initial.hasMoreHistory, true);
    assert.equal(initial.projection.messages.some(message => message.id === 'msg-a-reply-0'), false);

    const find = input => client.call('orchestration.searchThread', { threadId: 'chat', query: 'COPPER PLATE', ...input });
    const first = await find({});
    assert.equal(first.totalMatches, 3);
    assert.deepEqual(first.match, { entryId: 'msg-a-reply-0', runId: 'run-msg-question-0', occurrence: 0 });
    assert.deepEqual((await find({ start: { entryId: 'msg-a-reply-0', occurrence: 1 } })).match,
      { ...first.match, occurrence: 1 });
    assert.equal((await find({ offset: -1 })).match.entryId, 'msg-question-91');
    assert.equal((await find({ query: 'hidden-address' })).totalMatches, 0);

    const stream = await client.stream('orchestration.searchThreadStream', { threadId: 'chat', query: 'copper plate' });
    assert.equal((await stream.until(result => result.complete === false)).match.entryId, first.match.entryId);
    assert.deepEqual(await stream.until(result => result.totalMatches === 3), first);
    for (let i = 0; i < 20 && !stream.exit; i++) await tick(5);
    assert.equal(stream.exit?._tag, 'Success');

    let cursor = initial.historyCursor;
    let foundOld = false;
    while (cursor !== null) {
      const earlier = await host.readTimeline('chat', Number(cursor));
      foundOld ||= earlier.items.some(row => row.item.messageId === first.match.entryId);
      cursor = earlier.nextCursor;
    }
    assert(foundOld);
  } finally { close(); }
});

test('the global search uses cold completed messages, ranks a real message ID and excludes archived chats', async () => {
  const engine = new FakeEngine();
  engine.seed('chat', 'Unnamed transcript', [
    ...turn(1, 'Something unrelated', `${'Earlier details. '.repeat(50)}Café blueprint is ready.`),
    ...turn(2, 'Follow-up', 'Nothing here.'),
  ]);
  engine.seed('archive', 'Archived transcript', turn(1, 'Café blueprint', 'Done.'));
  const { client, close } = connect(engine);
  try {
    await command(client, 'thread.archive', { threadId: 'archive' });
    const search = input => client.call('orchestration.searchThreads', { query: 'cafe blueprint', ...input });
    const { matches } = await search({});
    assert.equal(matches.length, 1);
    assert.deepEqual([matches[0].threadId, matches[0].messageId, matches[0].source],
      ['chat', 'msg-a-reply-1', 'assistant']);
    assert.match(matches[0].snippet, /Café blueprint/);
    assert(matches[0].snippet.length <= 240);
    assert.equal(engine.requests.some(request => request.method === 'session/resume'), false);
    assert.deepEqual(await search({ titlesOnly: true }), { matches: [] });
    assert.deepEqual(await search({ projectIds: ['another-project'] }), { matches: [] });
  } finally { close(); }
});

test('visit, unread, snooze, settlement, tags and ordering survive reopening the host', async () => {
  const engine = new FakeEngine();
  const session = engine.seed('chat', 'Organization', turn(1, 'One', 'Done.'));
  const now = session.updatedAt + 60_000;
  const options = { storage: storage(), now: () => now };
  let connection = connect(engine, options);
  try {
    const run = (await project(connection.client)).runs[0];
    const wake = iso(now + 60_000);
    await command(connection.client, 'thread.visit', { visitedAt: run.completedAt, opened: true, searchQuery: 'plan' });
    await command(connection.client, 'thread.mark-unread');
    await command(connection.client, 'thread.tag.set', { tag: ' Review ', tagged: true });
    await command(connection.client, 'thread.active.reorder', { orderKey: 'a1' });
    const tagged = (await project(connection.client)).thread;
    assert.deepEqual(tagged.tags, ['Review']);
    assert.equal(tagged.activeOrderKey, 'a1');
    assert.equal(tagged.lastVisitedAt, iso(Date.parse(run.completedAt) - 1));
    assert.deepEqual(tagged.opens.picks, [{ query: 'plan', at: now }]);

    await command(connection.client, 'thread.snooze', { snoozedUntil: wake });
    connection.close();
    connection = connect(new FakeEngine({ store: engine.store }), options);
    const restored = (await project(connection.client)).thread;
    assert.deepEqual([restored.tags, restored.activeOrderKey, restored.lastVisitedAt, restored.snoozedUntil],
      [['Review'], 'a1', tagged.lastVisitedAt, wake]);
    await command(connection.client, 'thread.settle', { expectedLatestRunId: run.id });
    const settled = (await project(connection.client)).thread;
    assert.deepEqual([settled.settledOverride, settled.activeOrderKey, settled.snoozedUntil], ['settled', null, null]);
    await command(connection.client, 'thread.pin', { orderKey: 'b1' });
    assert.equal((await project(connection.client)).thread.settledOverride, 'active');
    await command(connection.client, 'thread.unpin');
    const rejected = await connection.client.request('orchestration.dispatchCommand', {
      type: 'thread.pin.reorder', commandId: 'raced-reorder', threadId: 'chat', orderKey: 'c1',
    });
    assert.equal(rejected._tag, 'Failure');
    assert.equal((await project(connection.client)).thread.pinnedAt, null);
  } finally { connection.close(); }
});

test('automatic rename reports the exact request outcome and a manual rename supersedes it', async () => {
  const engine = new FakeEngine();
  engine.seed('chat', 'Old title', turn(1, 'Show the context window', 'Done.'));
  const request = engine.request.bind(engine);
  let resolveTitle, rejectTitle;
  engine.request = (method, params) => {
    if (method !== 'hoh/executor/title/generate') return request(method, params);
    assert.equal(params.executor, 'claude');
    assert.match(params.prompt, /Show the context window/);
    return new Promise((resolve, reject) => { resolveTitle = resolve; rejectTitle = reject; });
  };
  const options = { storage: storage() };
  let connection = connect(engine, options);
  try {
    await project(connection.client);
    const shell = await connection.client.stream('orchestration.subscribeShell');
    await shell.until(value => value.kind === 'snapshot');
    const rename = commandId => command(connection.client, 'thread.metadata.update', { commandId, regenerateTitle: true });
    await rename('regenerate-one');
    await shell.until(value => value.thread?.titleRegeneration?.requestId === 'regenerate-one');
    resolveTitle({ title: 'Context window indicator' });
    const renamed = await shell.until(value => value.thread?.lastTitleRegeneration?.requestId === 'regenerate-one');
    assert.equal(renamed.thread.title, 'Context window indicator');
    assert.equal(renamed.thread.lastTitleRegeneration.status, 'renamed');

    await rename('regenerate-two');
    await shell.until(value => value.thread?.titleRegeneration?.requestId === 'regenerate-two');
    rejectTitle(new Error('engine detail'));
    const failed = await shell.until(value => value.thread?.lastTitleRegeneration?.requestId === 'regenerate-two');
    assert.equal(failed.thread.lastTitleRegeneration.status, 'failed');
    assert.equal(failed.thread.title, 'Context window indicator');

    await rename('regenerate-stale');
    await shell.until(value => value.thread?.titleRegeneration?.requestId === 'regenerate-stale');
    await command(connection.client, 'thread.metadata.update', { commandId: 'manual', title: 'My chat' });
    resolveTitle({ title: 'Stale title' });
    await tick(5);
    const current = (await project(connection.client)).thread;
    assert.deepEqual([current.title, current.titleRegeneration], ['My chat', null]);
    connection.close();
    connection = connect(new FakeEngine({ store: engine.store }), options);
    assert.equal((await project(connection.client)).thread.title, 'My chat');
  } finally { connection.close(); }
});

test('a folded tool row loads its retained output on demand, scoped to the chat', async () => {
  const engine = new FakeEngine();
  const output = 'Report line.\n'.repeat(1900) + 'End of report.\n';
  engine.seed('chat', 'Tool report', [
    ['turn/start', { turn: 1 }],
    user('question-1', 'Read the report'),
    ['tool/call', { turn: 1, callId: 'report', name: 'Bash', arguments: { command: 'cat report.txt' } }],
    ['tool/result', { turn: 1, message: { toolCallId: 'report', content: [{ type: 'text', text: output }] } }],
    ['turn/end', { turn: 1, reason: { kind: 'completed' } }],
  ]);
  engine.seed('other', 'Another chat', turn(1, 'Use another folder', 'Done.'));
  const { client, close } = connect(engine);
  try {
    const snapshot = await project(client);
    const row = snapshot.visibleTurnItems.find(entry => entry.item.type === 'command_execution');
    assert.equal(row.item.output, undefined);
    assert.equal(row.item.outputOmitted, true);
    const get = threadId => client.call('orchestration.getTurnItem', { threadId, itemId: row.sourceItemId });
    const { item } = await get('chat');
    assert.equal(item.output, output);
    assert.equal(item.status, 'completed');
    assert.deepEqual(await get('other'), { item: null });
  } finally { close(); }
});

test('a side chat leaves its running parent alone and uses a separate durable conversation', async () => {
  const engine = new FakeEngine();
  engine.seed('chat', 'Primary', [
    ...turn(1, 'Parent-only question', 'Parent-only answer'),
    ['turn/start', { turn: 2 }],
    user('parent-next', 'The primary chat continues'),
  ]);
  let connection = connect(engine);
  const ask = (client, sourceThreadId = 'chat', targetThreadId = 'side') => command(client, 'thread.fork', {
    sourceThreadId, targetThreadId, title: 'Side chat', sideChat: true, sourcePoint: { type: 'latest_stable' },
  });
  try {
    await project(connection.client);
    engine.emitStatus('chat', 'running');
    await ask(connection.client);
    const inSide = () => connection.client.call('orchestration.getThreadProjection', { threadId: 'side' });
    assert.deepEqual((await inSide()).thread.lineage, {
      parentThreadId: 'chat', relationshipToParent: 'fork', rootThreadId: 'chat', sideChat: true,
    });
    assert.deepEqual((await inSide()).messages, []);
    const wanted = { threadId: 'side', messageId: 'side-q', text: 'Side-only question' };
    await command(connection.client, 'message.dispatch', wanted);
    await tick(20);
    await command(connection.client, 'thread.delete', { threadId: 'side', onlyIfUnused: true });
    assert.deepEqual((await inSide()).messages.map(message => message.text), ['Side-only question']);
    assert((await project(connection.client)).messages.every(message => message.text !== 'Side-only question'));
    assert((await project(connection.client)).runs.some(run => run.status === 'running'));
    assert.deepEqual((await connection.client.call('orchestration.getThreadFamily', { threadId: 'chat' })).threads.map(thread => thread.id), ['chat', 'side']);
    assert(!engine.requests.some(request => request.method === 'session/cancel'));
    assert.deepEqual(await connection.client.call('orchestration.searchThreads', { query: 'Side-only question' }), { matches: [] });
    await assert.rejects(ask(connection.client, 'side', 'nested'));
    connection.close();

    // Reopen with fresh window storage. The engine inventory supplies authoritative lineage.
    connection = connect(new FakeEngine({ store: engine.store }));
    const loaded = await inSide();
    assert.equal(loaded.thread.lineage.sideChat, true);
    assert.deepEqual(loaded.messages.map(message => message.text), ['Side-only question']);
  } finally {
    connection.close();
  }
});
