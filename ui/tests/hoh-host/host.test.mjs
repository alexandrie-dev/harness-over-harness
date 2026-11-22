// The HoH host for T3's client, end to end over its in-memory socket, on a fake engine.
import assert from 'node:assert/strict';
import { after, before, test } from 'node:test';
import { rm } from 'node:fs/promises';
import { buildHost } from './build.mjs';
import { FakeEngine } from './fake-engine.mjs';
import { WireClient, tick } from './wire-client.mjs';

let built; let mod;
before(async () => {
  built = await buildHost();
  mod = {
    host: await built.load('host'), wire: await built.load('wire'), socket: await built.load('socket'),
    http: await built.load('http'), config: await built.load('config'), index: await built.load('index'),
  };
});
after(async () => { await rm(built.dir, { recursive: true, force: true }); });

function connect(engine, options = {}) {
  const host = new mod.host.HohHost({ engine, throttleMs: 0, ...options });
  const server = new mod.wire.WireServer(host.handlers());
  const client = new WireClient(mod.socket.HohWebSocket, server);
  return { host, server, client };
}
const lastSnapshot = stream => stream.values.filter(value => value.kind === 'snapshot').at(-1);
const itemsOf = snapshot => snapshot.projection.turnItems;

test('handshake: config snapshot, probe, settings, ping; only Claude is offered', async () => {
  const engine = new FakeEngine();
  const { client } = connect(engine);
  const config = await client.stream('subscribeServerConfig', { environmentThemes: true, workspaceSnapshotsOnDemand: true });
  const first = await config.until(value => value.type === 'snapshot');
  assert.equal(first.version, 1);
  assert.equal(first.config.environment.environmentId, 'hoh-local');
  assert.equal(first.config.environment.orchestrationProtocolVersion, 2);
  assert.deepEqual(first.config.providers.map(provider => provider.instanceId), ['claudeAgent']);
  assert.deepEqual(first.config.providers[0].models.map(model => model.slug), ['claude-sonnet', 'claude-opus']);
  assert.equal(first.config.providers[0].status, 'ready');
  assert.deepEqual(first.config.providers[0].supportedRuntimeModes, ['approval-required', 'full-access']);
  assert.deepEqual(first.config.settings.defaultModelSelection, { instanceId: 'claudeAgent', model: 'claude-sonnet' });
  assert.equal(first.config.settings.defaultRuntimeMode, 'approval-required');
  // Nothing optional is advertised: T3 hides git, terminal, preview, PRs, issues… by their absence.
  assert.deepEqual(Object.keys(first.config.environment.capabilities).sort(), ['connectionProbe', 'inlineMessageContext', 'repositoryIdentity', 'threadForceStop']);
  assert.deepEqual(await client.call('server.probe', {}), {});
  assert.deepEqual(await client.request('server.reportClientActivity', {}), { _tag: 'Success', value: null });
  assert.equal((await client.call('server.getConfig', {})).environment.label, 'HoH');
  assert.equal((await client.call('server.getSettings', {})).defaultRuntimeMode, 'approval-required');
  client.send({ _tag: 'Ping' });
  await tick(5);
  assert(client.frames.some(frame => frame._tag === 'Pong'));
});

test('methods HoH does not have: typed "not supported", defect without the typed error, hidden streams stay quiet', async () => {
  const { client } = connect(new FakeEngine());
  const terminal = await client.request('terminal.open', { threadId: 't', terminalId: 'x' });
  assert.equal(terminal._tag, 'Failure');
  assert.deepEqual(terminal.cause[0], { _tag: 'Fail', error: { _tag: 'EnvironmentAuthorizationError', message: 'Not available in HoH.', requiredScope: 'orchestration:operate' } });
  const issues = await client.request('issues.list', {});
  assert.equal(issues.cause[0]._tag, 'Die');
  const device = await client.stream('subscribeDeviceState', {});
  await tick(20);
  assert.equal(device.values.length, 0); assert.equal(device.exit, undefined);
});

test('HTTP before the socket: session is signed in, the descriptor names the same environment', async () => {
  const session = await mod.http.hohHttpResponse('GET', '/api/auth/session').json();
  assert.equal(session.authenticated, true);
  assert(session.scopes.includes('orchestration:operate'));
  const descriptor = await mod.http.hohHttpResponse('GET', '/.well-known/t3/environment').json();
  assert.equal(descriptor.environmentId, 'hoh-local');
  assert.equal(descriptor.orchestrationProtocolVersion, 2);
  assert.equal(mod.http.hohHttpResponse('GET', '/api/orchestration/shell').status, 503);
  const target = { fetch: async () => new Response('asset') };
  const restore = mod.http.installHohFetch(mod.socket.socketMatcher('hoh.local'), target);
  assert.equal((await (await target.fetch('http://hoh.local/api/auth/session')).json()).authenticated, true);
  assert.equal(await (await target.fetch('http://localhost/assets/app.js')).text(), 'asset');
  restore();
});

test('the list: chats of the engine, newest first, with the completion marker', async () => {
  const engine = new FakeEngine();
  engine.seed('chat-old', 'Первый разговор', [['user/message', { id: 'u1', source: { kind: 'user' }, content: [{ type: 'text', text: 'Привет' }] }]]);
  engine.seed('chat-new', 'Второй', []);
  const { client } = connect(engine);
  const shell = await client.stream('orchestration.subscribeShell', { requestCompletionMarker: true });
  const snapshot = await shell.until(value => value.kind === 'snapshot');
  assert.deepEqual(snapshot.snapshot.threads.map(thread => thread.id), ['chat-new', 'chat-old']);
  assert.equal(snapshot.snapshot.threads[1].title, 'Первый разговор');
  assert.equal(snapshot.snapshot.projects[0].id, 'hoh-project');
  assert.deepEqual(snapshot.snapshot.archivedThreads, []);
  await shell.until(value => value.kind === 'synchronized');
});

test('new chat with a first message, streamed answer, tool step, end of turn', async () => {
  const engine = new FakeEngine();
  const { client } = connect(engine);
  const shell = await client.stream('orchestration.subscribeShell', {});
  await shell.until(value => value.kind === 'snapshot');
  const launched = await client.call('orchestration.launchThread', {
    commandId: 'cmd-1', threadId: 'thread-1', projectId: 'hoh-project', title: 'Hello', modelSelection: { instanceId: 'claudeAgent', model: 'claude-opus' },
    runtimeMode: 'approval-required', interactionMode: 'default', workspaceStrategy: { type: 'root' },
    initialMessage: { messageId: 'm-t3-1', text: 'Посчитай файлы', attachments: [] },
  });
  assert.equal(launched.threadId, 'thread-1');
  assert.equal(launched.resumed, false);
  assert.deepEqual(engine.requests.filter(r => r.method === 'hoh/executor/select').map(r => r.params), [{ executor: 'claude', model: 'claude-opus' }]);
  assert.deepEqual(engine.requests.find(r => r.method === 'session/prompt').params, { sessionId: 'thread-1', contentBlocks: [{ type: 'text', text: 'Посчитай файлы' }] });
  const user = launched.projection.turnItems.find(item => item.type === 'user_message');
  assert.equal(user.messageId, 'm-t3-1'); assert.equal(user.text, 'Посчитай файлы');
  assert.equal(launched.projection.runs[0].status, 'running');
  assert.equal(launched.projection.thread.modelSelection.model, 'claude-opus');

  const thread = await client.stream('orchestration.subscribeThread', { threadId: 'thread-1', requestCompletionMarker: true });
  await thread.until(value => value.kind === 'synchronized');
  await tick(5); // the engine's echo of the message and turn/start
  assert.equal(itemsOf(lastSnapshot(thread)).filter(item => item.type === 'user_message').length, 1, 'the echo is not shown twice');

  engine.emitFrame('thread-1', { type: 'start', attemptId: 'att-1', revision: 1, turn: 1, step: 1 });
  engine.emitFrame('thread-1', { type: 'chunk', attemptId: 'att-1', revision: 1, index: 0, time: 1, chunk: { type: 'text-delta', text: 'Сейчас ' } });
  engine.emitFrame('thread-1', { type: 'chunk', attemptId: 'att-1', revision: 1, index: 1, time: 2, chunk: { type: 'text-delta', text: 'посмотрю.' } });
  const streaming = await thread.until(value => value.kind === 'snapshot' && itemsOf(value).some(item => item.type === 'assistant_message' && item.text === 'Сейчас посмотрю.'));
  const live = itemsOf(streaming).find(item => item.type === 'assistant_message');
  assert.equal(live.streaming, true); assert.equal(live.status, 'running');
  engine.emitFrame('thread-1', { type: 'end', attemptId: 'att-1', revision: 1, index: 2, outcome: 'completed' });
  engine.emitEvent('thread-1', 'assistant/message', { turn: 1, step: 1, message: { id: 'a1', content: [{ type: 'text', text: 'Сейчас посмотрю.' }] } });

  engine.emitEvent('thread-1', 'tool/call', { turn: 1, callId: 'c1', name: 'Bash', arguments: JSON.stringify({ command: 'ls | wc -l' }) });
  const running = await thread.until(value => value.kind === 'snapshot' && itemsOf(value).some(item => item.type === 'command_execution'));
  const step = itemsOf(running).find(item => item.type === 'command_execution');
  assert.equal(step.input, 'ls | wc -l'); assert.equal(step.status, 'running');
  engine.emitEvent('thread-1', 'tool/result', { turn: 1, message: { toolCallId: 'c1', content: [{ type: 'text', text: '7\n' }] } });
  const done = await thread.until(value => value.kind === 'snapshot' && itemsOf(value).some(item => item.type === 'command_execution' && item.status === 'completed'));
  assert.equal(itemsOf(done).find(item => item.type === 'command_execution').output, '7\n');

  engine.emitEvent('thread-1', 'assistant/message', { turn: 1, step: 2, message: { id: 'a2', content: [{ type: 'text', text: 'Файлов: 7.' }] } });
  engine.emitEvent('thread-1', 'turn/end', { reason: { kind: 'completed' } });
  const ended = await thread.until(value => value.kind === 'snapshot' && value.projection.runs[0]?.status === 'completed');
  const answers = itemsOf(ended).filter(item => item.type === 'assistant_message');
  assert.deepEqual(answers.map(item => [item.text, item.streaming, item.status]), [['Сейчас посмотрю.', false, 'completed'], ['Файлов: 7.', false, 'completed']]);
  assert.deepEqual(ended.projection.visibleTurnItems.map(row => row.item.type), ['user_message', 'assistant_message', 'command_execution', 'assistant_message']);
  assert(ended.snapshotSequence > 0);

  const row = await shell.until(value => value.kind === 'thread.updated' && value.thread.id === 'thread-1' && value.thread.status === 'completed');
  assert.equal(row.thread.latestVisibleMessage.text, 'Файлов: 7.');
  assert.equal(row.thread.title, 'Hello');
});

test('a follow-up message goes to the same chat; Stop is the engine\'s cancel and the run ends interrupted', async () => {
  const engine = new FakeEngine();
  const { client } = connect(engine);
  await client.call('orchestration.dispatchCommand', {
    type: 'thread.create', commandId: 'c0', threadId: 'thread-2', projectId: 'hoh-project', title: 'Second', createdBy: 'user', creationSource: 'web',
    modelSelection: { instanceId: 'claudeAgent', model: 'claude-sonnet' }, runtimeMode: 'approval-required', interactionMode: 'default', branch: null, worktreePath: null,
  });
  const thread = await client.stream('orchestration.subscribeThread', { threadId: 'thread-2' });
  await thread.until(value => value.kind === 'snapshot');
  const result = await client.call('orchestration.dispatchCommand', {
    type: 'message.dispatch', commandId: 'c1', threadId: 'thread-2', messageId: 'm-2', text: 'Долгая задача', attachments: [],
    createdBy: 'user', creationSource: 'web', dispatchMode: { type: 'start_immediately' },
  });
  assert(Number.isInteger(result.sequence));
  await thread.until(value => value.kind === 'snapshot' && value.projection.runs[0]?.status === 'running');
  await client.call('orchestration.dispatchCommand', { type: 'thread.stop', commandId: 'c2', threadId: 'thread-2' });
  assert.equal(engine.cancelled, 1);
  engine.emitEvent('thread-2', 'turn/end', { reason: { kind: 'aborted' } });
  await thread.until(value => value.kind === 'snapshot' && value.projection.runs[0]?.status === 'interrupted');
  assert.deepEqual(await client.call('orchestration.forceStopThread', { threadId: 'thread-2' }), { outcome: 'killed', sharedWithThreadIds: [] });
  assert.equal(engine.cancelled, 2);
});

test('permission request of Claude: shown in the chat, the answer goes to the engine', async () => {
  const engine = new FakeEngine();
  engine.seed('chat-p', 'P', [['user/message', { id: 'u1', source: { kind: 'user' }, content: [{ type: 'text', text: 'удали tmp' }] }], ['turn/start', {}]]);
  const { client } = connect(engine);
  const thread = await client.stream('orchestration.subscribeThread', { threadId: 'chat-p' });
  await thread.until(value => value.kind === 'snapshot');
  engine.emitStatus('chat-p', 'running');
  engine.askApproval('chat-p', 'appr-1', 'Claude хочет выполнить: rm -rf tmp');
  const asked = await thread.until(value => value.kind === 'snapshot' && value.projection.runtimeRequests.length === 1);
  const request = asked.projection.runtimeRequests[0];
  assert.deepEqual([request.id, request.kind, request.status, request.responseCapability.type], ['appr-1', 'permission', 'pending', 'live']);
  const item = itemsOf(asked).find(entry => entry.type === 'approval_request');
  assert.equal(item.requestId, 'appr-1'); assert.equal(item.prompt, 'Claude хочет выполнить: rm -rf tmp');
  assert.equal(asked.projection.runs[0].status, 'waiting');
  await client.call('orchestration.dispatchCommand', { type: 'runtime-request.respond', commandId: 'c1', threadId: 'chat-p', requestId: 'appr-1', decision: 'accept' });
  assert.deepEqual(engine.decisions, [{ approvalId: 'appr-1', outcome: 'allowed-once' }]);
  const answered = await thread.until(value => value.kind === 'snapshot' && value.projection.runtimeRequests[0]?.status === 'resolved');
  assert.equal(answered.projection.runtimeRequests[0].decision, 'accept');
  // The engine's own resolution afterwards changes nothing.
  engine.notify('approval.resolved', { approvalId: 'appr-1', outcome: 'allowed-once' });
  // A refusal maps to rejected.
  engine.askApproval('chat-p', 'appr-2', 'Ещё раз');
  await thread.until(value => value.kind === 'snapshot' && value.projection.runtimeRequests.length === 2);
  await client.call('orchestration.dispatchCommand', { type: 'runtime-request.respond', commandId: 'c2', threadId: 'chat-p', requestId: 'appr-2', decision: 'decline' });
  assert.deepEqual(engine.decisions.at(-1), { approvalId: 'appr-2', outcome: 'rejected' });
});

test('question of Claude: T3 question card, answers in the engine\'s form', async () => {
  const engine = new FakeEngine();
  engine.seed('chat-q', 'Q', [['user/message', { id: 'u1', source: { kind: 'user' }, content: [{ type: 'text', text: 'спроси меня' }] }]]);
  const { client } = connect(engine);
  const thread = await client.stream('orchestration.subscribeThread', { threadId: 'chat-q' });
  await thread.until(value => value.kind === 'snapshot');
  engine.askQuestion({ requestId: 'q-1', sessionId: 'chat-q', questions: [{ id: 'color', question: 'Какой цвет?', options: [{ label: 'Синий' }, { label: 'Красный', description: 'тёплый' }] }] });
  const asked = await thread.until(value => value.kind === 'snapshot' && value.projection.runtimeRequests.some(r => r.kind === 'user_input'));
  const card = itemsOf(asked).find(item => item.type === 'user_input_request');
  assert.deepEqual(card.questions[0].options, [{ label: 'Синий', description: 'Синий', value: 'Синий' }, { label: 'Красный', description: 'тёплый', value: 'Красный' }]);
  await client.call('orchestration.dispatchCommand', { type: 'runtime-request.respond', commandId: 'c1', threadId: 'chat-q', requestId: 'q-1', answers: { color: 'Красный' } });
  assert.deepEqual(engine.answers, [{ requestId: 'q-1', answer: { answers: [{ id: 'color', selected: ['Красный'] }] } }]);
  engine.askQuestion({ requestId: 'q-2', sessionId: 'chat-q', questions: [{ id: 'name', question: 'Имя?' }] });
  await thread.until(value => value.kind === 'snapshot' && value.projection.runtimeRequests.length === 2);
  await client.call('orchestration.dispatchCommand', { type: 'runtime-request.respond', commandId: 'c2', threadId: 'chat-q', requestId: 'q-2', answers: { name: 'Вася' } });
  assert.deepEqual(engine.answers.at(-1).answer, { answers: [{ id: 'name', selected: [], custom: 'Вася' }] });
});

test('access mode: T3\'s switch goes to the engine at once, every thread and the config follow', async () => {
  const engine = new FakeEngine();
  engine.seed('chat-m', 'M', []);
  const { client } = connect(engine);
  const config = await client.stream('subscribeServerConfig', {});
  await config.until(value => value.type === 'snapshot');
  await client.call('orchestration.dispatchCommand', { type: 'thread.runtime-mode.set', commandId: 'c1', threadId: 'chat-m', runtimeMode: 'full-access' });
  assert.deepEqual(engine.requests.filter(r => r.method === 'hoh/executor/access/set').map(r => r.params), [{ executor: 'claude', mode: 'full' }]);
  const next = await config.until(value => value.type === 'snapshot' && value.config.settings.defaultRuntimeMode === 'full-access');
  assert(next);
  assert.equal((await client.call('orchestration.getThreadProjection', { threadId: 'chat-m' })).thread.runtimeMode, 'full-access');
  // T3's in-between modes ask first.
  await client.call('orchestration.dispatchCommand', { type: 'thread.runtime-mode.set', commandId: 'c2', threadId: 'chat-m', runtimeMode: 'auto-accept-edits' });
  assert.equal(engine.access.mode, 'confirm');
});

test('after a restart the chat comes back from the engine\'s history; an unfinished turn ends as stopped', async () => {
  const store = new Map();
  const before = new FakeEngine({ store });
  before.seed('chat-r', 'Restart me', [
    ['user/message', { id: 'u1', source: { kind: 'user' }, content: [{ type: 'text', text: 'Первый вопрос' }] }],
    ['turn/start', {}],
    ['assistant/message', { turn: 1, step: 1, message: { id: 'a1', content: [{ type: 'text', text: 'Первый ответ' }] } }],
    ['turn/end', { reason: { kind: 'completed' } }],
    ['user/message', { id: 'u2', source: { kind: 'user' }, content: [{ type: 'text', text: 'Второй вопрос' }] }],
    ['turn/start', {}],
    ['tool/call', { turn: 2, callId: 'c1', name: 'Read', arguments: { file_path: 'note.txt' } }],
  ]);
  store.get('chat-r').model = 'claude-opus';
  const engine = new FakeEngine({ store });
  const { client } = connect(engine);
  const projection = await client.call('orchestration.getThreadProjection', { threadId: 'chat-r' });
  assert.equal(projection.thread.modelSelection.model, 'claude-opus', 'the chat keeps the model it was bound to');
  assert.equal(engine.requests.findIndex(r => r.method === 'session/resume') < engine.requests.findIndex(r => r.method === 'session/events'), true, 'resumed before reading');
  assert.deepEqual(projection.runs.map(run => run.status), ['completed', 'interrupted']);
  assert.deepEqual(projection.turnItems.map(item => [item.type, item.status]), [
    ['user_message', 'completed'], ['assistant_message', 'completed'], ['user_message', 'completed'], ['dynamic_tool', 'interrupted'],
  ]);
  assert.equal(projection.thread.title, 'Restart me');
  const thread = await client.stream('orchestration.subscribeThread', { threadId: 'chat-r' });
  const snapshot = await thread.until(value => value.kind === 'snapshot');
  assert.equal(snapshot.projection.messages.length, 3);
  // The chat continues after the restart.
  await client.call('orchestration.dispatchCommand', { type: 'message.dispatch', commandId: 'c1', threadId: 'chat-r', messageId: 'm-r', text: 'Продолжим', attachments: [], createdBy: 'user', creationSource: 'web', dispatchMode: { type: 'start_immediately' } });
  await thread.until(value => value.kind === 'snapshot' && value.projection.runs.length === 3);
});

test('engine lost mid-turn: the running turn ends as stopped', async () => {
  const engine = new FakeEngine();
  engine.seed('chat-l', 'L', []);
  const { client } = connect(engine);
  const thread = await client.stream('orchestration.subscribeThread', { threadId: 'chat-l' });
  await thread.until(value => value.kind === 'snapshot');
  engine.emitEvent('chat-l', 'user/message', { id: 'u1', source: { kind: 'user' }, content: [{ type: 'text', text: 'x' }] });
  engine.emitEvent('chat-l', 'turn/start', {});
  await thread.until(value => value.kind === 'snapshot' && value.projection.runs[0]?.status === 'running');
  engine.setStatus('restarting');
  await thread.until(value => value.kind === 'snapshot' && value.projection.runs[0]?.status === 'interrupted');
});

test('socket: interrupt and close end the subscriptions; other addresses are refused', async () => {
  const engine = new FakeEngine();
  engine.seed('chat-s', 'S', []);
  const { host, client } = connect(engine);
  const thread = await client.stream('orchestration.subscribeThread', { threadId: 'chat-s' });
  await thread.until(value => value.kind === 'snapshot');
  client.interrupt(thread);
  await tick(5);
  const count = thread.values.length;
  engine.emitEvent('chat-s', 'user/message', { id: 'u1', source: { kind: 'user' }, content: [{ type: 'text', text: 'после' }] });
  host.flush(); await tick(5);
  assert.equal(thread.values.length, count);
  const target = { WebSocket: class {} };
  const restore = mod.socket.installHohWebSocket(new mod.wire.WireServer(host.handlers()), mod.socket.socketMatcher('hoh.local'), target);
  assert.throws(() => new target.WebSocket('wss://example.com/ws'), /No network socket/);
  const local = new target.WebSocket('ws://hoh.local/ws?frameParts=1');
  await new Promise(resolve => local.addEventListener('open', resolve));
  assert.equal(local.readyState, 1);
  local.close();
  restore();
  assert.equal(typeof target.WebSocket, 'function');
});

test('installHohHost puts socket and fetch in place and starts the host', async () => {
  const engine = new FakeEngine();
  const target = { WebSocket: class {}, fetch: async () => new Response('x') };
  const { host, uninstall } = mod.index.installHohHost({ engine, target });
  const socket = new target.WebSocket('ws://hoh.local/ws');
  const client = { frames: [] };
  socket.addEventListener('message', event => client.frames.push(JSON.parse(event.data)));
  await new Promise(resolve => socket.addEventListener('open', resolve));
  socket.send(JSON.stringify({ _tag: 'Request', id: '1', tag: 'server.getConfig', payload: {}, headers: [] }));
  for (let i = 0; i < 50 && !client.frames.length; i++) await tick(5);
  assert.equal(client.frames[0].exit.value.environment.environmentId, 'hoh-local');
  assert.equal((await (await target.fetch('http://hoh.local/.well-known/t3/environment')).json()).environmentId, 'hoh-local');
  assert(host);
  uninstall();
});

test('settings: whole values are kept, partial nested patches are not; the folder of the engine becomes the project root', async () => {
  const engine = new FakeEngine();
  const { client } = connect(engine);
  const shell = await client.stream('orchestration.subscribeShell', {});
  const first = await shell.until(value => value.kind === 'snapshot');
  // No chats yet: no project either (T3's first run); the folder is offered as T3's scratch root.
  assert.deepEqual(first.snapshot.projects, []);
  assert.equal((await client.call('server.getConfig', {})).scratchWorkspaceRoot, '~/Documents/HoH');
  const settings = await client.call('server.updateSettings', { patch: { responseStreamingMode: 'turn', storageCleanup: { enabled: true }, defaultRuntimeMode: 'full-access' } });
  assert.equal(settings.responseStreamingMode, 'turn');
  assert.equal('storageCleanup' in settings, false);
  assert.equal(settings.defaultRuntimeMode, 'full-access');
  assert.equal(engine.access.mode, 'full');
  await client.call('orchestration.launchThread', {
    commandId: 'cmd-f', threadId: 'thread-f', projectId: 'hoh-project', title: 'F', modelSelection: { instanceId: 'claudeAgent', model: 'claude-sonnet' },
    runtimeMode: 'full-access', interactionMode: 'default', workspaceStrategy: { type: 'root' },
  });
  const project = await shell.until(value => value.kind === 'project.updated');
  assert.equal(project.project.workspaceRoot, '/home/person/Documents/HoH');
});

test('Stop answered only by the engine going idle still ends the turn as stopped', async () => {
  const engine = new FakeEngine();
  engine.seed('chat-i', 'I', []);
  const { client } = connect(engine);
  const thread = await client.stream('orchestration.subscribeThread', { threadId: 'chat-i' });
  await thread.until(value => value.kind === 'snapshot');
  await client.call('orchestration.dispatchCommand', { type: 'message.dispatch', commandId: 'c1', threadId: 'chat-i', messageId: 'm-i', text: 'долго', attachments: [], createdBy: 'user', creationSource: 'web', dispatchMode: { type: 'start_immediately' } });
  await thread.until(value => value.kind === 'snapshot' && value.projection.runs[0]?.status === 'running');
  await client.call('orchestration.dispatchCommand', { type: 'run.interrupt', commandId: 'c2', threadId: 'chat-i', runId: 'run-m-i' });
  engine.emitStatus('chat-i', 'idle');
  await thread.until(value => value.kind === 'snapshot' && value.projection.runs[0]?.status === 'interrupted');
});

test('a new thread never widens access by itself: T3\'s own default "full-access" is not applied, a narrower mode is', async () => {
  const engine = new FakeEngine();
  const { client } = connect(engine);
  const launched = await client.call('orchestration.launchThread', {
    commandId: 'cmd-w', threadId: 'thread-w', projectId: 'hoh-project', title: 'W', modelSelection: { instanceId: 'claudeAgent', model: 'claude-sonnet' },
    runtimeMode: 'full-access', interactionMode: 'default', workspaceStrategy: { type: 'root' },
    initialMessage: { messageId: 'm-w', text: 'hi', attachments: [] },
  });
  assert.equal(engine.requests.some(r => r.method === 'hoh/executor/access/set'), false);
  assert.equal(launched.projection.thread.runtimeMode, 'approval-required', 'the thread shows the mode actually in effect');
  // The switch is explicit: it widens.
  await client.call('orchestration.dispatchCommand', { type: 'thread.runtime-mode.set', commandId: 'c1', threadId: 'thread-w', runtimeMode: 'full-access' });
  assert.equal(engine.access.mode, 'full');
  // A thread created with a narrower mode narrows (safe direction).
  await client.call('orchestration.dispatchCommand', {
    type: 'thread.create', commandId: 'c2', threadId: 'thread-n', projectId: 'hoh-project', title: 'N', createdBy: 'user', creationSource: 'web',
    modelSelection: { instanceId: 'claudeAgent', model: 'claude-sonnet' }, runtimeMode: 'approval-required', interactionMode: 'default', branch: null, worktreePath: null,
  });
  assert.equal(engine.access.mode, 'confirm');
});
