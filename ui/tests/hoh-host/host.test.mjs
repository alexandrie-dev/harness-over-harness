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

test('permission request of Claude: shown in the chat as T3 shows Claude\'s (kind and text from the tool call), the answer goes to the engine', async () => {
  const engine = new FakeEngine();
  engine.seed('chat-p', 'P', [
    ['turn/start', { turn: 1 }], ['user/message', { id: 'u1', source: { kind: 'user' }, content: [{ type: 'text', text: 'удали tmp' }] }],
  ]);
  const { client } = connect(engine);
  const thread = await client.stream('orchestration.subscribeThread', { threadId: 'chat-p' });
  await thread.until(value => value.kind === 'snapshot');
  engine.emitStatus('chat-p', 'running');
  engine.emitEvent('chat-p', 'tool/call', { turn: 1, step: 1, callId: 'call-1', name: 'bash', arguments: JSON.stringify({ command: 'rm -rf tmp', workdir: '/home/person' }) });
  engine.askApproval('chat-p', 'appr-1', 'Claude хочет выполнить: rm -rf tmp');
  const asked = await thread.until(value => value.kind === 'snapshot' && value.projection.runtimeRequests.length === 1);
  const request = asked.projection.runtimeRequests[0];
  // T3: a Bash request is a "command" request whose text is the command itself (ClaudeAdapterV2 canUseTool).
  assert.deepEqual([request.id, request.kind, request.status, request.responseCapability.type], ['appr-1', 'command', 'pending', 'live']);
  const item = itemsOf(asked).find(entry => entry.type === 'approval_request');
  assert.deepEqual([item.requestId, item.requestKind, item.prompt], ['appr-1', 'command', 'rm -rf tmp']);
  assert.deepEqual(item.options.map(option => option.label), ['Decline', 'Approve']);
  assert.doesNotMatch(JSON.stringify(item), /[Ѐ-ӿ]/, 'the engine\'s own (Russian) words are not shown in T3');
  assert.equal(asked.projection.runs[0].status, 'waiting');
  await client.call('orchestration.dispatchCommand', { type: 'runtime-request.respond', commandId: 'c1', threadId: 'chat-p', requestId: 'appr-1', decision: 'accept' });
  assert.deepEqual(engine.decisions, [{ approvalId: 'appr-1', outcome: 'allowed-once' }]);
  const answered = await thread.until(value => value.kind === 'snapshot' && value.projection.runtimeRequests[0]?.status === 'resolved');
  assert.equal(answered.projection.runtimeRequests[0].decision, 'accept');
  // The engine's own resolution afterwards changes nothing.
  engine.notify('approval.resolved', { approvalId: 'appr-1', outcome: 'allowed-once' });
  // A file write: the engine names the call (`approval/asked`), T3 asks for a file change "Write: <file>".
  engine.emitEvent('chat-p', 'tool/call', { turn: 1, step: 2, callId: 'call-2', name: 'write', arguments: JSON.stringify({ file_path: '/home/person/notes.txt', content: 'x' }) });
  engine.askApproval('chat-p', 'appr-2', 'Ещё раз');
  engine.emitEvent('chat-p', 'approval/asked', { id: 'appr-2', toolName: 'write', callId: 'call-2' });
  const write = await thread.until(value => value.kind === 'snapshot' && value.projection.runtimeRequests.length === 2);
  assert.equal(write.projection.runtimeRequests[1].kind, 'file-change');
  assert.equal(itemsOf(write).find(entry => entry.requestId === 'appr-2').prompt, 'Write: /home/person/notes.txt');
  // A refusal maps to rejected.
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

test('access mode: T3\'s switch goes to the engine at once and every thread shows it; the default setting stays where new threads start', async () => {
  const engine = new FakeEngine();
  engine.seed('chat-m', 'M', []);
  const { client } = connect(engine);
  const config = await client.stream('subscribeServerConfig', {});
  await config.until(value => value.type === 'snapshot');
  await client.call('orchestration.dispatchCommand', { type: 'thread.runtime-mode.set', commandId: 'c1', threadId: 'chat-m', runtimeMode: 'full-access' });
  assert.deepEqual(engine.requests.filter(r => r.method === 'hoh/executor/access/set').map(r => r.params), [{ executor: 'claude', mode: 'full' }]);
  assert.equal((await client.call('orchestration.getThreadProjection', { threadId: 'chat-m' })).thread.runtimeMode, 'full-access');
  // T3 shows the mode chip under the composer of a started thread while its mode differs from this setting.
  assert.equal((await client.call('server.getSettings', {})).defaultRuntimeMode, 'approval-required');
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
  // The default is where new threads start: the engine's mode changes with the thread that starts in it.
  assert.equal(settings.defaultRuntimeMode, 'full-access');
  assert.equal(engine.access.mode, 'confirm');
  await client.call('orchestration.launchThread', {
    commandId: 'cmd-f', threadId: 'thread-f', projectId: 'hoh-project', title: 'F', modelSelection: { instanceId: 'claudeAgent', model: 'claude-sonnet' },
    runtimeMode: 'full-access', interactionMode: 'default', workspaceStrategy: { type: 'root' },
  });
  const project = await shell.until(value => value.kind === 'project.updated');
  assert.equal(project.project.workspaceRoot, '/home/person/Documents/HoH');
  assert.equal(engine.access.mode, 'full');
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

test('the mode chosen for a new thread is in effect for its first message: access/set before session/prompt', async () => {
  const engine = new FakeEngine();
  const { client } = connect(engine);
  // T3 sends the composer's choice with the launch of a new thread (useComposerSend.ts `createThread.runtimeMode`).
  const launched = await client.call('orchestration.launchThread', {
    commandId: 'cmd-w', threadId: 'thread-w', projectId: 'hoh-project', title: 'W', modelSelection: { instanceId: 'claudeAgent', model: 'claude-sonnet' },
    runtimeMode: 'full-access', interactionMode: 'default', workspaceStrategy: { type: 'root' },
    initialMessage: { messageId: 'm-w', text: 'hi', attachments: [] },
  });
  const order = engine.requests.map(r => r.method);
  assert.deepEqual(engine.requests.filter(r => r.method === 'hoh/executor/access/set').map(r => r.params), [{ executor: 'claude', mode: 'full' }]);
  assert(order.indexOf('hoh/executor/access/set') < order.indexOf('session/prompt'), 'the engine has the mode before Claude starts');
  assert.equal(launched.projection.thread.runtimeMode, 'full-access');
  // A started thread: T3 sends the switch right before the next message (ChatView `persistThreadSettingsForNextTurn`).
  engine.requests.length = 0;
  await client.call('orchestration.dispatchCommand', { type: 'thread.runtime-mode.set', commandId: 'c1', threadId: 'thread-w', runtimeMode: 'approval-required' });
  await client.call('orchestration.dispatchCommand', { type: 'message.dispatch', commandId: 'c2', threadId: 'thread-w', messageId: 'm-2', text: 'again', attachments: [], createdBy: 'user', creationSource: 'web', dispatchMode: { type: 'start_immediately' } });
  assert.deepEqual(engine.requests.filter(r => r.method === 'hoh/executor/access/set' || r.method === 'session/prompt').map(r => [r.method, r.params.mode ?? r.params.contentBlocks?.[0]?.text]),
    [['hoh/executor/access/set', 'confirm'], ['session/prompt', 'again']]);
  assert.equal(engine.access.mode, 'confirm');
  // A thread created with the default mode starts in it.
  await client.call('orchestration.dispatchCommand', { type: 'thread.runtime-mode.set', commandId: 'c3', threadId: 'thread-w', runtimeMode: 'full-access' });
  await client.call('orchestration.dispatchCommand', {
    type: 'thread.create', commandId: 'c4', threadId: 'thread-n', projectId: 'hoh-project', title: 'N', createdBy: 'user', creationSource: 'web',
    modelSelection: { instanceId: 'claudeAgent', model: 'claude-sonnet' }, runtimeMode: 'approval-required', interactionMode: 'default', branch: null, worktreePath: null,
  });
  assert.equal(engine.access.mode, 'confirm');
});

test('after a restart a finished history has no open turn, and the next answer closes its turn (the engine starts a turn before its message)', async () => {
  const store = new Map();
  const before = new FakeEngine({ store });
  const turn = (n, ask, answer) => [
    ['turn/start', { turn: n }], ['step/start', { turn: n, step: 1 }],
    ['user/message', { id: 'u' + n, source: { kind: 'user' }, content: [{ type: 'text', text: ask }] }],
    ['assistant/message', { turn: n, step: 1, message: { id: 'a' + n, content: [{ type: 'text', text: answer }] } }],
    ['step/end', { turn: n, step: 1 }], ['turn/end', { turn: n, reason: { kind: 'completed' } }],
  ];
  before.seed('chat-z', 'Z', [...turn(1, 'Запомни ZEBRA', 'Запомнил'), ...turn(2, 'Ещё', 'Готово'), ...turn(3, 'И ещё', 'Ок'), ['session/end-seed', {}]]);
  const engine = new FakeEngine({ store });
  const { client } = connect(engine);
  const thread = await client.stream('orchestration.subscribeThread', { threadId: 'chat-z' });
  const loaded = await thread.until(value => value.kind === 'snapshot' && value.projection.runs.length === 3);
  assert.deepEqual(loaded.projection.runs.map(run => run.status), ['completed', 'completed', 'completed']);
  await client.call('orchestration.dispatchCommand', { type: 'message.dispatch', commandId: 'c1', threadId: 'chat-z', messageId: 'm-4', text: 'Какое слово?', attachments: [], createdBy: 'user', creationSource: 'web', dispatchMode: { type: 'start_immediately' } });
  await thread.until(value => value.kind === 'snapshot' && value.projection.messages.filter(m => m.role === 'user').length === 4);
  engine.emitEvent('chat-z', 'assistant/message', { turn: 4, step: 1, message: { id: 'a4', content: [{ type: 'text', text: 'ZEBRA' }] } });
  engine.emitEvent('chat-z', 'turn/end', { turn: 4, reason: { kind: 'completed' } });
  const done = await thread.until(value => value.kind === 'snapshot' && value.projection.runs.length === 4 && value.projection.runs[3].status === 'completed');
  // T3 shows the thread working while ANY run is running or waiting (client-runtime threadExecution.ts `latestMatchingRun`).
  assert.deepEqual(done.projection.runs.map(run => run.status), ['completed', 'completed', 'completed', 'completed']);
  const shell = await (await client.stream('orchestration.subscribeShell', {})).until(value => value.kind === 'snapshot');
  assert.equal(shell.snapshot.threads.find(row => row.id === 'chat-z').activeRunId, null);
  assert.equal(done.projection.messages.filter(m => m.role === 'user').length, 4, 'the message is shown once');
});

test('Claude\'s models with T3\'s names, Claude Code\'s "default" left out, T3\'s default model for new chats; no Russian in what T3 shows', async () => {
  const engine = new FakeEngine({
    access: { mode: 'confirm', selected: { executor: 'claude', model: 'default' } },
    discovery: { executor: 'claude', installed: true, version: '2.1.329', versionStatus: 'supported', auth: 'authenticated', detail: null, models: [
      { id: 'default', displayName: 'По умолчанию (модель Claude Code)', isDefault: true },
      { id: 'sonnet', displayName: 'Sonnet 5.5' }, { id: 'sonnet[1m]', displayName: 'Sonnet 5.5 (1M context)' }, { id: 'opus', displayName: 'Opus 5.5' },
      { id: 'claude-haiku-4-5-20251001', displayName: 'Haiku 4.5' }, { id: 'experimental-x', displayName: 'Experimental X' },
    ] },
  });
  const { client } = connect(engine);
  const config = await client.call('server.getConfig', {});
  assert.deepEqual(config.providers[0].models.map(model => [model.slug, model.name]), [
    ['opus', 'Claude Opus 5.5'], ['sonnet', 'Claude Sonnet 5.5'], ['claude-haiku-4-5-20251001', 'Claude Haiku 4.5'], ['experimental-x', 'Experimental X'],
  ]);
  assert.deepEqual({ ...(await client.call('server.getSettings', {})).defaultModelSelection }, { instanceId: 'claudeAgent', model: 'opus' });
  assert.doesNotMatch(JSON.stringify(config), /[Ѐ-ӿ]/);
  // An engine that cannot run Claude: T3's own words, not the engine's.
  const missing = new FakeEngine({ discovery: { executor: 'claude', installed: false, version: null, auth: 'unknown', models: [], detail: 'Claude Code не установлен.' } });
  const other = connect(missing).client;
  assert.equal((await other.call('server.getConfig', {})).providers[0].message, 'Claude Agent CLI (`claude`) was not found on PATH.');
});

test('reasoning levels: T3\'s levels and default for the model, the chosen one goes to the engine as `effort` of session/prompt', async () => {
  const engine = new FakeEngine({
    access: { mode: 'confirm', selected: { executor: 'claude', model: 'opus' } },
    discovery: { executor: 'claude', installed: true, version: '2.1.329', versionStatus: 'supported', auth: 'authenticated', detail: null, models: [
      { id: 'opus', displayName: 'Opus 5.5' }, { id: 'sonnet', displayName: 'Sonnet 5.5' }, { id: 'claude-haiku-4-5', displayName: 'Haiku 4.5' }] },
  });
  const { client } = connect(engine);
  const config = await client.call('server.getConfig', {});
  const levels = slug => config.providers[0].models.find(model => model.slug === slug).capabilities?.optionDescriptors?.[0];
  assert.deepEqual(levels('opus').options.map(option => option.id), ['low', 'medium', 'high', 'xhigh', 'max']);
  assert.equal(levels('opus').options.find(option => option.isDefault).id, 'medium');
  assert.equal(levels('sonnet').options.find(option => option.isDefault).id, 'high');
  assert.equal(levels('claude-haiku-4-5'), undefined, 'T3 offers none for Haiku 4.5');
  const selection = options => ({ instanceId: 'claudeAgent', model: 'opus', options });
  await client.call('orchestration.launchThread', {
    commandId: 'c1', threadId: 't-e', projectId: 'hoh-project', title: 'E', modelSelection: selection([{ id: 'effort', value: 'xhigh' }]),
    runtimeMode: 'approval-required', interactionMode: 'default', workspaceStrategy: { type: 'root' }, initialMessage: { messageId: 'm1', text: 'one', attachments: [] },
  });
  const message = (id, text, options) => client.call('orchestration.dispatchCommand', { type: 'message.dispatch', commandId: 'c-' + id, threadId: 't-e', messageId: id, text, attachments: [], createdBy: 'user', creationSource: 'web', dispatchMode: { type: 'start_immediately' }, modelSelection: selection(options) });
  await message('m2', 'two', [{ id: 'effort', value: 'low' }]);
  await message('m3', 'three', []);
  await message('m4', 'four', [{ id: 'effort', value: 'ultrathink' }]);
  assert.deepEqual(engine.requests.filter(r => r.method === 'session/prompt').map(r => r.params.effort), ['xhigh', 'low', 'low', undefined]);
});

test('the end of a turn reaches the thread list at once, and the engine going idle closes any open run', async () => {
  const engine = new FakeEngine();
  engine.seed('chat-w', 'W', []);
  const { client } = connect(engine, { throttleMs: 600000 });
  const shell = await client.stream('orchestration.subscribeShell', {});
  await shell.until(value => value.kind === 'snapshot');
  engine.emitEvent('chat-w', 'turn/start', { turn: 1 });
  engine.emitEvent('chat-w', 'user/message', { id: 'u1', source: { kind: 'user' }, content: [{ type: 'text', text: 'go' }] });
  engine.emitEvent('chat-w', 'turn/end', { turn: 1, reason: { kind: 'completed' } });
  // No timer, no later change: the row says "completed".
  const row = await shell.until(value => value.kind === 'thread.updated' && value.thread.id === 'chat-w' && value.thread.status === 'completed');
  assert.equal(row.thread.activeRunId, null);
  // A run left open (the turn's end never came) is closed by `idle`.
  engine.emitEvent('chat-w', 'turn/start', { turn: 2 });
  engine.emitEvent('chat-w', 'user/message', { id: 'u2', source: { kind: 'user' }, content: [{ type: 'text', text: 'again' }] });
  engine.emitStatus('chat-w', 'idle');
  await shell.until(value => value.kind === 'thread.updated' && value.thread.latestRunId === 'run-msg-u2' && value.thread.status === 'completed');
});

test('reasoning level in a started chat: the switch is kept (chip), and every session/prompt carries the chat\'s level', async () => {
  const engine = new FakeEngine({
    access: { mode: 'confirm', selected: { executor: 'claude', model: 'opus' } },
    discovery: { executor: 'claude', installed: true, version: '2.1.329', versionStatus: 'supported', auth: 'authenticated', detail: null, models: [{ id: 'opus', displayName: 'Opus 5.5' }] },
  });
  const { client } = connect(engine);
  const sel = options => ({ instanceId: 'claudeAgent', model: 'opus', ...(options ? { options } : {}) });
  await client.call('orchestration.launchThread', {
    commandId: 'c1', threadId: 't-s', projectId: 'hoh-project', title: 'S', modelSelection: sel([{ id: 'effort', value: 'high' }]),
    runtimeMode: 'approval-required', interactionMode: 'default', workspaceStrategy: { type: 'root' }, initialMessage: { messageId: 'm1', text: 'one', attachments: [] },
  });
  const chip = async () => (await client.call('orchestration.getThreadProjection', { threadId: 't-s' })).thread.modelSelection.options;
  assert.deepEqual(await chip(), [{ id: 'effort', value: 'high' }], 'the first message keeps its level in the chat');
  // T3 changes level of a started chat with thread.model-selection.set (ChatView applyThreadModelPick).
  await client.call('orchestration.dispatchCommand', { type: 'thread.model-selection.set', commandId: 'c2', threadId: 't-s', modelSelection: sel([{ id: 'effort', value: 'max' }]) });
  assert.deepEqual(await chip(), [{ id: 'effort', value: 'max' }]);
  // The next messages carry no options of their own (or the same): the chat's level goes along.
  const message = (id, selection) => client.call('orchestration.dispatchCommand', { type: 'message.dispatch', commandId: 'c-' + id, threadId: 't-s', messageId: id, text: id, attachments: [], createdBy: 'user', creationSource: 'web', dispatchMode: { type: 'start_immediately' }, ...(selection ? { modelSelection: selection } : {}) });
  await message('m2'); await message('m3', sel());
  assert.deepEqual(engine.requests.filter(r => r.method === 'session/prompt').map(r => r.params.effort), ['high', 'max', 'max']);
});

test('the reasoning level of a chat survives Stop, reopening and a restart of the window; Stop closes the run for the thread list', async () => {
  const items = new Map();
  const storage = { getItem: key => items.get(key) ?? null, setItem: (key, value) => items.set(key, value) };
  const store = new Map();
  const discovery = { executor: 'claude', installed: true, version: '2.1.329', versionStatus: 'supported', auth: 'authenticated', detail: null, models: [{ id: 'opus', displayName: 'Opus 5.5' }] };
  const access = { mode: 'confirm', selected: { executor: 'claude', model: 'opus' } };
  const engine = new FakeEngine({ store, access, discovery });
  const { client } = connect(engine, { storage });
  const sel = options => ({ instanceId: 'claudeAgent', model: 'opus', ...(options ? { options } : {}) });
  const launch = id => client.call('orchestration.launchThread', { commandId: 'c-' + id, threadId: id, projectId: 'hoh-project', title: id, modelSelection: sel(), runtimeMode: 'approval-required', interactionMode: 'default', workspaceStrategy: { type: 'root' }, initialMessage: { messageId: 'm-' + id, text: id, attachments: [] } });
  const send = (c, threadId, id, selection) => c.call('orchestration.dispatchCommand', { type: 'message.dispatch', commandId: 'c-' + id, threadId, messageId: id, text: id, attachments: [], createdBy: 'user', creationSource: 'web', dispatchMode: { type: 'start_immediately' }, ...(selection ? { modelSelection: selection } : {}) });
  const shell = await client.stream('orchestration.subscribeShell', {});
  await launch('t-a'); await launch('t-b');
  await client.call('orchestration.dispatchCommand', { type: 'thread.model-selection.set', commandId: 'c1', threadId: 't-a', modelSelection: sel([{ id: 'effort', value: 'max' }]) });
  // Stop: the run closes at once, the list row stops saying "running".
  await client.call('orchestration.dispatchCommand', { type: 'thread.stop', commandId: 'c2', threadId: 't-a' });
  await shell.until(value => value.kind === 'thread.updated' && value.thread.id === 't-a' && value.thread.status === 'interrupted' && value.thread.activeRunId === null);
  // Another chat and back; T3 sends an empty option list with the next message: the level stays.
  await client.call('orchestration.getThreadProjection', { threadId: 't-b' });
  await send(client, 't-a', 'm2', sel([]));
  // A restart of the window: a new host over the same history and storage.
  const second = new FakeEngine({ store, access: engine.access, discovery });
  const again = connect(second, { storage });
  const projection = await again.client.call('orchestration.getThreadProjection', { threadId: 't-a' });
  assert.deepEqual(projection.thread.modelSelection.options, [{ id: 'effort', value: 'max' }]);
  await send(again.client, 't-a', 'm3');
  const efforts = (e, text) => e.requests.filter(r => r.method === 'session/prompt' && r.params.contentBlocks[0].text === text).map(r => r.params.effort);
  assert.deepEqual([efforts(engine, 'm2'), efforts(second, 'm3')], [['max'], ['max']]);
});

test('home screen data: working chats with real steps, a chat waiting for the person, finished ones; nothing when there is nothing', async () => {
  const engine = new FakeEngine();
  const { host } = connect(engine);
  await host.start();
  assert.deepEqual(host.home.entries(), []);
  const user = (id, text) => ({ id, source: { kind: 'user' }, content: [{ type: 'text', text }] });
  const turn = (session, n, text) => { engine.emitEvent(session, 'turn/start', { turn: n }); engine.emitEvent(session, 'user/message', user(session + n, text)); };
  for (const id of ['w1', 'w2', 'p1', 'd1', 'd2']) engine.seed(id, id, []);
  engine.emitStatus('w1', 'running');
  turn('w1', 1, 'Build report');
  engine.emitEvent('w1', 'tool/call', { turn: 1, callId: 'a', name: 'bash', arguments: { command: 'ls' } });
  engine.emitEvent('w1', 'tool/result', { turn: 1, message: { toolCallId: 'a', content: [{ type: 'text', text: 'x' }] } });
  engine.emitEvent('w1', 'tool/call', { turn: 1, callId: 'b', name: 'read', arguments: { file_path: '/a' } });
  turn('w2', 1, 'Second');
  turn('p1', 1, 'Delete tmp');
  engine.emitEvent('p1', 'tool/call', { turn: 1, callId: 'c', name: 'bash', arguments: { command: 'rm -rf tmp' } });
  engine.askApproval('p1', 'appr-h', 'ignored');
  for (const id of ['d1', 'd2']) { turn(id, 1, 'Done ' + id); engine.emitEvent(id, 'assistant/message', { turn: 1, step: 1, message: { id: 'a' + id, content: [{ type: 'text', text: 'ok' }] } }); engine.emitEvent(id, 'turn/end', { turn: 1, reason: { kind: 'completed' } }); }
  host.flush();
  const by = Object.fromEntries(host.home.entries().map(entry => [entry.id, entry]));
  assert.deepEqual([by.w1.state, by.w1.steps, by.w2.state, by.p1.state, by.d1.state, by.d2.state], ['working', { done: 1, total: 2 }, 'working', 'waiting', 'done', 'done']);
  assert.deepEqual(by.p1.request, { id: 'appr-h', kind: 'approval', requestKind: 'command', text: 'rm -rf tmp' });
  assert(by.d1.doneAt > 0 && by.w1.doneAt === null);
  // The card's buttons are the chat's own answer.
  let changes = 0; host.home.subscribe(() => { changes++; });
  await host.home.answer('p1', 'appr-h', 'accept');
  assert.deepEqual(engine.decisions, [{ approvalId: 'appr-h', outcome: 'allowed-once' }]);
  assert(changes > 0);
  assert.equal(host.home.entries().find(entry => entry.id === 'p1').request, undefined);
  // A new chat from the home screen's input.
  const id = await host.home.start('Plan the week');
  assert.equal(host.home.entries().find(entry => entry.id === id).state, 'working');
  assert.equal(host.home.projectTitle('hoh-project'), 'No project');
});

test('home screen = the chat\'s own data: the request text and kind are the chat\'s approval item, the steps are the chat\'s tool items, the answer is the chat\'s answer', async () => {
  const engine = new FakeEngine();
  engine.seed('chat-h', 'H', [['turn/start', { turn: 1 }], ['user/message', { id: 'u1', source: { kind: 'user' }, content: [{ type: 'text', text: 'Fix the notes' }] }]]);
  const { client, host } = connect(engine);
  const thread = await client.stream('orchestration.subscribeThread', { threadId: 'chat-h' });
  await thread.until(value => value.kind === 'snapshot');
  engine.emitStatus('chat-h', 'running');
  // Two finished tool calls, then a file write that waits for the person.
  for (const [n, name, args] of [[1, 'bash', { command: 'ls' }], [2, 'read', { file_path: '/home/person/a.txt' }]]) {
    engine.emitEvent('chat-h', 'tool/call', { turn: 1, step: n, callId: 'c' + n, name, arguments: JSON.stringify(args) });
    engine.emitEvent('chat-h', 'tool/result', { turn: 1, message: { toolCallId: 'c' + n, content: [{ type: 'text', text: 'ok' }] } });
  }
  engine.emitEvent('chat-h', 'tool/call', { turn: 1, step: 3, callId: 'c3', name: 'write', arguments: JSON.stringify({ file_path: '/home/person/notes.txt', content: 'x' }) });
  engine.askApproval('chat-h', 'appr-h2', 'Claude хочет записать файл');
  engine.emitEvent('chat-h', 'approval/asked', { id: 'appr-h2', toolName: 'write', callId: 'c3' });
  const snap = await thread.until(value => value.kind === 'snapshot' && value.projection.runtimeRequests.length === 1);
  host.flush();
  const entry = host.home.entries().find(candidate => candidate.id === 'chat-h');
  const item = itemsOf(snap).find(candidate => candidate.type === 'approval_request');
  // What the person is asked is what the chat shows: the same kind and text (no "step 1").
  assert.deepEqual(entry.request, { id: item.requestId, kind: 'approval', requestKind: item.requestKind, text: item.prompt });
  assert.deepEqual([entry.request.requestKind, entry.request.text], ['file-change', 'Write: /home/person/notes.txt']);
  // The strip counts the chat's tool items: three calls, two finished.
  const tools = itemsOf(snap).filter(candidate => ['command_execution', 'file_change', 'file_search', 'web_search', 'dynamic_tool'].includes(candidate.type));
  assert.equal(entry.steps.total, tools.length);
  assert.deepEqual(entry.steps, { done: tools.filter(candidate => candidate.status === 'completed').length, total: 3 });
  assert.equal(entry.state, 'waiting');
  // Approve on the home goes the same way as the chat's own button.
  await host.home.answer('chat-h', 'appr-h2', 'accept');
  assert.deepEqual(engine.decisions, [{ approvalId: 'appr-h2', outcome: 'allowed-once' }]);
  const answered = await thread.until(value => value.kind === 'snapshot' && value.projection.runtimeRequests[0]?.status === 'resolved');
  assert.equal(answered.projection.runtimeRequests[0].decision, 'accept');
});
